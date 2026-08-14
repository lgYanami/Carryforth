//! Bounded, read-only access to Relay-authoritative Meeting context.
//!
//! The tool deliberately maps a small enum to fixed `cf` CLI arguments and
//! starts the session shim directly. It never evaluates a shell command string
//! and cannot reach write-capable Meeting subcommands.

use crate::shell::SharedState;
use rmcp::{
    model::{CallToolResult, Content},
    ErrorData,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::{
    path::Path,
    process::{ExitStatus, Stdio},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const DEFAULT_LIMIT: u32 = 100;
const MAX_LIMIT: u32 = 500;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);
const REAP_TIMEOUT: Duration = Duration::from_secs(1);
const READER_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_STDOUT_BYTES: usize = 256 * 1024;
const MAX_STDERR_BYTES: usize = 16 * 1024;
const READ_CHUNK_BYTES: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
/// Fixed Relay-authoritative Meeting reads available to an Agent session.
pub enum MeetingReadOperation {
    Show,
    Participants,
    History,
    Intents,
    FloorStatus,
    FloorHistory,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
/// Validated arguments for one bounded [`MeetingReadOperation`].
pub struct MeetingReadParams {
    /// Fixed read operation to perform.
    pub operation: MeetingReadOperation,
    /// Canonical, hyphenated Meeting UUID.
    pub meeting: String,
    /// Result count for history/floor_history only (default 100, maximum 500).
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Eq, PartialEq)]
struct Invocation {
    args: Vec<String>,
}

#[derive(Default)]
struct Captured {
    bytes: Vec<u8>,
    exceeded: bool,
}

enum WaitOutcome {
    Completed(ExitStatus),
    Cancelled,
    TimedOut,
    Failed(String),
}

/// Execute one fixed Meeting read through the session-scoped Carryforth CLI shim.
pub async fn run(
    state: &SharedState,
    params: MeetingReadParams,
    cancellation: CancellationToken,
) -> Result<CallToolResult, ErrorData> {
    let binary = state.shim.cf_path();
    run_with_binary(state, &binary, params, cancellation, COMMAND_TIMEOUT).await
}

async fn run_with_binary(
    state: &SharedState,
    binary: &Path,
    params: MeetingReadParams,
    cancellation: CancellationToken,
    timeout: Duration,
) -> Result<CallToolResult, ErrorData> {
    let invocation = build_invocation(params)?;
    let mut command = Command::new(binary);
    command
        .args(&invocation.args)
        .current_dir(&state.cwd)
        .env("PATH", &state.shim.path_env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // CARRYFORTH_PRIVATE_KEY and CARRYFORTH_RELAY_URL are intentionally inherited by the
    // fixed Carryforth CLI process. NOSTR_PRIVATE_KEY was removed during Shim setup.
    for (key, value) in &state.shim.git_env {
        command.env(key, value);
    }
    crate::configure_no_window_async(&mut command);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return tool_error(format!(
                "meeting_read failed to start Carryforth CLI: {error}"
            ));
        }
    };
    let mut stdout_reader = spawn_reader(child.stdout.take(), MAX_STDOUT_BYTES);
    let mut stderr_reader = spawn_reader(child.stderr.take(), MAX_STDERR_BYTES);

    let outcome = tokio::select! {
        biased;
        _ = cancellation.cancelled() => WaitOutcome::Cancelled,
        result = tokio::time::timeout(timeout, child.wait()) => match result {
            Ok(Ok(status)) => WaitOutcome::Completed(status),
            Ok(Err(error)) => WaitOutcome::Failed(error.to_string()),
            Err(_) => WaitOutcome::TimedOut,
        },
    };

    if !matches!(outcome, WaitOutcome::Completed(_)) {
        let _ = child.start_kill();
        let _ = tokio::time::timeout(REAP_TIMEOUT, child.wait()).await;
    }

    let stdout = finish_reader(&mut stdout_reader).await;
    let stderr = finish_reader(&mut stderr_reader).await;

    match outcome {
        WaitOutcome::Cancelled => tool_error("meeting_read was cancelled"),
        WaitOutcome::TimedOut => tool_error(format!(
            "meeting_read exceeded its {} second timeout",
            timeout.as_secs_f64()
        )),
        WaitOutcome::Failed(error) => tool_error(format!(
            "meeting_read failed while waiting for Carryforth CLI: {error}"
        )),
        WaitOutcome::Completed(status) => finish_completed(status, stdout, stderr),
    }
}

