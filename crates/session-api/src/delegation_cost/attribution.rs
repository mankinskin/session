#[allow(unused_imports)]
use super::*;

/// Compute the delegation cost report for a single captured session.
///
/// A supported, tested replacement for the ad-hoc probe script: reproduces
/// per-sub-agent tool histograms, cross-agent duplicate-read detection
/// (path-normalization safe), duplicate-command detection, failure
/// classification, and real per-sub-agent token/cost totals when available.
pub fn compute_delegation_cost_report(record: &SessionRecord) -> DelegationCostReport {
    // Discover agent_name/description/declared_model per run_id from the
    // `runSubagent` wrapper's own completion turn. That turn's own
    // `subagent_run_id` is its *parent's* span (it is a call the parent
    // made), but its `tool_call_id` names the span it opens for descendants.
    let mut agent_info: BTreeMap<String, (Option<String>, Option<String>, Option<String>)> =
        BTreeMap::new();
    for turn in &record.turns {
        if turn.tool_name.as_deref() != Some("runSubagent") {
            continue;
        }
        let Some(meta) = &turn.event_meta else {
            continue;
        };
        let Some(run_id) = &meta.tool_call_id else {
            continue;
        };
        let args = meta.tool_arguments_json.as_ref();
        let agent_name = args
            .and_then(|v| v.get("agentName"))
            .and_then(|v| v.as_str())
            .map(String::from);
        let description = args
            .and_then(|v| v.get("description"))
            .and_then(|v| v.as_str())
            .map(String::from);
        let declared_model = args
            .and_then(|v| v.get("model"))
            .and_then(|v| v.as_str())
            .map(String::from);
        agent_info.insert(run_id.clone(), (agent_name, description, declared_model));
    }

    let mut per_run: BTreeMap<String, SubAgentDelegationReport> = BTreeMap::new();
    let mut parent_tool_call_count = 0u64;
    let mut parent_tools: BTreeMap<String, u64> = BTreeMap::new();
    let mut path_resolution_failures = 0u64;
    let mut substitutable_shell_count = 0u64;
    let mut exploratory_find_ls_count = 0u64;
    // run_id -> first-observed model that produced a turn in that span,
    // from each turn's own `model` field (see [`SessionTurn::model`]).
    let mut run_model: BTreeMap<String, String> = BTreeMap::new();
    // Dispatch order of `runSubagent` calls (by turn sequence), used to
    // compute `redispatch_count` after `per_run` is fully populated.
    let mut dispatch_order: Vec<String> = Vec::new();

    // path/command -> run bucket (PARENT_BUCKET or a run_id) -> count, used
    // for both within-agent repeat detection and cross-agent duplicates.
    let mut reads_by_key: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    let mut commands_by_key: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();

    // bucket (PARENT_BUCKET or a run_id) -> taxonomy category -> count
    // (ticket `77eb143b` AC1/AC2).
    let mut shell_categories_by_bucket: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    let mut shell_command_categories: BTreeMap<String, u64> = BTreeMap::new();
    // Ordered occurrences of CLI-shadowing-MCP shell commands, as
    // (bucket, mcp_family_prefix), used to compute the AC3 dominant-cause
    // split once `per_run`'s tool/failure histograms are fully populated.
    let mut cli_shadow_occurrences: Vec<(String, &'static str)> = Vec::new();

    for turn in &record.turns {
        if turn.tool_name.as_deref() != Some("runSubagent") {
            continue;
        }
        if let Some(meta) = &turn.event_meta {
            if let Some(run_id) = &meta.tool_call_id {
                dispatch_order.push(run_id.clone());
            }
        }
    }

    for turn in &record.turns {
        if turn.role != SessionRole::Tool {
            continue;
        }
        let Some(tool_name) = &turn.tool_name else {
            continue;
        };
        // The runSubagent wrapper call itself is a structural marker, not a
        // real tool call performed by an agent; exclude it from all tallies
        // (its span boundary is tracked separately via agent_info).
        if tool_name == "runSubagent" {
            continue;
        }

        let meta = turn.event_meta.as_ref();
        let run_id = meta.and_then(|m| m.subagent_run_id.clone());
        let bucket = run_id.clone().unwrap_or_else(|| PARENT_BUCKET.to_string());

        match &run_id {
            None => {
                parent_tool_call_count += 1;
                *parent_tools.entry(tool_name.clone()).or_insert(0) += 1;
            }
            Some(rid) => {
                let entry = per_run.entry(rid.clone()).or_insert_with(|| {
                    let (agent_name, description, declared_model) =
                        agent_info.get(rid).cloned().unwrap_or((None, None, None));
                    SubAgentDelegationReport {
                        run_id: rid.clone(),
                        agent_name,
                        description,
                        declared_model,
                        ..Default::default()
                    }
                });
                entry.tool_call_count += 1;
                *entry.tools.entry(tool_name.clone()).or_insert(0) += 1;
            }
        }

        let is_failure = meta
            .map(|m| {
                matches!(
                    m.result_code.as_deref(),
                    Some("error") | Some("timeout") | Some("hang")
                ) || m.tool_success == Some(false)
            })
            .unwrap_or(false);
        if is_failure {
            if PATH_RESOLUTION_TOOL_NAMES.contains(&tool_name.as_str()) {
                path_resolution_failures += 1;
            }
            if let Some(rid) = &run_id {
                let summary = meta
                    .and_then(|m| m.error_message.clone())
                    .unwrap_or_else(|| "tool call failed".to_string());
                if let Some(entry) = per_run.get_mut(rid) {
                    entry.failures.push(DelegationFailure {
                        tool_name: tool_name.clone(),
                        summary,
                    });
                }
            }
        }

        let args = meta.and_then(|m| m.tool_arguments_json.as_ref());
        if READ_TOOL_NAMES.contains(&tool_name.as_str()) {
            if let Some(raw_path) = args
                .and_then(|a| a.get("filePath").or_else(|| a.get("path")))
                .and_then(|v| v.as_str())
            {
                let key = normalize_path_for_dedup(raw_path);
                *reads_by_key
                    .entry(key)
                    .or_default()
                    .entry(bucket.clone())
                    .or_insert(0) += 1;
            }
        } else if tool_name == TERMINAL_TOOL_NAME {
            if let Some(raw_command) = args.and_then(|a| a.get("command")).and_then(|v| v.as_str())
            {
                if let Some(head) = command_head(raw_command) {
                    if SUBSTITUTABLE_SHELL_HEADS.contains(&head) {
                        substitutable_shell_count += 1;
                    }
                    if EXPLORATORY_FIND_LS_HEADS.contains(&head) {
                        exploratory_find_ls_count += 1;
                    }
                }
                let category = classify_shell_command(raw_command);
                *shell_command_categories
                    .entry(category.as_str().to_string())
                    .or_insert(0) += 1;
                *shell_categories_by_bucket
                    .entry(bucket.clone())
                    .or_default()
                    .entry(category.as_str().to_string())
                    .or_insert(0) += 1;
                if category == ShellCommandCategory::CliShadowingMcp {
                    if let Some(family) = mcp_family_for_raw_command(raw_command) {
                        cli_shadow_occurrences.push((bucket.clone(), family));
                    }
                }
                let key = normalize_command(raw_command);
                *commands_by_key
                    .entry(key)
                    .or_default()
                    .entry(bucket)
                    .or_insert(0) += 1;
            }
        }
    }

    // Token/cost attribution: usage is recorded per-turn (typically on
    // assistant turns), so walk every turn regardless of role. A span may
    // consist entirely of assistant turns with no tool calls, so ensure its
    // entry exists here rather than assuming the tool-call loop above
    // already created it.
    for turn in &record.turns {
        let Some(meta) = &turn.event_meta else {
            continue;
        };
        let Some(rid) = &meta.subagent_run_id else {
            continue;
        };
        let entry = per_run.entry(rid.clone()).or_insert_with(|| {
            let (agent_name, description, declared_model) =
                agent_info.get(rid).cloned().unwrap_or((None, None, None));
            SubAgentDelegationReport {
                run_id: rid.clone(),
                agent_name,
                description,
                declared_model,
                ..Default::default()
            }
        });
        entry.input_tokens += meta.input_tokens.unwrap_or(0);
        entry.output_tokens += meta.output_tokens.unwrap_or(0);
        entry.cache_read_tokens += meta.cache_read_tokens.unwrap_or(0);
        entry.cache_write_tokens += meta.cache_write_tokens.unwrap_or(0);
        if let Some(cost) = meta.cost_usd {
            entry.cost_usd = Some(entry.cost_usd.unwrap_or(0.0) + cost);
        }
        let observed_model = turn.model.clone().or_else(|| meta.model_id.clone());
        if let Some(model) = observed_model {
            run_model.entry(rid.clone()).or_insert(model);
        }
    }
    for (rid, entry) in per_run.iter_mut() {
        entry.model_used = run_model.get(rid).cloned();
    }

    // Within-agent repeats (count > 1 for that agent's own bucket).
    for (path, by_bucket) in &reads_by_key {
        for (bucket, count) in by_bucket {
            if *count > 1 && bucket != PARENT_BUCKET {
                if let Some(entry) = per_run.get_mut(bucket) {
                    entry.repeat_reads.push(RepeatCount {
                        key: path.clone(),
                        count: *count,
                    });
                }
            }
        }
    }
    for (command, by_bucket) in &commands_by_key {
        for (bucket, count) in by_bucket {
            if *count > 1 && bucket != PARENT_BUCKET {
                if let Some(entry) = per_run.get_mut(bucket) {
                    entry.repeat_commands.push(RepeatCount {
                        key: command.clone(),
                        count: *count,
                    });
                }
            }
        }
    }
    for entry in per_run.values_mut() {
        entry
            .repeat_reads
            .sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.key.cmp(&b.key)));
        entry
            .repeat_commands
            .sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.key.cmp(&b.key)));
    }

    // Cross-agent duplicates: read/run by more than one distinct sub-agent.
    let mut cross_agent_duplicate_reads = Vec::new();
    for (path, by_bucket) in &reads_by_key {
        let agent_buckets = by_bucket
            .iter()
            .filter(|(bucket, _)| bucket.as_str() != PARENT_BUCKET)
            .count();
        if agent_buckets > 1 {
            let total_count: u64 = by_bucket
                .iter()
                .filter(|(bucket, _)| bucket.as_str() != PARENT_BUCKET)
                .map(|(_, count)| *count)
                .sum();
            cross_agent_duplicate_reads.push(CrossAgentDuplicate {
                key: path.clone(),
                agent_count: agent_buckets,
                total_count,
            });
        }
    }
    cross_agent_duplicate_reads.sort_by(|a, b| {
        b.agent_count
            .cmp(&a.agent_count)
            .then_with(|| b.total_count.cmp(&a.total_count))
            .then_with(|| a.key.cmp(&b.key))
    });

    let mut cross_agent_duplicate_commands = Vec::new();
    for (command, by_bucket) in &commands_by_key {
        let agent_count = by_bucket
            .iter()
            .filter(|(bucket, _)| bucket.as_str() != PARENT_BUCKET)
            .count();
        let total_count: u64 = by_bucket.values().sum();
        if total_count > 2 {
            cross_agent_duplicate_commands.push(CrossAgentDuplicate {
                key: command.clone(),
                agent_count,
                total_count,
            });
        }
    }
    cross_agent_duplicate_commands.sort_by(|a, b| {
        b.total_count
            .cmp(&a.total_count)
            .then_with(|| a.key.cmp(&b.key))
    });

    // Model distribution: one vote per delegation, keyed by the model that
    // actually produced its turns, falling back to the declared model, else
    // "unknown".
    let mut model_distribution: BTreeMap<String, u64> = BTreeMap::new();
    for entry in per_run.values() {
        let key = entry
            .model_used
            .clone()
            .or_else(|| entry.declared_model.clone())
            .unwrap_or_else(|| "unknown".to_string());
        *model_distribution.entry(key).or_insert(0) += 1;
    }

    // Re-dispatch detection: group dispatches by (agent_name, description)
    // in the order they were issued, and count every dispatch after the
    // first in a group where an earlier dispatch in that same group already
    // recorded a failure.
    let mut dispatch_groups: BTreeMap<(Option<String>, Option<String>), Vec<String>> =
        BTreeMap::new();
    for run_id in &dispatch_order {
        let (agent_name, description, _) = agent_info
            .get(run_id)
            .cloned()
            .unwrap_or((None, None, None));
        dispatch_groups
            .entry((agent_name, description))
            .or_default()
            .push(run_id.clone());
    }
    let mut redispatch_count = 0u64;
    for run_ids in dispatch_groups.values() {
        if run_ids.len() < 2 {
            continue;
        }
        let mut any_failure_so_far = per_run
            .get(&run_ids[0])
            .map(|s| !s.failures.is_empty())
            .unwrap_or(false);
        for run_id in &run_ids[1..] {
            if any_failure_so_far {
                redispatch_count += 1;
            }
            any_failure_so_far = any_failure_so_far
                || per_run
                    .get(run_id)
                    .map(|s| !s.failures.is_empty())
                    .unwrap_or(false);
        }
    }

    let mut subagents: Vec<_> = per_run.into_values().collect();

    // Attach each sub-agent's own shell-command-category breakdown (ticket
    // `77eb143b` AC1/AC2).
    for entry in subagents.iter_mut() {
        if let Some(categories) = shell_categories_by_bucket.get(&entry.run_id) {
            entry.shell_command_categories = categories.clone();
        }
    }
    let parent_shell_command_categories = shell_categories_by_bucket
        .get(PARENT_BUCKET)
        .cloned()
        .unwrap_or_default();

    // AC3 dominant-cause split: for every `cli_shadowing_mcp` shell
    // occurrence, determine whether the equivalent MCP tool family was
    // called-and-failed in the same span (tool-failure fallback), never
    // called at all in the same span (tool-discovery failure), or called
    // and succeeded yet the agent still shelled out anyway (ambiguous —
    // reported separately, not folded into either dominant-cause bucket).
    let subagents_by_run_id: BTreeMap<&str, &SubAgentDelegationReport> = subagents
        .iter()
        .map(|entry| (entry.run_id.as_str(), entry))
        .collect();
    let mut mcp_tool_failure_fallback_count = 0u64;
    let mut mcp_tool_discovery_failure_count = 0u64;
    let mut mcp_tool_shadow_ambiguous_count = 0u64;
    for (bucket, family) in &cli_shadow_occurrences {
        let (ever_called, ever_failed) = if bucket == PARENT_BUCKET {
            // KNOWN LIMITATION (ticket `77eb143b` review): parent-level tool
            // calls are only tallied into `parent_tools` (a call-count map),
            // not into a `DelegationFailure` log like sub-agent spans get, so
            // there is no parent-level failure history to consult here.
            // `ever_failed` is hardcoded `false`, meaning a parent-level
            // `cli_shadowing_mcp` call can only ever land in
            // `mcp_tool_discovery_failure_count` or
            // `mcp_tool_shadow_ambiguous_count`, never
            // `mcp_tool_failure_fallback_count` — a small undercount risk on
            // the AC3 discovery-vs-fallback split. Tracking parent-level
            // failures would require adding a `parent_failures` log parallel
            // to `SubAgentDelegationReport::failures`; deferred as it does
            // not change the split's 36:1 conclusion (all `cli_shadowing_mcp`
            // occurrences in the analyzed baseline are sub-agent-bucketed).
            (parent_tools.keys().any(|k| k.starts_with(family)), false)
        } else {
            subagents_by_run_id
                .get(bucket.as_str())
                .map(|entry| {
                    let ever_called = entry.tools.keys().any(|k| k.starts_with(family));
                    let ever_failed = entry
                        .failures
                        .iter()
                        .any(|f| f.tool_name.starts_with(family));
                    (ever_called, ever_failed)
                })
                .unwrap_or((false, false))
        };
        if !ever_called {
            mcp_tool_discovery_failure_count += 1;
        } else if ever_failed {
            mcp_tool_failure_fallback_count += 1;
        } else {
            mcp_tool_shadow_ambiguous_count += 1;
        }
    }

    DelegationCostReport {
        session_id: record.session_id.clone(),
        subagent_count: subagents.len(),
        parent_tool_call_count,
        parent_tools,
        parent_shell_command_categories,
        subagents,
        cross_agent_duplicate_reads,
        cross_agent_duplicate_commands,
        model_distribution,
        substitutable_shell_count,
        exploratory_find_ls_count,
        path_resolution_failures,
        redispatch_count,
        shell_command_categories,
        mcp_tool_failure_fallback_count,
        mcp_tool_discovery_failure_count,
        mcp_tool_shadow_ambiguous_count,
    }
}
