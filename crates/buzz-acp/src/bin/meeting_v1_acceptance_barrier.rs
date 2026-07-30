#![deny(unsafe_code)]

use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const MAX_FRAME_BYTES: usize = 128 * 1024;

#[derive(Debug, Parser)]
#[command(
    name = "buzz-meeting-v1-acceptance-barrier",
    about = "Acceptance-only Meeting V1 pre-submit barrier"
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Serve one ACP barrier frame and wait for an explicit release client.
    Serve {
        #[arg(long)]
        socket: PathBuf,
        #[arg(long)]
        events: PathBuf,
    },
    /// Release the currently observed barrier token.
    Release {
        #[arg(long)]
        socket: PathBuf,
        #[arg(long)]
        token: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    match Args::parse().command {
        Command::Serve { socket, events } => serve(&socket, &events).await,
        Command::Release { socket, token } => release(&socket, &token).await,
    }
}

#[cfg(unix)]
async fn serve(socket_path: &Path, events_path: &Path) -> Result<()> {
    use tokio::net::UnixListener;

    if socket_path.exists() {
        return Err(anyhow!(
            "refusing to replace existing acceptance socket {}",
            socket_path.display()
        ));
    }
    let listener = UnixListener::bind(socket_path).with_context(|| {
        format!(
            "bind Meeting V1 acceptance socket {}",
            socket_path.display()
        )
    })?;
    let _socket_guard = SocketGuard(socket_path.to_path_buf());

    let (acp_stream, _) = listener
        .accept()
        .await
        .context("accept ACP barrier connection")?;
    let mut acp_stream = BufReader::new(acp_stream);
    let frame = read_json_line(&mut acp_stream)
        .await
        .context("read ACP barrier frame")?;
    if frame.get("frame_type").and_then(Value::as_str) != Some("meeting_v1_pre_submit") {
        return Err(anyhow!(
            "first barrier connection was not a pre-submit frame"
        ));
    }
    let token = frame
        .get("token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("pre-submit frame is missing its token"))?
        .to_string();
    let mut observed = frame;
    observed["barrier_observed_at"] = json!(chrono::Utc::now().to_rfc3339());
    append_ndjson(events_path, &observed)?;

    loop {
        let (release_stream, _) = listener
            .accept()
            .await
            .context("accept barrier release connection")?;
        let mut release_stream = BufReader::new(release_stream);
        let request = read_json_line(&mut release_stream)
            .await
            .context("read barrier release request")?;
        let supplied = request
            .get("token")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if request.get("command").and_then(Value::as_str) != Some("release") || supplied != token {
            let response = json!({
                "accepted": false,
                "reason": "token_mismatch",
            });
            write_json_line(release_stream.get_mut(), &response).await?;
            continue;
        }

        let released_at = chrono::Utc::now().to_rfc3339();
        let release = json!({
            "command": "release",
            "token": token,
            "released_at": released_at,
        });
        write_json_line(acp_stream.get_mut(), &release)
            .await
            .context("release ACP barrier connection")?;
        write_json_line(
            release_stream.get_mut(),
            &json!({"accepted": true, "token": token}),
        )
        .await
        .context("acknowledge barrier release client")?;
        append_ndjson(
            events_path,
            &json!({
                "frame_type": "meeting_v1_pre_submit_release",
                "token": token,
                "released_at": released_at,
            }),
        )?;
        return Ok(());
    }
}

#[cfg(not(unix))]
async fn serve(_socket_path: &Path, _events_path: &Path) -> Result<()> {
    Err(anyhow!(
        "Meeting V1 acceptance barrier requires Unix-domain sockets"
    ))
}

#[cfg(unix)]
async fn release(socket_path: &Path, token: &str) -> Result<()> {
    use tokio::net::UnixStream;

    let stream = UnixStream::connect(socket_path)
        .await
        .with_context(|| format!("connect acceptance socket {}", socket_path.display()))?;
    let mut stream = BufReader::new(stream);
    write_json_line(
        stream.get_mut(),
        &json!({"command": "release", "token": token}),
    )
    .await?;
    let response = read_json_line(&mut stream).await?;
    if response.get("accepted").and_then(Value::as_bool) != Some(true) {
        return Err(anyhow!("barrier release was rejected: {response}"));
    }
    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}

#[cfg(not(unix))]
async fn release(_socket_path: &Path, _token: &str) -> Result<()> {
    Err(anyhow!(
        "Meeting V1 acceptance barrier requires Unix-domain sockets"
    ))
}

async fn read_json_line<R>(reader: &mut R) -> Result<Value>
where
    R: AsyncBufReadExt + Unpin,
{
    let mut line = String::new();
    let bytes = reader.read_line(&mut line).await?;
    if bytes == 0 {
        return Err(anyhow!("peer closed without sending a frame"));
    }
    if bytes > MAX_FRAME_BYTES {
        return Err(anyhow!("barrier frame exceeds {MAX_FRAME_BYTES} bytes"));
    }
    serde_json::from_str(line.trim()).context("parse barrier JSON frame")
}

async fn write_json_line<W>(writer: &mut W, value: &Value) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let mut line = serde_json::to_vec(value)?;
    if line.len() > MAX_FRAME_BYTES {
        return Err(anyhow!("barrier response exceeds {MAX_FRAME_BYTES} bytes"));
    }
    line.push(b'\n');
    writer.write_all(&line).await?;
    writer.flush().await?;
    Ok(())
}

fn append_ndjson(path: &Path, value: &Value) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open acceptance evidence {}", path.display()))?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(())
}

#[cfg(unix)]
struct SocketGuard(PathBuf);

#[cfg(unix)]
impl Drop for SocketGuard {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.0) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "could not remove acceptance socket {}: {error}",
                    self.0.display()
                );
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tokio::net::UnixStream;

    #[tokio::test]
    async fn helper_records_one_frame_and_releases_the_matching_acp_connection() {
        let temporary = tempfile::tempdir().unwrap();
        let socket_path = temporary.path().join("barrier.sock");
        let events_path = temporary.path().join("events.ndjson");
        let server_socket = socket_path.clone();
        let server_events = events_path.clone();
        let server = tokio::spawn(async move { serve(&server_socket, &server_events).await });

        for _ in 0..100 {
            if socket_path.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        assert!(socket_path.exists());

        let acp_stream = UnixStream::connect(&socket_path).await.unwrap();
        let mut acp_stream = BufReader::new(acp_stream);
        write_json_line(
            acp_stream.get_mut(),
            &json!({
                "frame_type": "meeting_v1_pre_submit",
                "token": "token-1",
                "signed_event_id": "event-1",
            }),
        )
        .await
        .unwrap();

        for _ in 0..100 {
            if events_path.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        release(&socket_path, "token-1").await.unwrap();
        let response = read_json_line(&mut acp_stream).await.unwrap();
        assert_eq!(response["command"], "release");
        assert_eq!(response["token"], "token-1");

        server.await.unwrap().unwrap();
        let evidence = std::fs::read_to_string(events_path).unwrap();
        assert!(evidence.contains("\"frame_type\":\"meeting_v1_pre_submit\""));
        assert!(evidence.contains("\"frame_type\":\"meeting_v1_pre_submit_release\""));
        assert!(!socket_path.exists());
    }
}
