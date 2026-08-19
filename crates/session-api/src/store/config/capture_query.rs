impl SessionStoreConfig {
    /// Persist a hook event without requiring a transcript snapshot. This is
    /// used for UserPromptSubmit, which can arrive before VS Code flushes the
    /// submitted prompt into the transcript file.
    pub fn persist_hook_event(
        &self,
        session_id: &str,
        event: CopilotHookEvent,
    ) -> Result<(), SessionError> {
        let paths = self.paths_for_session_id(session_id)?;
        ensure_local_gitignore(&self.root)?;
        fs::create_dir_all(&paths.session_dir).map_err(|source| {
            SessionError::Io {
                path: paths.session_dir.clone(),
                source,
            }
        })?;
        let captured_at = chrono::Utc::now();
        let incoming = PersistedSessionEvents {
            schema_version: crate::SESSION_SCHEMA_VERSION,
            session_id: session_id.to_string(),
            captured_at,
            events: vec![event],
        };
        let events = merge_events(
            read_json_if_exists(&paths.events_path)?,
            Some(incoming),
            session_id.to_string(),
            captured_at,
        )?
        .expect("a supplied hook event always produces persisted events");
        write_json(&paths.events_path, &events)
    }



    pub fn new(
        root: impl Into<PathBuf>,
        workspace_slug: impl Into<String>,
    ) -> Self {
        Self {
            root: root.into(),
            workspace_slug: workspace_slug.into(),
        }
    }

    pub fn paths_for(
        &self,
        record: &SessionRecord,
    ) -> Result<SessionStorePaths, SessionError> {
        self.paths_for_session_id(&record.session_id)
    }

    pub fn capture_copilot_hook(
        &self,
        payload: CopilotHookPayload,
    ) -> Result<SessionStorePlan, SessionError> {
        self.persist_capture(SessionCaptureRequest::copilot(payload))
    }

    pub fn capture_copilot_transcript(
        &self,
        transcript_path: impl AsRef<Path>,
        trigger: impl Into<String>,
    ) -> Result<SessionStorePlan, SessionError> {
        let payload = copilot_payload_from_transcript_path(
            transcript_path,
            self.workspace_slug.clone(),
            Some(trigger.into()),
        )?;

        self.capture_copilot_hook(payload)
    }

    /// As [`Self::capture_copilot_transcript`], but merges a hook-invocation-
    /// scoped tool output size (ticket 44119807) into the matching terminal
    /// tool event before persisting, since the transcript file itself never
    /// carries the tool result payload.
    ///
    /// The PostToolUse hook fires before VS Code flushes the triggering tool
    /// call's own completion entry to the transcript file (confirmed via
    /// live capture: a fixed 750ms retry window never caught up, a 5s window
    /// reliably did), so a first parse is routinely missing exactly the one
    /// event the override needs to patch. This synchronous path does a
    /// single parse and persists immediately — no sleep, no retry (spec
    /// budget: ≤50ms p95). It relies on `record_event_tool_call` (in
    /// `tool_metrics.rs`) upgrading an already-counted `tool_call_id`'s
    /// recorded output when a later copy carries a higher-fidelity
    /// `output_source`: the next hook invocation for this session (e.g. the
    /// `Stop` trigger, which re-parses the by-then-flushed transcript and
    /// re-applies the override) produces a distinct `captured_event_key`
    /// (it fingerprints `data_json`, so an override-enriched copy differs
    /// from the override-less one already persisted) and `merge_events`
    /// keeps both; the override-rich copy then lands on that later call
    /// instead of being silently dropped.
    pub fn capture_copilot_transcript_with_tool_response(
        &self,
        transcript_path: impl AsRef<Path>,
        trigger: impl Into<String>,
        tool_response_override: Option<ToolResponseOverride>,
    ) -> Result<SessionStorePlan, SessionError> {
        let transcript_path = transcript_path.as_ref();
        let trigger = trigger.into();

        let payload =
            copilot_payload_from_transcript_path_with_tool_response_override(
                transcript_path,
                self.workspace_slug.clone(),
                Some(trigger.clone()),
                tool_response_override.clone(),
            )?;

        self.capture_copilot_hook(payload)
    }

    pub fn read_session(
        &self,
        session_id: &str,
    ) -> Result<SessionRecord, SessionError> {
        let paths = self.paths_for_session_id(session_id)?;
        let mut record = self.read_session_manifest(session_id)?;
        let transcript: PersistedSessionTranscript =
            read_json(&paths.transcript_path)?;
        ensure_supported_schema_version(
            &paths.transcript_path,
            transcript.schema_version,
        )?;
        record.turns = transcript.turns;
        Ok(record)
    }

    pub(crate) fn read_session_manifest(
        &self,
        session_id: &str,
    ) -> Result<SessionRecord, SessionError> {
        let paths = self.paths_for_session_id(session_id)?;
        let manifest: PersistedSessionManifest =
            read_json(&paths.manifest_path)?;
        ensure_supported_schema_version(
            &paths.manifest_path,
            manifest.schema_version,
        )?;

        Ok(SessionRecord {
            schema_version: manifest.schema_version,
            session_id: manifest.session_id,
            source: manifest.source,
            started_at: manifest.started_at,
            captured_at: manifest.captured_at,
            metadata: manifest.metadata,
            turns: Vec::new(),
            links: manifest.links,
            track_id: manifest.track_id,
            anchor_ticket_id: manifest.anchor_ticket_id,
            parent_session_id: manifest.parent_session_id,
            spawned_session_id: manifest.spawned_session_id,
            emitted_handoff_ids: manifest.emitted_handoff_ids,
            picked_up_handoff_ids: manifest.picked_up_handoff_ids,
        })
    }

    pub fn query_sessions(
        &self,
        query: &SessionQuery,
    ) -> Result<Vec<SessionRecord>, SessionError> {
        let mut records = vec![];
        for entry in self.federated_sessions()? {
            let record = match entry.store.read_session(&entry.session_id) {
                Ok(record) => record,
                Err(error) => {
                    eprintln!(
                        "[session-api] skipping unreadable session {} in query scan at {}: {error}",
                        entry.session_id,
                        entry.source_path.display()
                    );
                    continue;
                },
            };
            if session_matches_query(&record, query) {
                records.push(record);
            }
        }

        records.sort_by(|left, right| {
            right
                .captured_at
                .cmp(&left.captured_at)
                .then_with(|| left.session_id.cmp(&right.session_id))
        });

        if let Some(limit) = query.limit {
            records.truncate(limit);
        }

        Ok(records)
    }

    pub fn latest_session_id(&self) -> Result<Option<String>, SessionError> {
        let sessions_root = self.sessions_root()?;
        if !sessions_root.exists() {
            return Ok(None);
        }

        let mut newest: Option<SessionRecord> = None;
        for entry in
            fs::read_dir(&sessions_root).map_err(|source| SessionError::Io {
                path: sessions_root.clone(),
                source,
            })?
        {
            let entry = entry.map_err(|source| SessionError::Io {
                path: sessions_root.clone(),
                source,
            })?;
            let file_type =
                entry.file_type().map_err(|source| SessionError::Io {
                    path: entry.path(),
                    source,
                })?;
            if !file_type.is_dir() {
                continue;
            }

            let session_id = entry.file_name().to_string_lossy().into_owned();
            let record = match self.read_session(&session_id) {
                Ok(record) => record,
                Err(SessionError::NotFound { .. }) => continue,
                Err(SessionError::Deserialize { .. }) => continue,
                Err(err) => return Err(err),
            };

            let replace = match newest.as_ref() {
                None => true,
                Some(current) =>
                    record.captured_at > current.captured_at
                        || (record.captured_at == current.captured_at
                            && record.session_id < current.session_id),
            };
            if replace {
                newest = Some(record);
            }
        }

        Ok(newest.map(|record| record.session_id))
    }

    pub fn session_audit(
        &self,
        selector: SessionAuditSelector,
    ) -> Result<SessionAuditReport, SessionError> {
        let session_id = match selector {
            SessionAuditSelector::SessionId(session_id) => session_id,
            SessionAuditSelector::Latest => self.latest_session_id()?.ok_or(
                SessionError::NoSessionsFound {
                    root: self.sessions_root()?,
                },
            )?,
        };

        let record = self.read_session(&session_id)?;
        let paths = self.paths_for_session_id(&session_id)?;
        let events: Option<PersistedSessionEvents> =
            read_json_if_exists(&paths.events_path)?;
        if let Some(events) = &events {
            ensure_supported_schema_version(
                &paths.events_path,
                events.schema_version,
            )?;
        }

        Ok(build_session_audit_report(&record, events.as_ref()))
    }

    /// Compute the delegation cost report for a captured session: the
    /// supported, tested replacement for the ad-hoc
    /// `tmp/subagent_cost_probe.py` analysis (ticket b7c61f0e). Reproduces
    /// per-sub-agent tool histograms, cross-agent duplicate-read detection
    /// (path-normalization safe), duplicate-command detection, and real
    /// per-sub-agent token/cost totals once `data_json.usage` is populated.
    pub fn delegation_cost_report(
        &self,
        selector: SessionAuditSelector,
    ) -> Result<crate::DelegationCostReport, SessionError> {
        let session_id = match selector {
            SessionAuditSelector::SessionId(session_id) => session_id,
            SessionAuditSelector::Latest => self.latest_session_id()?.ok_or(
                SessionError::NoSessionsFound {
                    root: self.sessions_root()?,
                },
            )?,
        };

        let record = self.read_session(&session_id)?;
        Ok(crate::delegation_cost::compute_delegation_cost_report(&record))
    }

}
