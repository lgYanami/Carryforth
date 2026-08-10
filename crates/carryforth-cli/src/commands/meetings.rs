use std::collections::{BTreeMap, BTreeSet, HashSet};

use nostr::{Event, PublicKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::client::{
    extract_p_tags, extract_tag_value, normalize_write_response, print_create_response,
    CarryforthClient,
};
use crate::error::CliError;
use crate::validate::{parse_uuid, read_file_or_stdin, read_or_stdin, sdk_err, validate_hex64};
use crate::OutputFormat;

const KIND_GROUP_METADATA: u32 = 39000;
const KIND_GROUP_MEMBERS: u32 = 39002;
const KIND_MEETING_SPEECH: u32 = 9;
const KIND_MEETING_FLOOR_CLAIM: u32 = 42102;
const KIND_MEETING_ROUND_STATE: u32 = 42103;
const KIND_MEETING_FLOOR_SIGNAL: u32 = 42104;
const KIND_MEETING_BOARD: u32 = buzz_sdk::kind::KIND_MEETING_BOARD;
const MEETING_METADATA_CHUNK_SIZE: usize = 200;
const MEETING_SUMMARY_EXTENSION: &str = "buzz-meeting-summary-v1";

#[derive(Debug, Serialize)]
pub(crate) struct MeetingSummary {
    pub(crate) meeting_id: String,
    pub(crate) title: String,
    pub(crate) description: Option<String>,
    pub(crate) summary: Option<String>,
    pub(crate) room_kind: String,
    pub(crate) status: &'static str,
    pub(crate) updated_at: u64,
}

#[derive(Debug, Deserialize)]
struct MeetingRelayInfo {
    #[serde(default)]
    supported_extensions: Vec<String>,
    #[serde(rename = "self")]
    relay_self: Option<String>,
}

#[derive(Debug, Serialize)]
struct MeetingBoardOutput {
    meeting_id: String,
    format: String,
    body: String,
    moderator: String,
    event_id: String,
    created_at: u64,
}

struct CreateMeetingInput<'a> {
    title: &'a str,
    description: Option<&'a str>,
    source: Option<&'a str>,
    policy: crate::MeetingPolicy,
    moderator: Option<&'a str>,
    board: Option<&'a str>,
    participant_pubkeys: &'a [String],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeetingProtocol {
    UniformV0,
    ModeratedBatonV1,
    ModeratedBoardV2,
    ModeratedBoardActionsV2Legacy,
    ModeratedBoardActionsV2,
}

impl MeetingProtocol {
    fn from_create_event(event: &serde_json::Value) -> Result<Self, CliError> {
        let version = extract_tag_value(event, "v");
        let policy = extract_tag_value(event, "policy");
        match (version.as_str(), policy.as_str()) {
            ("1", "" | "uniform-v0") => Ok(Self::UniformV0),
            ("2", buzz_sdk::MEETING_V1_POLICY) => Ok(Self::ModeratedBatonV1),
            ("3", buzz_sdk::MEETING_V2_POLICY) => Ok(Self::ModeratedBoardV2),
            ("3", buzz_sdk::MEETING_V2_ACTIONS_V2_POLICY) => {
                Ok(Self::ModeratedBoardActionsV2Legacy)
            }
            ("3", buzz_sdk::MEETING_V2_ACTIONS_POLICY) => Ok(Self::ModeratedBoardActionsV2),
            ("2", "") => Err(CliError::Other(
                "invalid Meeting V1 Create: missing policy tag".into(),
            )),
            _ => Err(CliError::Other(format!(
                "unsupported meeting protocol: v={version}, policy={policy}"
            ))),
        }
    }

    fn policy(self) -> &'static str {
        match self {
            Self::UniformV0 => "uniform-v0",
            Self::ModeratedBatonV1 => buzz_sdk::MEETING_V1_POLICY,
            Self::ModeratedBoardV2 => buzz_sdk::MEETING_V2_POLICY,
            Self::ModeratedBoardActionsV2Legacy => buzz_sdk::MEETING_V2_ACTIONS_V2_POLICY,
            Self::ModeratedBoardActionsV2 => buzz_sdk::MEETING_V2_ACTIONS_POLICY,
        }
    }

    const fn is_v2(self) -> bool {
        matches!(
            self,
            Self::ModeratedBoardV2
                | Self::ModeratedBoardActionsV2Legacy
                | Self::ModeratedBoardActionsV2
        )
    }

    const fn has_action_finalization(self) -> bool {
        matches!(
            self,
            Self::ModeratedBoardActionsV2Legacy | Self::ModeratedBoardActionsV2
        )
    }
}

#[derive(Debug, Clone, Serialize)]
struct FloorState {
    meeting_id: String,
    round_number: u64,
    floor_revision: u64,
    phase: String,
    policy_version: String,
    state_event_id: String,
    holder_pubkey: Option<String>,
    grant_event_id: Option<String>,
    settle_not_before_ms: Option<i64>,
    claim_deadline_ms: Option<i64>,
    lease_expires_at_ms: Option<i64>,
    outcome: Option<String>,
    speech_event_id: Option<String>,
    previous_round_number: Option<u64>,
    previous_outcome: Option<String>,
    previous_speech_event_id: Option<String>,
    claim_event_ids: Vec<String>,
    claimant_pubkeys: Vec<String>,
    decision_cohort_pubkeys: Vec<String>,
    ready_pubkeys: Vec<String>,
    passer_pubkeys: Vec<String>,
    created_at: u64,
}

#[derive(Debug, Clone, Serialize)]
struct BatonState {
    meeting_id: String,
    state_revision: u64,
    floor_revision: u64,
    intent_revision: u64,
    speech_revision: u64,
    phase: String,
    policy_version: String,
    moderator_pubkey: String,
    state_event_id: String,
    content: serde_json::Value,
    created_at: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct MeetingActionRunView {
    action_run_id: Uuid,
    board_event_id: String,
    action_window_epoch: u64,
    condition: String,
}

fn parse_baton_state(event: &serde_json::Value) -> Option<BatonState> {
    if event.get("kind").and_then(serde_json::Value::as_u64)
        != Some(buzz_sdk::kind::KIND_MEETING_STATE as u64)
    {
        return None;
    }
    let version = extract_tag_value(event, "v");
    let policy = extract_tag_value(event, "policy");
    if !matches!(
        (version.as_str(), policy.as_str()),
        (
            buzz_sdk::MEETING_V1_SCHEMA_VERSION,
            buzz_sdk::MEETING_V1_POLICY
        ) | (
            buzz_sdk::MEETING_V2_SCHEMA_VERSION,
            buzz_sdk::MEETING_V2_POLICY
        ) | (
            buzz_sdk::MEETING_V2_SCHEMA_VERSION,
            buzz_sdk::MEETING_V2_ACTIONS_POLICY
        )
    ) {
        return None;
    }
    let meeting_id = extract_tag_value(event, "h");
    let state_revision = extract_tag_value(event, "state-revision").parse().ok()?;
    let floor_revision = extract_tag_value(event, "floor-revision").parse().ok()?;
    let intent_revision = extract_tag_value(event, "intent-revision").parse().ok()?;
    let speech_revision = extract_tag_value(event, "speech-revision").parse().ok()?;
    let phase = extract_tag_value(event, "phase");
    let moderator_pubkey = extract_tag_value(event, "moderator");
    let state_event_id = event.get("id")?.as_str()?.to_string();
    if meeting_id.is_empty() || phase.is_empty() || moderator_pubkey.is_empty() {
        return None;
    }
    let content = event
        .get("content")
        .and_then(serde_json::Value::as_str)
        .and_then(|content| serde_json::from_str(content).ok())?;
    Some(BatonState {
        meeting_id,
        state_revision,
        floor_revision,
        intent_revision,
        speech_revision,
        phase,
        policy_version: policy,
        moderator_pubkey,
        state_event_id,
        content,
        created_at: event
            .get("created_at")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
    })
}

fn parse_floor_state(event: &serde_json::Value) -> Option<FloorState> {
    if event.get("kind").and_then(serde_json::Value::as_u64)
        != Some(KIND_MEETING_ROUND_STATE as u64)
    {
        return None;
    }
    let meeting_id = extract_tag_value(event, "h");
    let round_number = extract_tag_value(event, "meeting-round").parse().ok()?;
    let floor_revision = extract_tag_value(event, "floor-revision").parse().ok()?;
    let phase = extract_tag_value(event, "phase");
    let policy_version = extract_tag_value(event, "policy");
    let state_event_id = event.get("id")?.as_str()?.to_string();
    if meeting_id.is_empty() || phase.is_empty() || policy_version.is_empty() {
        return None;
    }
    let content = event
        .get("content")
        .and_then(serde_json::Value::as_str)
        .and_then(|content| serde_json::from_str::<serde_json::Value>(content).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let string_array = |field: &str| {
        content
            .get(field)
            .and_then(serde_json::Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    Some(FloorState {
        meeting_id,
        round_number,
        floor_revision,
        phase: phase.clone(),
        policy_version,
        state_event_id: state_event_id.clone(),
        holder_pubkey: {
            let holder = extract_tag_value(event, "holder");
            (!holder.is_empty()).then_some(holder)
        },
        grant_event_id: (phase == "granted").then_some(state_event_id),
        settle_not_before_ms: content
            .get("settle_not_before_ms")
            .and_then(serde_json::Value::as_i64),
        claim_deadline_ms: content
            .get("claim_deadline_ms")
            .and_then(serde_json::Value::as_i64),
        lease_expires_at_ms: content
            .get("lease_expires_at_ms")
            .and_then(serde_json::Value::as_i64),
        outcome: content
            .get("outcome")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        speech_event_id: content
            .get("speech_event_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        previous_round_number: content
            .get("previous_round")
            .and_then(serde_json::Value::as_u64),
        previous_outcome: content
            .get("previous_outcome")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        previous_speech_event_id: content
            .get("previous_speech_event_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        claim_event_ids: string_array("claim_event_ids"),
        claimant_pubkeys: string_array("claimants"),
        decision_cohort_pubkeys: string_array("decision_cohort"),
        ready_pubkeys: string_array("ready"),
        passer_pubkeys: string_array("passed"),
        created_at: event
            .get("created_at")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
    })
}

async fn fetch_floor_states(
    client: &CarryforthClient,
    meeting_id: &str,
    limit: u32,
) -> Result<Vec<FloorState>, CliError> {
    let filter = serde_json::json!({
        "kinds": [KIND_MEETING_ROUND_STATE],
        "#h": [meeting_id],
    });
    let events = client.query_paginated(filter, limit).await?;
    let mut states: Vec<FloorState> = events.iter().filter_map(parse_floor_state).collect();
    states.sort_by(|left, right| {
        left.floor_revision
            .cmp(&right.floor_revision)
            .then_with(|| left.state_event_id.cmp(&right.state_event_id))
    });
    Ok(states)
}

async fn fetch_current_floor(
    client: &CarryforthClient,
    meeting_id: &str,
) -> Result<Option<FloorState>, CliError> {
    Ok(fetch_floor_states(client, meeting_id, 1000)
        .await?
        .into_iter()
        .max_by(|left, right| {
            left.floor_revision
                .cmp(&right.floor_revision)
                .then_with(|| left.state_event_id.cmp(&right.state_event_id))
        }))
}

async fn fetch_baton_states(
    client: &CarryforthClient,
    meeting_id: &str,
    limit: u32,
) -> Result<Vec<BatonState>, CliError> {
    let filter = serde_json::json!({
        "kinds": [buzz_sdk::kind::KIND_MEETING_STATE],
        "#h": [meeting_id],
    });
    let events = client.query_paginated(filter, limit).await?;
    let mut states: Vec<BatonState> = events.iter().filter_map(parse_baton_state).collect();
    states.sort_by(|left, right| {
        left.state_revision
            .cmp(&right.state_revision)
            .then_with(|| left.state_event_id.cmp(&right.state_event_id))
    });
    Ok(states)
}

async fn fetch_current_baton(
    client: &CarryforthClient,
    meeting_id: &str,
) -> Result<Option<BatonState>, CliError> {
    let filter = serde_json::json!({
        "kinds": [buzz_sdk::kind::KIND_MEETING_STATE],
        "#h": [meeting_id],
    });
    Ok(client
        .query_all(filter)
        .await?
        .into_iter()
        .filter_map(|event| parse_baton_state(&event))
        .max_by(|left, right| {
            left.state_revision
                .cmp(&right.state_revision)
                .then_with(|| left.state_event_id.cmp(&right.state_event_id))
        }))
}

async fn fetch_required_v1_baton(
    client: &CarryforthClient,
    meeting_id: Uuid,
) -> Result<BatonState, CliError> {
    let meeting_id_text = meeting_id.to_string();
    let protocol = fetch_meeting_protocol(client, &meeting_id_text).await?;
    if !matches!(
        protocol,
        MeetingProtocol::ModeratedBatonV1
            | MeetingProtocol::ModeratedBoardV2
            | MeetingProtocol::ModeratedBoardActionsV2
    ) {
        return Err(CliError::Usage(format!(
            "meeting {meeting_id} does not use a moderated Meeting protocol"
        )));
    }
    fetch_current_baton(client, &meeting_id_text)
        .await?
        .ok_or_else(|| CliError::NotFound(format!("Meeting State not found: {meeting_id}")))
}

fn baton_is_v2(state: &BatonState) -> bool {
    matches!(
        state.policy_version.as_str(),
        buzz_sdk::MEETING_V2_POLICY | buzz_sdk::MEETING_V2_ACTIONS_POLICY
    )
}

macro_rules! build_moderated {
    ($state:expr, $v1:path, $v2:path, $params:expr $(,)?) => {
        if baton_is_v2($state) {
            $v2($params)
        } else {
            $v1($params)
        }
    };
}

fn baton_u64(state: &BatonState, field: &str) -> Result<u64, CliError> {
    state
        .content
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            CliError::Other(format!(
                "moderated Meeting State {} has no valid {field}",
                state.state_event_id
            ))
        })
}

fn baton_array<'a>(
    state: &'a BatonState,
    field: &str,
) -> Result<&'a [serde_json::Value], CliError> {
    state
        .content
        .get(field)
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| {
            CliError::Other(format!(
                "moderated Meeting State {} has no valid {field}",
                state.state_event_id
            ))
        })
}

fn baton_object<'a>(
    state: &'a BatonState,
    array_field: &str,
    id_field: &str,
    id: &str,
) -> Result<&'a serde_json::Value, CliError> {
    baton_array(state, array_field)?
        .iter()
        .find(|value| value.get(id_field).and_then(serde_json::Value::as_str) == Some(id))
        .ok_or_else(|| {
            CliError::Conflict(format!(
                "{id_field} {id} is not canonical in State {}",
                state.state_event_id
            ))
        })
}

fn baton_active_object<'a>(
    state: &'a BatonState,
    field: &str,
) -> Result<&'a serde_json::Value, CliError> {
    state
        .content
        .get(field)
        .filter(|value| value.is_object())
        .ok_or_else(|| {
            CliError::Conflict(format!(
                "there is no active {field} in State {}",
                state.state_event_id
            ))
        })
}

fn object_string<'a>(
    object: &'a serde_json::Value,
    field: &str,
    context: &str,
) -> Result<&'a str, CliError> {
    object
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| CliError::Other(format!("{context} has no valid {field}")))
}

async fn submit_v1_event(
    client: &CarryforthClient,
    meeting_id: Uuid,
    event: nostr::Event,
) -> Result<String, CliError> {
    match client.submit_event(event).await {
        Ok(response) => Ok(response),
        Err(CliError::Relay { status, body })
            if status == 409 || (status == 400 && body.contains("conflict:")) =>
        {
            let canonical = fetch_current_baton(client, &meeting_id.to_string()).await?;
            let state = canonical
                .and_then(|state| serde_json::to_string(&state).ok())
                .unwrap_or_else(|| "null".to_string());
            Err(CliError::Conflict(format!(
                "{body}; canonical_state={state}"
            )))
        }
        Err(error) => Err(error),
    }
}

