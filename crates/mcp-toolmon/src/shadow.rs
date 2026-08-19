//! Shadow-copy execution: resolve the child binary's canonical path, copy it
//! to a private per-instance shadow path, and spawn the shadow copy instead
//! of the canonical path. This is what keeps `mcp-toolmon` from ever holding
//! an open handle on the canonical, `cargo install`-managed binary, which
//! previously caused `cargo install --force` to fail with a Windows file
//! lock (`os error 5`) while a proxy was running.

use std::{
    collections::hash_map::DefaultHasher,
    hash::{
        Hash,
        Hasher,
    },
    io,
    path::{
        Path,
        PathBuf,
    },
};

/// Resolve `name` to an absolute canonical path.
///
/// A name containing a path separator (or an absolute path) is canonicalized
/// directly. A bare name (e.g. `log-viewer`) is looked up on `PATH`, trying
/// the platform's executable extensions (`PATHEXT`) on Windows.
pub fn resolve_canonical(name: &str) -> io::Result<PathBuf> {
    let candidate = Path::new(name);
    if candidate.components().count() > 1 {
        return std::fs::canonicalize(candidate);
    }

    let path_var = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path_var) {
        let plain = dir.join(name);
        if plain.is_file() {
            return std::fs::canonicalize(plain);
        }
        if cfg!(windows) && Path::new(name).extension().is_none() {
            let pathext = std::env::var("PATHEXT")
                .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".to_string());
            for ext in pathext.split(';') {
                if ext.is_empty() {
                    continue;
                }
                let with_ext = dir.join(format!("{name}{ext}"));
                if with_ext.is_file() {
                    return std::fs::canonicalize(with_ext);
                }
            }
        }
    }

    // Not found on PATH; fall back to canonicalizing as given (surfaces a
    // clear not-found error to the caller rather than silently mismatching).
    std::fs::canonicalize(candidate)
}

/// Base directory holding all shadow copies. `override_dir` takes precedence
/// (used by tests to avoid relying on process-global env state); otherwise
/// `TOOLMON_SHADOW_DIR` is honored, falling back to the system temp dir.
pub fn shadow_root(override_dir: Option<&Path>) -> PathBuf {
    if let Some(dir) = override_dir {
        return dir.to_path_buf();
    }
    match std::env::var_os("TOOLMON_SHADOW_DIR") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => std::env::temp_dir().join("mcp-toolmon"),
    }
}

/// Copy `canonical` into a fresh, private directory under `root`, keyed by
/// child name + this process's pid + a path hash, so concurrent proxy
/// instances (and repeated runs against the same binary) never collide.
/// Returns the path to the copied executable.
pub fn make_shadow_copy(
    canonical: &Path,
    root: &Path,
) -> io::Result<PathBuf> {
    let pid = std::process::id();
    let name = canonical
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("child");
    let mut hasher = DefaultHasher::new();
    canonical.hash(&mut hasher);
    let hash = hasher.finish();

    let dir = root.join(format!("{name}-{pid}-{hash:x}"));
    std::fs::create_dir_all(&dir)?;

    let file_name = canonical.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "canonical path has no file name",
        )
    })?;
    let dest = dir.join(file_name);
    std::fs::copy(canonical, &dest)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dest)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&dest, perms)?;
    }

    Ok(dest)
}

/// Startup sweep: delete any shadow artifact directory under `root` whose
/// owning pid (encoded in the directory name as `<name>-<pid>-<hash>`) is no
/// longer alive. No TTL is used; liveness is the only signal. Best-effort —
/// missing root, unreadable entries, and failed deletes are all silently
/// skipped, never fatal.
pub fn sweep_startup(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(pid) = extract_pid(dir_name) else {
            continue;
        };
        if !is_process_alive(pid) {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// Parse the pid out of a `<name>-<pid>-<hash>` shadow directory name. `name`
/// itself may contain hyphens, so split from the right.
fn extract_pid(dir_name: &str) -> Option<u32> {
    let parts: Vec<&str> = dir_name.rsplitn(3, '-').collect();
    if parts.len() < 3 {
        return None;
    }
    parts[1].parse::<u32>().ok()
}

#[cfg(unix)]
fn is_process_alive(pid: u32) -> bool {
    if Path::new(&format!("/proc/{pid}")).exists() {
        return true;
    }
    // Non-Linux unix (no /proc): fall back to `kill -0`. If the command
    // itself cannot be run, assume alive so we never delete a live shadow.
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(true)
}

#[cfg(windows)]
fn is_process_alive(pid: u32) -> bool {
    match std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
    {
        Ok(out) =>
            String::from_utf8_lossy(&out.stdout).contains(&pid.to_string()),
        // Command failed to run: assume alive so we never delete a live shadow.
        Err(_) => true,
    }
}

#[cfg(not(any(unix, windows)))]
fn is_process_alive(_pid: u32) -> bool {
    true
}
