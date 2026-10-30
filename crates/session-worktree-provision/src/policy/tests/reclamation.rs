#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn reclaims_clean_inactive_worktree_and_preserves_marker() {
    let fixture = Fixture::new();
    fs::write(fixture.main.join(".git/info/exclude"), "marker.txt\n").unwrap();
    let git = fixture.git();
    let old = git.create_worktree("old", "agent/old", "main").unwrap();
    fs::write(old.path.join("marker.txt"), "keep\n").unwrap();
    assert!(!git.is_dirty(&old.path).unwrap());

    let outcome = provision_for_session(&git, &NeverActive, SESSION_ID, &policy(1)).unwrap();
    let reclaimed = match outcome {
        ProvisionOutcome::Reclaimed {
            worktree,
            previous_name,
        } => {
            assert_eq!(previous_name, "old");
            worktree
        }
        other => panic!("expected reclaim, got {other:?}"),
    };

    assert!(!old.path.exists());
    assert_eq!(
        fs::read_to_string(reclaimed.path.join("marker.txt")).unwrap(),
        "keep\n"
    );
    assert!(!git.branch_exists("agent/old").unwrap());
    assert!(
        git.branch_exists("agent/12345678-1234-4234-8234-123456789abc/session")
            .unwrap()
    );
}

#[test]
pub(super) fn dirty_worktree_is_not_reclaimed() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let old = git.create_worktree("old", "agent/old", "main").unwrap();
    fs::write(old.path.join("untracked.txt"), "preserve\n").unwrap();

    assert_cap_reached(provision_for_session(
        &git,
        &NeverActive,
        SESSION_ID,
        &policy(1),
    ));
    assert!(old.path.exists());
    assert!(git.branch_exists("agent/old").unwrap());
    assert!(
        !git.branch_exists("agent/12345678-1234-4234-8234-123456789abc/session")
            .unwrap()
    );
}

#[test]
pub(super) fn reclaims_worktree_after_checkpointing_its_owned_session_record() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let old = git
        .create_worktree_at(
            &PathBuf::from(OLD_SESSION_ID).join("session"),
            &format!("agent/{OLD_SESSION_ID}/session"),
            "main",
        )
        .unwrap();
    persist_worktree_owner(&fixture.main.join(".session"), OLD_SESSION_ID, &old);
    persist_worktree_owner(&old.path.join(".session"), OLD_SESSION_ID, &old);

    let activity = SessionStoreActivity::with_default_staleness(fixture.main.join(".session"));
    let eligibility = evaluate_reclaim_candidate(&git, &activity, &old, &policy(1)).unwrap();
    assert_eq!(eligibility, ReclaimEligibility::Reclaimable);
    assert_eq!(git.ahead_behind(&old.path, "main").unwrap().0, 1);
    assert!(
        git.is_owned_session_checkpoint(&old.path, OLD_SESSION_ID)
            .unwrap()
    );

    let outcome = provision_for_session(&git, &activity, SESSION_ID, &policy(1)).unwrap();

    assert!(matches!(outcome, ProvisionOutcome::Reclaimed { .. }));
    assert!(
        !git.is_dirty(
            &fixture
                .main
                .join(".worktrees")
                .join(SESSION_ID)
                .join("session")
        )
        .unwrap()
    );
}

#[test]
pub(super) fn ahead_worktree_is_not_reclaimed() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let old = git.create_worktree("old", "agent/old", "main").unwrap();
    fs::write(old.path.join("tracked.txt"), "advance\n").unwrap();
    commit(&old.path, "advance");
    assert_eq!(git.ahead_behind(&old.path, "main").unwrap().0, 1);

    assert_cap_reached(provision_for_session(
        &git,
        &NeverActive,
        SESSION_ID,
        &policy(1),
    ));
    assert!(old.path.exists());
    assert!(git.branch_exists("agent/old").unwrap());
    assert!(!git.branch_exists("agent/12345678-session").unwrap());
}

#[test]
pub(super) fn active_worktree_is_not_reclaimed() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let old = git.create_worktree("old", "agent/old", "main").unwrap();
    let activity = ActiveWorktree(old.path.clone());

    assert_cap_reached(provision_for_session(
        &git,
        &activity,
        SESSION_ID,
        &policy(1),
    ));
    assert!(old.path.exists());
    assert!(git.branch_exists("agent/old").unwrap());
    assert!(!git.branch_exists("agent/12345678-session").unwrap());
}

