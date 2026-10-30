#[allow(unused_imports)]
use super::*;

//
// verdict subcommand tests (BLOCKER 2)
//

#[test]
pub(super) fn test_verdict_allow() {
    let tmp = TempDir::new().unwrap();

    let price_table = json!({
        "models": [
            {
                "provider_id": "anthropic",
                "model_id": "claude-haiku-3-7",
                "output_mtok": 2.0
            }
        ]
    });
    let table_path = write_json(&tmp, "prices.json", &price_table);

    let rollup = json!({
        "report": {
            "tools": [
                {
                    "tool_name": "cheap_tool",
                    "call_count": 10,
                    "cost": 10
                }
            ]
        }
    });
    let rollup_path = write_json(&tmp, "rollup.json", &rollup);

    let output = Command::new(get_binary_path())
        .arg("verdict")
        .arg("--model")
        .arg("claude-haiku-3-7")
        .arg("--tool")
        .arg("cheap_tool")
        .arg("--table")
        .arg(table_path.display().to_string())
        .arg("--rollup")
        .arg(rollup_path.display().to_string())
        .output()
        .expect("Failed to execute verdict subcommand");

    assert!(output.status.success(), "verdict should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "Allow", "Expected 'Allow' verdict");
}

#[test]
pub(super) fn test_verdict_delegate() {
    let tmp = TempDir::new().unwrap();

    let price_table = json!({
        "models": [
            {
                "provider_id": "anthropic",
                "model_id": "claude-opus-4-8",
                "output_mtok": 60.0
            }
        ]
    });
    let table_path = write_json(&tmp, "prices.json", &price_table);

    let rollup = json!({
        "report": {
            "tools": [
                {
                    "tool_name": "expensive_tool",
                    "call_count": 10,
                    "cost": 50
                }
            ]
        }
    });
    let rollup_path = write_json(&tmp, "rollup.json", &rollup);

    let output = Command::new(get_binary_path())
        .arg("verdict")
        .arg("--model")
        .arg("claude-opus-4-8")
        .arg("--tool")
        .arg("expensive_tool")
        .arg("--table")
        .arg(table_path.display().to_string())
        .arg("--rollup")
        .arg(rollup_path.display().to_string())
        .output()
        .expect("Failed to execute verdict subcommand");

    assert!(output.status.success(), "verdict should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.starts_with("Delegate:"),
        "Expected 'Delegate:' verdict"
    );
    assert!(stdout.contains("cost 50"), "Expected cost in guidance");
}

#[test]
pub(super) fn test_verdict_reject_unknown_model() {
    let tmp = TempDir::new().unwrap();

    let price_table = json!({
        "models": [
            {
                "provider_id": "anthropic",
                "model_id": "claude-opus-4-8",
                "output_mtok": 60.0
            }
        ]
    });
    let table_path = write_json(&tmp, "prices.json", &price_table);

    let rollup = json!({
        "report": {
            "tools": [
                {
                    "tool_name": "some_tool",
                    "call_count": 10,
                    "cost": 10
                }
            ]
        }
    });
    let rollup_path = write_json(&tmp, "rollup.json", &rollup);

    let output = Command::new(get_binary_path())
        .arg("verdict")
        .arg("--model")
        .arg("unknown-model-99")
        .arg("--tool")
        .arg("some_tool")
        .arg("--table")
        .arg(table_path.display().to_string())
        .arg("--rollup")
        .arg(rollup_path.display().to_string())
        .output()
        .expect("Failed to execute verdict subcommand");

    assert!(output.status.success(), "verdict should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("Reject:"), "Expected 'Reject:' verdict");
    assert!(
        stdout.to_lowercase().contains("unrecognized"),
        "Expected unrecognized model guidance"
    );
}

#[test]
pub(super) fn test_verdict_missing_args() {
    // Test error case: missing required arguments.
    let output = Command::new(get_binary_path())
        .arg("verdict")
        .arg("--model")
        .arg("some-model")
        // Missing --tool and --table
        .output()
        .expect("Failed to execute verdict subcommand");

    assert!(!output.status.success(), "verdict should exit non-zero");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("usage"), "Expected usage message in stderr");
}
