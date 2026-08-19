//! Shadow-copy unit tests (T3): canonical-path resolution, shadow copy
//! creation, `TOOLMON_SHADOW_DIR` override, and startup sweep liveness
//! cleanup. See spec `1bef7b3d-…` Validation Strategy for the fixture design.

use std::path::PathBuf;

use mcp_toolmon::supervisor::Supervisor;
use tempfile::TempDir;

fn fake_v1_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake-mcp-v1"))
}

fn fake_v2_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake-mcp-v2"))
}

#[tokio::test]
async fn shadow_copy_spawns_from_shadow_path() {
    let shadow_root = TempDir::new().unwrap();
    let canonical = fake_v1_path();
    let command = vec![canonical.to_string_lossy().to_string()];

    let supervisor =
        Supervisor::spawn_with_shadow_dir(&command, Some(shadow_root.path()))
            .unwrap();

    let shadow_path = supervisor
        .shadow_path()
        .expect("shadow copy should have been created")
        .to_path_buf();

    assert_ne!(
        shadow_path, canonical,
        "spawned exe path must differ from canonical path P"
    );
    assert!(shadow_path.starts_with(shadow_root.path()));
    assert!(
        shadow_path.is_file(),
        "shadow copy must actually exist on disk at {shadow_path:?}"
    );

    let _ = supervisor.shutdown().await;
}

#[tokio::test]
async fn shadow_dir_env_override() {
    let shadow_root = TempDir::new().unwrap();
    // SAFETY: this test is the only one that reads TOOLMON_SHADOW_DIR (all
    // other shadow tests pass an explicit override to `spawn_with_shadow_dir`
    // and never consult the env var), so mutating it here cannot race other
    // tests in this binary.
    unsafe {
        std::env::set_var("TOOLMON_SHADOW_DIR", shadow_root.path());
    }

    let canonical = fake_v1_path();
    let command = vec![canonical.to_string_lossy().to_string()];
    let supervisor = Supervisor::spawn_with_shadow_dir(&command, None).unwrap();

    let shadow_path = supervisor
        .shadow_path()
        .expect("shadow copy should have been created")
        .to_path_buf();

    unsafe {
        std::env::remove_var("TOOLMON_SHADOW_DIR");
    }

    assert!(
        shadow_path.starts_with(shadow_root.path()),
        "shadow path {shadow_path:?} should be under TOOLMON_SHADOW_DIR {:?}",
        shadow_root.path()
    );

    let _ = supervisor.shutdown().await;
}

#[test]
fn startup_sweep_removes_dead_shadow() {
    let root = TempDir::new().unwrap();

    // Dead pid: spawn a trivial child and wait for it to exit, guaranteeing
    // its pid is no longer alive.
    let mut dead_child = std::process::Command::new(fake_v1_path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    drop(dead_child.stdin.take()); // close stdin -> fixture server exits (EOF)
    let dead_pid = dead_child.id();
    dead_child.wait().unwrap();

    let dead_dir = root.path().join(format!("fake-mcp-v1-{dead_pid}-abc123"));
    std::fs::create_dir_all(&dead_dir).unwrap();
    std::fs::write(dead_dir.join("fake-mcp-v1"), b"dead").unwrap();

    // Alive pid: this very test process.
    let alive_pid = std::process::id();
    let alive_dir = root.path().join(format!("fake-mcp-v1-{alive_pid}-def456"));
    std::fs::create_dir_all(&alive_dir).unwrap();
    std::fs::write(alive_dir.join("fake-mcp-v1"), b"alive").unwrap();

    mcp_toolmon::shadow::sweep_startup(root.path());

    assert!(
        !dead_dir.exists(),
        "shadow dir owned by a dead pid must be removed"
    );
    assert!(
        alive_dir.exists(),
        "shadow dir owned by a live pid must be retained"
    );
}

#[cfg(windows)]
#[tokio::test]
async fn windows_lock_freedom() {
    // Reproduces the incident this epic fixes: previously, spawning the
    // child directly from its canonical path P held a Windows mandatory file
    // lock on P for the child's lifetime, so overwriting/renaming P (as
    // `cargo install --force` does) failed with `os error 5`
    // ("Zugriff verweigert" / access denied).
    let shadow_root = TempDir::new().unwrap();
    let canonical_dir = TempDir::new().unwrap();
    let canonical = canonical_dir.path().join("fake-mcp.exe");
    std::fs::copy(fake_v1_path(), &canonical).unwrap();

    let command = vec![canonical.to_string_lossy().to_string()];
    let supervisor =
        Supervisor::spawn_with_shadow_dir(&command, Some(shadow_root.path()))
            .unwrap();

    assert!(
        supervisor.shadow_path().is_some(),
        "child must be running from a shadow copy, not P, for this test to be meaningful"
    );

    // While the child (spawned from the shadow copy) is running, overwrite
    // the canonical path P from this process. This must succeed because
    // mcp-toolmon never holds P open.
    let v2_bytes = std::fs::read(fake_v2_path()).unwrap();
    let overwrite = std::fs::write(&canonical, &v2_bytes);
    assert!(
        overwrite.is_ok(),
        "overwriting canonical path P while the shadow-spawned child runs must succeed, got: {overwrite:?}"
    );

    // The proxy must still be alive/serving after the overwrite: it accepts
    // a write on its stdin pipe to the still-running shadow-spawned child.
    let still_writable = supervisor.write_line("{}").await;
    assert!(
        still_writable,
        "supervisor should remain writable after P was overwritten"
    );

    let _ = supervisor.shutdown().await;
}
