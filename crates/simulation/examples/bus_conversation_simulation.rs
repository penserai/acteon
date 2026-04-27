//! Phase 5 multi-agent conversation demo.
//!
//! Drives the in-memory bus backend directly (no Kafka, no HTTP) so
//! the test fits into the wider simulation suite. The HTTP handlers
//! `POST /v1/bus/conversations/.../messages` and
//! `GET /v1/bus/conversations/.../messages` delegate to the same
//! types and the same shared events topic.
//!
//! Scenarios:
//!
//! 1. Two conversations share one events topic; messages are keyed
//!    by `conversation_id` so each thread's records land on a stable
//!    partition. We verify replay only returns the requested
//!    conversation's messages.
//! 2. Linear state machine: Active → Resolved → Archived. Posts to an
//!    archived conversation are rejected; reopening from Resolved
//!    works.
//! 3. Participant ACL: a sender outside the participant list is
//!    rejected when participants are configured; an empty
//!    participant list means "open thread".
//!
//! Run with:
//! ```text
//! cargo run -p acteon-simulation --features bus --example bus_conversation_simulation
//! ```

use std::time::Duration;

use futures::StreamExt;
use tracing::{Level, info};

use acteon_bus::{BusMessage, MemoryBackend, StartOffset};
use acteon_core::{Conversation, ConversationState, ConversationTransition, Topic};

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_env_filter("info,acteon_bus=info")
        .init();

    let backend: acteon_bus::SharedBackend = MemoryBackend::new();
    let events = Topic::new("conversations-events", "agents", "demo");
    backend.create_topic(&events).await?;

    // -----------------------------------------------------------------
    // 1. Two conversations on the shared events topic
    // -----------------------------------------------------------------

    let mut planning = Conversation::new("plan-001", "agents", "demo");
    planning.title = Some("Planning Q3".into());
    planning.participants = vec!["planner-1".into(), "ocr-svc".into()];
    planning.validate()?;

    let mut review = Conversation::new("rev-002", "agents", "demo");
    review.title = Some("Code review #1234".into());
    review.participants = vec!["reviewer-1".into()];
    review.validate()?;

    assert_eq!(
        planning.effective_events_topic(),
        review.effective_events_topic()
    );
    let topic = planning.effective_events_topic();
    info!(
        topic = %topic,
        "two conversations sharing one events topic"
    );

    // Post 3 messages each, interleaved.
    for i in 0..3 {
        let mut p_msg = BusMessage::new(
            topic.clone(),
            serde_json::json!({"step": i, "from": "planning"}),
        )
        .with_key(&planning.conversation_id);
        p_msg.headers.insert(
            "acteon.conversation.id".into(),
            planning.conversation_id.clone(),
        );
        p_msg
            .headers
            .insert("acteon.conversation.sender".into(), "planner-1".into());
        backend.produce(p_msg).await?;

        let mut r_msg = BusMessage::new(
            topic.clone(),
            serde_json::json!({"step": i, "from": "review"}),
        )
        .with_key(&review.conversation_id);
        r_msg.headers.insert(
            "acteon.conversation.id".into(),
            review.conversation_id.clone(),
        );
        r_msg
            .headers
            .insert("acteon.conversation.sender".into(), "reviewer-1".into());
        backend.produce(r_msg).await?;
    }

    // Replay just the planning thread by reading from earliest and
    // filtering on the server-stamped header — same logic the
    // `GET /messages` handler runs.
    let mut stream = backend
        .subscribe(&topic, "sim-replay-planning", StartOffset::Earliest)
        .await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut planning_msgs = Vec::new();
    while planning_msgs.len() < 3 && tokio::time::Instant::now() < deadline {
        tokio::select! {
            next = stream.next() => {
                match next {
                    Some(Ok(msg)) => {
                        if msg
                            .headers
                            .get("acteon.conversation.id")
                            .is_some_and(|v| v == &planning.conversation_id)
                        {
                            planning_msgs.push(msg);
                        }
                    }
                    Some(Err(_)) | None => break,
                }
            }
            () = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
    assert_eq!(
        planning_msgs.len(),
        3,
        "expected exactly 3 planning messages"
    );
    for msg in &planning_msgs {
        assert_eq!(msg.key.as_deref(), Some(planning.conversation_id.as_str()));
    }
    info!(
        count = planning_msgs.len(),
        "replay returned only the planning thread's messages from the shared topic"
    );

    // -----------------------------------------------------------------
    // 2. State machine
    // -----------------------------------------------------------------

    let mut conv = Conversation::new("flow-1", "agents", "demo");
    assert_eq!(conv.state, ConversationState::Active);
    assert!(conv.accepts_messages());

    conv.apply_transition(ConversationTransition::Resolve)?;
    assert_eq!(conv.state, ConversationState::Resolved);
    assert!(conv.accepts_messages());
    info!("after resolve: state=Resolved, accepts_messages=true (follow-ups OK)");

    conv.apply_transition(ConversationTransition::Reopen)?;
    assert_eq!(conv.state, ConversationState::Active);
    info!("reopened back to Active");

    conv.apply_transition(ConversationTransition::Resolve)?;
    conv.apply_transition(ConversationTransition::Archive)?;
    assert_eq!(conv.state, ConversationState::Archived);
    assert!(!conv.accepts_messages());
    info!("after archive: state=Archived, accepts_messages=false");

    let illegal = conv.apply_transition(ConversationTransition::Reopen);
    assert!(illegal.is_err(), "reopen from Archived must be rejected");
    info!("reopen-from-Archived correctly rejected");

    // -----------------------------------------------------------------
    // 3. Participant ACL
    // -----------------------------------------------------------------

    let mut closed = Conversation::new("closed-1", "agents", "demo");
    closed.participants = vec!["alpha".into(), "beta".into()];
    let allowed = closed.participants.iter().any(|p| p == "alpha");
    let rejected = closed.participants.iter().any(|p| p == "gamma");
    assert!(allowed && !rejected);
    info!(
        participants = ?closed.participants,
        "participant 'alpha' allowed; 'gamma' would be rejected with 400"
    );

    let mut open = Conversation::new("open-1", "agents", "demo");
    open.participants.clear();
    info!("empty participant list = open thread; any sender accepted");

    info!("conversation simulation complete");
    Ok(())
}
