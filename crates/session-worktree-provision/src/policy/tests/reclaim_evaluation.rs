#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn evaluate_reclaim_candidate_reports_outside_worktree_root() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let candidate = WorktreeRef {
        name: "outside".to_string(),
        path: fixture.main.clone(),
        branch: Some("main".to_string()),
    };

    let eligibility =
        evaluate_reclaim_candidate(&git, &NeverActive, &candidate, &policy(1)).unwrap();
    assert_eq!(
        eligibility,
        ReclaimEligibility::Rejected(ReclaimRejectionReason::OutsideWorktreeRoot)
    );
}

#[test]
pub(super) fn evaluate_reclaim_candidate_reports_session_active() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let worktree = git.create_worktree("old", "agent/old", "main").unwrap();
    let activity = ActiveWorktree(worktree.path.clone());

    let eligibility =
        evaluate_reclaim_candidate(&git, &activity, &only_worktree(&git), &policy(1)).unwrap();
    assert_eq!(
        eligibility,
        ReclaimEligibility::Rejected(ReclaimRejectionReason::SessionActive)
    );
}

#[test]
pub(super) fn federated_activity_and_unreadable_store_refuse_reclaim() {
    let active_fixture = Fixture::new();
    let active_git = active_fixture.git();
    let active_worktree = active_git
        .create_worktree("old", "agent/old", "main")
        .unwrap();
    let federated_store = active_fixture
        .main
        .join(".worktrees")
        .join("activity-session")
        .join("nested")
        .join(".session");
    persist_active_worktree_session(
        &federated_store,
        "session-visible-only-through-federation",
        &active_worktree,
    );
    let active_eligibility = evaluate_reclaim_candidate(
        &active_git,
        &SessionStoreActivity::new(
            active_fixture.main.join(".session"),
            Duration::from_secs(60),
        ),
        &only_worktree(&active_git),
        &policy(1),
    )
    .unwrap();
    assert_eq!(
        active_eligibility,
        ReclaimEligibility::Rejected(ReclaimRejectionReason::SessionActive)
    );

    let unreadable_fixture = Fixture::new();
    let unreadable_git = unreadable_fixture.git();
    unreadable_git
        .create_worktree("old", "agent/old", "main")
        .unwrap();
    let unreadable_sessions = unreadable_fixture
        .main
        .join(".worktrees")
        .join("unreadable-session")
        .join("nested")
        .join(".session")
        .join("sessions");
    fs::create_dir_all(unreadable_sessions.parent().unwrap()).unwrap();
    fs::write(
        &unreadable_sessions,
        "not a readable session directory
",
    )
    .unwrap();
    let unreadable_eligibility = evaluate_reclaim_candidate(
        &unreadable_git,
        &SessionStoreActivity::new(
            unreadable_fixture.main.join(".session"),
            Duration::from_secs(60),
        ),
        &only_worktree(&unreadable_git),
        &policy(1),
    )
    .unwrap();
    assert_eq!(
        unreadable_eligibility,
        ReclaimEligibility::Rejected(ReclaimRejectionReason::SessionActive)
    );
}

#[test]
pub(super) fn evaluate_reclaim_candidate_reports_detached() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let worktree = git.create_worktree("old", "agent/old", "main").unwrap();
    let candidate = WorktreeRef {
        name: worktree.name,
        path: worktree.path,
        branch: None,
    };

    let eligibility =
        evaluate_reclaim_candidate(&git, &NeverActive, &candidate, &policy(1)).unwrap();
    assert_eq!(
        eligibility,
        ReclaimEligibility::Rejected(ReclaimRejectionReason::Detached)
    );
}

#[test]
pub(super) fn evaluate_reclaim_candidate_reports_dirty() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let worktree = git.create_worktree("old", "agent/old", "main").unwrap();
    fs::write(worktree.path.join("untracked.txt"), "dirty\n").unwrap();

    let eligibility =
        evaluate_reclaim_candidate(&git, &NeverActive, &only_worktree(&git), &policy(1)).unwrap();
    assert_eq!(
        eligibility,
        ReclaimEligibility::Rejected(ReclaimRejectionReason::Dirty)
    );
}

#[test]
pub(super) fn evaluate_reclaim_candidate_reports_contains_current_directory() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let worktree = git.create_worktree("old", "agent/old", "main").unwrap();
    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(&worktree.path).unwrap();

    let eligibility =
        evaluate_reclaim_candidate(&git, &NeverActive, &only_worktree(&git), &policy(1)).unwrap();

    std::env::set_current_dir(original).unwrap();
    assert_eq!(
        eligibility,
        ReclaimEligibility::Rejected(ReclaimRejectionReason::ContainsCurrentDirectory)
    );
}

#[test]
pub(super) fn evaluate_reclaim_candidate_reports_not_idle() {
    let fixture = Fixture::new();
    let git = fixture.git();
    git.create_worktree("old", "agent/old", "main").unwrap();
    let mut not_idle_policy = policy(1);
    not_idle_policy.idle_before_reclaim = Duration::from_secs(60 * 60);

    let eligibility =
        evaluate_reclaim_candidate(&git, &NeverActive, &only_worktree(&git), &not_idle_policy)
            .unwrap();
    assert_eq!(
        eligibility,
        ReclaimEligibility::Rejected(ReclaimRejectionReason::NotIdle)
    );
}

#[test]
pub(super) fn evaluate_reclaim_candidate_reports_dirty_submodule() {
    let fixture = Fixture::new();
    fixture.add_submodule();
    let git = fixture.git();
    let worktree = git.create_worktree("old", "agent/old", "main").unwrap();
    fs::write(
        worktree.path.join("nested").join("inner.txt"),
        "dirty submodule\n",
    )
    .unwrap();

    let eligibility =
        evaluate_reclaim_candidate(&git, &NeverActive, &only_worktree(&git), &policy(1)).unwrap();
    assert_eq!(
        eligibility,
        ReclaimEligibility::Rejected(ReclaimRejectionReason::DirtySubmodule {
            path: PathBuf::from("nested"),
        })
    );
}

#[test]
pub(super) fn evaluate_reclaim_candidate_reports_ahead_of_main() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let worktree = git.create_worktree("old", "agent/old", "main").unwrap();
    fs::write(worktree.path.join("tracked.txt"), "advance\n").unwrap();
    commit(&worktree.path, "advance");

    let eligibility =
        evaluate_reclaim_candidate(&git, &NeverActive, &only_worktree(&git), &policy(1)).unwrap();
    assert_eq!(
        eligibility,
        ReclaimEligibility::Rejected(ReclaimRejectionReason::AheadOfMain)
    );
}

#[test]
pub(super) fn reclaim_candidates_are_ordered_by_oldest_mtime_then_name() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let old = git.create_worktree("a-old", "agent/a-old", "main").unwrap();
    let newer = git.create_worktree("b-new", "agent/b-new", "main").unwrap();
    let age = Duration::from_secs(2 * 60 * 60);
    backdate_activity_signals(&git, &old, age);

    let candidates = reclaim_candidates(&git, &NeverActive, &policy(8)).unwrap();
    let names = candidates
        .iter()
        .map(|worktree| worktree.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["a-old", "b-new"]);
    assert!(candidates.iter().any(|worktree| worktree.path == old.path));
    assert!(
        candidates
            .iter()
            .any(|worktree| worktree.path == newer.path)
    );
}
