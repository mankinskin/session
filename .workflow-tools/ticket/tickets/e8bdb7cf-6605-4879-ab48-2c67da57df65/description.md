# Goal

Populate the resolved validation `outcome` and `new_entities` when a handoff record is created (subticket 2 of `6431985e`). Depends on the model fields from subticket 1.

# Problem (root cause, verified)

`create_handoff_record` (`memory-api/crates/session-api/src/store.rs:1343`) calls `resolve_validation_gates(&context, validation, false)`. Inside `resolve_validation_gates` (`store.rs:1614`), if `required_specs` (workflow validation nodes marked `Required`) is empty, it returns the caller gates **verbatim with `outcome=None`**. Caller-supplied gates that are not backed by a required workflow node therefore never get an authoritative outcome — exactly why handoff `816f0807` persisted `outcome=None`.

# Solution Design

1. In `resolve_validation_gates`, after handling required workflow specs, also resolve an authoritative `outcome` for every caller-supplied gate by querying test-api (`test_store.list_executions` with `validation_spec_id`, newest first) and setting `outcome = validation_outcome_label(latest)`. Keep the existing `strict_required` fail-closed behavior for required specs unchanged; this only *fills in* display outcomes for non-required gates (never promotes them to completion authority).
2. In `create_handoff_record`, compute `new_entities` for this run: entities created/owned by the session's agent id (board `owned_files` → ticket ids; plus any explicitly supplied created URNs) and set the field added in subticket 1.
3. Populate `summary`/`remaining`/`decisions`/`blockers` from the request payload threaded in subticket 3 (leave empty here if not supplied).

# Acceptance Criteria

1. A handoff created for a run that recorded a validation execution persists a non-`None` resolved `outcome` for that gate, even when it is not a required workflow node.
2. `new_entities` reflects tickets created/owned during the run.
3. `strict_required` semantics for required validation nodes are unchanged (regression test).
4. Focused test asserts outcome + new_entities population at `create_handoff_record`.

# Traceability

- Parent: `6431985e-e729-426b-9f91-66ad4b1c6fe6`.
- Depends on model subticket (fields).
- Spec: `8c880efc-7083-4e1d-bf06-96b8254be913` (AC5).