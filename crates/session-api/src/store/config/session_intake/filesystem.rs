#[allow(unused_imports)]
use super::*;

pub(in super::super) fn ensure_safe_session_root(root: &Path) -> Result<(), SessionError> {
    if !root.is_absolute()
        || root
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(SessionError::InvalidStorePath(root.to_path_buf()));
    }
    let mut current = PathBuf::new();
    for component in root.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                return Err(SessionError::InvalidStorePath(current));
            }
            Ok(_) => {}
            Err(source) if source.kind() == ErrorKind::NotFound => return Ok(()),
            Err(source) => {
                return Err(SessionError::Io {
                    path: current,
                    source,
                });
            }
        }
    }
    Ok(())
}

pub(in super::super) fn ensure_real_directories(
    root: &Path,
    path: &Path,
) -> Result<(), SessionError> {
    ensure_safe_session_root(root)?;
    let relative = path
        .strip_prefix(root)
        .map_err(|_| SessionError::InvalidStorePath(path.to_path_buf()))?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                return Err(SessionError::InvalidStorePath(current));
            }
            Ok(_) => {}
            Err(source) if source.kind() == ErrorKind::NotFound => return Ok(()),
            Err(source) => {
                return Err(SessionError::Io {
                    path: current,
                    source,
                });
            }
        }
    }
    Ok(())
}

pub(in super::super) fn read_regular_file_if_exists(
    path: &Path,
) -> Result<Option<Vec<u8>>, SessionError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_file() => {}
        Ok(_) => return Err(SessionError::InvalidStorePath(path.to_path_buf())),
        Err(source) if source.kind() == ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(SessionError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    }
    fs::read(path).map(Some).map_err(|source| SessionError::Io {
        path: path.to_path_buf(),
        source,
    })
}

pub(in super::super) fn write_new_synced(path: &Path, bytes: &[u8]) -> Result<bool, SessionError> {
    use std::io::Write;
    let io_error = |target: &Path, source: std::io::Error| SessionError::Io {
        path: target.to_path_buf(),
        source,
    };
    let parent = path
        .parent()
        .ok_or_else(|| SessionError::InvalidStorePath(path.to_path_buf()))?;
    let tmp = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("intake"),
        Uuid::new_v4()
    ));
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|source| io_error(&tmp, source))?;
        file.write_all(bytes)
            .map_err(|source| io_error(&tmp, source))?;
        file.sync_all().map_err(|source| io_error(&tmp, source))?;
    }
    let linked = fs::hard_link(&tmp, path);
    let _ = fs::remove_file(&tmp);
    match linked {
        Ok(()) => {
            #[cfg(unix)]
            {
                fs::File::open(parent)
                    .and_then(|dir| dir.sync_all())
                    .map_err(|source| io_error(parent, source))?;
            }
            Ok(true)
        }
        Err(source) if source.kind() == ErrorKind::AlreadyExists => Ok(false),
        Err(source) => Err(io_error(path, source)),
    }
}
