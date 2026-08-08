use super::super::directory::{list_item_from_load, meeting_attention_reason};
use super::super::{
    MeetingAttentionReason, MeetingBoard, MeetingBoardControl, MeetingBoardSource,
    MeetingHostState, MeetingParticipant, MeetingParticipantType,
};
use super::*;
use nostr::Keys;

const MEETING_ID: &str = "00000000-0000-4000-8000-000000000001";
const ACTION_RUN_ID: &str = "00000000-0000-4000-8000-000000000002";

fn object_id(byte: u8) -> String {
    format!("{byte:02x}").repeat(32)
}

fn snapshot(host: &Keys, participant: &Keys) -> MeetingSnapshot {
    let host_pubkey = host.public_key().to_hex();
    MeetingSnapshot {
        meeting_id: MEETING_ID.to_string(),
        title: "Action review".to_string(),
        description: None,
        source_channel_id: None,
        schema_version: 3,
        policy: buzz_sdk_pkg::MEETING_V2_ACTIONS_POLICY.to_string(),
        host_pubkey: host_pubkey.clone(),
        moderator_pubkey: host_pubkey.clone(),
        create_event_id: object_id(1),
        created_at: 1,
        lifecycle: MeetingLifecycle::Active,
        phase: "moderator_idle".to_string(),
        state_revision: 10,
        floor_revision: 4,
        intent_revision: 2,
        speech_revision: 3,
        current_speaker_pubkey: None,
        current_offer_pubkey: None,
        floor: None,
        host: Some(MeetingHostState {
            control_token: object_id(2),
            state_event_id: object_id(3),
            control_epoch: 7,
            decision_epoch: 8,
            decision_deadline_ms: Some(60_000),
            next_action_at_ms: None,
            consecutive_moderator_speeches: 0,
            forced_return_to_moderator: false,
            pending_intents: Vec::new(),
            open_handoffs: Vec::new(),
            board_control: MeetingBoardControl {
                phase: "floor_ready".to_string(),
                control_epoch: 7,
                board_window: 5,
                board_started_at_ms: Some(1_000),
                board_deadline_at_ms: Some(2_000),
                board_completed_at_ms: Some(2_500),
                board_outcome: Some("updated".to_string()),
            },
            can_select: true,
            can_close: true,
            can_recall: false,
        }),
        participants: vec![
            MeetingParticipant {
                pubkey: host_pubkey.clone(),
                participant_type: MeetingParticipantType::Human,
                channel_role: "owner".to_string(),
            },
            MeetingParticipant {
                pubkey: participant.public_key().to_hex(),
                participant_type: MeetingParticipantType::Agent,
                channel_role: "member".to_string(),
            },
        ],
        board: MeetingBoard {
            event_id: object_id(4),
            format: "markdown".to_string(),
            body: "# Final Board".to_string(),
            moderator_pubkey: host_pubkey,
            updated_at: 1,
            source: MeetingBoardSource::Projection,
        },
        action: None,
        end: None,
        latest_speech_at: None,
        authoritative_updated_at: 1,
    }
}

fn enter_action_phase(snapshot: &mut MeetingSnapshot, condition: &str) {
    snapshot.lifecycle = MeetingLifecycle::FinalizingActions;
    snapshot
        .host
        .as_mut()
        .unwrap_or_else(|| panic!("host projection"))
        .board_control
        .phase = "finalizing_actions".to_string();
    snapshot.action = Some(MeetingActionState {
        action_run_id: ACTION_RUN_ID.to_string(),
        board_event_id: snapshot.board.event_id.clone(),
        action_window_epoch: 2,
        condition: condition.to_string(),
        terminal_status: None,
        completion_event_id: None,
        action_deadline_at_ms: (condition == "runnable").then_some(90_000),
        progress_seq: 0,
        last_progress_stage: None,
        last_progress_at_ms: None,
        operator_hard_deadline_ms: Some(3_600_000),
        created_at_ms: Some(1_000),
        last_error_code: (condition == "blocked").then(|| "external_operation_failed".to_string()),
    });
}

