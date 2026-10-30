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

fn copilot_gate() -> Gate {
    let row = |provider: &str, id: &str, out: f64| PriceRow {
        provider_id: provider.into(),
        model_id: id.into(),
        output_mtok: Some(out),
    };
    let rows = vec![
        row("01-ai", "yi-large", 3.0),
        row("github-copilot", "GPT-6.1 Sol", 10.0),
        row("github-copilot", "GPT-6 Luna", 4.0),
        row("github-copilot", "Claude Opus 5", 25.0),
        row("github-copilot", "Claude Sonnet 5", 10.0),
    ];
    Gate::new(rows, ModelBudgetCalibration::default(), None, None)
}

#[test]
fn hyphenated_id_matches_display_name_row() {
    let g = copilot_gate();
    assert_eq!(g.resolve_output_mtok("gpt-6.1-sol"), Some(10.0));
    assert_eq!(g.resolve_output_mtok("claude-opus-5"), Some(25.0));
    assert_eq!(g.resolve_output_mtok("Claude Sonnet 5"), Some(10.0));
    assert_eq!(g.resolve_output_mtok("claude_sonnet_5"), Some(10.0));
}

#[test]
fn hyphenated_id_is_allowed_by_evaluate() {
    let g = copilot_gate();
    assert!(!matches!(
        g.evaluate("gpt-6.1-sol", "any_tool", None),
        Decision::Reject { .. }
    ));
}

#[test]
fn available_ids_prefer_roster_in_key_form() {
    let g = copilot_gate();
    let ids = g.available_model_ids();
    assert!(ids.contains(&"gpt-6.1-sol".to_string()));
    assert!(!ids.iter().any(|id| id == "yi-large"));
}

#[test]
fn guidance_lists_same_family_first() {
    let g = copilot_gate();
    let msg = unknown_model_guidance("gpt-9-nova", &g.available_model_ids());
    assert!(msg.contains("gpt-6-luna, gpt-6.1-sol"));
    assert!(!msg.contains("claude-opus-5"));
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

#[path = "tests/graded_cost.rs"]
mod graded_cost;
#[allow(unused_imports)]
pub use graded_cost::*;
