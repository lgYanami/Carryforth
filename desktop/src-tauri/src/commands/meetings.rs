//! Verified, read-only Meeting V2 bridge for the Desktop client.
//!
//! Raw Relay events stop at this boundary. React receives a semantic snapshot
//! assembled from a signed Create event and Relay-authored State/Board
//! projections, so live WebSocket payloads can remain invalidation signals.

mod create;
pub use create::create_meeting;
mod activity;
pub use activity::get_meeting_activities;
mod actions;
pub use actions::submit_meeting_action_finalization;
mod floor;
pub use floor::submit_meeting_floor_action;
mod host;
pub use host::submit_meeting_host_action;
mod directory;
use directory::list_item_from_load;
mod model;
use model::*;
mod pending;

use std::collections::{BTreeMap, BTreeSet};

use buzz_core_pkg::kind::{
    KIND_MEETING_BOARD, KIND_MEETING_CREATE, KIND_MEETING_END, KIND_MEETING_STATE,
    KIND_STREAM_MESSAGE,
};
use nostr::{Event, PublicKey};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use tauri::State;
use uuid::Uuid;

use crate::app_state::AppState;
use crate::relay::{
    classify_request_error, parse_json_response, query_relay, query_relay_at_with_keys,
    relay_api_base_url_with_override, relay_error_message,
};

const MEETING_V2_EXTENSION: &str = "buzz-meeting-v2";
const MEETING_V2_CREATE_EXTENSION: &str = "buzz-meeting-v2-create";
const MEETING_V2_DIRECT_ACTIONS_EXTENSION: &str = "buzz-meeting-v2-direct-actions";
const MEETING_V2_DIRECT_ACTIONS_CREATE_EXTENSION: &str = "buzz-meeting-v2-direct-actions-create";
const SNAPSHOT_STATE_LIMIT: usize = 200;
const SNAPSHOT_EVENT_LIMIT: usize = 20;
const MAX_LIST_MEETINGS: usize = 64;
const DEFAULT_SPEECH_PAGE_SIZE: usize = 50;
const MAX_SPEECH_PAGE_SIZE: usize = 100;

#[derive(Debug, Deserialize)]
struct Nip11Document {
    #[serde(default)]
    supported_extensions: Vec<String>,
    #[serde(rename = "self")]
    relay_self: Option<String>,
}

#[derive(Debug, Clone)]
struct MeetingIdentity {
    relay_pubkey: PublicKey,
    capability: MeetingCapability,
}

/// Meeting protocol support advertised by the active Community Relay.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingCapability {
    status: MeetingCapabilityStatus,
    relay_pubkey: Option<String>,
    supports_direct_actions: bool,
    can_create_direct_actions: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum MeetingCapabilityStatus {
    Unsupported,
    Readable,
    Creatable,
}

#[derive(Debug)]
enum MeetingReadError {
    Forbidden,
    Other(String),
}

/// Read the active Relay's Meeting V2 capability.
#[tauri::command]
pub async fn get_meeting_capability(
    state: State<'_, AppState>,
) -> Result<MeetingCapability, String> {
    Ok(read_meeting_identity(&state).await?.map_or(
        MeetingCapability {
            status: MeetingCapabilityStatus::Unsupported,
            relay_pubkey: None,
            supports_direct_actions: false,
            can_create_direct_actions: false,
        },
        |identity| identity.capability,
    ))
}

/// Load one verified Meeting V2 snapshot.
#[tauri::command]
pub async fn get_meeting_snapshot(
    meeting_id: String,
    state: State<'_, AppState>,
) -> Result<MeetingLoadResult, String> {
    let meeting_id = canonical_meeting_id(&meeting_id)?;
    let Some(identity) = read_meeting_identity(&state).await? else {
        return Ok(MeetingLoadResult::UnsupportedRelay);
    };
    match load_meeting_snapshot(&state, &identity, &meeting_id).await {
        Ok(result) => Ok(result),
        Err(MeetingReadError::Forbidden) => Ok(MeetingLoadResult::Forbidden),
        Err(error) => Err(read_error_message(error)),
    }
}

