//! The cost-gate engine and env/CLI wiring, split out of `mcp-toolmon` so
//! transport (`proxy`/`supervisor`/`watcher`/`shadow`) never needs to know
//! how a verdict is computed. Depends on `toolmon-policy-api` for the
//! `Policy` trait and `Decision` type; has no dependency back on
//! `mcp-toolmon`, which is what breaks the pre-split dependency cycle.

pub mod config;
pub mod gate;
pub mod policy_impl;
pub mod verdict;

pub use gate::{
    Gate,
    ModelBudgetCalibration,
};
pub use policy_impl::CostGatePolicy;
