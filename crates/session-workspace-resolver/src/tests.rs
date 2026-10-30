use std::fs;

use tempfile::TempDir;

use super::*;

const SESSION_ID: &str = "11111111-1111-4111-8111-111111111111";

#[test]
fn unregistered_nested_checkout_does_not_assign_session() {
    let (_temp, repository, _worktree, resolver) = fixture();
    let leftover = repository
        .join(".worktrees")
        .join(SESSION_ID)
        .join("leftover");
    fs::create_dir_all(leftover.join(".git")).unwrap();
    fs::write(
        leftover.join(".git/HEAD"),
        "ref: refs/heads/agent/leftover\n",
    )
    .unwrap();

    assert!(matches!(
        resolve_root(&resolver, SESSION_ID),
        Err(ResolutionError::MissingSessionWorktree { .. })
    ));
}

fn fixture() -> (TempDir, PathBuf, PathBuf, SessionWorkspaceResolver) {
    let temp = TempDir::new().unwrap();
    let repository = temp.path().join("repository");
    let worktree = repository.join(".worktrees").join("feature");
    fs::create_dir_all(&worktree).unwrap();
    fs::create_dir_all(repository.join(".git")).unwrap();
    fs::create_dir_all(worktree.join(".git")).unwrap();
    let resolver = SessionWorkspaceResolver::new(ResolverConfig {
        main_checkout: repository.clone(),
        workspace_path: "default".to_string(),
    })
    .unwrap();
    (temp, repository, worktree, resolver)
}