#[test]
fn meeting_list_attention_is_viewer_scoped_and_product_level() {
    let host = Keys::generate();
    let participant = Keys::generate();
    let host_pubkey = host.public_key().to_hex();
    let participant_pubkey = participant.public_key().to_hex();
    let mut meeting = snapshot(&host, &participant);

    assert_eq!(
        meeting_attention_reason(&meeting, &host_pubkey),
        Some(MeetingAttentionReason::HostFloor)
    );
    assert_eq!(
        meeting_attention_reason(&meeting, &participant_pubkey),
        None
    );

    meeting
        .host
        .as_mut()
        .unwrap_or_else(|| panic!("host projection"))
        .board_control
        .phase = "board_pending".to_string();
    assert_eq!(
        meeting_attention_reason(&meeting, &host_pubkey),
        Some(MeetingAttentionReason::HostBoard)
    );

    enter_action_phase(&mut meeting, "runnable");
    assert_eq!(
        meeting_attention_reason(&meeting, &host_pubkey),
        Some(MeetingAttentionReason::HostAction)
    );
    meeting
        .action
        .as_mut()
        .unwrap_or_else(|| panic!("action projection"))
        .condition = "blocked".to_string();
    assert_eq!(
        meeting_attention_reason(&meeting, &host_pubkey),
        Some(MeetingAttentionReason::HostActionBlocked)
    );

    meeting.lifecycle = MeetingLifecycle::Aborted;
    assert_eq!(
        meeting_attention_reason(&meeting, &host_pubkey),
        Some(MeetingAttentionReason::MeetingAborted)
    );
}

#[test]
fn meeting_list_attention_does_not_expose_another_participants_floor_work() {
    let host = Keys::generate();
    let participant = Keys::generate();
    let observer = Keys::generate();
    let participant_pubkey = participant.public_key().to_hex();
    let observer_pubkey = observer.public_key().to_hex();
    let mut meeting = snapshot(&host, &participant);
    meeting.participants[1].participant_type = MeetingParticipantType::Human;
    meeting.participants.push(MeetingParticipant {
        pubkey: observer_pubkey.clone(),
        participant_type: MeetingParticipantType::Human,
        channel_role: "member".to_string(),
    });
    meeting.current_offer_pubkey = Some(participant_pubkey.clone());
    assert_eq!(
        meeting_attention_reason(&meeting, &participant_pubkey),
        Some(MeetingAttentionReason::FloorOffer)
    );
    assert_eq!(meeting_attention_reason(&meeting, &observer_pubkey), None);

    meeting.current_offer_pubkey = None;
    meeting.current_speaker_pubkey = Some(participant_pubkey.clone());
    assert_eq!(
        meeting_attention_reason(&meeting, &participant_pubkey),
        Some(MeetingAttentionReason::FloorGrant)
    );

    meeting.authoritative_updated_at = 42;
    let item = list_item_from_load(
        MEETING_ID.to_string(),
        Ok(MeetingLoadResult::Ready {
            snapshot: Box::new(meeting),
        }),
        &participant_pubkey,
    );
    assert!(item.needs_attention);
    assert_eq!(
        item.attention_reason,
        Some(MeetingAttentionReason::FloorGrant)
    );
    assert_eq!(item.updated_at, Some(42));
}

fn has_tag(event: &nostr::Event, key: &str, value: &str) -> bool {
    event.tags.iter().any(|tag| {
        let values = tag.as_slice();
        values.first().map(String::as_str) == Some(key)
            && values.get(1).map(String::as_str) == Some(value)
    })
}

fn desktop_action_input(action: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "submissionId": "00000000-0000-4000-8000-000000000010",
        "meetingId": MEETING_ID,
        "expectedControlToken": object_id(2),
        "action": action,
    })
}

#[test]
fn desktop_action_contract_accepts_every_camel_case_variant() {
    let cases = [
        (serde_json::json!({"type": "begin"}), "begin"),
        (
            serde_json::json!({
                "type": "block",
                "reasonCode": "external_state_conflict",
                "reason": "Project state changed",
            }),
            "block",
        ),
        (serde_json::json!({"type": "retry"}), "retry"),
        (
            serde_json::json!({"type": "return_to_board"}),
            "return_to_board",
        ),
        (serde_json::json!({"type": "confirm"}), "confirm"),
    ];

    for (action, expected_name) in cases {
        let input: MeetingActionFinalizationInput =
            serde_json::from_value(desktop_action_input(action))
                .unwrap_or_else(|error| panic!("deserialize {expected_name}: {error}"));
        assert_eq!(input.action.name(), expected_name);
    }

    assert!(
        serde_json::from_value::<MeetingActionFinalizationInput>(desktop_action_input(
            serde_json::json!({"type": "block", "reason_code": "provider_failure"})
        ))
        .is_err()
    );
    assert!(
        serde_json::from_value::<MeetingActionFinalizationInput>(desktop_action_input(
            serde_json::json!({"type": "block", "reasonCode": "affinity_lost"})
        ))
        .is_err()
    );
}

