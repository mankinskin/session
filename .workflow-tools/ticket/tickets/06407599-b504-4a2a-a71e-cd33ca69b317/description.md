## Problem

`session-cli handoff` hardcodes the handoff `package` argument to `None`, so handoff packages cannot be supplied over the CLI transport even though the underlying `session-api` (`SessionRuntime::create_handoff_result` / equivalent) supports them. Carried over from session 5b961940 and still unticketed as of 2026-07-30.

## Location

memory-api/tools/cli/session-cli/src/lib.rs, `SessionCommand::Handoff(args)` arm (around line 694-700):

```rust
SessionCommand::Handoff(args) => {
    let result = config.create_handoff_result(
        &args.workspace_session_id,
        None,
        parse_validation_gates(args.validation_json.as_deref())?,
        None,
    )?;
    to_value(&result)
},
```

The second `None` argument occupies the `package` parameter position of `create_handoff_result`. `HandoffArgs` (memory-api/tools/cli/session-cli/src/lib.rs, around line 549-555) has no field for supplying a package payload at all.

## Acceptance Criteria

1. `HandoffArgs` gains an optional argument (e.g. `--package-json <json>`) that accepts a serialized handoff package payload, following the same JSON-arg pattern used by `--validation-json` in the same struct.
2. The `SessionCommand::Handoff` arm parses this argument and passes the resulting package (or `None` if omitted) to `create_handoff_result` in place of the hardcoded `None`.
3. A CLI test in memory-api/tools/cli/session-cli/tests/cli.rs exercises `session handoff` with a package payload supplied and asserts the resulting handoff record/output reflects that package (not empty/absent).
4. Omitting the new argument preserves existing behavior (package is `None`) — no regression to current callers.