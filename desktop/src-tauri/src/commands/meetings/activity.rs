//! Bounded, product-level Meeting activity projected from verified Relay State.

use std::collections::{BTreeMap, BTreeSet};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use buzz_core_pkg::kind::{
    KIND_MEETING_ACTION_COMMAND, KIND_MEETING_BOARD_COMMAND, KIND_MEETING_CREATE, KIND_MEETING_END,
    KIND_MEETING_FLOOR_CLAIM, KIND_MEETING_FLOOR_SIGNAL, KIND_MEETING_GRANT_SIGNAL,
    KIND_MEETING_HUMAN_FLOOR_REQUEST, KIND_MEETING_MODERATOR_COMMAND, KIND_MEETING_OFFER_RESPONSE,
    KIND_MEETING_SPEECH_INTENT, KIND_MEETING_STATE, KIND_STREAM_MESSAGE,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use tauri::State;

use super::*;

const DEFAULT_ACTIVITY_PAGE_SIZE: usize = 30;
const MAX_ACTIVITY_PAGE_SIZE: usize = 50;
const MAX_ACTIVITY_SCAN_SIZE: usize = 500;
const ACTIVITY_SCAN_MULTIPLIER: usize = 8;
const MAX_TRANSITION_EFFECTS: usize = 128;
const MAX_JAVASCRIPT_TIMESTAMP_MS: i64 = 8_640_000_000_000_000;
const ACTIVITY_CURSOR_VERSION: u8 = 1;

#[derive(Debug, Deserialize, Serialize)]
struct ActivityCursor {
    version: u8,
    before: u64,
    before_id: String,
}

/// Load one bounded page of verified, sanitized Meeting control activity.
#[tauri::command]
pub async fn get_meeting_activities(
    meeting_id: String,
    cursor: Option<String>,
    limit: Option<usize>,
    state: State<'_, AppState>,
) -> Result<MeetingActivityPage, String> {
    let meeting_id = canonical_meeting_id(&meeting_id)?;
    let limit = limit
        .unwrap_or(DEFAULT_ACTIVITY_PAGE_SIZE)
        .clamp(1, MAX_ACTIVITY_PAGE_SIZE);
    let cursor = cursor.as_deref().map(decode_activity_cursor).transpose()?;
    let Some(identity) = read_meeting_identity(&state).await? else {
        return Err("Meeting V2 is not supported by this Community".to_string());
    };
    let loaded = load_meeting_snapshot(&state, &identity, &meeting_id)
        .await
        .map_err(read_error_message)?;
    let MeetingLoadResult::Ready { snapshot } = loaded else {
        return Err("Meeting activity is unavailable for this Meeting".to_string());
    };

    let scan_limit = limit
        .saturating_mul(ACTIVITY_SCAN_MULTIPLIER)
        .clamp(limit + 1, MAX_ACTIVITY_SCAN_SIZE);
    let filters = [
        json!({
            "kinds": [KIND_MEETING_CREATE],
            "#h": [&meeting_id],
            "limit": SNAPSHOT_EVENT_LIMIT,
        }),
        build_meeting_activity_filter(&meeting_id, cursor.as_ref(), scan_limit),
    ];
    let events = query_meeting(&state, &filters)
        .await
        .map_err(read_error_message)?;
    let create_event = events
        .iter()
        .find(|event| event.id.to_hex() == snapshot.create_event_id)
        .ok_or_else(|| integrity_error("Meeting activity could not verify its Create event"))?;
    let create = parse_create(create_event, &meeting_id)
        .map_err(read_error_message)?
        .ok_or_else(|| integrity_error("Meeting activity Create protocol is unsupported"))?;

    let mut states = Vec::new();
    for event in &events {
        let Some(projection) =
            parse_state(event, &identity, &create).map_err(read_error_message)?
        else {
            continue;
        };
        if cursor.as_ref().is_some_and(|cursor| {
            projection.created_at > cursor.before
                || (projection.created_at == cursor.before
                    && projection.event_id.as_str() <= cursor.before_id.as_str())
        }) {
            continue;
        }
        if projection.state.state_revision > snapshot.state_revision {
            return Err(integrity_error(
                "Meeting activity contains a State newer than the verified snapshot",
            ));
        }
        if validate_participants(&projection.state, &create).map_err(read_error_message)?
            != snapshot.participants
        {
            return Err(integrity_error(
                "Meeting activity changed the frozen participant roster",
            ));
        }
        validate_activity_transition(&projection)?;
        states.push(projection);
    }
    states.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| left.event_id.cmp(&right.event_id))
    });
    let mut revisions = BTreeMap::new();
    for projection in &states {
        if revisions
            .insert(
                projection.state.state_revision,
                projection.event_id.as_str(),
            )
            .is_some()
        {
            return Err(integrity_error(
                "Meeting activity contains conflicting State revisions",
            ));
        }
    }

    let roster = snapshot
        .participants
        .iter()
        .map(|participant| participant.pubkey.as_str())
        .collect::<BTreeSet<_>>();
    let actors = load_activity_actors(&state, &meeting_id, &states, &roster).await?;
    let mut activities = Vec::with_capacity(limit);
    let mut last_scanned = None;
    let mut stopped_for_limit = false;
    for (index, projection) in states.iter().enumerate() {
        last_scanned = Some(projection);
        if let Some(activity) = activity_from_state(&meeting_id, projection, &actors) {
            activities.push(activity);
            if activities.len() == limit {
                stopped_for_limit = index + 1 < states.len() || states.len() >= scan_limit;
                break;
            }
        }
    }
    let scan_exhausted = activities.len() < limit && states.len() >= scan_limit;
    let next_cursor = if stopped_for_limit || scan_exhausted {
        last_scanned.map(encode_cursor_for_state).transpose()?
    } else {
        None
    };

    Ok(MeetingActivityPage {
        activities,
        next_cursor,
    })
}

