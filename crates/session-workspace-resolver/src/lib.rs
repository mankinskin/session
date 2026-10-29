use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use memory_kernel::workspace::{
    WorkspacePathError, canonicalize_workspace_root_strict, normalize_slashes, working_dir,
};
use session_api::{SessionStoreConfig, SessionWorktreeStatus};
use thiserror::Error;
use uuid::Uuid;

/// Canonical repository root, discovered from the process working directory.
///
/// The MCP servers are always launched at the checkout they serve, so the
/// working directory is the repository anchor; assignments come only from
/// the session API's explicit check-in registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryRoot(PathBuf);

impl RepositoryRoot {
    pub fn new(path: impl AsRef<Path>) -> Result<Self, ResolutionError> {
        Ok(Self(canonicalize(path.as_ref())?))
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

/// The checkout class owning a resolved target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckoutScope {
    MainCheckout {
        checkout_root: PathBuf,
    },
    Worktree {
        worktree_root: PathBuf,
        branch: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedWorkspace {
    repository: RepositoryRoot,
    checkout: CheckoutScope,
    target_root: PathBuf,
    relative_path: PathBuf,
}

impl ResolvedWorkspace {
    pub fn repository_root(&self) -> &Path {
        self.repository.as_path()
    }

    pub fn checkout(&self) -> &CheckoutScope {
        &self.checkout
    }

    pub fn target_root(&self) -> &Path {
        &self.target_root
    }

    pub fn is_worktree(&self) -> bool {
        matches!(self.checkout, CheckoutScope::Worktree { .. })
    }

    pub fn is_main_checkout(&self) -> bool {
        matches!(self.checkout, CheckoutScope::MainCheckout { .. })
    }

    /// Refuses mutations targeted at the repository's main checkout.
    pub fn require_mutation_target(&self) -> Result<(), ResolutionError> {
        if self.is_main_checkout() {
            return Err(ResolutionError::MainCheckoutMutationBlocked);
        }
        Ok(())
    }

    /// Returns the store's resolved location under `target_root`, preferring
    /// an existing canonical `.workflow-tools/<domain>` layout over a legacy
    /// bare `<store_dir>` layout (delegated to
    /// [`memory_kernel::workspace::resolve_store_root_at_fixed_workspace`],
    /// bounded to this fixed target so resolution never escapes it).
    pub fn store_root(&self, store_dir: &str) -> Result<PathBuf, ResolutionError> {
        validate_store_dir(store_dir)?;
        let store_root = memory_kernel::workspace::resolve_store_root_at_fixed_workspace(
            &self.target_root,
            store_dir,
        );
        let canonical_target = canonicalize(&self.target_root)?;
        let canonical_ancestor = canonicalize_existing_ancestor(&store_root)?;
        if !canonical_ancestor.starts_with(&canonical_target) {
            return Err(ResolutionError::StoreDirectoryEscapesTarget {
                store_dir: store_dir.to_string(),
            });
        }
        Ok(store_root)
    }

    /// Returns a validated store root only when the resolved target permits mutation.
    pub fn mutation_store_root(&self, store_dir: &str) -> Result<PathBuf, ResolutionError> {
        self.require_mutation_target()?;
        self.store_root(store_dir)
    }

    pub fn relative_path(&self) -> &Path {
        &self.relative_path
    }
}

pub struct ResolveRequest<'a> {
    pub session_id: &'a str,
    /// Optional path relative to the worktree. It is never a selector.
    pub relative_workspace: Option<&'a Path>,
    /// Entity-store directory for the calling server, for example `.ticket`.
    pub store_dir: &'a str,
}

#[derive(Debug, Clone)]
pub struct ResolverConfig {
    pub main_checkout: PathBuf,
    pub workspace_path: String,
}

impl ResolverConfig {
    /// Anchors resolution on the process working directory.
    ///
    /// MCP servers are launched at the checkout they serve — including when
    /// that checkout is a submodule working directory rather than a
    /// superproject root — so the working directory is the anchor. A
    /// `git --git-common-dir` walk would escape to the superproject, and an
    /// environment variable would only restate what the working directory
    /// already says.
    pub fn from_working_dir(workspace_path: impl Into<String>) -> Result<Self, ResolutionError> {
        let main_checkout = working_dir().ok_or_else(|| {
            ResolutionError::InvalidConfiguration(
                "unable to determine the process working directory".to_string(),
            )
        })?;
        Ok(Self {
            main_checkout,
            workspace_path: workspace_path.into(),
        })
    }
}

pub struct SessionWorkspaceResolver {
    main_checkout: RepositoryRoot,
}

impl SessionWorkspaceResolver {
    pub fn new(config: ResolverConfig) -> Result<Self, ResolutionError> {
        if config.workspace_path.trim().is_empty() {
            return Err(ResolutionError::InvalidConfiguration(
                "workspace_path must not be empty".to_string(),
            ));
        }
        let main_checkout = RepositoryRoot::new(&config.main_checkout)?;
        Ok(Self { main_checkout })
    }

    /// Resolves only a registered, active assignment. Directory existence
    /// and captured metadata never opt a session into worktree isolation.
    pub fn resolve(
        &self,
        request: ResolveRequest<'_>,
    ) -> Result<ResolvedWorkspace, ResolutionError> {
        validate_session_id(request.session_id)?;
        validate_store_dir(request.store_dir)?;
        let repository = self.main_checkout.clone();
        let store_root = memory_kernel::workspace::resolve_store_root_at_fixed_workspace(
            repository.as_path(),
            ".session",
        );
        let assignment = SessionStoreConfig::new(store_root)
            .registered_worktree(request.session_id)?
            .ok_or_else(|| ResolutionError::MissingSessionWorktree {
                session_id: request.session_id.to_string(),
            })?;
        if assignment.status != SessionWorktreeStatus::Active {
            return Err(ResolutionError::InactiveSessionWorktree {
                session_id: request.session_id.to_string(),
                status: assignment.status,
            });
        }
        if assignment.worktree_path.as_os_str().is_empty()
            || !assignment.worktree_path.is_absolute()
        {
            return Err(ResolutionError::InvalidConfiguration(format!(
                "registered worktree path must be absolute and nonempty for session '{}'",
                request.session_id
            )));
        }
        let worktree_root =
            canonicalize(&assignment.worktree_path).map_err(|error| match error {
                ResolutionError::InvalidConfiguration(_) => {
                    ResolutionError::SessionWorktreeMissing {
                        path: assignment.worktree_path.clone(),
                    }
                }
                other => other,
            })?;
        if !worktree_root.starts_with(repository.as_path()) {
            return Err(ResolutionError::SessionWorktreeOutsideRepository {
                path: worktree_root,
                repository: repository.as_path().to_path_buf(),
            });
        }
        if !is_git_checkout(&worktree_root)? {
            return Err(ResolutionError::SessionWorktreeNotGitCheckout {
                path: worktree_root,
            });
        }
        let relative_path = resolve_relative_path(&worktree_root, request.relative_workspace)?;
        let invocation_is_linked_worktree = repository
            .as_path()
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == ".worktrees");
        let checkout = if worktree_root == repository.as_path() && !invocation_is_linked_worktree {
            CheckoutScope::MainCheckout {
                checkout_root: worktree_root.clone(),
            }
        } else {
            CheckoutScope::Worktree {
                worktree_root: worktree_root.clone(),
                branch: assignment.branch,
            }
        };
        Ok(ResolvedWorkspace {
            repository,
            checkout,
            target_root: worktree_root.join(&relative_path),
            relative_path,
        })
    }

    /// Returns only the invocation checkout's store for diagnostics.
    pub fn refused_candidates(&self, store_dir: &str) -> Result<Vec<PathBuf>, ResolutionError> {
        validate_store_dir(store_dir)?;
        Ok(vec![self.main_checkout.as_path().join(store_dir)])
    }
}

#[derive(Debug, Error)]
pub enum ResolutionError {
    #[error("invalid resolver configuration: {0}")]
    InvalidConfiguration(String),
    #[error("session id is required")]
    MissingSessionId,
    #[error("session id '{session_id}' must be a UUID from the Copilot hook payload")]
    InvalidSessionId { session_id: String },
    #[error("session '{session_id}' has no worktree assignment in the session store")]
    MissingSessionWorktree { session_id: String },
    #[error(
        "session '{session_id}' matches {} worktrees; refusing to choose: {}",
        candidates.len(),
        candidates.iter().map(|path| normalize_slashes(path)).collect::<Vec<_>>().join(", ")
    )]
    AmbiguousSessionWorktree {
        session_id: String,
        candidates: Vec<PathBuf>,
    },
    #[error("assigned session worktree is missing: {}", normalize_slashes(path))]
    SessionWorktreeMissing { path: PathBuf },
    #[error(
        "assigned session worktree {} is outside repository {}",
        normalize_slashes(path),
        normalize_slashes(repository)
    )]
    SessionWorktreeOutsideRepository { path: PathBuf, repository: PathBuf },
    #[error(
        "assigned session worktree is not a git checkout: {}",
        normalize_slashes(path)
    )]
    SessionWorktreeNotGitCheckout { path: PathBuf },
    #[error("session '{session_id}' has inactive worktree assignment: {status:?}")]
    InactiveSessionWorktree {
        session_id: String,
        status: SessionWorktreeStatus,
    },
    #[error(
        "relative workspace path must not be absolute: {}",
        normalize_slashes(path)
    )]
    AbsoluteRelativeWorkspace { path: PathBuf },
    #[error(
        "relative workspace path escapes the worktree: {}",
        normalize_slashes(path)
    )]
    RelativeWorkspaceEscapesWorktree { path: PathBuf },
    #[error(
        "main checkout mutations are blocked; run session_check_in from an assigned worktree path under <repository>/.worktrees/<name> and retry"
    )]
    MainCheckoutMutationBlocked,
    #[error("store directory escapes resolved target: '{store_dir}'")]
    StoreDirectoryEscapesTarget { store_dir: String },
    #[error("workspace selector 'default' for session '{session_id}' is unanchored; refused to select a store from candidates: {}", candidates.iter().map(|path| normalize_slashes(path)).collect::<Vec<_>>().join(", "))]
    UnanchoredDefault {
        session_id: String,
        candidates: Vec<PathBuf>,
    },
    #[error("session store lookup failed: {0}")]
    SessionLookup(#[from] session_api::SessionError),
    #[error("I/O failed for {}: {source}", normalize_slashes(path))]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

fn canonicalize(path: &Path) -> Result<PathBuf, ResolutionError> {
    canonicalize_workspace_root_strict(path).map_err(|error| match error {
        WorkspacePathError::CanonicalizeFailed { input, .. } => {
            ResolutionError::InvalidConfiguration(format!("unable to canonicalize '{input}'"))
        }
        other => ResolutionError::InvalidConfiguration(other.to_string()),
    })
}

fn validate_session_id(session_id: &str) -> Result<(), ResolutionError> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err(ResolutionError::MissingSessionId);
    }
    if Uuid::parse_str(session_id).is_err() {
        return Err(ResolutionError::InvalidSessionId {
            session_id: session_id.to_string(),
        });
    }
    Ok(())
}

