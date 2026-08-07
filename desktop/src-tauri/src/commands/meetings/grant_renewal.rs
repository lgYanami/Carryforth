//! Native liveness keeper for a Human-held Meeting Speech Grant.
//!
//! React only ensures an exact verified Grant. The native task owns renewal,
//! re-reads canonical State before every signature, and retains the same
//! prepared event while delivery is indeterminate.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use buzz_sdk_pkg::{MeetingV1GrantProgressParams, MeetingV1ProgressStage};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    meeting_runtime::{
        MeetingGrantRenewalBinding, MeetingGrantRenewalRegistration, RegisterMeetingGrantRenewal,
    },
    relay::{
        parse_command_response, relay_api_base_url_with_override,
        submit_signed_event_at_with_keys_typed, RelayHttpError, RelayHttpErrorCategory,
        SubmitEventResponse,
    },
};

use super::pending::{canonical_hex64, canonical_uuid};
use super::{
    load_meeting_snapshot_at, read_meeting_identity_at, MeetingLifecycle, MeetingLoadResult,
    MeetingParticipantType,
};

const DEFAULT_PROGRESS_INTERVAL: Duration = Duration::from_secs(10);
const MIN_PROGRESS_INTERVAL: Duration = Duration::from_secs(1);
const RENEW_RETRY_DELAY: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Exact Human Grant that Desktop should retain until its hard deadline.
pub struct EnsureMeetingHumanGrantRenewalInput {
    meeting_id: String,
    grant_id: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EnsureMeetingHumanGrantRenewalResult {
    Started,
    AlreadyActive,
}

#[derive(Debug, Deserialize)]
struct GrantProgressReceipt {
    meeting_id: String,
    canonical_object_id: Option<String>,
    state_revision: Option<i64>,
    recovery_transitions: usize,
    duplicate: bool,
    outcome: String,
}

struct HumanGrantHead {
    keys: nostr::Keys,
    progress_seq: u64,
    progress_interval: Duration,
    soft_lease_expires_at_ms: i64,
    hard_deadline_ms: i64,
}

struct PreparedGrantProgress {
    event: nostr::Event,
    progress_seq: u64,
}

/// Ensure one process-local renewal owner for the current frozen Human Grant.
///
/// The task intentionally outlives the React Meeting route so a Human can
/// consult another Buzz surface without silently losing a five-minute turn.
#[tauri::command]
pub async fn ensure_meeting_human_grant_renewal(
    input: EnsureMeetingHumanGrantRenewalInput,
    app: AppHandle,
) -> Result<EnsureMeetingHumanGrantRenewalResult, String> {
    let meeting_id = canonical_uuid(&input.meeting_id, "Meeting ID")?;
    canonical_hex64(&input.grant_id, "Meeting Grant")?;

    let state = app.state::<AppState>();
    let api_base_url = relay_api_base_url_with_override(&state);
    let keys = state.signing_keys()?;
    let signer_pubkey = keys.public_key().to_hex();
    let Some(head) = load_human_grant_head(
        &state,
        &api_base_url,
        &signer_pubkey,
        &meeting_id,
        &input.grant_id,
    )
    .await?
    else {
        return Err(
            "only the frozen Human holder can renew the current active Meeting Grant".to_string(),
        );
    };
    let binding = MeetingGrantRenewalBinding {
        api_base_url,
        signer_pubkey,
        meeting_id,
        grant_id: input.grant_id,
        hard_deadline_ms: head.hard_deadline_ms,
    };
    match state.meeting_grant_renewals.register(binding)? {
        RegisterMeetingGrantRenewal::Existing => {
            Ok(EnsureMeetingHumanGrantRenewalResult::AlreadyActive)
        }
        RegisterMeetingGrantRenewal::Started(registration) => {
            tauri::async_runtime::spawn(run_human_grant_renewal(app.clone(), registration));
            Ok(EnsureMeetingHumanGrantRenewalResult::Started)
        }
    }
}

async fn load_human_grant_head(
    state: &AppState,
    api_base_url: &str,
    signer_pubkey: &str,
    meeting_id: &str,
    grant_id: &str,
) -> Result<Option<HumanGrantHead>, String> {
    if relay_api_base_url_with_override(state) != api_base_url {
        return Ok(None);
    }
    let keys = state.signing_keys()?;
    if keys.public_key().to_hex() != signer_pubkey {
        return Ok(None);
    }
    let Some(identity) = read_meeting_identity_at(state, api_base_url).await? else {
        return Ok(None);
    };
    let loaded = load_meeting_snapshot_at(state, &identity, meeting_id, api_base_url, &keys)
        .await
        .map_err(super::read_error_message)?;
    let MeetingLoadResult::Ready { snapshot } = loaded else {
        return Ok(None);
    };
    if !matches!(snapshot.lifecycle, MeetingLifecycle::Active)
        || snapshot.phase != "granted"
        || !snapshot.participants.iter().any(|participant| {
            participant.pubkey == signer_pubkey
                && participant.participant_type == MeetingParticipantType::Human
        })
    {
        return Ok(None);
    }
    let Some(grant) = snapshot
        .floor
        .as_ref()
        .and_then(|floor| floor.grant.as_ref())
    else {
        return Ok(None);
    };
    if grant.grant_id != grant_id
        || grant.holder_pubkey != signer_pubkey
        || grant.hard_deadline_ms <= 0
        || grant.soft_lease_expires_at_ms <= 0
    {
        return Ok(None);
    }
    let progress_interval = grant
        .progress_interval_ms
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_PROGRESS_INTERVAL)
        .max(MIN_PROGRESS_INTERVAL);
    Ok(Some(HumanGrantHead {
        keys,
        progress_seq: grant.progress_seq,
        progress_interval,
        soft_lease_expires_at_ms: grant.soft_lease_expires_at_ms,
        hard_deadline_ms: grant.hard_deadline_ms,
    }))
}

