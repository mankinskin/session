//! Delegation cost analyzer (ticket b7c61f0e).
//!
//! Promotes the ad-hoc `tmp/subagent_cost_probe.py` analysis into a supported,
//! tested report: per-sub-agent tool histograms, path-normalized duplicate
//! reads, duplicate command detection, failure classification, and — once
//! real usage flows through `data_json.usage` (ticket 9d527ad1) — real token
//! and cost figures per sub-agent instead of derived estimates.
//!
//! Sub-agent span attribution is resolved at capture time in
//! [`crate::hook::transcript`] via `parent_event_id` ancestry and stamped onto
//! [`crate::SessionTurnEventMeta::subagent_run_id`]. This means every event
//! belongs to exactly one owning span regardless of how parallel sub-agent
//! spans interleave in the flat transcript, so this module can group turns by
//! that key directly without re-deriving ancestry and without double-counting
//! overlapping spans.

use std::collections::BTreeMap;

use serde::{
    Deserialize,
    Serialize,
};

use crate::{
    SessionRecord,
    SessionRole,
};

/// Normalize a file path spelling for cross-agent duplicate-read detection.
///
/// Converts backslashes to forward slashes and lowercases a leading Windows
/// drive letter, so `C:\foo\bar` and `c:/foo/bar` dedupe to the same key.
pub fn normalize_path_for_dedup(path: &str) -> String {
    let unified = path.replace('\\', "/");
    let mut chars = unified.chars();
    match (chars.next(), chars.next()) {
        (Some(drive), Some(':')) if drive.is_ascii_alphabetic() => {
            let rest: String = chars.collect();
            format!("{}:{}", drive.to_ascii_lowercase(), rest)
        },
        _ => unified,
    }
}

fn normalize_command(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

const READ_TOOL_NAMES: &[&str] = &[
    "read_file",
    "peek_read",
    "peek_grep",
    "peek_count",
    "peek_skeleton",
];

const TERMINAL_TOOL_NAME: &str = "run_in_terminal";

/// Tool names whose failures count as "path-resolution failures" (ticket
/// `10d21210` threshold: fb14754e AC4 targets zero of these on the
/// benchmark).
const PATH_RESOLUTION_TOOL_NAMES: &[&str] = &["read_file", "list_dir"];

/// Shell command heads that have a dedicated first-class tool substitute
/// (`cat`/`head`/`tail`/`grep`/`find`/`ls` -> `read_file`/`peek_grep`/
/// `list_dir`, etc). A `run_in_terminal` command headed by one of these is
/// counted as a "substitutable shell command" (ticket `10d21210` threshold:
/// 77eb143b AC4 tracks this against a 116/298 baseline ratio).
const SUBSTITUTABLE_SHELL_HEADS: &[&str] = &[
    "cat", "head", "tail", "grep", "find", "ls", "dir", "wc", "sed", "type",
    "more", "less",
];

/// Shell command heads that are exploratory directory/file search commands,
/// the subset of substitutable commands most associated with "locate a named
/// crate" scans (ticket `10d21210` threshold: fb14754e AC5 targets zero of
/// these on the benchmark).
const EXPLORATORY_FIND_LS_HEADS: &[&str] = &["find", "ls", "dir"];

/// `run_in_terminal` command taxonomy (ticket `77eb143b` AC1). Every
/// classified command falls into exactly one category, in this priority
/// order: a read-like head wins over everything else, then a `cargo run -p
/// *-cli` invocation, then a CLI binary shadowing a loaded MCP tool, then
/// known-legitimate dev commands, else `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ShellCommandCategory {
    /// (a) `cat`/`head`/`tail`/`grep`/`find`/`ls`/... — substitutable by
    /// `peek-mcp`/`grep_search`/`read_file`.
    ReadLikeExploratory,
    /// (b) A CLI binary shadowing a loaded MCP tool (e.g. `ticket.exe get`
    /// where `ticket-mcp` is loaded).
    CliShadowingMcp,
    /// (c) `cargo run -p *-cli` — compiling/running a repo CLI at runtime
    /// instead of calling the equivalent MCP tool. Disallowed per AC6.
    CargoRunCli,
    /// (d) Legitimate dev commands: `cargo build`/`test`/`check`/`clippy`,
    /// `git`.
    LegitimateDev,
    /// Anything not matched by the above.
    Other,
}

impl ShellCommandCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            ShellCommandCategory::ReadLikeExploratory =>
                "read_like_exploratory",
            ShellCommandCategory::CliShadowingMcp => "cli_shadowing_mcp",
            ShellCommandCategory::CargoRunCli => "cargo_run_cli",
            ShellCommandCategory::LegitimateDev => "legitimate_dev",
            ShellCommandCategory::Other => "other",
        }
    }
}