fn validate_store_dir(store_dir: &str) -> Result<(), ResolutionError> {
    let path = Path::new(store_dir);
    if store_dir.trim().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(ResolutionError::InvalidConfiguration(format!(
            "invalid store directory '{store_dir}'",
        )));
    }
    Ok(())
}

fn canonicalize_existing_ancestor(path: &Path) -> Result<PathBuf, ResolutionError> {
    let mut ancestor = path;
    while !ancestor.exists() {
        ancestor = ancestor.parent().ok_or_else(|| {
            ResolutionError::InvalidConfiguration(format!(
                "unable to find existing ancestor for '{}'",
                normalize_slashes(path)
            ))
        })?;
    }
    canonicalize(ancestor)
}

fn is_git_checkout(root: &Path) -> Result<bool, ResolutionError> {
    let git_entry = root.join(".git");
    match fs::metadata(&git_entry) {
        Ok(metadata) if metadata.is_dir() => Ok(true),
        Ok(metadata) if metadata.is_file() => fs::read_to_string(&git_entry)
            .map(|contents| {
                contents
                    .lines()
                    .any(|line| line.trim_start().starts_with("gitdir:"))
            })
            .map_err(|source| ResolutionError::Io {
                path: git_entry,
                source,
            }),
        Ok(_) | Err(_) => Ok(false),
    }
}

