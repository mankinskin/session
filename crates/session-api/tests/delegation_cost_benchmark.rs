//! Replay-only delegation-cost benchmark (ticket `10d21210`).
//!
//! Loads the two checked-in baseline sessions' raw event logs from
//! `.benchmark/10d21210/baseline/sessions/<id>/events.json` and asserts that
//! `compute_delegation_cost_report_from_events` reproduces the checked-in
//! `delegation_cost_report.json` exactly. This is the AC6 proof: the harness
//! is deterministic by construction (replay, not a live run), so re-running
//! against the unchanged repo reproduces the baseline metrics exactly — zero
//! spread.
//!
//! These two sessions only ever populated their `messages`/`transcript.json`
//! turns with user/assistant text (tool-call turns were never written there
//! — see `crate::hook::transcript::handle_message_event`), so the events
//! path is required to recover any delegation signal at all; see
//! `.benchmark/10d21210/README.md` for the full scenario definition, metric
//! set, thresholds, and the documented replay-only limitation.

use std::{
    collections::BTreeMap,
    path::PathBuf,
};

use session_api::{
    PersistedSessionEvents,
    compute_delegation_cost_report_from_events,
};

const BASELINE_SESSION_IDS: &[&str] = &[
    "3e9bc20b-4fe8-4996-ae7f-7be32525e429",
    "41966513-a8fa-4b44-98fa-9c57f0437cc0",
];

fn benchmark_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../.benchmark/10d21210/baseline")
}

fn checked_in_report_path() -> PathBuf {
    benchmark_root().join("delegation_cost_report.json")
}

fn load_events(session_id: &str) -> PersistedSessionEvents {
    let path = benchmark_root()
        .join("sessions")
        .join(session_id)
        .join("events.json");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!("failed to read {}: {err}", path.display())
    });
    serde_json::from_str(&raw).unwrap_or_else(|err| {
        panic!("failed to parse {}: {err}", path.display())
    })
}

fn compute_all() -> BTreeMap<String, serde_json::Value> {
    let mut computed = BTreeMap::new();
    for session_id in BASELINE_SESSION_IDS {
        let events = load_events(session_id);
        let report =
            compute_delegation_cost_report_from_events(session_id, &events);
        computed.insert(
            (*session_id).to_string(),
            serde_json::to_value(&report).expect("report serializes to JSON"),
        );
    }
    computed
}

#[test]
fn replay_reproduces_checked_in_baseline_report_exactly() {
    let root = benchmark_root();
    assert!(
        root.join("sessions").is_dir(),
        "expected checked-in baseline session fixtures at {}",
        root.display()
    );

    let computed = compute_all();

    let checked_in_raw = std::fs::read_to_string(checked_in_report_path())
        .unwrap_or_else(|err| {
            panic!(
                "failed to read checked-in baseline report at {}: {err}",
                checked_in_report_path().display()
            )
        });
    let checked_in: BTreeMap<String, serde_json::Value> =
        serde_json::from_str(&checked_in_raw)
            .expect("checked-in report is valid JSON");

    pretty_assertions::assert_eq!(
        computed,
        checked_in,
        "replay of the checked-in baseline event logs did not reproduce the checked-in \
         delegation_cost_report.json exactly; the harness is replay-only and must be \
         byte-identical on every run over an unchanged repo"
    );
}

#[test]
#[ignore = "one-off generator for the checked-in baseline artifact; run with --ignored after \
            intentionally changing the analyzer's metric set"]
fn generate_checked_in_baseline_report() {
    let computed = compute_all();
    let pretty =
        serde_json::to_string_pretty(&computed).expect("pretty json") + "\n";
    std::fs::write(checked_in_report_path(), pretty)
        .expect("write checked-in report");
}

