#[allow(unused_imports)]
use super::*;

pub(super) fn move_command(
    config: &SessionStoreConfig,
    args: MoveArgs,
) -> Result<Value, CliRunError> {
    let source_config = args
        .source_store_root
        .as_ref()
        .map(|root| SessionStoreConfig::new(root.clone()));
    let config = source_config.as_ref().unwrap_or(config);

    if args.resume.is_some() && args.rollback.is_some() {
        return Err(CliRunError::BadRequest(
            "move accepts only one of --resume or --rollback".to_string(),
        ));
    }

    if let Some(journal_id) = args.resume.as_deref() {
        let journal_id = journal_id.parse::<Uuid>().map_err(|error| {
            CliRunError::BadRequest(format!("invalid --resume journal UUID: {error}"))
        })?;
        let outcome = config.resume_move_with_journal(journal_id)?;
        return to_value(&json!({
            "command": "move",
            "status": "ok",
            "mode": "resume",
            "journal_id": outcome.journal.id,
            "phase": outcome.journal.phase,
            "recovery": recovery_hint(),
        }));
    }

    if let Some(journal_id) = args.rollback.as_deref() {
        let journal_id = journal_id.parse::<Uuid>().map_err(|error| {
            CliRunError::BadRequest(format!("invalid --rollback journal UUID: {error}"))
        })?;
        let outcome = config.rollback_move_with_journal(journal_id)?;
        return to_value(&json!({
            "command": "move",
            "status": "ok",
            "mode": "rollback",
            "journal_id": outcome.journal.id,
            "phase": outcome.journal.phase,
            "recovery": recovery_hint(),
        }));
    }

    let id = args.id.as_deref().ok_or_else(|| {
        CliRunError::BadRequest("move requires <id> unless --resume/--rollback is used".to_string())
    })?;
    let target_selector = args.to_workspace_root.as_deref().ok_or_else(|| {
        CliRunError::BadRequest(
            "move requires --to-workspace-root in plan/execute mode".to_string(),
        )
    })?;

    let target_selector = target_selector.to_string_lossy();
    let target_selector = workspace::normalize_explicit_workspace_selector(Some(&target_selector))
        .map_err(|error| CliRunError::BadRequest(error.to_string()))?;
    let session_id = id
        .parse::<Uuid>()
        .map_err(|error| CliRunError::BadRequest(format!("invalid session UUID: {error}")))?;
    let target_workspace_root = workspace::canonicalize_workspace_root_strict(&target_selector)
        .map_err(|error| {
            CliRunError::BadRequest(format!(
                "workspace root canonicalization failed for '{}': {error}",
                target_selector.display()
            ))
        })?;
    let report = config.plan_move_preflight(&session_id, &target_workspace_root)?;

    if args.dry_run || !report.supported() {
        return to_value(&json!({
            "command": "move",
            "status": if report.supported() { "ok" } else { "blocked" },
            "mode": "plan",
            "dry_run": true,
            "session_id": session_id,
            "plan": move_plan_json(&report)?,
            "recovery": recovery_hint(),
        }));
    }

    let outcome = config.execute_move_with_journal(&report)?;
    to_value(&json!({
        "command": "move",
        "status": "ok",
        "mode": "execute",
        "session_id": session_id,
        "plan": move_plan_json(&report)?,
        "outcome": move_outcome_json(&outcome)?,
        "recovery": recovery_hint(),
    }))
}

pub(super) fn move_plan_json(
    report: &memory_kernel::storage::move_kernel::MovePlan,
) -> Result<Value, CliRunError> {
    Ok(json!({
        "supported": report.supported(),
        "entity_id": report.entity_id,
        "source_workspace_root": path_display(&report.source_workspace_root)?,
        "target_workspace_root": path_display(&report.target_workspace_root)?,
        "source_store_root": path_display(&report.source_store_root)?,
        "target_store_root": path_display(&report.target_store_root)?,
        "source_git_worktree_root": path_display(&report.source_git_worktree_root)?,
        "target_git_worktree_root": path_display(&report.target_git_worktree_root)?,
        "git_worktree_topology": report.git_worktree_topology,
        "source_entity_path": path_display(&report.source_entity_path)?,
        "destination_entity_path": path_display(&report.destination_entity_path)?,
        "inbound_related_entity_ids": report.inbound_related_entity_ids,
        "outbound_related_entity_ids": report.outbound_related_entity_ids,
        "reference_visibility": report.reference_visibility,
        "active_board_entries": report.active_board_entries,
        "historical_board_entries": report.historical_board_entries,
        "active_leases": report.active_leases,
        "path_reference_files": report.path_reference_files
            .iter()
            .map(|path| path_display(path))
            .collect::<Result<Vec<_>, _>>()?,
        "blockers": report.blockers,
        "captured_at": report.captured_at,
    }))
}

pub(super) fn move_outcome_json(
    outcome: &memory_kernel::storage::move_kernel::MoveOutcome,
) -> Result<Value, CliRunError> {
    Ok(json!({
        "resumed": outcome.resumed,
        "rolled_back": outcome.rolled_back,
        "journal": {
            "id": outcome.journal.id,
            "entity_id": outcome.journal.entity_id,
            "source_store_root": path_display(&outcome.journal.source_store_root)?,
            "target_store_root": path_display(&outcome.journal.target_store_root)?,
            "source_entity_path": path_display(&outcome.journal.source_entity_path)?,
            "destination_entity_path": path_display(&outcome.journal.destination_entity_path)?,
            "phase": outcome.journal.phase,
            "created_at": outcome.journal.created_at,
            "updated_at": outcome.journal.updated_at,
            "steps": outcome.journal.steps,
            "rollback_steps": outcome.journal.rollback_steps,
            "lock_paths": outcome.journal.lock_paths,
            "migrated_board_entries": outcome.journal.migrated_board_entries,
            "rewritten_path_files": outcome.journal.rewritten_path_files,
            "manual_followups": outcome.journal.manual_followups,
            "failure": outcome.journal.failure,
            "next_recovery_step": outcome.journal.next_recovery_step,
        },
    }))
}

pub(super) fn recovery_hint() -> Value {
    json!({
        "resume": "session move --resume <journal-uuid>",
        "rollback": "session move --rollback <journal-uuid>",
    })
}

pub(super) fn path_display(path: &std::path::Path) -> Result<String, CliRunError> {
    workspace::normalize_path_for_display_strict(path).map_err(|error| {
        CliRunError::BadRequest(format!(
            "path payload normalization failed for '{}': {error}",
            path.display()
        ))
    })
}
