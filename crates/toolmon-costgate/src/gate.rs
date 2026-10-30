//! Model-aware cost gate: the transport-agnostic decision core.
//!
//! Rust port of `tools/model-prices/cost_gate.py`. It resolves a model's
//! `output_mtok` from the shared price table (`model_prices.json`) and uses a
//! graded budget model with empirical tool costs to decide whether a tool may
//! run directly or must be delegated. Policy (see AGENTS.md):
//!
//! * base_budget(model): LINEAR inverse of output_mtok on a 1..100 scale
//! * tool cost: empirical rollup (if sufficient data) else static fallback
//! * offset grants: optional per-session/subagent budget boosts
//! * Decision: Allow if cost <= (base_budget + offset); otherwise Delegate

use std::path::Path;

use serde::Deserialize;
use toolmon_policy_api::Decision;

/// Default threshold on `output_mtok` (USD per 1M output tokens). Kept for
/// backward compatibility and as the `budget_zero_price` default.
pub const DEFAULT_THRESHOLD_X: f64 = 15.0;

/// Minimum call count for using empirical tool cost from rollup.
pub const MIN_CALLS: u64 = 1;

/// Model budget calibration for the graded cost scale.
#[derive(Debug, Clone, Copy)]
pub struct ModelBudgetCalibration {
    /// Maximum value of the budget scale (default 100).
    pub scale_max: u32,
    /// Price at which base_budget becomes zero (tunable anchor, default 60.0).
    pub budget_zero_price: f64,
}

impl Default for ModelBudgetCalibration {
    fn default() -> Self {
        Self {
            scale_max: 100,
            budget_zero_price: 60.0,
        }
    }
}

/// Per-tool token statistics from the T2 rollup.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolTokenStats {
    pub tool_name: String,
    pub call_count: u64,
    pub cost: Option<u32>,
}

/// Tool metrics report from the rollup.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ToolMetricsReport {
    #[serde(default)]
    pub tools: Vec<ToolTokenStats>,
}

/// Schema-versioned rollup.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolMetricsRollup {
    #[serde(default)]
    pub report: ToolMetricsReport,
}

/// Grant record for budget offsets.
#[derive(Debug, Clone, Deserialize)]
pub struct Grant {
    #[serde(default)]
    pub grant_id: String,
    #[serde(default)]
    pub offset: u32,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub expires_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PriceRow {
    provider_id: String,
    model_id: String,
    output_mtok: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct PriceTable {
    #[serde(default)]
    models: Vec<PriceRow>,
}

/// The loaded price table, calibration, optional rollup, and grants directory.
#[derive(Debug)]
pub struct Gate {
    models: Vec<PriceRow>,
    calibration: ModelBudgetCalibration,
    rollup: Option<ToolMetricsRollup>,
    grants_dir: Option<std::path::PathBuf>,
}

impl Gate {
    /// Build a gate from parsed model rows and optional rollup/grants.
    fn new(
        models: Vec<PriceRow>,
        calibration: ModelBudgetCalibration,
        rollup: Option<ToolMetricsRollup>,
        grants_dir: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            models,
            calibration,
            rollup,
            grants_dir,
        }
    }

    /// Load the price table from `path`. Returns an error string on failure so
    /// callers can decide to fail open.
    pub fn load(
        path: &Path,
        calibration: ModelBudgetCalibration,
        rollup_path: Option<&Path>,
        grants_dir: Option<std::path::PathBuf>,
    ) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let table: PriceTable = serde_json::from_str(&text)
            .map_err(|e| format!("bad JSON in {}: {e}", path.display()))?;

        let rollup = rollup_path
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|text| serde_json::from_str::<ToolMetricsRollup>(&text).ok());

