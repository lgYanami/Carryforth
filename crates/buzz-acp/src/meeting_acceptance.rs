//! Acceptance-only Meeting evidence and timing hooks.
//!
//! This module is compiled only with the `meeting-acceptance` feature. It
//! deliberately stays out of production binaries: the local Unix-socket
//! barrier exists solely to make real-provider race acceptance reproducible.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

pub(crate) const ACCEPTANCE_EVENTS_PATH_ENV: &str = "BUZZ_ACP_MEETING_ACCEPTANCE_EVENTS_PATH";
pub(crate) const PRE_SUBMIT_BARRIER_SOCKET_ENV: &str = "BUZZ_ACP_MEETING_PRE_SUBMIT_BARRIER_SOCKET";
const LEGACY_ACCEPTANCE_EVENTS_PATH_ENV: &str = "BUZZ_ACP_MEETING_V1_ACCEPTANCE_EVENTS_PATH";
const LEGACY_PRE_SUBMIT_BARRIER_SOCKET_ENV: &str = "BUZZ_ACP_MEETING_V1_PRE_SUBMIT_BARRIER_SOCKET";

const MAX_BARRIER_FRAME_BYTES: usize = 128 * 1024;

/// One source reference from the Relay-authoritative Candidate Cohort.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct AcceptanceCandidateRef {
    pub(crate) source_type: String,
    pub(crate) source_id: String,
    pub(crate) current_event_id: Option<String>,
    pub(crate) author_pubkey: Option<String>,
    pub(crate) eligible_decision_epoch: u64,
}

/// Privacy-safe evidence published immediately before a primary Moderator
/// action enters the normal protocol submission timeout.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct PreSubmitBarrierFrame {
    pub(crate) frame_type: &'static str,
    pub(crate) token: String,
    pub(crate) harness_pid: u32,
    pub(crate) session_id: String,
    pub(crate) turn_id: Option<String>,
    pub(crate) attempt_id: String,
    pub(crate) control_epoch: u64,
    pub(crate) decision_epoch: u64,
    pub(crate) attempt_number: u64,
    pub(crate) speech_revision: u64,
    pub(crate) snapshot_intent_revision: u64,
    pub(crate) current_intent_revision: u64,
    pub(crate) candidate_snapshot_hash: String,
    pub(crate) candidate_cohort: Vec<AcceptanceCandidateRef>,
    pub(crate) selected_source_type: String,
    pub(crate) selected_source_id: String,
    pub(crate) selected_source_event_id: Option<String>,
    pub(crate) action_kind: String,
    pub(crate) signed_event_id: String,
    pub(crate) hard_deadline_unix_ms: i64,
}

/// Per-controller one-shot acceptance barrier configuration.
#[derive(Debug, Clone)]
pub(crate) struct PreSubmitAcceptanceBarrier {
    socket_path: Option<PathBuf>,
    claimed: bool,
}

impl PreSubmitAcceptanceBarrier {
    pub(crate) fn from_env() -> Self {
        Self {
            socket_path: nonempty_env_path(PRE_SUBMIT_BARRIER_SOCKET_ENV)
                .or_else(|| nonempty_env_path(LEGACY_PRE_SUBMIT_BARRIER_SOCKET_ENV)),
            claimed: false,
        }
    }

    /// Claim the configured barrier exactly once for this ACP process.
    pub(crate) fn claim(&mut self) -> Option<PathBuf> {
        if self.claimed {
            return None;
        }
        let socket_path = self.socket_path.clone()?;
        self.claimed = true;
        Some(socket_path)
    }
}

pub(crate) fn acceptance_events_path() -> Option<PathBuf> {
    nonempty_env_path(ACCEPTANCE_EVENTS_PATH_ENV)
        .or_else(|| nonempty_env_path(LEGACY_ACCEPTANCE_EVENTS_PATH_ENV))
}

fn nonempty_env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Wait for the acceptance runner to release one already-signed primary
/// Moderator action. The protocol submit timeout is intentionally started by
/// the caller only after this function returns.
pub(crate) async fn await_pre_submit_release(
    socket_path: &Path,
    frame: &PreSubmitBarrierFrame,
) -> Result<Duration> {
    let remaining_ms = frame
        .hard_deadline_unix_ms
        .saturating_sub(chrono::Utc::now().timestamp_millis());
    if remaining_ms <= 0 {
        return Err(anyhow!(
            "acceptance barrier was reached after the authoritative attempt deadline"
        ));
    }
    let remaining = Duration::from_millis(remaining_ms as u64);
    tokio::time::timeout(
        remaining,
        await_pre_submit_release_inner(socket_path, frame),
    )
    .await
    .map_err(|_| anyhow!("acceptance barrier release exceeded the authoritative deadline"))?
}

