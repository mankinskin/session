use std::{
    path::{Path, PathBuf},
    process,
};

use memory_kernel::domain_store::CreateEntity;
use memory_kernel::workspace::{CANONICAL_STORES_DIR, canonical_store_root};
use session_api::{
    CopilotHookEvent, FeedbackSignalKind, FollowUpSynthesisOutcome, HostPromptIngress,
    PersistedSessionEvents, SessionCreateInput, SessionDomainStore, SessionError,
    SessionProvisioningDiagnostic, SessionStoreConfig, SessionStorePlan, ToolMetricsWindow,
    ToolResponseOverride, build_follow_up_ticket_draft, mine_explicit_ingestion_signals,
    mine_failed_tool_call_signals, mine_structured_feedback_signals, synthesize_follow_up_ticket,
};
use session_workspace_resolver::{
    ResolutionError, ResolveRequest, ResolverConfig, SessionWorkspaceResolver,
};
use ticket_api::storage::TicketStore;

/// Domain store directory name passed to the workspace resolvers.
const SESSION_STORE_DIR: &str = ".session";

#[path = "args.rs"]
mod args;
#[path = "logging.rs"]
mod logging;

use args::{args_from_hook_stdin, normalize_transcript_path, parse_args, print_usage};

fn main() {
    let _log_guard = logging::init_file_logging();
    match run() {
        Ok(()) => {}
        Err(SessionError::InvalidHookInput(message)) if message == "help" => {
            print_usage();
        }
        Err(error) => {
            tracing::error!(%error, "session-capture-hook failed");
            eprintln!("[session-capture-hook] {error}");
            process::exit(1);
        }
    }
}