async fn load_exact_human_grant_head(
    state: &AppState,
    binding: &MeetingGrantRenewalBinding,
) -> Result<Option<HumanGrantHead>, String> {
    let head = load_human_grant_head(
        state,
        &binding.api_base_url,
        &binding.signer_pubkey,
        &binding.meeting_id,
        &binding.grant_id,
    )
    .await?;
    Ok(head.filter(|head| head.hard_deadline_ms == binding.hard_deadline_ms))
}

fn prepare_human_grant_progress(
    binding: &MeetingGrantRenewalBinding,
    head: &HumanGrantHead,
) -> Result<PreparedGrantProgress, String> {
    let session_id = Uuid::parse_str(&binding.meeting_id)
        .map_err(|error| format!("invalid Meeting ID after validation: {error}"))?;
    let progress_seq = head
        .progress_seq
        .checked_add(1)
        .ok_or_else(|| "Meeting Grant progress sequence overflow".to_string())?;
    let event = buzz_sdk_pkg::build_meeting_v2_grant_progress(MeetingV1GrantProgressParams {
        session_id,
        grant_id: &binding.grant_id,
        progress_seq,
        stage: MeetingV1ProgressStage::Composing,
    })
    .map_err(|error| format!("invalid Human Meeting Grant Progress: {error}"))?
    .sign_with_keys(&head.keys)
    .map_err(|error| format!("failed to sign Human Meeting Grant Progress: {error}"))?;
    Ok(PreparedGrantProgress {
        event,
        progress_seq,
    })
}

