use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nostr::Keys;

use super::{AppliedWorkspaceCaptureError, WorkspaceSigningEligibility};
use crate::app_state::build_app_state;

const SEMANTIC_PROXY_PROBE_CHILD: &str =
    "app_state::workspace_transition::tests::semantic_proxy_probe_child";

fn serve_probe(
    listener: TcpListener,
    hit: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    response: &'static [u8],
) -> std::thread::JoinHandle<()> {
    listener
        .set_nonblocking(true)
        .expect("set probe listener nonblocking");
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && !stop.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    hit.store(true, Ordering::Release);
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut request = [0_u8; 2048];
                    let _ = stream.read(&mut request);
                    let _ = stream.write_all(response);
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("accept probe connection: {error}"),
            }
        }
    })
}

#[test]
fn semantic_query_client_bypasses_system_proxy() {
    let target = TcpListener::bind("127.0.0.1:0").expect("bind direct target");
    let target_addr = target.local_addr().expect("direct target address");
    let proxy = TcpListener::bind("127.0.0.1:0").expect("bind proxy trap");
    let proxy_addr = proxy.local_addr().expect("proxy trap address");
    let target_hit = Arc::new(AtomicBool::new(false));
    let proxy_hit = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let target_server = serve_probe(
        target,
        Arc::clone(&target_hit),
        Arc::clone(&stop),
        b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
    let proxy_server = serve_probe(
        proxy,
        Arc::clone(&proxy_hit),
        Arc::clone(&stop),
        b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
    let proxy_url = format!("http://{proxy_addr}");
    let status = Command::new(std::env::current_exe().expect("current test executable"))
        .args(["--exact", SEMANTIC_PROXY_PROBE_CHILD, "--nocapture"])
        .env(
            "CARRYFORTH_SEMANTIC_PROXY_PROBE_URL",
            format!("http://localhost:{}/probe", target_addr.port()),
        )
        .env("HTTP_PROXY", &proxy_url)
        .env("http_proxy", &proxy_url)
        .env("ALL_PROXY", &proxy_url)
        .env("all_proxy", &proxy_url)
        .env("NO_PROXY", "")
        .env("no_proxy", "")
        .status()
        .expect("run isolated proxy probe");
    stop.store(true, Ordering::Release);
    target_server.join().expect("join direct target");
    proxy_server.join().expect("join proxy trap");

    assert!(status.success(), "isolated semantic proxy probe failed");
    assert!(
        target_hit.load(Ordering::Acquire),
        "semantic request did not reach the direct loopback target"
    );
    assert!(
        !proxy_hit.load(Ordering::Acquire),
        "semantic request leaked through the configured system proxy"
    );
}

#[test]
fn semantic_proxy_probe_child() {
    let Ok(url) = std::env::var("CARRYFORTH_SEMANTIC_PROXY_PROBE_URL") else {
        return;
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build proxy probe runtime");
    let status = runtime.block_on(async {
        super::build_semantic_query_http_client()
            .expect("build semantic query client")
            .get(url)
            .send()
            .await
            .expect("send semantic proxy probe")
            .status()
    });
    assert_eq!(status, reqwest::StatusCode::NO_CONTENT);
}

#[test]
fn semantic_query_timeout_leaves_transport_grace_after_the_relay_budget() {
    assert_eq!(
        super::SEMANTIC_QUERY_HTTP_TIMEOUT,
        std::time::Duration::from_secs(195)
    );
    assert!(
        super::SEMANTIC_QUERY_HTTP_TIMEOUT
            > std::time::Duration::from_millis(u64::from(
                buzz_semantic_query_pkg::MAX_WALL_TIME_MS
            ),)
    );
}

#[test]
fn applied_workspace_capture_is_exact_and_redacted() {
    let state = build_app_state();
    let keys = Keys::generate();
    let applied = state
        .apply_workspace_transition(
            "community-a".to_owned(),
            "ws://localhost:3000".to_owned(),
            Some(keys.clone()),
        )
        .expect("apply workspace tuple");

    let captured = state
        .capture_applied_workspace("community-a", &applied.applied_workspace_token)
        .expect("exact tuple captures");
    assert_eq!(captured.relay_http_origin, "http://localhost:3000");
    assert_eq!(captured.caller, keys.public_key());
    assert_eq!(
        captured.signing_eligibility,
        WorkspaceSigningEligibility::Ready
    );
    assert!(matches!(
        state.capture_applied_workspace("community-b", &applied.applied_workspace_token),
        Err(AppliedWorkspaceCaptureError::Mismatch)
    ));
    assert!(matches!(
        state.capture_applied_workspace("community-a", "stale-token"),
        Err(AppliedWorkspaceCaptureError::Mismatch)
    ));

    let debug = format!("{captured:?}");
    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains(&applied.applied_workspace_token));
    assert!(!debug.contains("http://localhost:3000"));
}

#[test]
fn identity_replacement_rotates_applied_token_and_caller_atomically() {
    let state = build_app_state();
    let caller_a = Keys::generate();
    let caller_b = Keys::generate();
    let applied_a = state
        .apply_workspace_transition(
            "community-a".to_owned(),
            "ws://localhost:3000".to_owned(),
            Some(caller_a),
        )
        .expect("apply caller A");

    state
        .replace_runtime_identity(caller_b.clone(), false, false)
        .expect("replace caller");
    assert!(
        matches!(
            state.capture_applied_workspace("community-a", &applied_a.applied_workspace_token),
            Err(AppliedWorkspaceCaptureError::Mismatch)
        ),
        "the caller-A token must stop accepting immediately"
    );

    let applied_b = state
        .workspace_transition
        .lock()
        .expect("transition lock")
        .applied
        .clone()
        .expect("applied caller B");
    let captured_b = state
        .capture_applied_workspace("community-a", &applied_b.applied_workspace_token)
        .expect("caller B captures");
    assert_eq!(captured_b.caller, caller_b.public_key());
    assert_ne!(
        applied_a.applied_workspace_token,
        applied_b.applied_workspace_token
    );
}

#[test]
fn recovery_eligibility_fails_before_a_signing_capture() {
    let state = build_app_state();
    let applied = state
        .apply_workspace_transition(
            "community-a".to_owned(),
            "ws://localhost:3000".to_owned(),
            Some(Keys::generate()),
        )
        .expect("apply ready workspace");
    state
        .replace_runtime_identity(Keys::generate(), true, false)
        .expect("publish lost identity");
    let lost = state
        .workspace_transition
        .lock()
        .expect("transition lock")
        .applied
        .clone()
        .expect("lost tuple");
    assert_ne!(
        applied.applied_workspace_token,
        lost.applied_workspace_token
    );
    assert!(matches!(
        state.capture_applied_workspace("community-a", &lost.applied_workspace_token),
        Err(AppliedWorkspaceCaptureError::IdentityLost)
    ));

    state
        .replace_runtime_identity(Keys::generate(), false, true)
        .expect("publish locked identity");
    let locked = state
        .workspace_transition
        .lock()
        .expect("transition lock")
        .applied
        .clone()
        .expect("locked tuple");
    assert!(matches!(
        state.capture_applied_workspace("community-a", &locked.applied_workspace_token),
        Err(AppliedWorkspaceCaptureError::KeyringLocked)
    ));

    state
        .reset_failed
        .store(true, std::sync::atomic::Ordering::Release);
    let reset_failed = state
        .apply_workspace_transition(
            "community-a".to_owned(),
            "ws://localhost:3000".to_owned(),
            None,
        )
        .expect("publish reset-failed workspace");
    assert!(matches!(
        state.capture_applied_workspace("community-a", &reset_failed.applied_workspace_token,),
        Err(AppliedWorkspaceCaptureError::ResetFailed)
    ));
}