fn v1_write_response(response: &str, id_key: &str, id: &str) -> serde_json::Value {
    let normalized = normalize_write_response(response);
    let mut output = serde_json::from_str::<serde_json::Value>(&normalized).unwrap_or_else(|_| {
        serde_json::json!({
            "accepted": false,
            "message": normalized,
        })
    });
    output[id_key] = serde_json::json!(id);
    if let Some(payload) = output
        .get("message")
        .and_then(serde_json::Value::as_str)
        .and_then(|message| message.strip_prefix("response:"))
        .and_then(|payload| serde_json::from_str::<serde_json::Value>(payload).ok())
        .and_then(|payload| payload.as_object().cloned())
    {
        for (key, value) in payload {
            output[&key] = value;
        }
    }
    output
}

fn print_v1_write_response(response: &str, id_key: &str, id: &str) {
    println!("{}", v1_write_response(response, id_key, id));
}

async fn fetch_meeting_create(
    client: &CarryforthClient,
    meeting_id: &str,
) -> Result<serde_json::Value, CliError> {
    let filter = serde_json::json!({
        "kinds": [buzz_sdk::kind::KIND_MEETING_CREATE],
        "#h": [meeting_id],
        "limit": 10,
    });
    let response = client.query(&filter).await?;
    let events: Vec<serde_json::Value> = serde_json::from_str(&response).unwrap_or_default();
    events
        .into_iter()
        .find(|event| {
            event.get("kind").and_then(serde_json::Value::as_u64)
                == Some(buzz_sdk::kind::KIND_MEETING_CREATE as u64)
                && extract_tag_value(event, "h") == meeting_id
        })
        .ok_or_else(|| CliError::NotFound(format!("meeting not found: {meeting_id}")))
}

async fn fetch_meeting_protocol(
    client: &CarryforthClient,
    meeting_id: &str,
) -> Result<MeetingProtocol, CliError> {
    let create = fetch_meeting_create(client, meeting_id).await?;
    MeetingProtocol::from_create_event(&create)
}

async fn require_uniform_v0(client: &CarryforthClient, meeting_id: Uuid) -> Result<(), CliError> {
    match fetch_meeting_protocol(client, &meeting_id.to_string()).await? {
        MeetingProtocol::UniformV0 => Ok(()),
        MeetingProtocol::ModeratedBatonV1 => Err(CliError::Usage(format!(
            "meeting {meeting_id} uses {}; use the Meeting V1 command surface",
            buzz_sdk::MEETING_V1_POLICY
        ))),
        protocol @ (MeetingProtocol::ModeratedBoardV2
        | MeetingProtocol::ModeratedBoardActionsV2Legacy
        | MeetingProtocol::ModeratedBoardActionsV2) => Err(CliError::Usage(format!(
            "meeting {meeting_id} uses {}; use the moderated Meeting command surface",
            protocol.policy()
        ))),
    }
}

async fn meeting_relay_identity(
    client: &CarryforthClient,
    require_summary_write: bool,
) -> Result<PublicKey, CliError> {
    let raw = client.get_public("/info").await?;
    let info: MeetingRelayInfo = serde_json::from_str(&raw)
        .map_err(|error| CliError::Other(format!("invalid NIP-11 document: {error}")))?;
    if require_summary_write
        && !info
            .supported_extensions
            .iter()
            .any(|extension| extension == MEETING_SUMMARY_EXTENSION)
    {
        return Err(CliError::Other(format!(
            "unavailable: Relay does not advertise {MEETING_SUMMARY_EXTENSION}"
        )));
    }
    let relay_self = info
        .relay_self
        .ok_or_else(|| CliError::Other("NIP-11 has no canonical Relay `self` key".into()))?;
    let relay_pubkey = PublicKey::from_hex(&relay_self)
        .map_err(|error| CliError::Other(format!("invalid NIP-11 Relay `self`: {error}")))?;
    if relay_pubkey.to_hex() != relay_self {
        return Err(CliError::Other(
            "NIP-11 Relay `self` is not canonical lowercase hex".into(),
        ));
    }
    Ok(relay_pubkey)
}

fn strict_metadata_tag(event: &Event, name: &str) -> Result<Option<String>, CliError> {
    let values = event
        .tags
        .iter()
        .filter_map(|tag| {
            let parts = tag.as_slice();
            (parts.first().map(String::as_str) == Some(name)).then_some(parts)
        })
        .collect::<Vec<_>>();
    match values.as_slice() {
        [] => Ok(None),
        [parts] if parts.len() == 2 && !parts[1].is_empty() => Ok(Some(parts[1].clone())),
        _ => Err(CliError::Other(format!(
            "malformed Relay Meeting metadata {name} tag"
        ))),
    }
}

fn verified_meeting_summary(
    value: serde_json::Value,
    relay_pubkey: &PublicKey,
) -> Result<Option<MeetingSummary>, CliError> {
    let event: Event = serde_json::from_value(value)
        .map_err(|error| CliError::Other(format!("invalid Meeting metadata event: {error}")))?;
    event
        .verify()
        .map_err(|error| CliError::Other(format!("invalid Meeting metadata signature: {error}")))?;
    if event.pubkey != *relay_pubkey || event.kind.as_u16() as u32 != KIND_GROUP_METADATA {
        return Err(CliError::Other(
            "Meeting metadata was not signed by the advertised Relay".into(),
        ));
    }
    if strict_metadata_tag(&event, "room_kind")?.as_deref() != Some("meeting") {
        return Ok(None);
    }
    let meeting_id = strict_metadata_tag(&event, "d")?
        .ok_or_else(|| CliError::Other("Meeting metadata has no identity".into()))?;
    Uuid::parse_str(&meeting_id)
        .map_err(|_| CliError::Other("Meeting metadata identity is not a UUID".into()))?;
    let title = strict_metadata_tag(&event, "name")?
        .ok_or_else(|| CliError::Other("Meeting metadata has no title".into()))?;
    let description = strict_metadata_tag(&event, "about")?;
    let summary = strict_metadata_tag(&event, "summary")?;
    let archived = strict_metadata_tag(&event, "archived")?.as_deref() == Some("true");
    Ok(Some(MeetingSummary {
        meeting_id,
        title,
        description,
        summary,
        room_kind: "meeting".to_string(),
        status: if archived { "ended" } else { "active" },
        updated_at: event.created_at.as_secs(),
    }))
}

async fn fetch_meeting_metadata(
    client: &CarryforthClient,
    meeting_id: &str,
) -> Result<Option<MeetingSummary>, CliError> {
    let relay_pubkey = meeting_relay_identity(client, false).await?;
    let filter = serde_json::json!({
        "kinds": [KIND_GROUP_METADATA],
        "authors": [relay_pubkey.to_hex()],
        "#d": [meeting_id],
        "limit": 1,
    });
    for value in client.query_all(filter).await? {
        let Some(metadata) = verified_meeting_summary(value, &relay_pubkey)? else {
            continue;
        };
        if metadata.meeting_id != meeting_id {
            return Err(CliError::Other(
                "Meeting metadata response contained an unrequested identity".into(),
            ));
        }
        return Ok(Some(metadata));
    }
    Ok(None)
}

/// Read bounded, metadata-only Meeting summaries for Project Context output.
///
/// Missing or malformed individual metadata heads remain absent so callers can
/// retain the verified Edge and mark only that coordinate detail unavailable.
pub(crate) async fn fetch_meeting_context_summaries(
    client: &CarryforthClient,
    requested: &BTreeSet<Uuid>,
) -> Result<BTreeMap<Uuid, MeetingSummary>, CliError> {
    let relay_pubkey = meeting_relay_identity(client, false).await?;
    let mut summaries = BTreeMap::new();
    let requested_ids = requested.iter().copied().collect::<Vec<_>>();
    for chunk in requested_ids.chunks(MEETING_METADATA_CHUNK_SIZE) {
        let ids = chunk.iter().map(Uuid::to_string).collect::<Vec<_>>();
        let filter = serde_json::json!({
            "kinds": [KIND_GROUP_METADATA],
            "authors": [relay_pubkey.to_hex()],
            "#d": ids,
            "limit": chunk.len(),
        });
        for event in client.query_all(filter).await? {
            let Some(summary) = verified_meeting_summary(event, &relay_pubkey)? else {
                continue;
            };
            let Ok(meeting_id) = Uuid::parse_str(&summary.meeting_id) else {
                continue;
            };
            if !requested.contains(&meeting_id) {
                return Err(CliError::Other(
                    "Meeting metadata response contained an unrequested identity".to_owned(),
                ));
            }
            let replace = summaries
                .get(&meeting_id)
                .is_none_or(|current: &MeetingSummary| current.updated_at < summary.updated_at);
            if replace {
                summaries.insert(meeting_id, summary);
            }
        }
    }
    Ok(summaries)
}

async fn cmd_create_meeting(
    client: &CarryforthClient,
    input: CreateMeetingInput<'_>,
) -> Result<(), CliError> {
    let CreateMeetingInput {
        title,
        description,
        source,
        policy,
        moderator,
        board,
        participant_pubkeys,
    } = input;
    if participant_pubkeys.is_empty() || participant_pubkeys.len() > 11 {
        return Err(CliError::Usage(
            "--participant must provide 1-11 other participant pubkeys".into(),
        ));
    }

    let self_pubkey = client.keys().public_key().to_hex();
    let mut seen = HashSet::with_capacity(participant_pubkeys.len());
    for pubkey in participant_pubkeys {
        validate_hex64(pubkey)?;
        let normalized = pubkey.to_ascii_lowercase();
        if normalized == self_pubkey {
            return Err(CliError::Usage(
                "--participant must not repeat the current identity".into(),
            ));
        }
        if !seen.insert(normalized) {
            return Err(CliError::Usage(format!(
                "duplicate --participant pubkey: {pubkey}"
            )));
        }
    }

    let source_channel_id = source.map(parse_uuid).transpose()?;
    let meeting_id = Uuid::new_v4();
    let participant_refs: Vec<&str> = participant_pubkeys.iter().map(String::as_str).collect();
    let builder = match policy {
        crate::MeetingPolicy::UniformV0 => {
            if moderator.is_some() {
                return Err(CliError::Usage(
                    "--moderator requires --policy moderated-baton-v1".into(),
                ));
            }
            if board.is_some() {
                return Err(CliError::Usage(
                    "--board requires a moderated-board policy".into(),
                ));
            }
            buzz_sdk::build_meeting_create(
                meeting_id,
                title,
                description,
                source_channel_id,
                &participant_refs,
            )
        }
        crate::MeetingPolicy::ModeratedBatonV1 => {
            if board.is_some() {
                return Err(CliError::Usage(
                    "--board requires a moderated-board policy".into(),
                ));
            }
            let moderator_pubkey = moderator.unwrap_or(&self_pubkey);
            validate_hex64(moderator_pubkey)?;
            let normalized_moderator = moderator_pubkey.to_ascii_lowercase();
            if normalized_moderator != self_pubkey && !seen.contains(&normalized_moderator) {
                return Err(CliError::Usage(
                    "--moderator must be the current identity or one of the --participant values"
                        .into(),
                ));
            }
            buzz_sdk::build_meeting_v1_create(buzz_sdk::MeetingV1CreateParams {
                session_id: meeting_id,
                title,
                description,
                source_channel_id,
                author_pubkey: &self_pubkey,
                moderator_pubkey,
                participant_pubkeys: &participant_refs,
            })
        }
        v2_policy @ (crate::MeetingPolicy::ModeratedBoardV2
        | crate::MeetingPolicy::ModeratedBoardActionsV3) => {
            if moderator.is_some() {
                return Err(CliError::Usage(
                    "Meeting V2 fixes the creator as moderator; do not pass --moderator".into(),
                ));
            }
            let board = board.ok_or_else(|| {
                CliError::Usage("--board is required with a moderated-board policy".into())
            })?;
            let board = read_or_stdin(board)?;
            let params = buzz_sdk::MeetingV2CreateParams {
                session_id: meeting_id,
                title,
                description,
                source_channel_id,
                author_pubkey: &self_pubkey,
                participant_pubkeys: &participant_refs,
                initial_board: &board,
            };
            if v2_policy == crate::MeetingPolicy::ModeratedBoardActionsV3 {
                buzz_sdk::build_meeting_v2_actions_create(params)
            } else {
                buzz_sdk::build_meeting_v2_create(params)
            }
        }
    }
    .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let response = client.submit_event(event).await?;
    print_create_response(&response, "meeting_id", &meeting_id.to_string());
    Ok(())
}

pub async fn cmd_list_meetings(
    client: &CarryforthClient,
    include_ended: bool,
    limit: u32,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let relay_pubkey = meeting_relay_identity(client, false).await?;
    let filter = serde_json::json!({
        "kinds": [KIND_GROUP_METADATA],
        "authors": [relay_pubkey.to_hex()],
    });
    let events = client.query_paginated(filter, limit).await?;
    let mut meetings = Vec::new();
    for event in events {
        if let Some(meeting) = verified_meeting_summary(event, &relay_pubkey)? {
            if include_ended || meeting.status == "active" {
                meetings.push(meeting);
            }
        }
    }

    match format {
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string(&meetings).unwrap_or_else(|_| "[]".to_string())
        ),
        OutputFormat::Compact => {
            let compact: Vec<serde_json::Value> = meetings
                .iter()
                .map(|meeting| {
                    serde_json::json!({
                        "meeting_id": meeting.meeting_id,
                        "title": meeting.title,
                        "summary": meeting.summary,
                        "status": meeting.status,
                    })
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string(&compact).unwrap_or_else(|_| "[]".to_string())
            );
        }
    }
    Ok(())
}

