// Test gitignore rules for session artifacts (ticket 4817a5cc AC5)

use std::{
    path::{
        Path,
        PathBuf,
    },
    process::Command,
};

// `gitignore_tracks_durable_ignores_local` was intentionally dropped during
// the session domain-crate extraction: it asserted policy about the
// *consuming repository's* ambient root .gitignore (context-engine's), not
// about session-api's own behavior, and no longer has a stable repo root to
// walk up to once this crate lives in its own repository. The remaining
// test below is self-contained: it verifies session-api itself writes a
// correct store-local `.session/.gitignore`.

#[test]
fn runtime_init_writes_an_idempotent_store_local_ignore_rule() {
    let tempdir = tempfile::TempDir::new().expect("create temp Git repository");
    let repo_root = tempdir.path();
    let session_id = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
    run_git(repo_root, ["init"]);

    let config = session_api::SessionStoreConfig::new(
        repo_root.join(".session"),
        "test-workspace",
    );
    for _ in 0..2 {
        config
            .init_runtime_context(session_api::SessionRuntimeInitRequest {
                session_id: Some(session_id.to_string()),
                predecessor_run_id: None,
                force_new_run: false,
            })
            .expect("initialize session runtime");
    }

    assert!(
        check_git_ignore(
            repo_root,
            ".session/local/worktrees/cccccccc-cccc-4ccc-8ccc-cccccccccccc.json",
        ),
        "the local registry must be ignored in a newly initialized repository"
    );
    for path in [
        ".session/sessions/cccccccc-cccc-4ccc-8ccc-cccccccccccc/session.json",
        ".session/sessions/cccccccc-cccc-4ccc-8ccc-cccccccccccc/transcript.json",
    ] {
        assert!(
            !check_git_ignore(repo_root, path),
            "durable session artifact {path} must remain trackable"
        );
    }

    let ignore = std::fs::read_to_string(repo_root.join(".session/.gitignore"))
        .expect("read store-local gitignore");
    assert_eq!(
        ignore
            .lines()
            .filter(|line| line.trim() == "local/")
            .count(),
        1,
        "repeated initialization must not duplicate the local ignore rule"
    );
}

/// Check if a path is ignored by git using `git check-ignore`.
///
/// Returns true if the path is ignored, false if it would be tracked.
fn check_git_ignore(
    repo_root: &Path,
    path: &str,
) -> bool {
    let output = Command::new("git")
        .arg("check-ignore")
        .arg("-q") // Quiet mode: exit code only
        .arg(path)
        .current_dir(repo_root)
        .output()
        .expect("Failed to run git check-ignore");

    // Exit code 0 means the path is ignored
    // Exit code 1 means the path is NOT ignored (would be tracked)
    output.status.code() == Some(0)
}

fn run_git<const N: usize>(
    repo_root: &std::path::Path,
    args: [&str; N],
) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_root)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