#[test]
fn desktop_action_result_contract_serializes_camel_case_fields() {
    let accepted = serde_json::to_value(MeetingActionFinalizationResult::Accepted {
        meeting_id: MEETING_ID.to_string(),
        event_id: object_id(40),
        action: "confirm".to_string(),
        state_revision: Some(22),
        duplicate: false,
    })
    .unwrap_or_else(|error| panic!("serialize accepted Action result: {error}"));
    assert_eq!(accepted["meetingId"], MEETING_ID);
    assert_eq!(accepted["stateRevision"], 22);
    assert!(accepted.get("state_revision").is_none());

    let indeterminate = serde_json::to_value(MeetingActionFinalizationResult::Indeterminate {
        meeting_id: MEETING_ID.to_string(),
        event_id: object_id(41),
        action: "confirm".to_string(),
        message: "retry exact Action command".to_string(),
    })
    .unwrap_or_else(|error| panic!("serialize indeterminate Action result: {error}"));
    assert_eq!(indeterminate["eventId"], object_id(41));
    assert!(indeterminate.get("event_id").is_none());
}

fn pending_action_command() -> PendingMeetingCommand {
    let keys = Keys::generate();
    let event = nostr::EventBuilder::new(nostr::Kind::TextNote, "pending action command")
        .sign_with_keys(&keys)
        .unwrap_or_else(|error| panic!("sign pending action command: {error}"));
    PendingMeetingCommand {
        event,
        api_base_url: "http://localhost:3000".to_string(),
        signer_pubkey: keys.public_key().to_hex(),
        meeting_id: MEETING_ID.to_string(),
        fingerprint: "fingerprint".to_string(),
        action: "confirm".to_string(),
    }
}

fn completion_response(
    pending: &PendingMeetingCommand,
    terminal_outcome: serde_json::Value,
) -> SubmitEventResponse {
    SubmitEventResponse {
        event_id: pending.event.id.to_hex(),
        accepted: true,
        message: serde_json::json!({
            "meeting_id": MEETING_ID,
            "status": "ended",
            "already_ended": true,
            "terminal_outcome": terminal_outcome,
        })
        .to_string(),
    }
}

#[test]
fn action_completion_distinguishes_conflict_from_unverifiable_receipt() {
    let pending = pending_action_command();
    let confirm = MeetingActionFinalizationAction::Confirm;
    assert!(validate_receipt(
        &completion_response(&pending, serde_json::json!("closed")),
        &pending,
        &confirm,
    )
    .is_ok());
    assert!(matches!(
        validate_receipt(
            &completion_response(&pending, serde_json::json!("aborted")),
            &pending,
            &confirm,
        ),
        Err(ReceiptValidationError::CanonicalConflict(_))
    ));
    assert!(matches!(
        validate_receipt(
            &completion_response(&pending, serde_json::Value::Null),
            &pending,
            &confirm,
        ),
        Err(ReceiptValidationError::Unverifiable(_))
    ));
}

#[test]
fn block_text_is_bounded_and_normalized() {
    let normalized = normalize_action(MeetingActionFinalizationAction::Block {
        reason_code: ActionBlockReasonInput::ExternalOperationFailed,
        reason: Some("  Project service rejected the write  ".to_string()),
    })
    .unwrap_or_else(|error| panic!("normalize action block: {error}"));
    assert!(matches!(
        normalized,
        MeetingActionFinalizationAction::Block { reason: Some(reason), .. }
            if reason == "Project service rejected the write"
    ));
    assert!(normalize_action(MeetingActionFinalizationAction::Block {
        reason_code: ActionBlockReasonInput::ToolUnavailable,
        reason: Some("line one\nline two".to_string()),
    })
    .is_err());
}

