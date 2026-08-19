impl SessionStoreConfig {
    /// Enumerate one selected source for every session visible from this
    /// checkout's `.session` store and discoverable worktree stores.
    fn federated_sessions(&self) -> Result<Vec<FederatedSessionEntry>, SessionError> {
        let mut candidates = Vec::new();
        for (store, is_worktree_store) in self.federated_store_roots()? {
            let sessions_root = store.sessions_root()?;
            if !sessions_root.exists() {
                continue;
            }

            for entry in fs::read_dir(&sessions_root).map_err(|source| SessionError::Io {
                path: sessions_root.clone(),
                source,
            })? {
                let entry = entry.map_err(|source| SessionError::Io {
                    path: sessions_root.clone(),
                    source,
                })?;
                if !entry.file_type().map_err(|source| SessionError::Io {
                    path: entry.path(),
                    source,
                })?.is_dir() {
                    continue;
                }

                let session_id = entry.file_name().to_string_lossy().into_owned();
                let session_dir = entry.path();
                let source_path = fs::canonicalize(&session_dir).map_err(|source| {
                    SessionError::Io {
                        path: session_dir.clone(),
                        source,
                    }
                })?;
                let priority = federated_source_priority(
                    &store.root,
                    &session_id,
                    is_worktree_store,
                );
                candidates.push(FederatedSessionEntry {
                    store: store.clone(),
                    session_id,
                    session_dir,
                    source_path,
                    priority,
                });
            }
        }

        candidates.sort_by(|left, right| {
            left.session_id
                .cmp(&right.session_id)
                .then_with(|| left.source_path.cmp(&right.source_path))
        });

        let mut selected = Vec::new();
        let mut candidates = candidates.into_iter().peekable();
        while let Some(first) = candidates.next() {
            let session_id = first.session_id.clone();
            let mut duplicates = vec![first];
            while candidates
                .peek()
                .is_some_and(|candidate| candidate.session_id == session_id)
            {
                duplicates.push(candidates.next().expect("peeked candidate exists"));
            }
            duplicates.sort_by(|left, right| {
                right
                    .priority
                    .cmp(&left.priority)
                    .then_with(|| left.source_path.cmp(&right.source_path))
            });
            if duplicates.len() > 1 {
                eprintln!(
                    "[session-api] duplicate session sources for {session_id}; selected {} over {}",
                    duplicates[0].source_path.display(),
                    duplicates[1..]
                        .iter()
                        .map(|entry| entry.source_path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            selected.push(duplicates.remove(0));
        }

        Ok(selected)
    }

    fn federated_store_roots(&self) -> Result<Vec<(SessionStoreConfig, bool)>, SessionError> {
        let mut stores = vec![(self.clone(), false)];
        let Some(main_checkout) = self.root.parent() else {
            return Ok(stores);
        };
        let worktree_root = main_checkout.join(".worktrees");
        let entries = match fs::read_dir(&worktree_root) {
            Ok(entries) => entries,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(stores),
            Err(source) => return Err(SessionError::Io {
                path: worktree_root,
                source,
            }),
        };

        let mut roots = BTreeSet::new();
        for entry in entries {
            let entry = entry.map_err(|source| SessionError::Io {
                path: worktree_root.clone(),
                source,
            })?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let store = path.join(".session");
            if store.is_dir() {
                roots.insert(store);
                continue;
            }
            for nested in fs::read_dir(&path).map_err(|source| SessionError::Io {
                path: path.clone(),
                source,
            })? {
                let nested = nested.map_err(|source| SessionError::Io {
                    path: path.clone(),
                    source,
                })?;
                let store = nested.path().join(".session");
                if store.is_dir() {
                    roots.insert(store);
                }
            }
        }

        stores.extend(roots.into_iter().map(|root| {
            (SessionStoreConfig::new(root, self.workspace_slug.clone()), true)
        }));
        Ok(stores)
    }
}

fn federated_source_priority(
    store_root: &Path,
    session_id: &str,
    is_worktree_store: bool,
) -> u8 {
    if !is_worktree_store {
        return 0;
    }
    let Some(worktree) = store_root.parent() else {
        return 1;
    };
    let Some(layout_root) = worktree.parent() else {
        return 1;
    };
    if layout_root.file_name().is_some_and(|name| name == session_id) {
        return 3;
    }
    let short_id = session_id.get(..8).unwrap_or_default();
    if layout_root.file_name().is_some_and(|name| name == ".worktrees")
        && worktree
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(short_id) && name.as_bytes().get(8) == Some(&b'-'))
    {
        return 2;
    }
    1
}