/// Read the current Board through the same verified snapshot boundary.
#[tauri::command]
pub async fn get_meeting_board(
    meeting_id: String,
    state: State<'_, AppState>,
) -> Result<MeetingLoadResult, String> {
    get_meeting_snapshot(meeting_id, state).await
}

/// Build a bounded Meeting directory for rooms already discovered through
/// membership-scoped kind:39000 metadata.
#[tauri::command]
pub async fn list_meetings(
    meeting_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<Vec<MeetingListItem>, String> {
    if meeting_ids.len() > MAX_LIST_MEETINGS {
        return Err(format!(
            "Meeting list accepts at most {MAX_LIST_MEETINGS} room IDs"
        ));
    }
    let mut ids = BTreeSet::new();
    for id in meeting_ids {
        ids.insert(canonical_meeting_id(&id)?);
    }
    let viewer_keys = state.signing_keys()?;
    let viewer_pubkey = viewer_keys.public_key().to_hex();
    let api_base_url = relay_api_base_url_with_override(&state);
    let identity = read_meeting_identity(&state).await?;
    let mut items = Vec::with_capacity(ids.len());
    for meeting_id in ids {
        let loaded = if let Some(identity) = &identity {
            load_meeting_snapshot_at(&state, identity, &meeting_id, &api_base_url, &viewer_keys)
                .await
        } else {
            Ok(MeetingLoadResult::UnsupportedRelay)
        };
        items.push(list_item_from_load(meeting_id, loaded, &viewer_pubkey));
    }
    Ok(items)
}

/// Load one page of verified canonical Meeting Speech events.
#[tauri::command]
pub async fn get_meeting_speeches(
    meeting_id: String,
    before: Option<u64>,
    before_id: Option<String>,
    limit: Option<usize>,
    state: State<'_, AppState>,
) -> Result<MeetingSpeechPage, String> {
    let meeting_id = canonical_meeting_id(&meeting_id)?;
    let limit = limit
        .unwrap_or(DEFAULT_SPEECH_PAGE_SIZE)
        .clamp(1, MAX_SPEECH_PAGE_SIZE);
    if before.is_some() != before_id.is_some() {
        return Err("Meeting Speech cursor requires both before and beforeId".to_string());
    }
    if let Some(id) = &before_id {
        require_hex64(id, "Meeting Speech cursor event ID")?;
    }
    let Some(identity) = read_meeting_identity(&state).await? else {
        return Err("Meeting V2 is not supported by this Community".to_string());
    };
    let loaded = load_meeting_snapshot(&state, &identity, &meeting_id)
        .await
        .map_err(read_error_message)?;
    let MeetingLoadResult::Ready { snapshot } = loaded else {
        return Err("Meeting Speech is unavailable for this Meeting".to_string());
    };

    let fetch_limit = (limit.saturating_mul(4)).clamp(limit + 1, 500);
    let filter =
        build_meeting_speech_filter(&meeting_id, before, before_id.as_deref(), fetch_limit);
    let events = query_meeting(&state, &[filter])
        .await
        .map_err(read_error_message)?;
    let roster = snapshot
        .participants
        .iter()
        .map(|participant| (participant.pubkey.clone(), participant.participant_type))
        .collect::<BTreeMap<_, _>>();
    let mut by_revision = BTreeMap::new();
    for event in events {
        let Some(speech) = parse_speech(
            &event,
            &meeting_id,
            &roster,
            &snapshot.moderator_pubkey,
            snapshot.speech_revision,
        )?
        else {
            continue;
        };
        if let (Some(before), Some(before_id)) = (before, before_id.as_deref()) {
            if speech.created_at > before
                || (speech.created_at == before && speech.event_id.as_str() <= before_id)
            {
                continue;
            }
        }
        if by_revision.insert(speech.speech_revision, speech).is_some() {
            return Err("Meeting integrity error: duplicate canonical Speech revision".to_string());
        }
    }
    let mut speeches = by_revision.into_values().collect::<Vec<_>>();
    speeches.sort_by(|left, right| {
        right
            .speech_revision
            .cmp(&left.speech_revision)
            .then_with(|| right.event_id.cmp(&left.event_id))
    });
    let has_more = speeches.len() > limit;
    speeches.truncate(limit);
    let next_cursor = if has_more {
        speeches.last().map(|speech| MeetingSpeechCursor {
            before: speech.created_at,
            before_id: speech.event_id.clone(),
        })
    } else {
        None
    };
    speeches.reverse();
    Ok(MeetingSpeechPage {
        speeches,
        next_cursor,
    })
}

fn build_meeting_speech_filter(
    meeting_id: &str,
    before: Option<u64>,
    before_id: Option<&str>,
    limit: usize,
) -> Value {
    let mut filter = json!({
        "kinds": [KIND_STREAM_MESSAGE],
        "#h": [meeting_id],
        "limit": limit,
    });
    if let Some(before) = before {
        filter["until"] = json!(before);
    }
    if let Some(before_id) = before_id {
        // Buzz's authenticated /query bridge uses this composite tiebreak to
        // advance within a dense second (created_at DESC, id ASC).
        filter["before_id"] = json!(before_id);
    }
    filter
}

async fn read_meeting_identity(state: &AppState) -> Result<Option<MeetingIdentity>, String> {
    let api_base_url = relay_api_base_url_with_override(state);
    read_meeting_identity_at(state, &api_base_url).await
}

async fn read_meeting_identity_at(
    state: &AppState,
    api_base_url: &str,
) -> Result<Option<MeetingIdentity>, String> {
    crate::relay_admission::wait_for_rate_limit().await;
    let url = format!("{}/info", api_base_url.trim_end_matches('/'));
    let response = state
        .http_client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/nostr+json")
        .send()
        .await
        .map_err(|error| classify_request_error(&error))?;
    if !response.status().is_success() {
        return Err(relay_error_message(response).await);
    }
    let info: Nip11Document = parse_json_response(response).await?;
    if !has_extension(&info, MEETING_V2_EXTENSION) {
        return Ok(None);
    }
    let relay_self = info.relay_self.as_deref().ok_or_else(|| {
        integrity_error("NIP-11 advertises Meeting V2 without a Relay `self` key")
    })?;
    require_hex64(relay_self, "NIP-11 Relay self")?;
    let relay_pubkey = PublicKey::from_hex(relay_self)
        .map_err(|error| integrity_error(format!("invalid NIP-11 Relay self: {error}")))?;
    if relay_pubkey.to_hex() != relay_self {
        return Err(integrity_error(
            "NIP-11 Relay self is not canonical lowercase hex",
        ));
    }
    let can_create = has_extension(&info, MEETING_V2_CREATE_EXTENSION);
    let supports_direct_actions = has_extension(&info, MEETING_V2_DIRECT_ACTIONS_EXTENSION);
    let can_create_direct_actions =
        has_extension(&info, MEETING_V2_DIRECT_ACTIONS_CREATE_EXTENSION);
    Ok(Some(MeetingIdentity {
        relay_pubkey,
        capability: MeetingCapability {
            status: if can_create {
                MeetingCapabilityStatus::Creatable
            } else {
                MeetingCapabilityStatus::Readable
            },
            relay_pubkey: Some(relay_self.to_string()),
            supports_direct_actions,
            can_create_direct_actions,
        },
    }))
}

fn has_extension(info: &Nip11Document, extension: &str) -> bool {
    info.supported_extensions
        .iter()
        .any(|value| value == extension)
}

async fn load_meeting_snapshot(
    state: &AppState,
    identity: &MeetingIdentity,
    meeting_id: &str,
) -> Result<MeetingLoadResult, MeetingReadError> {
    let api_base_url = relay_api_base_url_with_override(state);
    let keys = state.signing_keys().map_err(MeetingReadError::Other)?;
    load_meeting_snapshot_at(state, identity, meeting_id, &api_base_url, &keys).await
}

async fn load_meeting_snapshot_at(
    state: &AppState,
    identity: &MeetingIdentity,
    meeting_id: &str,
    api_base_url: &str,
    keys: &nostr::Keys,
) -> Result<MeetingLoadResult, MeetingReadError> {
    let filters = [
        json!({
            "kinds": [KIND_MEETING_CREATE],
            "#h": [meeting_id],
            "limit": SNAPSHOT_EVENT_LIMIT,
        }),
        json!({
            "kinds": [KIND_MEETING_STATE],
            "#h": [meeting_id],
            "limit": SNAPSHOT_STATE_LIMIT,
        }),
        json!({
            "kinds": [KIND_MEETING_BOARD],
            "#h": [meeting_id],
            "limit": SNAPSHOT_EVENT_LIMIT,
        }),
        json!({
            "kinds": [KIND_MEETING_END],
            "#h": [meeting_id],
            "limit": SNAPSHOT_EVENT_LIMIT,
        }),
        json!({
            "kinds": [KIND_STREAM_MESSAGE],
            "#h": [meeting_id],
            "limit": SNAPSHOT_EVENT_LIMIT,
        }),
    ];
    let events = query_meeting_at(state, api_base_url, keys, &filters).await?;
    let create_events = events
        .iter()
        .filter(|event| event.kind.as_u16() as u32 == KIND_MEETING_CREATE)
        .collect::<Vec<_>>();
    if create_events.is_empty() {
        return Ok(MeetingLoadResult::NotFound);
    }
    let protocol_hint = create_events
        .iter()
        .find(|event| single_tag(event, "h") == Some(meeting_id));
    let schema_version = protocol_hint.and_then(|event| single_tag(event, "v"));
    let policy = protocol_hint.and_then(|event| single_tag(event, "policy"));
    if schema_version != Some(buzz_sdk_pkg::MEETING_V2_SCHEMA_VERSION)
        || !matches!(
            policy,
            Some(buzz_sdk_pkg::MEETING_V2_POLICY | buzz_sdk_pkg::MEETING_V2_ACTIONS_POLICY)
        )
    {
        return Ok(MeetingLoadResult::UnsupportedProtocol {
            meeting_id: meeting_id.to_string(),
            schema_version: schema_version.map(str::to_string),
            policy: policy.map(str::to_string),
        });
    }
    if policy == Some(buzz_sdk_pkg::MEETING_V2_ACTIONS_POLICY)
        && !identity.capability.supports_direct_actions
    {
        return Ok(MeetingLoadResult::UnsupportedProtocol {
            meeting_id: meeting_id.to_string(),
            schema_version: schema_version.map(str::to_string),
            policy: policy.map(str::to_string),
        });
    }
    let mut creates = create_events
        .into_iter()
        .filter_map(|event| parse_create(event, meeting_id).transpose())
        .collect::<Result<Vec<_>, _>>()?;
    creates.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.event_id.cmp(&right.event_id))
    });
    if creates.len() != 1 {
        return Err(MeetingReadError::Other(integrity_error(
            "Meeting has no unique valid Create event",
        )));
    }
    let create = creates.remove(0);

    let states = events
        .iter()
        .filter_map(|event| parse_state(event, identity, &create).transpose())
        .collect::<Result<Vec<_>, _>>()?;
    let current_state = select_current_state(states, &create)?;
    let board = parse_current_board(&events, identity, &create)?;
    let end = parse_current_end(&events, &create)?;

    let (participants, phase, revisions, current_speaker, current_offer, floor, host, action) =
        if let Some(state) = &current_state {
            let participants = validate_participants(&state.state, &create)?;
            let current_speaker = state
                .state
                .grant
                .as_ref()
                .map(|grant| grant.holder_pubkey.clone());
            let current_offer = state
                .state
                .offer
                .as_ref()
                .map(|offer| offer.target_pubkey.clone());
            let floor = floor_from_projection(state);
            let host = host_from_projection(state, meeting_id);
            let action = state
                .state
                .board_control
                .as_ref()
                .and_then(|control| control.action.as_ref())
                .map(action_from_wire);
            (
                participants,
                state.state.phase.clone(),
                (
                    state.state.state_revision,
                    state.state.floor_revision,
                    state.state.intent_revision,
                    state.state.speech_revision,
                ),
                current_speaker,
                current_offer,
                Some(floor),
                host,
                action,
            )
        } else {
            let participants = create
                .participant_pubkeys
                .iter()
                .map(|pubkey| MeetingParticipant {
                    pubkey: pubkey.clone(),
                    participant_type: MeetingParticipantType::Unknown,
                    channel_role: if pubkey == &create.host_pubkey {
                        "owner".to_string()
                    } else {
                        "member".to_string()
                    },
                })
                .collect();
            (
                participants,
                "initializing".to_string(),
                (0, 0, 0, 0),
                None,
                None,
                None,
                None,
                None,
            )
        };

    let lifecycle = if let Some(end) = &end {
        if end.outcome == "closed" {
            MeetingLifecycle::Closed
        } else {
            MeetingLifecycle::Aborted
        }
    } else if current_state
        .as_ref()
        .and_then(|state| state.state.board_control.as_ref())
        .is_some_and(|control| control.phase == "finalizing_actions")
    {
        MeetingLifecycle::FinalizingActions
    } else if current_state.is_some() {
        MeetingLifecycle::Active
    } else {
        MeetingLifecycle::Initializing
    };
    if matches!(lifecycle, MeetingLifecycle::FinalizingActions)
        && action
            .as_ref()
            .is_some_and(|action| action.board_event_id != board.event_id)
    {
        return Err(MeetingReadError::Other(integrity_error(
            "Meeting action run does not reference the current frozen Board",
        )));
    }
    let roster = participants
        .iter()
        .map(|participant| (participant.pubkey.clone(), participant.participant_type))
        .collect::<BTreeMap<_, _>>();
    let mut latest_speech_at: Option<u64> = None;
    for event in &events {
        if let Some(speech) =
            parse_speech(event, meeting_id, &roster, &create.host_pubkey, revisions.3)
                .map_err(MeetingReadError::Other)?
        {
            latest_speech_at = Some(
                latest_speech_at
                    .map_or(speech.created_at, |current| current.max(speech.created_at)),
            );
        }
    }

    let authoritative_updated_at = current_state
        .as_ref()
        .map(|state| state.created_at)
        .into_iter()
        .chain([create.created_at, board.updated_at])
        .chain(end.as_ref().map(|end| end.ended_at))
        .chain(latest_speech_at)
        .max()
        .unwrap_or(create.created_at);

    Ok(MeetingLoadResult::Ready {
        snapshot: Box::new(MeetingSnapshot {
            meeting_id: create.meeting_id,
            title: create.title,
            description: create.description,
            source_channel_id: create.source_channel_id,
            schema_version: 3,
            policy: create.policy,
            host_pubkey: create.host_pubkey.clone(),
            moderator_pubkey: create.host_pubkey,
            create_event_id: create.event_id,
            created_at: create.created_at,
            lifecycle,
            phase,
            state_revision: revisions.0,
            floor_revision: revisions.1,
            intent_revision: revisions.2,
            speech_revision: revisions.3,
            current_speaker_pubkey: current_speaker,
            current_offer_pubkey: current_offer,
            floor,
            host,
            participants,
            board,
            action,
            end,
            latest_speech_at,
            authoritative_updated_at,
        }),
    })
}