fn register_assignment(repository: &Path, session_id: &str, worktree: &Path, branch: &str) {
    let store = SessionStoreConfig::new(repository.join(".session"));
    store
        .register_provisioned_worktree(
            session_id,
            worktree,
            branch,
            session_api::SessionWorktreeAllocationMode::New,
        )
        .unwrap();
    let registry = repository.join(".session/local/worktrees");
    fs::create_dir_all(&registry).unwrap();
    fs::write(
        registry.join(format!("{session_id}.json")),
        serde_json::to_vec(&serde_json::json!({
            "agent_id": "test",
            "ticket_id": "test",
            "assignment": {
                "path": worktree,
                "branch": branch,
                "allocation_mode": "new",
                "status": "active"
            }
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn assignment_changes_are_visible_to_an_existing_resolver() {
    let (_temp, repository, _worktree, resolver) = fixture();
    let first = make_nested_worktree(&repository, SESSION_ID, "first", "agent/first");
    let second = make_nested_worktree(&repository, SESSION_ID, "second", "agent/second");
    register_assignment(&repository, SESSION_ID, &first, "agent/first");
    assert_eq!(
        resolve_root(&resolver, SESSION_ID).unwrap().target_root(),
        first
    );
    register_assignment(&repository, SESSION_ID, &second, "agent/second");
    assert_eq!(
        resolve_root(&resolver, SESSION_ID).unwrap().target_root(),
        second
    );

    fs::remove_file(
        repository
            .join(".session/local/worktrees")
            .join(format!("{SESSION_ID}.json")),
    )
    .unwrap();
    assert!(matches!(
        resolve_root(&resolver, SESSION_ID),
        Err(ResolutionError::MissingSessionWorktree { .. })
    ));
}

#[test]
fn inactive_and_broken_registrations_do_not_fall_back_to_directory_discovery() {
    let (_temp, repository, _worktree, resolver) = fixture();
    let worktree = make_nested_worktree(&repository, SESSION_ID, "feature", "agent/feature");
    register_assignment(&repository, SESSION_ID, &worktree, "agent/feature");
    let path = repository
        .join(".session/local/worktrees")
        .join(format!("{SESSION_ID}.json"));
    let original: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

    for status in ["superseded", "invalidated"] {
        let mut entry = original.clone();
        entry["assignment"]["status"] = serde_json::json!(status);
        fs::write(&path, serde_json::to_vec(&entry).unwrap()).unwrap();
        assert!(matches!(
            resolve_root(&resolver, SESSION_ID),
            Err(ResolutionError::InactiveSessionWorktree { .. })
        ));
    }
    for invalid_path in ["", "relative"] {
        let mut entry = original.clone();
        entry["assignment"]["path"] = serde_json::json!(invalid_path);
        fs::write(&path, serde_json::to_vec(&entry).unwrap()).unwrap();
        assert!(matches!(
            resolve_root(&resolver, SESSION_ID),
            Err(ResolutionError::InvalidConfiguration(_))
        ));
    }
    fs::write(&path, b"not json").unwrap();
    assert!(matches!(
        resolve_root(&resolver, SESSION_ID),
        Err(ResolutionError::SessionLookup(_))
    ));
    fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
    fs::remove_dir_all(&worktree).unwrap();
    assert!(matches!(
        resolve_root(&resolver, SESSION_ID),
        Err(ResolutionError::SessionWorktreeMissing { .. })
    ));
}

#[test]
fn canonical_registry_is_used_without_reading_or_rewriting_session_manifests() {
    let (_temp, repository, _worktree, resolver) = fixture();
    let worktree = make_nested_worktree(&repository, SESSION_ID, "feature", "agent/feature");
    register_assignment(&repository, SESSION_ID, &worktree, "agent/feature");
    fs::create_dir_all(repository.join(".workflow-tools")).unwrap();
    let store = repository.join(".workflow-tools/session");
    fs::rename(repository.join(".session"), &store).unwrap();
    let manifest = store.join("sessions").join(SESSION_ID).join("session.json");
    fs::write(&manifest, b"unrelated legacy manifest").unwrap();

    assert_eq!(
        resolve_root(&resolver, SESSION_ID).unwrap().target_root(),
        worktree
    );
    assert_eq!(fs::read(&manifest).unwrap(), b"unrelated legacy manifest");
}

#[test]
fn resolves_nested_worktree_to_worktree_scope() {
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

    let resolved = resolver
        .resolve(ResolveRequest {
            session_id: SESSION_ID,
            relative_workspace: Some(Path::new("nested")),
            store_dir: ".ticket",
        })
        .unwrap();

    assert!(matches!(
        resolved.checkout(),
        CheckoutScope::Worktree { .. }
    ));
    assert_eq!(resolved.target_root(), worktree.join("nested"));
}

#[test]
fn session_without_assignment_is_rejected() {
    let (_temp, _repository, _worktree, resolver) = fixture();

    assert!(matches!(
        resolve_root(&resolver, SESSION_ID),
        Err(ResolutionError::MissingSessionWorktree { .. })
    ));
}

#[test]
fn resolve_accepts_linked_worktree_git_file() {
    let (_temp, repository, _worktree, resolver) = fixture();
    let worktree = repository
        .join(".worktrees")
        .join(SESSION_ID)
        .join("linked");
    fs::create_dir_all(&worktree).unwrap();
    let private_git_dir = repository.join(".git").join("worktrees").join("linked");
    fs::create_dir_all(&private_git_dir).unwrap();
    fs::write(
        private_git_dir.join("HEAD"),
        "ref: refs/heads/agent/session-a/linked\n",
    )
    .unwrap();
    fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", private_git_dir.display()),
    )
    .unwrap();
    register_assignment(&repository, SESSION_ID, &worktree, "agent/session-a/linked");

    let resolved = resolve_root(&resolver, SESSION_ID).unwrap();

    assert!(matches!(
        resolved.checkout(),
        CheckoutScope::Worktree { .. }
    ));
    assert!(resolved.require_mutation_target().is_ok());
}

#[test]
fn root_invocation_without_assignment_is_rejected() {
    let (_temp, repository, worktree, resolver) = fixture();
    fs::create_dir_all(repository.join(".ticket")).unwrap();
    fs::create_dir_all(worktree.join(".ticket")).unwrap();
    for number in 0..15 {
        fs::create_dir_all(
            repository
                .join(".worktrees")
                .join(format!("sibling-{number}"))
                .join(".ticket"),
        )
        .unwrap();
    }

    assert!(matches!(
        resolve_root(&resolver, SESSION_ID),
        Err(ResolutionError::MissingSessionWorktree { .. })
    ));
    assert_eq!(
        resolver.refused_candidates(".ticket").unwrap(),
        vec![repository.join(".ticket")]
    );
}

#[test]
fn worktree_invocation_without_assignment_is_rejected() {
    let (_temp, _repository, worktree, _resolver) = fixture();
    fs::create_dir_all(worktree.join(".ticket")).unwrap();
    let resolver = SessionWorkspaceResolver::new(ResolverConfig {
        main_checkout: worktree.clone(),
        workspace_path: "default".to_string(),
    })
    .unwrap();

    assert!(matches!(
        resolve_root(&resolver, SESSION_ID),
        Err(ResolutionError::MissingSessionWorktree { .. })
    ));
}

fn resolve_root(
    resolver: &SessionWorkspaceResolver,
    session_id: &str,
) -> Result<ResolvedWorkspace, ResolutionError> {
    resolver.resolve(ResolveRequest {
        session_id,
        relative_workspace: None,
        store_dir: ".ticket",
    })
}

#[cfg(unix)]
fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

// Registry routing must not infer ownership from checkout names.

const SESSION: &str = "70abae1b-14c4-4033-9265-d37fe08b02b2";

/// Creates a worktree directory whose `.git` is a real directory, as an
/// ordinary repository has.
fn make_worktree(repository: &Path, name: &str, branch: &str) -> PathBuf {
    let root = repository.join(".worktrees").join(name);
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(
        root.join(".git").join("HEAD"),
        format!("ref: refs/heads/{branch}\n"),
    )
    .unwrap();
    root
}

fn make_nested_worktree(repository: &Path, session_id: &str, slug: &str, branch: &str) -> PathBuf {
    let root = repository.join(".worktrees").join(session_id).join(slug);
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(
        root.join(".git").join("HEAD"),
        format!("ref: refs/heads/{branch}\n"),
    )
    .unwrap();
    root
}

/// Writes a session record into a worktree's own `.session` store, which
/// is what the scan fallback looks for.
fn seed_session_record(worktree: &Path, session_id: &str) {
    let dir = worktree.join(".session").join("sessions").join(session_id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("session.json"), "{}").unwrap();
}

#[test]
fn missing_assignment_is_rejected() {
    let (_temp, _repository, _worktree, resolver) = fixture();

    assert!(matches!(
        resolve_root(&resolver, SESSION),
        Err(ResolutionError::MissingSessionWorktree { .. })
    ));
}

#[test]
fn registered_linked_worktree_is_resolved() {
    let (_temp, repository, _worktree, resolver) = fixture();
    // A linked worktree's `.git` is a file pointing at its private git
    // directory under the main checkout, which holds its own HEAD.
    let private_git_dir = repository.join(".git").join("worktrees").join("linked");
    fs::create_dir_all(&private_git_dir).unwrap();
    fs::write(
        private_git_dir.join("HEAD"),
        "ref: refs/heads/agent/linked\n",
    )
    .unwrap();

    let root = repository.join(".worktrees").join("70abae1b-linked");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join(".git"),
        format!("gitdir: {}\n", private_git_dir.display()),
    )
    .unwrap();
    seed_session_record(&root, SESSION);
    register_assignment(&repository, SESSION, &root, "agent/linked");

    let resolved = resolver
        .resolve(ResolveRequest {
            session_id: SESSION,
            relative_workspace: None,
            store_dir: ".ticket",
        })
        .unwrap();

    assert_eq!(resolved.target_root(), root);
    assert!(matches!(
        resolved.checkout(),
        CheckoutScope::Worktree { .. }
    ));
}

#[path = "tests/store_boundaries.rs"]
mod store_boundaries;
#[allow(unused_imports)]
pub use store_boundaries::*;

#[path = "tests/assignment_discovery.rs"]
mod assignment_discovery;
#[allow(unused_imports)]
pub use assignment_discovery::*;
