use super::super::{
    MeetingBoard, MeetingBoardControl, MeetingBoardSource, MeetingHostState, MeetingOpenHandoff,
    MeetingParticipant, MeetingParticipantType, MeetingPendingIntent,
};
use super::*;
use crate::relay::SubmitEventResponse;
use nostr::Keys;

const MEETING_ID: &str = "00000000-0000-4000-8000-000000000001";

fn object_id(byte: u8) -> String {
    format!("{byte:02x}").repeat(32)
}

fn snapshot(host: &Keys, participant: &Keys) -> MeetingSnapshot {
    let host_pubkey = host.public_key().to_hex();
    let participant_pubkey = participant.public_key().to_hex();
    MeetingSnapshot {
        meeting_id: MEETING_ID.to_string(),
        title: "Design review".to_string(),
        description: None,
        summary: None,
        source_channel_id: None,
        schema_version: 3,
        policy: buzz_sdk_pkg::MEETING_V2_ACTIONS_POLICY.to_string(),
        host_pubkey: host_pubkey.clone(),
        moderator_pubkey: host_pubkey.clone(),
        create_event_id: object_id(1),
        created_at: 1,
        lifecycle: MeetingLifecycle::Active,
        phase: "moderator_control".to_string(),
        state_revision: 12,
        floor_revision: 8,
        intent_revision: 3,
        speech_revision: 4,
        current_speaker_pubkey: None,
        current_offer_pubkey: None,
        floor: None,
        host: Some(MeetingHostState {
            control_token: object_id(2),
            state_event_id: object_id(3),
            control_epoch: 7,
            decision_epoch: 9,
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
                board_deadline_at_ms: Some(31_000),
                board_completed_at_ms: Some(2_000),
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
                pubkey: participant_pubkey,
                participant_type: MeetingParticipantType::Agent,
                channel_role: "member".to_string(),
            },
        ],
        board: MeetingBoard {
            event_id: object_id(4),
            format: "markdown".to_string(),
            body: "# Goal".to_string(),
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

fn pending_intent(author_pubkey: String, byte: u8) -> MeetingPendingIntent {
    MeetingPendingIntent {
        intent_id: object_id(byte),
        current_event_id: object_id(byte.saturating_add(1)),
        author_pubkey,
        basis_speech_revision: 4,
        summary: "Discuss the remaining risk".to_string(),
        addressed_to: None,
        created_at_ms: 1_000,
        deferred: false,
        selection_attempt_count: 0,
        last_offer_id: None,
        last_attempt_outcome: None,
        eligible_decision_epoch: 9,
        selectable: true,
    }
}

fn open_handoff(host_pubkey: String, participant_pubkey: String) -> MeetingOpenHandoff {
    MeetingOpenHandoff {
        handoff_id: object_id(20),
        source_speech_event_id: object_id(21),
        from_pubkey: participant_pubkey,
        to_pubkey: host_pubkey,
        reason_type: "question".to_string(),
        reason_text: "Can the host clarify the constraint?".to_string(),
        created_at_ms: 1_000,
        attempt_count: 0,
        last_offer_id: None,
        last_grant_id: None,
        last_attempt_outcome: None,
        blocked_by: None,
        moderator_retry_blocked: false,
        eligible_decision_epoch: 9,
        attempt_active: false,
        selectable: true,
    }
}

fn has_tag(event: &nostr::Event, key: &str, value: &str) -> bool {
    event.tags.iter().any(|tag| {
        let values = tag.as_slice();
        values.first().map(String::as_str) == Some(key)
            && values.get(1).map(String::as_str) == Some(value)
    })
}

fn desktop_host_input(action: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "submissionId": "00000000-0000-4000-8000-000000000010",
        "meetingId": MEETING_ID,
        "expectedControlToken": object_id(2),
        "action": action,
    })
}

#[test]
fn desktop_host_action_contract_accepts_every_camel_case_variant() {
    let intent_id = object_id(10);
    let handoff_id = object_id(20);
    let addressee = object_id(30);
    let cases = [
        (
            serde_json::json!({"type": "board_update", "body": "# Board"}),
            "board_update",
        ),
        (
            serde_json::json!({"type": "board_unchanged"}),
            "board_unchanged",
        ),
        (
            serde_json::json!({
                "type": "intent_submit",
                "summary": "I should speak",
                "addressedTo": addressee,
            }),
            "intent_submit",
        ),
        (
            serde_json::json!({
                "type": "intent_refresh",
                "intentId": intent_id,
                "summary": "Updated intent",
                "addressedTo": addressee,
            }),
            "intent_refresh",
        ),
        (
            serde_json::json!({"type": "intent_withdraw", "intentId": intent_id}),
            "intent_withdraw",
        ),
        (
            serde_json::json!({
                "type": "select_intent",
                "intentId": intent_id,
                "selectionReason": "Best next speaker",
                "deferralReason": "Resolve this first",
            }),
            "select_intent",
        ),
        (
            serde_json::json!({
                "type": "select_handoff",
                "handoffId": handoff_id,
                "selectionReason": "Answer the open question",
            }),
            "select_handoff",
        ),
        (
            serde_json::json!({
                "type": "reject_intent",
                "intentId": intent_id,
                "reasonCode": "off_topic",
                "reason": "Outside the agenda",
            }),
            "reject_intent",
        ),
        (
            serde_json::json!({
                "type": "dismiss_handoff",
                "handoffId": handoff_id,
                "reasonCode": "no_longer_needed",
                "reason": "Resolved already",
            }),
            "dismiss_handoff",
        ),
        (
            serde_json::json!({"type": "recall", "reason": "Return to the agenda"}),
            "recall",
        ),
        (serde_json::json!({"type": "close"}), "close"),
        (
            serde_json::json!({
                "type": "abort",
                "reasonCode": "discussion_blocked",
                "reason": "Required evidence is unavailable",
            }),
            "abort",
        ),
    ];

    for (action, expected_name) in cases {
        let input: MeetingHostActionInput = serde_json::from_value(desktop_host_input(action))
            .unwrap_or_else(|error| panic!("deserialize {expected_name}: {error}"));
        assert_eq!(input.action.name(), expected_name);
    }

    assert!(
        serde_json::from_value::<MeetingHostActionInput>(desktop_host_input(
            serde_json::json!({"type": "select_intent", "intent_id": intent_id})
        ))
        .is_err()
    );
}

#[test]
fn desktop_host_result_contract_serializes_camel_case_fields() {
    let accepted = serde_json::to_value(MeetingHostActionResult::Accepted {
        meeting_id: MEETING_ID.to_string(),
        event_id: object_id(40),
        action: "select_intent".to_string(),
        canonical_object_id: Some(object_id(41)),
        state_revision: Some(12),
        duplicate: false,
    })
    .unwrap_or_else(|error| panic!("serialize accepted Host result: {error}"));
    assert_eq!(accepted["meetingId"], MEETING_ID);
    assert_eq!(accepted["stateRevision"], 12);
    assert!(accepted.get("canonicalObjectId").is_some());
    assert!(accepted.get("state_revision").is_none());

    let indeterminate = serde_json::to_value(MeetingHostActionResult::Indeterminate {
        meeting_id: MEETING_ID.to_string(),
        event_id: object_id(42),
        action: "select_intent".to_string(),
        message: "retry exact Host command".to_string(),
    })
    .unwrap_or_else(|error| panic!("serialize indeterminate Host result: {error}"));
    assert_eq!(indeterminate["eventId"], object_id(42));
    assert!(indeterminate.get("event_id").is_none());
}

fn pending_host_command(action: &str) -> PendingMeetingCommand {
    let keys = Keys::generate();
    let event = nostr::EventBuilder::new(nostr::Kind::TextNote, "pending host command")
        .sign_with_keys(&keys)
        .unwrap_or_else(|error| panic!("sign pending host command: {error}"));
    PendingMeetingCommand {
        event,
        api_base_url: "http://localhost:3000".to_string(),
        signer_pubkey: keys.public_key().to_hex(),
        meeting_id: MEETING_ID.to_string(),
        fingerprint: "fingerprint".to_string(),
        action: action.to_string(),
    }
}

fn end_response(
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
fn host_terminal_receipt_must_match_the_requested_outcome() {
    let pending = pending_host_command("close");
    let close = MeetingHostAction::Close;
    assert!(validate_receipt(
        &end_response(&pending, serde_json::json!("closed")),
        &pending,
        &close,
    )
    .is_ok());
    assert!(matches!(
        validate_receipt(
            &end_response(&pending, serde_json::json!("aborted")),
            &pending,
            &close,
        ),
        Err(ReceiptValidationError::CanonicalConflict(_))
    ));

    let abort = MeetingHostAction::Abort {
        reason_code: AbortReasonInput::DiscussionBlocked,
        reason: Some("Blocked".to_string()),
    };
    assert!(validate_receipt(
        &end_response(&pending, serde_json::json!("aborted")),
        &pending,
        &abort,
    )
    .is_ok());
    assert!(matches!(
        validate_receipt(
            &end_response(&pending, serde_json::json!("closed")),
            &pending,
            &abort,
        ),
        Err(ReceiptValidationError::CanonicalConflict(_))
    ));
    assert!(matches!(
        validate_receipt(
            &end_response(&pending, serde_json::Value::Null),
            &pending,
            &close,
        ),
        Err(ReceiptValidationError::Unverifiable(_))
    ));
}

#[test]
fn normalizes_closed_reason_vocabularies_and_board_without_trimming() {
    let board = normalize_action(MeetingHostAction::BoardUpdate {
        body: "  # Board\n".to_string(),
    })
    .unwrap_or_else(|error| panic!("normalize board: {error}"));
    assert!(matches!(
        board,
        MeetingHostAction::BoardUpdate { body } if body == "  # Board\n"
    ));

    let rejection = normalize_action(MeetingHostAction::RejectIntent {
        intent_id: "ab".repeat(32),
        reason_code: IntentRejectionReasonInput::OffTopic,
        reason: "  Not relevant  ".to_string(),
    })
    .unwrap_or_else(|error| panic!("normalize rejection: {error}"));
    assert!(matches!(
        rejection,
        MeetingHostAction::RejectIntent { reason, .. } if reason == "Not relevant"
    ));
}

#[test]
fn rejects_unbounded_or_multiline_control_text() {
    assert!(normalize_action(MeetingHostAction::BoardUpdate {
        body: "  \n".to_string(),
    })
    .is_err());
    assert!(normalize_action(MeetingHostAction::IntentSubmit {
        summary: "line one\nline two".to_string(),
        addressed_to: None,
    })
    .is_err());
    assert!(normalize_action(MeetingHostAction::Abort {
        reason_code: AbortReasonInput::DiscussionBlocked,
        reason: Some("x".repeat(1025)),
    })
    .is_err());
}

#[test]
fn authority_separates_board_and_floor_windows_and_rejects_self_addressee() {
    let keys = Keys::generate();
    let participant = Keys::generate();
    let mut snapshot = snapshot(&keys, &participant);
    let signer = keys.public_key().to_hex();

    let floor_host = snapshot
        .host
        .as_ref()
        .unwrap_or_else(|| panic!("host projection"));
    assert!(validate_action_authority(
        &MeetingHostAction::BoardUnchanged,
        &snapshot,
        floor_host,
        &signer,
    )
    .is_err());
    assert!(validate_action_authority(
        &MeetingHostAction::IntentSubmit {
            summary: "I should speak".to_string(),
            addressed_to: Some(signer.clone()),
        },
        &snapshot,
        floor_host,
        &signer,
    )
    .is_err());

    let board_host = snapshot
        .host
        .as_mut()
        .unwrap_or_else(|| panic!("host projection"));
    board_host.board_control.phase = "board_pending".to_string();
    board_host.can_select = false;
    board_host.can_close = false;
    let board_host = snapshot
        .host
        .as_ref()
        .unwrap_or_else(|| panic!("host projection"));
    assert!(validate_action_authority(
        &MeetingHostAction::BoardUnchanged,
        &snapshot,
        board_host,
        &signer,
    )
    .is_ok());
    assert!(
        validate_action_authority(&MeetingHostAction::Close, &snapshot, board_host, &signer,)
            .is_err()
    );
}

#[test]
fn self_intent_priority_and_consecutive_speech_deferral_are_enforced() {
    let keys = Keys::generate();
    let participant = Keys::generate();
    let mut snapshot = snapshot(&keys, &participant);
    let signer = keys.public_key().to_hex();
    let participant_pubkey = participant.public_key().to_hex();
    let self_intent = pending_intent(signer.clone(), 10);
    let other_intent = pending_intent(participant_pubkey, 12);
    let self_id = self_intent.intent_id.clone();
    let other_id = other_intent.intent_id.clone();
    let host = snapshot
        .host
        .as_mut()
        .unwrap_or_else(|| panic!("host projection"));
    host.consecutive_moderator_speeches = 1;
    host.pending_intents = vec![self_intent, other_intent];
    let host = snapshot
        .host
        .as_ref()
        .unwrap_or_else(|| panic!("host projection"));

    assert!(validate_action_authority(
        &MeetingHostAction::SelectIntent {
            intent_id: other_id,
            selection_reason: None,
            deferral_reason: None,
        },
        &snapshot,
        host,
        &signer,
    )
    .is_err());
    assert!(validate_action_authority(
        &MeetingHostAction::SelectIntent {
            intent_id: self_id.clone(),
            selection_reason: None,
            deferral_reason: None,
        },
        &snapshot,
        host,
        &signer,
    )
    .is_err());

    let select_self = MeetingHostAction::SelectIntent {
        intent_id: self_id,
        selection_reason: Some("I can resolve this blocker".to_string()),
        deferral_reason: Some("The blocker must be resolved first".to_string()),
    };
    assert!(validate_action_authority(&select_self, &snapshot, host, &signer).is_ok());
    let event = build_event(
        &select_self,
        &snapshot,
        host,
        Uuid::parse_str(MEETING_ID).unwrap_or_else(|error| panic!("Meeting ID: {error}")),
        &signer,
    )
    .unwrap_or_else(|error| panic!("build self selection: {error}"))
    .sign_with_keys(&keys)
    .unwrap_or_else(|error| panic!("sign self selection: {error}"));
    let content: serde_json::Value = serde_json::from_str(&event.content)
        .unwrap_or_else(|error| panic!("parse self selection: {error}"));
    assert_eq!(content["deferrals"].as_array().map(Vec::len), Some(1));
}

#[test]
fn asynchronous_pool_actions_preserve_owner_and_active_attempt_rules() {
    let keys = Keys::generate();
    let participant = Keys::generate();
    let mut snapshot = snapshot(&keys, &participant);
    let signer = keys.public_key().to_hex();
    let self_intent = pending_intent(signer.clone(), 10);
    let other_intent = pending_intent(participant.public_key().to_hex(), 12);
    let self_id = self_intent.intent_id.clone();
    let other_id = other_intent.intent_id.clone();
    let handoff = open_handoff(signer.clone(), participant.public_key().to_hex());
    let handoff_id = handoff.handoff_id.clone();
    let host = snapshot
        .host
        .as_mut()
        .unwrap_or_else(|| panic!("host projection"));
    host.pending_intents = vec![self_intent, other_intent];
    host.open_handoffs = vec![handoff];
    let host = snapshot
        .host
        .as_ref()
        .unwrap_or_else(|| panic!("host projection"));

    let rejection = |intent_id| MeetingHostAction::RejectIntent {
        intent_id,
        reason_code: IntentRejectionReasonInput::Duplicate,
        reason: "Already covered".to_string(),
    };
    assert!(
        validate_action_authority(&rejection(self_id.clone()), &snapshot, host, &signer).is_err()
    );
    assert!(validate_action_authority(&rejection(other_id), &snapshot, host, &signer).is_ok());
    assert!(validate_action_authority(
        &MeetingHostAction::IntentWithdraw { intent_id: self_id },
        &snapshot,
        host,
        &signer,
    )
    .is_ok());

    let dismiss = MeetingHostAction::DismissHandoff {
        handoff_id,
        reason_code: HandoffDismissReasonInput::NoLongerNeeded,
        reason: "Resolved in the Board".to_string(),
    };
    assert!(validate_action_authority(&dismiss, &snapshot, host, &signer).is_ok());
    let mut active_host = host.clone();
    active_host.open_handoffs[0].attempt_active = true;
    assert!(validate_action_authority(&dismiss, &snapshot, &active_host, &signer).is_err());
}

#[test]
fn native_builder_derives_board_and_selection_fences_from_snapshot() {
    let keys = Keys::generate();
    let participant = Keys::generate();
    let mut snapshot = snapshot(&keys, &participant);
    let signer = keys.public_key().to_hex();
    let intent = pending_intent(participant.public_key().to_hex(), 12);
    let intent_id = intent.intent_id.clone();
    snapshot
        .host
        .as_mut()
        .unwrap_or_else(|| panic!("host projection"))
        .pending_intents = vec![intent];
    let session_id =
        Uuid::parse_str(MEETING_ID).unwrap_or_else(|error| panic!("Meeting ID: {error}"));
    let host = snapshot
        .host
        .as_ref()
        .unwrap_or_else(|| panic!("host projection"));
    let selection = build_event(
        &MeetingHostAction::SelectIntent {
            intent_id,
            selection_reason: None,
            deferral_reason: None,
        },
        &snapshot,
        host,
        session_id,
        &signer,
    )
    .unwrap_or_else(|error| panic!("build selection: {error}"))
    .sign_with_keys(&keys)
    .unwrap_or_else(|error| panic!("sign selection: {error}"));
    assert!(has_tag(&selection, "expected-control-epoch", "7"));
    assert!(has_tag(&selection, "expected-decision-epoch", "9"));
    assert!(has_tag(&selection, "expected-intent-revision", "3"));
    assert!(has_tag(&selection, "expected-speech-revision", "4"));

    let mut board_snapshot = snapshot.clone();
    board_snapshot
        .host
        .as_mut()
        .unwrap_or_else(|| panic!("host projection"))
        .board_control
        .phase = "board_pending".to_string();
    let board_host = board_snapshot
        .host
        .as_ref()
        .unwrap_or_else(|| panic!("host projection"));
    let board = build_event(
        &MeetingHostAction::BoardUpdate {
            body: "# Updated Board".to_string(),
        },
        &board_snapshot,
        board_host,
        session_id,
        &signer,
    )
    .unwrap_or_else(|error| panic!("build Board update: {error}"))
    .sign_with_keys(&keys)
    .unwrap_or_else(|error| panic!("sign Board update: {error}"));
    assert!(has_tag(&board, "expected-control-epoch", "7"));
    assert!(has_tag(&board, "board-window", "5"));
}
