use std::collections::HashSet;

use serde::Serialize;
use uuid::Uuid;

use crate::client::{
    extract_d_tag, extract_p_tags, extract_tag_value, normalize_write_response,
    print_create_response, BuzzClient,
};
use crate::error::CliError;
use crate::validate::{parse_uuid, read_or_stdin, sdk_err, validate_hex64};
use crate::OutputFormat;

const KIND_GROUP_METADATA: u32 = 39000;
const KIND_GROUP_MEMBERS: u32 = 39002;
const KIND_MEETING_SPEECH: u32 = 9;
const KIND_MEETING_FLOOR_CLAIM: u32 = 42102;
const KIND_MEETING_ROUND_STATE: u32 = 42103;
const KIND_MEETING_FLOOR_SIGNAL: u32 = 42104;

#[derive(Debug, Serialize)]
struct MeetingSummary {
    meeting_id: String,
    title: String,
    description: Option<String>,
    room_kind: String,
    status: &'static str,
    updated_at: u64,
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
    client: &BuzzClient,
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
    client: &BuzzClient,
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

fn is_meeting_metadata(event: &serde_json::Value) -> bool {
    extract_tag_value(event, "room_kind") == "meeting"
}

fn meeting_summary(event: &serde_json::Value) -> Option<MeetingSummary> {
    if !is_meeting_metadata(event) {
        return None;
    }
    let meeting_id = extract_d_tag(event);
    let title = extract_tag_value(event, "name");
    if meeting_id.is_empty() || title.is_empty() {
        return None;
    }
    let description = extract_tag_value(event, "about");
    let archived = extract_tag_value(event, "archived") == "true";
    Some(MeetingSummary {
        meeting_id,
        title,
        description: (!description.is_empty()).then_some(description),
        room_kind: "meeting".to_string(),
        status: if archived { "ended" } else { "active" },
        updated_at: event
            .get("created_at")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
    })
}

async fn fetch_meeting_metadata(
    client: &BuzzClient,
    meeting_id: &str,
) -> Result<Option<serde_json::Value>, CliError> {
    let filter = serde_json::json!({
        "kinds": [KIND_GROUP_METADATA],
        "#d": [meeting_id],
        "limit": 1,
    });
    let response = client.query(&filter).await?;
    let events: Vec<serde_json::Value> = serde_json::from_str(&response).unwrap_or_default();
    Ok(events.into_iter().find(is_meeting_metadata))
}

pub async fn cmd_create_meeting(
    client: &BuzzClient,
    title: &str,
    description: Option<&str>,
    source: Option<&str>,
    participant_pubkeys: &[String],
) -> Result<(), CliError> {
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
    let builder = buzz_sdk::build_meeting_create(
        meeting_id,
        title,
        description,
        source_channel_id,
        &participant_refs,
    )
    .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let response = client.submit_event(event).await?;
    print_create_response(&response, "meeting_id", &meeting_id.to_string());
    Ok(())
}

pub async fn cmd_list_meetings(
    client: &BuzzClient,
    include_ended: bool,
    limit: u32,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let filter = serde_json::json!({
        "kinds": [KIND_GROUP_METADATA],
    });
    let events = client.query_paginated(filter, limit).await?;
    let meetings: Vec<MeetingSummary> = events
        .iter()
        .filter_map(meeting_summary)
        .filter(|meeting| include_ended || meeting.status == "active")
        .collect();

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

pub async fn cmd_show_meeting(client: &BuzzClient, meeting_id: &str) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?.to_string();
    let filters = [
        serde_json::json!({
            "kinds": [KIND_GROUP_METADATA],
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
    let Some(metadata) = events
        .iter()
        .find(|event| {
            event.get("kind").and_then(serde_json::Value::as_u64)
                == Some(KIND_GROUP_METADATA as u64)
                && is_meeting_metadata(event)
        })
        .and_then(meeting_summary)
    else {
        println!("null");
        return Ok(());
    };

    let create = events.iter().find(|event| {
        event.get("kind").and_then(serde_json::Value::as_u64)
            == Some(buzz_sdk::kind::KIND_MEETING_CREATE as u64)
    });
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

    let output = serde_json::json!({
        "meeting_id": metadata.meeting_id,
        "title": metadata.title,
        "description": metadata.description,
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
        "floor": floor,
    });
    println!("{output}");
    Ok(())
}

pub async fn cmd_list_participants(client: &BuzzClient, meeting_id: &str) -> Result<(), CliError> {
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
    client: &BuzzClient,
    meeting_id: &str,
    limit: u32,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?.to_string();
    let filter = serde_json::json!({
        "kinds": [KIND_MEETING_SPEECH],
        "#h": [&meeting_id],
    });
    let mut events = client.query_paginated(filter, limit).await?;
    events.sort_by(|left, right| {
        event_round(left)
            .cmp(&event_round(right))
            .then_with(|| event_id(left).cmp(event_id(right)))
    });

    let output = match format {
        OutputFormat::Json => events,
        OutputFormat::Compact => events
            .iter()
            .map(|event| {
                serde_json::json!({
                    "event_id": event_id(event),
                    "round_number": event_round(event),
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

pub async fn cmd_floor_status(client: &BuzzClient, meeting_id: &str) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?.to_string();
    match fetch_current_floor(client, &meeting_id).await? {
        Some(state) => println!(
            "{}",
            serde_json::to_string(&state).unwrap_or_else(|_| "null".to_string())
        ),
        None => println!("null"),
    }
    Ok(())
}

pub async fn cmd_floor_history(
    client: &BuzzClient,
    meeting_id: &str,
    limit: u32,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?.to_string();
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

async fn current_open_floor(
    client: &BuzzClient,
    meeting_id: Uuid,
    operation: &str,
) -> Result<FloorState, CliError> {
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
    client: &BuzzClient,
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
    client: &BuzzClient,
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

pub async fn cmd_floor_yield(client: &BuzzClient, meeting_id: &str) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
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
    client: &BuzzClient,
    meeting_id: &str,
    wait: bool,
    timeout_secs: u64,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
    let meeting_id_text = meeting_id.to_string();
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
    client: &BuzzClient,
    meeting_id: &str,
    content: &str,
) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
    let state = fetch_current_floor(client, &meeting_id.to_string())
        .await?
        .ok_or_else(|| CliError::NotFound(format!("meeting floor not found: {meeting_id}")))?;
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
    let grant_event_id = state
        .grant_event_id
        .as_deref()
        .ok_or_else(|| CliError::Other("granted floor state has no Grant event ID".into()))?;
    let content = read_or_stdin(content)?;
    let builder = buzz_sdk::build_meeting_speech(
        meeting_id,
        state.round_number,
        grant_event_id,
        &content,
        &[],
    )
    .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;
    let response = client.submit_event(event).await?;
    println!("{}", normalize_write_response(&response));
    Ok(())
}

pub async fn cmd_end_meeting(client: &BuzzClient, meeting_id: &str) -> Result<(), CliError> {
    let meeting_id = parse_uuid(meeting_id)?;
    let filter = serde_json::json!({
        "kinds": [buzz_sdk::kind::KIND_MEETING_CREATE],
        "#h": [meeting_id.to_string()],
        "limit": 1,
    });
    let response = client.query(&filter).await?;
    let events: Vec<serde_json::Value> = serde_json::from_str(&response).unwrap_or_default();
    let create_event_id = events
        .first()
        .and_then(|event| event.get("id"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| CliError::Other(format!("meeting not found: {meeting_id}")))?;

    let builder = buzz_sdk::build_meeting_end(meeting_id, create_event_id).map_err(sdk_err)?;
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

fn event_id(event: &serde_json::Value) -> &str {
    event
        .get("id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
}

pub async fn dispatch(
    command: crate::MeetingsCmd,
    client: &BuzzClient,
    format: &OutputFormat,
) -> Result<(), CliError> {
    use crate::MeetingsCmd;
    match command {
        MeetingsCmd::Create {
            title,
            description,
            source,
            participants,
        } => {
            cmd_create_meeting(
                client,
                &title,
                description.as_deref(),
                source.as_deref(),
                &participants,
            )
            .await
        }
        MeetingsCmd::List {
            include_ended,
            limit,
        } => cmd_list_meetings(client, include_ended, limit, format).await,
        MeetingsCmd::Show { meeting } => cmd_show_meeting(client, &meeting).await,
        MeetingsCmd::Participants { meeting } => cmd_list_participants(client, &meeting).await,
        MeetingsCmd::History { meeting, limit } => {
            cmd_meeting_history(client, &meeting, limit, format).await
        }
        MeetingsCmd::Say { meeting, content } => cmd_say_meeting(client, &meeting, &content).await,
        MeetingsCmd::Floor { command } => {
            use crate::MeetingFloorCmd;
            match command {
                MeetingFloorCmd::Status { meeting } => cmd_floor_status(client, &meeting).await,
                MeetingFloorCmd::History { meeting, limit } => {
                    cmd_floor_history(client, &meeting, limit, format).await
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
    }
}