#[path = "meetings/projection.rs"]
mod projection;

use projection::{
    action_from_wire, parse_create, parse_current_board, parse_current_end, parse_speech,
    parse_state, select_current_state, validate_participants,
};

fn handoff_from_wire(context: &HandoffContextWire) -> MeetingHandoffContext {
    MeetingHandoffContext {
        from_pubkey: context.from_pubkey.clone(),
        reason_type: context.reason_type.clone(),
        reason_text: context.reason_text.clone(),
    }
}

fn floor_from_projection(projection: &StateProjection) -> MeetingFloorState {
    MeetingFloorState {
        state_event_id: projection.event_id.clone(),
        human_queue: projection
            .state
            .human_queue
            .iter()
            .map(|request| MeetingHumanFloorRequest {
                request_id: request.request_id.clone(),
                requester_pubkey: request.requester_pubkey.clone(),
                queue_position: request.queue_position as u64,
                state: request.state.clone(),
            })
            .collect(),
        offer: projection.state.offer.as_ref().map(|offer| MeetingOffer {
            offer_id: offer.offer_id.clone(),
            target_pubkey: offer.target_pubkey.clone(),
            target_participant_type: offer.target_participant_type.into(),
            allocation_source: offer.allocation_source.clone(),
            turn_role: offer.turn_role.clone(),
            selection_reason: offer.selection_reason.clone(),
            source_intent_id: offer.source_intent_id.clone(),
            source_request_id: offer.source_request_id.clone(),
            source_handoff_id: offer.source_handoff_id.clone(),
            source_speech_event_id: offer.source_speech_event_id.clone(),
            handoff_context: offer.handoff_context.as_ref().map(handoff_from_wire),
            created_at_ms: offer.created_at_ms,
            ack_deadline_ms: offer.ack_deadline_ms,
        }),
        grant: projection.state.grant.as_ref().map(|grant| MeetingGrant {
            grant_id: grant.grant_id.clone(),
            holder_pubkey: grant.holder_pubkey.clone(),
            allocation_source: grant.allocation_source.clone(),
            turn_role: grant.turn_role.clone(),
            selection_reason: grant.selection_reason.clone(),
            source_intent_id: grant.source_intent_id.clone(),
            source_request_id: grant.source_request_id.clone(),
            source_handoff_id: grant.source_handoff_id.clone(),
            source_speech_event_id: grant.source_speech_event_id.clone(),
            handoff_context: grant.handoff_context.as_ref().map(handoff_from_wire),
            created_at_ms: grant.created_at_ms,
            soft_lease_expires_at_ms: grant.soft_lease_expires_at_ms,
            hard_deadline_ms: grant.hard_deadline_ms,
            progress_seq: grant.progress_seq as u64,
        }),
    }
}