fn build_invocation(params: MeetingReadParams) -> Result<Invocation, ErrorData> {
    let meeting = canonical_meeting_id(&params.meeting)?;
    let limit = match params.operation {
        MeetingReadOperation::History | MeetingReadOperation::FloorHistory => {
            Some(validate_limit(params.limit)?)
        }
        _ if params.limit.is_some() => {
            return Err(ErrorData::invalid_params(
                "limit is allowed only for history and floor_history".to_string(),
                None,
            ));
        }
        _ => None,
    };

    let mut args = vec![
        "--format".to_string(),
        "compact".to_string(),
        "meetings".to_string(),
    ];
    match params.operation {
        MeetingReadOperation::Show => args.push("show".to_string()),
        MeetingReadOperation::Participants => args.push("participants".to_string()),
        MeetingReadOperation::History => args.push("history".to_string()),
        MeetingReadOperation::Intents => {
            args.extend(["intents".to_string(), "list".to_string()]);
        }
        MeetingReadOperation::FloorStatus => {
            args.extend(["floor".to_string(), "status".to_string()]);
        }
        MeetingReadOperation::FloorHistory => {
            args.extend(["floor".to_string(), "history".to_string()]);
        }
    }
    args.extend(["--meeting".to_string(), meeting]);
    if let Some(limit) = limit {
        args.extend(["--limit".to_string(), limit.to_string()]);
    }
    Ok(Invocation { args })
}

fn canonical_meeting_id(raw: &str) -> Result<String, ErrorData> {
    if raw.len() != 36 {
        return Err(ErrorData::invalid_params(
            "meeting must be a canonical, hyphenated UUID".to_string(),
            None,
        ));
    }
    let parsed = Uuid::parse_str(raw).map_err(|_| {
        ErrorData::invalid_params(
            "meeting must be a canonical, hyphenated UUID".to_string(),
            None,
        )
    })?;
    let canonical = parsed.to_string();
    if raw != canonical {
        return Err(ErrorData::invalid_params(
            "meeting must be a lowercase canonical UUID".to_string(),
            None,
        ));
    }
    Ok(canonical)
}

fn validate_limit(requested: Option<u32>) -> Result<u32, ErrorData> {
    let limit = requested.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(ErrorData::invalid_params(
            format!("limit must be between 1 and {MAX_LIMIT}"),
            None,
        ));
    }
    Ok(limit)
}

fn spawn_reader<R>(pipe: Option<R>, limit: usize) -> JoinHandle<Captured>
where
    R: AsyncRead + Send + Unpin + 'static,
{
    tokio::spawn(async move {
        match pipe {
            Some(pipe) => read_bounded(pipe, limit).await,
            None => Captured::default(),
        }
    })
}

async fn read_bounded<R>(mut reader: R, limit: usize) -> Captured
where
    R: AsyncRead + Unpin,
{
    let mut captured = Captured {
        bytes: Vec::with_capacity(limit.min(READ_CHUNK_BYTES)),
        exceeded: false,
    };
    let mut chunk = [0u8; READ_CHUNK_BYTES];
    loop {
        let count = match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(count) => count,
        };
        let remaining = limit.saturating_sub(captured.bytes.len());
        captured
            .bytes
            .extend_from_slice(&chunk[..count.min(remaining)]);
        captured.exceeded |= count > remaining;
    }
    captured
}

async fn finish_reader(reader: &mut JoinHandle<Captured>) -> Captured {
    match tokio::time::timeout(READER_TIMEOUT, &mut *reader).await {
        Ok(Ok(captured)) => captured,
        Ok(Err(_)) | Err(_) => {
            reader.abort();
            Captured {
                bytes: Vec::new(),
                exceeded: true,
            }
        }
    }
}

fn finish_completed(
    status: ExitStatus,
    stdout: Captured,
    stderr: Captured,
) -> Result<CallToolResult, ErrorData> {
    if stdout.exceeded {
        return tool_error(format!(
            "meeting_read output exceeded {MAX_STDOUT_BYTES} bytes; retry history with a smaller limit"
        ));
    }
    let stdout_text = match String::from_utf8(stdout.bytes) {
        Ok(text) => text,
        Err(_) => return tool_error("meeting_read returned non-UTF-8 output"),
    };
    if !status.success() {
        let diagnostic = String::from_utf8_lossy(&stderr.bytes);
        let suffix = if stderr.exceeded {
            "\n[stderr truncated]"
        } else {
            ""
        };
        return tool_error(format!(
            "Carryforth CLI exited with {}: {}{}",
            status.code().unwrap_or(-1),
            diagnostic.trim(),
            suffix
        ));
    }
    if stdout_text.trim().is_empty() {
        return tool_error("meeting_read returned no output");
    }
    Ok(CallToolResult::success(vec![Content::text(
        stdout_text.trim_end().to_string(),
    )]))
}