        Ok(Self::new(table.models, calibration, rollup, grants_dir))
    }

    /// Resolve `output_mtok` for `model`, mirroring the Python gate: exact
    /// (case-insensitive) `model_id` match wins; otherwise a case-insensitive
    /// substring match on `provider_id`/`model_id`, taking the **maximum**
    /// price among matches so an ambiguous id is never cheaper than its most
    /// expensive variant. `None` when nothing matches.
    ///
    /// Both sides are compared in [`model_key`] form, so a hyphenated caller id
    /// (`gpt-6.1-sol`) matches a display-name row (`GPT-6.1 Sol`).
    pub fn resolve_output_mtok(&self, model: &str) -> Option<f64> {
        let key = model_key(model);

        let exact_max = self
            .models
            .iter()
            .filter(|r| model_key(&r.model_id) == key)
            .filter_map(|r| r.output_mtok)
            .fold(None, fold_max);
        if exact_max.is_some() {
            return exact_max;
        }

        self.models
            .iter()
            .filter(|r| {
                model_key(&r.provider_id).contains(&key) || model_key(&r.model_id).contains(&key)
            })
            .filter_map(|r| r.output_mtok)
            .fold(None, fold_max)
    }

    /// True when `model` resolves to a price-table entry via [`resolve_output_mtok`].
    pub fn resolves(&self, model: &str) -> bool {
        self.resolve_output_mtok(model).is_some()
    }

    /// Sorted, deduplicated [`model_key`] ids for rejection guidance. The
    /// `github-copilot` roster (the models actually offered to callers) is
    /// listed when present; otherwise every loaded id is listed.
    pub fn available_model_ids(&self) -> Vec<String> {
        let roster: Vec<&PriceRow> = self
            .models
            .iter()
            .filter(|r| r.provider_id == ROSTER_PROVIDER)
            .collect();
        let source: Vec<&PriceRow> = if roster.is_empty() {
            self.models.iter().collect()
        } else {
            roster
        };
        let mut ids: Vec<String> = source.iter().map(|r| model_key(&r.model_id)).collect();
        ids.sort();
        ids.dedup();
        ids
    }

    /// Compute base_budget from model's output_mtok using linear inverse mapping.
    /// Returns a value in [0, scale_max]. Unknown model → 0 (conservative).
    pub fn base_budget(&self, model: &str) -> u32 {
        let Some(out) = self.resolve_output_mtok(model) else {
            return 0;
        };
        let ratio = 1.0 - (out / self.calibration.budget_zero_price);
        let scaled = (ratio * self.calibration.scale_max as f64).round();
        scaled.clamp(0.0, self.calibration.scale_max as f64) as u32
    }

    /// Resolve tool cost from empirical rollup. Fail open (return 0) for unmeasured tools.
    fn tool_cost(&self, tool: &str) -> u32 {
        let Some(rollup) = &self.rollup else {
            return 0;
        };
        let tool_low = tool.to_lowercase();

        // Try exact match first (case-insensitive)
        let exact_match = rollup.report.tools.iter().find(|t| {
            t.tool_name.to_lowercase() == tool_low && t.call_count >= MIN_CALLS && t.cost.is_some()
        });
        if let Some(entry) = exact_match {
            return entry.cost.unwrap_or(0);
        }

        // Fall back to bidirectional substring matching
        let matches: Vec<_> = rollup
            .report
            .tools
            .iter()
            .filter(|t| {
                let name_low = t.tool_name.to_lowercase();
                name_low.contains(&tool_low) || tool_low.contains(&name_low)
            })
            .filter(|t| t.call_count >= MIN_CALLS && t.cost.is_some())
            .collect();
        if !matches.is_empty() {
            matches.iter().filter_map(|t| t.cost).max().unwrap_or(0)
        } else {
            0
        }
    }

    /// Load grant offset from grants_dir/<grant_id>.json. Returns 0 on any error.
    fn load_grant_offset(&self, grant_id: &str, model: &str) -> u32 {
        let Some(dir) = &self.grants_dir else {
            return 0;
        };
        let path = dir.join(format!("{}.json", grant_id));
        let Ok(text) = std::fs::read_to_string(&path) else {
            return 0;
        };
        let Ok(grant) = serde_json::from_str::<Grant>(&text) else {
            return 0;
        };
        // Check expiry (RFC3339)
        if let Some(expires) = &grant.expires_at {
            if let Ok(exp_time) = chrono::DateTime::parse_from_rfc3339(expires) {
                if exp_time < chrono::Utc::now() {
                    return 0;
                }
            }
        }
        // Check model match (case-insensitive)
        if let Some(grant_model) = &grant.model {
            if grant_model.to_lowercase() != model.to_lowercase() {
                return 0;
            }
        }
        grant.offset
    }

    /// True when `output_mtok` is strictly greater than the threshold (legacy).
    pub fn is_orchestrator(&self, output_mtok: f64) -> bool {
        output_mtok > DEFAULT_THRESHOLD_X
    }

    /// Decide whether `model` may call `tool` directly, with optional grant_id.
    ///
    /// * Compute: base_budget, tool_cost, offset.
    /// * effective = base_budget + offset (capped at 2*scale_max).
    /// * Allow if cost <= effective; else Delegate with guidance.
    pub fn evaluate(&self, model: &str, tool: &str, grant_id: Option<&str>) -> Decision {
        // Reject an unresolvable caller_model before any budget math. An unknown
        // id must never fall through to a zero budget: that silently disables
        // price-awareness enforcement instead of surfacing the mistake.
        if self.resolve_output_mtok(model).is_none() {
            return Decision::Reject {
                guidance: unknown_model_guidance(model, &self.available_model_ids()),
            };
        }
        let tool_cost = self.tool_cost(tool);
        if tool_cost == 0 {
            return Decision::Allow;
        }
        let base = self.base_budget(model);
        let offset = grant_id.map_or(0, |gid| self.load_grant_offset(gid, model));
        let effective = (base + offset).min(2 * self.calibration.scale_max);
        if tool_cost <= effective {
            Decision::Allow
        } else {
            Decision::Delegate {
                guidance: format!(
                    "Tool '{}' requires cost {} but model '{}' has effective budget {} \
                     (base {} + offset {}). An offset grant or delegation to a cheaper model \
                     is required. Delegate via runSubagent(model=<cheaper>, ...).",
                    tool, tool_cost, model, effective, base, offset
                ),
            }
        }
    }

    /// Legacy evaluate without grant_id (for backward compatibility).
    pub fn evaluate_legacy(&self, model: &str, tool: &str) -> Decision {
        self.evaluate(model, tool, None)
    }
}