/// Repo CLI binary basenames that shadow a loaded MCP tool family, mapped to
/// the MCP tool-name prefix an agent should have called instead (ticket
/// `77eb143b` AC1/AC3). Only binaries with a corresponding MCP surface
/// granted to the Implement Agent role are listed; extend this list as new
/// CLI/MCP pairs are added. Matching is against the invoked binary's
/// basename (after stripping any directory path), so `ticket`,
/// `./target/debug/ticket.exe`, and `c:/.../target/release/ticket.exe` all
/// classify identically.
const CLI_SHADOW_BASENAMES: &[(&str, &str)] = &[
    ("ticket", "mcp_ticket-mcp_"),
    ("ticket.exe", "mcp_ticket-mcp_"),
    ("spec", "mcp_spec-mcp_"),
    ("spec.exe", "mcp_spec-mcp_"),
    ("test.exe", "mcp_test-mcp_"),
    ("session", "mcp_session-mcp_"),
    ("session.exe", "mcp_session-mcp_"),
    ("context", "mcp_context-mcp_"),
    ("context.exe", "mcp_context-mcp_"),
];

/// Bare `test <subcommand>` invocations that are the `test-cli` binary
/// rather than the POSIX shell builtin `test` (as in `test -f file`,
/// `test ! -f file`). The bare basename `test` is deliberately excluded
/// from [`CLI_SHADOW_BASENAMES`] and handled separately via this allowlist
/// so shell-builtin file-existence checks are never miscounted as
/// `cli_shadowing_mcp` (ticket `77eb143b` AC1).
const TEST_CLI_SUBCOMMANDS: &[&str] = &[
    "list-specs",
    "get-spec",
    "record",
    "record-spec",
    "record-execution",
    "list-executions",
    "get-execution",
];

/// The invoked binary's basename: the final path component, so
/// `./target/debug/ticket.exe` and `ticket` both yield `ticket`/`ticket.exe`
/// respectively (directory separators only, extension is left intact so a
/// bare `test` command and a `test.exe` binary can be told apart).
fn command_basename(token: &str) -> &str {
    token.rsplit(['/', '\\']).next().unwrap_or(token)
}

/// The MCP tool-name prefix a CLI-shadowing shell command corresponds to, if
/// any, given its invoked binary `basename` and (for the ambiguous bare
/// `test` binary) its immediate next token.
fn mcp_family_for_command(
    basename: &str,
    second_token: Option<&str>,
) -> Option<&'static str> {
    if basename == "test" {
        return second_token
            .filter(|t| TEST_CLI_SUBCOMMANDS.contains(t))
            .map(|_| "mcp_test-mcp_");
    }
    CLI_SHADOW_BASENAMES
        .iter()
        .find(|(cli_basename, _)| *cli_basename == basename)
        .map(|(_, family)| *family)
}

/// Strip a leading `cd <path> &&`/`cd <path>;` chain from a captured shell
/// command. Captured sessions frequently re-`cd` to the workspace root
/// before the command that actually matters (shells are not guaranteed to
/// preserve `cwd` across turns); classifying by the first token alone would
/// otherwise misclassify `cd /repo && ./target/debug/ticket.exe get ...` as
/// an unclassified `cd` invocation instead of `cli_shadowing_mcp` (ticket
/// `77eb143b` AC1/AC2).
fn strip_leading_cd_chain(command: &str) -> &str {
    let mut rest = command.trim_start();
    while rest.starts_with("cd ") {
        if let Some(idx) = rest.find("&&") {
            rest = rest[idx + 2..].trim_start();
            continue;
        }
        if let Some(idx) = rest.find(';') {
            rest = rest[idx + 1..].trim_start();
            continue;
        }
        break;
    }
    rest
}

/// Whitespace-split tokens of a shell command, skipping a leading `rtk`
/// proxy prefix (see `AGENTS.md`'s `rtk <cmd>` convention).
fn normalized_tokens(command: &str) -> Vec<&str> {
    let mut tokens: Vec<&str> = command.split_whitespace().collect();
    if tokens.first() == Some(&"rtk") {
        tokens.remove(0);
    }
    tokens
}

/// The MCP tool-name prefix shadowed by `command`, if any, after stripping a
/// leading `cd` chain and `rtk` prefix and matching the invoked binary's
/// basename.
fn mcp_family_for_raw_command(command: &str) -> Option<&'static str> {
    let tokens = normalized_tokens(strip_leading_cd_chain(command));
    let basename = command_basename(*tokens.first()?);
    mcp_family_for_command(basename, tokens.get(1).copied())
}

/// True if `tokens` (already `cd`-chain- and `rtk`-stripped) invoke `cargo
/// run -p <crate>-cli` / `cargo run --package <crate>-cli` — compiling and
/// running a repo CLI binary at runtime instead of calling the equivalent
/// MCP tool (ticket `77eb143b` AC6).
fn is_cargo_run_cli(tokens: &[&str]) -> bool {
    tokens.iter().any(|t| *t == "run")
        && tokens.iter().enumerate().any(|(idx, t)| {
            (*t == "-p" || *t == "--package")
                && tokens
                    .get(idx + 1)
                    .map(|v| v.ends_with("-cli"))
                    .unwrap_or(false)
        })
}