#[test]
pub(super) fn cap_without_reclaim_candidate_returns_cap_reached() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let old = git.create_worktree("old", "agent/old", "main").unwrap();
    fs::write(old.path.join("untracked.txt"), "preserve\n").unwrap();

    let result = provision_for_session(&git, &NeverActive, SESSION_ID, &policy(1));
    assert_cap_reached(result);
    assert_eq!(git.list_worktrees().unwrap().len(), 1);
    assert!(
        !fixture
            .main
            .join(".worktrees/12345678-1234-4234-8234-123456789abc/session")
            .exists()
    );
}

#[test]
pub(super) fn cap_with_reclaim_candidate_reclaims_instead() {
    let fixture = Fixture::new();
    let git = fixture.git();
    git.create_worktree("old", "agent/old", "main").unwrap();

    let outcome = provision_for_session(&git, &NeverActive, SESSION_ID, &policy(1)).unwrap();
    assert!(matches!(outcome, ProvisionOutcome::Reclaimed { .. }));
    assert_eq!(git.list_worktrees().unwrap().len(), 1);
}

#[test]
pub(super) fn recently_active_worktree_is_not_reclaimed() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let old = git.create_worktree("old", "agent/old", "main").unwrap();
    let mut policy = policy(1);
    policy.idle_before_reclaim = Duration::from_secs(60 * 60);

    assert_cap_reached(provision_for_session(
        &git,
        &NeverActive,
        SESSION_ID,
        &policy,
    ));
    assert!(old.path.exists());
    assert!(
        !fixture
            .main
            .join(".worktrees/12345678-1234-4234-8234-123456789abc/session")
            .exists()
    );
}

#[test]
pub(super) fn idle_worktree_is_reclaimed_after_activity_signals_are_backdated() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let old = git.create_worktree("old", "agent/old", "main").unwrap();
    let mut policy = policy(1);
    policy.idle_before_reclaim = Duration::from_secs(60 * 60);
    backdate_activity_signals(&git, &old, Duration::from_secs(2 * 60 * 60));

    let outcome = provision_for_session(&git, &NeverActive, SESSION_ID, &policy).unwrap();
    assert!(matches!(outcome, ProvisionOutcome::Reclaimed { .. }));
}

#[test]
pub(super) fn worktree_containing_the_current_directory_is_not_reclaimed() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let old = git.create_worktree("old", "agent/old", "main").unwrap();
    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(&old.path).unwrap();

    let result = provision_for_session(&git, &NeverActive, SESSION_ID, &policy(1));

    std::env::set_current_dir(original).unwrap();
    assert_cap_reached(result);
    assert!(old.path.exists());
}

#[test]
pub(super) fn failed_reclaim_falls_through_to_fresh_creation() {
    let fixture = Fixture::new();
    fixture.add_submodule();
    let git = fixture.git();
    let old = git.create_worktree("old", "agent/old", "main").unwrap();
    fs::remove_dir_all(fixture.main.join("nested")).unwrap();

    let outcome = provision_for_session(&git, &NeverActive, SESSION_ID, &policy(2)).unwrap();
    assert!(matches!(outcome, ProvisionOutcome::Created(_)));
    assert!(old.path.exists());
    assert!(
        fixture
            .main
            .join(".worktrees/12345678-1234-4234-8234-123456789abc/session")
            .exists()
    );
}

#[test]
pub(super) fn failed_reclaim_at_cap_returns_cap_reached_without_creating() {
    let fixture = Fixture::new();
    fixture.add_submodule();
    let git = fixture.git();
    let old = git.create_worktree("old", "agent/old", "main").unwrap();
    fs::remove_dir_all(fixture.main.join("nested")).unwrap();

    assert_cap_reached(provision_for_session(
        &git,
        &NeverActive,
        SESSION_ID,
        &policy(1),
    ));
    assert!(old.path.exists());
    assert!(
        !fixture
            .main
            .join(".worktrees/12345678-1234-4234-8234-123456789abc/session")
            .exists()
    );
}
