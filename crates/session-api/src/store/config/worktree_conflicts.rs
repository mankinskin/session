impl SessionStoreConfig {
    fn ensure_no_active_worktree_conflict(
        &self,
        main_checkout: &Path,
        requested_path: &Path,
        ignored_session_ids: &[&str],
    ) -> Result<(), SessionError> {
        let registry_dir = main_checkout.join(".session/local/worktrees");
        let entries = match std::fs::read_dir(&registry_dir) {
            Ok(entries) => entries,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(source) => {
                return Err(SessionError::Io {
                    path: registry_dir,
                    source,
                });
            }
        };

        for entry in entries {
            let entry = entry.map_err(|source| SessionError::Io {
                path: registry_dir.clone(),
                source,
            })?;
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            let session_id = path.file_stem().and_then(|stem| stem.to_str()).ok_or_else(|| {
                SessionError::InvalidManagedWorktree {
                    path: path.clone(),
                    reason: "registry entry has no UTF-8 session id".to_string(),
                }
            })?;
            if ignored_session_ids.contains(&session_id) {
                continue;
            }
            let record: WorktreeRegistryEntry = read_json(&path)?;
            if record.assignment.status == SessionWorktreeStatus::Active
                && canonicalize_worktree_path(&record.assignment.path)? == requested_path
            {
                return Err(SessionError::WorktreeConflict {
                    path: requested_path.to_path_buf(),
                    session_id: session_id.to_string(),
                });
            }
        }

        Ok(())
    }
}