fn validate_progress_receipt(
    response: &SubmitEventResponse,
    binding: &MeetingGrantRenewalBinding,
    prepared: &PreparedGrantProgress,
) -> Result<GrantProgressReceipt, String> {
    if response.event_id != prepared.event.id.to_hex() || !response.accepted {
        return Err("Grant Progress response does not match the signed event".to_string());
    }
    let receipt: GrantProgressReceipt = parse_command_response(&response.message)?;
    if receipt.meeting_id != binding.meeting_id
        || receipt.canonical_object_id.as_deref() != Some(binding.grant_id.as_str())
        || receipt.state_revision.is_none_or(|revision| revision <= 0)
        || receipt.recovery_transitions > 64
        || receipt.outcome != "accepted"
    {
        return Err("Grant Progress receipt does not match the exact Grant".to_string());
    }
    Ok(receipt)
}

async fn wait_for_tick(cancel: &mut tokio::sync::watch::Receiver<bool>, delay: Duration) -> bool {
    if *cancel.borrow() {
        return false;
    }
    tokio::select! {
        _ = tokio::time::sleep(delay) => true,
        changed = cancel.changed() => changed.is_ok() && !*cancel.borrow(),
    }
}

fn definitive_progress_error(error: &RelayHttpError) -> bool {
    !error.request_may_have_reached_relay
        && matches!(
            error.category,
            RelayHttpErrorCategory::Forbidden
                | RelayHttpErrorCategory::Conflict
                | RelayHttpErrorCategory::Http
                | RelayHttpErrorCategory::Malformed
                | RelayHttpErrorCategory::Internal
        )
}

fn log_progress(
    binding: &MeetingGrantRenewalBinding,
    progress_seq: u64,
    outcome: &str,
    head: &HumanGrantHead,
) {
    let now_ms = local_now_ms();
    eprintln!(
        "buzz-desktop: Human Meeting Grant renewal meeting={} grant={} progress_seq={} outcome={} soft_remaining_ms={} hard_remaining_ms={}",
        binding.meeting_id,
        binding.grant_id,
        progress_seq,
        outcome,
        head.soft_lease_expires_at_ms.saturating_sub(now_ms),
        head.hard_deadline_ms.saturating_sub(now_ms),
    );
}

fn local_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

fn renewal_delay_at(head: &HumanGrantHead, now_ms: i64) -> Duration {
    let remaining_soft_ms = head.soft_lease_expires_at_ms.saturating_sub(now_ms);
    let Ok(remaining_soft_ms) = u64::try_from(remaining_soft_ms) else {
        // A canonical active Grant that appears expired against local wall
        // time most likely reflects clock skew. Keep the frozen cadence; only
        // Relay database time is allowed to expire the Grant.
        return head.progress_interval;
    };
    let half_remaining = Duration::from_millis(remaining_soft_ms / 2);
    if half_remaining < MIN_PROGRESS_INTERVAL {
        Duration::ZERO
    } else {
        head.progress_interval.min(half_remaining)
    }
}

fn renewal_delay(head: &HumanGrantHead) -> Duration {
    renewal_delay_at(head, local_now_ms())
}

