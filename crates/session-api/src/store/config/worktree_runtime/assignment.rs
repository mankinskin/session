#[allow(unused_imports)]
use super::*;

impl SessionStoreConfig {
    pub fn check_in_worktree(
        &self,
        mut request: SessionWorktreeCheckInRequest,
    ) -> Result<SessionWorktreeCheckInReceipt, SessionError> {
        validate_worktree_request(&request)?;
        let (main_checkout, worktree_path) = self.validate_managed_worktree(&request)?;
        request.worktree_path = worktree_path;
        let main_store = main_session_store_root(&self.root, &main_checkout);
        let main_config = SessionStoreConfig::new(&main_store);
        let mut locked_sessions = vec![request.session_id.as_str()];
        if let Some(predecessor) = request.predecessor_session_id.as_deref() {
            locked_sessions.push(predecessor);
        }
        locked_sessions.sort_unstable();
        locked_sessions.dedup();
        let _locks = locked_sessions
            .into_iter()
            .map(|session_id| main_config.acquire_runtime_lock(session_id))
            .collect::<Result<Vec<_>, _>>()?;
        let registry_file = registry_path(&main_store, &request.session_id);
        let mut replaced_assignment = None;
        if let Some(mut existing) = self.worktree_registry_entry(&request.session_id)? {
            if existing.agent_id != request.owner_id || existing.ticket_id != request.ticket_id {
                return Err(SessionError::SessionOwnershipMismatch {
                    session_id: request.session_id,
                });
            }
            if can_reuse_assignment(&existing.assignment, &request) {
                existing.assignment.allocation_mode = SessionWorktreeAllocationMode::Reused;
                let snapshots =
                    self.worktree_mutation_snapshots(&main_checkout, &request.session_id, None)?;
                let result = (|| {
                    write_json(&registry_file, &existing)?;
                    #[cfg(test)]
                    self.inject_worktree_check_in_failure(
                        WorktreeCheckInFailurePoint::AfterSuccessorRegistry,
                    )?;
                    self.persist_worktree_manifest(&request, &main_checkout)
                })();
                if let Err(error) = result {
                    Self::restore_worktree_snapshots(&snapshots)?;
                    return Err(error);
                }
                return receipt_from_registry(&request.session_id, existing);
            }
            if existing.assignment.status == SessionWorktreeStatus::Active
                && !existing.assignment.path.exists()
            {
                replaced_assignment = Some(existing.assignment);
            }
        }

        let mut predecessor_path = replaced_assignment
            .as_ref()
            .map(|assignment| assignment.path.clone());
        let mut predecessor_update = None;
        if let Some(predecessor_session_id) = &request.predecessor_session_id {
            let predecessor_registry = registry_path(&main_store, predecessor_session_id);
            let mut predecessor = read_json::<WorktreeRegistryEntry>(&predecessor_registry)?;
            let predecessor_assignment = predecessor.assignment.clone();

            if canonicalize_worktree_path(&predecessor_assignment.path)? == request.worktree_path {
                return Err(SessionError::CrossSessionReuseRequiresAdopt {
                    session_id: predecessor_session_id.clone(),
                    path: predecessor_assignment.path,
                });
            }

            predecessor_path = Some(predecessor_assignment.path.clone());
            predecessor.assignment.status = SessionWorktreeStatus::Superseded;
            predecessor_update = Some((predecessor_registry, predecessor));
        }

        self.ensure_no_active_worktree_conflict(
            &main_checkout,
            &request.worktree_path,
            &[
                request.session_id.as_str(),
                request
                    .predecessor_session_id
                    .as_deref()
                    .unwrap_or_default(),
            ],
        )?;

        let entry = WorktreeRegistryEntry {
            agent_id: request.owner_id.clone(),
            ticket_id: request.ticket_id.clone(),
            assignment: SessionWorktreeAssignment {
                path: request.worktree_path.clone(),
                branch: request.branch.clone(),
                allocation_mode: if request.predecessor_session_id.is_some()
                    || replaced_assignment.is_some()
                {
                    SessionWorktreeAllocationMode::Rotated
                } else {
                    SessionWorktreeAllocationMode::New
                },
                status: SessionWorktreeStatus::Active,
                predecessor_session_id: request.predecessor_session_id.clone(),
                predecessor_path,
            },
        };
        let snapshots = self.worktree_mutation_snapshots(
            &main_checkout,
            &request.session_id,
            request.predecessor_session_id.as_deref(),
        )?;
        let result = (|| {
            write_json(&registry_file, &entry)?;
            #[cfg(test)]
            self.inject_worktree_check_in_failure(
                WorktreeCheckInFailurePoint::AfterSuccessorRegistry,
            )?;
            self.persist_worktree_manifest(&request, &main_checkout)?;
            if let Some((predecessor_registry, predecessor)) = &predecessor_update {
                write_json(predecessor_registry, predecessor)?;
                #[cfg(test)]
                self.inject_worktree_check_in_failure(
                    WorktreeCheckInFailurePoint::AfterPredecessorUpdate,
                )?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            Self::restore_worktree_snapshots(&snapshots)?;
            return Err(error);
        }
        receipt_from_registry(&request.session_id, entry)
    }

    /// Reads the explicit check-in registry, never inferring an assignment
    /// from checkout names, branch names, or captured session metadata.
    pub fn registered_worktree(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionWorktreeCheckInReceipt>, SessionError> {
        self.worktree_registry_entry(session_id)?
            .map(|entry| receipt_from_registry(session_id, entry))
            .transpose()
    }

    /// Explicitly returns a session to main without touching checkout contents.
    /// The caller must select the owning main store and supply the recorded
    /// owner, ticket, path and branch, even when the checkout no longer exists.
    pub fn unassign_worktree(
        &self,
        request: SessionWorktreeUnassignRequest,
    ) -> Result<SessionWorktreeUnassignReceipt, SessionError> {
        validate_worktree_request(&SessionWorktreeCheckInRequest {
            session_id: request.session_id.clone(),
            owner_id: request.owner_id.clone(),
            ticket_id: request.ticket_id.clone(),
            worktree_path: request.expected_worktree_path.clone(),
            branch: request.expected_branch.clone(),
            predecessor_session_id: None,
        })?;
        if request.reason.trim().is_empty() {
            return Err(SessionError::MissingWorktreeUnassignReason);
        }
        let main_checkout = canonicalize_worktree_path(&self.main_checkout_for_store()?)?;
        let main_store = main_session_store_root(&self.root, &main_checkout);
        let config = SessionStoreConfig::new(&main_store);
        if canonicalize_worktree_path(&self.root)? != canonicalize_worktree_path(&main_store)? {
            return Err(SessionError::InvalidManagedWorktree {
                path: self.root.clone(),
                reason: "unassignment must target the owning main session store".to_string(),
            });
        }
        let _lock = config.acquire_runtime_lock(&request.session_id)?;
        let previous = config
            .registered_worktree(&request.session_id)?
            .ok_or_else(|| SessionError::MissingWorktreeAssignment {
                session_id: request.session_id.clone(),
            })?;
        if previous.owner_id != request.owner_id || previous.ticket_id != request.ticket_id {
            return Err(SessionError::SessionOwnershipMismatch {
                session_id: request.session_id,
            });
        }
        // Compare recorded values, not filesystem identities: recovery must
        // remain possible after deletion, without following a replacement link.
        if previous.worktree_path != request.expected_worktree_path
            || previous.branch != request.expected_branch
        {
            return Err(SessionError::WorktreeAssignmentChanged {
                session_id: request.session_id,
                expected_path: request.expected_worktree_path,
                expected_branch: request.expected_branch,
            });
        }
        let audit_path = main_store
            .join("local/worktree-unassignments")
            .join(&request.session_id)
            .join(format!("{}.json", Uuid::new_v4()));
        let receipt = SessionWorktreeUnassignReceipt {
            session_id: request.session_id.clone(),
            main_checkout,
            previous_assignment: previous,
            reason: request.reason,
            unassigned_at: Utc::now(),
            audit_path: audit_path.clone(),
        };
        let manifest_path = config
            .paths_for_session_id(&request.session_id)?
            .manifest_path;
        let registry_file = registry_path(&main_store, &request.session_id);
        let snapshots = [
            registry_file.clone(),
            manifest_path.clone(),
            audit_path.clone(),
        ]
        .into_iter()
        .map(WorktreeFileSnapshot::capture)
        .collect::<Result<Vec<_>, _>>()?;
        let result = (|| {
            write_json(&audit_path, &receipt)?;
            match read_json::<serde_json::Value>(&manifest_path) {
                Ok(mut manifest) => {
                    let metadata = manifest
                        .get_mut("metadata")
                        .and_then(serde_json::Value::as_object_mut)
                        .ok_or_else(|| SessionError::InvalidManagedWorktree {
                            path: manifest_path.clone(),
                            reason: "session manifest metadata must be an object".to_string(),
                        })?;
                    metadata.remove("worktree");
                    write_json(&manifest_path, &manifest)?;
                }
                Err(SessionError::NotFound { .. }) => {}
                Err(error) => return Err(error),
            }
            #[cfg(test)]
            self.inject_worktree_check_in_failure(
                WorktreeCheckInFailurePoint::AfterUnassignManifest,
            )?;
            // Routing stays fail-closed until metadata and audit are persisted.
            fs::remove_file(&registry_file).map_err(|source| SessionError::Io {
                path: registry_file,
                source,
            })
        })();
        if let Err(error) = result {
            Self::restore_worktree_snapshots(&snapshots)?;
            return Err(error);
        }
        Ok(receipt)
    }

    pub fn lookup_worktree(
        &self,
        session_id: &str,
    ) -> Result<SessionWorktreeCheckInReceipt, SessionError> {
        let manifest = self.read_session_manifest(session_id)?;
        let receipt = self.registered_worktree(session_id)?.ok_or_else(|| {
            SessionError::MissingWorktreeAssignment {
                session_id: session_id.to_string(),
            }
        })?;
        if !receipt.worktree_path.exists() {
            return Err(SessionError::RegisteredWorktreeMissing {
                session_id: session_id.to_string(),
                path: receipt.worktree_path,
            });
        }
        if manifest
            .metadata
            .worktree
            .as_ref()
            .map(|worktree| &worktree.branch)
            != Some(&receipt.branch)
        {
            return Err(SessionError::MissingWorktreeAssignment {
                session_id: session_id.to_string(),
            });
        }
        Ok(receipt)
    }
}