fn tool_error(message: impl Into<String>) -> Result<CallToolResult, ErrorData> {
    Ok(CallToolResult::error(vec![Content::text(message.into())]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shim::Shim;
    #[cfg(unix)]
    use tempfile::{tempdir, TempDir};

    const MEETING_ID: &str = "123e4567-e89b-12d3-a456-426614174000";

    fn params(operation: MeetingReadOperation, limit: Option<u32>) -> MeetingReadParams {
        MeetingReadParams {
            operation,
            meeting: MEETING_ID.to_string(),
            limit,
        }
    }

    fn args(operation: MeetingReadOperation, limit: Option<u32>) -> Vec<String> {
        build_invocation(params(operation, limit))
            .expect("valid invocation")
            .args
    }

    #[test]
    fn maps_only_the_six_read_operations_to_fixed_cli_arguments() {
        assert_eq!(
            args(MeetingReadOperation::Show, None),
            [
                "--format",
                "compact",
                "meetings",
                "show",
                "--meeting",
                MEETING_ID
            ]
        );
        assert_eq!(
            args(MeetingReadOperation::Participants, None),
            [
                "--format",
                "compact",
                "meetings",
                "participants",
                "--meeting",
                MEETING_ID
            ]
        );
        assert_eq!(
            args(MeetingReadOperation::History, Some(7)),
            [
                "--format",
                "compact",
                "meetings",
                "history",
                "--meeting",
                MEETING_ID,
                "--limit",
                "7"
            ]
        );
        assert_eq!(
            args(MeetingReadOperation::Intents, None),
            [
                "--format",
                "compact",
                "meetings",
                "intents",
                "list",
                "--meeting",
                MEETING_ID
            ]
        );
        assert_eq!(
            args(MeetingReadOperation::FloorStatus, None),
            [
                "--format",
                "compact",
                "meetings",
                "floor",
                "status",
                "--meeting",
                MEETING_ID
            ]
        );
        assert_eq!(
            args(MeetingReadOperation::FloorHistory, None),
            [
                "--format",
                "compact",
                "meetings",
                "floor",
                "history",
                "--meeting",
                MEETING_ID,
                "--limit",
                "100"
            ]
        );
    }

    #[test]
    fn rejects_noncanonical_ids_and_out_of_scope_limits() {
        let bad_id = MeetingReadParams {
            operation: MeetingReadOperation::Show,
            meeting: format!("{MEETING_ID};say"),
            limit: None,
        };
        assert!(build_invocation(bad_id).is_err());
        assert!(build_invocation(params(MeetingReadOperation::Show, Some(1))).is_err());
        assert!(build_invocation(params(MeetingReadOperation::History, Some(0))).is_err());
        assert!(build_invocation(params(MeetingReadOperation::History, Some(501))).is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg(unix)]
    async fn fake_binary_receives_arguments_without_shell_evaluation() {
        let (directory, binary) = fake_binary("#!/bin/sh\nprintf '%s\\n' \"$@\"\n");
        let state = make_state(&directory);
        let result = run_with_binary(
            &state,
            &binary,
            params(MeetingReadOperation::FloorHistory, Some(3)),
            CancellationToken::new(),
            Duration::from_secs(2),
        )
        .await
        .expect("tool result");
        assert_eq!(result.is_error, Some(false));
        let output = result.content[0]
            .as_text()
            .expect("text result")
            .text
            .lines()
            .collect::<Vec<_>>();
        assert_eq!(
            output,
            [
                "--format",
                "compact",
                "meetings",
                "floor",
                "history",
                "--meeting",
                MEETING_ID,
                "--limit",
                "3"
            ]
        );
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg(unix)]
    async fn fake_binary_is_stopped_at_the_tool_timeout() {
        let (directory, binary) = fake_binary("#!/bin/sh\nwhile :; do :; done\n");
        let state = make_state(&directory);
        let result = run_with_binary(
            &state,
            &binary,
            params(MeetingReadOperation::Show, None),
            CancellationToken::new(),
            Duration::from_millis(50),
        )
        .await
        .expect("tool result");
        assert_eq!(result.is_error, Some(true));
        let output = result.content[0].as_text().expect("text result");
        assert!(output.text.contains("timeout"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bounded_reader_drains_but_retains_only_the_cap() {
        let input = vec![b'x'; 32];
        let captured = read_bounded(input.as_slice(), 8).await;
        assert_eq!(captured.bytes, vec![b'x'; 8]);
        assert!(captured.exceeded);
    }

    #[cfg(unix)]
    fn fake_binary(script: &str) -> (TempDir, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().expect("tempdir");
        let binary = directory.path().join("fake-cf");
        std::fs::write(&binary, script).expect("write fake binary");
        let mut permissions = std::fs::metadata(&binary)
            .expect("fake binary metadata")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&binary, permissions).expect("chmod fake binary");
        (directory, binary)
    }

    #[cfg(unix)]
    fn make_state(directory: &TempDir) -> SharedState {
        let shim = Shim::install().expect("shim install");
        SharedState::new(directory.path().to_path_buf(), shim).expect("state")
    }
}
