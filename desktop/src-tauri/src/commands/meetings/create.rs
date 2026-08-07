//! Human Meeting V2 creation boundary.
//!
//! A stable frontend `submission_id` maps to one signed Create event. When a
//! network response is indeterminate, retrying the same submission republishes
//! that exact event instead of minting a second Meeting command.

use std::collections::BTreeSet;

use buzz_sdk_pkg::{MeetingV2CreateParams, MEETING_V2_ACTIONS_POLICY, MEETING_V2_SCHEMA_VERSION};
use nostr::{Event, EventId, PublicKey};
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    pending_writes::PendingMeetingCreate,
    relay::{
        parse_command_response, relay_api_base_url_with_override, submit_signed_event_at_with_keys,
        SubmitEventResponse,
    },
};

use super::{read_meeting_identity_at, MeetingCapabilityStatus, MeetingIdentity};

const MAX_PENDING_MEETING_CREATES: usize = 32;

/// Closed Human intent accepted by the Desktop Meeting creation boundary.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateMeetingInput {
    /// Stable UUID generated once for this submission and reused on retry.
    submission_id: String,
    title: String,
    description: Option<String>,
    source_channel_id: Option<String>,
    /// Frozen roster excluding the current Human host.
    participant_pubkeys: Vec<String>,
    /// Complete initial Markdown Board.
    initial_board: String,
}

/// Result of publishing a Human-authored Meeting Create.
#[derive(Debug, Serialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum CreateMeetingResult {
    /// Relay accepted the exact signed Create command.
    Accepted {
        meeting_id: String,
        event_id: String,
        host_pubkey: String,
        participant_pubkeys: Vec<String>,
        title: String,
    },
    /// The request may have reached the Relay. Retrying with the same
    /// `submission_id` is the only safe next write.
    Indeterminate {
        meeting_id: String,
        event_id: String,
        message: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateFingerprint<'a> {
    title: &'a str,
    description: Option<&'a str>,
    source_channel_id: Option<String>,
    participant_pubkeys: &'a [String],
    initial_board: &'a str,
}

struct ValidatedCreate {
    submission_id: String,
    title: String,
    description: Option<String>,
    source_channel_id: Option<Uuid>,
    participant_pubkeys: Vec<String>,
    initial_board: String,
    fingerprint: String,
}

#[derive(Debug, Deserialize)]
struct MeetingCreateReceipt {
    meeting_id: String,
    room_kind: String,
    status: String,
    participant_count: usize,
    schema_version: String,
    floor_policy_version: String,
    moderator: Option<String>,
    board_event_id: Option<String>,
}

/// Create a direct-action Meeting V2 with the current Human as immutable host.
#[tauri::command]
pub async fn create_meeting(
    input: CreateMeetingInput,
    state: State<'_, AppState>,
) -> Result<CreateMeetingResult, String> {
    execute_create(input, &state).await
}

async fn execute_create(
    input: CreateMeetingInput,
    state: &AppState,
) -> Result<CreateMeetingResult, String> {
    // Capture target and signer before the first await. A Community or identity
    // switch must never retarget an already-started Human intent.
    let api_base_url = relay_api_base_url_with_override(state);
    let keys = state.signing_keys()?;
    let signer_pubkey = keys.public_key().to_hex();
    let validated = validate_input(input, &signer_pubkey)?;

    let pending =
        if let Some(pending) = find_pending(state, &validated, &api_base_url, &signer_pubkey)? {
            pending
        } else {
            let identity = read_meeting_identity_at(state, &api_base_url)
                .await?
                .ok_or_else(|| "unsupported: Relay does not advertise Meeting V2".to_string())?;
            ensure_create_capability(&identity)?;
            let prepared = prepare_create(&validated, &api_base_url, &keys)?;
            insert_or_reuse_pending(state, prepared, &validated, &api_base_url, &signer_pubkey)?
        };

    let response =
        match submit_signed_event_at_with_keys(&pending.event, state, &pending.api_base_url, &keys)
            .await
        {
            Ok(response) => response,
            Err(message) if is_indeterminate_submit_error(&message) => {
                return Ok(indeterminate_result(&pending, message));
            }
            Err(message) => {
                remove_pending(state, &validated.submission_id, &pending.event);
                return Err(message);
            }
        };

    if let Err(message) = validate_receipt(&response, &pending.event, &pending.meeting_id) {
        return Ok(indeterminate_result(
            &pending,
            format!(
                "Relay accepted the Meeting Create, but its receipt could not be verified: {message}. Retry to confirm the same signed event."
            ),
        ));
    }

    remove_pending(state, &validated.submission_id, &pending.event);
    accepted_result(&pending)
}

