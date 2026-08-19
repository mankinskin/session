use std::{
    path::Path,
    process::Command,
};

fn run_git(working_dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(working_dir)
        .args(args)
        .status()
        .expect("git command available");
    assert!(status.success(), "git {args:?} failed");
}

/// Initializes a bare-minimum git repo at `working_dir` and checks out
/// `branch`, so `infer_worktree_from_environment` can resolve a real branch
/// name via `git rev-parse` without touching a shared repo. Tolerates the
/// initial branch (`init.defaultBranch`, commonly `main`) already matching
/// the requested name.
fn init_git_repo_on_branch(working_dir: &Path, branch: &str) {
    run_git(working_dir, &["init", "-q"]);
    run_git(working_dir, &["config", "user.email", "test@example.com"]);
    run_git(working_dir, &["config", "user.name", "Test"]);
    std::fs::write(working_dir.join("README.md"), "fixture").unwrap();
    run_git(working_dir, &["add", "."]);
    run_git(working_dir, &["commit", "-q", "-m", "init"]);
    run_git(working_dir, &["checkout", "-q", "-B", branch]);
}

fn seed_ticket_worktree_inference(
    ticket_store_root: &Path,
    ticket_id: uuid::Uuid,
) {
    let store =
        ticket_api::storage::TicketStore::open_or_init(ticket_store_root)
            .unwrap();
    store
        .create(
            Some(ticket_id),
            "task",
            Some("worktree inference fixture ticket"),
            None,
            BTreeMap::new(),
            None,
            None,
        )
        .unwrap();
}

#[test]
fn infer_worktree_from_environment_resolves_existing_ticket_from_branch() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone(), "context-engine");

    let ticket_id =
        uuid::Uuid::parse_str("bbbbbbbb-2222-4222-8222-222222222222")
            .unwrap();
    seed_ticket_worktree_inference(&store_root.join(".ticket"), ticket_id);

    let repo_dir = tempdir.path().join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    init_git_repo_on_branch(&repo_dir, "agent/bbbbbbbb-some-slug");

    config
        .capture_copilot_hook(sample_payload(
            "session-infer-existing",
            None,
            sample_time(),
            &["hello"],
        ))
        .unwrap();

    config
        .infer_worktree_from_environment("session-infer-existing", &repo_dir)
        .unwrap();

    let record = config.read_session("session-infer-existing").unwrap();
    assert_eq!(
        record.metadata.ticket_id.as_deref(),
        Some(ticket_id.to_string()).as_deref()
    );
    let worktree = record.metadata.worktree.expect("worktree assignment set");
    assert_eq!(worktree.branch, "agent/bbbbbbbb-some-slug");
}

#[test]
fn infer_worktree_from_environment_leaves_ticket_id_empty_when_unresolvable()
{
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone(), "context-engine");

    let repo_dir = tempdir.path().join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    init_git_repo_on_branch(&repo_dir, "agent/deadbeef-some-slug");

    config
        .capture_copilot_hook(sample_payload(
            "session-infer-unresolvable",
            None,
            sample_time(),
            &["hello"],
        ))
        .unwrap();

    config
        .infer_worktree_from_environment(
            "session-infer-unresolvable",
            &repo_dir,
        )
        .unwrap();

    let record = config.read_session("session-infer-unresolvable").unwrap();
    assert_eq!(record.metadata.ticket_id, None);
    assert!(record.metadata.worktree.is_some());
}

#[test]
fn infer_worktree_from_environment_is_quiet_on_plain_main_branch() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone(), "context-engine");

    let repo_dir = tempdir.path().join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    init_git_repo_on_branch(&repo_dir, "main");

    config
        .capture_copilot_hook(sample_payload(
            "session-infer-main",
            None,
            sample_time(),
            &["hello"],
        ))
        .unwrap();

    config
        .infer_worktree_from_environment("session-infer-main", &repo_dir)
        .unwrap();

    let record = config.read_session("session-infer-main").unwrap();
    assert_eq!(record.metadata.ticket_id, None);
    let worktree = record.metadata.worktree.expect("worktree assignment set");
    assert_eq!(worktree.branch, "main");
}

#[test]
fn infer_worktree_from_environment_is_quiet_on_detached_head() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone(), "context-engine");

    let repo_dir = tempdir.path().join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    init_git_repo_on_branch(&repo_dir, "main");
    run_git(&repo_dir, &["checkout", "-q", "--detach"]);

    config
        .capture_copilot_hook(sample_payload(
            "session-infer-detached",
            None,
            sample_time(),
            &["hello"],
        ))
        .unwrap();

    config
        .infer_worktree_from_environment("session-infer-detached", &repo_dir)
        .unwrap();

    let record = config.read_session("session-infer-detached").unwrap();
    assert_eq!(record.metadata.ticket_id, None);
    assert!(record.metadata.worktree.is_some());
}

#[test]
fn infer_worktree_from_environment_succeeds_quietly_in_non_git_directory() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone(), "context-engine");

    let non_git_dir = tempdir.path().join("not-a-repo");
    std::fs::create_dir_all(&non_git_dir).unwrap();

    config
        .capture_copilot_hook(sample_payload(
            "session-infer-non-git",
            None,
            sample_time(),
            &["hello"],
        ))
        .unwrap();

    config
        .infer_worktree_from_environment("session-infer-non-git", &non_git_dir)
        .unwrap();

    let record = config.read_session("session-infer-non-git").unwrap();
    assert_eq!(record.metadata.ticket_id, None);
    assert_eq!(record.metadata.worktree, None);
}

#[test]
fn infer_worktree_from_environment_never_overwrites_real_check_in() {
    let tempdir = TempDir::new().unwrap();
    let store_root = tempdir.path().join("store");
    let config = SessionStoreConfig::new(store_root.clone(), "context-engine");

    let checked_in_ticket = "ticket-checked-in-by-hand";
    config
        .check_in_worktree(SessionWorktreeCheckInRequest {
            session_id: WORKTREE_SESSION_REAL.to_string(),
            owner_id: "agent-real".to_string(),
            ticket_id: checked_in_ticket.to_string(),
            worktree_path: managed_worktree(
                &tempdir,
                WORKTREE_SESSION_REAL,
                "real",
                "agent/realbeef-real-slug",
            ),
            branch: "agent/realbeef-real-slug".to_string(),
            predecessor_session_id: None,
        })
        .unwrap();

    // A different ticket that the branch shape below *would* resolve to,
    // proving hook inference does not clobber the real check-in even when
    // it disagrees.
    let other_ticket_id =
        uuid::Uuid::parse_str("cccccccc-3333-4333-8333-333333333333")
            .unwrap();
    seed_ticket_worktree_inference(
        &store_root.join(".ticket"),
        other_ticket_id,
    );

    let repo_dir = tempdir.path().join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    init_git_repo_on_branch(&repo_dir, "agent/cccccccc-other-slug");

    config
        .infer_worktree_from_environment(WORKTREE_SESSION_REAL, &repo_dir)
        .unwrap();

    let record = config.read_session(WORKTREE_SESSION_REAL).unwrap();
    assert_eq!(record.metadata.ticket_id, None);
    assert_eq!(
        record.metadata.worktree.unwrap().branch,
        "agent/realbeef-real-slug"
    );
    assert_eq!(
        config
            .lookup_worktree(WORKTREE_SESSION_REAL)
            .unwrap()
            .ticket_id,
        checked_in_ticket
    );
}
