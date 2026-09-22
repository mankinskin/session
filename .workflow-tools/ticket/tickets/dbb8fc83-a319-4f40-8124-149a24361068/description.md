## Problem

The ticket store can create `task` and `epic` records but cannot transition or update them because no workflow schema is registered for either type. Independent review of the session-workflow flexibility epic hit `no schema for type 'task'` when closing approved children and updating the changes-requested child; the epic was already known to fail for type `epic`.

## Requirement

Register workflow schemas for both `task` and `epic` ticket types so normal update and close operations work.

## Acceptance criteria

1. `close_ticket` transitions an in-review task to done.
2. `update_ticket` can return an in-review task to in-implementation with findings.
3. The session-workflow epic can transition after dependencies resolve.
4. Regression tests cover both ticket types.