fn validate_input(
    input: CreateMeetingInput,
    signer_pubkey: &str,
) -> Result<ValidatedCreate, String> {
    let submission_id = canonical_uuid(&input.submission_id, "Meeting submission ID")?;
    canonical_pubkey(signer_pubkey, "current Human identity")?;

    let description = input
        .description
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let source_channel_id = input
        .source_channel_id
        .as_deref()
        .map(|value| canonical_uuid(value, "source Channel ID"))
        .transpose()?
        .map(|value| {
            Uuid::parse_str(&value)
                .map_err(|error| format!("invalid source Channel ID after validation: {error}"))
        })
        .transpose()?;

    let mut seen = BTreeSet::from([signer_pubkey.to_string()]);
    let mut participant_pubkeys = Vec::with_capacity(input.participant_pubkeys.len());
    for pubkey in input.participant_pubkeys {
        canonical_pubkey(&pubkey, "Meeting participant")?;
        if !seen.insert(pubkey.clone()) {
            return Err(format!("duplicate Meeting participant: {pubkey}"));
        }
        participant_pubkeys.push(pubkey);
    }

    let fingerprint = serde_json::to_string(&CreateFingerprint {
        title: &input.title,
        description: description.as_deref(),
        source_channel_id: source_channel_id.map(|value| value.to_string()),
        participant_pubkeys: &participant_pubkeys,
        initial_board: &input.initial_board,
    })
    .map_err(|error| format!("serialize Meeting Create fingerprint: {error}"))?;

    Ok(ValidatedCreate {
        submission_id,
        title: input.title,
        description,
        source_channel_id,
        participant_pubkeys,
        initial_board: input.initial_board,
        fingerprint,
    })
}

fn prepare_create(
    input: &ValidatedCreate,
    api_base_url: &str,
    keys: &nostr::Keys,
) -> Result<PendingMeetingCreate, String> {
    let meeting_id = Uuid::new_v4();
    let participant_refs = input
        .participant_pubkeys
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let author_pubkey = keys.public_key().to_hex();
    let builder = buzz_sdk_pkg::build_meeting_v2_actions_create(MeetingV2CreateParams {
        session_id: meeting_id,
        title: &input.title,
        description: input.description.as_deref(),
        source_channel_id: input.source_channel_id,
        author_pubkey: &author_pubkey,
        participant_pubkeys: &participant_refs,
        initial_board: &input.initial_board,
    })
    .map_err(|error| format!("invalid Meeting Create: {error}"))?;
    let event = builder
        .sign_with_keys(keys)
        .map_err(|error| format!("failed to sign Meeting Create: {error}"))?;

    Ok(PendingMeetingCreate {
        event,
        api_base_url: api_base_url.to_string(),
        signer_pubkey: author_pubkey,
        meeting_id: meeting_id.to_string(),
        fingerprint: input.fingerprint.clone(),
    })
}

fn find_pending(
    state: &AppState,
    input: &ValidatedCreate,
    api_base_url: &str,
    signer_pubkey: &str,
) -> Result<Option<PendingMeetingCreate>, String> {
    let pending = state
        .pending_writes
        .meeting_creates
        .lock()
        .map_err(|error| error.to_string())?;
    let Some(existing) = pending.get(&input.submission_id) else {
        return Ok(None);
    };
    validate_pending_binding(existing, input, api_base_url, signer_pubkey)?;
    Ok(Some(existing.clone()))
}

fn insert_or_reuse_pending(
    state: &AppState,
    prepared: PendingMeetingCreate,
    input: &ValidatedCreate,
    api_base_url: &str,
    signer_pubkey: &str,
) -> Result<PendingMeetingCreate, String> {
    let mut pending = state
        .pending_writes
        .meeting_creates
        .lock()
        .map_err(|error| error.to_string())?;
    if let Some(existing) = pending.get(&input.submission_id) {
        validate_pending_binding(existing, input, api_base_url, signer_pubkey)?;
        return Ok(existing.clone());
    }
    if pending.len() >= MAX_PENDING_MEETING_CREATES {
        return Err("too many unresolved Meeting Create submissions; resolve or retry an existing submission first".to_string());
    }
    pending.insert(input.submission_id.clone(), prepared.clone());
    Ok(prepared)
}

