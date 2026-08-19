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
            .and_then(|text| {
                serde_json::from_str::<ToolMetricsRollup>(&text).ok()
            });

        Ok(Self::new(table.models, calibration, rollup, grants_dir))
    }

    /// Resolve `output_mtok` for `model`, mirroring the Python gate: exact
    /// (case-insensitive) `model_id` match wins; otherwise a case-insensitive
    /// substring match on `provider_id`/`model_id`, taking the **maximum**
    /// price among matches so an ambiguous id is never cheaper than its most
    /// expensive variant. `None` when nothing matches.
    pub fn resolve_output_mtok(
        &self,
        model: &str,
    ) -> Option<f64> {
        let low = model.to_lowercase();

        let exact_max = self
            .models
            .iter()
            .filter(|r| r.model_id.to_lowercase() == low)
            .filter_map(|r| r.output_mtok)
            .fold(None, fold_max);
        if exact_max.is_some() {
            return exact_max;
        }

        self.models
            .iter()
            .filter(|r| {
                r.provider_id.to_lowercase().contains(&low)
                    || r.model_id.to_lowercase().contains(&low)
            })
            .filter_map(|r| r.output_mtok)
            .fold(None, fold_max)
    }

    /// True when `model` resolves to a price-table entry via [`resolve_output_mtok`].
    pub fn resolves(
        &self,
        model: &str,
    ) -> bool {
        self.resolve_output_mtok(model).is_some()
    }

    /// Compact, sorted, deduplicated list of known `model_id`s, for rejection guidance.
    pub fn available_model_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> =
            self.models.iter().map(|r| r.model_id.clone()).collect();
        ids.sort();
        ids.dedup();
        ids
    }

    /// Compute base_budget from model's output_mtok using linear inverse mapping.
    /// Returns a value in [0, scale_max]. Unknown model → 0 (conservative).
    pub fn base_budget(
        &self,
        model: &str,
    ) -> u32 {
        let Some(out) = self.resolve_output_mtok(model) else {
            return 0;
        };
        let ratio = 1.0 - (out / self.calibration.budget_zero_price);
        let scaled = (ratio * self.calibration.scale_max as f64).round();
        scaled.clamp(0.0, self.calibration.scale_max as f64) as u32
    }

    /// Resolve tool cost from empirical rollup. Fail open (return 0) for unmeasured tools.
    fn tool_cost(
        &self,
        tool: &str,
    ) -> u32 {
        let Some(rollup) = &self.rollup else {
            return 0;
        };
        let tool_low = tool.to_lowercase();

        // Try exact match first (case-insensitive)
        let exact_match = rollup.report.tools.iter().find(|t| {
            t.tool_name.to_lowercase() == tool_low
                && t.call_count >= MIN_CALLS
                && t.cost.is_some()
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
    fn load_grant_offset(
        &self,
        grant_id: &str,
        model: &str,
    ) -> u32 {
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
            if let Ok(exp_time) = chrono::DateTime::parse_from_rfc3339(expires)
            {
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
    pub fn is_orchestrator(
        &self,
        output_mtok: f64,
    ) -> bool {
        output_mtok > DEFAULT_THRESHOLD_X
    }

    /// Decide whether `model` may call `tool` directly, with optional grant_id.
    ///
    /// * Compute: base_budget, tool_cost, offset.
    /// * effective = base_budget + offset (capped at 2*scale_max).
    /// * Allow if cost <= effective; else Delegate with guidance.
    pub fn evaluate(
        &self,
        model: &str,
        tool: &str,
        grant_id: Option<&str>,
    ) -> Decision {
        // Reject an unresolvable caller_model before any budget math. An unknown
        // id must never fall through to a zero budget: that silently disables
        // price-awareness enforcement instead of surfacing the mistake.
        if self.resolve_output_mtok(model).is_none() {
            return Decision::Reject {
                guidance: unknown_model_guidance(
                    model,
                    &self.available_model_ids(),
                ),
            };
        }
        let tool_cost = self.tool_cost(tool);
        if tool_cost == 0 {
            return Decision::Allow;
        }
        let base = self.base_budget(model);
        let offset =
            grant_id.map_or(0, |gid| self.load_grant_offset(gid, model));
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
    pub fn evaluate_legacy(
        &self,
        model: &str,
        tool: &str,
    ) -> Decision {
        self.evaluate(model, tool, None)
    }
}

fn fold_max(
    acc: Option<f64>,
    v: f64,
) -> Option<f64> {
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
pub fn unknown_model_guidance(
    model: &str,
    available: &[String],
) -> String {
    let sample = if available.is_empty() {
        "(no models loaded)".to_string()
    } else {
        available
            .iter()
            .take(12)
            .cloned()
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
pub fn delegation_guidance(
    model: &str,
    tool: &str,
    x: f64,
) -> String {
    format!(
        "Model '{model}' exceeds the orchestrator threshold (output_mtok > {x} \
         USD/1M). Do not call the token-heavy tool '{tool}' directly. Delegate \
         it to a cheaper sub-agent via runSubagent(model=<cheaper>, ...) and \
         aggregate the result. Reserve this model for strategic decisions, \
         code/change planning, and tool-call planning."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_gate() -> Gate {
        let rows = vec![
            PriceRow {
                provider_id: "anthropic".into(),
                model_id: "claude-opus-4-1".into(),
                output_mtok: Some(75.0),
            },
            PriceRow {
                provider_id: "anthropic".into(),
                model_id: "claude-opus-4-5".into(),
                output_mtok: Some(25.0),
            },
            PriceRow {
                provider_id: "openai".into(),
                model_id: "o3".into(),
                output_mtok: Some(40.0),
            },
            PriceRow {
                provider_id: "anthropic".into(),
                model_id: "claude-sonnet-4-5".into(),
                output_mtok: Some(15.0),
            },
            PriceRow {
                provider_id: "openai".into(),
                model_id: "gpt-5".into(),
                output_mtok: Some(10.0),
            },
            PriceRow {
                provider_id: "openai".into(),
                model_id: "gpt-5-mini".into(),
                output_mtok: Some(2.0),
            },
            PriceRow {
                provider_id: "anthropic".into(),
                model_id: "claude-haiku".into(),
                output_mtok: Some(1.0),
            },
        ];
        Gate::new(rows, ModelBudgetCalibration::default(), None, None)
    }

    #[test]
    fn exact_match_and_case_insensitive() {
        let g = test_gate();
        assert_eq!(g.resolve_output_mtok("claude-opus-4-1"), Some(75.0));
        assert_eq!(g.resolve_output_mtok("GPT-5-MINI"), Some(2.0));
    }

    #[test]
    fn ambiguous_substring_takes_max() {
        let g = test_gate();
        assert_eq!(g.resolve_output_mtok("claude-opus"), Some(75.0));
    }

    #[test]
    fn exact_wins_over_substring() {
        let g = test_gate();
        assert_eq!(g.resolve_output_mtok("gpt-5"), Some(10.0));
    }

    #[test]
    fn unknown_model_none() {
        let g = test_gate();
        assert_eq!(g.resolve_output_mtok("no-such"), None);
    }

    #[test]
    fn strict_threshold_boundary() {
        let g = test_gate();
        assert!(g.is_orchestrator(15.0001));
        assert!(!g.is_orchestrator(15.0));
    }

    #[test]
    fn base_budget_linear_inverse() {
        let g = test_gate();
        // Haiku (1): high budget
        let haiku_budget = g.base_budget("claude-haiku");
        assert!(haiku_budget >= 90 && haiku_budget <= 100);
        // Sonnet (15): mid budget
        let sonnet_budget = g.base_budget("claude-sonnet-4-5");
        assert!(sonnet_budget >= 70 && sonnet_budget <= 80);
        // Opus-4-5 (25): lower
        let opus_budget = g.base_budget("claude-opus-4-5");
        assert!(opus_budget >= 50 && opus_budget <= 65);
        // Opus-4-1 (75): near zero (above budget_zero_price=60)
        let opus_old_budget = g.base_budget("claude-opus-4-1");
        assert_eq!(opus_old_budget, 0);
        // Unknown: conservative 0
        assert_eq!(g.base_budget("mystery"), 0);
    }

    #[test]
    fn tool_cost_fail_open_no_rollup() {
        let g = test_gate();
        // Without rollup, all tools return cost 0 (fail open)
        assert_eq!(g.tool_cost("runSubagent"), 0);
        assert_eq!(g.tool_cost("read_file"), 0);
        assert_eq!(g.tool_cost("some_unknown_tool"), 0);
    }

    #[test]
    fn tool_cost_from_rollup() {
        let rollup = ToolMetricsRollup {
            report: ToolMetricsReport {
                tools: vec![
                    ToolTokenStats {
                        tool_name: "read_file".into(),
                        call_count: 10,
                        cost: Some(80),
                    },
                    ToolTokenStats {
                        tool_name: "grep_search".into(),
                        call_count: 1, // at MIN_CALLS threshold
                        cost: Some(50),
                    },
                    ToolTokenStats {
                        tool_name: "unmeasured_tool".into(),
                        call_count: 0, // below threshold
                        cost: Some(30),
                    },
                ],
            },
        };
        let g = Gate::new(
            test_gate().models,
            ModelBudgetCalibration::default(),
            Some(rollup),
            None,
        );
        assert_eq!(g.tool_cost("read_file"), 80); // from rollup
        assert_eq!(g.tool_cost("grep_search"), 50); // at MIN_CALLS=1, use rollup
        assert_eq!(g.tool_cost("unmeasured_tool"), 0); // below MIN_CALLS -> fail open
        assert_eq!(g.tool_cost("runSubagent"), 0); // not in rollup -> fail open
        assert_eq!(g.tool_cost("completely_unknown"), 0); // not in rollup -> fail open
    }

    #[test]
    fn grant_offset_valid() {
        let tmp = tempfile::tempdir().unwrap();
        let grant_path = tmp.path().join("sess1.json");
        std::fs::write(
            &grant_path,
            r#"{"grant_id":"sess1","offset":30,"model":"claude-sonnet-4-5"}"#,
        )
        .unwrap();

        let g = Gate::new(
            test_gate().models,
            ModelBudgetCalibration::default(),
            None,
            Some(tmp.path().to_path_buf()),
        );
        assert_eq!(g.load_grant_offset("sess1", "claude-sonnet-4-5"), 30);
        assert_eq!(g.load_grant_offset("sess1", "other-model"), 0); // model mismatch
        assert_eq!(g.load_grant_offset("missing", "claude-sonnet-4-5"), 0); // missing file
    }

    #[test]
    fn grant_offset_expired() {
        let tmp = tempfile::tempdir().unwrap();
        let grant_path = tmp.path().join("expired.json");
        std::fs::write(
            &grant_path,
            r#"{"grant_id":"expired","offset":50,"expires_at":"2020-01-01T00:00:00Z"}"#,
        )
        .unwrap();

        let g = Gate::new(
            test_gate().models,
            ModelBudgetCalibration::default(),
            None,
            Some(tmp.path().to_path_buf()),
        );
        assert_eq!(g.load_grant_offset("expired", "any-model"), 0);
    }

    #[test]
    fn grant_offset_malformed() {
        let tmp = tempfile::tempdir().unwrap();
        let grant_path = tmp.path().join("bad.json");
        std::fs::write(&grant_path, r#"{"not valid json"#).unwrap();

        let g = Gate::new(
            test_gate().models,
            ModelBudgetCalibration::default(),
            None,
            Some(tmp.path().to_path_buf()),
        );
        assert_eq!(g.load_grant_offset("bad", "any-model"), 0);
    }

    #[test]
    fn evaluate_with_rollup() {
        let rollup = ToolMetricsRollup {
            report: ToolMetricsReport {
                tools: vec![ToolTokenStats {
                    tool_name: "read_file".into(),
                    call_count: 10,
                    cost: Some(80),
                }],
            },
        };
        let g = Gate::new(
            test_gate().models,
            ModelBudgetCalibration::default(),
            Some(rollup),
            None,
        );
        // Haiku (high budget ~98) vs measured tool (cost 80) -> allow
        assert_eq!(
            g.evaluate("claude-haiku", "read_file", None),
            Decision::Allow
        );
        // Opus-4-5 (budget ~58) vs measured tool (cost 80) -> delegate
        assert!(matches!(
            g.evaluate("claude-opus-4-5", "read_file", None),
            Decision::Delegate { .. }
        ));
        // Any model vs unmeasured tool -> allow (fail open)
        assert_eq!(
            g.evaluate("claude-opus-4-1", "unknown_tool", None),
            Decision::Allow
        );
    }

    #[test]
    fn evaluate_graded_with_offset() {
        let tmp = tempfile::tempdir().unwrap();
        let grant_path = tmp.path().join("boost.json");
        std::fs::write(&grant_path, r#"{"grant_id":"boost","offset":30}"#)
            .unwrap();

        let rollup = ToolMetricsRollup {
            report: ToolMetricsReport {
                tools: vec![ToolTokenStats {
                    tool_name: "read_file".into(),
                    call_count: 10,
                    cost: Some(75),
                }],
            },
        };
        let g = Gate::new(
            test_gate().models,
            ModelBudgetCalibration::default(),
            Some(rollup),
            Some(tmp.path().to_path_buf()),
        );
        // Sonnet base ~75 + offset 30 = 105 > 75 tool cost -> allow
        assert_eq!(
            g.evaluate("claude-sonnet-4-5", "read_file", Some("boost")),
            Decision::Allow
        );
        // Without grant: Sonnet (75) vs tool (75) -> allow at boundary
        assert_eq!(
            g.evaluate("claude-sonnet-4-5", "read_file", None),
            Decision::Allow
        );
    }

    #[test]
    fn fail_open_unmeasured() {
        let g = test_gate();
        // Without rollup, all tools fail open (cost 0) -> allow
        assert_eq!(
            g.evaluate("claude-opus-4-1", "runSubagent", None),
            Decision::Allow
        );
        assert_eq!(
            g.evaluate("claude-opus-4-1", "read_file", None),
            Decision::Allow
        );
    }

    #[test]
    fn evaluate_legacy_delegates_to_evaluate() {
        let rollup = ToolMetricsRollup {
            report: ToolMetricsReport {
                tools: vec![ToolTokenStats {
                    tool_name: "read_file".into(),
                    call_count: 10,
                    cost: Some(80),
                }],
            },
        };
        let g = Gate::new(
            test_gate().models,
            ModelBudgetCalibration::default(),
            Some(rollup),
            None,
        );
        // Legacy now delegates to evaluate(), so behavior is consistent
        assert!(matches!(
            g.evaluate_legacy("claude-opus-4-5", "read_file"),
            Decision::Delegate { .. }
        ));
        // Unmeasured tool -> allow
        assert_eq!(
            g.evaluate_legacy("claude-opus-4-1", "unknown_tool"),
            Decision::Allow
        );
    }

    #[test]
    fn graded_budget_boundary_with_rollup() {
        let rollup = ToolMetricsRollup {
            report: ToolMetricsReport {
                tools: vec![
                    ToolTokenStats {
                        tool_name: "read_file".into(),
                        call_count: 10,
                        cost: Some(75),
                    },
                    ToolTokenStats {
                        tool_name: "light_tool".into(),
                        call_count: 5,
                        cost: Some(1),
                    },
                ],
            },
        };
        let g = Gate::new(
            test_gate().models,
            ModelBudgetCalibration::default(),
            Some(rollup),
            None,
        );

        // Models with output_mtok 1, 10, 15 → Allow (budget >= 75)
        assert_eq!(
            g.evaluate("claude-haiku", "read_file", None),
            Decision::Allow
        ); // 1 → budget ~98
        assert_eq!(g.evaluate("gpt-5", "read_file", None), Decision::Allow); // 10 → budget ~83
        assert_eq!(
            g.evaluate("claude-sonnet-4-5", "read_file", None),
            Decision::Allow
        ); // 15 → budget 75 (at boundary)

        // Models with output_mtok 25, 40, 75 → Delegate (budget < 75)
        assert!(matches!(
            g.evaluate("claude-opus-4-5", "read_file", None),
            Decision::Delegate { .. }
        )); // 25 → budget ~58
        assert!(matches!(
            g.evaluate("o3", "read_file", None),
            Decision::Delegate { .. }
        )); // 40 → budget ~33
        assert!(matches!(
            g.evaluate("claude-opus-4-1", "read_file", None),
            Decision::Delegate { .. }
        )); // 75 → budget 0

        // Unknown model → Reject (caller_model unresolvable)
        assert!(matches!(
            g.evaluate("unknown-model", "read_file", None),
            Decision::Reject { .. }
        ));

        // Light tool → Allow for all models with budget >= 1
        assert_eq!(
            g.evaluate("claude-haiku", "light_tool", None),
            Decision::Allow
        );
        assert_eq!(g.evaluate("gpt-5", "light_tool", None), Decision::Allow);
        assert_eq!(
            g.evaluate("claude-sonnet-4-5", "light_tool", None),
            Decision::Allow
        );
        assert_eq!(
            g.evaluate("claude-opus-4-5", "light_tool", None),
            Decision::Allow
        );
        assert_eq!(g.evaluate("o3", "light_tool", None), Decision::Allow);
        assert!(matches!(
            g.evaluate("claude-opus-4-1", "light_tool", None),
            Decision::Delegate { .. }
        )); // 75 > 60 → budget 0, cost 1 → delegate

        // Unmeasured tools fail open → Always allow (for resolvable models)
        assert_eq!(
            g.evaluate("claude-haiku", "runSubagent", None),
            Decision::Allow
        );
        assert_eq!(
            g.evaluate("claude-opus-4-1", "runSubagent", None),
            Decision::Allow
        );
        // Unknown model is rejected even for unmeasured tools
        assert!(matches!(
            g.evaluate("unknown-model", "runSubagent", None),
            Decision::Reject { .. }
        ));
    }

    #[test]
    fn evaluate_rejects_unknown_caller_model() {
        let g = test_gate();
        // Unknown caller_model is refused regardless of tool, so a
        // zero-cost budget can never silently bypass enforcement.
        assert!(matches!(
            g.evaluate("github-copilot", "read_file", None),
            Decision::Reject { .. }
        ));
        assert!(matches!(
            g.evaluate("github-copilot", "update_ticket", None),
            Decision::Reject { .. }
        ));
        assert!(matches!(
            g.evaluate("github-copilot", "runSubagent", None),
            Decision::Reject { .. }
        ));
        // A resolvable model is not rejected, and unmeasured tools fail open.
        assert_eq!(
            g.evaluate("claude-haiku", "update_ticket", None),
            Decision::Allow
        );
    }

    #[test]
    fn tool_cost_exact_match_precedence() {
        // Rollup with both "read" and "read_file" entries
        let rollup = ToolMetricsRollup {
            report: ToolMetricsReport {
                tools: vec![
                    ToolTokenStats {
                        tool_name: "read".into(),
                        call_count: 5,
                        cost: Some(20),
                    },
                    ToolTokenStats {
                        tool_name: "read_file".into(),
                        call_count: 10,
                        cost: Some(80),
                    },
                ],
            },
        };
        let g = Gate::new(
            test_gate().models,
            ModelBudgetCalibration::default(),
            Some(rollup),
            None,
        );

        // Exact match "read_file" should return 80, not 20 from substring match "read"
        assert_eq!(g.tool_cost("read_file"), 80);

        // Exact match "read" should return 20
        assert_eq!(g.tool_cost("read"), 20);
    }

    #[test]
    fn tool_cost_substring_fallback_when_no_exact() {
        // Rollup with only partial names
        let rollup = ToolMetricsRollup {
            report: ToolMetricsReport {
                tools: vec![ToolTokenStats {
                    tool_name: "file".into(),
                    call_count: 5,
                    cost: Some(50),
                }],
            },
        };
        let g = Gate::new(
            test_gate().models,
            ModelBudgetCalibration::default(),
            Some(rollup),
            None,
        );

        // No exact match, but substring "file" matches "read_file"
        assert_eq!(g.tool_cost("read_file"), 50);

        // Bidirectional: "read_file" contains "file"
        assert_eq!(g.tool_cost("file"), 50);
    }

    #[test]
    fn tool_cost_exact_match_case_insensitive() {
        let rollup = ToolMetricsRollup {
            report: ToolMetricsReport {
                tools: vec![ToolTokenStats {
                    tool_name: "Read_File".into(),
                    call_count: 10,
                    cost: Some(75),
                }],
            },
        };
        let g = Gate::new(
            test_gate().models,
            ModelBudgetCalibration::default(),
            Some(rollup),
            None,
        );

        // Case-insensitive exact match
        assert_eq!(g.tool_cost("read_file"), 75);
        assert_eq!(g.tool_cost("Read_File"), 75);
        assert_eq!(g.tool_cost("READ_FILE"), 75);
    }

    #[test]
    fn tool_cost_exact_match_ineligible_falls_through_to_substring() {
        // Rollup with an exact-match entry that is ineligible (call_count < MIN_CALLS)
        // and an eligible substring-match entry
        let rollup = ToolMetricsRollup {
            report: ToolMetricsReport {
                tools: vec![
                    ToolTokenStats {
                        tool_name: "read_file".into(),
                        call_count: 0, // below MIN_CALLS
                        cost: Some(999),
                    },
                    ToolTokenStats {
                        tool_name: "file".into(),
                        call_count: 5, // eligible
                        cost: Some(40),
                    },
                ],
            },
        };
        let g = Gate::new(
            test_gate().models,
            ModelBudgetCalibration::default(),
            Some(rollup),
            None,
        );

        // Query "read_file": exact match exists but is ineligible -> must fall through
        // to substring match "file" and return 40, not 0
        assert_eq!(g.tool_cost("read_file"), 40);
    }
}