pub async fn cmd_show_meeting(client: &CarryforthClient, meeting_id: &str) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?.to_string();
    let relay_pubkey = meeting_relay_identity(client, false).await?;
    let filters = [
        serde_json::json!({
            "kinds": [KIND_GROUP_METADATA],
            "authors": [relay_pubkey.to_hex()],
            "#d": [&meeting_id],
            "limit": 1,
        }),
        serde_json::json!({
            "kinds": [
                buzz_sdk::kind::KIND_MEETING_CREATE,
                buzz_sdk::kind::KIND_MEETING_END
            ],
            "#h": [&meeting_id],
            "limit": 20,
        }),
        serde_json::json!({
            "kinds": [KIND_MEETING_ROUND_STATE],
            "#h": [&meeting_id],
            "limit": 1000,
        }),
    ];
    let response = client.query_multi(&filters).await?;
    let events: Vec<serde_json::Value> = serde_json::from_str(&response).unwrap_or_default();
    let metadata_value = events.iter().find(|event| {
        event.get("kind").and_then(serde_json::Value::as_u64) == Some(KIND_GROUP_METADATA as u64)
    });
    let Some(metadata) = metadata_value
        .cloned()
        .map(|value| verified_meeting_summary(value, &relay_pubkey))
        .transpose()?
        .flatten()
    else {
        println!("null");
        return Ok(());
    };

    let create = events.iter().find(|event| {
        event.get("kind").and_then(serde_json::Value::as_u64)
            == Some(buzz_sdk::kind::KIND_MEETING_CREATE as u64)
    });
    let protocol = create.map(MeetingProtocol::from_create_event).transpose()?;
    let end = events.iter().find(|event| {
        event.get("kind").and_then(serde_json::Value::as_u64)
            == Some(buzz_sdk::kind::KIND_MEETING_END as u64)
    });
    let floor = events
        .iter()
        .filter_map(parse_floor_state)
        .max_by(|left, right| {
            left.floor_revision
                .cmp(&right.floor_revision)
                .then_with(|| left.state_event_id.cmp(&right.state_event_id))
        });
    let baton = if matches!(
        protocol,
        Some(
            MeetingProtocol::ModeratedBatonV1
                | MeetingProtocol::ModeratedBoardV2
                | MeetingProtocol::ModeratedBoardActionsV2
        )
    ) {
        fetch_current_baton(client, &meeting_id).await?
    } else {
        None
    };
    let moderator_pubkey = match protocol {
        Some(MeetingProtocol::ModeratedBoardV2 | MeetingProtocol::ModeratedBoardActionsV2) => {
            create
                .and_then(|event| event.get("pubkey"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        }
        _ => create
            .map(|event| extract_tag_value(event, "moderator"))
            .filter(|value| !value.is_empty()),
    };

    let output = serde_json::json!({
        "meeting_id": metadata.meeting_id,
        "title": metadata.title,
        "description": metadata.description,
        "summary": metadata.summary,
        "room_kind": metadata.room_kind,
        "status": metadata.status,
        "host_pubkey": create
            .and_then(|event| event.get("pubkey"))
            .and_then(serde_json::Value::as_str),
        "source_channel_id": create
            .map(|event| extract_tag_value(event, "source"))
            .filter(|value| !value.is_empty()),
        "schema_version": create
            .map(|event| extract_tag_value(event, "v"))
            .filter(|value| !value.is_empty()),
        "floor_policy_version": protocol.map(MeetingProtocol::policy),
        "moderator_pubkey": moderator_pubkey,
        "create_event_id": create
            .and_then(|event| event.get("id"))
            .and_then(serde_json::Value::as_str),
        "created_at": create
            .and_then(|event| event.get("created_at"))
            .and_then(serde_json::Value::as_u64),
        "end_event_id": end
            .and_then(|event| event.get("id"))
            .and_then(serde_json::Value::as_str),
        "ended_at": end
            .and_then(|event| event.get("created_at"))
            .and_then(serde_json::Value::as_u64),
        "ended_by": end
            .and_then(|event| event.get("pubkey"))
            .and_then(serde_json::Value::as_str),
        "terminal_outcome": end
            .map(|event| extract_tag_value(event, "outcome"))
            .filter(|value| !value.is_empty()),
        "terminal_reason_code": end
            .map(|event| extract_tag_value(event, "reason-code"))
            .filter(|value| !value.is_empty()),
        "terminal_reason": end
            .and_then(|event| event.get("content"))
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty()),
        "floor": floor,
        "baton": baton,
    });
    println!("{output}");
    Ok(())
}

async fn fetch_current_meeting_board_event(
    client: &CarryforthClient,
    meeting_id: Uuid,
    protocol: MeetingProtocol,
) -> Result<Value, CliError> {
    let meeting_id = meeting_id.to_string();
    if !protocol.is_v2() {
        return Err(CliError::Usage(format!(
            "meeting {meeting_id} does not use {}",
            buzz_sdk::MEETING_V2_POLICY
        )));
    }

    let filter = serde_json::json!({
        "kinds": [KIND_MEETING_BOARD],
        "#h": [&meeting_id],
        "limit": 10,
    });
    let response = client.query(&filter).await?;
    let events: Vec<serde_json::Value> = serde_json::from_str(&response).unwrap_or_default();
    events
        .into_iter()
        .filter(|event| {
            event.get("kind").and_then(serde_json::Value::as_u64) == Some(KIND_MEETING_BOARD as u64)
                && extract_tag_value(event, "h") == meeting_id
        })
        .max_by(|left, right| {
            left.get("created_at")
                .and_then(serde_json::Value::as_u64)
                .cmp(&right.get("created_at").and_then(serde_json::Value::as_u64))
                .then_with(|| event_id(left).cmp(event_id(right)))
        })
        .ok_or_else(|| CliError::NotFound(format!("meeting board not found: {meeting_id}")))
}

pub async fn cmd_get_meeting_board(
    client: &CarryforthClient,
    meeting_id: &str,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
    let protocol = fetch_meeting_protocol(client, &meeting_id.to_string()).await?;
    let event = fetch_current_meeting_board_event(client, meeting_id, protocol).await?;

    if extract_tag_value(&event, "v") != buzz_sdk::MEETING_V2_SCHEMA_VERSION
        || extract_tag_value(&event, "policy") != protocol.policy()
        || extract_tag_value(&event, "format") != buzz_sdk::MEETING_V2_BOARD_FORMAT
    {
        return Err(CliError::Other(format!(
            "invalid Meeting V2 board projection: {meeting_id}"
        )));
    }
    let moderator = extract_tag_value(&event, "moderator");
    validate_hex64(&moderator).map_err(|_| {
        CliError::Other(format!("invalid Meeting V2 board moderator: {meeting_id}"))
    })?;
    let content = event
        .get("content")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            CliError::Other(format!("invalid Meeting V2 board content: {meeting_id}"))
        })?;
    let board = buzz_sdk::parse_meeting_v2_board_content(content).map_err(|error| {
        CliError::Other(format!(
            "invalid Meeting V2 board content for {meeting_id}: {error}"
        ))
    })?;
    let event_id = event_id(&event);
    validate_hex64(event_id)
        .map_err(|_| CliError::Other(format!("invalid Meeting V2 board event ID: {meeting_id}")))?;
    let output = MeetingBoardOutput {
        meeting_id: meeting_id.to_string(),
        format: board.format,
        body: board.body,
        moderator,
        event_id: event_id.to_string(),
        created_at: event
            .get("created_at")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
    };
    println!(
        "{}",
        serde_json::to_string(&output).unwrap_or_else(|_| "null".to_string())
    );
    Ok(())
}

fn meeting_v2_board_fences(
    state: &BatonState,
    control_epoch: Option<u64>,
    board_window: Option<u64>,
) -> Result<(u64, u64), CliError> {
    if !baton_is_v2(state) {
        return Err(CliError::Usage(
            "Board Maintenance requires a Meeting V2 State".into(),
        ));
    }
    let board_control = state
        .content
        .get("board_control")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            CliError::Other(format!(
                "Meeting V2 State {} has no board_control",
                state.state_event_id
            ))
        })?;
    let current_control_epoch = board_control
        .get("control_epoch")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| CliError::Other("Meeting V2 State has no valid control_epoch".into()))?;
    let current_board_window = board_control
        .get("board_window")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| CliError::Other("Meeting V2 State has no valid board_window".into()))?;
    Ok((
        control_epoch.unwrap_or(current_control_epoch),
        board_window.unwrap_or(current_board_window),
    ))
}

async fn cmd_meeting_board_action(
    client: &CarryforthClient,
    meeting_id: &str,
    board: Option<&str>,
    control_epoch: Option<u64>,
    board_window: Option<u64>,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
    let state = fetch_required_v1_baton(client, meeting_id).await?;
    let (expected_control_epoch, board_window) =
        meeting_v2_board_fences(&state, control_epoch, board_window)?;
    let board = board.map(read_file_or_stdin).transpose()?;
    let params = buzz_sdk::MeetingV2BoardActionParams {
        session_id: meeting_id,
        expected_control_epoch,
        board_window,
        board: board.as_deref(),
    };
    let builder = if state.policy_version == buzz_sdk::MEETING_V2_ACTIONS_POLICY {
        buzz_sdk::build_meeting_v2_actions_board_action(params)
    } else {
        buzz_sdk::build_meeting_v2_board_action(params)
    }
    .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let command_event_id = event.id.to_hex();
    let response = submit_v1_event(client, meeting_id, event).await?;
    print_v1_write_response(&response, "board_command_id", &command_event_id);
    Ok(())
}

pub async fn cmd_list_participants(
    client: &CarryforthClient,
    meeting_id: &str,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?.to_string();
    if fetch_meeting_metadata(client, &meeting_id).await?.is_none() {
        println!("[]");
        return Ok(());
    }

    let filter = serde_json::json!({
        "kinds": [KIND_GROUP_MEMBERS],
        "#d": [&meeting_id],
        "limit": 1,
    });
    let response = client.query(&filter).await?;
    let events: Vec<serde_json::Value> = serde_json::from_str(&response).unwrap_or_default();
    let participants = events.first().map(extract_p_tags).unwrap_or_default();
    println!(
        "{}",
        serde_json::to_string(&participants).unwrap_or_else(|_| "[]".to_string())
    );
    Ok(())
}

pub async fn cmd_meeting_history(
    client: &CarryforthClient,
    meeting_id: &str,
    limit: u32,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?.to_string();
    let protocol = fetch_meeting_protocol(client, &meeting_id).await?;
    let filter = serde_json::json!({
        "kinds": [KIND_MEETING_SPEECH],
        "#h": [&meeting_id],
    });
    let mut events = client.query_paginated(filter, limit).await?;
    events.sort_by(|left, right| {
        let left_revision = match protocol {
            MeetingProtocol::UniformV0 => event_round(left),
            MeetingProtocol::ModeratedBatonV1
            | MeetingProtocol::ModeratedBoardV2
            | MeetingProtocol::ModeratedBoardActionsV2Legacy
            | MeetingProtocol::ModeratedBoardActionsV2 => event_speech_revision(left),
        };
        let right_revision = match protocol {
            MeetingProtocol::UniformV0 => event_round(right),
            MeetingProtocol::ModeratedBatonV1
            | MeetingProtocol::ModeratedBoardV2
            | MeetingProtocol::ModeratedBoardActionsV2Legacy
            | MeetingProtocol::ModeratedBoardActionsV2 => event_speech_revision(right),
        };
        left_revision
            .cmp(&right_revision)
            .then_with(|| event_id(left).cmp(event_id(right)))
    });

    let output = match format {
        OutputFormat::Json => events,
        OutputFormat::Compact => events
            .iter()
            .map(|event| {
                let grant = extract_tag_value(event, "meeting-grant");
                serde_json::json!({
                    "event_id": event_id(event),
                    "round_number": (protocol == MeetingProtocol::UniformV0)
                        .then(|| event_round(event)),
                    "speech_revision": matches!(
                        protocol,
                        MeetingProtocol::ModeratedBatonV1
                            | MeetingProtocol::ModeratedBoardV2
                            | MeetingProtocol::ModeratedBoardActionsV2
                    )
                        .then(|| event_speech_revision(event)),
                    "grant_id": (!grant.is_empty()).then_some(grant),
                    "author_pubkey": event.get("pubkey").and_then(serde_json::Value::as_str),
                    "content": event.get("content").and_then(serde_json::Value::as_str),
                })
            })
            .collect(),
    };
    println!(
        "{}",
        serde_json::to_string(&output).unwrap_or_else(|_| "[]".to_string())
    );
    Ok(())
}

pub async fn cmd_floor_status(client: &CarryforthClient, meeting_id: &str) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?.to_string();
    match fetch_meeting_protocol(client, &meeting_id).await? {
        MeetingProtocol::UniformV0 => match fetch_current_floor(client, &meeting_id).await? {
            Some(state) => println!(
                "{}",
                serde_json::to_string(&state).unwrap_or_else(|_| "null".to_string())
            ),
            None => println!("null"),
        },
        MeetingProtocol::ModeratedBatonV1
        | MeetingProtocol::ModeratedBoardV2
        | MeetingProtocol::ModeratedBoardActionsV2Legacy
        | MeetingProtocol::ModeratedBoardActionsV2 => {
            match fetch_current_baton(client, &meeting_id).await? {
                Some(state) => println!(
                    "{}",
                    serde_json::to_string(&state).unwrap_or_else(|_| "null".to_string())
                ),
                None => println!("null"),
            }
        }
    }
    Ok(())
}

pub async fn cmd_floor_history(
    client: &CarryforthClient,
    meeting_id: &str,
    limit: u32,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?.to_string();
    match fetch_meeting_protocol(client, &meeting_id).await? {
        MeetingProtocol::ModeratedBatonV1
        | MeetingProtocol::ModeratedBoardV2
        | MeetingProtocol::ModeratedBoardActionsV2Legacy
        | MeetingProtocol::ModeratedBoardActionsV2 => {
            let states = fetch_baton_states(client, &meeting_id, limit).await?;
            let output = match format {
                OutputFormat::Json => serde_json::to_value(states)
                    .unwrap_or_else(|_| serde_json::Value::Array(Vec::new())),
                OutputFormat::Compact => serde_json::Value::Array(
                    states
                        .iter()
                        .map(|state| {
                            serde_json::json!({
                                "event_id": state.state_event_id,
                                "state_revision": state.state_revision,
                                "floor_revision": state.floor_revision,
                                "intent_revision": state.intent_revision,
                                "speech_revision": state.speech_revision,
                                "phase": state.phase,
                                "moderator_pubkey": state.moderator_pubkey,
                            })
                        })
                        .collect(),
                ),
            };
            println!("{output}");
            return Ok(());
        }
        MeetingProtocol::UniformV0 => {}
    }

    let filter = serde_json::json!({
        "kinds": [
            KIND_MEETING_FLOOR_CLAIM,
            KIND_MEETING_ROUND_STATE,
            KIND_MEETING_FLOOR_SIGNAL
        ],
        "#h": [&meeting_id],
    });
    let mut events = client.query_paginated(filter, limit).await?;
    events.sort_by(|left, right| {
        event_round(left)
            .cmp(&event_round(right))
            .then_with(|| event_floor_revision(left).cmp(&event_floor_revision(right)))
            .then_with(|| event_id(left).cmp(event_id(right)))
    });
    let output = match format {
        OutputFormat::Json => events,
        OutputFormat::Compact => events
            .iter()
            .map(|event| {
                let phase = extract_tag_value(event, "phase");
                let action = extract_tag_value(event, "action");
                let intent_basis = extract_tag_value(event, "intent-basis");
                serde_json::json!({
                    "event_id": event_id(event),
                    "kind": event.get("kind").and_then(serde_json::Value::as_u64),
                    "round_number": event_round(event),
                    "floor_revision": event_floor_revision(event),
                    "phase": (!phase.is_empty()).then_some(phase),
                    "action": (!action.is_empty()).then_some(action),
                    "intent_basis": (!intent_basis.is_empty()).then_some(intent_basis),
                    "author_pubkey": event.get("pubkey").and_then(serde_json::Value::as_str),
                })
            })
            .collect(),
    };
    println!(
        "{}",
        serde_json::to_string(&output).unwrap_or_else(|_| "[]".to_string())
    );
    Ok(())
}

pub async fn cmd_meeting_actions_status(
    client: &CarryforthClient,
    meeting_id: &str,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
    let protocol = fetch_meeting_protocol(client, &meeting_id.to_string()).await?;
    if !protocol.has_action_finalization() {
        return Err(CliError::Usage(format!(
            "meeting {meeting_id} does not use {}",
            buzz_sdk::MEETING_V2_ACTIONS_POLICY
        )));
    }
    let state = fetch_required_v1_baton(client, meeting_id).await?;
    let board_control = state
        .content
        .get("board_control")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| CliError::Other("Meeting State has no board_control".into()))?;
    let output = serde_json::json!({
        "meeting_id": meeting_id,
        "policy": state.policy_version,
        "state_event_id": state.state_event_id,
        "state_revision": state.state_revision,
        "meeting_phase": board_control.get("phase"),
        "action": board_control.get("action"),
    });
    println!("{output}");
    Ok(())
}

