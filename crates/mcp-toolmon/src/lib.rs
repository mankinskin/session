//! Model-aware MCP middleware library (transport only).
//!
//! The cost decision core and the pluggable `Policy` trait it implements now
//! live in the `toolmon-costgate` and `toolmon-policy-api` crates. This crate
//! is transport: [`proxy`] for the pure JSON-RPC interception logic (against
//! `toolmon_policy_api::Policy` only, never the gate), and [`supervisor`] for
//! the async child-process transport. The binary in `main.rs` wires
//! `toolmon-costgate`'s default policy to stdio.

pub mod proxy;
pub mod shadow;
pub mod supervisor;
pub mod watcher;
