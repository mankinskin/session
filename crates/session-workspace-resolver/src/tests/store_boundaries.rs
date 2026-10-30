#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn relative_workspace_rejects_escape_and_accepts_nested_path() {
    let (_temp, repository, _worktree, resolver) = fixture();
    let worktree = make_nested_worktree(
        &repository,
        SESSION_ID,
        "feature",
        "agent/session-a/feature",
    );
    register_assignment(
        &repository,
        SESSION_ID,
        &worktree,
        "agent/session-a/feature",
    );
    fs::create_dir_all(worktree.join("nested")).unwrap();

    let escaped = resolver.resolve(ResolveRequest {
        session_id: SESSION_ID,
        relative_workspace: Some(Path::new("nested/../../outside")),
        store_dir: ".ticket",
    });
    let nested = resolver.resolve(ResolveRequest {
        session_id: SESSION_ID,
        relative_workspace: Some(Path::new("nested")),
        store_dir: ".ticket",
    });

    assert!(matches!(
        escaped,
        Err(ResolutionError::RelativeWorkspaceEscapesWorktree { .. })
    ));
    assert_eq!(nested.unwrap().target_root(), worktree.join("nested"));
}

#[test]
pub(super) fn mutation_gate_distinguishes_checkout_scopes() {
    let (_temp, repository, worktree, resolver) = fixture();
    let main = ResolvedWorkspace {
        repository: RepositoryRoot::new(&repository).unwrap(),
        checkout: CheckoutScope::MainCheckout {
            checkout_root: repository.clone(),
        },
        target_root: repository,
        relative_path: PathBuf::new(),
    };
    let worktree = ResolvedWorkspace {
        repository: RepositoryRoot::new(main.repository_root()).unwrap(),
        checkout: CheckoutScope::Worktree {
            worktree_root: worktree.clone(),
            branch: "agent/session".to_string(),
        },
        target_root: worktree,
        relative_path: PathBuf::new(),
    };

    assert!(matches!(
        main.require_mutation_target(),
        Err(ResolutionError::MainCheckoutMutationBlocked)
    ));
    assert!(worktree.require_mutation_target().is_ok());
    drop(resolver);
}

#[test]
pub(super) fn store_root_rejects_symlink_escape() {
    let (temp, repository, worktree, _resolver) = fixture();
    let outside = temp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::create_dir_all(worktree.join(".workflow-tools")).unwrap();
    let link = worktree.join(".workflow-tools/ticket");
    if let Err(error) = create_dir_symlink(&outside, &link) {
        if error.kind() == std::io::ErrorKind::PermissionDenied
            || error.raw_os_error() == Some(1314)
        {
            return;
        }
        panic!("failed to create symlink: {error}");
    }
    let resolved = ResolvedWorkspace {
        repository: RepositoryRoot::new(&repository).unwrap(),
        checkout: CheckoutScope::Worktree {
            worktree_root: worktree.clone(),
            branch: "agent/session".to_string(),
        },
        target_root: worktree,
        relative_path: PathBuf::new(),
    };

    assert!(matches!(
        resolved.store_root(".ticket"),
        Err(ResolutionError::StoreDirectoryEscapesTarget { store_dir }) if store_dir == ".ticket"
    ));
}

#[test]
pub(super) fn store_root_prefers_existing_canonical_layout_over_legacy() {
    let (_temp, repository, worktree, _resolver) = fixture();
    let resolved = ResolvedWorkspace {
        repository: RepositoryRoot::new(&repository).unwrap(),
        checkout: CheckoutScope::Worktree {
            worktree_root: worktree.clone(),
            branch: "agent/session".to_string(),
        },
        target_root: worktree.clone(),
        relative_path: PathBuf::new(),
    };
    let canonical = worktree.join(".workflow-tools").join("session");
    fs::create_dir_all(&canonical).unwrap();

    assert_eq!(resolved.store_root(".session").unwrap(), canonical);
}

#[test]
pub(super) fn store_root_allows_missing_directory_and_rejects_lexical_escapes() {
    let (_temp, repository, worktree, _resolver) = fixture();
    let resolved = ResolvedWorkspace {
        repository: RepositoryRoot::new(&repository).unwrap(),
        checkout: CheckoutScope::Worktree {
            worktree_root: worktree.clone(),
            branch: "agent/session".to_string(),
        },
        target_root: worktree.clone(),
        relative_path: PathBuf::new(),
    };

    assert_eq!(
        resolved.store_root(".ticket").unwrap(),
        worktree.join(".workflow-tools").join("ticket")
    );
    assert!(matches!(
        resolved.store_root("../.ticket"),
        Err(ResolutionError::InvalidConfiguration(_))
    ));
    assert!(matches!(
        resolved.store_root("/absolute"),
        Err(ResolutionError::InvalidConfiguration(_))
    ));
}

#[test]
pub(super) fn malformed_nested_worktree_is_not_discovered() {
    let (_temp, repository, _worktree, resolver) = fixture();
    fs::create_dir_all(
        repository
            .join(".worktrees")
            .join(SESSION_ID)
            .join("missing-git"),
    )
    .unwrap();

    assert!(matches!(
        resolve_root(&resolver, SESSION_ID),
        Err(ResolutionError::MissingSessionWorktree { .. })
    ));
}

#[test]
pub(super) fn slug_session_id_is_rejected_before_worktree_discovery() {
    let (_temp, _repository, _worktree, resolver) = fixture();

    assert!(matches!(
        resolve_root(&resolver, "epic-kickoff-8fdfe135"),
        Err(ResolutionError::InvalidSessionId { .. })
    ));
}