async fn meeting_action_context(
    client: &CarryforthClient,
    meeting_id: Uuid,
) -> Result<(BatonState, MeetingActionRunView), CliError> {
    let protocol = fetch_meeting_protocol(client, &meeting_id.to_string()).await?;
    if !protocol.has_action_finalization() {
        return Err(CliError::Usage(format!(
            "meeting {meeting_id} does not use {}",
            buzz_sdk::MEETING_V2_ACTIONS_POLICY
        )));
    }
    let state = fetch_required_v1_baton(client, meeting_id).await?;
    if state.moderator_pubkey != client.public_key().to_hex() {
        return Err(CliError::Auth(
            "only the frozen Meeting moderator can operate its action run".to_owned(),
        ));
    }
    let board_control = state
        .content
        .get("board_control")
        .and_then(Value::as_object)
        .ok_or_else(|| CliError::Other("Meeting State has no board_control".to_owned()))?;
    if board_control.get("phase").and_then(Value::as_str) != Some("finalizing_actions") {
        return Err(CliError::Conflict(
            "Meeting is not finalizing actions".to_owned(),
        ));
    }
    let action: MeetingActionRunView = serde_json::from_value(
        board_control
            .get("action")
            .cloned()
            .ok_or_else(|| CliError::Conflict("Meeting has no active action run".to_owned()))?,
    )
    .map_err(|error| CliError::Other(format!("invalid Meeting action State: {error}")))?;
    Ok((state, action))
}

fn meeting_action_fence<'a>(
    action: &'a MeetingActionRunView,
) -> buzz_sdk::MeetingV2ActionRunFence<'a> {
    buzz_sdk::MeetingV2ActionRunFence {
        action_run_id: action.action_run_id,
        action_window: action.action_window_epoch,
        board_event_id: &action.board_event_id,
    }
}

pub async fn cmd_update_meeting_summary(
    client: &CarryforthClient,
    meeting_id: &str,
    summary: Option<&str>,
    clear_summary: bool,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
    let intended = match (summary, clear_summary) {
        (Some(value), false) => {
            let value = read_or_stdin(value)?;
            if value.trim().is_empty() || value.contains('\0') {
                return Err(CliError::Usage(
                    "--summary must contain non-blank text without NUL".into(),
                ));
            }
            Some(value)
        }
        (None, true) => None,
        _ => {
            return Err(CliError::Usage(
                "choose exactly one of --summary or --clear-summary".into(),
            ));
        }
    };
    meeting_relay_identity(client, true).await?;
    let (_, action) = meeting_action_context(client, meeting_id).await?;
    if action.condition != "runnable" {
        return Err(CliError::Conflict(
            "Meeting action run is not runnable".into(),
        ));
    }
    let current = fetch_meeting_metadata(client, &meeting_id.to_string())
        .await?
        .ok_or_else(|| CliError::NotFound(format!("meeting not found: {meeting_id}")))?;
    if current.summary == intended {
        println!(
            "{}",
            serde_json::json!({
                "meeting_id": meeting_id,
                "accepted": true,
                "changed": false,
                "summary": intended,
            })
        );
        return Ok(());
    }

    let mutation = match intended.as_deref() {
        Some(value) => buzz_sdk::MeetingSummaryMutation::Set(value),
        None => buzz_sdk::MeetingSummaryMutation::Clear,
    };
    let builder = buzz_sdk::build_meeting_summary_update(buzz_sdk::MeetingSummaryUpdateParams {
        session_id: meeting_id,
        mutation,
        action_fence: meeting_action_fence(&action),
    })
    .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let event_id = event.id.to_hex();
    let response = submit_v1_event(client, meeting_id, event).await?;

    for attempt in 0..3 {
        let observed = fetch_meeting_metadata(client, &meeting_id.to_string()).await?;
        if observed
            .as_ref()
            .and_then(|metadata| metadata.summary.as_ref())
            == intended.as_ref()
        {
            let mut output = serde_json::from_str::<Value>(&normalize_write_response(&response))
                .unwrap_or_else(|_| serde_json::json!({ "accepted": true }));
            if let Some(object) = output.as_object_mut() {
                object.insert("meeting_id".into(), serde_json::json!(meeting_id));
                object.insert("event_id".into(), Value::String(event_id));
                object.insert("summary".into(), serde_json::json!(intended));
                object.insert("readback_verified".into(), Value::Bool(true));
            }
            println!("{output}");
            return Ok(());
        }
        if attempt < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(50_u64 << attempt)).await;
        }
    }
    Err(CliError::Other(format!(
        "Meeting summary command {event_id} was accepted but canonical metadata readback is not yet confirmed"
    )))
}

async fn submit_meeting_action_builder(
    client: &CarryforthClient,
    meeting_id: Uuid,
    builder: nostr::EventBuilder,
    id_key: &str,
) -> Result<Value, CliError> {
    let event = client.sign_event(builder)?;
    let event_id = event.id.to_hex();
    let response = submit_v1_event(client, meeting_id, event).await?;
    Ok(v1_write_response(&response, id_key, &event_id))
}

pub async fn cmd_meeting_actions_begin(
    client: &CarryforthClient,
    meeting_id: &str,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
    let protocol = fetch_meeting_protocol(client, &meeting_id.to_string()).await?;
    if !protocol.has_action_finalization() {
        return Err(CliError::Usage(format!(
            "meeting {meeting_id} does not use {}",
            buzz_sdk::MEETING_V2_ACTIONS_POLICY
        )));
    }
    let state = fetch_required_v1_baton(client, meeting_id).await?;
    if state.moderator_pubkey != client.public_key().to_hex() {
        return Err(CliError::Auth(
            "only the frozen Meeting moderator can begin action finalization".to_owned(),
        ));
    }
    let board_control = state
        .content
        .get("board_control")
        .and_then(Value::as_object)
        .ok_or_else(|| CliError::Other("Meeting State has no board_control".to_owned()))?;
    if board_control.get("phase").and_then(Value::as_str) != Some("floor_ready")
        || !matches!(
            board_control.get("board_outcome").and_then(Value::as_str),
            Some("updated" | "unchanged")
        )
    {
        return Err(CliError::Conflict(
            "Meeting does not have a completed final Board".to_owned(),
        ));
    }
    let control_epoch = board_control
        .get("control_epoch")
        .and_then(Value::as_u64)
        .ok_or_else(|| CliError::Other("Meeting State has no control_epoch".to_owned()))?;
    let board_window = board_control
        .get("board_window")
        .and_then(Value::as_u64)
        .ok_or_else(|| CliError::Other("Meeting State has no board_window".to_owned()))?;
    let board = fetch_current_meeting_board_event(client, meeting_id, protocol).await?;
    let board_event_id = event_id(&board);
    validate_hex64(board_event_id)?;
    let builder = buzz_sdk::build_meeting_v2_action_begin(buzz_sdk::MeetingV2ActionBeginParams {
        session_id: meeting_id,
        expected_control_epoch: control_epoch,
        board_window,
        expected_state_event_id: &state.state_event_id,
        board_event_id,
        expected_decision_attempt_id: None,
    })
    .map_err(sdk_err)?;
    println!(
        "{}",
        submit_meeting_action_builder(client, meeting_id, builder, "action_command_id").await?
    );
    Ok(())
}

async fn cmd_meeting_actions_empty_command(
    client: &CarryforthClient,
    meeting_id: &str,
    action_name: &str,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
    let (_, action) = meeting_action_context(client, meeting_id).await?;
    if action_name == "retry" && action.condition != "blocked" {
        return Err(CliError::Conflict(
            "Meeting action run is not blocked".to_owned(),
        ));
    }
    let params = buzz_sdk::MeetingV2ActionCommandParams {
        session_id: meeting_id,
        fence: meeting_action_fence(&action),
    };
    let builder = match action_name {
        "retry" => buzz_sdk::build_meeting_v2_action_retry(params),
        "return-to-board" => buzz_sdk::build_meeting_v2_action_return_to_board(params),
        _ => {
            return Err(CliError::Other(
                "unsupported Meeting action command".to_owned(),
            ))
        }
    }
    .map_err(sdk_err)?;
    println!(
        "{}",
        submit_meeting_action_builder(client, meeting_id, builder, "action_command_id").await?
    );
    Ok(())
}

pub async fn cmd_meeting_actions_block(
    client: &CarryforthClient,
    meeting_id: &str,
    reason_code: &str,
    reason: Option<&str>,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
    let (_, action) = meeting_action_context(client, meeting_id).await?;
    if action.condition != "runnable" {
        return Err(CliError::Conflict(
            "Meeting action run is not runnable".to_owned(),
        ));
    }
    let builder = buzz_sdk::build_meeting_v2_action_block(buzz_sdk::MeetingV2ActionBlockParams {
        session_id: meeting_id,
        fence: meeting_action_fence(&action),
        reason_code,
        reason,
    })
    .map_err(sdk_err)?;
    println!(
        "{}",
        submit_meeting_action_builder(client, meeting_id, builder, "action_command_id").await?
    );
    Ok(())
}

pub async fn cmd_meeting_actions_retry(
    client: &CarryforthClient,
    meeting_id: &str,
) -> Result<(), CliError> {
    cmd_meeting_actions_empty_command(client, meeting_id, "retry").await
}

pub async fn cmd_meeting_actions_return_to_board(
    client: &CarryforthClient,
    meeting_id: &str,
) -> Result<(), CliError> {
    cmd_meeting_actions_empty_command(client, meeting_id, "return-to-board").await
}

async fn cmd_intents_list(
    client: &CarryforthClient,
    meeting_id: &str,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
    let state = fetch_required_v1_baton(client, meeting_id).await?;
    let intents = baton_array(&state, "pending_intents")?;
    let output = match format {
        OutputFormat::Json => serde_json::Value::Array(intents.to_vec()),
        OutputFormat::Compact => serde_json::Value::Array(
            intents
                .iter()
                .map(|intent| {
                    serde_json::json!({
                        "intent_id": intent.get("intent_id"),
                        "current_event_id": intent.get("current_event_id"),
                        "author_pubkey": intent.get("author_pubkey"),
                        "summary": intent.get("summary"),
                        "addressed_to": intent.get("addressed_to"),
                        "deferred": intent.get("deferred"),
                        "selection_attempt_count": intent.get("selection_attempt_count"),
                    })
                })
                .collect(),
        ),
    };
    println!("{output}");
    Ok(())
}

async fn cmd_intent_submit(
    client: &CarryforthClient,
    meeting_id: &str,
    summary: &str,
    addressed_to: Option<&str>,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
    let state = fetch_required_v1_baton(client, meeting_id).await?;
    let summary = read_or_stdin(summary)?;
    let builder = build_moderated!(
        &state,
        buzz_sdk::build_meeting_v1_intent_submit,
        buzz_sdk::build_meeting_v2_intent_submit,
        buzz_sdk::MeetingV1IntentSubmitParams {
            session_id: meeting_id,
            basis_speech_revision: state.speech_revision,
            addressed_to,
            summary: &summary,
        }
    )
    .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let intent_id = event.id.to_hex();
    let response = submit_v1_event(client, meeting_id, event).await?;
    print_v1_write_response(&response, "intent_id", &intent_id);
    Ok(())
}

async fn cmd_intent_refresh(
    client: &CarryforthClient,
    meeting_id: &str,
    intent_id: &str,
    summary: &str,
    addressed_to: Option<&str>,
) -> Result<(), CliError> {
    validate_hex64(intent_id)?;
    let meeting_id = parse_uuid(meeting_id)?;
    let state = fetch_required_v1_baton(client, meeting_id).await?;
    let intent = baton_object(
        &state,
        "pending_intents",
        "intent_id",
        &intent_id.to_ascii_lowercase(),
    )?;
    let previous_event_id = object_string(intent, "current_event_id", "canonical pending Intent")?;
    let summary = read_or_stdin(summary)?;
    let builder = build_moderated!(
        &state,
        buzz_sdk::build_meeting_v1_intent_refresh,
        buzz_sdk::build_meeting_v2_intent_refresh,
        buzz_sdk::MeetingV1IntentRefreshParams {
            session_id: meeting_id,
            intent_id,
            previous_event_id,
            basis_speech_revision: state.speech_revision,
            addressed_to,
            summary: &summary,
        }
    )
    .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let command_event_id = event.id.to_hex();
    let response = submit_v1_event(client, meeting_id, event).await?;
    print_v1_write_response(&response, "intent_event_id", &command_event_id);
    Ok(())
}

async fn cmd_intent_withdraw(
    client: &CarryforthClient,
    meeting_id: &str,
    intent_id: &str,
) -> Result<(), CliError> {
    validate_hex64(intent_id)?;
    let meeting_id = parse_uuid(meeting_id)?;
    let state = fetch_required_v1_baton(client, meeting_id).await?;
    let intent = baton_object(
        &state,
        "pending_intents",
        "intent_id",
        &intent_id.to_ascii_lowercase(),
    )?;
    let previous_event_id = object_string(intent, "current_event_id", "canonical pending Intent")?;
    let builder = build_moderated!(
        &state,
        buzz_sdk::build_meeting_v1_intent_withdraw,
        buzz_sdk::build_meeting_v2_intent_withdraw,
        buzz_sdk::MeetingV1IntentWithdrawParams {
            session_id: meeting_id,
            intent_id,
            previous_event_id,
        }
    )
    .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let response = submit_v1_event(client, meeting_id, event).await?;
    print_v1_write_response(&response, "intent_id", intent_id);
    Ok(())
}

fn sdk_rejection_reason(
    reason: crate::MeetingIntentRejectionReason,
) -> buzz_sdk::MeetingV1IntentRejectionReason {
    match reason {
        crate::MeetingIntentRejectionReason::OffTopic => {
            buzz_sdk::MeetingV1IntentRejectionReason::OffTopic
        }
        crate::MeetingIntentRejectionReason::Duplicate => {
            buzz_sdk::MeetingV1IntentRejectionReason::Duplicate
        }
        crate::MeetingIntentRejectionReason::Superseded => {
            buzz_sdk::MeetingV1IntentRejectionReason::Superseded
        }
        crate::MeetingIntentRejectionReason::Unsupported => {
            buzz_sdk::MeetingV1IntentRejectionReason::Unsupported
        }
        crate::MeetingIntentRejectionReason::AgendaMismatch => {
            buzz_sdk::MeetingV1IntentRejectionReason::AgendaMismatch
        }
    }
}

fn sdk_dismiss_reason(
    reason: crate::MeetingHandoffDismissReason,
) -> buzz_sdk::MeetingV1HandoffDismissReason {
    match reason {
        crate::MeetingHandoffDismissReason::Superseded => {
            buzz_sdk::MeetingV1HandoffDismissReason::Superseded
        }
        crate::MeetingHandoffDismissReason::AnsweredElsewhere => {
            buzz_sdk::MeetingV1HandoffDismissReason::AnsweredElsewhere
        }
        crate::MeetingHandoffDismissReason::OutOfScope => {
            buzz_sdk::MeetingV1HandoffDismissReason::OutOfScope
        }
        crate::MeetingHandoffDismissReason::NoLongerNeeded => {
            buzz_sdk::MeetingV1HandoffDismissReason::NoLongerNeeded
        }
    }
}

fn active_attempt_candidate<'a>(
    state: &'a BatonState,
    attempt_id: &str,
    source_type: &str,
    source_id: &str,
) -> Result<&'a serde_json::Value, CliError> {
    validate_hex64(attempt_id)?;
    let attempt = state
        .content
        .get("active_decision_attempt")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            CliError::Other(format!(
                "moderated Meeting State {} has no active DecisionAttempt",
                state.state_event_id
            ))
        })?;
    let canonical_attempt = attempt
        .get("attempt_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| CliError::Other("active DecisionAttempt has no attempt_id".into()))?;
    if !canonical_attempt.eq_ignore_ascii_case(attempt_id) {
        return Err(CliError::Other(format!(
            "DecisionAttempt {attempt_id} is not active; Relay State has {canonical_attempt}"
        )));
    }
    attempt
        .get("candidate_refs")
        .and_then(serde_json::Value::as_array)
        .and_then(|candidates| {
            candidates.iter().find(|candidate| {
                candidate
                    .get("source_type")
                    .and_then(serde_json::Value::as_str)
                    == Some(source_type)
                    && candidate
                        .get("source_id")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|value| value.eq_ignore_ascii_case(source_id))
            })
        })
        .ok_or_else(|| {
            CliError::Other(format!(
                "{source_type} {source_id} is not in DecisionAttempt {attempt_id}"
            ))
        })
}