#[cfg(unix)]
async fn await_pre_submit_release_inner(
    socket_path: &Path,
    frame: &PreSubmitBarrierFrame,
) -> Result<Duration> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    let started = std::time::Instant::now();
    let stream = UnixStream::connect(socket_path).await.with_context(|| {
        format!(
            "connect Meeting acceptance barrier {}",
            socket_path.display()
        )
    })?;
    let mut stream = BufReader::new(stream);
    let mut serialized =
        serde_json::to_vec(frame).context("serialize Meeting acceptance barrier frame")?;
    if serialized.len() > MAX_BARRIER_FRAME_BYTES {
        return Err(anyhow!(
            "Meeting acceptance barrier frame exceeds {MAX_BARRIER_FRAME_BYTES} bytes"
        ));
    }
    serialized.push(b'\n');
    stream
        .get_mut()
        .write_all(&serialized)
        .await
        .context("write Meeting acceptance barrier frame")?;
    stream
        .get_mut()
        .flush()
        .await
        .context("flush Meeting acceptance barrier frame")?;

    let mut response = String::new();
    stream
        .read_line(&mut response)
        .await
        .context("read Meeting acceptance barrier release")?;
    if response.len() > MAX_BARRIER_FRAME_BYTES {
        return Err(anyhow!("Meeting acceptance barrier response is too large"));
    }
    let release: BarrierRelease =
        serde_json::from_str(response.trim()).context("parse acceptance barrier release")?;
    if release.command != "release" || release.token != frame.token {
        return Err(anyhow!(
            "acceptance barrier returned a mismatched release token"
        ));
    }
    Ok(started.elapsed())
}

#[cfg(not(unix))]
async fn await_pre_submit_release_inner(
    _socket_path: &Path,
    _frame: &PreSubmitBarrierFrame,
) -> Result<Duration> {
    Err(anyhow!(
        "Meeting acceptance barrier requires a Unix-domain socket"
    ))
}

#[derive(Debug, Deserialize)]
struct BarrierRelease {
    command: String,
    token: String,
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::UnixListener,
    };

    fn frame(deadline_ms: i64) -> PreSubmitBarrierFrame {
        PreSubmitBarrierFrame {
            frame_type: "meeting_v1_pre_submit",
            token: "token-1".to_string(),
            harness_pid: 42,
            session_id: uuid::Uuid::nil().to_string(),
            turn_id: Some("turn-1".to_string()),
            attempt_id: "attempt-1".to_string(),
            control_epoch: 1,
            decision_epoch: 2,
            attempt_number: 1,
            speech_revision: 3,
            snapshot_intent_revision: 4,
            current_intent_revision: 5,
            candidate_snapshot_hash: "hash".to_string(),
            candidate_cohort: Vec::new(),
            selected_source_type: "intent".to_string(),
            selected_source_id: "intent-1".to_string(),
            selected_source_event_id: Some("event-1".to_string()),
            action_kind: "select_intent".to_string(),
            signed_event_id: "signed-1".to_string(),
            hard_deadline_unix_ms: deadline_ms,
        }
    }

    #[tokio::test]
    async fn barrier_publishes_before_waiting_for_matching_release() {
        let temporary = tempfile::tempdir().unwrap();
        let socket = temporary.path().join("barrier.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            let observed: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
            assert_eq!(observed["signed_event_id"], "signed-1");
            stream
                .get_mut()
                .write_all(b"{\"command\":\"release\",\"token\":\"token-1\"}\n")
                .await
                .unwrap();
            stream.get_mut().flush().await.unwrap();
        });

        let elapsed = await_pre_submit_release(
            &socket,
            &frame(chrono::Utc::now().timestamp_millis() + 5_000),
        )
        .await
        .unwrap();
        assert!(elapsed < Duration::from_secs(5));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn barrier_rejects_a_mismatched_release() {
        let temporary = tempfile::tempdir().unwrap();
        let socket = temporary.path().join("barrier.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            stream
                .get_mut()
                .write_all(b"{\"command\":\"release\",\"token\":\"wrong\"}\n")
                .await
                .unwrap();
            stream.get_mut().flush().await.unwrap();
        });

        let error = await_pre_submit_release(
            &socket,
            &frame(chrono::Utc::now().timestamp_millis() + 5_000),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("mismatched release token"));
        server.await.unwrap();
    }
}