fn build_meeting_activity_filter(
    meeting_id: &str,
    cursor: Option<&ActivityCursor>,
    limit: usize,
) -> Value {
    let mut filter = json!({
        "kinds": [KIND_MEETING_STATE],
        "#h": [meeting_id],
        "limit": limit,
    });
    if let Some(cursor) = cursor {
        filter["until"] = json!(cursor.before);
        filter["before_id"] = json!(cursor.before_id);
    }
    filter
}

fn decode_activity_cursor(value: &str) -> Result<ActivityCursor, String> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| "Meeting activity cursor is invalid".to_string())?;
    let cursor: ActivityCursor = serde_json::from_slice(&bytes)
        .map_err(|_| "Meeting activity cursor is invalid".to_string())?;
    if cursor.version != ACTIVITY_CURSOR_VERSION {
        return Err("Meeting activity cursor version is unsupported".to_string());
    }
    require_hex64(&cursor.before_id, "Meeting activity cursor event ID")?;
    Ok(cursor)
}

fn encode_cursor_for_state(projection: &StateProjection) -> Result<String, String> {
    let bytes = serde_json::to_vec(&ActivityCursor {
        version: ACTIVITY_CURSOR_VERSION,
        before: projection.created_at,
        before_id: projection.event_id.clone(),
    })
    .map_err(|error| format!("encode Meeting activity cursor: {error}"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn validate_activity_transition(projection: &StateProjection) -> Result<(), String> {
    let Some(transition) = projection.state.transition.as_ref() else {
        return Ok(());
    };
    if !(0..=MAX_JAVASCRIPT_TIMESTAMP_MS).contains(&transition.at_ms)
        || transition.primary_type.trim().is_empty()
        || transition.primary_type.trim() != transition.primary_type
        || transition.primary_type.chars().any(char::is_control)
        || transition.effects.len() > MAX_TRANSITION_EFFECTS
    {
        return Err(integrity_error(
            "Meeting activity contains an invalid State transition",
        ));
    }
    if let Some(event_id) = transition.caused_by_event_id.as_deref() {
        require_hex64(event_id, "Meeting activity cause event ID")?;
    }
    Ok(())
}

async fn load_activity_actors(
    state: &AppState,
    meeting_id: &str,
    states: &[StateProjection],
    roster: &BTreeSet<&str>,
) -> Result<BTreeMap<String, String>, String> {
    let cause_ids = states
        .iter()
        .filter_map(|projection| projection.state.transition.as_ref())
        .filter_map(|transition| transition.caused_by_event_id.as_ref())
        .cloned()
        .collect::<BTreeSet<_>>();
    if cause_ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let filter = json!({
        "ids": cause_ids,
        "kinds": [
            KIND_STREAM_MESSAGE,
            KIND_MEETING_END,
            KIND_MEETING_FLOOR_CLAIM,
            KIND_MEETING_FLOOR_SIGNAL,
            KIND_MEETING_SPEECH_INTENT,
            KIND_MEETING_MODERATOR_COMMAND,
            KIND_MEETING_HUMAN_FLOOR_REQUEST,
            KIND_MEETING_OFFER_RESPONSE,
            KIND_MEETING_GRANT_SIGNAL,
            KIND_MEETING_BOARD_COMMAND,
            KIND_MEETING_ACTION_COMMAND,
        ],
        "#h": [meeting_id],
        "limit": states.len(),
    });
    let events = query_meeting(state, &[filter])
        .await
        .map_err(read_error_message)?;
    let mut actors = BTreeMap::new();
    for event in events {
        verify_event(&event, "Meeting activity cause").map_err(read_error_message)?;
        if single_tag(&event, "h") != Some(meeting_id) {
            return Err(integrity_error(
                "Meeting activity cause escaped its Meeting scope",
            ));
        }
        let actor = event.pubkey.to_hex();
        if !roster.contains(actor.as_str()) {
            return Err(integrity_error(
                "Meeting activity cause author is outside the frozen roster",
            ));
        }
        actors.insert(event.id.to_hex(), actor);
    }
    Ok(actors)
}

fn activity_from_state(
    meeting_id: &str,
    projection: &StateProjection,
    actors: &BTreeMap<String, String>,
) -> Option<MeetingActivity> {
    let transition = projection.state.transition.as_ref()?;
    let actor = transition
        .caused_by_event_id
        .as_ref()
        .and_then(|event_id| actors.get(event_id))
        .cloned();
    let has_effect = |effect_type: &str| {
        transition
            .effects
            .iter()
            .any(|effect| effect.effect_type == effect_type)
    };
    let handoff_target = transition
        .effects
        .iter()
        .find(|effect| {
            matches!(
                effect.effect_type.as_str(),
                "handoff_created" | "handoff_open_limit_blocked" | "handoff_attempted"
            )
        })
        .and_then(|effect| {
            projection
                .state
                .unresolved_handoffs
                .iter()
                .find(|handoff| handoff.handoff_id == effect.object_id)
        })
        .map(|handoff| handoff.to_pubkey.clone());
    let (kind, target_pubkey, summary) = match transition.primary_type.as_str() {
        "meeting_closed" => (
            MeetingActivityKind::MeetingClosed,
            None,
            "The host closed the meeting after confirming its outcome.",
        ),
        "meeting_aborted" => (
            MeetingActivityKind::MeetingAborted,
            None,
            "The meeting was ended without a successful conclusion.",
        ),
        "action_finalization_began" => (
            MeetingActivityKind::ActionFinalizationStarted,
            None,
            "The meeting entered action finalization.",
        ),
        "action_blocked" => (
            MeetingActivityKind::ActionBlocked,
            None,
            "Recording the meeting actions became blocked.",
        ),
        "action_retried" => (
            MeetingActivityKind::ActionRetried,
            None,
            "The host retried recording the meeting actions.",
        ),
        "action_returned_to_board" => (
            MeetingActivityKind::ActionReturnedToBoard,
            None,
            "Action finalization returned to Board maintenance.",
        ),
        "action_deadline_exceeded" => (
            MeetingActivityKind::ActionDeadlineExceeded,
            None,
            "The action-recording window reached its deadline.",
        ),
        "board_updated" => (
            MeetingActivityKind::BoardUpdated,
            None,
            "The host updated the Meeting Board.",
        ),
        "board_unchanged" => (
            MeetingActivityKind::BoardUnchanged,
            None,
            "The host completed Board maintenance without changes.",
        ),
        "board_timed_out" => (
            MeetingActivityKind::BoardTimedOut,
            None,
            "The Board maintenance window timed out.",
        ),
        "offer_created" if has_effect("handoff_attempted") => (
            MeetingActivityKind::HandoffAttempted,
            handoff_target,
            "The host offered the floor for a directed handoff.",
        ),
        "offer_created" => (
            MeetingActivityKind::FloorOffered,
            projection
                .state
                .offer
                .as_ref()
                .map(|offer| offer.target_pubkey.clone()),
            "The host offered the floor to a participant.",
        ),
        "offer_acked" => (
            MeetingActivityKind::FloorGranted,
            projection
                .state
                .grant
                .as_ref()
                .map(|grant| grant.holder_pubkey.clone()),
            "The participant accepted the offer and received the floor.",
        ),
        "offer_declined" => (
            MeetingActivityKind::OfferDeclined,
            actor.clone(),
            if has_effect("handoff_attempt_failed") {
                "The floor offer was declined; the directed handoff remains open."
            } else {
                "The participant declined the floor offer."
            },
        ),
        "offer_timed_out" => (
            MeetingActivityKind::OfferExpired,
            None,
            "The floor offer expired before it was accepted.",
        ),
        "grant_yielded" => (
            MeetingActivityKind::FloorYielded,
            actor.clone(),
            if has_effect("handoff_attempt_failed") {
                "The participant yielded the floor; the directed handoff remains open."
            } else {
                "The participant yielded the floor."
            },
        ),
        "grant_soft_expired" | "grant_hard_expired" => (
            MeetingActivityKind::FloorExpired,
            None,
            "The active floor grant expired.",
        ),
        "offer_recalled" | "recall_latched" => (
            MeetingActivityKind::FloorRecalled,
            None,
            "The host recalled meeting control.",
        ),
        "handoff_dismissed" => (
            MeetingActivityKind::HandoffResolved,
            None,
            "The host dismissed an open directed handoff.",
        ),
        "speech_accepted" if has_effect("handoff_answered") => (
            MeetingActivityKind::HandoffResolved,
            handoff_target,
            if has_effect("handoff_created") || has_effect("handoff_open_limit_blocked") {
                "A directed handoff was answered and a new handoff was recorded."
            } else {
                "A directed handoff was answered by the accepted Speech."
            },
        ),
        "speech_accepted"
            if has_effect("handoff_created") || has_effect("handoff_open_limit_blocked") =>
        {
            (
                MeetingActivityKind::HandoffOpened,
                handoff_target,
                "The accepted Speech recorded a new directed handoff.",
            )
        }
        "human_requested"
            if projection
                .state
                .board_control
                .as_ref()
                .and_then(|control| control.board_outcome.as_deref())
                == Some("preempted") =>
        {
            (
                MeetingActivityKind::BoardPreempted,
                actor.clone(),
                "A Human floor request preempted Board maintenance.",
            )
        }
        _ => return None,
    };
    let activity_id = hex::encode(Sha256::digest(
        format!("{meeting_id}|{}|{kind:?}", projection.event_id).as_bytes(),
    ));
    Some(MeetingActivity {
        activity_id,
        kind,
        occurred_at_ms: transition.at_ms,
        actor_pubkey: actor,
        target_pubkey,
        summary: summary.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_MEETING_ID: &str = "00000000-0000-4000-8000-000000000001";

    #[test]
    fn activity_cursor_is_opaque_and_round_trips() {
        let projection = StateProjection {
            event_id: "ab".repeat(32),
            created_at: 1_765_000_000,
            state: serde_json::from_value(json!({
                "phase": "moderator_idle",
                "state_revision": 1,
                "floor_revision": 1,
                "intent_revision": 0,
                "speech_revision": 0,
                "control_epoch": 1,
                "decision_epoch": 1,
                "consecutive_moderator_speeches": 0,
                "forced_return_to_moderator": false,
                "moderator_pubkey": "11".repeat(32),
                "participants": [],
                "board_control": null
            }))
            .unwrap_or_else(|error| panic!("test State: {error}")),
        };
        let encoded = encode_cursor_for_state(&projection)
            .unwrap_or_else(|error| panic!("encode cursor: {error}"));
        assert!(!encoded.contains(&projection.event_id));
        let decoded = decode_activity_cursor(&encoded)
            .unwrap_or_else(|error| panic!("decode cursor: {error}"));
        assert_eq!(decoded.before, projection.created_at);
        assert_eq!(decoded.before_id, projection.event_id);
    }

    #[test]
    fn activity_filter_uses_a_bounded_composite_cursor() {
        let cursor = ActivityCursor {
            version: ACTIVITY_CURSOR_VERSION,
            before: 1_765_000_000,
            before_id: "cd".repeat(32),
        };
        let filter = build_meeting_activity_filter(TEST_MEETING_ID, Some(&cursor), 240);
        assert_eq!(filter["kinds"], json!([KIND_MEETING_STATE]));
        assert_eq!(filter["#h"], json!([TEST_MEETING_ID]));
        assert_eq!(filter["until"], json!(cursor.before));
        assert_eq!(filter["before_id"], json!(cursor.before_id));
        assert_eq!(filter["limit"], json!(240));
    }

    #[test]
    fn activity_projection_exposes_product_fields_without_raw_control_data() {
        let cause_id = "ef".repeat(32);
        let projection = StateProjection {
            event_id: "ab".repeat(32),
            created_at: 1_765_000_000,
            state: serde_json::from_value(json!({
                "phase": "moderator_idle",
                "state_revision": 42,
                "floor_revision": 30,
                "intent_revision": 12,
                "speech_revision": 9,
                "control_epoch": 7,
                "decision_epoch": 6,
                "consecutive_moderator_speeches": 0,
                "forced_return_to_moderator": false,
                "moderator_pubkey": "11".repeat(32),
                "participants": [],
                "board_control": {
                    "phase": "floor_ready",
                    "control_epoch": 7,
                    "board_window": 4,
                    "board_started_at_ms": 1,
                    "board_deadline_at_ms": 2,
                    "board_completed_at_ms": 3,
                    "board_outcome": "updated",
                    "action": null
                },
                "transition": {
                    "primary_type": "board_updated",
                    "outcome": "accepted",
                    "primary_object_id": null,
                    "caused_by_event_id": cause_id,
                    "deadline_type": null,
                    "blocked_by": null,
                    "at_ms": 1_765_000_000_123_i64,
                    "effects": [{
                        "type": "board_updated",
                        "object_type": "board_window",
                        "object_id": "4",
                        "from": "board_pending",
                        "to": "floor_ready"
                    }]
                }
            }))
            .unwrap_or_else(|error| panic!("test State: {error}")),
        };
        let activity = activity_from_state(
            TEST_MEETING_ID,
            &projection,
            &BTreeMap::from([(cause_id.clone(), "22".repeat(32))]),
        )
        .unwrap_or_else(|| panic!("Board update activity"));
        assert_eq!(activity.kind, MeetingActivityKind::BoardUpdated);
        let serialized = serde_json::to_string(&activity)
            .unwrap_or_else(|error| panic!("serialize activity: {error}"));
        for secret in [
            cause_id.as_str(),
            projection.event_id.as_str(),
            "stateRevision",
            "floorRevision",
            "controlEpoch",
            "boardWindow",
            "controlToken",
            "lease",
        ] {
            assert!(!serialized.contains(secret), "leaked {secret}");
        }
    }
}