fn validate_pending_binding(
    pending: &PendingMeetingCreate,
    input: &ValidatedCreate,
    api_base_url: &str,
    signer_pubkey: &str,
) -> Result<(), String> {
    if pending.fingerprint != input.fingerprint {
        return Err(
            "Meeting submission ID is already bound to a different creation draft".to_string(),
        );
    }
    if pending.api_base_url != api_base_url {
        return Err(
            "Meeting submission belongs to a different Community; switch back before retrying"
                .to_string(),
        );
    }
    if pending.signer_pubkey != signer_pubkey {
        return Err(
            "Meeting submission belongs to a different identity; restore that identity before retrying"
                .to_string(),
        );
    }
    Ok(())
}

fn remove_pending(state: &AppState, submission_id: &str, event: &Event) {
    if let Ok(mut pending) = state.pending_writes.meeting_creates.lock() {
        if pending
            .get(submission_id)
            .is_some_and(|candidate| candidate.event.id == event.id)
        {
            pending.remove(submission_id);
        }
    }
}

fn ensure_create_capability(identity: &MeetingIdentity) -> Result<(), String> {
    if !matches!(
        identity.capability.status,
        MeetingCapabilityStatus::Creatable
    ) {
        return Err("unsupported: Relay has Meeting V2 creation disabled".to_string());
    }
    if !identity.capability.supports_direct_actions
        || !identity.capability.can_create_direct_actions
    {
        return Err(
            "unsupported: Relay cannot create direct-action Meeting V2 sessions".to_string(),
        );
    }
    Ok(())
}

fn validate_receipt(
    response: &SubmitEventResponse,
    event: &Event,
    meeting_id: &str,
) -> Result<(), String> {
    if response.event_id != event.id.to_hex() {
        return Err("event ID does not match the signed Create".to_string());
    }
    if response.message == "duplicate: already processed" {
        return Ok(());
    }
    let receipt: MeetingCreateReceipt = parse_command_response(&response.message)?;
    let expected_moderator = event.pubkey.to_hex();
    let participant_count = event
        .tags
        .iter()
        .filter(|tag| tag.as_slice().first().is_some_and(|name| name == "p"))
        .count()
        + 1;
    if receipt.meeting_id != meeting_id
        || receipt.room_kind != "meeting"
        || receipt.status != "active"
        || receipt.participant_count != participant_count
        || receipt.schema_version != MEETING_V2_SCHEMA_VERSION
        || receipt.floor_policy_version != MEETING_V2_ACTIONS_POLICY
        || receipt.moderator.as_deref() != Some(expected_moderator.as_str())
    {
        return Err("receipt fields do not match the signed Create".to_string());
    }
    let board_event_id = receipt
        .board_event_id
        .as_deref()
        .ok_or_else(|| "receipt is missing the initial Board event ID".to_string())?;
    canonical_event_id(board_event_id, "initial Board event ID")?;
    Ok(())
}

fn accepted_result(pending: &PendingMeetingCreate) -> Result<CreateMeetingResult, String> {
    let title = single_event_tag(&pending.event, "name")
        .ok_or_else(|| "signed Meeting Create has no unique title".to_string())?;
    let participant_pubkeys = pending
        .event
        .tags
        .iter()
        .filter_map(|tag| {
            let values = tag.as_slice();
            (values.first().is_some_and(|name| name == "p"))
                .then(|| values.get(1).cloned())
                .flatten()
        })
        .collect();
    Ok(CreateMeetingResult::Accepted {
        meeting_id: pending.meeting_id.clone(),
        event_id: pending.event.id.to_hex(),
        host_pubkey: pending.signer_pubkey.clone(),
        participant_pubkeys,
        title,
    })
}

fn indeterminate_result(pending: &PendingMeetingCreate, message: String) -> CreateMeetingResult {
    CreateMeetingResult::Indeterminate {
        meeting_id: pending.meeting_id.clone(),
        event_id: pending.event.id.to_hex(),
        message,
    }
}

fn is_indeterminate_submit_error(message: &str) -> bool {
    message.starts_with("relay unreachable:")
        || message.starts_with("relay returned malformed response:")
        || message.starts_with("relay returned 408")
        || message.starts_with("relay returned 5")
}

fn canonical_uuid(value: &str, context: &str) -> Result<String, String> {
    let parsed = Uuid::parse_str(value).map_err(|_| format!("{context} must be a UUID"))?;
    if parsed.is_nil() || parsed.to_string() != value {
        return Err(format!("{context} must be a canonical non-nil UUID"));
    }
    Ok(value.to_string())
}

fn canonical_pubkey(value: &str, context: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || PublicKey::from_hex(value).is_err()
    {
        return Err(format!("{context} must be canonical lowercase hex"));
    }
    Ok(())
}