fn resolve_relative_path(
    worktree_root: &Path,
    relative_workspace: Option<&Path>,
) -> Result<PathBuf, ResolutionError> {
    let Some(relative_workspace) = relative_workspace else {
        return Ok(PathBuf::new());
    };
    if relative_workspace.is_absolute() {
        return Err(ResolutionError::AbsoluteRelativeWorkspace {
            path: relative_workspace.to_path_buf(),
        });
    }
    if relative_workspace.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(ResolutionError::RelativeWorkspaceEscapesWorktree {
            path: relative_workspace.to_path_buf(),
        });
    }
    let target = worktree_root.join(relative_workspace);
    let canonical_target =
        canonicalize(&target).map_err(|_| ResolutionError::RelativeWorkspaceEscapesWorktree {
            path: relative_workspace.to_path_buf(),
        })?;
    if !canonical_target.starts_with(worktree_root) {
        return Err(ResolutionError::RelativeWorkspaceEscapesWorktree {
            path: relative_workspace.to_path_buf(),
        });
    }
    canonical_target
        .strip_prefix(worktree_root)
        .map(PathBuf::from)
        .map_err(|_| ResolutionError::RelativeWorkspaceEscapesWorktree {
            path: relative_workspace.to_path_buf(),
        })
}

#[cfg(test)]
mod tests {
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
        let original: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

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
    fn relative_workspace_rejects_escape_and_accepts_nested_path() {
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
    fn mutation_gate_distinguishes_checkout_scopes() {
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
    fn store_root_rejects_symlink_escape() {
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
    fn store_root_prefers_existing_canonical_layout_over_legacy() {
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
    fn store_root_allows_missing_directory_and_rejects_lexical_escapes() {
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
    fn malformed_nested_worktree_is_not_discovered() {
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

    #[test]
    fn slug_session_id_is_rejected_before_worktree_discovery() {
        let (_temp, _repository, _worktree, resolver) = fixture();

        assert!(matches!(
            resolve_root(&resolver, "epic-kickoff-8fdfe135"),
            Err(ResolutionError::InvalidSessionId { .. })
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

    fn make_nested_worktree(
        repository: &Path,
        session_id: &str,
        slug: &str,
        branch: &str,
    ) -> PathBuf {
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
    fn registered_legacy_checkout_resolves_without_name_inference() {
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
    fn nested_worktree_requires_main_checkout_registry() {
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
    fn sibling_for_another_session_is_not_discovered() {
        let (_temp, repository, _worktree, resolver) = fixture();
        make_worktree(&repository, "deadbeef-other-slug", "agent/other");

        assert!(matches!(
            resolve_root(&resolver, SESSION),
            Err(ResolutionError::MissingSessionWorktree { .. })
        ));
    }

    #[test]
    fn sibling_session_record_is_not_discovered() {
        let (_temp, repository, _worktree, resolver) = fixture();
        let worktree = make_worktree(&repository, "a1b911ab-by-ticket-id", "agent/ticket");
        seed_session_record(&worktree, "sibling-session");

        assert!(matches!(
            resolve_root(&resolver, SESSION),
            Err(ResolutionError::MissingSessionWorktree { .. })
        ));
    }

    #[test]
    fn legacy_worktree_requires_matching_local_session_record() {
        let (_temp, repository, _worktree, resolver) = fixture();
        let worktree = make_worktree(&repository, "70abae1b-some-slug", "agent/ticket");
        seed_session_record(&worktree, "other-session");

        assert!(matches!(
            resolve_root(&resolver, SESSION),
            Err(ResolutionError::MissingSessionWorktree { .. })
        ));
    }

    #[test]
    fn multiple_unregistered_prefix_matches_do_not_assign_session() {
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
    fn missing_assignment_is_rejected() {
        let (_temp, _repository, _worktree, resolver) = fixture();

        assert!(matches!(
            resolve_root(&resolver, SESSION),
            Err(ResolutionError::MissingSessionWorktree { .. })
        ));
    }

    #[test]
    fn new_sibling_worktrees_do_not_change_root_resolution() {
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

    #[test]
    fn registered_assignment_wins_over_directory_candidates() {
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
    fn multiple_unregistered_nested_checkouts_do_not_assign_session() {
        let (_temp, repository, _worktree, resolver) = fixture();
        make_nested_worktree(&repository, SESSION, "alpha", "agent/alpha");
        make_nested_worktree(&repository, SESSION, "zeta", "agent/zeta");

        assert!(matches!(
            resolve_root(&resolver, SESSION),
            Err(ResolutionError::MissingSessionWorktree { .. })
        ));
    }
}
