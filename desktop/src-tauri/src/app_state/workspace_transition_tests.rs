use nostr::Keys;

use super::{AppliedWorkspaceCaptureError, WorkspaceSigningEligibility};
use crate::app_state::build_app_state;

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