fn host_from_projection(
    projection: &StateProjection,
    meeting_id: &str,
) -> Option<MeetingHostState> {
    let state = &projection.state;
    let control = state.board_control.as_ref()?;
    let moderator_has_control =
        matches!(state.phase.as_str(), "moderator_idle" | "moderator_control")
            && state.offer.is_none()
            && state.grant.is_none()
            && state.human_queue.is_empty();
    let floor_ready = control.phase == "floor_ready";
    let can_select = moderator_has_control && floor_ready;
    let can_close = can_select
        && matches!(
            control.board_outcome.as_deref(),
            Some("updated" | "unchanged")
        );
    let active_handoff_id = state
        .offer
        .as_ref()
        .and_then(|offer| offer.source_handoff_id.as_deref())
        .or_else(|| {
            state
                .grant
                .as_ref()
                .and_then(|grant| grant.source_handoff_id.as_deref())
        });
    let control_binding = match (control.phase.as_str(), control.action.as_ref()) {
        ("board_pending", _) => format!(
            "board|{meeting_id}|{}|{}",
            control.control_epoch, control.board_window
        ),
        ("finalizing_actions", Some(action)) => format!(
            "action|{meeting_id}|{}|{}|{}|{}",
            action.action_run_id,
            action.action_window_epoch,
            action.board_event_id,
            projection.event_id
        ),
        _ => format!("state|{meeting_id}|{}", projection.event_id),
    };
    let control_token = hex::encode(Sha256::digest(control_binding.as_bytes()));
    Some(MeetingHostState {
        control_token,
        state_event_id: projection.event_id.clone(),
        control_epoch: state.control_epoch,
        decision_epoch: state.decision_epoch,
        decision_deadline_ms: state.moderator_decision_deadline_ms,
        next_action_at_ms: state.next_action_at_ms,
        consecutive_moderator_speeches: state.consecutive_moderator_speeches as u32,
        forced_return_to_moderator: state.forced_return_to_moderator,
        pending_intents: state
            .pending_intents
            .iter()
            .map(|intent| MeetingPendingIntent {
                intent_id: intent.intent_id.clone(),
                current_event_id: intent.current_event_id.clone(),
                author_pubkey: intent.author_pubkey.clone(),
                basis_speech_revision: intent.basis_speech_revision,
                summary: intent.summary.clone(),
                addressed_to: intent.addressed_to.clone(),
                created_at_ms: intent.created_at_ms,
                deferred: intent.deferred,
                selection_attempt_count: intent.selection_attempt_count as u32,
                last_offer_id: intent.last_offer_id.clone(),
                last_attempt_outcome: intent.last_attempt_outcome.clone(),
                eligible_decision_epoch: intent.eligible_decision_epoch,
                selectable: can_select
                    && !intent.deferred
                    && intent.eligible_decision_epoch <= state.decision_epoch,
            })
            .collect(),
        open_handoffs: state
            .unresolved_handoffs
            .iter()
            .map(|handoff| {
                let attempt_active = active_handoff_id == Some(handoff.handoff_id.as_str());
                MeetingOpenHandoff {
                    handoff_id: handoff.handoff_id.clone(),
                    source_speech_event_id: handoff.source_speech_event_id.clone(),
                    from_pubkey: handoff.from_pubkey.clone(),
                    to_pubkey: handoff.to_pubkey.clone(),
                    reason_type: handoff.reason_type.clone(),
                    reason_text: handoff.reason_text.clone(),
                    created_at_ms: handoff.created_at_ms,
                    attempt_count: handoff.attempt_count as u32,
                    last_offer_id: handoff.last_offer_id.clone(),
                    last_grant_id: handoff.last_grant_id.clone(),
                    last_attempt_outcome: handoff.last_attempt_outcome.clone(),
                    blocked_by: handoff.blocked_by.clone(),
                    moderator_retry_blocked: handoff.moderator_retry_blocked,
                    eligible_decision_epoch: handoff.eligible_decision_epoch,
                    attempt_active,
                    selectable: can_select
                        && !attempt_active
                        && handoff.blocked_by.is_none()
                        && !handoff.moderator_retry_blocked
                        && handoff.eligible_decision_epoch <= state.decision_epoch,
                }
            })
            .collect(),
        board_control: MeetingBoardControl {
            phase: control.phase.clone(),
            control_epoch: control.control_epoch,
            board_window: control.board_window,
            board_started_at_ms: control.board_started_at_ms,
            board_deadline_at_ms: control.board_deadline_at_ms,
            board_completed_at_ms: control.board_completed_at_ms,
            board_outcome: control.board_outcome.clone(),
        },
        can_select,
        can_close,
        can_recall: matches!(state.phase.as_str(), "offered" | "granted")
            && !state.forced_return_to_moderator
            && state.human_queue.is_empty(),
    })
}

