use std::str::FromStr;

use feedback_api::EntityUrn;

use super::{
    FailedToolCallMapping,
    StructuredFeedbackSignal,
};

/// Deterministic, order-preserving, deduplicated discovery queue of feedback
/// target entities.
///
/// Entities are enqueued in first-discovery order; a given [`EntityUrn`] is
/// enqueued at most once (first discovery wins — later re-references of the
/// same entity are deduped rather than reprocessed). This is the
/// deterministic breadth-first iteration the structured feedback ring
/// requires: process signals in a fixed order, and append newly-discovered
/// entities to the queue as they are found, never revisiting one already
/// seen.
///
/// Today's signal kinds (`ExplicitIngestion`'s `target`, `FailedToolCall`'s
/// resolved [`FailedToolCallMapping::Entity`]) each reference at most one
/// entity directly, with no further related entities to expand into — so
/// this is currently the documented acceptable alternative of "mine only
/// the entities detected at the beginning" rather than a multi-level BFS
/// traversal. The queue abstraction is kept independent of any particular
/// signal kind so a future signal that discovers *related* entities (for
/// example a ticket's `depends_on` links) can enqueue onto it without
/// changing the ordering/dedup contract relied on here.
#[derive(Debug, Default)]
pub struct EntityDiscoveryQueue {
    seen: std::collections::HashSet<EntityUrn>,
    order: Vec<EntityUrn>,
}

impl EntityDiscoveryQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueue an entity if it has not been seen before. Returns `true` if
    /// this was a new entity (now queued), `false` if it was already
    /// discovered and is therefore skipped.
    pub fn enqueue(
        &mut self,
        urn: EntityUrn,
    ) -> bool {
        if self.seen.insert(urn.clone()) {
            self.order.push(urn);
            true
        } else {
            false
        }
    }

    /// Consume the queue, returning discovered entities in first-discovery
    /// order.
    pub fn into_ordered(self) -> Vec<EntityUrn> {
        self.order
    }
}

/// Discover the distinct feedback target entities referenced by a session's
/// structured feedback signals, in deterministic first-discovery order.
///
/// This is a pure function over already-mined signals (see
/// [`crate::mine_failed_tool_call_signals`] and
/// [`crate::mine_explicit_ingestion_signals`]) — it performs no store writes
/// and creates no tickets.
pub fn discover_entities_from_signals(
    signals: &[StructuredFeedbackSignal]
) -> Vec<EntityUrn> {
    let mut queue = EntityDiscoveryQueue::new();
    for signal in signals {
        for urn in entity_refs(signal) {
            queue.enqueue(urn);
        }
    }
    queue.into_ordered()
}

fn entity_refs(signal: &StructuredFeedbackSignal) -> Vec<EntityUrn> {
    let mut refs = Vec::new();
    if let Some(FailedToolCallMapping::Entity { urn }) = &signal.mapping {
        refs.push(urn.clone());
    }
    if let Some(target_urn) = signal
        .ingestion
        .as_ref()
        .and_then(|args| args.target.as_deref())
        .and_then(|raw| EntityUrn::from_str(raw).ok())
    {
        refs.push(target_urn);
    }
    refs
}
