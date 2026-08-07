//! Strict parser for canonical Meeting Speech events.

use super::*;

pub(in crate::commands::meetings) fn parse_speech(
    event: &Event,
    meeting_id: &str,
    roster: &BTreeMap<String, MeetingParticipantType>,
    moderator_pubkey: &str,
    current_speech_revision: u64,
) -> Result<Option<MeetingSpeech>, String> {
    if event.kind.as_u16() as u32 != KIND_STREAM_MESSAGE
        || single_tag(event, "h") != Some(meeting_id)
        || single_tag(event, "v") != Some(buzz_sdk_pkg::MEETING_V2_SCHEMA_VERSION)
    {
        return Ok(None);
    }
    event
        .verify()
        .map_err(|error| integrity_error(format!("invalid Meeting Speech signature: {error}")))?;
    let author_pubkey = event.pubkey.to_hex();
    let Some(author_participant_type) = roster.get(&author_pubkey).copied() else {
        return Ok(None);
    };
    let grant_event_id = required_tag_string(event, "meeting-grant", "Meeting Speech")?;
    require_hex64(&grant_event_id, "Meeting Speech grant")?;
    let speech_revision = required_tag_string(event, "speech-revision", "Meeting Speech")?
        .parse::<u64>()
        .map_err(|_| integrity_error("Meeting Speech has an invalid revision"))?;
    if speech_revision == 0 || speech_revision > current_speech_revision {
        return Ok(None);
    }
    if event.content.trim().is_empty() {
        return Err(integrity_error("Meeting Speech content is empty"));
    }
    if matches!(author_participant_type, MeetingParticipantType::Unknown) {
        return Err(integrity_error(
            "Meeting Speech author has no frozen participant type",
        ));
    }
    let handoff_to = unique_speech_tag(event, "handoff-to")?;
    let handoff_type = unique_speech_tag(event, "handoff-type")?;
    let handoff_reason = unique_speech_tag(event, "handoff-reason")?;
    let handoff = match (handoff_to, handoff_type, handoff_reason) {
        (None, None, None) => None,
        (Some(target_pubkey), Some(handoff_type), Some(reason)) => {
            require_hex64(target_pubkey, "Meeting Speech Handoff target")?;
            if target_pubkey == author_pubkey.as_str() {
                return Err(integrity_error(
                    "Meeting Speech Handoff target must be another participant",
                ));
            }
            if !roster.contains_key(target_pubkey) {
                return Err(integrity_error(
                    "Meeting Speech Handoff target is outside the frozen roster",
                ));
            }
            let handoff_type = match handoff_type {
                "question" => MeetingSpeechHandoffType::Question,
                "information_request" => MeetingSpeechHandoffType::InformationRequest,
                "clarification" => MeetingSpeechHandoffType::Clarification,
                "review" => MeetingSpeechHandoffType::Review,
                "response_requested" => MeetingSpeechHandoffType::ResponseRequested,
                _ => {
                    return Err(integrity_error(
                        "Meeting Speech has an unsupported Handoff type",
                    ));
                }
            };
            if reason.trim().is_empty()
                || reason.trim() != reason
                || reason.len() > 1024
                || reason.chars().any(char::is_control)
            {
                return Err(integrity_error(
                    "Meeting Speech has an invalid Handoff reason",
                ));
            }
            Some(MeetingSpeechHandoff {
                target_pubkey: target_pubkey.to_string(),
                handoff_type,
                reason: reason.to_string(),
            })
        }
        _ => {
            return Err(integrity_error(
                "Meeting Speech Handoff fields must appear together",
            ));
        }
    };
    let mentions = tags_named(event, "p")
        .filter_map(|tag| tag.get(1).cloned())
        .filter(|pubkey| roster.contains_key(pubkey))
        .collect();
    let author_is_moderator = author_pubkey == moderator_pubkey;
    Ok(Some(MeetingSpeech {
        event_id: event.id.to_hex(),
        author_pubkey,
        content: event.content.clone(),
        created_at: event.created_at.as_secs(),
        speech_revision,
        grant_event_id,
        mentions,
        author_participant_type,
        author_is_moderator,
        handoff,
    }))
}

fn unique_speech_tag<'a>(event: &'a Event, name: &str) -> Result<Option<&'a str>, String> {
    let mut tags = tags_named(event, name);
    let Some(tag) = tags.next() else {
        return Ok(None);
    };
    if tags.next().is_some() {
        return Err(integrity_error(format!(
            "Meeting Speech has duplicate {name} tags"
        )));
    }
    let value = tag
        .get(1)
        .ok_or_else(|| integrity_error(format!("Meeting Speech has an invalid {name} tag")))?;
    if value.is_empty() {
        return Err(integrity_error(format!(
            "Meeting Speech has an empty {name} tag"
        )));
    }
    Ok(Some(value))
}
