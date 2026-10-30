use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime},
};

use filetime::{FileTime, set_file_mtime};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::{
    NeverActive, ProvisionError, ProvisionOutcome, ProvisionPolicy, ReclaimEligibility,
    ReclaimRejectionReason, SessionActivity, SessionStoreActivity, evaluate_reclaim_candidate,
    provision_for_session, reclaim_candidates,
};
use crate::{WorktreeRef, tests::Fixture};

const SESSION_ID: &str = "12345678-1234-4234-8234-123456789abc";
const OLD_SESSION_ID: &str = "abcdefab-cdef-4def-8def-abcdefabcdef";
const SAME_PREFIX_SESSION_ID: &str = "12345678-5678-4678-9678-123456789abc";

struct ActiveWorktree(PathBuf);

impl SessionActivity for ActiveWorktree {
    fn is_active(&self, worktree: &Path) -> bool {
        worktree == self.0
    }
}

fn policy(max_worktrees: usize) -> ProvisionPolicy {
    ProvisionPolicy {
        max_worktrees,
        stale_after: Duration::from_secs(60),
        idle_before_reclaim: Duration::ZERO,
        base_ref: "main".to_string(),
    }
}

fn backdate_activity_signals(git: &crate::WorktreeGit, worktree: &WorktreeRef, age: Duration) {
    let timestamp = FileTime::from_system_time(SystemTime::now() - age);
    let admin = git
        .main_checkout()
        .join(".git/worktrees")
        .join(&worktree.name);
    for path in [
        worktree.path.as_path(),
        admin.as_path(),
        admin.join("index").as_path(),
    ] {
        set_file_mtime(path, timestamp).unwrap();
    }
}

fn commit(directory: &Path, message: &str) {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args([
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-am",
            message,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_cap_reached(result: Result<ProvisionOutcome, ProvisionError>) {
    assert!(matches!(result, Err(ProvisionError::CapReached { .. })));
}

fn persist_worktree_owner(session_store: &Path, session_id: &str, worktree: &WorktreeRef) {
    let record = session_store
        .join("sessions")
        .join(session_id)
        .join("session.json");
    fs::create_dir_all(record.parent().unwrap()).unwrap();
    fs::write(
        record,
        serde_json::json!({
            "session_id": session_id,
            "metadata": {
                "worktree": {
                    "path": worktree.path,
                },
            },
        })
        .to_string(),
    )
    .unwrap();
}

fn persist_active_worktree_session(session_store: &Path, session_id: &str, worktree: &WorktreeRef) {
    let record = session_store
        .join("sessions")
        .join(session_id)
        .join("session.json");
    fs::create_dir_all(record.parent().unwrap()).unwrap();
    fs::write(
        record,
        serde_json::json!({
            "session_id": session_id,
            "captured_at": OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .unwrap(),
            "metadata": {
                "worktree": {
                    "path": worktree.path,
                },
            },
        })
        .to_string(),
    )
    .unwrap();
}

fn only_worktree(git: &crate::WorktreeGit) -> WorktreeRef {
    let mut worktrees = git.list_worktrees().unwrap();
    assert_eq!(worktrees.len(), 1);
    worktrees.remove(0)
}

#[test]
fn second_call_for_session_is_idempotent() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let policy = policy(8);
    let first = provision_for_session(&git, &NeverActive, SESSION_ID, &policy).unwrap();
    let expected = match first {
        ProvisionOutcome::Created(worktree) => worktree,
        other => panic!("expected creation, got {other:?}"),
    };

    let second = provision_for_session(&git, &NeverActive, SESSION_ID, &policy).unwrap();
    assert!(matches!(
        second,
        ProvisionOutcome::AlreadyProvisioned(worktree) if worktree == expected
    ));
    assert_eq!(git.list_worktrees().unwrap().len(), 1);
}

#[test]
fn session_reuses_its_own_persisted_worktree_assignment() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let session_store = fixture.main.join(".session");
    let worktree = match provision_for_session(&git, &NeverActive, SESSION_ID, &policy(8)).unwrap()
    {
        ProvisionOutcome::Created(worktree) => worktree,
        other => panic!("expected creation, got {other:?}"),
    };
    persist_worktree_owner(&session_store, SESSION_ID, &worktree);

    let fresh_activity = SessionStoreActivity::with_default_staleness(&session_store);
    let reuse = provision_for_session(&git, &fresh_activity, SESSION_ID, &policy(8)).unwrap();

    assert!(matches!(
        reuse,
        ProvisionOutcome::AlreadyProvisioned(reused) if reused == worktree
    ));
}

#[test]
fn sessions_sharing_prefix_do_not_receive_the_same_worktree() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let session_store = fixture.main.join(".session");
    let worktree = git
        .create_worktree("12345678-session", "agent/12345678-session", "main")
        .unwrap();
    persist_worktree_owner(&session_store, SESSION_ID, &worktree);

    let fresh_activity = SessionStoreActivity::with_default_staleness(&session_store);
    let result = provision_for_session(&git, &fresh_activity, SAME_PREFIX_SESSION_ID, &policy(8));

    assert!(matches!(
        result,
        Err(ProvisionError::SessionOwnershipConflict {
            session_id,
            owner_session_id,
            ..
        }) if session_id == SAME_PREFIX_SESSION_ID
            && owner_session_id == SESSION_ID
    ));
    assert_eq!(git.list_worktrees().unwrap(), vec![worktree]);
}

