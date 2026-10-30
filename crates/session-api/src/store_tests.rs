#[path = "store_tests/capture/basics.rs"]
mod basics;
#[allow(unused_imports)]
pub use basics::*;
#[path = "store_tests/capture/missing_transcript.rs"]
mod missing_transcript;
#[allow(unused_imports)]
pub use missing_transcript::*;
#[path = "store_tests/capture/query_and_worktree.rs"]
mod query_and_worktree;
#[allow(unused_imports)]
pub use query_and_worktree::*;
#[path = "store_tests/capture/federated_sessions.rs"]
mod federated_sessions;
#[allow(unused_imports)]
pub use federated_sessions::*;
#[path = "store_tests/capture/ticket_relation.rs"]
mod ticket_relation;
#[allow(unused_imports)]
pub use ticket_relation::*;
#[path = "store_tests/capture/ticket_backfill.rs"]
mod ticket_backfill;
#[allow(unused_imports)]
pub use ticket_backfill::*;
#[path = "store_tests/capture/worktree_capture_inference.rs"]
mod worktree_capture_inference;
#[allow(unused_imports)]
pub use worktree_capture_inference::*;
#[path = "store_tests/worktree/audit_and_runtime.rs"]
mod audit_and_runtime;
#[allow(unused_imports)]
pub use audit_and_runtime::*;
#[path = "store_tests/runtime/pins_and_workflow_mutation.rs"]
mod pins_and_workflow_mutation;
#[allow(unused_imports)]
pub use pins_and_workflow_mutation::*;
#[path = "store_tests/workflow/snapshot_and_handoff.rs"]
mod snapshot_and_handoff;
#[allow(unused_imports)]
pub use snapshot_and_handoff::*;
#[path = "store_tests/finish/handoff_lifecycle.rs"]
mod handoff_lifecycle;
#[allow(unused_imports)]
pub use handoff_lifecycle::*;
#[path = "store_tests/finish/validation_authority.rs"]
mod validation_authority;
#[allow(unused_imports)]
pub use validation_authority::*;
#[path = "store_tests/finish/ticket_enforcement.rs"]
mod ticket_enforcement;
#[allow(unused_imports)]
pub use ticket_enforcement::*;
#[path = "store_tests/finish/spec_enforcement.rs"]
mod spec_enforcement;
#[allow(unused_imports)]
pub use spec_enforcement::*;
#[path = "store_tests/finish/workspace_mutation_and_locking.rs"]
mod workspace_mutation_and_locking;
#[allow(unused_imports)]
pub use workspace_mutation_and_locking::*;
#[path = "store_tests/finish/workspace_lock_ordering.rs"]
mod workspace_lock_ordering;
#[allow(unused_imports)]
pub use workspace_lock_ordering::*;
#[path = "store_tests/terminals.rs"]
mod terminals;
#[allow(unused_imports)]
pub use terminals::*;
#[path = "store_tests/intake.rs"]
mod intake;
#[allow(unused_imports)]
pub use intake::*;