async fn cmd_moderator_select(
    client: &CarryforthClient,
    meeting_id: &str,
    intent_id: Option<&str>,
    handoff_id: Option<&str>,
    reason: Option<&str>,
    deferrals: &[String],
    attempt_id: Option<&str>,
) -> Result<(), CliError> {
    let source_count = usize::from(intent_id.is_some()) + usize::from(handoff_id.is_some());
    if source_count != 1 {
        return Err(CliError::Usage(
            "--intent and --handoff require exactly one value".into(),
        ));
    }
    let meeting_id = parse_uuid(meeting_id)?;
    let state = fetch_required_v1_baton(client, meeting_id).await?;
    let mut expected_source_event_id = None;
    let selection = if let Some(intent_id) = intent_id {
        validate_hex64(intent_id)?;
        baton_object(
            &state,
            "pending_intents",
            "intent_id",
            &intent_id.to_ascii_lowercase(),
        )?;
        if let Some(attempt_id) = attempt_id {
            let candidate = active_attempt_candidate(&state, attempt_id, "intent", intent_id)?;
            expected_source_event_id = Some(
                object_string(
                    candidate,
                    "current_event_id",
                    "DecisionAttempt Intent candidate",
                )?
                .to_string(),
            );
        }
        buzz_sdk::MeetingV1Selection::Intent { intent_id }
    } else {
        let handoff_id = handoff_id.unwrap_or_default();
        validate_hex64(handoff_id)?;
        let handoff = baton_object(
            &state,
            "unresolved_handoffs",
            "handoff_id",
            &handoff_id.to_ascii_lowercase(),
        )?;
        let expected_attempt_count = handoff
            .get("attempt_count")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| CliError::Other("canonical Handoff has no attempt_count".into()))?;
        if let Some(attempt_id) = attempt_id {
            active_attempt_candidate(&state, attempt_id, "handoff", handoff_id)?;
        }
        buzz_sdk::MeetingV1Selection::Handoff {
            handoff_id,
            expected_attempt_count,
        }
    };

    let mut parsed_deferrals = Vec::with_capacity(deferrals.len());
    for value in deferrals {
        let (deferred_id, deferred_reason) = value
            .split_once(':')
            .ok_or_else(|| CliError::Usage("--defer must use INTENT_ID:REASON format".into()))?;
        validate_hex64(deferred_id)?;
        let deferred = baton_object(
            &state,
            "pending_intents",
            "intent_id",
            &deferred_id.to_ascii_lowercase(),
        )?;
        let previous_event_id =
            object_string(deferred, "current_event_id", "canonical deferred Intent")?;
        parsed_deferrals.push((
            deferred_id.to_string(),
            previous_event_id.to_string(),
            deferred_reason.to_string(),
        ));
    }
    let sdk_deferrals: Vec<buzz_sdk::MeetingV1IntentDeferral<'_>> = parsed_deferrals
        .iter()
        .map(
            |(intent_id, previous_event_id, reason)| buzz_sdk::MeetingV1IntentDeferral {
                intent_id,
                previous_event_id,
                reason,
            },
        )
        .collect();
    let builder = build_moderated!(
        &state,
        buzz_sdk::build_meeting_v1_moderator_select,
        buzz_sdk::build_meeting_v2_moderator_select,
        buzz_sdk::MeetingV1ModeratorSelectParams {
            session_id: meeting_id,
            selection,
            expected_control_epoch: baton_u64(&state, "control_epoch")?,
            expected_decision_epoch: baton_u64(&state, "decision_epoch")?,
            expected_intent_revision: state.intent_revision,
            expected_speech_revision: state.speech_revision,
            selection_reason: reason,
            deferrals: &sdk_deferrals,
            attempt_id,
            expected_source_event_id: expected_source_event_id.as_deref(),
        }
    )
    .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let command_event_id = event.id.to_hex();
    let response = submit_v1_event(client, meeting_id, event).await?;
    print_v1_write_response(&response, "selection_event_id", &command_event_id);
    Ok(())
}

async fn cmd_moderator_reject(
    client: &CarryforthClient,
    meeting_id: &str,
    intent_id: &str,
    reason_code: crate::MeetingIntentRejectionReason,
    reason: &str,
    attempt_id: Option<&str>,
) -> Result<(), CliError> {
    validate_hex64(intent_id)?;
    let meeting_id = parse_uuid(meeting_id)?;
    let state = fetch_required_v1_baton(client, meeting_id).await?;
    let intent = baton_object(
        &state,
        "pending_intents",
        "intent_id",
        &intent_id.to_ascii_lowercase(),
    )?;
    let previous_event_id = object_string(intent, "current_event_id", "canonical pending Intent")?;
    let author_pubkey = object_string(intent, "author_pubkey", "canonical pending Intent")?;
    let builder = build_moderated!(
        &state,
        buzz_sdk::build_meeting_v1_moderator_reject,
        buzz_sdk::build_meeting_v2_moderator_reject,
        buzz_sdk::MeetingV1ModeratorRejectParams {
            session_id: meeting_id,
            intent_id,
            previous_event_id,
            intent_author_pubkey: author_pubkey,
            reason_code: sdk_rejection_reason(reason_code),
            reason_text: reason,
            attempt_id,
        }
    )
    .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let response = submit_v1_event(client, meeting_id, event).await?;
    print_v1_write_response(&response, "intent_id", intent_id);
    Ok(())
}

async fn cmd_moderator_dismiss_handoff(
    client: &CarryforthClient,
    meeting_id: &str,
    handoff_id: &str,
    reason_code: crate::MeetingHandoffDismissReason,
    reason: &str,
    attempt_id: Option<&str>,
) -> Result<(), CliError> {
    validate_hex64(handoff_id)?;
    let meeting_id = parse_uuid(meeting_id)?;
    let state = fetch_required_v1_baton(client, meeting_id).await?;
    let handoff = baton_object(
        &state,
        "unresolved_handoffs",
        "handoff_id",
        &handoff_id.to_ascii_lowercase(),
    )?;
    let expected_attempt_count = handoff
        .get("attempt_count")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| CliError::Other("canonical Handoff has no attempt_count".into()))?;
    let builder = build_moderated!(
        &state,
        buzz_sdk::build_meeting_v1_moderator_dismiss_handoff,
        buzz_sdk::build_meeting_v2_moderator_dismiss_handoff,
        buzz_sdk::MeetingV1ModeratorDismissHandoffParams {
            session_id: meeting_id,
            handoff_id,
            expected_speech_revision: state.speech_revision,
            expected_attempt_count,
            reason_code: sdk_dismiss_reason(reason_code),
            reason_text: reason,
            attempt_id,
        },
    )
    .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let response = submit_v1_event(client, meeting_id, event).await?;
    print_v1_write_response(&response, "handoff_id", handoff_id);
    Ok(())
}

async fn cmd_moderator_attempt_start(
    client: &CarryforthClient,
    meeting_id: &str,
    replacement: Option<&str>,
) -> Result<(), CliError> {
    if let Some(replacement) = replacement {
        validate_hex64(replacement)?;
    }
    let meeting_id = parse_uuid(meeting_id)?;
    let state = fetch_required_v1_baton(client, meeting_id).await?;
    let builder = build_moderated!(
        &state,
        buzz_sdk::build_meeting_v1_decision_attempt_start,
        buzz_sdk::build_meeting_v2_decision_attempt_start,
        buzz_sdk::MeetingV1DecisionAttemptStartParams {
            session_id: meeting_id,
            expected_control_epoch: baton_u64(&state, "control_epoch")?,
            expected_decision_epoch: baton_u64(&state, "decision_epoch")?,
            expected_intent_revision: state.intent_revision,
            expected_speech_revision: state.speech_revision,
            expected_state_event_id: &state.state_event_id,
            replacement_of_attempt_id: replacement,
        },
    )
    .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let response = submit_v1_event(client, meeting_id, event).await?;
    println!("{}", normalize_write_response(&response));
    Ok(())
}

async fn cmd_moderator_attempt_finish(
    client: &CarryforthClient,
    meeting_id: &str,
    attempt_id: &str,
    outcome: crate::MeetingDecisionAttemptFinishOutcome,
    reason_code: &str,
) -> Result<(), CliError> {
    validate_hex64(attempt_id)?;
    let meeting_id = parse_uuid(meeting_id)?;
    let state = fetch_required_v1_baton(client, meeting_id).await?;
    let outcome = match outcome {
        crate::MeetingDecisionAttemptFinishOutcome::Completed => {
            buzz_sdk::MeetingV1DecisionAttemptFinishOutcome::Completed
        }
        crate::MeetingDecisionAttemptFinishOutcome::Discarded => {
            buzz_sdk::MeetingV1DecisionAttemptFinishOutcome::Discarded
        }
    };
    let builder = build_moderated!(
        &state,
        buzz_sdk::build_meeting_v1_decision_attempt_finish,
        buzz_sdk::build_meeting_v2_decision_attempt_finish,
        buzz_sdk::MeetingV1DecisionAttemptFinishParams {
            session_id: meeting_id,
            attempt_id,
            outcome,
            reason_code,
        },
    )
    .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let response = submit_v1_event(client, meeting_id, event).await?;
    println!("{}", normalize_write_response(&response));
    Ok(())
}

async fn cmd_moderator_retry(
    client: &CarryforthClient,
    meeting_id: &str,
    attempt_id: &str,
    retry_ticket_id: &str,
    failed_action_event_id: &str,
    attempt_number: u64,
) -> Result<(), CliError> {
    for id in [attempt_id, retry_ticket_id, failed_action_event_id] {
        validate_hex64(id)?;
    }
    let meeting_id = parse_uuid(meeting_id)?;
    let state = fetch_required_v1_baton(client, meeting_id).await?;
    let builder = build_moderated!(
        &state,
        buzz_sdk::build_meeting_v1_decision_retry,
        buzz_sdk::build_meeting_v2_decision_retry,
        buzz_sdk::MeetingV1DecisionRetryParams {
            session_id: meeting_id,
            attempt_id,
            retry_ticket_id,
            failed_action_event_id,
            expected_control_epoch: baton_u64(&state, "control_epoch")?,
            expected_decision_epoch: baton_u64(&state, "decision_epoch")?,
            expected_attempt_number: attempt_number,
        }
    )
    .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let response = submit_v1_event(client, meeting_id, event).await?;
    println!("{}", normalize_write_response(&response));
    Ok(())
}

async fn cmd_moderator_complete_cohort(
    client: &CarryforthClient,
    meeting_id: &str,
    attempt_id: &str,
) -> Result<(), CliError> {
    validate_hex64(attempt_id)?;
    let meeting_id = parse_uuid(meeting_id)?;
    let state = fetch_required_v1_baton(client, meeting_id).await?;
    let builder = build_moderated!(
        &state,
        buzz_sdk::build_meeting_v1_complete_cohort,
        buzz_sdk::build_meeting_v2_complete_cohort,
        buzz_sdk::MeetingV1CompleteCohortParams {
            session_id: meeting_id,
            attempt_id,
            expected_control_epoch: baton_u64(&state, "control_epoch")?,
            expected_decision_epoch: baton_u64(&state, "decision_epoch")?,
        }
    )
    .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let response = submit_v1_event(client, meeting_id, event).await?;
    println!("{}", normalize_write_response(&response));
    Ok(())
}

async fn cmd_moderator_attempt_abandon(
    client: &CarryforthClient,
    meeting_id: &str,
    attempt_id: &str,
) -> Result<(), CliError> {
    validate_hex64(attempt_id)?;
    let meeting_id = parse_uuid(meeting_id)?;
    let state = fetch_required_v1_baton(client, meeting_id).await?;
    let builder = build_moderated!(
        &state,
        buzz_sdk::build_meeting_v1_decision_attempt_abandon,
        buzz_sdk::build_meeting_v2_decision_attempt_abandon,
        buzz_sdk::MeetingV1DecisionAttemptAbandonParams {
            session_id: meeting_id,
            attempt_id,
        },
    )
    .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let response = submit_v1_event(client, meeting_id, event).await?;
    println!("{}", normalize_write_response(&response));
    Ok(())
}

async fn cmd_moderator_withdraw_self(
    client: &CarryforthClient,
    meeting_id: &str,
    attempt_id: &str,
    intent_id: &str,
) -> Result<(), CliError> {
    validate_hex64(attempt_id)?;
    validate_hex64(intent_id)?;
    let meeting_id = parse_uuid(meeting_id)?;
    let state = fetch_required_v1_baton(client, meeting_id).await?;
    let intent = baton_object(
        &state,
        "pending_intents",
        "intent_id",
        &intent_id.to_ascii_lowercase(),
    )?;
    let previous_event_id = object_string(intent, "current_event_id", "canonical pending Intent")?;
    let builder = build_moderated!(
        &state,
        buzz_sdk::build_meeting_v1_moderator_withdraw_self,
        buzz_sdk::build_meeting_v2_moderator_withdraw_self,
        buzz_sdk::MeetingV1ModeratorWithdrawSelfParams {
            session_id: meeting_id,
            attempt_id,
            intent_id,
            previous_event_id,
        },
    )
    .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let response = submit_v1_event(client, meeting_id, event).await?;
    println!("{}", normalize_write_response(&response));
    Ok(())
}

async fn cmd_moderator_recall(
    client: &CarryforthClient,
    meeting_id: &str,
    reason: Option<&str>,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
    let state = fetch_required_v1_baton(client, meeting_id).await?;
    let builder = build_moderated!(
        &state,
        buzz_sdk::build_meeting_v1_moderator_recall,
        buzz_sdk::build_meeting_v2_moderator_recall,
        buzz_sdk::MeetingV1ModeratorRecallParams {
            session_id: meeting_id,
            control_epoch: baton_u64(&state, "control_epoch")?,
            reason,
        }
    )
    .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let command_event_id = event.id.to_hex();
    let response = submit_v1_event(client, meeting_id, event).await?;
    print_v1_write_response(&response, "recall_event_id", &command_event_id);
    Ok(())
}

async fn cmd_human_floor_request(
    client: &CarryforthClient,
    meeting_id: &str,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
    let state = fetch_required_v1_baton(client, meeting_id).await?;
    let builder = build_moderated!(
        &state,
        buzz_sdk::build_meeting_v1_human_floor_request,
        buzz_sdk::build_meeting_v2_human_floor_request,
        buzz_sdk::MeetingV1HumanFloorRequestParams {
            session_id: meeting_id,
        },
    )
    .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let request_id = event.id.to_hex();
    let response = submit_v1_event(client, meeting_id, event).await?;
    print_v1_write_response(&response, "request_id", &request_id);
    Ok(())
}

async fn cmd_human_floor_withdraw(
    client: &CarryforthClient,
    meeting_id: &str,
    explicit_request_id: Option<&str>,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
    let state = fetch_required_v1_baton(client, meeting_id).await?;
    let self_pubkey = client.keys().public_key().to_hex();
    let request = if let Some(request_id) = explicit_request_id {
        validate_hex64(request_id)?;
        baton_object(
            &state,
            "human_queue",
            "request_id",
            &request_id.to_ascii_lowercase(),
        )?
    } else {
        baton_array(&state, "human_queue")?
            .iter()
            .find(|request| {
                request
                    .get("requester_pubkey")
                    .and_then(serde_json::Value::as_str)
                    == Some(self_pubkey.as_str())
            })
            .ok_or_else(|| {
                CliError::Conflict(format!(
                    "current identity has no active Human request in State {}",
                    state.state_event_id
                ))
            })?
    };
    let request_id = object_string(request, "request_id", "canonical Human request")?;
    if request
        .get("requester_pubkey")
        .and_then(serde_json::Value::as_str)
        != Some(self_pubkey.as_str())
    {
        return Err(CliError::Usage(
            "Human Floor Request belongs to another participant".into(),
        ));
    }
    let builder = build_moderated!(
        &state,
        buzz_sdk::build_meeting_v1_human_floor_withdraw,
        buzz_sdk::build_meeting_v2_human_floor_withdraw,
        buzz_sdk::MeetingV1HumanFloorWithdrawParams {
            session_id: meeting_id,
            request_id,
        },
    )
    .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let response = submit_v1_event(client, meeting_id, event).await?;
    print_v1_write_response(&response, "request_id", request_id);
    Ok(())
}

