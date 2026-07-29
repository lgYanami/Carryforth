//! Policy-aware Meeting canonical speech ingestion.

use std::sync::Arc;

use buzz_core::tenant::TenantContext;
use buzz_db::meeting_floor::{SayOutcome, WinnerSelector};
use nostr::Event;

use crate::state::AppState;

use super::command_executor::{
    map_meeting_db_error, parse_positive_round, validate_meeting_tag_vocabulary, MeetingProtocol,
};
use super::ingest::{extract_channel_id, IngestError, IngestResult};

/// Route a Meeting speech by the Session's frozen protocol.
///
/// Stage one intentionally has no V1 speech mutation yet. Persisted policy is
/// resolved before any V0 tag validation so a V1 command can never enter the
/// V0 floor state machine.
pub async fn handle_speech(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: &Event,
) -> Result<IngestResult, IngestError> {
    let session_id = extract_channel_id(event)
        .ok_or_else(|| IngestError::Rejected("invalid: bad meeting session id".into()))?;
    let persisted = buzz_db::meeting::get_meeting_policy(&state.db, tenant.community(), session_id)
        .await
        .map_err(map_meeting_db_error)?;
    let protocol =
        MeetingProtocol::from_persisted(persisted.schema_version, &persisted.floor_policy_version)?;
    if protocol == MeetingProtocol::ModeratedBatonV1 {
        return Err(IngestError::Rejected(
            "invalid: Meeting V1 speech is not available in stage one".into(),
        ));
    }

    handle_v0_speech(tenant, state, event, session_id).await
}

async fn handle_v0_speech(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: &Event,
    session_id: uuid::Uuid,
) -> Result<IngestResult, IngestError> {
    if event.content.trim().is_empty() {
        return Err(IngestError::Rejected(
            "invalid: meeting speech content is required".into(),
        ));
    }
    validate_meeting_tag_vocabulary(event, &["h", "meeting-round", "meeting-grant"], &["p"])?;
    let round_number = parse_positive_round(event)?;
    let grant_event_id = single_hex_event_tag(event, "meeting-grant")?;
    let config = crate::meeting_runtime::floor_config_from_env();

    let outcome = buzz_db::meeting_floor::say(
        &state.db,
        tenant.community(),
        session_id,
        round_number,
        &grant_event_id,
        event,
        &state.relay_keypair,
        config,
        WinnerSelector::UniformRandom,
    )
    .await
    .map_err(map_meeting_db_error)?;

    match outcome {
        SayOutcome::Accepted {
            round_number,
            speech_event_id,
            next_round_number,
            floor_revision,
        } => Ok(IngestResult {
            event_id: event.id.to_hex(),
            accepted: true,
            message: format!(
                "response:{}",
                serde_json::json!({
                    "meeting_id": session_id,
                    "round_number": round_number,
                    "speech_event_id": hex::encode(speech_event_id),
                    "next_round_number": next_round_number,
                    "floor_revision": floor_revision,
                })
            ),
        }),
        SayOutcome::Duplicate {
            round_number,
            speech_event_id,
        } => Ok(IngestResult {
            event_id: event.id.to_hex(),
            accepted: true,
            message: format!(
                "response:{}",
                serde_json::json!({
                    "meeting_id": session_id,
                    "round_number": round_number,
                    "speech_event_id": hex::encode(speech_event_id),
                    "duplicate": true,
                })
            ),
        }),
        SayOutcome::GrantConsumed {
            accepted_speech_event_id,
        } => Err(IngestError::Rejected(format!(
            "conflict: meeting Grant already consumed by speech {}",
            hex::encode(accepted_speech_event_id)
        ))),
    }
}

fn single_hex_event_tag(event: &Event, name: &str) -> Result<Vec<u8>, IngestError> {
    let value = event
        .tags
        .iter()
        .find_map(|tag| {
            let parts = tag.as_slice();
            (parts.len() == 2 && parts[0].as_str() == name).then(|| parts[1].as_str())
        })
        .ok_or_else(|| IngestError::Rejected(format!("invalid: missing {name} tag")))?;
    if value.len() != 64 || !value.chars().all(|character| character.is_ascii_hexdigit()) {
        return Err(IngestError::Rejected(format!(
            "invalid: {name} must be a 64-character event ID"
        )));
    }
    hex::decode(value).map_err(|_| IngestError::Rejected(format!("invalid: bad {name} hex")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};

    #[test]
    fn grant_tag_requires_exact_event_id() {
        let keys = Keys::generate();
        let event = EventBuilder::new(Kind::Custom(9), "hello")
            .tags([
                Tag::parse(["h", &uuid::Uuid::new_v4().to_string()]).unwrap(),
                Tag::parse(["meeting-round", "1"]).unwrap(),
                Tag::parse(["meeting-grant", "not-an-id"]).unwrap(),
            ])
            .sign_with_keys(&keys)
            .unwrap();
        assert!(single_hex_event_tag(&event, "meeting-grant").is_err());
    }
}