async fn query_meeting(
    state: &AppState,
    filters: &[Value],
) -> Result<Vec<Event>, MeetingReadError> {
    query_relay(state, filters).await.map_err(|message| {
        let normalized = message.to_ascii_lowercase();
        if normalized.contains("forbidden")
            || normalized.contains("restricted")
            || normalized.contains("403")
        {
            MeetingReadError::Forbidden
        } else {
            MeetingReadError::Other(message)
        }
    })
}

async fn query_meeting_at(
    state: &AppState,
    api_base_url: &str,
    keys: &nostr::Keys,
    filters: &[Value],
) -> Result<Vec<Event>, MeetingReadError> {
    query_relay_at_with_keys(state, api_base_url, filters, keys, None)
        .await
        .map_err(map_meeting_query_error)
}

fn map_meeting_query_error(message: String) -> MeetingReadError {
    let normalized = message.to_ascii_lowercase();
    if normalized.contains("forbidden")
        || normalized.contains("restricted")
        || normalized.contains("403")
    {
        MeetingReadError::Forbidden
    } else {
        MeetingReadError::Other(message)
    }
}

fn canonical_meeting_id(value: &str) -> Result<String, String> {
    let parsed =
        Uuid::parse_str(value).map_err(|_| "Meeting ID must be a canonical UUID".to_string())?;
    if parsed.is_nil() || parsed.to_string() != value {
        return Err("Meeting ID must be a canonical non-nil UUID".to_string());
    }
    Ok(value.to_string())
}

