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
#[path = "tests.rs"]
mod tests;
