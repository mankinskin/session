use feedback_api::EntityUrn;

use super::super::super::*;

fn failed_tool_call_signal(urn: EntityUrn) -> StructuredFeedbackSignal {
    StructuredFeedbackSignal {
        kind: FeedbackSignalKind::FailedToolCall,
        sequence: None,
        tool_name: Some("mcp_rmcp6_board_check_out".to_string()),
        tool_call_id: None,
        event_id: None,
        tool_success: Some(false),
        ingestion: None,
        mapping: Some(FailedToolCallMapping::Entity { urn }),
    }
}

fn unmapped_failed_tool_call_signal() -> StructuredFeedbackSignal {
    StructuredFeedbackSignal {
        kind: FeedbackSignalKind::FailedToolCall,
        sequence: None,
        tool_name: Some("read_file".to_string()),
        tool_call_id: None,
        event_id: None,
        tool_success: Some(false),
        ingestion: None,
        mapping: Some(FailedToolCallMapping::Unmapped {
            reason: UnmappedReason::NoSupportedEntityStore,
        }),
    }
}

fn explicit_ingestion_signal(target: &str) -> StructuredFeedbackSignal {
    StructuredFeedbackSignal {
        kind: FeedbackSignalKind::ExplicitIngestion,
        sequence: None,
        tool_name: Some("mcp_rmcp5_feedback_ingest".to_string()),
        tool_call_id: None,
        event_id: None,
        tool_success: Some(true),
        ingestion: Some(ExplicitIngestionArgs {
            target: Some(target.to_string()),
            source: Some("agent".to_string()),
            rating: None,
            note: None,
            note_kind: None,
            session_id: None,
            author: None,
        }),
        mapping: None,
    }
}

#[test]
fn discovers_entities_in_first_seen_order_deduped_across_a_session() {
    let t1 = EntityUrn::ticket("memory-api", "t1").unwrap();
    let t2 = EntityUrn::ticket("memory-api", "t2").unwrap();
    let signals = vec![
        failed_tool_call_signal(t1.clone()),
        explicit_ingestion_signal("ce://memory-api/rule/r1"),
        failed_tool_call_signal(t1.clone()),
        unmapped_failed_tool_call_signal(),
        explicit_ingestion_signal("ce://memory-api/ticket/t2"),
        failed_tool_call_signal(t1.clone()),
    ];

    let discovered = discover_entities_from_signals(&signals);

    assert_eq!(
        discovered,
        vec![t1, EntityUrn::rule("memory-api", "r1").unwrap(), t2]
    );
}

#[test]
fn entity_discovery_queue_dedupes_repeated_enqueues() {
    let mut queue = EntityDiscoveryQueue::new();
    let urn = EntityUrn::ticket("memory-api", "t1").unwrap();

    assert!(queue.enqueue(urn.clone()));
    assert!(!queue.enqueue(urn.clone()));
    assert!(!queue.enqueue(urn.clone()));

    assert_eq!(queue.into_ordered(), vec![urn]);
}
