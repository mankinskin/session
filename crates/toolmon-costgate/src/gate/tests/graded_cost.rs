#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn tool_cost_fail_open_no_rollup() {
    let g = test_gate();
    // Without rollup, all tools return cost 0 (fail open)
    assert_eq!(g.tool_cost("runSubagent"), 0);
    assert_eq!(g.tool_cost("read_file"), 0);
    assert_eq!(g.tool_cost("some_unknown_tool"), 0);
}

#[test]
pub(super) fn tool_cost_from_rollup() {
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
pub(super) fn grant_offset_valid() {
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
pub(super) fn grant_offset_expired() {
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
pub(super) fn grant_offset_malformed() {
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
pub(super) fn evaluate_with_rollup() {
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
pub(super) fn evaluate_graded_with_offset() {
    let tmp = tempfile::tempdir().unwrap();
    let grant_path = tmp.path().join("boost.json");
    std::fs::write(&grant_path, r#"{"grant_id":"boost","offset":30}"#).unwrap();

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
pub(super) fn fail_open_unmeasured() {
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
pub(super) fn graded_budget_boundary_with_rollup() {
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
pub(super) fn tool_cost_exact_match_precedence() {
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
pub(super) fn tool_cost_substring_fallback_when_no_exact() {
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
pub(super) fn tool_cost_exact_match_case_insensitive() {
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
pub(super) fn tool_cost_exact_match_ineligible_falls_through_to_substring() {
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
