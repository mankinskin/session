impl SessionStoreConfig {
    pub fn create_terminal_observer(
        &self,
        request: SessionTerminalCreateRequest,
    ) -> Result<SessionTerminalManifest, SessionError> {
        validate_session_id(&request.session_id)?;
        if request.label.trim().is_empty() {
            return Err(SessionError::InvalidHookInput(
                "terminal label cannot be empty".to_string(),
            ));
        }

        let _lock = self.acquire_runtime_lock(&request.session_id)?;
        let paths = self.paths_for_session_id(&request.session_id)?;
        if !paths.manifest_path.is_file() {
            return Err(SessionError::NotFound {
                path: paths.manifest_path,
            });
        }

        let manifest = SessionTerminalManifest {
            terminal_id: uuid::Uuid::new_v4().to_string(),
            session_id: request.session_id,
            label: request.label,
            cwd: request.cwd,
            created_at: chrono::Utc::now(),
            status: SessionTerminalStatus::Open,
            closed_at: None,
        };
        let terminal_dir = self.terminal_dir(&manifest.session_id, &manifest.terminal_id)?;
        write_json(&terminal_dir.join("manifest.json"), &manifest)?;
        write_json(&terminal_dir.join("events.json"), &Vec::<SessionTerminalEvent>::new())?;
        Ok(manifest)
    }

    pub fn append_terminal_output(
        &self,
        session_id: &str,
        terminal_id: &str,
        output: String,
    ) -> Result<SessionTerminalEvent, SessionError> {
        let _lock = self.acquire_runtime_lock(session_id)?;
        let mut record = self.read_terminal_record(session_id, terminal_id)?;
        if record.manifest.status != SessionTerminalStatus::Open {
            return Err(SessionError::TerminalClosed {
                terminal_id: terminal_id.to_string(),
            });
        }

        let event = SessionTerminalEvent {
            sequence: record.events.len(),
            captured_at: chrono::Utc::now(),
            output,
        };
        record.events.push(event.clone());
        write_json(&self.terminal_events_path(session_id, terminal_id)?, &record.events)?;
        Ok(event)
    }

    pub fn terminal_status(
        &self,
        session_id: &str,
        terminal_id: &str,
    ) -> Result<SessionTerminalManifest, SessionError> {
        Ok(self.read_terminal_record(session_id, terminal_id)?.manifest)
    }

    pub fn peek_terminal_output(
        &self,
        session_id: &str,
        terminal_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<SessionTerminalPeekResult, SessionError> {
        let record = self.read_terminal_record(session_id, terminal_id)?;
        let limit = limit.clamp(1, 200);
        let start = offset.min(record.events.len());
        let end = start.saturating_add(limit).min(record.events.len());
        Ok(SessionTerminalPeekResult {
            manifest: record.manifest,
            events: record.events[start..end].to_vec(),
            next_offset: end,
            has_more: end < record.events.len(),
        })
    }

    pub fn close_terminal_observer(
        &self,
        session_id: &str,
        terminal_id: &str,
    ) -> Result<SessionTerminalManifest, SessionError> {
        let _lock = self.acquire_runtime_lock(session_id)?;
        let mut record = self.read_terminal_record(session_id, terminal_id)?;
        if record.manifest.status == SessionTerminalStatus::Open {
            record.manifest.status = SessionTerminalStatus::Closed;
            record.manifest.closed_at = Some(chrono::Utc::now());
            write_json(
                &self.terminal_manifest_path(session_id, terminal_id)?,
                &record.manifest,
            )?;
        }
        Ok(record.manifest)
    }

    pub fn read_terminal_record(
        &self,
        session_id: &str,
        terminal_id: &str,
    ) -> Result<SessionTerminalRecord, SessionError> {
        let manifest_path = self.terminal_manifest_path(session_id, terminal_id)?;
        let manifest = read_json_if_exists(&manifest_path)?.ok_or_else(|| {
            SessionError::TerminalNotFound {
                session_id: session_id.to_string(),
                terminal_id: terminal_id.to_string(),
            }
        })?;
        let events = read_json_if_exists(&self.terminal_events_path(session_id, terminal_id)?)?
            .unwrap_or_default();
        Ok(SessionTerminalRecord { manifest, events })
    }

    fn terminal_dir(
        &self,
        session_id: &str,
        terminal_id: &str,
    ) -> Result<PathBuf, SessionError> {
        validate_session_id(session_id)?;
        if uuid::Uuid::parse_str(terminal_id).is_err() {
            return Err(SessionError::InvalidTerminalId(terminal_id.to_string()));
        }
        Ok(self.paths_for_session_id(session_id)?.session_dir.join("terminals").join(terminal_id))
    }

    fn terminal_manifest_path(
        &self,
        session_id: &str,
        terminal_id: &str,
    ) -> Result<PathBuf, SessionError> {
        Ok(self.terminal_dir(session_id, terminal_id)?.join("manifest.json"))
    }

    fn terminal_events_path(
        &self,
        session_id: &str,
        terminal_id: &str,
    ) -> Result<PathBuf, SessionError> {
        Ok(self.terminal_dir(session_id, terminal_id)?.join("events.json"))
    }
}