fn canonical_event_id(value: &str, context: &str) -> Result<(), String> {
    let event_id =
        EventId::from_hex(value).map_err(|_| format!("{context} must be a 32-byte event ID"))?;
    if event_id.to_hex() != value {
        return Err(format!("{context} must be canonical lowercase hex"));
    }
    Ok(())
}

fn single_event_tag(event: &Event, name: &str) -> Option<String> {
    let mut values = event.tags.iter().filter_map(|tag| {
        let values = tag.as_slice();
        (values.first().is_some_and(|candidate| candidate == name))
            .then(|| values.get(1).cloned())
            .flatten()
    });
    let first = values.next()?;
    values.next().is_none().then_some(first)
}

#[cfg(test)]
mod tests {
    use nostr::Keys;

    use super::*;
    use crate::commands::meetings::{MeetingCapability, MeetingCapabilityStatus};

    fn input(participant: &Keys) -> CreateMeetingInput {
        CreateMeetingInput {
            submission_id: "30000000-0000-4000-8000-000000000001".to_string(),
            title: "# Desktop lifecycle review".to_string(),
            description: Some(" Agree on delivery ".to_string()),
            source_channel_id: None,
            participant_pubkeys: vec![participant.public_key().to_hex()],
            initial_board: "# Goal\n\nAgree on delivery.\n\n## Agenda\n\n- Review".to_string(),
        }
    }

    #[test]
    fn desktop_create_contract_uses_camel_case_fields() {
        let participant = Keys::generate().public_key().to_hex();
        let input: CreateMeetingInput = serde_json::from_value(serde_json::json!({
            "submissionId": "30000000-0000-4000-8000-000000000001",
            "title": "Desktop lifecycle review",
            "description": "Agree on delivery",
            "sourceChannelId": "30000000-0000-4000-8000-000000000002",
            "participantPubkeys": [participant],
            "initialBoard": "# Goal\n\nAgree on delivery."
        }))
        .unwrap_or_else(|error| panic!("deserialize Desktop Create payload: {error}"));
        assert_eq!(
            input.source_channel_id.as_deref(),
            Some("30000000-0000-4000-8000-000000000002")
        );
        assert_eq!(input.participant_pubkeys.len(), 1);

        let accepted = serde_json::to_value(CreateMeetingResult::Accepted {
            meeting_id: "30000000-0000-4000-8000-000000000003".to_string(),
            event_id: "ab".repeat(32),
            host_pubkey: "cd".repeat(32),
            participant_pubkeys: vec!["ef".repeat(32)],
            title: "Desktop lifecycle review".to_string(),
        })
        .unwrap_or_else(|error| panic!("serialize accepted Create result: {error}"));
        assert_eq!(
            accepted["meetingId"],
            "30000000-0000-4000-8000-000000000003"
        );
        assert_eq!(accepted["hostPubkey"], "cd".repeat(32));
        assert!(accepted.get("participantPubkeys").is_some());
        assert!(accepted.get("meeting_id").is_none());

        let indeterminate = serde_json::to_value(CreateMeetingResult::Indeterminate {
            meeting_id: "30000000-0000-4000-8000-000000000003".to_string(),
            event_id: "ab".repeat(32),
            message: "retry exact Create".to_string(),
        })
        .unwrap_or_else(|error| panic!("serialize indeterminate Create result: {error}"));
        assert_eq!(indeterminate["eventId"], "ab".repeat(32));
        assert!(indeterminate.get("event_id").is_none());
    }

    #[test]
    fn prepared_create_is_direct_action_self_hosted_and_stable() {
        let host = Keys::generate();
        let participant = Keys::generate();
        let validated = validate_input(input(&participant), &host.public_key().to_hex())
            .unwrap_or_else(|error| panic!("validate input: {error}"));
        let prepared = prepare_create(&validated, "http://relay.test", &host)
            .unwrap_or_else(|error| panic!("prepare Create: {error}"));

        assert_eq!(prepared.event.pubkey, host.public_key());
        assert_eq!(single_event_tag(&prepared.event, "v").as_deref(), Some("3"));
        assert_eq!(
            single_event_tag(&prepared.event, "policy").as_deref(),
            Some(MEETING_V2_ACTIONS_POLICY)
        );
        assert_eq!(
            single_event_tag(&prepared.event, "name").as_deref(),
            Some("Desktop lifecycle review")
        );
        let participant_pubkey = participant.public_key().to_hex();
        assert_eq!(
            single_event_tag(&prepared.event, "p").as_deref(),
            Some(participant_pubkey.as_str())
        );
        let board = buzz_sdk_pkg::parse_meeting_v2_board_content(&prepared.event.content)
            .unwrap_or_else(|error| panic!("parse Board: {error}"));
        assert_eq!(board.body, validated.initial_board);

        let cloned = prepared.clone();
        assert_eq!(cloned.event.id, prepared.event.id);
        assert_eq!(cloned.meeting_id, prepared.meeting_id);
    }