const ROSTER_PROVIDER: &str = "github-copilot";

/// Separator-insensitive comparison key: trimmed, lowercased, with spaces and
/// underscores folded to hyphens (`"GPT-6.1 Sol"` -> `"gpt-6.1-sol"`).
pub fn model_key(id: &str) -> String {
    id.trim()
        .chars()
        .map(|c| if c == ' ' || c == '_' { '-' } else { c })
        .collect::<String>()
        .to_lowercase()
}

fn fold_max(acc: Option<f64>, v: f64) -> Option<f64> {
    Some(match acc {
        Some(a) if a >= v => a,
        _ => v,
    })
}

/// Guidance returned when `caller_model` cannot be resolved in the price table,
/// even after the transport wrapper's fallback normalization (trailing
/// parenthetical qualifier stripped; spaces/underscores folded to hyphens).
/// `available` is the compact list of known `model_id`s, sourced from
/// [`Gate::available_model_ids`].
pub fn unknown_model_guidance(model: &str, available: &[String]) -> String {
    let family = model_key(model)
        .split('-')
        .next()
        .unwrap_or_default()
        .to_string();
    let same_family: Vec<&String> = available
        .iter()
        .filter(|id| !family.is_empty() && id.split('-').next() == Some(family.as_str()))
        .collect();
    let candidates: Vec<&String> = if same_family.is_empty() {
        available.iter().collect()
    } else {
        same_family
    };
    let sample = if candidates.is_empty() {
        "(no models loaded)".to_string()
    } else {
        candidates
            .iter()
            .take(24)
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "Unknown caller_model '{model}': it does not match any model_id in the \
         price table, even after normalizing common deviations (stripping a \
         trailing client qualifier such as '(copilot)', and folding spaces/ \
         underscores to hyphens). Pass the actual id of the model issuing this \
         call (its real price-table model_id, e.g. claude-opus-5, claude-sonnet-5, \
         gpt-5.3-codex, ...), not a generic vendor or product label such as \
         'github-copilot', 'copilot', 'openai', or 'anthropic'. Detected available \
         models: {sample}. An unrecognized caller_model is refused so \
         price-awareness enforcement is never silently bypassed."
    )
}

/// Delegation guidance returned when an orchestrator-tier model calls a
/// token-heavy tool.
pub fn delegation_guidance(model: &str, tool: &str, x: f64) -> String {
    format!(
        "Model '{model}' exceeds the orchestrator threshold (output_mtok > {x} \
         USD/1M). Do not call the token-heavy tool '{tool}' directly. Delegate \
         it to a cheaper sub-agent via runSubagent(model=<cheaper>, ...) and \
         aggregate the result. Reserve this model for strategic decisions, \
         code/change planning, and tool-call planning."
    )
}

#[cfg(test)]
#[path = "gate/tests.rs"]
mod tests;