#[test]
fn authority_keeps_discussion_and_action_conditions_separate() {
    let host = Keys::generate();
    let participant = Keys::generate();
    let mut current = snapshot(&host, &participant);
    assert!(validate_action_authority(&MeetingActionFinalizationAction::Begin, &current).is_ok());
    current.phase = "moderator_control".to_string();
    assert!(validate_action_authority(&MeetingActionFinalizationAction::Begin, &current).is_err());
    current.phase = "moderator_idle".to_string();
    assert!(
        validate_action_authority(&MeetingActionFinalizationAction::Confirm, &current).is_err()
    );

    enter_action_phase(&mut current, "runnable");
    assert!(validate_action_authority(&MeetingActionFinalizationAction::Begin, &current).is_err());
    assert!(validate_action_authority(&MeetingActionFinalizationAction::Confirm, &current).is_ok());
    assert!(validate_action_authority(
        &MeetingActionFinalizationAction::Block {
            reason_code: ActionBlockReasonInput::ExternalStateConflict,
            reason: None,
        },
        &current,
    )
    .is_ok());
    assert!(validate_action_authority(&MeetingActionFinalizationAction::Retry, &current).is_err());

    let action = current
        .action
        .as_mut()
        .unwrap_or_else(|| panic!("action projection"));
    action.condition = "blocked".to_string();
    action.action_deadline_at_ms = None;
    action.last_error_code = Some("external_state_conflict".to_string());
    assert!(validate_action_authority(&MeetingActionFinalizationAction::Retry, &current).is_ok());
    assert!(
        validate_action_authority(&MeetingActionFinalizationAction::Confirm, &current).is_err()
    );
    assert!(
        validate_action_authority(&MeetingActionFinalizationAction::ReturnToBoard, &current,)
            .is_ok()
    );
}

#[test]
fn builders_derive_begin_run_and_completion_fences_from_snapshot() {
    let host = Keys::generate();
    let participant = Keys::generate();
    let mut current = snapshot(&host, &participant);
    let session_id =
        Uuid::parse_str(MEETING_ID).unwrap_or_else(|error| panic!("Meeting ID: {error}"));
    let begin = build_event(
        &MeetingActionFinalizationAction::Begin,
        &current,
        session_id,
    )
    .unwrap_or_else(|error| panic!("build action begin: {error}"))
    .sign_with_keys(&host)
    .unwrap_or_else(|error| panic!("sign action begin: {error}"));
    assert!(has_tag(&begin, "action", "begin"));
    assert!(has_tag(&begin, "expected-control-epoch", "7"));
    assert!(has_tag(&begin, "board-window", "5"));
    assert!(has_tag(&begin, "expected-state", &object_id(3)));
    assert!(has_tag(&begin, "board", &object_id(4)));

    enter_action_phase(&mut current, "runnable");
    let block = build_event(
        &MeetingActionFinalizationAction::Block {
            reason_code: ActionBlockReasonInput::ProviderFailure,
            reason: Some("Provider is unavailable".to_string()),
        },
        &current,
        session_id,
    )
    .unwrap_or_else(|error| panic!("build action block: {error}"))
    .sign_with_keys(&host)
    .unwrap_or_else(|error| panic!("sign action block: {error}"));
    assert!(has_tag(&block, "action-run", ACTION_RUN_ID));
    assert!(has_tag(&block, "action-window", "2"));
    assert!(has_tag(&block, "board", &object_id(4)));
    assert!(has_tag(&block, "reason-code", "provider_failure"));

    let returned = build_event(
        &MeetingActionFinalizationAction::ReturnToBoard,
        &current,
        session_id,
    )
    .unwrap_or_else(|error| panic!("build return to Board: {error}"))
    .sign_with_keys(&host)
    .unwrap_or_else(|error| panic!("sign return to Board: {error}"));
    assert!(has_tag(&returned, "external-effects", "preserved"));

    let confirm = build_event(
        &MeetingActionFinalizationAction::Confirm,
        &current,
        session_id,
    )
    .unwrap_or_else(|error| panic!("build action completion: {error}"))
    .sign_with_keys(&host)
    .unwrap_or_else(|error| panic!("sign action completion: {error}"));
    assert!(has_tag(&confirm, "outcome", "closed"));
    assert!(has_tag(&confirm, "action-run", ACTION_RUN_ID));
    assert!(has_tag(&confirm, "action-window", "2"));
    assert!(has_tag(&confirm, "attestation", "actions-recorded"));
}
