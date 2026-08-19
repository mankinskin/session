//! The `verdict` CLI subcommand: print the gate decision for a `(model, tool)`
//! pair. Moved out of `mcp-toolmon`'s `main.rs` with identical CLI surface
//! and output text; `main.rs` now just forwards argv here.

use std::path::PathBuf;

use toolmon_policy_api::Decision;

use crate::gate::{
    Gate,
    ModelBudgetCalibration,
};

/// Parse a flag value: --flag <value>
fn parse_flag<'a>(
    argv: &'a [String],
    flag: &str,
) -> Option<&'a str> {
    argv.iter()
        .position(|arg| arg == flag)
        .and_then(|pos| argv.get(pos + 1))
        .map(String::as_str)
}

/// verdict subcommand: print the gate decision for a given (model, tool) pair.
pub fn run_verdict(argv: &[String]) {
    let model = parse_flag(argv, "--model").unwrap_or("");
    let tool = parse_flag(argv, "--tool").unwrap_or("");
    let table_path = parse_flag(argv, "--table").unwrap_or("");
    let rollup_path = parse_flag(argv, "--rollup");
    let grant_id = parse_flag(argv, "--grant");

    if model.is_empty() || tool.is_empty() || table_path.is_empty() {
        eprintln!(
            "usage: mcp-toolmon verdict --model <model> --tool <tool> --table <path> [--rollup <path>] [--grant <id>]"
        );
        std::process::exit(2);
    }

    let calibration = ModelBudgetCalibration::default();
    let rollup_path_buf = rollup_path.map(PathBuf::from);
    let gate = match Gate::load(
        &PathBuf::from(table_path),
        calibration,
        rollup_path_buf.as_deref(),
        None,
    ) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("error loading gate: {e}");
            std::process::exit(1);
        },
    };

    let decision = gate.evaluate(model, tool, grant_id);
    match decision {
        Decision::Allow => println!("Allow"),
        Decision::Delegate { guidance } => println!("Delegate: {guidance}"),
        Decision::Reject { guidance } => println!("Reject: {guidance}"),
    }
}
