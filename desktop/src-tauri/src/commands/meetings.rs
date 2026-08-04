//! Verified, read-only Meeting V2 bridge for the Desktop client.
//!
//! Raw Relay events stop at this boundary. React receives a semantic snapshot
//! assembled from a signed Create event and Relay-authored State/Board
//! projections, so live WebSocket payloads can remain invalidation signals.

use std::collections::{BTreeMap, BTreeSet};

use buzz_core_pkg::kind::{
    KIND_MEETING_BOARD, KIND_MEETING_CREATE, KIND_MEETING_END, KIND_MEETING_STATE,
    KIND_STREAM_MESSAGE,
};
use nostr::{Event, PublicKey};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::State;
use uuid::Uuid;

use crate::app_state::AppState;
use crate::relay::{
    classify_request_error, parse_json_response, query_relay, relay_api_base_url_with_override,
    relay_error_message,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingParticipant {
    pubkey: String,
    participant_type: MeetingParticipantType,
    channel_role: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum MeetingParticipantType {
    Human,
    Agent,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum FrozenParticipantType {
    Human,
    Agent,
}

impl From<FrozenParticipantType> for MeetingParticipantType {
    fn from(value: FrozenParticipantType) -> Self {
        match value {
            FrozenParticipantType::Human => Self::Human,
            FrozenParticipantType::Agent => Self::Agent,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingBoard {
    event_id: String,
    format: String,
    body: String,
    moderator_pubkey: String,
    updated_at: u64,
    source: MeetingBoardSource,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum MeetingBoardSource {
    Projection,
    Create,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingActionState {
    action_run_id: String,
    board_event_id: String,
    action_window_epoch: u64,
    condition: String,
    terminal_status: Option<String>,
    completion_event_id: Option<String>,
    action_deadline_at_ms: Option<i64>,
    last_error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingEndState {
    event_id: String,
    outcome: String,
    reason_code: Option<String>,
    reason: Option<String>,
    ended_by: String,
    ended_at: u64,
    actions_attested: bool,
}

/// Complete read-only Meeting view consumed by React.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingSnapshot {
    meeting_id: String,
    title: String,
    description: Option<String>,
    source_channel_id: Option<String>,
    schema_version: u16,
    policy: String,
    host_pubkey: String,
    moderator_pubkey: String,
    create_event_id: String,
    created_at: u64,
    lifecycle: MeetingLifecycle,
    phase: String,
    state_revision: u64,
    floor_revision: u64,
    intent_revision: u64,
    speech_revision: u64,
    current_speaker_pubkey: Option<String>,
    current_offer_pubkey: Option<String>,
    participants: Vec<MeetingParticipant>,
    board: MeetingBoard,
    action: Option<MeetingActionState>,
    end: Option<MeetingEndState>,
    latest_speech_at: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MeetingLifecycle {
    Initializing,
    Active,
    FinalizingActions,
    Closed,
    Aborted,
}

/// Safe load states for Meeting routes. Unsupported protocols remain isolated
/// from the normal Channel surface.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MeetingLoadResult {
    UnsupportedRelay,
    Forbidden,
    NotFound,
    UnsupportedProtocol {
        meeting_id: String,
        schema_version: Option<String>,
        policy: Option<String>,
    },
    Ready {
        snapshot: Box<MeetingSnapshot>,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingListItem {
    meeting_id: String,
    title: String,
    lifecycle: Option<MeetingLifecycle>,
    phase: Option<String>,
    current_speaker_pubkey: Option<String>,
    moderator_pubkey: Option<String>,
    policy: Option<String>,
    updated_at: Option<u64>,
    ended_at: Option<u64>,
    latest_speech_at: Option<u64>,
    compatibility: MeetingListCompatibility,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum MeetingListCompatibility {
    Ready,
    UnsupportedRelay,
    UnsupportedProtocol,
    Forbidden,
    NotFound,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingSpeech {
    event_id: String,
    author_pubkey: String,
    content: String,
    created_at: u64,
    speech_revision: u64,
    grant_event_id: String,
    mentions: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingSpeechCursor {
    before: u64,
    before_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingSpeechPage {
    speeches: Vec<MeetingSpeech>,
    next_cursor: Option<MeetingSpeechCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct StateParticipant {
    pubkey: String,
    participant_type: FrozenParticipantType,
    channel_role: String,
}

#[derive(Debug, Deserialize)]
struct StateTarget {
    #[serde(default)]
    target_pubkey: Option<String>,
    #[serde(default)]
    holder_pubkey: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ActionWire {
    action_run_id: Uuid,
    board_event_id: String,
    action_window_epoch: u64,
    condition: String,
    #[serde(default)]
    terminal_status: Option<String>,
    #[serde(default)]
    completion_event_id: Option<String>,
    #[serde(default)]
    action_deadline_at_ms: Option<i64>,
    #[serde(default)]
    last_error_code: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BoardControlWire {
    phase: String,
    #[serde(default)]
    action: Option<ActionWire>,
}

#[derive(Debug, Deserialize)]
struct StateWire {
    phase: String,
    state_revision: u64,
    floor_revision: u64,
    intent_revision: u64,
    speech_revision: u64,
    moderator_pubkey: String,
    participants: Vec<StateParticipant>,
    #[serde(default)]
    offer: Option<StateTarget>,
    #[serde(default)]
    grant: Option<StateTarget>,
    #[serde(default)]
    board_control: Option<BoardControlWire>,
}

#[derive(Debug)]
struct CreateProjection {
    meeting_id: String,
    title: String,
    description: Option<String>,
    source_channel_id: Option<String>,
    policy: String,
    host_pubkey: String,
    participant_pubkeys: BTreeSet<String>,
    event_id: String,
    created_at: u64,
    initial_board: buzz_sdk_pkg::MeetingV2BoardContent,
}

#[derive(Debug)]
struct StateProjection {
    event_id: String,
    state: StateWire,
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
    let identity = read_meeting_identity(&state).await?;
    let mut items = Vec::with_capacity(ids.len());
    for meeting_id in ids {
        let loaded = if let Some(identity) = &identity {
            load_meeting_snapshot(&state, identity, &meeting_id).await
        } else {
            Ok(MeetingLoadResult::UnsupportedRelay)
        };
        items.push(list_item_from_load(meeting_id, loaded));
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
        .map(|participant| participant.pubkey.clone())
        .collect::<BTreeSet<_>>();
    let mut by_revision = BTreeMap::new();
    for event in events {
        let Some(speech) = parse_speech(&event, &meeting_id, &roster, snapshot.speech_revision)?
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
    crate::relay_admission::wait_for_rate_limit().await;
    let url = format!("{}/info", relay_api_base_url_with_override(state));
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
    let events = query_meeting(state, &filters).await?;
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

    let (participants, phase, revisions, current_speaker, current_offer, action) =
        if let Some(state) = &current_state {
            let participants = validate_participants(&state.state, &create)?;
            let current_speaker = state
                .state
                .grant
                .as_ref()
                .and_then(|grant| grant.holder_pubkey.clone());
            let current_offer = state
                .state
                .offer
                .as_ref()
                .and_then(|offer| offer.target_pubkey.clone());
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
        .map(|participant| participant.pubkey.clone())
        .collect::<BTreeSet<_>>();
    let mut latest_speech_at: Option<u64> = None;
    for event in &events {
        if let Some(speech) = parse_speech(event, meeting_id, &roster, revisions.3)
            .map_err(MeetingReadError::Other)?
        {
            latest_speech_at = Some(
                latest_speech_at
                    .map_or(speech.created_at, |current| current.max(speech.created_at)),
            );
        }
    }

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
            participants,
            board,
            action,
            end,
            latest_speech_at,
        }),
    })
}

#[path = "meetings/projection.rs"]
mod projection;

use projection::{
    action_from_wire, parse_create, parse_current_board, parse_current_end, parse_speech,
    parse_state, select_current_state, validate_participants,
};

fn list_item_from_load(
    meeting_id: String,
    loaded: Result<MeetingLoadResult, MeetingReadError>,
) -> MeetingListItem {
    match loaded {
        Ok(MeetingLoadResult::Ready { snapshot }) => MeetingListItem {
            meeting_id,
            title: snapshot.title.clone(),
            lifecycle: Some(snapshot.lifecycle),
            phase: Some(snapshot.phase.clone()),
            current_speaker_pubkey: snapshot.current_speaker_pubkey.clone(),
            moderator_pubkey: Some(snapshot.moderator_pubkey.clone()),
            policy: Some(snapshot.policy.clone()),
            updated_at: Some(
                snapshot
                    .end
                    .as_ref()
                    .map_or(snapshot.board.updated_at, |end| end.ended_at),
            ),
            ended_at: snapshot.end.as_ref().map(|end| end.ended_at),
            latest_speech_at: snapshot.latest_speech_at,
            compatibility: MeetingListCompatibility::Ready,
        },
        Ok(MeetingLoadResult::UnsupportedRelay) => {
            empty_list_item(meeting_id, MeetingListCompatibility::UnsupportedRelay)
        }
        Ok(MeetingLoadResult::UnsupportedProtocol { .. }) => {
            empty_list_item(meeting_id, MeetingListCompatibility::UnsupportedProtocol)
        }
        Ok(MeetingLoadResult::Forbidden) | Err(MeetingReadError::Forbidden) => {
            empty_list_item(meeting_id, MeetingListCompatibility::Forbidden)
        }
        Ok(MeetingLoadResult::NotFound) => {
            empty_list_item(meeting_id, MeetingListCompatibility::NotFound)
        }
        Err(MeetingReadError::Other(_)) => {
            empty_list_item(meeting_id, MeetingListCompatibility::UnsupportedProtocol)
        }
    }
}

fn empty_list_item(meeting_id: String, compatibility: MeetingListCompatibility) -> MeetingListItem {
    MeetingListItem {
        title: meeting_id.clone(),
        meeting_id,
        lifecycle: None,
        phase: None,
        current_speaker_pubkey: None,
        moderator_pubkey: None,
        policy: None,
        updated_at: None,
        ended_at: None,
        latest_speech_at: None,
        compatibility,
    }
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
