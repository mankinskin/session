use std::{
    path::{
        Path,
        PathBuf,
    },
    process,
};

use session_api::{
    CopilotHookEvent,
    FeedbackSignalKind,
    FollowUpSynthesisOutcome,
    PersistedSessionEvents,
    SessionError,
    SessionProvisioningDiagnostic,
    SessionStoreConfig,
    SessionStorePlan,
    ToolMetricsWindow,
    ToolResponseOverride,
    build_follow_up_ticket_draft,
    mine_explicit_ingestion_signals,
    mine_failed_tool_call_signals,
    mine_structured_feedback_signals,
    synthesize_follow_up_ticket,
};
use session_workspace_resolver::{
    ResolveRequest,
    ResolverConfig,
    SessionWorkspaceResolver,
};
use session_worktree_provision::{
    IndexRebuildOutcome,
    ProvisionError,
    ProvisionOutcome,
    ProvisionPolicy,
    SessionStoreActivity,
    WorktreeGit,
    provision_for_session,
    rebuild_entity_indexes,
};
use ticket_api::storage::TicketStore;

mod args;
mod logging;

use args::{
    args_from_hook_stdin,
    normalize_transcript_path,
    parse_args,
    print_usage,
};

fn main() {
    let _log_guard = logging::init_file_logging();
    match run() {
        Ok(()) => {},
        Err(SessionError::InvalidHookInput(message)) if message == "help" => {
            print_usage();
        },
        Err(error) => {
            tracing::error!(%error, "session-capture-hook failed");
            eprintln!("[session-capture-hook] {error}");
            process::exit(1);
        },
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
    let mut routing_outcome = initialize_session_routing(
        &args.trigger,
        args.session_id.as_deref(),
        args.store_root.as_deref(),
    );
    let mut store_root = resolve_capture_store_root(
        args.store_root.clone(),
        args.session_id.as_deref(),
    );
    // SessionStart provisions the worktree assignment; if that hook was
    // missed (e.g. hooks were reconfigured mid-session), lazily provision on
    // the first later event that carries a session id instead of skipping
    // capture for the rest of the session's lifetime. Stop is excluded: a
    // session that never provisioned during its own lifetime should not
    // spring a fresh worktree into existence only at its very end.
    if store_root.is_none()
        && !args.trigger.eq_ignore_ascii_case("SessionStart")
        && !args.trigger.eq_ignore_ascii_case("Stop")
    {
        tracing::warn!(
            "no store root resolved on non-SessionStart trigger; attempting lazy provisioning fallback for a missed SessionStart"
        );
        let lazy_outcome = initialize_session_routing(
            "SessionStart",
            args.session_id.as_deref(),
            args.store_root.as_deref(),
        );
        if lazy_outcome.is_some() {
            routing_outcome = lazy_outcome;
            store_root = resolve_capture_store_root(
                args.store_root.clone(),
                args.session_id.as_deref(),
            );
        }
    }
    let Some(store_root) = store_root else {
        tracing::warn!("skip: no capture store root resolved");
        emit_hook_payload(routing_outcome.as_ref());
        return Ok(());
    };
    tracing::debug!(store_root = %store_root.display(), "resolved capture store root");
    let config = SessionStoreConfig::new(store_root.clone(), "default");
    let hook_event_name = hook_event_name(&args);
    let captured_hook_event = hook_event(&args, &hook_event_name);
    if !transcript_path.is_file() {
        if let (Some(session_id), Some(event)) =
            (args.session_id.as_deref(), captured_hook_event)
        {
            config.persist_hook_event(session_id, event)?;
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

    let tool_response_override = build_tool_response_override(
        args.tool_call_id.as_deref(),
        args.tool_response_chars,
        args.session_id.as_deref(),
        &transcript_path,
    );
    let mut plan = config.capture_copilot_transcript_with_tool_response(
        transcript_path,
        args.trigger.clone(),
        tool_response_override,
    )?;
    if let Some(event) = captured_hook_event {
        append_hook_event(&mut plan, event);
    }
    if let Some(outcome) = routing_outcome.as_ref() {
        plan.record.metadata.provisioning =
            Some(outcome.metadata(&hook_event_name));
    }
    plan.persist()?;
    tracing::info!(session_id = %plan.record.session_id, "persisted capture plan");
    report_structured_feedback_signals(&plan);
    synthesize_follow_up_tickets(
        &plan,
        memory_kernel::workspace::working_dir().as_deref(),
    );

    // Best-effort worktree/branch/ticket-id inference from the resolved
    // session store's parent (ticket bba9b313): must never fail capture — a lost
    // session record would be a far worse bug than the linkage this fixes.
    if store_root.parent().is_none() {
        eprintln!(
            "[session-capture-hook] worktree/ticket inference skipped: resolved session store has no parent"
        );
    } else if let Err(error) =
        infer_capture_worktree(&config, &plan.record.session_id, &store_root)
    {
        eprintln!(
            "[session-capture-hook] worktree/ticket inference skipped: {error}"
        );
    }

    // Self-heals the main checkout's registry (ticket 842d74cb D1) on every
    // capture, not just fresh provisioning: a worktree resolves positionally
    // regardless of whether main's own record for it still exists, so a
    // deleted or never-written main-checkout record would otherwise stay
    // missing for the rest of the session's lifetime.
    mirror_worktree_assignment_to_main(
        &config,
        &plan.record.session_id,
        &store_root,
    );

    // Refresh tool metrics rollup (best-effort)
    refresh_tool_metrics_rollup(&config);

    emit_hook_payload(routing_outcome.as_ref());
    Ok(())
}

#[derive(Debug)]
enum ProvisioningDiagnostic {
    Provisioned {
        outcome: &'static str,
        worktree: PathBuf,
    },
    Skipped {
        reason: &'static str,
        worktree: Option<PathBuf>,
    },
    Failed {
        reason: String,
    },
}

impl ProvisioningDiagnostic {
    fn worktree(&self) -> Option<&Path> {
        match self {
            Self::Provisioned { worktree, .. } => Some(worktree.as_path()),
            Self::Skipped {
                worktree: Some(worktree),
                ..
            } => Some(worktree.as_path()),
            Self::Skipped { worktree: None, .. } | Self::Failed { .. } => None,
        }
    }

    fn set_worktree(
        &mut self,
        resolved_worktree: &Path,
    ) {
        match self {
            Self::Provisioned {
                worktree: diagnostic_worktree,
                ..
            }
            | Self::Skipped {
                worktree: Some(diagnostic_worktree),
                ..
            } => *diagnostic_worktree = resolved_worktree.to_path_buf(),
            Self::Skipped { worktree, .. } => {
                *worktree = Some(resolved_worktree.to_path_buf());
            },
            Self::Failed { .. } => {},
        }
    }

    fn metadata(
        &self,
        hook_event_name: &str,
    ) -> SessionProvisioningDiagnostic {
        match self {
            Self::Provisioned { outcome, .. } =>
                SessionProvisioningDiagnostic {
                    outcome: (*outcome).to_string(),
                    reason: None,
                    hook_event_name: hook_event_name.to_string(),
                },
            Self::Skipped { reason, .. } => SessionProvisioningDiagnostic {
                outcome: "skipped".to_string(),
                reason: Some((*reason).to_string()),
                hook_event_name: hook_event_name.to_string(),
            },
            Self::Failed { reason } => SessionProvisioningDiagnostic {
                outcome: "failed".to_string(),
                reason: Some(reason.clone()),
                hook_event_name: hook_event_name.to_string(),
            },
        }
    }
}

fn emit_hook_payload(outcome: Option<&ProvisioningDiagnostic>) {
    let _ = outcome;
    println!("{{}}");
}

/// Resolves the checkout the hook was launched in, which anchors positional
/// worktree discovery.
///
/// `MCP_MAIN_CHECKOUT` stays available as an override for callers that cannot
/// control the working directory, but it is not required.
fn anchor_checkout(current_dir: &Path) -> PathBuf {
    std::env::var_os("MCP_MAIN_CHECKOUT")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| current_dir.to_path_buf())
}

fn initialize_session_routing(
    trigger: &str,
    session_id: Option<&str>,
    store_root: Option<&Path>,
) -> Option<ProvisioningDiagnostic> {
    if !trigger.eq_ignore_ascii_case("SessionStart") {
        return Some(ProvisioningDiagnostic::Skipped {
            reason: "trigger_not_session_start",
            worktree: None,
        });
    }
    let Some(session_id) =
        session_id.filter(|session_id| !session_id.trim().is_empty())
    else {
        return Some(ProvisioningDiagnostic::Skipped {
            reason: "missing_session_id",
            worktree: None,
        });
    };
    let current_dir = match std::env::current_dir() {
        Ok(current_dir) => current_dir,
        Err(error) => {
            eprintln!(
                "[session-capture-hook] session routing skipped: could not determine current directory: {error}"
            );
            return Some(ProvisioningDiagnostic::Skipped {
                reason: "current_directory_unavailable",
                worktree: None,
            });
        },
    };
    let anchor = anchor_checkout(&current_dir);
    if !anchor.is_dir() {
        eprintln!(
            "[session-capture-hook] session routing skipped: anchor checkout '{}' does not exist",
            anchor.display()
        );
        return Some(ProvisioningDiagnostic::Skipped {
            reason: "anchor_checkout_invalid",
            worktree: None,
        });
    }
    let mut diagnostic = if eager_provisioning_enabled() {
        provision_session_worktree(&anchor, store_root, session_id)
    } else {
        ProvisioningDiagnostic::Skipped {
            reason: "eager_provisioning_disabled",
            worktree: None,
        }
    };

    // SessionStart can provision a worktree before an assignment exists;
    // in that case route assignment repair through the provisioned path first.
    let resolved_worktree = if let Some(worktree) = diagnostic.worktree() {
        worktree.to_path_buf()
    } else {
        let resolver = match SessionWorkspaceResolver::new(ResolverConfig {
            main_checkout: anchor.clone(),
            workspace_slug: "default".to_string(),
        }) {
            Ok(resolver) => resolver,
            Err(error) => {
                eprintln!(
                    "[session-capture-hook] session routing skipped: could not configure session workspace resolver: {error}"
                );
                return Some(diagnostic);
            },
        };
        let workspace = match resolver.resolve(ResolveRequest {
            session_id,
            relative_workspace: None,
            store_dir: ".session",
        }) {
            Ok(workspace) if workspace.is_worktree() => workspace,
            Ok(_) => {
                eprintln!(
                    "[session-capture-hook] session routing skipped: resolver selected the main checkout for session {session_id}"
                );
                return Some(diagnostic);
            },
            Err(error) => {
                eprintln!(
                    "[session-capture-hook] session routing skipped: no active worktree assignment for session {session_id}: {error}"
                );
                return Some(diagnostic);
            },
        };
        workspace.target_root().to_path_buf()
    };

    diagnostic.set_worktree(&resolved_worktree);
    Some(diagnostic)
}

fn eager_provisioning_enabled() -> bool {
    std::env::var_os("WORKTREE_EAGER_PROVISION")
        .is_none_or(|value| value != "0")
}

fn provision_session_worktree(
    anchor: &Path,
    store_root: Option<&Path>,
    session_id: &str,
) -> ProvisioningDiagnostic {
    if let Some(store_root) = store_root {
        let anchor_store = anchor.join(".session");
        let anchor_root = anchor.canonicalize();
        let resolved_store_root = store_root.canonicalize();
        let store_belongs_to_anchor = anchor_store.is_dir()
            && matches!(
                (&anchor_root, &resolved_store_root),
                (Ok(anchor_root), Ok(store_root)) if store_root.starts_with(anchor_root)
            );
        if !store_belongs_to_anchor {
            eprintln!(
                "[session-capture-hook] worktree provisioning skipped for session {session_id}: anchor checkout '{}' and resolved session store '{}' do not match",
                anchor.display(),
                store_root.display()
            );
            return ProvisioningDiagnostic::Skipped {
                reason: "external_store_mismatch",
                worktree: None,
            };
        }
    }
    let git = match WorktreeGit::open(anchor) {
        Ok(git) => git,
        Err(error) => {
            eprintln!(
                "[session-capture-hook] worktree provisioning failed for session {session_id}: {error}"
            );
            return ProvisioningDiagnostic::Failed {
                reason: format!("worktree_git_open_failed: {error}"),
            };
        },
    };
    let policy = ProvisionPolicy::default();
    let activity =
        SessionStoreActivity::new(anchor.join(".session"), policy.stale_after);
    let (outcome, worktree) =
        match provision_for_session(&git, &activity, session_id, &policy) {
            Ok(ProvisionOutcome::AlreadyProvisioned(worktree)) =>
                ("reused", worktree),
            Ok(ProvisionOutcome::Created(worktree)) => ("created", worktree),
            Ok(ProvisionOutcome::Reclaimed { worktree, .. }) =>
                ("reclaimed", worktree),
            Err(error) => {
                report_provision_error(session_id, error);
                return ProvisioningDiagnostic::Failed {
                    reason: "provisioning_failed".to_string(),
                };
            },
        };

    // Register the assignment in the main checkout's own store (ticket
    // 842d74cb D1: main is the authoritative session-to-worktree registry),
    // independent of and before whatever the worktree's own store captures.
    let main_config = SessionStoreConfig::new(anchor.join(".session"), "default");
    let branch = worktree
        .branch
        .clone()
        .unwrap_or_else(|| format!("agent/{session_id}/session"));
    let allocation_mode = match outcome {
        "reused" => session_api::SessionWorktreeAllocationMode::Reused,
        "reclaimed" => session_api::SessionWorktreeAllocationMode::Rotated,
        _ => session_api::SessionWorktreeAllocationMode::New,
    };
    if let Err(error) = main_config.register_provisioned_worktree(
        session_id,
        &worktree.path,
        &branch,
        allocation_mode,
    ) {
        eprintln!(
            "[session-capture-hook] main-checkout registration failed for session {session_id}: {error}"
        );
    }

    for outcome in rebuild_entity_indexes(&worktree.path) {
        report_index_rebuild_outcome(&worktree.path, outcome);
    }
    ProvisioningDiagnostic::Provisioned {
        outcome,
        worktree: worktree.path,
    }
}

fn report_provision_error(
    session_id: &str,
    error: ProvisionError,
) {
    match error {
        ProvisionError::CapReached {
            max_worktrees,
            current_count,
            reason,
        } => eprintln!(
            "[session-capture-hook] === WORKTREE PROVISION CAP REACHED ===\n\
             session: {session_id}\n\
             cap: {max_worktrees}\n\
             registered worktrees: {current_count}\n\
             reclaimable worktrees: none ({reason})\n\
             remediation: remove a finished worktree, or raise WORKTREE_MAX\n\
             session will continue on the main checkout\n\
             [session-capture-hook] === END WORKTREE PROVISION CAP MESSAGE ==="
        ),
        error => eprintln!(
            "[session-capture-hook] worktree provisioning failed for session {session_id}: {error}"
        ),
    }
}

fn report_index_rebuild_outcome(
    worktree: &Path,
    outcome: IndexRebuildOutcome,
) {
    match outcome {
        IndexRebuildOutcome::Rebuilt { .. } => {},
        IndexRebuildOutcome::Failed { store, error, .. } => eprintln!(
            "[session-capture-hook] index rebuild failed for {store:?} store in {}: {error}",
            worktree.display()
        ),
        IndexRebuildOutcome::Skipped { store, reason, .. } => eprintln!(
            "[session-capture-hook] index rebuild skipped for {store:?} store in {}: {reason}",
            worktree.display()
        ),
    }
}

/// Build the layered output-size override for the tool call that triggered
/// this hook invocation (ticket 44119807 T2).
///
/// The hook stdin's `tool_use_id` is the full on-disk spill entry name,
/// `<bare_id>__vscode-<epoch>`, while the transcript's own `toolCallId` is
/// the bare id without that suffix. The two must be split apart: the bare id
/// is what `apply_tool_response_override` matches against transcript events,
/// while the full suffixed id is the literal spill directory name.
///
/// Layer 1 (`hook_payload`): the hook stdin's `tool_response` string, used
/// only when non-empty (observed to be populated for some tool types, e.g.
/// `run_in_terminal`, and empty for others, e.g. `read_file`).
///
/// Layer 2 (`spill_file`): VS Code Copilot Chat spills large tool outputs to
/// `<workspaceStorage>/<hash>/GitHub.copilot-chat/chat-session-resources/
/// <session_id>/<tool_use_id>/content.txt` (or `content.json`), derived here
/// from the hook stdin's own `transcript_path` (its
/// `GitHub.copilot-chat/transcripts/<session>.jsonl` layout shares the same
/// `GitHub.copilot-chat` root) plus `session_id` and the full `tool_use_id`.
fn build_tool_response_override(
    tool_use_id: Option<&str>,
    tool_response_chars: Option<u64>,
    session_id: Option<&str>,
    transcript_path: &Path,
) -> Option<ToolResponseOverride> {
    let tool_use_id = tool_use_id?;
    let bare_tool_call_id =
        tool_use_id.split("__vscode-").next().unwrap_or(tool_use_id);

    if let Some(output_chars) = tool_response_chars.filter(|chars| *chars > 0) {
        return Some(ToolResponseOverride {
            tool_call_id: bare_tool_call_id.to_string(),
            output_chars,
            output_source: "hook_payload".to_string(),
        });
    }

    let session_id = session_id?;
    let output_chars =
        stat_spill_output_chars(transcript_path, session_id, tool_use_id)?;
    Some(ToolResponseOverride {
        tool_call_id: bare_tool_call_id.to_string(),
        output_chars,
        output_source: "spill_file".to_string(),
    })
}

/// Stat the `chat-session-resources/<session_id>/<tool_use_id>` spill entry
/// relative to the hook stdin's `transcript_path`
/// (`.../GitHub.copilot-chat/transcripts/<session>.jsonl`). `tool_use_id` is
/// the full suffixed id (`<bare_id>__vscode-<epoch>`), matching the literal
/// on-disk directory name. Returns `None` (unmeasured, never a fabricated
/// zero) when the root can't be derived or no spill file is found.
///
/// VS Code writes the spill file asynchronously after invoking the
/// PostToolUse hook, so the file can be briefly absent at hook-fire time;
/// this retries a few times with a short backoff before giving up.
fn stat_spill_output_chars(
    transcript_path: &Path,
    session_id: &str,
    tool_use_id: &str,
) -> Option<u64> {
    let chat_root = transcript_path.parent()?.parent()?;
    let entry_dir = chat_root
        .join("chat-session-resources")
        .join(session_id)
        .join(tool_use_id);

    const MAX_ATTEMPTS: u32 = 5;
    const RETRY_DELAY: std::time::Duration =
        std::time::Duration::from_millis(100);
    for attempt in 0..MAX_ATTEMPTS {
        if let Some(candidate) = ["content.txt", "content.json"]
            .iter()
            .map(|name| entry_dir.join(name))
            .find(|path| path.is_file())
        {
            let bytes = std::fs::read(&candidate).ok()?;
            return Some(String::from_utf8_lossy(&bytes).chars().count() as u64);
        }
        if attempt + 1 < MAX_ATTEMPTS {
            std::thread::sleep(RETRY_DELAY);
        }
    }
    None
}

/// Detect structured feedback signals in the just-captured session and log a
/// compact summary for observability.
///
/// This intentionally does **not** create tickets or write feedback entries.
/// The previous implementation mined free-text with a keyword / confusion-marker
/// heuristic and auto-created tracker tickets, which produced large volumes of
/// false positives (over a hundred spurious tickets in a single run). Auto
/// synthesis is paused until (1) signals are derived only from structured
/// metadata and (2) a backtraceable, verifiable ticket format is defined.
///
/// Two failed-tool-call miners run: the turn-based
/// [`mine_structured_feedback_signals`] (kept for forward compatibility, in
/// case a future capture path populates `role: tool` turns) and the
/// event-based [`mine_failed_tool_call_signals`], which is what actually
/// fires against real captured transcripts — every committed session has
/// zero `role: tool` turns, so tool call/result metadata lives only in the
/// captured events list. The event-based miner also resolves each failure's
/// [`session_api::FailedToolCallMapping`] per the grounded policy.
///
/// `ExplicitIngestion` signals (captured `feedback_ingest` tool calls) are
/// also summarized here for observability, but are never auto-recorded: a
/// successful live call already persisted its own `FeedbackEntry`, and a
/// failed one is left for a dedicated recovery entry point
/// (`recover_feedback_entry_from_signal`) to avoid silently double-writing
/// or partially-writing feedback from a stop-hook code path.
fn report_structured_feedback_signals(plan: &SessionStorePlan) {
    let turn_signals = if plan.record.turns.is_empty() {
        Vec::new()
    } else {
        mine_structured_feedback_signals(&plan.record.turns)
    };
    let workspace_slug = plan.record.metadata.workspace_slug.as_str();
    let event_failed_tool_calls = plan
        .events
        .as_ref()
        .map(|events| {
            mine_failed_tool_call_signals(&events.events, workspace_slug)
        })
        .unwrap_or_default();
    let event_ingestions = plan
        .events
        .as_ref()
        .map(|events| mine_explicit_ingestion_signals(&events.events))
        .unwrap_or_default();

    if turn_signals.is_empty()
        && event_failed_tool_calls.is_empty()
        && event_ingestions.is_empty()
    {
        return;
    }

    let failed_tool_calls = turn_signals
        .iter()
        .chain(event_failed_tool_calls.iter())
        .filter(|signal| {
            matches!(signal.kind, FeedbackSignalKind::FailedToolCall)
        })
        .count();
    let explicit_ingestions = event_ingestions
        .iter()
        .filter(|signal| {
            matches!(signal.kind, FeedbackSignalKind::ExplicitIngestion)
        })
        .count();

    let signals: Vec<_> = turn_signals
        .iter()
        .chain(event_failed_tool_calls.iter())
        .chain(event_ingestions.iter())
        .collect();

    match serde_json::to_string(&signals) {
        Ok(json) => eprintln!(
            "[session-capture-hook] structured feedback signals for session {}: {} total ({} failed tool calls, {} explicit ingestions) {}",
            plan.record.session_id,
            signals.len(),
            failed_tool_calls,
            explicit_ingestions,
            json
        ),
        Err(error) => eprintln!(
            "[session-capture-hook] structured feedback signals for session {}: {} total ({} failed tool calls, {} explicit ingestions); summary serialization failed: {error}",
            plan.record.session_id,
            signals.len(),
            failed_tool_calls,
            explicit_ingestions
        ),
    }
}

/// Re-enable backtraceable, verifiable follow-up ticket synthesis, gated on
/// confident `ExplicitIngestion` signals only (see `session_api::follow_up`
/// module docs for the gating rationale and the idempotent-dedupe design).
/// Ticket-store errors are logged and skipped rather than failing the hook:
/// session capture must still succeed even if the ticket store is
/// unavailable.
fn synthesize_follow_up_tickets(
    plan: &SessionStorePlan,
    cwd: Option<&Path>,
) {
    let Some(events) = plan.events.as_ref() else {
        return;
    };
    let ingestion_signals = mine_explicit_ingestion_signals(&events.events);
    if ingestion_signals.is_empty() {
        return;
    }

    let ticket_root = match cwd {
        Some(cwd) =>
            memory_kernel::workspace::resolve_local_root_from(cwd, ".ticket"),
        None => PathBuf::from(".ticket"),
    };
    let ticket_store = match TicketStore::open_or_init(&ticket_root) {
        Ok(store) => store,
        Err(error) => {
            eprintln!(
                "[session-capture-hook] follow-up synthesis skipped: failed to open ticket store at {}: {error}",
                ticket_root.display()
            );
            return;
        },
    };

    for signal in &ingestion_signals {
        let draft = match build_follow_up_ticket_draft(
            signal,
            &plan.record.session_id,
        ) {
            Ok(Some(draft)) => draft,
            Ok(None) => continue,
            Err(error) => {
                eprintln!(
                    "[session-capture-hook] follow-up draft build failed for session {}: {error}",
                    plan.record.session_id
                );
                continue;
            },
        };

        match synthesize_follow_up_ticket(&ticket_store, &draft, None) {
            Ok(FollowUpSynthesisOutcome::Created(id)) => eprintln!(
                "[session-capture-hook] synthesized follow-up ticket {id} ({})",
                draft.dedupe_key
            ),
            Ok(FollowUpSynthesisOutcome::AlreadyExists(id)) => eprintln!(
                "[session-capture-hook] follow-up ticket {id} already exists for {} (no duplicate created)",
                draft.dedupe_key
            ),
            Err(error) => eprintln!(
                "[session-capture-hook] follow-up ticket synthesis failed for {}: {error}",
                draft.dedupe_key
            ),
        }
    }
}

/// Refresh the tool metrics rollup for the store after a successful capture.
/// Best-effort: rollup write failures do NOT fail the capture.
fn refresh_tool_metrics_rollup(config: &SessionStoreConfig) {
    let window = ToolMetricsWindow::default();
    if let Err(error) = config.write_tool_metrics_rollup(window) {
        eprintln!(
            "[session-capture-hook] tool metrics rollup refresh failed (non-fatal): {error}"
        );
    }
}

fn resolve_capture_store_root(
    store_root: Option<PathBuf>,
    session_id: Option<&str>,
) -> Option<PathBuf> {
    if let Some(store_root) = store_root {
        return Some(store_root);
    }

    let Some(session_id) =
        session_id.filter(|session_id| !session_id.trim().is_empty())
    else {
        eprintln!(
            "[session-capture-hook] capture skipped: hook payload has no session id; refusing to write a default .session store"
        );
        return None;
    };
    let current_dir = match std::env::current_dir() {
        Ok(current_dir) => current_dir,
        Err(error) => {
            eprintln!(
                "[session-capture-hook] capture skipped: could not determine current directory: {error}"
            );
            return None;
        },
    };
    let anchor = anchor_checkout(&current_dir);
    if !anchor.join(".session").is_dir() {
        eprintln!(
            "[session-capture-hook] capture skipped: no session store beneath '{}'; refusing to write a default .session store",
            anchor.display()
        );
        return None;
    }
    let resolver = match SessionWorkspaceResolver::new(ResolverConfig {
        main_checkout: anchor,
        workspace_slug: "default".to_string(),
    }) {
        Ok(resolver) => resolver,
        Err(error) => {
            eprintln!(
                "[session-capture-hook] capture skipped: could not configure session workspace resolver: {error}"
            );
            return None;
        },
    };
    match resolver.resolve(ResolveRequest {
        session_id,
        relative_workspace: None,
        store_dir: ".session",
    }) {
        Ok(workspace) => match workspace.mutation_store_root(".session") {
            Ok(store_root) => Some(store_root),
            Err(error) => {
                eprintln!(
                    "[session-capture-hook] capture skipped: could not resolve worktree session store: {error}"
                );
                None
            },
        },
        Err(error) => {
            eprintln!(
                "[session-capture-hook] capture skipped: no active worktree assignment for session {session_id}: {error}"
            );
            None
        },
    }
}

fn hook_event_name(args: &args::Args) -> String {
    args.hook_event_name
        .as_deref()
        .unwrap_or(&args.trigger)
        .to_owned()
}

fn hook_event(
    args: &args::Args,
    hook_event_name: &str,
) -> Option<CopilotHookEvent> {
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
        },
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

fn append_hook_event(
    plan: &mut SessionStorePlan,
    event: CopilotHookEvent,
) {
    let events = plan.events.get_or_insert_with(|| PersistedSessionEvents {
        schema_version: plan.record.schema_version,
        session_id: plan.record.session_id.clone(),
        captured_at: plan.record.captured_at,
        events: Vec::new(),
    });
    events.events.push(event);
}

fn infer_capture_worktree(
    config: &SessionStoreConfig,
    session_id: &str,
    store_root: &Path,
) -> Result<(), SessionError> {
    let Some(worktree_root) = store_root.parent() else {
        return Ok(());
    };
    config.infer_worktree_from_environment(session_id, worktree_root)
}

/// Mirrors the worktree's own resolved assignment into the main checkout's
/// registry (ticket 842d74cb D1), best-effort: a missing or unreadable main
/// checkout must never fail capture, and re-reading the just-captured record
/// (rather than trusting the in-memory `plan`) picks up whatever
/// `infer_capture_worktree` just persisted.
fn mirror_worktree_assignment_to_main(
    config: &SessionStoreConfig,
    session_id: &str,
    store_root: &Path,
) {
    let Some(worktree_root) = store_root.parent() else {
        return;
    };
    let Some(anchor) = anchor_checkout_for_worktree(worktree_root) else {
        return;
    };
    if !anchor.join(".session").is_dir() {
        return;
    }
    let record = match config.read_session(session_id) {
        Ok(record) => record,
        Err(error) => {
            eprintln!(
                "[session-capture-hook] main-checkout registry mirror skipped for session {session_id}: could not read worktree record: {error}"
            );
            return;
        },
    };
    let Some(assignment) = record.metadata.worktree else {
        return;
    };
    let main_config = SessionStoreConfig::new(anchor.join(".session"), "default");
    if let Err(error) = main_config.register_provisioned_worktree(
        session_id,
        &assignment.path,
        &assignment.branch,
        assignment.allocation_mode,
    ) {
        eprintln!(
            "[session-capture-hook] main-checkout registry mirror failed for session {session_id}: {error}"
        );
    }
}

/// Resolves the main checkout above a nested `.worktrees/<uuid>/<slug>`
/// worktree directory. Returns `None` for a legacy flat layout or a worktree
/// path with no `.worktrees` ancestor rather than guessing.
fn anchor_checkout_for_worktree(worktree_root: &Path) -> Option<PathBuf> {
    let session_dir = worktree_root.parent()?;
    let worktrees_dir = session_dir.parent()?;
    if worktrees_dir.file_name()?.to_str()? != ".worktrees" {
        return None;
    }
    worktrees_dir.parent().map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use std::{
        env,
        path::{
            Path,
            PathBuf,
        },
        sync::{
            Mutex,
            MutexGuard,
        },
    };

    use session_api::SessionStoreConfig;
    use tempfile::tempdir;

    use super::{
        eager_provisioning_enabled,
        infer_capture_worktree,
        initialize_session_routing,
        resolve_capture_store_root,
    };
    use crate::args::normalize_transcript_path;

    struct PoisonTolerantMutex(Mutex<()>);

    impl PoisonTolerantMutex {
        const fn new() -> Self {
            Self(Mutex::new(()))
        }

        fn lock(&self) -> PoisonTolerantLock<'_> {
            PoisonTolerantLock(self.0.lock())
        }
    }

    struct PoisonTolerantLock<'a>(std::sync::LockResult<MutexGuard<'a, ()>>);

    impl<'a> PoisonTolerantLock<'a> {
        fn unwrap(self) -> MutexGuard<'a, ()> {
            self.0.unwrap_or_else(|poisoned| poisoned.into_inner())
        }
    }

    static CWD_LOCK: PoisonTolerantMutex = PoisonTolerantMutex::new();
    static ENV_LOCK: PoisonTolerantMutex = PoisonTolerantMutex::new();

    fn create_git_checkout(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        git(&["init", "--quiet", "--initial-branch", "main"], path);
        git(&["config", "user.email", "hook@example.com"], path);
        git(&["config", "user.name", "hook"], path);
        git(&["commit", "--quiet", "--allow-empty", "-m", "init"], path);
    }

    fn register_active_worktree(
        main_checkout: &Path,
        session_id: &str,
    ) -> PathBuf {
        let worktree = main_checkout
            .join(".worktrees")
            .join(session_id)
            .join("capture");
        create_git_worktree(
            main_checkout,
            &worktree,
            &format!("agent/{session_id}/capture"),
        );
        worktree
    }

    #[test]
    fn capture_writes_to_worktree_store_while_cwd_is_main_checkout() {
        let _cwd_lock = CWD_LOCK.lock().unwrap();
        let _env_lock = ENV_LOCK.lock().unwrap();
        let fixture = tempdir().unwrap();
        let main_checkout = fixture.path().join("main");
        let worktree = register_active_worktree(
            &main_checkout,
            "44444444-4444-4444-8444-444444444444",
        );
        std::fs::create_dir_all(main_checkout.join(".session")).unwrap();
        let original_cwd = std::env::current_dir().unwrap();
        let original_main_checkout = env::var_os("MCP_MAIN_CHECKOUT");
        unsafe { env::set_var("MCP_MAIN_CHECKOUT", &main_checkout) };
        std::env::set_current_dir(&main_checkout).unwrap();

        let store_root = resolve_capture_store_root(
            None,
            Some("44444444-4444-4444-8444-444444444444"),
        )
        .expect("active worktree assignment should resolve");

        let transcript_path = fixture.path().join("capture.jsonl");
        std::fs::write(
            &transcript_path,
            include_str!("../../../tests/fixtures/local_parse_fixture_a.jsonl"),
        )
        .unwrap();
        let config = SessionStoreConfig::new(&store_root, "default");
        let plan = config
            .capture_copilot_transcript_with_tool_response(
                &transcript_path,
                "Stop",
                None,
            )
            .expect("capture should persist into the resolved worktree store");

        std::env::set_current_dir(original_cwd).unwrap();
        unsafe {
            match original_main_checkout {
                Some(value) => env::set_var("MCP_MAIN_CHECKOUT", value),
                None => env::remove_var("MCP_MAIN_CHECKOUT"),
            }
        }
        assert_eq!(store_root, worktree.join(".session"));
        let record = config.read_session(&plan.record.session_id).expect(
            "captured session should be readable from the worktree store",
        );
        assert_eq!(record.session_id, plan.record.session_id);
        assert!(
            !main_checkout
                .join(".session")
                .join("sessions")
                .join(&plan.record.session_id)
                .exists(),
            "capture must not write a same-session record into the main-checkout store"
        );
    }

    #[test]
    fn capture_without_assignment_warns_and_does_not_write_main_checkout() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let fixture = tempdir().unwrap();
        let main_checkout = fixture.path().join("main");
        create_git_worktree(
            &main_checkout,
            &main_checkout.join("seed-worktree"),
            "seed",
        );
        std::fs::create_dir_all(main_checkout.join(".session")).unwrap();
        let original_main_checkout = env::var_os("MCP_MAIN_CHECKOUT");
        unsafe { env::set_var("MCP_MAIN_CHECKOUT", &main_checkout) };

        assert_eq!(resolve_capture_store_root(None, Some("missing")), None);
        unsafe {
            match original_main_checkout {
                Some(value) => env::set_var("MCP_MAIN_CHECKOUT", value),
                None => env::remove_var("MCP_MAIN_CHECKOUT"),
            }
        }
        assert!(
            std::fs::read_dir(main_checkout.join(".session"))
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[test]
    fn capture_with_inactive_assignment_does_not_write_main_checkout() {
        assert_eq!(resolve_capture_store_root(None, Some("inactive")), None);
    }

    #[test]
    fn capture_store_resolution_ignores_process_current_directory() {
        let _cwd_lock = CWD_LOCK.lock().unwrap();
        let _env_lock = ENV_LOCK.lock().unwrap();
        let fixture = tempdir().unwrap();
        let main_checkout = fixture.path().join("main");
        let worktree = register_active_worktree(
            &main_checkout,
            "55555555-5555-4555-8555-555555555555",
        );
        std::fs::create_dir_all(main_checkout.join(".session")).unwrap();
        let unrelated = fixture.path().join("unrelated");
        std::fs::create_dir_all(&unrelated).unwrap();
        let original_cwd = std::env::current_dir().unwrap();
        let original_main_checkout = env::var_os("MCP_MAIN_CHECKOUT");
        unsafe { env::set_var("MCP_MAIN_CHECKOUT", &main_checkout) };
        std::env::set_current_dir(&unrelated).unwrap();

        let result = resolve_capture_store_root(
            None,
            Some("55555555-5555-4555-8555-555555555555"),
        );

        std::env::set_current_dir(original_cwd).unwrap();
        unsafe {
            match original_main_checkout {
                Some(value) => env::set_var("MCP_MAIN_CHECKOUT", value),
                None => env::remove_var("MCP_MAIN_CHECKOUT"),
            }
        }
        assert_eq!(result, Some(worktree.join(".session")));
    }

    #[test]
    fn capture_inference_uses_resolved_store_parent_not_process_directory() {
        let _cwd_lock = CWD_LOCK.lock().unwrap();
        let fixture = tempdir().unwrap();
        let main_checkout = fixture.path().join("main");
        let worktree = main_checkout.join("worktree");
        create_git_worktree(&main_checkout, &worktree, "feature");
        let store_root = worktree.join(".session");
        let config = SessionStoreConfig::new(&store_root, "default");
        let original_cwd = env::current_dir().unwrap();
        env::set_current_dir(&main_checkout).unwrap();

        infer_capture_worktree(&config, "session-store-parent", &store_root)
            .unwrap();

        env::set_current_dir(original_cwd).unwrap();
        let record = config.read_session("session-store-parent").unwrap();
        assert_eq!(
            record
                .metadata
                .worktree
                .unwrap()
                .path
                .canonicalize()
                .unwrap(),
            worktree.canonicalize().unwrap()
        );
    }

    #[test]
    fn normalize_transcript_path_keeps_plain_paths() {
        let path = PathBuf::from("C:/repo/transcript.jsonl");
        let normalized = normalize_transcript_path(&path);
        assert!(!normalized.as_os_str().is_empty());
    }

    fn git(
        args: &[&str],
        cwd: &Path,
    ) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed in {}", cwd.display());
    }

    /// Creates a real repository plus a real linked worktree.
    ///
    /// Worktree inference shells out to `git rev-parse`, so a fixture that only
    /// fabricates a `.git` entry would silently no-op instead of assigning.
    fn create_git_worktree(
        main_checkout: &Path,
        worktree: &Path,
        branch: &str,
    ) {
        std::fs::create_dir_all(main_checkout).unwrap();
        git(&["init", "--quiet"], main_checkout);
        git(&["config", "user.email", "hook@example.com"], main_checkout);
        git(&["config", "user.name", "hook"], main_checkout);
        git(
            &["commit", "--quiet", "--allow-empty", "-m", "init"],
            main_checkout,
        );
        std::fs::create_dir_all(worktree.parent().unwrap()).unwrap();
        git(
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                branch,
                worktree.to_str().unwrap(),
            ],
            main_checkout,
        );
    }

    fn run_session_start(
        main_checkout: &Path,
        process_directory: &Path,
        session_id: Option<&str>,
    ) {
        let original_cwd = env::current_dir().unwrap();
        let original_main_checkout = env::var_os("MCP_MAIN_CHECKOUT");
        unsafe { env::set_var("MCP_MAIN_CHECKOUT", main_checkout) };
        env::set_current_dir(process_directory).unwrap();

        initialize_session_routing("SessionStart", session_id, None);

        env::set_current_dir(original_cwd).unwrap();
        unsafe {
            match original_main_checkout {
                Some(value) => env::set_var("MCP_MAIN_CHECKOUT", value),
                None => env::remove_var("MCP_MAIN_CHECKOUT"),
            }
        }
    }

    #[test]
    fn session_start_provisions_a_positional_worktree_without_anchor_state() {
        let _cwd_lock = CWD_LOCK.lock().unwrap();
        let _env_lock = ENV_LOCK.lock().unwrap();
        let fixture = tempdir().unwrap();
        let main_checkout = fixture.path().join("main");
        create_git_checkout(&main_checkout);
        std::fs::create_dir_all(main_checkout.join(".session")).unwrap();
        let session_id = "99999999-9999-4999-8999-999999999999";

        run_session_start(&main_checkout, &main_checkout, Some(session_id));

        assert!(
            main_checkout
                .join(".worktrees")
                .join(session_id)
                .join("session")
                .is_dir()
        );
        // Ticket 842d74cb D1: the main checkout is the authoritative
        // session-to-worktree registry, so SessionStart seeds a minimal
        // registration record there in addition to provisioning the worktree.
        assert!(
            main_checkout
                .join(".session")
                .join("sessions")
                .join(session_id)
                .join("session.json")
                .is_file()
        );
    }

    #[test]
    fn session_start_without_session_id_or_with_blank_id_does_not_assign() {
        let _cwd_lock = CWD_LOCK.lock().unwrap();
        let _env_lock = ENV_LOCK.lock().unwrap();
        let fixture = tempdir().unwrap();
        let main_checkout = fixture.path().join("main");
        let worktree = main_checkout.join("worktree");
        create_git_worktree(&main_checkout, &worktree, "feature");

        run_session_start(&main_checkout, &worktree, None);
        run_session_start(&main_checkout, &worktree, Some("   "));

        assert!(!main_checkout.join(".session").exists());
        assert!(!main_checkout.join(".worktrees").exists());
    }

    #[test]
    fn non_session_start_does_not_provision_or_assign() {
        let _cwd_lock = CWD_LOCK.lock().unwrap();
        let _env_lock = ENV_LOCK.lock().unwrap();
        let fixture = tempdir().unwrap();
        let main_checkout = fixture.path().join("main");
        let worktree = main_checkout.join("worktree");
        create_git_worktree(&main_checkout, &worktree, "feature");
        let original_cwd = env::current_dir().unwrap();
        let original_main_checkout = env::var_os("MCP_MAIN_CHECKOUT");
        unsafe { env::set_var("MCP_MAIN_CHECKOUT", &main_checkout) };
        env::set_current_dir(&worktree).unwrap();

        initialize_session_routing(
            "Stop",
            Some("session-one"),
            Some(&main_checkout.join(".session")),
        );

        env::set_current_dir(original_cwd).unwrap();
        unsafe {
            match original_main_checkout {
                Some(value) => env::set_var("MCP_MAIN_CHECKOUT", value),
                None => env::remove_var("MCP_MAIN_CHECKOUT"),
            }
        }
        assert!(!main_checkout.join(".session").exists());
        assert!(!main_checkout.join(".worktrees").exists());
    }

    #[test]
    fn eager_provision_kill_switch_disables_provisioning() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let original = env::var_os("WORKTREE_EAGER_PROVISION");
        unsafe { env::set_var("WORKTREE_EAGER_PROVISION", "0") };

        assert!(!eager_provisioning_enabled());

        unsafe {
            match original {
                Some(value) => env::set_var("WORKTREE_EAGER_PROVISION", value),
                None => env::remove_var("WORKTREE_EAGER_PROVISION"),
            }
        }
    }

    #[test]
    fn session_start_skips_a_missing_anchor_override() {
        let _cwd_lock = CWD_LOCK.lock().unwrap();
        let _env_lock = ENV_LOCK.lock().unwrap();
        let fixture = tempdir().unwrap();
        let main_checkout = fixture.path().join("main");
        let worktree = main_checkout.join("worktree");
        let invalid_main_checkout = fixture.path().join("missing-main");
        create_git_worktree(&main_checkout, &worktree, "feature");
        let original_cwd = env::current_dir().unwrap();
        let original_main_checkout = env::var_os("MCP_MAIN_CHECKOUT");
        unsafe { env::set_var("MCP_MAIN_CHECKOUT", &invalid_main_checkout) };
        env::set_current_dir(&worktree).unwrap();

        initialize_session_routing(
            "SessionStart",
            Some("session-one"),
            Some(&invalid_main_checkout.join(".session")),
        );

        env::set_current_dir(original_cwd).unwrap();
        unsafe {
            match original_main_checkout {
                Some(value) => env::set_var("MCP_MAIN_CHECKOUT", value),
                None => env::remove_var("MCP_MAIN_CHECKOUT"),
            }
        }
        assert!(!invalid_main_checkout.exists());
    }
}