fn run() -> Result<(), SessionError> {
    tracing::debug!(args = ?std::env::args().collect::<Vec<_>>(), "invoked");
    let args = parse_args()?;
    let args = if args.from_hook_stdin {
        args_from_hook_stdin(args)?
    } else {
        args
    };
    tracing::info!(
        trigger = %args.trigger,
        hook_event_name = ?args.hook_event_name,
        session_id = ?args.session_id,
        "parsed hook args"
    );

    let transcript_path = normalize_transcript_path(&args.transcript_path);
    let routing_outcome =
        initialize_session_routing(args.session_id.as_deref(), args.store_root.as_deref());
    let store_root =
        resolve_capture_store_root(args.store_root.clone(), args.session_id.as_deref());
    let Some(store_root) = store_root else {
        tracing::warn!("skip: no capture store root resolved");
        emit_hook_payload(routing_outcome.as_ref());
        return Ok(());
    };
    tracing::debug!(store_root = %store_root.display(), "resolved capture store root");
    let config = SessionStoreConfig::new(store_root.clone());
    let hook_event_name = hook_event_name(&args);
    let mut captured_hook_event = hook_event(&args, &hook_event_name);
    bind_requester_decision(&config, &args, &hook_event_name, &mut captured_hook_event)?;
    if !transcript_path.is_file() {
        if let (Some(session_id), Some(event)) = (args.session_id.as_deref(), captured_hook_event) {
            persist_hook_event(&config, &store_root, session_id, event)?;
        }
        tracing::warn!(
            transcript_path = %transcript_path.display(),
            "skip: transcript not found"
        );
        eprintln!(
            "[session-capture-hook] skip: transcript not found at {}",
            transcript_path.display()
        );
        emit_hook_payload(routing_outcome.as_ref());
        return Ok(());
    }

    if transcript_path
        .metadata()
        .map_err(|error| SessionError::Io {
            path: transcript_path.clone(),
            source: error,
        })?
        .len()
        == 0
    {
        if let (Some(session_id), Some(event)) = (args.session_id.as_deref(), captured_hook_event) {
            persist_hook_event(&config, &store_root, session_id, event)?;
        }
        tracing::debug!("skip: transcript is empty and not flushed yet");
        emit_hook_payload(routing_outcome.as_ref());
        return Ok(());
    }
    let tool_response_override = build_tool_response_override(
        args.tool_call_id.as_deref(),
        args.tool_response_chars,
        args.session_id.as_deref(),
        &transcript_path,
    );
    let mut plan = match config.capture_copilot_transcript_with_tool_response(
        transcript_path,
        args.trigger.clone(),
        tool_response_override,
    ) {
        Ok(plan) => plan,
        Err(SessionError::EmptyTurns) => {
            if let (Some(session_id), Some(event)) =
                (args.session_id.as_deref(), captured_hook_event)
            {
                config.persist_hook_event(session_id, event)?;
            }
            tracing::debug!("skip: transcript has no messages yet");
            emit_hook_payload(routing_outcome.as_ref());
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    if let Some(event) = captured_hook_event.as_ref() {
        append_hook_event(&mut plan, event.clone());
    }
    if let Some(outcome) = routing_outcome.as_ref() {
        plan.record.metadata.provisioning = Some(outcome.metadata(&hook_event_name));
    }
    plan.persist()?;
    tracing::info!(session_id = %plan.record.session_id, "persisted capture plan");
    if let (Some(session_id), Some(event)) = (args.session_id.as_deref(), captured_hook_event) {
        mirror_user_prompt_to_main(&store_root, session_id, event)?;
    }
    report_structured_feedback_signals(&plan);
    synthesize_follow_up_tickets(&plan, memory_kernel::workspace::working_dir().as_deref());

    // Best-effort worktree/branch/ticket-id inference from the resolved
    // session store's parent (ticket bba9b313): must never fail capture — a lost
    // session record would be a far worse bug than the linkage this fixes.
    if checkout_for_store_root(&store_root).is_none() {
        eprintln!(
            "[session-capture-hook] worktree/ticket inference skipped: resolved session store has no parent"
        );
    } else if let Err(error) = infer_capture_worktree(&config, &plan.record.session_id, &store_root)
    {
        eprintln!("[session-capture-hook] worktree/ticket inference skipped: {error}");
    }

    // Self-heals the main checkout's registry (ticket 842d74cb D1) on every
    // capture, not just fresh provisioning: a worktree resolves positionally
    // regardless of whether main's own record for it still exists, so a
    // deleted or never-written main-checkout record would otherwise stay
    // missing for the rest of the session's lifetime.
    mirror_worktree_assignment_to_main(&config, &plan.record.session_id, &store_root);

    // Refresh tool metrics rollup (best-effort)
    refresh_tool_metrics_rollup(&config);

    emit_hook_payload(routing_outcome.as_ref());
    Ok(())
}

// Run before ordinary capture so every mirror keeps the decision's event identity.
fn bind_requester_decision(
    config: &SessionStoreConfig,
    args: &args::Args,
    hook_event_name: &str,
    event: &mut Option<CopilotHookEvent>,
) -> Result<(), SessionError> {
    if hook_event_name != "UserPromptSubmit" {
        return Ok(());
    }
    let (Some(session_id), Some(raw)) = (args.session_id.as_deref(), args.raw_prompt.as_deref())
    else {
        return Ok(());
    };
    let ingress = HostPromptIngress::trusted_local_hook(
        hook_event_name,
        raw.as_bytes().to_vec(),
        args.host_event_id.clone(),
    );
    match config.ingest_requester_prompt(session_id, &ingress) {
        Ok(None) => Ok(()),
        Ok(Some(receipt)) => {
            if let Some(event) = event.as_mut() {
                event.event_id = Some(receipt.reference.source_event_id.clone());
                event.data_json = Some(serde_json::json!({ "prompt": raw }));
            }
            Ok(())
        }
        Err(error)
            if matches!(
                error.requester_code(),
                Some("approval_authority_unavailable" | "approval_stale")
            ) =>
        {
            eprintln!("[session-capture-hook] decision line rejected: {error}");
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn persist_hook_event(
    config: &SessionStoreConfig,
    store_root: &Path,
    session_id: &str,
    event: session_api::CopilotHookEvent,
) -> Result<(), SessionError> {
    SessionDomainStore::new(config.clone()).create_entity(SessionCreateInput::HookEvent {
        session_id: session_id.to_string(),
        event: event.clone(),
    })?;
    mirror_user_prompt_to_main(store_root, session_id, event)
}

fn mirror_user_prompt_to_main(
    store_root: &Path,
    session_id: &str,
    event: session_api::CopilotHookEvent,
) -> Result<(), SessionError> {
    if !event
        .event_type
        .as_deref()
        .is_some_and(|event_type| event_type.eq_ignore_ascii_case("UserPromptSubmit"))
    {
        return Ok(());
    }
    let Some(worktree_root) = checkout_for_store_root(store_root) else {
        return Ok(());
    };
    let Some(anchor) = anchor_checkout_for_worktree(worktree_root) else {
        return Ok(());
    };
    let main_store = session_store_root_at(&anchor);
    if main_store == store_root {
        return Ok(());
    }
    SessionDomainStore::new(SessionStoreConfig::new(main_store)).create_entity(
        SessionCreateInput::HookEvent {
            session_id: session_id.to_string(),
            event,
        },
    )?;
    Ok(())
}
#[derive(Debug)]
enum ProvisioningDiagnostic {
    Skipped {
        reason: &'static str,
        worktree: Option<PathBuf>,
    },
}

impl ProvisioningDiagnostic {
    fn set_worktree(&mut self, resolved_worktree: &Path) {
        match self {
            Self::Skipped {
                worktree: Some(diagnostic_worktree),
                ..
            } => *diagnostic_worktree = resolved_worktree.to_path_buf(),
            Self::Skipped { worktree, .. } => {
                *worktree = Some(resolved_worktree.to_path_buf());
            }
        }
    }

    fn metadata(&self, hook_event_name: &str) -> SessionProvisioningDiagnostic {
        match self {
            Self::Skipped { reason, .. } => SessionProvisioningDiagnostic {
                outcome: "skipped".to_string(),
                reason: Some((*reason).to_string()),
                hook_event_name: hook_event_name.to_string(),
            },
        }
    }
}

fn emit_hook_payload(outcome: Option<&ProvisioningDiagnostic>) {
    let _ = outcome;
    println!("{{}}");
}

fn hook_event_name(args: &args::Args) -> String {
    args.hook_event_name
        .as_deref()
        .unwrap_or(&args.trigger)
        .to_owned()
}

fn hook_event(args: &args::Args, hook_event_name: &str) -> Option<CopilotHookEvent> {
    let data_json = match hook_event_name {
        "UserPromptSubmit" => args
            .prompt
            .as_deref()
            .map(|prompt| serde_json::json!({ "prompt": prompt })),
        "SubagentStart" | "SubagentStop" => {
            let agent_id = args.agent_id.as_deref()?;
            Some(serde_json::json!({
                "agent_id": agent_id,
                "agent_type": args.agent_type.as_deref(),
                "stop_hook_active": args.stop_hook_active,
                "timestamp": args.hook_timestamp.as_deref(),
            }))
        }
        _ => None,
    };
    data_json.map(|data_json| CopilotHookEvent {
        event_id: None,
        parent_event_id: None,
        event_type: Some(hook_event_name.to_string()),
        captured_at: None,
        turn_id: None,
        message_id: None,
        tool_call_id: None,
        tool_name: None,
        tool_success: None,
        reasoning_text: None,
        tool_requests_json: None,
        tool_arguments_json: None,
        data_json: Some(data_json),
        raw_event_json: None,
    })
}

fn append_hook_event(plan: &mut SessionStorePlan, event: CopilotHookEvent) {
    let events = plan.events.get_or_insert_with(|| PersistedSessionEvents {
        schema_version: plan.record.schema_version,
        session_id: plan.record.session_id.clone(),
        captured_at: plan.record.captured_at,
        events: Vec::new(),
    });
    events.events.push(event);
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[path = "routing.rs"]
mod routing;
#[allow(unused_imports)]
pub use routing::*;

#[path = "tool_output.rs"]
mod tool_output;
#[allow(unused_imports)]
pub use tool_output::*;

#[path = "feedback.rs"]
mod feedback;
#[allow(unused_imports)]
pub use feedback::*;
