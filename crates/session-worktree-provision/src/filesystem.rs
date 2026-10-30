#[allow(unused_imports)]
use super::*;

pub(super) fn validate_name(name: &str) -> Result<(), WorktreeGitError> {
    if name.is_empty() || Path::new(name).components().count() != 1 {
        return Err(WorktreeGitError::InvalidWorktreeName {
            name: name.to_string(),
        });
    }
    Ok(())
}

pub(super) fn validate_relative_worktree_path(path: &Path) -> Result<(), WorktreeGitError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(WorktreeGitError::InvalidWorktreeName {
            name: path.display().to_string(),
        });
    }
    Ok(())
}

pub(super) fn branch_for_worktree(path: &Path) -> Result<Option<String>, WorktreeGitError> {
    let repository = Repository::open(path)?;
    if repository.head_detached()? {
        return Ok(None);
    }
    let head = repository.head()?;
    Ok(head.shorthand().map(str::to_string))
}

pub(super) fn relocate_directory(from: &Path, to: &Path) -> Result<(), WorktreeGitError> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::CrossesDevices => {
            copy_directory_recursively(from, to).map_err(|source| {
                WorktreeGitError::MoveFallback {
                    from: from.to_path_buf(),
                    to: to.to_path_buf(),
                    source,
                }
            })?;
            fs::remove_dir_all(from).map_err(|source| WorktreeGitError::MoveFallback {
                from: from.to_path_buf(),
                to: to.to_path_buf(),
                source,
            })
        }
        Err(source) => Err(WorktreeGitError::Io {
            path: from.to_path_buf(),
            source,
        }),
    }
}

pub(super) fn copy_directory_recursively(from: &Path, to: &Path) -> Result<(), std::io::Error> {
    fs::create_dir(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let source = entry.path();
        let destination = to.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source)?;
        if metadata.is_dir() {
            copy_directory_recursively(&source, &destination)?;
        } else if metadata.is_symlink() {
            copy_symlink(&source, &destination)?;
        } else {
            fs::copy(&source, &destination)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn copy_symlink(source: &Path, destination: &Path) -> Result<(), std::io::Error> {
    std::os::unix::fs::symlink(fs::read_link(source)?, destination)
}

#[cfg(windows)]
pub(super) fn copy_symlink(source: &Path, destination: &Path) -> Result<(), std::io::Error> {
    let target = fs::read_link(source)?;
    if fs::metadata(source)?.is_dir() {
        std::os::windows::fs::symlink_dir(target, destination)
    } else {
        std::os::windows::fs::symlink_file(target, destination)
    }
}

pub(super) fn paths_equal(left: &Path, right: &Path) -> bool {
    let normalize = |path: &Path| {
        fs::canonicalize(path)
            .unwrap_or_else(|_| path.to_path_buf())
            .to_string_lossy()
            .replace('\\', "/")
            .to_lowercase()
    };
    normalize(left) == normalize(right)
}