/// Ticket `77eb143b` AC1/AC2: the classifier partitions every
/// `run_in_terminal` call into exactly one named taxonomy category, and the
/// per-category totals sum to the session's total `run_in_terminal` call
/// count (parent + sub-agent), reproducing the two-session baseline the
/// `10d21210` README documents (182 calls in `3e9bc20b…`, 152 in
/// `41966513…`).
#[test]
fn shell_command_categories_partition_every_terminal_call_per_session() {
    for session_id in BASELINE_SESSION_IDS {
        let events = load_events(session_id);
        let report =
            compute_delegation_cost_report_from_events(session_id, &events);

        let total_terminal_calls: u64 = report
            .parent_tools
            .get("run_in_terminal")
            .copied()
            .unwrap_or(0)
            + report
                .subagents
                .iter()
                .filter_map(|s| s.tools.get("run_in_terminal"))
                .sum::<u64>();
        let categorized_total: u64 =
            report.shell_command_categories.values().sum();
        assert_eq!(
            categorized_total, total_terminal_calls,
            "session {session_id}: shell_command_categories must partition every \
             run_in_terminal call exactly once"
        );
    }
}

/// Ticket `77eb143b` AC2/AC6: locks in the analyzer-computed per-category
/// breakdown on the checked-in baseline so a classifier regression is
/// caught by CI. These are the `77eb143b`-era analyzer numbers, not the
/// original ad-hoc-probe 116/298 figure — see `.benchmark/10d21210/README.md`
/// for why that number is not reproduced by this classifier (different
/// denominator/session scope), and `77eb143b`'s ticket notes for the full
/// resolution.
#[test]
fn cli_shadowing_and_cargo_run_cli_counts_match_baseline() {
    let session_a = "3e9bc20b-4fe8-4996-ae7f-7be32525e429";
    let session_b = "41966513-a8fa-4b44-98fa-9c57f0437cc0";

    let report_a = compute_delegation_cost_report_from_events(
        session_a,
        &load_events(session_a),
    );
    let report_b = compute_delegation_cost_report_from_events(
        session_b,
        &load_events(session_b),
    );

    assert_eq!(
        report_a.shell_command_categories.get("cli_shadowing_mcp"),
        Some(&18)
    );
    assert_eq!(
        report_a.shell_command_categories.get("cargo_run_cli"),
        Some(&16)
    );
    assert_eq!(
        report_b.shell_command_categories.get("cli_shadowing_mcp"),
        Some(&20)
    );
    assert_eq!(
        report_b.shell_command_categories.get("cargo_run_cli"),
        None,
        "session {session_b} has zero cargo run -p *-cli occurrences"
    );

    // AC6: no agent compiles a repo CLI at runtime to perform an operation
    // a loaded MCP tool already exposes — flagged non-zero here (16 on the
    // pre-fix baseline), reported honestly rather than asserted zero.
    let combined_cargo_run_cli = report_a
        .shell_command_categories
        .get("cargo_run_cli")
        .copied()
        .unwrap_or(0)
        + report_b
            .shell_command_categories
            .get("cargo_run_cli")
            .copied()
            .unwrap_or(0);
    assert_eq!(combined_cargo_run_cli, 16);

    // Sub-agent-level spot check: the worst-case "Materialize spec and
    // validation files" sub-agent from the epic ticket's own writeup is
    // where all 16 cargo_run_cli calls live.
    let worst_case = report_a
        .subagents
        .iter()
        .find(|s| {
            s.description.as_deref()
                == Some("Materialize spec and validation files")
        })
        .expect("worst-case sub-agent present in baseline");
    assert_eq!(
        worst_case.shell_command_categories.get("cargo_run_cli"),
        Some(&16)
    );
}

/// Ticket `77eb143b` AC3: the dominant cause of CLI-over-MCP preference is
/// tool-discovery failure (the MCP tool was never called in the same span),
/// not tool-failure fallback (the MCP tool was called and failed first),
/// on the checked-in baseline — reported per session with evidence.
#[test]
fn dominant_cause_is_discovery_failure_not_failure_fallback() {
    for session_id in BASELINE_SESSION_IDS {
        let events = load_events(session_id);
        let report =
            compute_delegation_cost_report_from_events(session_id, &events);
        assert!(
            report.mcp_tool_discovery_failure_count
                > report.mcp_tool_failure_fallback_count,
            "session {session_id}: expected tool-discovery failure ({}) to dominate over \
             tool-failure fallback ({}) as the cause of cli_shadowing_mcp shell calls",
            report.mcp_tool_discovery_failure_count,
            report.mcp_tool_failure_fallback_count
        );
    }
}