/// Classify a raw `run_in_terminal` command into its taxonomy category
/// (ticket `77eb143b` AC1).
pub fn classify_shell_command(command: &str) -> ShellCommandCategory {
    // Every category — including the read-like check below — looks past a
    // leading `cd <path> &&`/`;` chain, so a path-qualified invocation
    // classifies the same as a bare one (ticket `77eb143b` defect 1: the
    // read-like branch previously ran `command_head` on the raw,
    // un-stripped command while every other branch stripped the `cd`
    // chain first, so `cd /repo && cat file.md` fell through past
    // `ReadLikeExploratory` into `Other` instead of being recognized as a
    // read-like command).
    let stripped = strip_leading_cd_chain(command);
    if let Some(head) = command_head(stripped) {
        if SUBSTITUTABLE_SHELL_HEADS.contains(&head) {
            return ShellCommandCategory::ReadLikeExploratory;
        }
    }

    let tokens = normalized_tokens(stripped);
    let Some(&head) = tokens.first() else {
        return ShellCommandCategory::Other;
    };
    let basename = command_basename(head);
    if basename == "cargo" {
        if is_cargo_run_cli(&tokens) {
            return ShellCommandCategory::CargoRunCli;
        }
        return ShellCommandCategory::LegitimateDev;
    }
    if basename == "git" {
        return ShellCommandCategory::LegitimateDev;
    }
    if mcp_family_for_command(basename, tokens.get(1).copied()).is_some() {
        return ShellCommandCategory::CliShadowingMcp;
    }
    ShellCommandCategory::Other
}

/// The first whitespace-separated token of a shell command, skipping a
/// leading `rtk` proxy prefix (see `AGENTS.md`'s `rtk <cmd>` convention) so
/// `rtk cat foo` and `cat foo` classify identically.
fn command_head(command: &str) -> Option<&str> {
    let mut parts = command.split_whitespace();
    let mut head = parts.next()?;
    if head == "rtk" {
        head = parts.next()?;
    }
    Some(head)
}

/// A repeated read or command within a single sub-agent's own span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepeatCount {
    pub key: String,
    pub count: u64,
}

/// A failed tool call observed within a sub-agent's span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegationFailure {
    pub tool_name: String,
    pub summary: String,
}

/// A duplicate artifact (file read or command) shared across more than one
/// sub-agent span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossAgentDuplicate {
    pub key: String,
    pub agent_count: usize,
    pub total_count: u64,
}

/// Per-sub-agent cost and waste attribution for a single delegation span.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SubAgentDelegationReport {
    /// The `tool_call_id` of the `runSubagent` invocation that opened this
    /// span; stable per delegation within the session.
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The model requested for this delegation, when declared by the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_model: Option<String>,
    /// The model that actually produced turns within this span (from each
    /// turn's own `model` field), when known. Falls back to `declared_model`
    /// only in the session-level `model_distribution` rollup, not here, so
    /// this field stays an honest "what we observed" signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_used: Option<String>,
    pub tool_call_count: u64,
    pub tools: BTreeMap<String, u64>,
    /// `run_in_terminal` calls issued inside this span, by taxonomy category
    /// (ticket `77eb143b` AC1/AC2). Keys are [`ShellCommandCategory::as_str`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub shell_command_categories: BTreeMap<String, u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repeat_reads: Vec<RepeatCount>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repeat_commands: Vec<RepeatCount>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<DelegationFailure>,
    /// Real token/cost attribution (ticket 9d527ad1), summed from turns whose
    /// `subagent_run_id` matches this span.
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

