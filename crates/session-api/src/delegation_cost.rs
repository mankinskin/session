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

use serde::{Deserialize, Serialize};

use crate::{SessionRecord, SessionRole};

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
        }
        _ => unified,
    }
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
    "cat", "head", "tail", "grep", "find", "ls", "dir", "wc", "sed", "type", "more", "less",
];

/// Shell command heads that are exploratory directory/file search commands,
/// the subset of substitutable commands most associated with "locate a named
/// crate" scans (ticket `10d21210` threshold: fb14754e AC5 targets zero of
/// these on the benchmark).
const EXPLORATORY_FIND_LS_HEADS: &[&str] = &["find", "ls", "dir"];

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

#[cfg(test)]
#[path = "delegation_cost/tests.rs"]
mod tests;

#[path = "delegation_cost/attribution.rs"]
mod attribution;
#[allow(unused_imports)]
pub use attribution::*;

#[path = "delegation_cost/event_attribution.rs"]
mod event_attribution;
#[allow(unused_imports)]
pub use event_attribution::*;

#[path = "delegation_cost/shell_commands.rs"]
mod shell_commands;
#[allow(unused_imports)]
pub use shell_commands::*;