fn verify_event(event: &Event, context: &str) -> Result<(), MeetingReadError> {
    event.verify().map_err(|error| {
        MeetingReadError::Other(integrity_error(format!(
            "invalid {context} signature: {error}"
        )))
    })
}

fn verify_relay_event(
    event: &Event,
    identity: &MeetingIdentity,
    context: &str,
) -> Result<(), MeetingReadError> {
    verify_event(event, context)?;
    if event.pubkey != identity.relay_pubkey {
        return Err(MeetingReadError::Other(integrity_error(format!(
            "{context} is not signed by the active Community Relay"
        ))));
    }
    Ok(())
}

fn required_tag<'a>(
    event: &'a Event,
    name: &str,
    context: &str,
) -> Result<&'a str, MeetingReadError> {
    single_tag(event, name).ok_or_else(|| {
        MeetingReadError::Other(integrity_error(format!(
            "{context} has no unique {name} tag"
        )))
    })
}

fn required_tag_string(event: &Event, name: &str, context: &str) -> Result<String, String> {
    single_tag(event, name)
        .map(str::to_string)
        .ok_or_else(|| integrity_error(format!("{context} has no unique {name} tag")))
}

fn optional_tag<'a>(
    event: &'a Event,
    name: &str,
    context: &str,
) -> Result<Option<&'a str>, MeetingReadError> {
    let mut tags = tags_named(event, name);
    let Some(tag) = tags.next() else {
        return Ok(None);
    };
    if tags.next().is_some() {
        return Err(MeetingReadError::Other(integrity_error(format!(
            "{context} has duplicate {name} tags"
        ))));
    }
    let value = tag.get(1).ok_or_else(|| {
        MeetingReadError::Other(integrity_error(format!(
            "{context} has an invalid {name} tag"
        )))
    })?;
    Ok((!value.is_empty()).then_some(value.as_str()))
}

