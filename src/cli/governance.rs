#[allow(unused_imports)]
use super::*;

/// Handler for grant subcommands.
pub(super) fn handle_grant_command(
    config: &SessionStoreConfig,
    command: GrantCommand,
) -> Result<Value, CliRunError> {
    use chrono::{DateTime, Utc};
    use session_api::{BudgetGrantScope, create_grant, list_grants, revoke_grant};

    match command {
        GrantCommand::Create(args) => {
            let scope = match args.scope.to_lowercase().as_str() {
                "session" => BudgetGrantScope::Session,
                "subagent" => BudgetGrantScope::Subagent,
                _ => {
                    return Err(CliRunError::BadRequest(format!(
                        "invalid scope: {}. allowed values: session, subagent",
                        args.scope
                    )));
                }
            };

            // Handle expiry: prefer TTL, fall back to explicit expires_at
            let ttl_seconds = if let Some(ttl) = args.ttl_seconds {
                Some(ttl)
            } else if let Some(expires_at) = &args.expires_at {
                // Convert RFC3339 to TTL from now
                let expires = DateTime::parse_from_rfc3339(expires_at).map_err(|e| {
                    CliRunError::BadRequest(format!("invalid expires-at timestamp: {e}"))
                })?;
                let now = Utc::now();
                let duration = expires.signed_duration_since(now);
                if duration.num_seconds() < 0 {
                    return Err(CliRunError::BadRequest(
                        "expires-at is in the past".to_string(),
                    ));
                }
                Some(duration.num_seconds() as u64)
            } else {
                None
            };

            let grant = create_grant(config, scope, args.offset, args.model, ttl_seconds)?;
            to_value(&grant)
        }
        GrantCommand::List => {
            let grants = list_grants(config)?;
            to_value(&grants)
        }
        GrantCommand::Revoke(args) => {
            let revoked = revoke_grant(config, &args.grant_id)?;
            to_value(&json!({
                "revoked": revoked,
                "grant_id": args.grant_id,
            }))
        }
    }
}

pub(super) fn handle_escalation_command(
    config: &SessionStoreConfig,
    command: EscalationCommand,
) -> Result<Value, CliRunError> {
    use chrono::Utc;
    use session_api::{
        EscalationAction, EscalationResolution, EscalationStatus, create_escalation,
        escalation_marker, get_escalation, list_escalations, resolve_escalation,
    };

    match command {
        EscalationCommand::Create(args) => {
            let escalation = create_escalation(
                config,
                args.blocking,
                args.context,
                args.requested_capability,
                args.options_considered,
                args.session_id,
                args.from_model,
            )?;

            // Include the marker in the response
            let mut result = serde_json::to_value(&escalation)
                .map_err(|e| CliRunError::Serialization(format!("serialization error: {e}")))?;

            if let Some(obj) = result.as_object_mut() {
                obj.insert(
                    "marker".to_string(),
                    serde_json::Value::String(escalation_marker(&escalation.escalation_id)),
                );
            }

            Ok(result)
        }
        EscalationCommand::List(args) => {
            let status_filter = if let Some(status_str) = args.status {
                match status_str.to_lowercase().as_str() {
                    "open" => Some(EscalationStatus::Open),
                    "resolved" => Some(EscalationStatus::Resolved),
                    _ => {
                        return Err(CliRunError::BadRequest(format!(
                            "invalid status: {}. allowed values: open, resolved",
                            status_str
                        )));
                    }
                }
            } else {
                None
            };

            let escalations = list_escalations(config, status_filter)?;
            to_value(&escalations)
        }
        EscalationCommand::Get(args) => {
            let escalation = get_escalation(config, &args.escalation_id).ok_or_else(|| {
                CliRunError::BadRequest(format!("escalation not found: {}", args.escalation_id))
            })?;
            to_value(&escalation)
        }
        EscalationCommand::Resolve(args) => {
            let action = match args.action.to_lowercase().as_str() {
                "handled" => EscalationAction::Handled,
                "granted-offset" => EscalationAction::GrantedOffset,
                "escalated-to-user" => EscalationAction::EscalatedToUser,
                "spawned-session" => EscalationAction::SpawnedSession,
                _ => {
                    return Err(CliRunError::BadRequest(format!(
                        "invalid action: {}. allowed values: handled, granted-offset, escalated-to-user, spawned-session",
                        args.action
                    )));
                }
            };

            let resolution = EscalationResolution {
                action,
                note: args.note,
                offset_grant_id: args.grant_id,
                spawned_session_id: args.spawned_session_id,
                resolved_at: Utc::now(),
            };

            let escalation = resolve_escalation(config, &args.escalation_id, resolution)?;
            to_value(&escalation)
        }
    }
}
