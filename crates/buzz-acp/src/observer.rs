//! In-process observer bus for ACP session activity.
//!
//! This is intentionally process-local infrastructure: it lets the harness
//! collect raw ACP JSON-RPC activity and publish owner-scoped encrypted relay
//! frames without exposing a local HTTP port.

use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

#[cfg(feature = "meeting-acceptance")]
use std::{
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    path::Path,
};

use serde::Serialize;
use tokio::sync::broadcast;

const OBSERVER_BUFFER_CAP: usize = 1_000;

/// Best-effort metadata attached to observer events.
#[derive(Clone, Debug, Default)]
pub struct ObserverContext {
    /// Buzz channel UUID for the current turn, when channel-scoped.
    pub channel_id: Option<String>,
    /// ACP session ID associated with the current turn, once known.
    pub session_id: Option<String>,
    /// Local UUID for one prompt turn.
    pub turn_id: Option<String>,
    /// RFC3339 timestamp at which the current turn began, when known.
    pub started_at: Option<String>,
}

/// Handle used by the harness to publish local observer events.
#[derive(Clone)]
pub struct ObserverHandle {
    inner: Arc<ObserverInner>,
}

struct ObserverInner {
    tx: broadcast::Sender<ObserverEvent>,
    buffer: Mutex<VecDeque<ObserverEvent>>,
    seq: AtomicU64,
    #[cfg(feature = "meeting-acceptance")]
    acceptance_sink: Option<Mutex<BufWriter<File>>>,
}

fn new_observer_handle() -> ObserverHandle {
    #[cfg(feature = "meeting-acceptance")]
    {
        new_observer_handle_with_sink(None)
    }
    #[cfg(not(feature = "meeting-acceptance"))]
    {
        let (tx, _) = broadcast::channel(OBSERVER_BUFFER_CAP);
        ObserverHandle {
            inner: Arc::new(ObserverInner {
                tx,
                buffer: Mutex::new(VecDeque::with_capacity(OBSERVER_BUFFER_CAP)),
                seq: AtomicU64::new(1),
            }),
        }
    }
}

#[cfg(feature = "meeting-acceptance")]
fn new_observer_handle_with_sink(acceptance_sink: Option<BufWriter<File>>) -> ObserverHandle {
    let (tx, _) = broadcast::channel(OBSERVER_BUFFER_CAP);
    ObserverHandle {
        inner: Arc::new(ObserverInner {
            tx,
            buffer: Mutex::new(VecDeque::with_capacity(OBSERVER_BUFFER_CAP)),
            seq: AtomicU64::new(1),
            acceptance_sink: acceptance_sink.map(Mutex::new),
        }),
    }
}

/// Event delivered through the in-process observer bus.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObserverEvent {
    /// Monotonic process-local sequence number.
    pub seq: u64,
    /// RFC3339 UTC timestamp.
    pub timestamp: String,
    /// Observer event kind, for example `acp_read` or `turn_started`.
    pub kind: String,
    /// Pool slot index for the agent process that emitted the event.
    pub agent_index: Option<usize>,
    /// Buzz channel UUID for channel-scoped events.
    pub channel_id: Option<String>,
    /// ACP session ID when known.
    pub session_id: Option<String>,
    /// Local UUID for one prompt turn.
    pub turn_id: Option<String>,
    /// RFC3339 timestamp at which the current turn began, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// Raw or semantic event payload.
    pub payload: serde_json::Value,
}

impl ObserverHandle {
    /// Create an in-process observer feed.
    pub fn in_process() -> Self {
        new_observer_handle()
    }

    /// Create an in-process feed that also writes a privacy-filtered,
    /// flush-on-every-event acceptance NDJSON file.
    #[cfg(feature = "meeting-acceptance")]
    pub(crate) fn in_process_with_acceptance_sink(path: &Path) -> std::io::Result<Self> {
        let file = OpenOptions::new().create_new(true).write(true).open(path)?;
        Ok(new_observer_handle_with_sink(Some(BufWriter::new(file))))
    }

    /// Subscribe to live observer events.
    pub fn subscribe(&self) -> broadcast::Receiver<ObserverEvent> {
        self.inner.tx.subscribe()
    }

    /// Return the current replay buffer.
    pub fn snapshot(&self) -> Vec<ObserverEvent> {
        match self.inner.buffer.lock() {
            Ok(buffer) => buffer.iter().cloned().collect(),
            Err(error) => {
                tracing::warn!(target: "observer", "observer replay buffer lock poisoned: {error}");
                Vec::new()
            }
        }
    }

