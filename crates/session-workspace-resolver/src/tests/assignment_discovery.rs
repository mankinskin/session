#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn registered_legacy_checkout_resolves_without_name_inference() {
    let (_temp, repository, _worktree, resolver) = fixture();
    let worktree = make_worktree(&repository, "70abae1b-some-slug", "agent/some-slug");
    seed_session_record(&worktree, SESSION);
    register_assignment(&repository, SESSION, &worktree, "agent/some-slug");

    let resolved = resolver
        .resolve(ResolveRequest {
            session_id: SESSION,
            relative_workspace: None,
            store_dir: ".ticket",
        })
        .unwrap();

    assert_eq!(
        resolved.target_root(),
        repository.join(".worktrees").join("70abae1b-some-slug")
    );
}

#[test]
pub(super) fn nested_worktree_requires_main_checkout_registry() {
    let (_temp, repository, _worktree, resolver) = fixture();
    let worktree = repository.join(".worktrees").join(SESSION).join("nested");
    fs::create_dir_all(worktree.join(".git")).unwrap();
    fs::write(
        worktree.join(".git").join("HEAD"),
        "ref: refs/heads/agent/nested\n",
    )
    .unwrap();

    assert!(matches!(
        resolve_root(&resolver, SESSION),
        Err(ResolutionError::MissingSessionWorktree { .. })
    ));
    assert!(
        !repository
            .join(".session")
            .join("sessions")
            .join(SESSION)
            .join("session.json")
            .exists()
    );
}

#[test]
pub(super) fn sibling_for_another_session_is_not_discovered() {
    let (_temp, repository, _worktree, resolver) = fixture();
    make_worktree(&repository, "deadbeef-other-slug", "agent/other");

    assert!(matches!(
        resolve_root(&resolver, SESSION),
        Err(ResolutionError::MissingSessionWorktree { .. })
    ));
}

#[test]
pub(super) fn sibling_session_record_is_not_discovered() {
    let (_temp, repository, _worktree, resolver) = fixture();
    let worktree = make_worktree(&repository, "a1b911ab-by-ticket-id", "agent/ticket");
    seed_session_record(&worktree, "sibling-session");

    assert!(matches!(
        resolve_root(&resolver, SESSION),
        Err(ResolutionError::MissingSessionWorktree { .. })
    ));
}

#[test]
pub(super) fn legacy_worktree_requires_matching_local_session_record() {
    let (_temp, repository, _worktree, resolver) = fixture();
    let worktree = make_worktree(&repository, "70abae1b-some-slug", "agent/ticket");
    seed_session_record(&worktree, "other-session");

    assert!(matches!(
        resolve_root(&resolver, SESSION),
        Err(ResolutionError::MissingSessionWorktree { .. })
    ));
}

#[test]
pub(super) fn multiple_unregistered_prefix_matches_do_not_assign_session() {
    let (_temp, repository, _worktree, resolver) = fixture();
    let first = make_worktree(&repository, "70abae1b-first", "agent/first");
    let second = make_worktree(&repository, "70abae1b-second", "agent/second");
    seed_session_record(&first, SESSION);
    seed_session_record(&second, SESSION);

    assert!(matches!(
        resolve_root(&resolver, SESSION),
        Err(ResolutionError::MissingSessionWorktree { .. })
    ));
}

#[test]
pub(super) fn new_sibling_worktrees_do_not_change_root_resolution() {
    let (_temp, repository, _worktree, resolver) = fixture();
    let first = make_worktree(&repository, "70abae1b-some-slug", "agent/some-slug");
    seed_session_record(&first, SESSION);
    register_assignment(&repository, SESSION, &first, "agent/some-slug");
    assert_eq!(
        resolve_root(&resolver, SESSION).unwrap().target_root(),
        repository.join(".worktrees").join("70abae1b-some-slug")
    );

    make_worktree(&repository, "70abae1b-appeared-later", "agent/later");

    assert_eq!(
        resolve_root(&resolver, SESSION).unwrap().target_root(),
        repository.join(".worktrees").join("70abae1b-some-slug")
    );
}

#[test]
pub(super) fn registered_assignment_wins_over_directory_candidates() {
    let (_temp, repository, _worktree, resolver) = fixture();
    let nested = make_nested_worktree(&repository, SESSION, "nested", "agent/nested");
    let legacy = make_worktree(&repository, "70abae1b-legacy", "agent/legacy");
    seed_session_record(&legacy, SESSION);
    register_assignment(&repository, SESSION, &nested, "agent/nested");

    assert_eq!(
        resolve_root(&resolver, SESSION).unwrap().target_root(),
        nested
    );
}

#[test]
pub(super) fn multiple_unregistered_nested_checkouts_do_not_assign_session() {
    let (_temp, repository, _worktree, resolver) = fixture();
    make_nested_worktree(&repository, SESSION, "alpha", "agent/alpha");
    make_nested_worktree(&repository, SESSION, "zeta", "agent/zeta");

    assert!(matches!(
        resolve_root(&resolver, SESSION),
        Err(ResolutionError::MissingSessionWorktree { .. })
    ));
}