#[test]
fn foreign_owned_prefix_candidate_returns_ownership_conflict() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let session_store = fixture.main.join(".session");
    let worktree = git
        .create_worktree("12345678-session", "agent/12345678-session", "main")
        .unwrap();
    persist_worktree_owner(&session_store, SESSION_ID, &worktree);

    let fresh_activity = SessionStoreActivity::with_default_staleness(&session_store);
    let result = provision_for_session(&git, &fresh_activity, SAME_PREFIX_SESSION_ID, &policy(8));

    assert!(matches!(
        result,
        Err(ProvisionError::SessionOwnershipConflict { worktree: path, .. })
            if path == worktree.path
    ));
}

#[test]
fn unowned_legacy_prefix_candidate_remains_claimable() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let worktree = git
        .create_worktree("12345678-session", "agent/12345678-session", "main")
        .unwrap();
    let session_store = fixture.main.join(".session");
    let fresh_activity = SessionStoreActivity::with_default_staleness(&session_store);

    let reuse =
        provision_for_session(&git, &fresh_activity, SAME_PREFIX_SESSION_ID, &policy(8)).unwrap();

    assert!(matches!(
        reuse,
        ProvisionOutcome::AlreadyProvisioned(reused) if reused == worktree
    ));
}

#[test]
fn creates_named_worktree_when_no_candidate_exists() {
    let fixture = Fixture::new();
    let outcome =
        provision_for_session(&fixture.git(), &NeverActive, SESSION_ID, &policy(8)).unwrap();

    assert!(matches!(
        outcome,
        ProvisionOutcome::Created(worktree)
            if worktree.name == "session"
                && worktree
                    .path
                    .ends_with(Path::new(SESSION_ID).join("session"))
                && worktree.path.exists()
                && worktree.branch.as_deref()
                    == Some("agent/12345678-1234-4234-8234-123456789abc/session")
    ));
}

#[test]
fn discovery_reuses_legacy_flat_worktree() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let legacy = git
        .create_worktree("12345678-legacy", "agent/12345678-legacy", "main")
        .unwrap();

    let outcome = provision_for_session(&git, &NeverActive, SESSION_ID, &policy(8)).unwrap();

    assert!(matches!(
        outcome,
        ProvisionOutcome::AlreadyProvisioned(worktree) if worktree == legacy
    ));
}

