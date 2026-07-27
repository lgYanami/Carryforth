use std::collections::HashSet;

use serde::Serialize;
use uuid::Uuid;

use crate::client::{
    extract_d_tag, extract_p_tags, extract_tag_value, normalize_write_response,
    print_create_response, BuzzClient,
};
use crate::error::CliError;
use crate::validate::{parse_uuid, sdk_err, validate_hex64};
use crate::OutputFormat;

const KIND_GROUP_METADATA: u32 = 39000;
const KIND_GROUP_MEMBERS: u32 = 39002;

#[derive(Debug, Serialize)]
struct MeetingSummary {
    meeting_id: String,
    title: String,
    description: Option<String>,
    room_kind: String,
    status: &'static str,
    updated_at: u64,
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
        MeetingsCmd::End { meeting } => cmd_end_meeting(client, &meeting).await,
    }
}