fn active_object_for_identity<'a>(
    state: &'a BatonState,
    field: &str,
    object_id_field: &str,
    explicit_id: Option<&str>,
    identity_field: &str,
    self_pubkey: &str,
) -> Result<(&'a serde_json::Value, &'a str), CliError> {
    let object = baton_active_object(state, field)?;
    let canonical_id = object_string(object, object_id_field, &format!("active {field}"))?;
    if let Some(explicit_id) = explicit_id {
        validate_hex64(explicit_id)?;
        if !canonical_id.eq_ignore_ascii_case(explicit_id) {
            return Err(CliError::Conflict(format!(
                "{object_id_field} {explicit_id} is stale; canonical {object_id_field} is {canonical_id}"
            )));
        }
    }
    if object
        .get(identity_field)
        .and_then(serde_json::Value::as_str)
        != Some(self_pubkey)
    {
        return Err(CliError::Usage(format!(
            "active {field} belongs to another participant"
        )));
    }
    Ok((object, canonical_id))
}

async fn cmd_offer_ack(
    client: &CarryforthClient,
    meeting_id: &str,
    offer_id: Option<&str>,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
    let state = fetch_required_v1_baton(client, meeting_id).await?;
    let self_pubkey = client.keys().public_key().to_hex();
    let (_, offer_id) = active_object_for_identity(
        &state,
        "offer",
        "offer_id",
        offer_id,
        "target_pubkey",
        &self_pubkey,
    )?;
    let builder = build_moderated!(
        &state,
        buzz_sdk::build_meeting_v1_offer_ack,
        buzz_sdk::build_meeting_v2_offer_ack,
        buzz_sdk::MeetingV1OfferAckParams {
            session_id: meeting_id,
            offer_id,
        }
    )
    .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let response = submit_v1_event(client, meeting_id, event).await?;
    print_v1_write_response(&response, "offer_id", offer_id);
    Ok(())
}

async fn cmd_offer_decline(
    client: &CarryforthClient,
    meeting_id: &str,
    offer_id: Option<&str>,
    reason: Option<&str>,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
    let state = fetch_required_v1_baton(client, meeting_id).await?;
    let self_pubkey = client.keys().public_key().to_hex();
    let (_, offer_id) = active_object_for_identity(
        &state,
        "offer",
        "offer_id",
        offer_id,
        "target_pubkey",
        &self_pubkey,
    )?;
    let builder = build_moderated!(
        &state,
        buzz_sdk::build_meeting_v1_offer_decline,
        buzz_sdk::build_meeting_v2_offer_decline,
        buzz_sdk::MeetingV1OfferDeclineParams {
            session_id: meeting_id,
            offer_id,
            reason,
        }
    )
    .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let response = submit_v1_event(client, meeting_id, event).await?;
    print_v1_write_response(&response, "offer_id", offer_id);
    Ok(())
}

fn sdk_progress_stage(stage: crate::MeetingGrantProgressStage) -> buzz_sdk::MeetingV1ProgressStage {
    match stage {
        crate::MeetingGrantProgressStage::ContextSync => {
            buzz_sdk::MeetingV1ProgressStage::ContextSync
        }
        crate::MeetingGrantProgressStage::ToolUse => buzz_sdk::MeetingV1ProgressStage::ToolUse,
        crate::MeetingGrantProgressStage::Generating => {
            buzz_sdk::MeetingV1ProgressStage::Generating
        }
        crate::MeetingGrantProgressStage::Composing => buzz_sdk::MeetingV1ProgressStage::Composing,
        crate::MeetingGrantProgressStage::Submitting => {
            buzz_sdk::MeetingV1ProgressStage::Submitting
        }
    }
}

fn sdk_yield_reason(reason: crate::MeetingGrantYieldReason) -> buzz_sdk::MeetingV1GrantYieldReason {
    match reason {
        crate::MeetingGrantYieldReason::NoLongerNeeded => {
            buzz_sdk::MeetingV1GrantYieldReason::NoLongerNeeded
        }
        crate::MeetingGrantYieldReason::UnableToAnswer => {
            buzz_sdk::MeetingV1GrantYieldReason::UnableToAnswer
        }
        crate::MeetingGrantYieldReason::InsufficientContext => {
            buzz_sdk::MeetingV1GrantYieldReason::InsufficientContext
        }
        crate::MeetingGrantYieldReason::ToolFailure => {
            buzz_sdk::MeetingV1GrantYieldReason::ToolFailure
        }
        crate::MeetingGrantYieldReason::Cancelled => buzz_sdk::MeetingV1GrantYieldReason::Cancelled,
    }
}

async fn cmd_grant_progress(
    client: &CarryforthClient,
    meeting_id: &str,
    grant_id: Option<&str>,
    stage: crate::MeetingGrantProgressStage,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
    let state = fetch_required_v1_baton(client, meeting_id).await?;
    let self_pubkey = client.keys().public_key().to_hex();
    let (grant, grant_id) = active_object_for_identity(
        &state,
        "grant",
        "grant_id",
        grant_id,
        "holder_pubkey",
        &self_pubkey,
    )?;
    let progress_seq = grant
        .get("progress_seq")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| CliError::Other("canonical Grant has no valid progress_seq".into()))?
        .checked_add(1)
        .ok_or_else(|| CliError::Other("Grant progress sequence overflow".into()))?;
    let builder = build_moderated!(
        &state,
        buzz_sdk::build_meeting_v1_grant_progress,
        buzz_sdk::build_meeting_v2_grant_progress,
        buzz_sdk::MeetingV1GrantProgressParams {
            session_id: meeting_id,
            grant_id,
            progress_seq,
            stage: sdk_progress_stage(stage),
        }
    )
    .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let response = submit_v1_event(client, meeting_id, event).await?;
    print_v1_write_response(&response, "grant_id", grant_id);
    Ok(())
}

async fn cmd_grant_yield(
    client: &CarryforthClient,
    meeting_id: &str,
    grant_id: Option<&str>,
    reason_code: Option<crate::MeetingGrantYieldReason>,
    reason: Option<&str>,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
    let state = fetch_required_v1_baton(client, meeting_id).await?;
    let self_pubkey = client.keys().public_key().to_hex();
    let (_, grant_id) = active_object_for_identity(
        &state,
        "grant",
        "grant_id",
        grant_id,
        "holder_pubkey",
        &self_pubkey,
    )?;
    let builder = build_moderated!(
        &state,
        buzz_sdk::build_meeting_v1_grant_yield,
        buzz_sdk::build_meeting_v2_grant_yield,
        buzz_sdk::MeetingV1GrantYieldParams {
            session_id: meeting_id,
            grant_id,
            reason_code: reason_code.map(sdk_yield_reason),
            reason,
        }
    )
    .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let response = submit_v1_event(client, meeting_id, event).await?;
    print_v1_write_response(&response, "grant_id", grant_id);
    Ok(())
}

fn sdk_handoff_type(handoff_type: crate::MeetingHandoffType) -> buzz_sdk::MeetingV1HandoffType {
    match handoff_type {
        crate::MeetingHandoffType::Question => buzz_sdk::MeetingV1HandoffType::Question,
        crate::MeetingHandoffType::InformationRequest => {
            buzz_sdk::MeetingV1HandoffType::InformationRequest
        }
        crate::MeetingHandoffType::Clarification => buzz_sdk::MeetingV1HandoffType::Clarification,
        crate::MeetingHandoffType::Review => buzz_sdk::MeetingV1HandoffType::Review,
        crate::MeetingHandoffType::ResponseRequested => {
            buzz_sdk::MeetingV1HandoffType::ResponseRequested
        }
    }
}

async fn current_open_floor(
    client: &CarryforthClient,
    meeting_id: Uuid,
    operation: &str,
) -> Result<FloorState, CliError> {
    require_uniform_v0(client, meeting_id).await?;
    let state = fetch_current_floor(client, &meeting_id.to_string())
        .await?
        .ok_or_else(|| CliError::NotFound(format!("meeting floor not found: {meeting_id}")))?;
    if !matches!(state.phase.as_str(), "open" | "claiming") {
        return Err(CliError::Usage(format!(
            "meeting round {} is {}; {operation} is only allowed while open or claiming",
            state.round_number, state.phase
        )));
    }
    Ok(state)
}

pub async fn cmd_floor_ready(
    client: &CarryforthClient,
    meeting_id: &str,
    intent_basis: &str,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
    let state = current_open_floor(client, meeting_id, "Ready").await?;
    let builder = buzz_sdk::build_meeting_floor_ready(meeting_id, state.round_number, intent_basis)
        .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let response = client.submit_event(event).await?;
    println!("{}", normalize_write_response(&response));
    Ok(())
}

pub async fn cmd_floor_pass(
    client: &CarryforthClient,
    meeting_id: &str,
    intent_basis: &str,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
    let state = current_open_floor(client, meeting_id, "Pass").await?;
    let builder = buzz_sdk::build_meeting_floor_pass(meeting_id, state.round_number, intent_basis)
        .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let response = client.submit_event(event).await?;
    println!("{}", normalize_write_response(&response));
    Ok(())
}

pub async fn cmd_floor_yield(client: &CarryforthClient, meeting_id: &str) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
    require_uniform_v0(client, meeting_id).await?;
    let state = fetch_current_floor(client, &meeting_id.to_string())
        .await?
        .ok_or_else(|| CliError::NotFound(format!("meeting floor not found: {meeting_id}")))?;
    if state.phase != "granted" {
        return Err(CliError::Usage(format!(
            "meeting round {} is {}; Yield requires an active Grant",
            state.round_number, state.phase
        )));
    }
    let self_pubkey = client.keys().public_key().to_hex();
    if state.holder_pubkey.as_deref() != Some(self_pubkey.as_str()) {
        return Err(CliError::Usage(format!(
            "current Grant belongs to {}, not the current identity",
            state.holder_pubkey.as_deref().unwrap_or("unknown")
        )));
    }
    let grant_event_id = state
        .grant_event_id
        .as_deref()
        .ok_or_else(|| CliError::Other("granted floor state has no Grant event ID".into()))?;
    let builder =
        buzz_sdk::build_meeting_floor_yield(meeting_id, state.round_number, grant_event_id)
            .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let response = client.submit_event(event).await?;
    println!("{}", normalize_write_response(&response));
    Ok(())
}