/// Per-session delegation cost report: the promoted equivalent of the
/// throwaway `tmp/subagent_cost_probe.py` analysis.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DelegationCostReport {
    pub session_id: String,
    pub subagent_count: usize,
    pub parent_tool_call_count: u64,
    pub parent_tools: BTreeMap<String, u64>,
    /// Parent-level (non-delegated) `run_in_terminal` calls, by taxonomy
    /// category (ticket `77eb143b` AC1/AC2).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parent_shell_command_categories: BTreeMap<String, u64>,
    pub subagents: Vec<SubAgentDelegationReport>,
    /// Files read by more than one distinct sub-agent, path-normalization
    /// safe.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cross_agent_duplicate_reads: Vec<CrossAgentDuplicate>,
    /// Commands run more than twice in total across sub-agents.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cross_agent_duplicate_commands: Vec<CrossAgentDuplicate>,
    /// Count of delegations by the model that produced them (`model_used`,
    /// falling back to `declared_model`, else the literal key `"unknown"`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_distribution: BTreeMap<String, u64>,
    /// `run_in_terminal` calls (parent or sub-agent) headed by a command that
    /// has a dedicated tool substitute (see [`SUBSTITUTABLE_SHELL_HEADS`]).
    pub substitutable_shell_count: u64,
    /// `run_in_terminal` calls headed by `find`/`ls`/`dir` (see
    /// [`EXPLORATORY_FIND_LS_HEADS`]); a subset of
    /// `substitutable_shell_count` most associated with exploratory crate
    /// location scans.
    pub exploratory_find_ls_count: u64,
    /// Failed `read_file`/`list_dir` calls across all spans (see
    /// [`PATH_RESOLUTION_TOOL_NAMES`]).
    pub path_resolution_failures: u64,
    /// Count of `runSubagent` dispatches sharing the same `(agent_name,
    /// description)` as an earlier dispatch in this session, where that
    /// earlier dispatch's span recorded at least one failure — a proxy for
    /// "re-dispatch of the same task after a blocked delegation". This is a
    /// heuristic keyed on exact description-text equality, not on the
    /// orchestrator's actual intent; see the benchmark README for the
    /// documented limitation.
    pub redispatch_count: u64,
    /// All `run_in_terminal` calls (parent + sub-agent), by taxonomy category
    /// (ticket `77eb143b` AC1/AC2). Keys are [`ShellCommandCategory::as_str`];
    /// this is `parent_shell_command_categories` plus every sub-agent's
    /// `shell_command_categories`, summed.
    pub shell_command_categories: BTreeMap<String, u64>,
    /// Count of `cli_shadowing_mcp` shell calls where the equivalent MCP
    /// tool family had already been called in the same span and recorded a
    /// failure — evidence the agent tried the MCP tool first and fell back
    /// to shell (ticket `77eb143b` AC3).
    pub mcp_tool_failure_fallback_count: u64,
    /// Count of `cli_shadowing_mcp` shell calls where the equivalent MCP
    /// tool family was never called anywhere in the same span — evidence the
    /// agent did not discover/use the loaded MCP tool at all (ticket
    /// `77eb143b` AC3).
    pub mcp_tool_discovery_failure_count: u64,
    /// Count of `cli_shadowing_mcp` shell calls where the equivalent MCP
    /// tool family had already been called successfully in the same span,
    /// yet the agent still shelled out — neither a failure fallback nor a
    /// discovery failure. Reported separately so it is not silently folded
    /// into either dominant-cause bucket (ticket `77eb143b` AC3).
    pub mcp_tool_shadow_ambiguous_count: u64,
}

const PARENT_BUCKET: &str = "__parent__";