#[test]
fn discovery_reuses_nested_worktree_for_exact_session_id() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let nested_path = PathBuf::from(SESSION_ID).join("exact-match");
    let nested = git
        .create_worktree_at(
            &nested_path,
            "agent/12345678-1234-4234-8234-123456789abc/exact-match",
            "main",
        )
        .unwrap();

    let outcome = provision_for_session(&git, &NeverActive, SESSION_ID, &policy(8)).unwrap();

    assert!(matches!(
        outcome,
        ProvisionOutcome::AlreadyProvisioned(worktree) if worktree == nested
    ));
}

#[test]
fn multiple_nested_slugs_return_sorted_ambiguity_error() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let alpha = git
        .create_worktree_at(
            &PathBuf::from(SESSION_ID).join("alpha"),
            "agent/12345678-1234-4234-8234-123456789abc/alpha",
            "main",
        )
        .unwrap();
    let beta = git
        .create_worktree_at(
            &PathBuf::from(SESSION_ID).join("beta"),
            "agent/12345678-1234-4234-8234-123456789abc/beta",
            "main",
        )
        .unwrap();

    let result = provision_for_session(&git, &NeverActive, SESSION_ID, &policy(8));

    assert!(matches!(
        result,
        Err(ProvisionError::AmbiguousSessionWorktree { candidates, .. })
            if candidates == vec![alpha.path, beta.path]
    ));
}

#[test]
fn session_store_activity_honors_fresh_and_stale_records() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let worktree = git.create_worktree("old", "agent/old", "main").unwrap();
    let record = fixture.main.join(".session/sessions/session/session.json");
    fs::create_dir_all(record.parent().unwrap()).unwrap();
    let fresh = OffsetDateTime::now_utc().format(&Rfc3339).unwrap();
    let worktree_path = worktree.path.to_string_lossy().replace('\\', "/");
    fs::write(
        &record,
        format!(
            r#"{{"captured_at":"{fresh}","metadata":{{"worktree":{{"path":"{}"}}}}}}"#,
            worktree_path
        ),
    )
    .unwrap();

    let activity =
        SessionStoreActivity::new(fixture.main.join(".session"), Duration::from_secs(60));
    assert!(activity.is_active(&worktree.path));

    fs::write(
        &record,
        format!(
            r#"{{"captured_at":"2000-01-01T00:00:00Z","metadata":{{"worktree":{{"path":"{}"}}}}}}"#,
            worktree_path
        ),
    )
    .unwrap();
    assert!(!activity.is_active(&worktree.path));
}

#[test]
fn session_store_activity_prevents_reclaim_for_nested_worktree_record() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let worktree = git
        .create_worktree_at(
            &PathBuf::from("other-session").join("active"),
            "agent/other-session/active",
            "main",
        )
        .unwrap();
    let record = worktree.path.join(".session/sessions/active/session.json");
    fs::create_dir_all(record.parent().unwrap()).unwrap();
    let fresh = OffsetDateTime::now_utc().format(&Rfc3339).unwrap();
    let worktree_path = worktree.path.to_string_lossy().replace('\\', "/");
    fs::write(
        record,
        format!(
            r#"{{"captured_at":"{fresh}","metadata":{{"worktree":{{"path":"{}"}}}}}}"#,
            worktree_path
        ),
    )
    .unwrap();

    let activity =
        SessionStoreActivity::new(fixture.main.join(".session"), Duration::from_secs(60));
    let eligibility =
        evaluate_reclaim_candidate(&git, &activity, &only_worktree(&git), &policy(1)).unwrap();

    assert_eq!(
        eligibility,
        ReclaimEligibility::Rejected(ReclaimRejectionReason::SessionActive)
    );
}

#[path = "tests/reclaim_evaluation.rs"]
mod reclaim_evaluation;
#[allow(unused_imports)]
pub use reclaim_evaluation::*;

#[path = "tests/reclamation.rs"]
mod reclamation;
#[allow(unused_imports)]
pub use reclamation::*;