pub async fn cmd_claim_floor(
    client: &CarryforthClient,
    meeting_id: &str,
    wait: bool,
    timeout_secs: u64,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
    let meeting_id_text = meeting_id.to_string();
    require_uniform_v0(client, meeting_id).await?;
    let state = fetch_current_floor(client, &meeting_id_text)
        .await?
        .ok_or_else(|| CliError::NotFound(format!("meeting floor not found: {meeting_id}")))?;
    if !matches!(state.phase.as_str(), "open" | "claiming") {
        return Err(CliError::Usage(format!(
            "meeting round {} is {}; Claim is only allowed while open or claiming",
            state.round_number, state.phase
        )));
    }
    let self_pubkey = client.keys().public_key().to_hex();
    if let Some(index) = state
        .claimant_pubkeys
        .iter()
        .position(|claimant| claimant == &self_pubkey)
    {
        let canonical = state
            .claim_event_ids
            .get(index)
            .cloned()
            .unwrap_or_default();
        return Err(CliError::Conflict(format!(
            "already claimed round {} with event {}",
            state.round_number, canonical
        )));
    }

    let builder =
        buzz_sdk::build_meeting_floor_claim(meeting_id, state.round_number).map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let claim_event_id = event.id.to_hex();
    let write_response = client.submit_event(event).await?;
    if !wait {
        println!("{}", normalize_write_response(&write_response));
        return Ok(());
    }
    if timeout_secs == 0 {
        return Err(CliError::Usage("--timeout must be positive".into()));
    }

    let deadline =
        tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs.min(300));
    loop {
        let states = fetch_floor_states(client, &meeting_id_text, 2000).await?;
        let latest = states
            .iter()
            .filter(|candidate| candidate.round_number == state.round_number)
            .max_by_key(|candidate| candidate.floor_revision);
        if latest.is_some_and(|candidate| {
            candidate.phase == "closed" && candidate.outcome.as_deref() == Some("ended")
        }) {
            println!(
                "{}",
                serde_json::json!({
                    "meeting_id": meeting_id_text,
                    "round_number": state.round_number,
                    "claim_event_id": claim_event_id,
                    "result": "ended",
                    "write": serde_json::from_str::<serde_json::Value>(
                        &normalize_write_response(&write_response)
                    ).unwrap_or(serde_json::Value::String(write_response.clone())),
                })
            );
            return Ok(());
        }
        let grant = states
            .iter()
            .filter(|candidate| {
                candidate.round_number == state.round_number && candidate.phase == "granted"
            })
            .max_by_key(|candidate| candidate.floor_revision);
        if let (Some(latest), Some(grant)) = (latest, grant) {
            if !matches!(latest.phase.as_str(), "granted" | "closed") {
                continue;
            }
            let result = if grant.holder_pubkey.as_deref() == Some(self_pubkey.as_str()) {
                "won"
            } else {
                "lost"
            };
            println!(
                "{}",
                serde_json::json!({
                    "meeting_id": meeting_id_text,
                    "round_number": state.round_number,
                    "claim_event_id": claim_event_id,
                    "result": result,
                    "holder_pubkey": grant.holder_pubkey,
                    "grant_event_id": grant.grant_event_id,
                    "lease_expires_at_ms": grant.lease_expires_at_ms,
                    "active": latest.phase == "granted",
                    "settled_phase": latest.phase,
                    "outcome": latest.outcome,
                    "write": serde_json::from_str::<serde_json::Value>(
                        &normalize_write_response(&write_response)
                    ).unwrap_or(serde_json::Value::String(write_response.clone())),
                })
            );
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(CliError::Other(format!(
                "timed out waiting for round {} arbitration",
                state.round_number
            )));
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

pub async fn cmd_say_meeting(
    client: &CarryforthClient,
    meeting_id: &str,
    content: &str,
    mentions: &[String],
    handoff_to: Option<&str>,
    handoff_type: Option<crate::MeetingHandoffType>,
    handoff_reason: Option<&str>,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
    let content = read_or_stdin(content)?;
    let mention_refs: Vec<&str> = mentions.iter().map(String::as_str).collect();
    match fetch_meeting_protocol(client, &meeting_id.to_string()).await? {
        MeetingProtocol::UniformV0 => {
            if handoff_to.is_some() || handoff_type.is_some() || handoff_reason.is_some() {
                return Err(CliError::Usage(
                    "directed handoff requires a moderated-baton-v1 meeting".into(),
                ));
            }
            let state = fetch_current_floor(client, &meeting_id.to_string())
                .await?
                .ok_or_else(|| {
                    CliError::NotFound(format!("meeting floor not found: {meeting_id}"))
                })?;
            if state.phase != "granted" {
                return Err(CliError::Usage(format!(
                    "meeting round {} is {}; no active Grant can be used",
                    state.round_number, state.phase
                )));
            }
            let self_pubkey = client.keys().public_key().to_hex();
            if state.holder_pubkey.as_deref() != Some(self_pubkey.as_str()) {
                return Err(CliError::Usage(format!(
                    "current Grant belongs to {}, not the current identity",
                    state.holder_pubkey.as_deref().unwrap_or("unknown")
                )));
            }
            let grant_event_id = state.grant_event_id.as_deref().ok_or_else(|| {
                CliError::Other("granted floor state has no Grant event ID".into())
            })?;
            let builder = buzz_sdk::build_meeting_speech(
                meeting_id,
                state.round_number,
                grant_event_id,
                &content,
                &mention_refs,
            )
            .map_err(sdk_err)?;
            let event = client.sign_event(builder)?;
            let response = client.submit_event(event).await?;
            println!("{}", normalize_write_response(&response));
            Ok(())
        }
        MeetingProtocol::ModeratedBatonV1
        | MeetingProtocol::ModeratedBoardV2
        | MeetingProtocol::ModeratedBoardActionsV2 => {
            let handoff = match (handoff_to, handoff_type, handoff_reason) {
                (None, None, None) => None,
                (Some(target_pubkey), Some(handoff_type), Some(reason)) => {
                    Some(buzz_sdk::MeetingV1DirectedHandoff {
                        target_pubkey,
                        handoff_type: sdk_handoff_type(handoff_type),
                        reason,
                    })
                }
                _ => {
                    return Err(CliError::Usage(
                        "--handoff-to, --handoff-type, and --handoff-reason must be supplied together"
                            .into(),
                    ));
                }
            };
            let state = fetch_required_v1_baton(client, meeting_id).await?;
            let self_pubkey = client.keys().public_key().to_hex();
            let (_, grant_id) = active_object_for_identity(
                &state,
                "grant",
                "grant_id",
                None,
                "holder_pubkey",
                &self_pubkey,
            )?;
            let speech_revision = state
                .speech_revision
                .checked_add(1)
                .ok_or_else(|| CliError::Other("speech revision overflow".into()))?;
            let builder = build_moderated!(
                &state,
                buzz_sdk::build_meeting_v1_speech,
                buzz_sdk::build_meeting_v2_speech,
                buzz_sdk::MeetingV1SpeechParams {
                    session_id: meeting_id,
                    grant_id,
                    speech_revision,
                    content: &content,
                    mentions: &mention_refs,
                    handoff,
                }
            )
            .map_err(sdk_err)?;
            let event = client.sign_event(builder)?;
            let speech_event_id = event.id.to_hex();
            let response = submit_v1_event(client, meeting_id, event).await?;
            print_v1_write_response(&response, "speech_event_id", &speech_event_id);
            Ok(())
        }
        MeetingProtocol::ModeratedBoardActionsV2Legacy => Err(CliError::Usage(
            "ended moderated-board-actions-v2 Meetings are read-only history".into(),
        )),
    }
}

async fn submit_meeting_v2_end(
    client: &CarryforthClient,
    meeting_id: &str,
    outcome: buzz_sdk::MeetingV2EndOutcome,
    reason_code: Option<&str>,
    reason: Option<&str>,
    confirm_actions_recorded: bool,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
    let create = fetch_meeting_create(client, &meeting_id.to_string()).await?;
    let protocol = MeetingProtocol::from_create_event(&create)?;
    if !protocol.is_v2() {
        return Err(CliError::Usage(format!(
            "meeting {meeting_id} does not use a Meeting V2 policy"
        )));
    }
    let create_event_id = create
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| CliError::Other(format!("meeting not found: {meeting_id}")))?;
    struct OwnedActionEndFence {
        action_run_id: Uuid,
        action_window: u64,
        board_event_id: String,
    }
    let action_fence = if protocol.has_action_finalization()
        && outcome == buzz_sdk::MeetingV2EndOutcome::Closed
    {
        let state = fetch_required_v1_baton(client, meeting_id).await?;
        let board_control = state
            .content
            .get("board_control")
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| CliError::Other("Meeting State has no board_control".into()))?;
        if board_control
            .get("phase")
            .and_then(serde_json::Value::as_str)
            == Some("finalizing_actions")
        {
            if !confirm_actions_recorded {
                return Err(CliError::Conflict(
                    "Meeting is finalizing actions; use `meetings actions confirm-recorded` to attest completion and close"
                        .into(),
                ));
            }
            let action = board_control
                .get("action")
                .and_then(serde_json::Value::as_object)
                .ok_or_else(|| CliError::Conflict("Meeting has no active action run".into()))?;
            if action.get("condition").and_then(serde_json::Value::as_str) != Some("runnable") {
                return Err(CliError::Conflict(
                    "Meeting action run is blocked and cannot be confirmed".into(),
                ));
            }
            let action_run_id = action
                .get("action_run_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| CliError::Other("action run has no valid ID".into()))?;
            let action_run_id = parse_uuid(action_run_id)?;
            let action_window = action
                .get("action_window_epoch")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| CliError::Other("action run has no valid window".into()))?;
            let board_event_id = action
                .get("board_event_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| CliError::Other("action run has no frozen Board".into()))?;
            validate_hex64(board_event_id)?;
            Some(OwnedActionEndFence {
                action_run_id,
                action_window,
                board_event_id: board_event_id.to_string(),
            })
        } else {
            if confirm_actions_recorded {
                return Err(CliError::Conflict(
                    "Meeting is not finalizing actions".into(),
                ));
            }
            None
        }
    } else {
        if confirm_actions_recorded {
            return Err(CliError::Usage(format!(
                "meeting {meeting_id} does not use direct action finalization"
            )));
        }
        None
    };
    let builder = if protocol.has_action_finalization() {
        buzz_sdk::build_meeting_v2_actions_end(buzz_sdk::MeetingV2ActionsEndParams {
            session_id: meeting_id,
            create_event_id,
            outcome,
            reason_code,
            reason,
            action_fence: action_fence
                .as_ref()
                .map(|fence| buzz_sdk::MeetingV2ActionsEndFence {
                    action_run_id: fence.action_run_id,
                    action_window: fence.action_window,
                    board_event_id: &fence.board_event_id,
                }),
        })
    } else {
        buzz_sdk::build_meeting_v2_end(buzz_sdk::MeetingV2EndParams {
            session_id: meeting_id,
            create_event_id,
            outcome,
            reason_code,
            reason,
        })
    }
    .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let end_event_id = event.id.to_hex();
    let response = client.submit_event(event).await?;
    print_v1_write_response(&response, "end_event_id", &end_event_id);
    Ok(())
}

pub async fn cmd_close_meeting_v2(
    client: &CarryforthClient,
    meeting_id: &str,
) -> Result<(), CliError> {
    submit_meeting_v2_end(
        client,
        meeting_id,
        buzz_sdk::MeetingV2EndOutcome::Closed,
        None,
        None,
        false,
    )
    .await
}

pub async fn cmd_meeting_actions_confirm_recorded(
    client: &CarryforthClient,
    meeting_id: &str,
) -> Result<(), CliError> {
    submit_meeting_v2_end(
        client,
        meeting_id,
        buzz_sdk::MeetingV2EndOutcome::Closed,
        None,
        None,
        true,
    )
    .await
}

pub async fn cmd_abort_meeting_v2(
    client: &CarryforthClient,
    meeting_id: &str,
    reason_code: &str,
    reason: Option<&str>,
) -> Result<(), CliError> {
    submit_meeting_v2_end(
        client,
        meeting_id,
        buzz_sdk::MeetingV2EndOutcome::Aborted,
        Some(reason_code),
        reason,
        false,
    )
    .await
}

pub async fn cmd_end_meeting(client: &CarryforthClient, meeting_id: &str) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
    let meeting_id_text = meeting_id.to_string();
    let create = fetch_meeting_create(client, &meeting_id_text).await?;
    let protocol = MeetingProtocol::from_create_event(&create)?;
    let create_event_id = create
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| CliError::Other(format!("meeting not found: {meeting_id}")))?;

    let builder = match protocol {
        MeetingProtocol::UniformV0 => buzz_sdk::build_meeting_end(meeting_id, create_event_id),
        MeetingProtocol::ModeratedBatonV1 => {
            buzz_sdk::build_meeting_v1_end(buzz_sdk::MeetingV1EndParams {
                session_id: meeting_id,
                create_event_id,
            })
        }
        MeetingProtocol::ModeratedBoardV2 | MeetingProtocol::ModeratedBoardActionsV2 => {
            return cmd_close_meeting_v2(client, &meeting_id_text).await;
        }
        MeetingProtocol::ModeratedBoardActionsV2Legacy => {
            return Err(CliError::Usage(
                "ended moderated-board-actions-v2 Meetings are read-only history".into(),
            ));
        }
    }
    .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let response = client.submit_event(event).await?;
    println!("{}", normalize_write_response(&response));
    Ok(())
}

fn event_round(event: &serde_json::Value) -> u64 {
    extract_tag_value(event, "meeting-round")
        .parse()
        .unwrap_or(u64::MAX)
}

fn event_floor_revision(event: &serde_json::Value) -> u64 {
    extract_tag_value(event, "floor-revision")
        .parse()
        .unwrap_or(u64::MAX)
}

fn event_speech_revision(event: &serde_json::Value) -> u64 {
    extract_tag_value(event, "speech-revision")
        .parse()
        .unwrap_or(u64::MAX)
}

