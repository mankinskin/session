#[allow(unused_imports)]
use super::*;

pub(super) fn parse_validation_gates(
    raw: Option<&str>,
) -> Result<Vec<SessionValidationGate>, CliRunError> {
    let Some(raw) = raw else {
        return Ok(vec![]);
    };
    serde_json::from_str::<Vec<SessionValidationGate>>(raw)
        .map_err(|err| CliRunError::BadRequest(format!("invalid --validation-json payload: {err}")))
}

pub(super) fn handoff_package_from_args(
    args: &HandoffArgs,
) -> Result<Option<SessionHandoffPackage>, CliRunError> {
    let target_tickets = args
        .target_tickets
        .iter()
        .map(|raw| {
            if raw.starts_with('{') {
                serde_json::from_str(raw).map_err(|error| {
                    CliRunError::BadRequest(format!(
                        "invalid --target-ticket JSON payload: {error}"
                    ))
                })
            } else {
                Ok(SessionHandoffTargetTicket {
                    id: raw.clone(),
                    why: String::new(),
                    state: String::new(),
                    acceptance_criteria: Vec::new(),
                })
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let upward_context = args
        .upward_context
        .iter()
        .map(|raw| {
            serde_json::from_str::<SessionHandoffUpwardContextEntry>(raw).map_err(|error| {
                CliRunError::BadRequest(format!("invalid --upward-context JSON payload: {error}"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let objective = args.objective.clone().unwrap_or_default();
    let higher_level_objective = args.higher_level_objective.clone().unwrap_or_default();
    let has_package = !objective.is_empty()
        || !target_tickets.is_empty()
        || !higher_level_objective.is_empty()
        || !upward_context.is_empty()
        || !args.target_files.is_empty()
        || !args.decisions.is_empty()
        || !args.non_goals.is_empty()
        || !args.context_anchors.is_empty()
        || !args.open_escalations.is_empty()
        || args.risk_notes.is_some()
        || args.predecessor_handoff.is_some();
    Ok(has_package.then_some(SessionHandoffPackage {
        objective,
        target_tickets,
        higher_level_objective,
        upward_context,
        target_files: args.target_files.clone(),
        decisions: args.decisions.clone(),
        non_goals: args.non_goals.clone(),
        context_anchors: args.context_anchors.clone(),
        open_escalations: args.open_escalations.clone(),
        risk_notes: args.risk_notes.clone(),
        predecessor_handoff: args.predecessor_handoff.clone(),
    }))
}
