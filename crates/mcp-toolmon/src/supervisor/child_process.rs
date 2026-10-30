#[allow(unused_imports)]
use super::*;

pub(super) fn resolve_shadow_exe(command: &[String], root: &Path) -> (PathBuf, Option<PathBuf>) {
    match shadow::resolve_canonical(&command[0]) {
        Ok(canonical) => match shadow::make_shadow_copy(&canonical, root) {
            Ok(shadow_exe) => (shadow_exe.clone(), Some(shadow_exe)),
            Err(e) => {
                eprintln!(
                    "[mcp-toolmon] shadow copy of {} failed ({e}); falling back to spawning the canonical path directly",
                    canonical.display()
                );
                (canonical, None)
            }
        },
        Err(e) => {
            eprintln!(
                "[mcp-toolmon] canonical path resolution failed for {:?} ({e}); spawning as given",
                command[0]
            );
            (PathBuf::from(&command[0]), None)
        }
    }
}

pub(super) fn spawn_exe(
    exe: &Path,
    args: &[String],
    shadow_path: Option<PathBuf>,
) -> std::io::Result<ChildHandles> {
    let mut child = Command::new(exe)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    Ok(ChildHandles {
        child: Mutex::new(child),
        stdin: Mutex::new(Some(stdin)),
        stdout: Mutex::new(BufReader::new(stdout)),
        shadow_path,
    })
}

pub(super) fn spawn_handles(command: &[String], root: &Path) -> std::io::Result<ChildHandles> {
    let (exe, shadow_path) = resolve_shadow_exe(command, root);
    spawn_exe(&exe, &command[1..], shadow_path)
}

/// Snapshot a healthy generation's shadow exe into a dedicated path that
/// regular re-copies of the (possibly now-corrupt) canonical binary can
/// never collide with or overwrite.
///
/// `shadow::make_shadow_copy` keys its destination directory only by a hash
/// of the CANONICAL path, not by content or call count, so calling it twice
/// in a row for the same canonical path copies into the SAME destination
/// file. Left unguarded, a failed respawn attempt would overwrite the last
/// known-good shadow copy with corrupt bytes before we could fall back to
/// it, defeating R7. This snapshot is what makes the fallback durable.
pub(super) fn snapshot_last_known_good(shadow_exe: &Path, root: &Path) -> std::io::Result<PathBuf> {
    let pid = std::process::id();
    let dir = root.join(format!("lastgood-{pid}"));
    std::fs::create_dir_all(&dir)?;
    let file_name = shadow_exe.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "shadow exe has no file name",
        )
    })?;
    let dest = dir.join(file_name);
    std::fs::copy(shadow_exe, &dest)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dest)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&dest, perms)?;
    }
    Ok(dest)
}