fn event_id(event: &serde_json::Value) -> &str {
    event
        .get("id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
}

pub async fn dispatch(
    command: crate::MeetingsCmd,
    client: &CarryforthClient,
    format: &OutputFormat,
) -> Result<(), CliError> {
    use crate::MeetingsCmd;
    match command {
        MeetingsCmd::Create {
            title,
            description,
            source,
            policy,
            moderator,
            board,
            participants,
        } => {
            cmd_create_meeting(
                client,
                CreateMeetingInput {
                    title: &title,
                    description: description.as_deref(),
                    source: source.as_deref(),
                    policy,
                    moderator: moderator.as_deref(),
                    board: board.as_deref(),
                    participant_pubkeys: &participants,
                },
            )
            .await
        }
        MeetingsCmd::List {
            include_ended,
            limit,
        } => cmd_list_meetings(client, include_ended, limit, format).await,
        MeetingsCmd::Show { meeting } => cmd_show_meeting(client, &meeting).await,
        MeetingsCmd::Update {
            meeting,
            summary,
            clear_summary,
        } => cmd_update_meeting_summary(client, &meeting, summary.as_deref(), clear_summary).await,
        MeetingsCmd::Board { command } => {
            use crate::MeetingBoardCmd;
            match command {
                MeetingBoardCmd::Get { meeting } => cmd_get_meeting_board(client, &meeting).await,
                MeetingBoardCmd::Update {
                    meeting,
                    board,
                    control_epoch,
                    board_window,
                } => {
                    cmd_meeting_board_action(
                        client,
                        &meeting,
                        Some(&board),
                        control_epoch,
                        board_window,
                    )
                    .await
                }
                MeetingBoardCmd::Unchanged {
                    meeting,
                    control_epoch,
                    board_window,
                } => {
                    cmd_meeting_board_action(client, &meeting, None, control_epoch, board_window)
                        .await
                }
            }
        }
        MeetingsCmd::Actions { command } => {
            use crate::MeetingActionsCmd;
            match command {
                MeetingActionsCmd::Status { meeting } => {
                    cmd_meeting_actions_status(client, &meeting).await
                }
                MeetingActionsCmd::Begin { meeting } => {
                    cmd_meeting_actions_begin(client, &meeting).await
                }
                MeetingActionsCmd::Block {
                    meeting,
                    reason_code,
                    reason,
                } => {
                    cmd_meeting_actions_block(client, &meeting, &reason_code, reason.as_deref())
                        .await
                }
                MeetingActionsCmd::Retry { meeting } => {
                    cmd_meeting_actions_retry(client, &meeting).await
                }
                MeetingActionsCmd::ConfirmRecorded { meeting } => {
                    cmd_meeting_actions_confirm_recorded(client, &meeting).await
                }
                MeetingActionsCmd::ReturnToBoard { meeting } => {
                    cmd_meeting_actions_return_to_board(client, &meeting).await
                }
            }
        }
        MeetingsCmd::Participants { meeting } => cmd_list_participants(client, &meeting).await,
        MeetingsCmd::History { meeting, limit } => {
            cmd_meeting_history(client, &meeting, limit, format).await
        }
        MeetingsCmd::Say {
            meeting,
            content,
            mentions,
            handoff_to,
            handoff_type,
            handoff_reason,
        } => {
            cmd_say_meeting(
                client,
                &meeting,
                &content,
                &mentions,
                handoff_to.as_deref(),
                handoff_type,
                handoff_reason.as_deref(),
            )
            .await
        }
        MeetingsCmd::Intents { command } => {
            use crate::MeetingIntentsCmd;
            match command {
                MeetingIntentsCmd::List { meeting } => {
                    cmd_intents_list(client, &meeting, format).await
                }
                MeetingIntentsCmd::Submit {
                    meeting,
                    summary,
                    addressed_to,
                } => cmd_intent_submit(client, &meeting, &summary, addressed_to.as_deref()).await,
                MeetingIntentsCmd::Refresh {
                    meeting,
                    intent,
                    summary,
                    addressed_to,
                } => {
                    cmd_intent_refresh(client, &meeting, &intent, &summary, addressed_to.as_deref())
                        .await
                }
                MeetingIntentsCmd::Withdraw { meeting, intent } => {
                    cmd_intent_withdraw(client, &meeting, &intent).await
                }
            }
        }
        MeetingsCmd::Moderator { command } => {
            use crate::MeetingModeratorCmd;
            match command {
                MeetingModeratorCmd::Select {
                    meeting,
                    intent,
                    handoff,
                    reason,
                    deferrals,
                    attempt,
                } => {
                    cmd_moderator_select(
                        client,
                        &meeting,
                        intent.as_deref(),
                        handoff.as_deref(),
                        reason.as_deref(),
                        &deferrals,
                        attempt.as_deref(),
                    )
                    .await
                }
                MeetingModeratorCmd::Reject {
                    meeting,
                    intent,
                    reason_code,
                    reason,
                    attempt,
                } => {
                    cmd_moderator_reject(
                        client,
                        &meeting,
                        &intent,
                        reason_code,
                        &reason,
                        attempt.as_deref(),
                    )
                    .await
                }
                MeetingModeratorCmd::DismissHandoff {
                    meeting,
                    handoff,
                    reason_code,
                    reason,
                    attempt,
                } => {
                    cmd_moderator_dismiss_handoff(
                        client,
                        &meeting,
                        &handoff,
                        reason_code,
                        &reason,
                        attempt.as_deref(),
                    )
                    .await
                }
                MeetingModeratorCmd::AttemptStart {
                    meeting,
                    replacement,
                } => cmd_moderator_attempt_start(client, &meeting, replacement.as_deref()).await,
                MeetingModeratorCmd::AttemptFinish {
                    meeting,
                    attempt,
                    outcome,
                    reason_code,
                } => {
                    cmd_moderator_attempt_finish(client, &meeting, &attempt, outcome, &reason_code)
                        .await
                }
                MeetingModeratorCmd::Retry {
                    meeting,
                    attempt,
                    ticket,
                    failed_action,
                    attempt_number,
                } => {
                    cmd_moderator_retry(
                        client,
                        &meeting,
                        &attempt,
                        &ticket,
                        &failed_action,
                        attempt_number,
                    )
                    .await
                }
                MeetingModeratorCmd::CompleteCohort { meeting, attempt } => {
                    cmd_moderator_complete_cohort(client, &meeting, &attempt).await
                }
                MeetingModeratorCmd::AttemptAbandon { meeting, attempt } => {
                    cmd_moderator_attempt_abandon(client, &meeting, &attempt).await
                }
                MeetingModeratorCmd::WithdrawSelf {
                    meeting,
                    attempt,
                    intent,
                } => cmd_moderator_withdraw_self(client, &meeting, &attempt, &intent).await,
                MeetingModeratorCmd::Recall { meeting, reason } => {
                    cmd_moderator_recall(client, &meeting, reason.as_deref()).await
                }
            }
        }
        MeetingsCmd::Offer { command } => {
            use crate::MeetingOfferCmd;
            match command {
                MeetingOfferCmd::Ack { meeting, offer } => {
                    cmd_offer_ack(client, &meeting, offer.as_deref()).await
                }
                MeetingOfferCmd::Decline {
                    meeting,
                    offer,
                    reason,
                } => cmd_offer_decline(client, &meeting, offer.as_deref(), reason.as_deref()).await,
            }
        }
        MeetingsCmd::Grant { command } => {
            use crate::MeetingGrantCmd;
            match command {
                MeetingGrantCmd::Progress {
                    meeting,
                    stage,
                    grant,
                } => cmd_grant_progress(client, &meeting, grant.as_deref(), stage).await,
                MeetingGrantCmd::Yield {
                    meeting,
                    grant,
                    reason_code,
                    reason,
                } => {
                    cmd_grant_yield(
                        client,
                        &meeting,
                        grant.as_deref(),
                        reason_code,
                        reason.as_deref(),
                    )
                    .await
                }
            }
        }
        MeetingsCmd::Floor { command } => {
            use crate::MeetingFloorCmd;
            match command {
                MeetingFloorCmd::Status { meeting } => cmd_floor_status(client, &meeting).await,
                MeetingFloorCmd::History { meeting, limit } => {
                    cmd_floor_history(client, &meeting, limit, format).await
                }
                MeetingFloorCmd::Request { meeting } => {
                    cmd_human_floor_request(client, &meeting).await
                }
                MeetingFloorCmd::Withdraw { meeting, request } => {
                    cmd_human_floor_withdraw(client, &meeting, request.as_deref()).await
                }
                MeetingFloorCmd::Claim {
                    meeting,
                    wait,
                    timeout,
                } => cmd_claim_floor(client, &meeting, wait, timeout).await,
                MeetingFloorCmd::Ready { meeting, basis } => {
                    cmd_floor_ready(client, &meeting, &basis).await
                }
                MeetingFloorCmd::Pass { meeting, basis } => {
                    cmd_floor_pass(client, &meeting, &basis).await
                }
                MeetingFloorCmd::Yield { meeting } => cmd_floor_yield(client, &meeting).await,
            }
        }
        MeetingsCmd::End { meeting } => cmd_end_meeting(client, &meeting).await,
        MeetingsCmd::Close { meeting } => cmd_close_meeting_v2(client, &meeting).await,
        MeetingsCmd::Abort {
            meeting,
            reason_code,
            reason,
        } => cmd_abort_meeting_v2(client, &meeting, &reason_code, reason.as_deref()).await,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::extract::State;
    use axum::routing::post;
    use axum::{Json, Router};
    use nostr::{Event, EventBuilder, Keys, Kind, Tag};
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;

    use super::*;

    #[derive(Clone)]
    struct StageOneCliServerState {
        create: Arc<Mutex<Option<Event>>>,
        query_kinds: Arc<Mutex<Vec<u32>>>,
        relay_keys: Arc<Keys>,
    }

    async fn stage_one_cli_events(
        State(state): State<StageOneCliServerState>,
        Json(event): Json<Event>,
    ) -> Json<serde_json::Value> {
        assert_eq!(
            event.kind.as_u16() as u32,
            buzz_sdk::kind::KIND_MEETING_CREATE
        );
        assert_eq!(extract_tag_value(&serde_json::json!(event), "v"), "3");
        assert_eq!(
            extract_tag_value(&serde_json::json!(event), "policy"),
            buzz_sdk::MEETING_V2_POLICY
        );
        assert!(extract_tag_value(&serde_json::json!(event), "moderator").is_empty());
        buzz_sdk::parse_meeting_v2_board_content(&event.content)
            .expect("CLI submits strict Meeting V2 board content");

        let event_id = event.id.to_hex();
        *state.create.lock().await = Some(event);
        Json(serde_json::json!({
            "event_id": event_id,
            "accepted": true,
            "message": "accepted"
        }))
    }

    async fn stage_one_cli_query(
        State(state): State<StageOneCliServerState>,
        Json(filters): Json<Vec<serde_json::Value>>,
    ) -> Json<serde_json::Value> {
        let kind = filters
            .first()
            .and_then(|filter| filter.get("kinds"))
            .and_then(serde_json::Value::as_array)
            .and_then(|kinds| kinds.first())
            .and_then(serde_json::Value::as_u64)
            .map(|kind| kind as u32)
            .expect("CLI query carries an explicit kind");
        state.query_kinds.lock().await.push(kind);
        let create = state
            .create
            .lock()
            .await
            .clone()
            .expect("CLI Create precedes board reads");

        let events = if kind == buzz_sdk::kind::KIND_MEETING_CREATE {
            vec![create]
        } else if kind == KIND_MEETING_BOARD {
            let create_json = serde_json::json!(create);
            let session_id = extract_tag_value(&create_json, "h");
            let moderator = create_json["pubkey"]
                .as_str()
                .expect("Create author pubkey");
            let board = EventBuilder::new(
                Kind::Custom(KIND_MEETING_BOARD as u16),
                create_json["content"]
                    .as_str()
                    .expect("Create board content"),
            )
            .tags([
                Tag::parse(["h", session_id.as_str()]).expect("board h tag"),
                Tag::parse(["v", buzz_sdk::MEETING_V2_SCHEMA_VERSION]).expect("board version tag"),
                Tag::parse(["policy", buzz_sdk::MEETING_V2_POLICY]).expect("board policy tag"),
                Tag::parse(["format", buzz_sdk::MEETING_V2_BOARD_FORMAT])
                    .expect("board format tag"),
                Tag::parse(["moderator", moderator]).expect("board moderator tag"),
            ])
            .sign_with_keys(&state.relay_keys)
            .expect("sign board fixture");
            vec![board]
        } else {
            Vec::new()
        };
        Json(serde_json::to_value(events).expect("serialize CLI query fixture"))
    }

    async fn spawn_stage_one_cli_server() -> (String, StageOneCliServerState) {
        let state = StageOneCliServerState {
            create: Arc::new(Mutex::new(None)),
            query_kinds: Arc::new(Mutex::new(Vec::new())),
            relay_keys: Arc::new(Keys::generate()),
        };
        let app = Router::new()
            .route("/events", post(stage_one_cli_events))
            .route("/query", post(stage_one_cli_query))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind Meeting V2 CLI test server");
        let address = listener.local_addr().expect("CLI test server address");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve Meeting V2 CLI fixture");
        });
        (format!("http://{address}"), state)
    }

    fn create_event(version: &str, policy: Option<&str>) -> serde_json::Value {
        let mut tags = vec![
            serde_json::json!(["h", "00000000-0000-4000-8000-000000000001"]),
            serde_json::json!(["v", version]),
        ];
        if let Some(policy) = policy {
            tags.push(serde_json::json!(["policy", policy]));
        }
        serde_json::json!({
            "id": "aa".repeat(32),
            "kind": buzz_sdk::kind::KIND_MEETING_CREATE,
            "tags": tags,
        })
    }

    #[test]
    fn create_protocol_is_strict_for_v1_v2_and_compatible_for_v0() {
        assert_eq!(
            MeetingProtocol::from_create_event(&create_event("1", None)).unwrap(),
            MeetingProtocol::UniformV0
        );
        assert_eq!(
            MeetingProtocol::from_create_event(&create_event(
                "2",
                Some(buzz_sdk::MEETING_V1_POLICY)
            ))
            .unwrap(),
            MeetingProtocol::ModeratedBatonV1
        );
        assert!(MeetingProtocol::from_create_event(&create_event("2", None)).is_err());
        assert!(
            MeetingProtocol::from_create_event(&create_event("2", Some("uniform-v0"))).is_err()
        );
        assert_eq!(
            MeetingProtocol::from_create_event(&create_event(
                "3",
                Some(buzz_sdk::MEETING_V2_POLICY)
            ))
            .unwrap(),
            MeetingProtocol::ModeratedBoardV2
        );
        assert_eq!(
            MeetingProtocol::from_create_event(&create_event(
                "3",
                Some(buzz_sdk::MEETING_V2_ACTIONS_POLICY)
            ))
            .unwrap(),
            MeetingProtocol::ModeratedBoardActionsV2
        );
        assert!(MeetingProtocol::from_create_event(&create_event("3", None)).is_err());
        assert!(MeetingProtocol::from_create_event(&create_event(
            "3",
            Some(buzz_sdk::MEETING_V1_POLICY)
        ))
        .is_err());
    }

    #[test]
    fn meeting_metadata_summary_requires_the_advertised_relay_signature() {
        let meeting_id = Uuid::new_v4();
        let relay = Keys::generate();
        let event = EventBuilder::new(Kind::Custom(KIND_GROUP_METADATA as u16), "")
            .tags([
                Tag::parse(["d", &meeting_id.to_string()]).expect("d"),
                Tag::parse(["name", "Summary review"]).expect("name"),
                Tag::parse(["room_kind", "meeting"]).expect("room kind"),
                Tag::parse(["summary", "Decision and verified outputs."]).expect("summary"),
            ])
            .sign_with_keys(&relay)
            .expect("sign metadata");
        let parsed = verified_meeting_summary(
            serde_json::to_value(&event).expect("serialize metadata"),
            &relay.public_key(),
        )
        .expect("verify metadata")
        .expect("Meeting metadata");
        assert_eq!(parsed.meeting_id, meeting_id.to_string());
        assert_eq!(
            parsed.summary.as_deref(),
            Some("Decision and verified outputs.")
        );

        assert!(verified_meeting_summary(
            serde_json::to_value(&event).expect("serialize metadata"),
            &Keys::generate().public_key()
        )
        .is_err());
    }

    #[tokio::test]
    async fn full_cli_create_then_get_board_uses_the_stage_one_wire_contract() {
        let (relay, state) = spawn_stage_one_cli_server().await;
        let private_key = "0000000000000000000000000000000000000000000000000000000000000001";
        let participant = Keys::generate().public_key().to_hex();
        let create_exit = crate::run_from_args(vec![
            "cf".to_string(),
            "--relay".to_string(),
            relay.clone(),
            "--private-key".to_string(),
            private_key.to_string(),
            "--auth-tag".to_string(),
            String::new(),
            "meetings".to_string(),
            "create".to_string(),
            "--policy".to_string(),
            buzz_sdk::MEETING_V2_POLICY.to_string(),
            "--title".to_string(),
            "CLI stage one".to_string(),
            "--board".to_string(),
            "# Goal\nVerify the CLI path.".to_string(),
            "--participant".to_string(),
            participant,
        ])
        .await;
        assert_eq!(create_exit, 0);

        let create = state
            .create
            .lock()
            .await
            .clone()
            .expect("CLI submitted Meeting V2 Create");
        let meeting_id = extract_tag_value(&serde_json::json!(create), "h");
        let board_exit = crate::run_from_args(vec![
            "cf".to_string(),
            "--relay".to_string(),
            relay,
            "--private-key".to_string(),
            private_key.to_string(),
            "--auth-tag".to_string(),
            String::new(),
            "meetings".to_string(),
            "board".to_string(),
            "get".to_string(),
            "--meeting".to_string(),
            meeting_id,
        ])
        .await;
        assert_eq!(board_exit, 0);
        assert_eq!(
            *state.query_kinds.lock().await,
            vec![buzz_sdk::kind::KIND_MEETING_CREATE, KIND_MEETING_BOARD]
        );
    }

    #[test]
    fn baton_state_uses_state_revision_wire_shape() {
        let event = serde_json::json!({
            "id": "bb".repeat(32),
            "kind": buzz_sdk::kind::KIND_MEETING_STATE,
            "created_at": 123,
            "tags": [
                ["h", "00000000-0000-4000-8000-000000000001"],
                ["v", "2"],
                ["policy", "moderated-baton-v1"],
                ["phase", "moderator_idle"],
                ["floor-revision", "1"],
                ["intent-revision", "0"],
                ["speech-revision", "0"],
                ["state-revision", "7"],
                ["moderator", "cc".repeat(32)]
            ],
            "content": serde_json::json!({
                "phase": "moderator_idle",
                "state_revision": 7
            }).to_string(),
        });
        let state = parse_baton_state(&event).expect("valid Baton State");
        assert_eq!(state.state_revision, 7);
        assert_eq!(state.floor_revision, 1);
        assert_eq!(state.phase, "moderator_idle");
    }

    #[test]
    fn v2_baton_state_exposes_inferred_board_fences() {
        let event = serde_json::json!({
            "id": "bb".repeat(32),
            "kind": buzz_sdk::kind::KIND_MEETING_STATE,
            "created_at": 123,
            "tags": [
                ["h", "00000000-0000-4000-8000-000000000001"],
                ["v", "3"],
                ["policy", "moderated-board-v1"],
                ["phase", "moderator_idle"],
                ["floor-revision", "1"],
                ["intent-revision", "0"],
                ["speech-revision", "0"],
                ["state-revision", "1"],
                ["moderator", "cc".repeat(32)]
            ],
            "content": serde_json::json!({
                "phase": "moderator_idle",
                "state_revision": 1,
                "board_control": {
                    "phase": "board_pending",
                    "control_epoch": 4,
                    "board_window": 9
                }
            }).to_string(),
        });
        let state = parse_baton_state(&event).expect("valid V2 Baton State");
        assert!(baton_is_v2(&state));
        assert_eq!(meeting_v2_board_fences(&state, None, None).unwrap(), (4, 9));
        assert_eq!(
            meeting_v2_board_fences(&state, Some(3), Some(8)).unwrap(),
            (3, 8),
            "explicit fences remain available for CAS/race tests"
        );

        let action_event = serde_json::json!({
            "id": "dd".repeat(32),
            "kind": buzz_sdk::kind::KIND_MEETING_STATE,
            "created_at": 124,
            "tags": [
                ["h", "00000000-0000-4000-8000-000000000001"],
                ["v", "3"],
                ["policy", buzz_sdk::MEETING_V2_ACTIONS_POLICY],
                ["phase", "moderator_idle"],
                ["floor-revision", "1"],
                ["intent-revision", "0"],
                ["speech-revision", "0"],
                ["state-revision", "2"],
                ["moderator", "cc".repeat(32)]
            ],
            "content": serde_json::json!({
                "phase": "moderator_idle",
                "state_revision": 2,
                "board_control": {
                    "phase": "finalizing_actions",
                    "control_epoch": 4,
                    "board_window": 9,
                    "action": {
                        "mode": "host_direct",
                        "action_run_id": "00000000-0000-4000-8000-000000000001",
                        "board_event_id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "action_window_epoch": 1,
                        "condition": "runnable"
                    }
                }
            }).to_string(),
        });
        let action_state = parse_baton_state(&action_event).expect("valid action-capable State");
        assert!(baton_is_v2(&action_state));
        assert_eq!(
            action_state.content["board_control"]["action"]["mode"],
            "host_direct"
        );
    }

    #[test]
    fn v1_write_response_exposes_canonical_object_and_local_id() {
        let canonical_id = "aa".repeat(32);
        let response = serde_json::json!({
            "event_id": "bb".repeat(32),
            "accepted": true,
            "message": format!(
                "response:{}",
                serde_json::json!({
                    "canonical_object_id": canonical_id,
                    "state_revision": 9,
                    "duplicate": false,
                })
            ),
        })
        .to_string();
        let output = v1_write_response(&response, "intent_id", "cc");
        assert_eq!(output["intent_id"], "cc");
        assert_eq!(output["canonical_object_id"], "aa".repeat(32));
        assert_eq!(output["state_revision"], 9);
        assert_eq!(output["accepted"], true);
    }
}