fn single_tag<'a>(event: &'a Event, name: &str) -> Option<&'a str> {
    let mut values = tags_named(event, name).filter_map(|tag| tag.get(1).map(String::as_str));
    let first = values.next()?;
    values.next().is_none().then_some(first)
}

fn tags_named<'a>(event: &'a Event, name: &str) -> impl Iterator<Item = &'a [String]> + 'a {
    let name = name.to_string();
    event.tags.iter().filter_map(move |tag| {
        let values = tag.as_slice();
        (values.first() == Some(&name)).then_some(values)
    })
}

fn parse_revision_tag(event: &Event, name: &str, context: &str) -> Result<u64, MeetingReadError> {
    required_tag(event, name, context)?
        .parse::<u64>()
        .map_err(|_| {
            MeetingReadError::Other(integrity_error(format!(
                "{context} has an invalid {name} tag"
            )))
        })
}

fn require_hex64(value: &str, context: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(integrity_error(format!(
            "{context} must be canonical lowercase hex"
        )));
    }
    Ok(())
}

fn integrity_error(message: impl AsRef<str>) -> String {
    format!("Meeting integrity error: {}", message.as_ref())
}

fn read_error_message(error: MeetingReadError) -> String {
    match error {
        MeetingReadError::Forbidden => {
            "restricted: Meeting requires current roster membership".to_string()
        }
        MeetingReadError::Other(message) => message,
    }
}

#[cfg(test)]
#[path = "meetings/tests.rs"]
mod tests;