    #[test]
    fn input_rejects_self_duplicate_noncanonical_and_invalid_board() {
        let host = Keys::generate();
        let participant = Keys::generate();
        let host_pubkey = host.public_key().to_hex();

        let mut duplicate = input(&participant);
        duplicate
            .participant_pubkeys
            .push(participant.public_key().to_hex());
        assert!(validate_input(duplicate, &host_pubkey).is_err());

        let mut self_roster = input(&participant);
        self_roster.participant_pubkeys = vec![host_pubkey.clone()];
        assert!(validate_input(self_roster, &host_pubkey).is_err());

        let mut noncanonical = input(&participant);
        noncanonical.submission_id.push(' ');
        assert!(validate_input(noncanonical, &host_pubkey).is_err());

        let mut empty_board = input(&participant);
        empty_board.initial_board = " \n ".to_string();
        let validated = validate_input(empty_board, &host_pubkey)
            .unwrap_or_else(|error| panic!("validate outer input: {error}"));
        assert!(prepare_create(&validated, "http://relay.test", &host).is_err());
    }

    #[test]
    fn capability_requires_both_create_extensions() {
        let relay = Keys::generate();
        let identity =
            |status, supports_direct_actions, can_create_direct_actions| MeetingIdentity {
                relay_pubkey: relay.public_key(),
                capability: MeetingCapability {
                    status,
                    relay_pubkey: Some(relay.public_key().to_hex()),
                    supports_direct_actions,
                    can_create_direct_actions,
                },
            };
        assert!(ensure_create_capability(&identity(
            MeetingCapabilityStatus::Creatable,
            true,
            true
        ))
        .is_ok());
        assert!(
            ensure_create_capability(&identity(MeetingCapabilityStatus::Readable, true, true))
                .is_err()
        );
        assert!(ensure_create_capability(&identity(
            MeetingCapabilityStatus::Creatable,
            true,
            false
        ))
        .is_err());
    }

    #[test]
    fn receipt_accepts_exact_response_and_duplicate_replay() {
        let host = Keys::generate();
        let participant = Keys::generate();
        let validated = validate_input(input(&participant), &host.public_key().to_hex())
            .unwrap_or_else(|error| panic!("validate input: {error}"));
        let prepared = prepare_create(&validated, "http://relay.test", &host)
            .unwrap_or_else(|error| panic!("prepare Create: {error}"));
        let response = SubmitEventResponse {
            event_id: prepared.event.id.to_hex(),
            accepted: true,
            message: format!(
                "response:{}",
                serde_json::json!({
                    "meeting_id": prepared.meeting_id,
                    "room_kind": "meeting",
                    "status": "active",
                    "participant_count": 2,
                    "schema_version": "3",
                    "floor_policy_version": MEETING_V2_ACTIONS_POLICY,
                    "moderator": host.public_key().to_hex(),
                    "board_event_id": "ab".repeat(32),
                })
            ),
        };
        assert!(validate_receipt(&response, &prepared.event, &prepared.meeting_id).is_ok());

        let duplicate = SubmitEventResponse {
            event_id: prepared.event.id.to_hex(),
            accepted: true,
            message: "duplicate: already processed".to_string(),
        };
        assert!(validate_receipt(&duplicate, &prepared.event, &prepared.meeting_id).is_ok());
    }

    #[test]
    fn board_receipt_accepts_canonical_hash_and_rejects_uppercase() {
        let arbitrary_hash = "ff".repeat(32);
        assert!(canonical_event_id(&arbitrary_hash, "Board event ID").is_ok());
        let uppercase_event_id = "ab".repeat(32).to_uppercase();
        assert!(canonical_event_id(&uppercase_event_id, "Board event ID").is_err());
    }

    #[test]
    fn only_ambiguous_transport_failures_keep_the_signed_submission() {
        assert!(is_indeterminate_submit_error(
            "relay unreachable: request timed out"
        ));
        assert!(is_indeterminate_submit_error(
            "relay returned malformed response: not valid JSON"
        ));
        assert!(is_indeterminate_submit_error(
            "relay returned 502 Bad Gateway"
        ));
        assert!(!is_indeterminate_submit_error(
            "relay rejected event: restricted"
        ));
        assert!(!is_indeterminate_submit_error(
            "relay rate-limited: retry in 2s"
        ));
    }
}
