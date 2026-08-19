//! Gate-owned environment wiring: reads every `COST_GATE_*` variable, builds
//! the default [`Policy`] the transport wires in, and provides the generic
//! JSONL telemetry writer the transport calls with its own telemetry record
//! type. Moved out of `mcp-toolmon`'s `main.rs` unchanged in semantics — the
//! variable names, defaults, and log lines are identical to before the split.

use std::{
    io::Write as _,
    path::PathBuf,
    sync::Arc,
};

use serde::Serialize;
use toolmon_policy_api::Policy;

use crate::{
    gate::{
        Gate,
        ModelBudgetCalibration,
    },
    policy_impl::CostGatePolicy,
};

fn log(msg: &str) {
    eprintln!("[mcp-toolmon] {msg}");
}

/// Load the cost gate from `COST_GATE_TABLE` (and friends), or `None` if the
/// table is unset/unreadable (fail-open passthrough).
pub fn load_gate() -> Option<Gate> {
    let table = std::env::var("COST_GATE_TABLE").ok().map(PathBuf::from)?;

    let scale_max = std::env::var("COST_GATE_SCALE_MAX")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(100);
    let budget_zero_price = std::env::var("COST_GATE_BUDGET_ZERO_PRICE")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(60.0);
    let calibration = ModelBudgetCalibration {
        scale_max,
        budget_zero_price,
    };

    let rollup_path = std::env::var("COST_GATE_TOOL_METRICS")
        .ok()
        .map(PathBuf::from);
    let grants_dir = std::env::var("COST_GATE_GRANTS_DIR")
        .ok()
        .map(PathBuf::from);

    match Gate::load(&table, calibration, rollup_path.as_deref(), grants_dir) {
        Ok(g) => {
            log(&format!(
                "enforcing graded cost model (table={}, scale_max={}, budget_zero_price={:.1})",
                table.display(),
                scale_max,
                budget_zero_price
            ));
            Some(g)
        },
        Err(e) => {
            log(&format!("disabled (fail-open): {e}"));
            None
        },
    }
}

/// Build the default `Arc<dyn Policy>` the transport wires in, or `None` when
/// the gate fails open (transparent passthrough).
pub fn build_policy_from_env() -> Option<Arc<dyn Policy>> {
    load_gate().map(|g| Arc::new(CostGatePolicy::new(g)) as Arc<dyn Policy>)
}

/// Path named by `COST_GATE_TELEMETRY_LOG`, or `None` when unset (telemetry
/// dropped silently, matching the other `COST_GATE_*` optional-config
/// conventions).
pub fn telemetry_log_path_from_env() -> Option<PathBuf> {
    std::env::var("COST_GATE_TELEMETRY_LOG")
        .ok()
        .map(PathBuf::from)
}

/// Append any `Serialize` telemetry record as a JSONL line to `path`. Generic
/// over the record type so this crate stays unaware of the transport's
/// concrete telemetry shape (`proxy::CallTelemetry`).
pub fn emit_telemetry_jsonl<T: Serialize>(
    path: Option<&PathBuf>,
    telemetry: &T,
) {
    let Some(path) = path else { return };
    let Ok(line) = serde_json::to_string(telemetry) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}