async fn run_human_grant_renewal(
    app: AppHandle,
    mut registration: MeetingGrantRenewalRegistration,
) {
    let mut prepared: Option<PreparedGrantProgress> = None;
    // Renew immediately. This closes the snapshot/route delay window after ACK
    // and establishes a fresh lease before switching to the frozen cadence.
    let mut delay = Duration::ZERO;
    loop {
        if !wait_for_tick(&mut registration.cancel, delay).await {
            break;
        }
        let state = app.state::<AppState>();
        let head = match load_exact_human_grant_head(&state, &registration.binding).await {
            Ok(Some(head)) => head,
            Ok(None) => {
                eprintln!(
                    "buzz-desktop: Human Meeting Grant renewal stopped meeting={} grant={} reason=grant_not_active",
                    registration.binding.meeting_id, registration.binding.grant_id
                );
                break;
            }
            Err(error) => {
                eprintln!(
                    "buzz-desktop: Human Meeting Grant renewal read failed meeting={} grant={} error={error}",
                    registration.binding.meeting_id, registration.binding.grant_id
                );
                delay = RENEW_RETRY_DELAY;
                continue;
            }
        };

        if prepared
            .as_ref()
            .is_some_and(|pending| head.progress_seq >= pending.progress_seq)
        {
            if let Some(pending) = prepared.take() {
                log_progress(
                    &registration.binding,
                    pending.progress_seq,
                    "reconciled",
                    &head,
                );
            }
            delay = renewal_delay(&head);
            continue;
        }

        if prepared.is_none() {
            match prepare_human_grant_progress(&registration.binding, &head) {
                Ok(event) => prepared = Some(event),
                Err(error) => {
                    eprintln!(
                        "buzz-desktop: Human Meeting Grant renewal stopped meeting={} grant={} reason=prepare_failed error={error}",
                        registration.binding.meeting_id, registration.binding.grant_id
                    );
                    break;
                }
            }
        }
        let Some(pending) = prepared.as_ref() else {
            break;
        };
        let submit = submit_signed_event_at_with_keys_typed(
            &pending.event,
            &state,
            &registration.binding.api_base_url,
            &head.keys,
        );
        let response = tokio::select! {
            changed = registration.cancel.changed() => {
                let _ = changed;
                break;
            }
            response = submit => response,
        };
        match response {
            Ok(response) => {
                match validate_progress_receipt(&response, &registration.binding, pending) {
                    Ok(receipt) => {
                        let outcome = if receipt.duplicate {
                            "accepted_duplicate"
                        } else {
                            "accepted"
                        };
                        log_progress(&registration.binding, pending.progress_seq, outcome, &head);
                        // A receipt is not enough to allocate the next seq.
                        // Immediately reconcile canonical State; if that read
                        // is stale or fails, retain and later republish this
                        // exact signed event.
                        match load_exact_human_grant_head(&state, &registration.binding).await {
                            Ok(Some(canonical))
                                if canonical.progress_seq >= pending.progress_seq =>
                            {
                                prepared = None;
                                delay = renewal_delay(&canonical);
                            }
                            Ok(Some(_)) | Err(_) => delay = RENEW_RETRY_DELAY,
                            Ok(None) => break,
                        }
                    }
                    Err(error) => {
                        eprintln!(
                        "buzz-desktop: Human Meeting Grant renewal receipt needs reconciliation meeting={} grant={} progress_seq={} error={error}",
                        registration.binding.meeting_id,
                        registration.binding.grant_id,
                        pending.progress_seq,
                    );
                        delay = RENEW_RETRY_DELAY;
                    }
                }
            }
            Err(error) => {
                eprintln!(
                    "buzz-desktop: Human Meeting Grant renewal submit failed meeting={} grant={} progress_seq={} category={:?} error={}",
                    registration.binding.meeting_id,
                    registration.binding.grant_id,
                    pending.progress_seq,
                    error.category,
                    error.message,
                );
                if definitive_progress_error(&error) {
                    match load_exact_human_grant_head(&state, &registration.binding).await {
                        Ok(Some(canonical)) if canonical.progress_seq >= pending.progress_seq => {
                            log_progress(
                                &registration.binding,
                                pending.progress_seq,
                                "reconciled_after_error",
                                &canonical,
                            );
                            prepared = None;
                            delay = renewal_delay(&canonical);
                            continue;
                        }
                        Ok(Some(_)) | Ok(None) => break,
                        Err(_) => {}
                    }
                }
                delay = RENEW_RETRY_DELAY;
            }
        }
    }
    app.state::<AppState>()
        .meeting_grant_renewals
        .finish(&registration.key, registration.generation);
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEETING_ID: &str = "00000000-0000-4000-8000-000000000001";

    fn binding(keys: &nostr::Keys) -> MeetingGrantRenewalBinding {
        MeetingGrantRenewalBinding {
            api_base_url: "http://localhost:3000".to_string(),
            signer_pubkey: keys.public_key().to_hex(),
            meeting_id: MEETING_ID.to_string(),
            grant_id: "77".repeat(32),
            hard_deadline_ms: 300_000,
        }
    }

    fn head(keys: nostr::Keys, progress_seq: u64) -> HumanGrantHead {
        HumanGrantHead {
            keys,
            progress_seq,
            progress_interval: Duration::from_secs(10),
            soft_lease_expires_at_ms: 30_000,
            hard_deadline_ms: 300_000,
        }
    }

    #[test]
    fn progress_uses_exact_grant_next_sequence_and_composing_stage() {
        let keys = nostr::Keys::generate();
        let binding = binding(&keys);
        let prepared = prepare_human_grant_progress(&binding, &head(keys, 4))
            .unwrap_or_else(|error| panic!("prepare Human Grant Progress: {error}"));

        assert_eq!(
            super::super::single_tag(&prepared.event, "action"),
            Some("progress")
        );
        assert_eq!(
            super::super::single_tag(&prepared.event, "meeting-grant"),
            Some(binding.grant_id.as_str())
        );
        assert_eq!(
            super::super::single_tag(&prepared.event, "progress-seq"),
            Some("5")
        );
        assert_eq!(
            super::super::single_tag(&prepared.event, "stage"),
            Some("composing")
        );
        assert_eq!(prepared.progress_seq, 5);
    }

    #[test]
    fn progress_receipt_is_bound_to_event_meeting_and_grant() {
        let keys = nostr::Keys::generate();
        let binding = binding(&keys);
        let prepared = prepare_human_grant_progress(&binding, &head(keys, 0))
            .unwrap_or_else(|error| panic!("prepare Human Grant Progress: {error}"));
        let response = SubmitEventResponse {
            event_id: prepared.event.id.to_hex(),
            accepted: true,
            message: format!(
                "response:{}",
                serde_json::json!({
                    "meeting_id": binding.meeting_id,
                    "canonical_object_id": binding.grant_id,
                    "state_revision": 7,
                    "recovery_transitions": 0,
                    "duplicate": false,
                    "outcome": "accepted"
                })
            ),
        };

        let receipt = validate_progress_receipt(&response, &binding, &prepared)
            .unwrap_or_else(|error| panic!("validate Human Grant receipt: {error}"));
        assert!(!receipt.duplicate);

        let mismatched = SubmitEventResponse {
            event_id: prepared.event.id.to_hex(),
            accepted: true,
            message: format!(
                "response:{}",
                serde_json::json!({
                    "meeting_id": binding.meeting_id,
                    "canonical_object_id": "88".repeat(32),
                    "state_revision": 8,
                    "recovery_transitions": 0,
                    "duplicate": false,
                    "outcome": "accepted"
                })
            ),
        };
        assert!(validate_progress_receipt(&mismatched, &binding, &prepared).is_err());
    }

    #[test]
    fn only_definitive_precommit_failures_stop_without_retry() {
        let conflict = RelayHttpError {
            status: Some(409),
            category: RelayHttpErrorCategory::Conflict,
            message: "conflict".to_string(),
            retry_after_seconds: None,
            request_may_have_reached_relay: false,
        };
        assert!(definitive_progress_error(&conflict));

        let timeout = RelayHttpError {
            status: None,
            category: RelayHttpErrorCategory::Timeout,
            message: "timeout".to_string(),
            retry_after_seconds: None,
            request_may_have_reached_relay: true,
        };
        assert!(!definitive_progress_error(&timeout));
    }

    #[test]
    fn renewal_delay_uses_frozen_cadence_and_half_of_soft_remaining() {
        let keys = nostr::Keys::generate();
        let mut head = head(keys, 0);
        head.soft_lease_expires_at_ms = 30_000;

        assert_eq!(renewal_delay_at(&head, 0), Duration::from_secs(10));
        assert_eq!(
            renewal_delay_at(&head, 21_000),
            Duration::from_millis(4_500)
        );
        assert_eq!(renewal_delay_at(&head, 29_999), Duration::ZERO);
        assert_eq!(renewal_delay_at(&head, 30_001), Duration::from_secs(10));
    }
}