    /// Emit a local observer event.
    pub fn emit(
        &self,
        kind: impl Into<String>,
        agent_index: Option<usize>,
        context: &ObserverContext,
        payload: serde_json::Value,
    ) {
        let event = ObserverEvent {
            seq: self.inner.seq.fetch_add(1, Ordering::Relaxed),
            timestamp: chrono::Utc::now().to_rfc3339(),
            kind: kind.into(),
            agent_index,
            channel_id: context.channel_id.clone(),
            session_id: context.session_id.clone(),
            turn_id: context.turn_id.clone(),
            started_at: context.started_at.clone(),
            payload,
        };

        match self.inner.buffer.lock() {
            Ok(mut buffer) => {
                if buffer.len() >= OBSERVER_BUFFER_CAP {
                    buffer.pop_front();
                }
                buffer.push_back(event.clone());
            }
            Err(error) => {
                tracing::warn!(target: "observer", "observer replay buffer lock poisoned: {error}");
            }
        }

        #[cfg(feature = "meeting-acceptance")]
        if acceptance_safe_kind(&event.kind) {
            match self
                .inner
                .acceptance_sink
                .as_ref()
                .map(Mutex::lock)
                .transpose()
            {
                Ok(Some(mut sink)) => {
                    if let Err(error) = serde_json::to_writer(&mut *sink, &event)
                        .and_then(|_| sink.write_all(b"\n").map_err(serde_json::Error::io))
                        .and_then(|_| sink.flush().map_err(serde_json::Error::io))
                    {
                        tracing::warn!(
                            target: "observer",
                            "acceptance observer NDJSON write failed: {error}"
                        );
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(
                        target: "observer",
                        "acceptance observer sink lock poisoned: {error}"
                    );
                }
            }
        }

        let _ = self.inner.tx.send(event);
    }
}

#[cfg(feature = "meeting-acceptance")]
fn acceptance_safe_kind(kind: &str) -> bool {
    matches!(
        kind,
        "harness_started"
            | "harness_stopped"
            | "turn_started"
            | "turn_completed"
            | "prompt_request_started"
            | "prompt_cancelled_before_request"
            | "prompt_terminal"
            | "acp_cancel_requested"
            | "acp_session_cancel_sent"
            | "agent_process_started"
            | "agent_process_terminal"
            | "agent_panic"
            | "respawn_scheduled"
            | "respawn_completed"
            | "respawn_failed"
            | "model_applied"
            | "meeting_v1_ended"
            | "meeting_v1_grant_received"
            | "meeting_v1_grant_yielded"
            | "meeting_v1_intent_completed"
            | "meeting_v1_intent_started"
            | "meeting_v1_moderator_action_submitted"
            | "meeting_v1_moderator_attempt_registered"
            | "meeting_v1_moderator_decision_committed"
            | "meeting_v1_moderator_decision_completed"
            | "meeting_v1_moderator_decision_discarded"
            | "meeting_v1_moderator_decision_rebased"
            | "meeting_v1_moderator_decision_retry_requested"
            | "meeting_v1_moderator_decision_retry_started"
            | "meeting_v1_moderator_decision_started"
            | "meeting_v1_moderator_decision_validated"
            | "meeting_v1_offer_decision"
            | "meeting_v1_progress"
            | "meeting_v1_reservation_released"
            | "meeting_v1_speech_submitted"
            | "meeting_v1_state_applied"
            | "meeting_v1_sync_completed"
            | "meeting_v1_turn_started"
            | "meeting_v2_board_load_completed"
            | "meeting_v2_board_load_discarded"
            | "meeting_v2_board_load_failed"
            | "meeting_v2_board_load_started"
            | "meeting_v2_board_turn_completed"
            | "meeting_v2_board_turn_queued"
            | "meeting_v2_action_format_retry"
            | "meeting_v2_action_turn_queued"
            | "meeting_v2_direct_action_turn_completed"
            | "meeting_v2_floor_turn_completed"
            | "meeting_v2_floor_turn_queued"
            | "meeting_v2_host_turn_discarded"
    )
}

/// Build observer context values from optional channel/session/turn IDs.
pub fn context_for(
    channel_id: Option<uuid::Uuid>,
    session_id: Option<String>,
    turn_id: Option<String>,
) -> ObserverContext {
    ObserverContext {
        channel_id: channel_id.map(|id| id.to_string()),
        session_id,
        turn_id,
        started_at: None,
    }
}

/// Attach the authoritative start timestamp to every observer frame for a turn.
pub fn context_for_turn(
    channel_id: Option<uuid::Uuid>,
    session_id: Option<String>,
    turn_id: String,
    started_at: String,
) -> ObserverContext {
    ObserverContext {
        channel_id: channel_id.map(|id| id.to_string()),
        session_id,
        turn_id: Some(turn_id),
        started_at: Some(started_at),
    }
}

#[cfg(all(test, feature = "meeting-acceptance"))]
mod acceptance_tests {
    use super::*;

    #[test]
    fn acceptance_sink_excludes_raw_wire_and_flushes_safe_events() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("events.ndjson");
        let observer = ObserverHandle::in_process_with_acceptance_sink(&path).unwrap();
        observer.emit(
            "acp_read",
            Some(0),
            &ObserverContext::default(),
            serde_json::json!({"private": "do-not-persist"}),
        );
        observer.emit(
            "meeting_v1_moderator_decision_started",
            Some(0),
            &ObserverContext::default(),
            serde_json::json!({"attempt_id": "attempt-1"}),
        );
        observer.emit(
            "meeting_v1_sync_failed",
            Some(0),
            &ObserverContext::default(),
            serde_json::json!({"error": "private failure detail"}),
        );
        observer.emit(
            "prompt_cancelled_before_request",
            Some(0),
            &ObserverContext::default(),
            serde_json::json!({"reason": "explicit_cancel"}),
        );
        observer.emit(
            "meeting_v2_board_turn_completed",
            Some(0),
            &ObserverContext::default(),
            serde_json::json!({
                "action": "UPDATE",
                "control_epoch": 7,
                "board_window": 3
            }),
        );

        let contents = std::fs::read_to_string(path).unwrap();
        assert!(!contents.contains("do-not-persist"));
        assert!(!contents.contains("private failure detail"));
        assert!(contents.contains("meeting_v1_moderator_decision_started"));
        assert!(contents.contains("attempt-1"));
        assert!(contents.contains("prompt_cancelled_before_request"));
        assert!(contents.contains("explicit_cancel"));
        assert!(contents.contains("meeting_v2_board_turn_completed"));
        assert!(contents.contains("\"board_window\":3"));
    }
}