/// Compute the delegation cost report for a single captured session.
///
/// A supported, tested replacement for the ad-hoc probe script: reproduces
/// per-sub-agent tool histograms, cross-agent duplicate-read detection
/// (path-normalization safe), duplicate-command detection, failure
/// classification, and real per-sub-agent token/cost totals when available.
pub fn compute_delegation_cost_report(
    record: &SessionRecord
) -> DelegationCostReport {
    // Discover agent_name/description/declared_model per run_id from the
    // `runSubagent` wrapper's own completion turn. That turn's own
    // `subagent_run_id` is its *parent's* span (it is a call the parent
    // made), but its `tool_call_id` names the span it opens for descendants.
    let mut agent_info: BTreeMap<
        String,
        (Option<String>, Option<String>, Option<String>),
    > = BTreeMap::new();
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
        agent_info
            .insert(run_id.clone(), (agent_name, description, declared_model));
    }

    let mut per_run: BTreeMap<String, SubAgentDelegationReport> =
        BTreeMap::new();
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
    let mut reads_by_key: BTreeMap<String, BTreeMap<String, u64>> =
        BTreeMap::new();
    let mut commands_by_key: BTreeMap<String, BTreeMap<String, u64>> =
        BTreeMap::new();

    // bucket (PARENT_BUCKET or a run_id) -> taxonomy category -> count
    // (ticket `77eb143b` AC1/AC2).
    let mut shell_categories_by_bucket: BTreeMap<
        String,
        BTreeMap<String, u64>,
    > = BTreeMap::new();
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
        let bucket =
            run_id.clone().unwrap_or_else(|| PARENT_BUCKET.to_string());

        match &run_id {
            None => {
                parent_tool_call_count += 1;
                *parent_tools.entry(tool_name.clone()).or_insert(0) += 1;
            },
            Some(rid) => {
                let entry = per_run.entry(rid.clone()).or_insert_with(|| {
                    let (agent_name, description, declared_model) = agent_info
                        .get(rid)
                        .cloned()
                        .unwrap_or((None, None, None));
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
            },
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
            if let Some(raw_command) =
                args.and_then(|a| a.get("command")).and_then(|v| v.as_str())
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
                    if let Some(family) =
                        mcp_family_for_raw_command(raw_command)
                    {
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
        let observed_model =
            turn.model.clone().or_else(|| meta.model_id.clone());
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
        entry.repeat_reads.sort_by(|a, b| {
            b.count.cmp(&a.count).then_with(|| a.key.cmp(&b.key))
        });
        entry.repeat_commands.sort_by(|a, b| {
            b.count.cmp(&a.count).then_with(|| a.key.cmp(&b.key))
        });
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
    let mut dispatch_groups: BTreeMap<
        (Option<String>, Option<String>),
        Vec<String>,
    > = BTreeMap::new();
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
        if let Some(categories) = shell_categories_by_bucket.get(&entry.run_id)
        {
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
    let subagents_by_run_id: BTreeMap<&str, &SubAgentDelegationReport> =
        subagents
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
                    let ever_called =
                        entry.tools.keys().any(|k| k.starts_with(family));
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

/// Compute the delegation cost report from a session's raw
/// [`crate::PersistedSessionEvents`] audit trail rather than its compacted
/// `messages` turns.
///
/// Some captured sessions only ever populate `messages`
/// (`SessionRecord::turns`) with `user`/`assistant` text turns — the
/// `tool.execution_start`/`tool.execution_complete` events that carry
/// sub-agent span, tool-call, and failure signals are recorded solely in the
/// raw event stream (see `crate::hook::transcript::handle_message_event`,
/// which only ever pushes `user.message`/`assistant.message` events into
/// `messages`). This reconstructs the missing tool-call turns from that
/// event stream, attributing each to its owning sub-agent span using the
/// same `parent_event_id` ancestry rule the live capture path uses for
/// `subagent_run_id` (see `crate::hook::transcript`), then hands the result
/// to the same tested [`compute_delegation_cost_report`] used everywhere
/// else — no metric logic is duplicated here.
pub fn compute_delegation_cost_report_from_events(
    session_id: &str,
    events: &crate::PersistedSessionEvents,
) -> DelegationCostReport {
    use chrono::Utc;

    // event_id -> the subagent_run_id (tool_call_id of the nearest enclosing
    // `runSubagent` span) that descendants of that event should inherit.
    let mut span_owner_by_event_id: BTreeMap<String, Option<String>> =
        BTreeMap::new();
    let mut turns: Vec<crate::SessionTurn> = Vec::new();

    for (sequence, event) in events.events.iter().enumerate() {
        let parent_owner = event
            .parent_event_id
            .as_ref()
            .and_then(|parent_id| {
                span_owner_by_event_id.get(parent_id.as_str()).cloned()
            })
            .flatten();

        let is_subagent_start = event.tool_name.as_deref()
            == Some("runSubagent")
            && matches!(
                event.event_type.as_deref(),
                Some("tool.execution_start") | Some("tool_execution_start")
            );
        let owner_for_descendants = if is_subagent_start {
            event.tool_call_id.clone()
        } else {
            parent_owner.clone()
        };
        if let Some(event_id) = &event.event_id {
            span_owner_by_event_id
                .insert(event_id.clone(), owner_for_descendants);
        }

        let is_tool_complete = matches!(
            event.event_type.as_deref(),
            Some("tool.execution_complete") | Some("tool_execution_complete")
        );
        if !is_subagent_start && !is_tool_complete {
            continue;
        }

        let data = event.data_json.as_ref();
        let tool_name = event.tool_name.clone().or_else(|| {
            data.and_then(|d| d.get("tool_name"))
                .and_then(|v| v.as_str())
                .map(String::from)
        });
        let Some(tool_name) = tool_name else {
            continue;
        };
        let tool_arguments_json = event
            .tool_arguments_json
            .clone()
            .or_else(|| data.and_then(|d| d.get("arguments")).cloned());
        let result_code = data
            .and_then(|d| d.get("result_code"))
            .and_then(|v| v.as_str())
            .map(String::from);
        let error_message = data
            .and_then(|d| d.get("error").or_else(|| d.get("message")))
            .and_then(|v| v.as_str())
            .map(String::from);

        turns.push(crate::SessionTurn {
            sequence,
            role: SessionRole::Tool,
            content: String::new(),
            captured_at: event.captured_at.unwrap_or_else(Utc::now),
            tool_name: Some(tool_name),
            model: None,
            event_meta: Some(crate::SessionTurnEventMeta {
                event_id: event.event_id.clone(),
                parent_event_id: event.parent_event_id.clone(),
                event_type: event.event_type.clone(),
                tool_call_id: event.tool_call_id.clone(),
                tool_success: event.tool_success,
                tool_arguments_json,
                result_code,
                error_message,
                subagent_run_id: parent_owner,
                ..Default::default()
            }),
        });
    }

    let record = crate::SessionRecord {
        schema_version: events.schema_version,
        session_id: session_id.to_string(),
        source: "events-replay".to_string(),
        started_at: events.captured_at,
        captured_at: events.captured_at,
        metadata: crate::SessionMetadata {
            workspace_slug: "default".to_string(),
            conversation_id: None,
            agent_id: None,
            ticket_id: None,
            model: None,
            trigger: None,
            provisioning: None,
            producer: None,
            copilot_version: None,
            vscode_version: None,
            protocol_version: None,
            worktree: None,
        },
        turns,
        links: crate::SessionLinks::default(),
        track_id: None,
        anchor_ticket_id: None,
        parent_session_id: None,
        spawned_session_id: None,
        emitted_handoff_ids: Vec::new(),
        picked_up_handoff_ids: Vec::new(),
    };

    compute_delegation_cost_report(&record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        SessionLinks,
        SessionMetadata,
        SessionTurn,
        SessionTurnEventMeta,
    };
    use chrono::Utc;

    fn base_meta() -> SessionTurnEventMeta {
        SessionTurnEventMeta::default()
    }

    fn record_with_turns(turns: Vec<SessionTurn>) -> SessionRecord {
        SessionRecord {
            schema_version: 1,
            session_id: "sess-1".to_string(),
            source: "test".to_string(),
            started_at: Utc::now(),
            captured_at: Utc::now(),
            metadata: SessionMetadata {
                workspace_slug: "test".to_string(),
                conversation_id: None,
                agent_id: None,
                ticket_id: None,
                model: None,
                trigger: None,
                provisioning: None,
                producer: None,
                copilot_version: None,
                vscode_version: None,
                protocol_version: None,
                worktree: None,
            },
            turns,
            links: SessionLinks::default(),
            track_id: None,
            anchor_ticket_id: None,
            parent_session_id: None,
            spawned_session_id: None,
            emitted_handoff_ids: Vec::new(),
            picked_up_handoff_ids: Vec::new(),
        }
    }

    fn tool_turn(
        sequence: usize,
        tool_name: &str,
        run_id: Option<&str>,
        args: serde_json::Value,
    ) -> SessionTurn {
        SessionTurn {
            sequence,
            role: SessionRole::Tool,
            content: "ok".to_string(),
            captured_at: Utc::now(),
            tool_name: Some(tool_name.to_string()),
            model: None,
            event_meta: Some(SessionTurnEventMeta {
                tool_success: Some(true),
                tool_arguments_json: Some(args),
                subagent_run_id: run_id.map(String::from),
                ..base_meta()
            }),
        }
    }

    #[test]
    fn normalizes_backslash_and_drive_letter_case_for_dedup() {
        assert_eq!(
            normalize_path_for_dedup("C:\\foo\\bar.md"),
            normalize_path_for_dedup("c:/foo/bar.md")
        );
        assert_eq!(normalize_path_for_dedup("c:/foo/bar.md"), "c:/foo/bar.md");
    }

    #[test]
    fn parallel_spans_are_attributed_without_double_counting() {
        // Two sub-agent spans dispatched in parallel: their tool calls
        // interleave in the flat transcript, but each turn's own
        // subagent_run_id (stamped from parent_event_id ancestry, not
        // index-range overlap) unambiguously identifies its owner.
        let turns = vec![
            tool_turn(
                0,
                "runSubagent",
                None,
                serde_json::json!({"agentName": "Explore", "description": "probe A"}),
            ),
            tool_turn(
                1,
                "runSubagent",
                None,
                serde_json::json!({"agentName": "Explore", "description": "probe B"}),
            ),
            tool_turn(
                2,
                "read_file",
                Some("call-a"),
                serde_json::json!({"filePath": "x.rs"}),
            ),
            tool_turn(
                3,
                "read_file",
                Some("call-b"),
                serde_json::json!({"filePath": "y.rs"}),
            ),
            tool_turn(
                4,
                "read_file",
                Some("call-a"),
                serde_json::json!({"filePath": "z.rs"}),
            ),
        ];
        // Re-key the wrapper turns' own tool_call_id so agent_info resolves.
        let mut turns = turns;
        turns[0].event_meta.as_mut().unwrap().tool_call_id =
            Some("call-a".to_string());
        turns[1].event_meta.as_mut().unwrap().tool_call_id =
            Some("call-b".to_string());

        let record = record_with_turns(turns);
        let report = compute_delegation_cost_report(&record);

        assert_eq!(report.subagent_count, 2);
        let call_a = report
            .subagents
            .iter()
            .find(|s| s.run_id == "call-a")
            .expect("call-a span");
        let call_b = report
            .subagents
            .iter()
            .find(|s| s.run_id == "call-b")
            .expect("call-b span");
        assert_eq!(call_a.tool_call_count, 2);
        assert_eq!(call_b.tool_call_count, 1);
        // No tool call is double-counted or dropped: 3 real reads total.
        assert_eq!(
            call_a.tool_call_count
                + call_b.tool_call_count
                + report.parent_tool_call_count,
            3
        );
    }

    #[test]
    fn duplicate_reads_are_path_normalization_safe() {
        let turns = vec![
            tool_turn(
                0,
                "read_file",
                Some("call-a"),
                serde_json::json!({"filePath": "C:\\repo\\notes.md"}),
            ),
            tool_turn(
                1,
                "read_file",
                Some("call-b"),
                serde_json::json!({"filePath": "c:/repo/notes.md"}),
            ),
        ];
        let record = record_with_turns(turns);
        let report = compute_delegation_cost_report(&record);

        assert_eq!(report.cross_agent_duplicate_reads.len(), 1);
        let dup = &report.cross_agent_duplicate_reads[0];
        assert_eq!(dup.agent_count, 2);
        assert_eq!(dup.total_count, 2);
        assert_eq!(dup.key, "c:/repo/notes.md");
    }

    #[test]
    fn failures_are_attributed_to_the_owning_span() {
        let mut turn = tool_turn(
            0,
            "run_in_terminal",
            Some("call-a"),
            serde_json::json!({"command": "cargo test"}),
        );
        turn.event_meta.as_mut().unwrap().tool_success = Some(false);
        turn.event_meta.as_mut().unwrap().result_code =
            Some("error".to_string());
        turn.event_meta.as_mut().unwrap().error_message =
            Some("compile error".to_string());

        let record = record_with_turns(vec![turn]);
        let report = compute_delegation_cost_report(&record);

        let span = &report.subagents[0];
        assert_eq!(span.failures.len(), 1);
        assert_eq!(span.failures[0].summary, "compile error");
    }

    #[test]
    fn real_token_and_cost_totals_flow_per_span() {
        let mut assistant_turn = SessionTurn {
            sequence: 0,
            role: SessionRole::Assistant,
            content: "work".to_string(),
            captured_at: Utc::now(),
            tool_name: None,
            model: Some("gpt-5".to_string()),
            event_meta: Some(SessionTurnEventMeta {
                input_tokens: Some(1000),
                output_tokens: Some(200),
                cost_usd: Some(0.05),
                subagent_run_id: Some("call-a".to_string()),
                ..base_meta()
            }),
        };
        assistant_turn.event_meta.as_mut().unwrap().model_id =
            Some("gpt-5".to_string());

        let record = record_with_turns(vec![assistant_turn]);
        let report = compute_delegation_cost_report(&record);

        assert_eq!(report.subagent_count, 1);
        let span = &report.subagents[0];
        assert_eq!(span.input_tokens, 1000);
        assert_eq!(span.output_tokens, 200);
        assert!((span.cost_usd.unwrap() - 0.05).abs() < 1e-9);
    }

    // -- classify_shell_command taxonomy (ticket 77eb143b AC1) --

    #[test]
    fn classifies_read_like_exploratory_heads() {
        assert_eq!(
            classify_shell_command("grep -n foo bar.rs"),
            ShellCommandCategory::ReadLikeExploratory
        );
        assert_eq!(
            classify_shell_command("ls -la .spec/specs/"),
            ShellCommandCategory::ReadLikeExploratory
        );
        assert_eq!(
            classify_shell_command("rtk cat foo.txt"),
            ShellCommandCategory::ReadLikeExploratory
        );
    }

    #[test]
    fn classifies_cli_shadowing_mcp_bare_and_path_qualified() {
        assert_eq!(
            classify_shell_command("ticket create --type defect --title x"),
            ShellCommandCategory::CliShadowingMcp
        );
        assert_eq!(
            classify_shell_command("spec get 7be68a48 --full 2>&1"),
            ShellCommandCategory::CliShadowingMcp
        );
        assert_eq!(
            classify_shell_command(
                "./target/debug/ticket.exe board check-in bd5e9aee"
            ),
            ShellCommandCategory::CliShadowingMcp
        );
        assert_eq!(
            classify_shell_command(
                "rtk c:/Users/linus/git/graph_app/context-engine/target/release/test.exe list-specs --workspace default"
            ),
            ShellCommandCategory::CliShadowingMcp
        );
    }

    #[test]
    fn strips_leading_cd_chain_before_classifying() {
        assert_eq!(
            classify_shell_command(
                "cd /c/Users/linus/git/graph_app/context-engine && ./target/debug/spec.exe health --all"
            ),
            ShellCommandCategory::CliShadowingMcp
        );
    }

    /// Regression test for ticket `77eb143b` defect 1: a `cd`-chained
    /// read-like command must classify as `ReadLikeExploratory`, not fall
    /// through to `Other`. This exact command shape (`cd <dir> && grep ...
    /// | head ...`) is drawn from the `3e9bc20b` baseline session log
    /// (`.session/sessions/3e9bc20b-4fe8-4996-ae7f-7be32525e429/events.json`,
    /// event `3fc6d1d5-1e14-4195-9d76-cf0ded4fa8ed`), one of the 13
    /// misclassified commands the bug caused across the two baseline
    /// sessions before this fix.
    #[test]
    fn cd_chained_read_like_command_classifies_as_read_like_not_other() {
        assert_eq!(
            classify_shell_command(
                "cd .session/sessions/aaf84892-09e4-4be6-be4c-b360b140b15e && grep -A 30 '### **validation**' transcript.json | head -40"
            ),
            ShellCommandCategory::ReadLikeExploratory
        );
        // Semicolon-separated cd chain, read-like via `cat`.
        assert_eq!(
            classify_shell_command("cd /repo; cat file.md"),
            ShellCommandCategory::ReadLikeExploratory
        );
    }

    #[test]
    fn bare_test_is_shell_builtin_not_test_cli_unless_known_subcommand() {
        // `test -f`/`test -d`/`test !` are the POSIX shell builtin, not the
        // test-cli binary, and must not be miscounted as cli_shadowing_mcp.
        assert_eq!(
            classify_shell_command(
                "test -f .spec/specs/x/body.md && echo exists"
            ),
            ShellCommandCategory::Other
        );
        assert_eq!(
            classify_shell_command("test ! -f tmp/scratch.md && echo gone"),
            ShellCommandCategory::Other
        );
        // A bare `test` invocation with a known test-cli subcommand is the
        // real CLI shadowing mcp_test-mcp_.
        assert_eq!(
            classify_shell_command("test list-specs --workspace default"),
            ShellCommandCategory::CliShadowingMcp
        );
    }

    #[test]
    fn classifies_cargo_run_cli_vs_legitimate_cargo() {
        assert_eq!(
            classify_shell_command("cargo run -p test-cli -- record --id foo"),
            ShellCommandCategory::CargoRunCli
        );
        assert_eq!(
            classify_shell_command(
                "cargo run --release --package spec-cli -- get 123"
            ),
            ShellCommandCategory::CargoRunCli
        );
        assert_eq!(
            classify_shell_command("cargo build -p session-api"),
            ShellCommandCategory::LegitimateDev
        );
        assert_eq!(
            classify_shell_command("rtk cargo test -p session-api"),
            ShellCommandCategory::LegitimateDev
        );
    }

    #[test]
    fn classifies_git_as_legitimate_dev() {
        assert_eq!(
            classify_shell_command("git status"),
            ShellCommandCategory::LegitimateDev
        );
        assert_eq!(
            classify_shell_command("rtk git log --oneline -5"),
            ShellCommandCategory::LegitimateDev
        );
    }

    #[test]
    fn classifies_unmatched_commands_as_other() {
        assert_eq!(
            classify_shell_command("python3 -c \"print(1)\""),
            ShellCommandCategory::Other
        );
        assert_eq!(
            classify_shell_command("mkdir -p .spec/specs/x"),
            ShellCommandCategory::Other
        );
    }

    #[test]
    fn shell_command_categories_partition_every_terminal_call() {
        let turns = vec![
            tool_turn(
                0,
                "runSubagent",
                None,
                serde_json::json!({"agentName": "Implement Agent", "description": "d"}),
            ),
            tool_turn(
                1,
                "run_in_terminal",
                Some("run-1"),
                serde_json::json!({"command": "grep -n foo bar.rs"}),
            ),
            tool_turn(
                2,
                "run_in_terminal",
                Some("run-1"),
                serde_json::json!({"command": "./target/debug/ticket.exe get abc"}),
            ),
            tool_turn(
                3,
                "run_in_terminal",
                Some("run-1"),
                serde_json::json!({"command": "cargo run -p test-cli -- record"}),
            ),
            tool_turn(
                4,
                "run_in_terminal",
                Some("run-1"),
                serde_json::json!({"command": "cargo build -p session-api"}),
            ),
            tool_turn(
                5,
                "run_in_terminal",
                Some("run-1"),
                serde_json::json!({"command": "python3 -c \"print(1)\""}),
            ),
        ];
        let mut turns = turns;
        turns[0].event_meta.as_mut().unwrap().tool_call_id =
            Some("run-1".to_string());

        let record = record_with_turns(turns);
        let report = compute_delegation_cost_report(&record);

        let span = &report.subagents[0];
        assert_eq!(
            span.shell_command_categories.get("read_like_exploratory"),
            Some(&1)
        );
        assert_eq!(
            span.shell_command_categories.get("cli_shadowing_mcp"),
            Some(&1)
        );
        assert_eq!(
            span.shell_command_categories.get("cargo_run_cli"),
            Some(&1)
        );
        assert_eq!(
            span.shell_command_categories.get("legitimate_dev"),
            Some(&1)
        );
        assert_eq!(span.shell_command_categories.get("other"), Some(&1));
        let total: u64 = report.shell_command_categories.values().sum();
        assert_eq!(total, 5);
        assert_eq!(report.mcp_tool_discovery_failure_count, 1);
        assert_eq!(report.mcp_tool_failure_fallback_count, 0);
    }
}
