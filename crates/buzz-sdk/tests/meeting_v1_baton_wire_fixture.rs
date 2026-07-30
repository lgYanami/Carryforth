use buzz_sdk::{
    build_meeting_v1_grant_progress, build_meeting_v1_grant_yield,
    build_meeting_v1_human_floor_request, build_meeting_v1_human_floor_withdraw,
    build_meeting_v1_intent_refresh, build_meeting_v1_intent_submit,
    build_meeting_v1_intent_withdraw, build_meeting_v1_moderator_dismiss_handoff,
    build_meeting_v1_moderator_recall, build_meeting_v1_moderator_reject,
    build_meeting_v1_moderator_select, build_meeting_v1_offer_ack, build_meeting_v1_offer_decline,
    build_meeting_v1_speech, MeetingV1DirectedHandoff, MeetingV1GrantProgressParams,
    MeetingV1GrantYieldParams, MeetingV1GrantYieldReason, MeetingV1HandoffDismissReason,
    MeetingV1HandoffType, MeetingV1HumanFloorRequestParams, MeetingV1HumanFloorWithdrawParams,
    MeetingV1IntentDeferral, MeetingV1IntentRefreshParams, MeetingV1IntentRejectionReason,
    MeetingV1IntentSubmitParams, MeetingV1IntentWithdrawParams,
    MeetingV1ModeratorDismissHandoffParams, MeetingV1ModeratorRecallParams,
    MeetingV1ModeratorRejectParams, MeetingV1ModeratorSelectParams, MeetingV1OfferAckParams,
    MeetingV1OfferDeclineParams, MeetingV1ProgressStage, MeetingV1Selection, MeetingV1SpeechParams,
};
use nostr::{Event, EventBuilder, Keys};
use serde_json::Value;
use uuid::Uuid;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/meeting_v1_baton_commands_v1.json"))
        .expect("parse Meeting V1 Baton fixture")
}

fn event(builder: EventBuilder) -> Event {
    builder
        .sign_with_keys(&Keys::generate())
        .expect("sign fixture event")
}

fn event_tags(event: &Event) -> Value {
    serde_json::to_value(
        event
            .tags
            .iter()
            .map(|tag| tag.as_slice())
            .collect::<Vec<_>>(),
    )
    .expect("serialize tags")
}

fn assert_shape(builder: EventBuilder, fixture: &Value, key: &str, kind: u32) {
    let event = event(builder);
    assert_eq!(event.kind.as_u16(), kind as u16, "{key} kind");
    assert_eq!(
        event_tags(&event),
        fixture[key]["expected_tags"],
        "{key} tags"
    );
    let expected_content = fixture[key].get("content").unwrap_or(&Value::Null);
    if expected_content.is_null() {
        assert!(event.content.is_empty(), "{key} content");
    } else if expected_content.is_object() {
        assert_eq!(
            serde_json::from_str::<Value>(&event.content).expect("JSON command content"),
            *expected_content,
            "{key} content"
        );
    } else {
        assert_eq!(event.content, expected_content.as_str().unwrap_or_default());
    }
}

#[test]
fn meeting_v1_baton_builders_match_wire_fixture() {
    let fixture = fixture();
    let session_id =
        Uuid::parse_str(fixture["session_id"].as_str().expect("session id")).expect("UUID");
    let intent = fixture["ids"]["intent"].as_str().expect("intent");
    let previous = fixture["ids"]["previous"].as_str().expect("previous");
    let handoff = fixture["ids"]["handoff"].as_str().expect("handoff");
    let offer = fixture["ids"]["offer"].as_str().expect("offer");
    let grant = fixture["ids"]["grant"].as_str().expect("grant");
    let participant = fixture["ids"]["participant"].as_str().expect("participant");

    assert_shape(
        build_meeting_v1_intent_submit(MeetingV1IntentSubmitParams {
            session_id,
            basis_speech_revision: 4,
            addressed_to: Some(participant),
            summary: fixture["intent_submit"]["content"]
                .as_str()
                .expect("submit content"),
        })
        .expect("Submit"),
        &fixture,
        "intent_submit",
        buzz_sdk::kind::KIND_MEETING_SPEECH_INTENT,
    );
    assert_shape(
        build_meeting_v1_intent_refresh(MeetingV1IntentRefreshParams {
            session_id,
            intent_id: intent,
            previous_event_id: previous,
            basis_speech_revision: 5,
            addressed_to: None,
            summary: fixture["intent_refresh"]["content"]
                .as_str()
                .expect("refresh content"),
        })
        .expect("Refresh"),
        &fixture,
        "intent_refresh",
        buzz_sdk::kind::KIND_MEETING_SPEECH_INTENT,
    );
    assert_shape(
        build_meeting_v1_intent_withdraw(MeetingV1IntentWithdrawParams {
            session_id,
            intent_id: intent,
            previous_event_id: previous,
        })
        .expect("Withdraw"),
        &fixture,
        "intent_withdraw",
        buzz_sdk::kind::KIND_MEETING_SPEECH_INTENT,
    );
    assert_shape(
        build_meeting_v1_moderator_select(MeetingV1ModeratorSelectParams {
            session_id,
            selection: MeetingV1Selection::Intent { intent_id: intent },
            expected_control_epoch: 3,
            expected_decision_epoch: 5,
            expected_intent_revision: 7,
            expected_speech_revision: 4,
            selection_reason: Some("Highest-impact risk."),
            deferrals: &[],
            attempt_id: None,
            expected_source_event_id: None,
        })
        .expect("Select Intent"),
        &fixture,
        "moderator_select_intent",
        buzz_sdk::kind::KIND_MEETING_MODERATOR_COMMAND,
    );
    assert_shape(
        build_meeting_v1_moderator_select(MeetingV1ModeratorSelectParams {
            session_id,
            selection: MeetingV1Selection::Handoff {
                handoff_id: handoff,
                expected_attempt_count: 2,
            },
            expected_control_epoch: 3,
            expected_decision_epoch: 5,
            expected_intent_revision: 7,
            expected_speech_revision: 4,
            selection_reason: None,
            deferrals: &[],
            attempt_id: None,
            expected_source_event_id: None,
        })
        .expect("Select Handoff"),
        &fixture,
        "moderator_select_handoff",
        buzz_sdk::kind::KIND_MEETING_MODERATOR_COMMAND,
    );
    assert_shape(
        build_meeting_v1_moderator_reject(MeetingV1ModeratorRejectParams {
            session_id,
            intent_id: intent,
            previous_event_id: previous,
            intent_author_pubkey: participant,
            reason_code: MeetingV1IntentRejectionReason::Duplicate,
            reason_text: fixture["moderator_reject"]["content"]
                .as_str()
                .expect("reject content"),
            attempt_id: None,
        })
        .expect("Reject"),
        &fixture,
        "moderator_reject",
        buzz_sdk::kind::KIND_MEETING_MODERATOR_COMMAND,
    );
    assert_shape(
        build_meeting_v1_moderator_dismiss_handoff(MeetingV1ModeratorDismissHandoffParams {
            session_id,
            handoff_id: handoff,
            expected_speech_revision: 4,
            expected_attempt_count: 2,
            reason_code: MeetingV1HandoffDismissReason::AnsweredElsewhere,
            reason_text: fixture["moderator_dismiss_handoff"]["content"]
                .as_str()
                .expect("dismiss content"),
            attempt_id: None,
        })
        .expect("Dismiss"),
        &fixture,
        "moderator_dismiss_handoff",
        buzz_sdk::kind::KIND_MEETING_MODERATOR_COMMAND,
    );
    assert_shape(
        build_meeting_v1_moderator_recall(MeetingV1ModeratorRecallParams {
            session_id,
            control_epoch: 3,
            reason: fixture["moderator_recall"]["content"].as_str(),
        })
        .expect("Recall"),
        &fixture,
        "moderator_recall",
        buzz_sdk::kind::KIND_MEETING_MODERATOR_COMMAND,
    );
    assert_shape(
        build_meeting_v1_human_floor_request(MeetingV1HumanFloorRequestParams { session_id })
            .expect("Human Request"),
        &fixture,
        "human_request",
        buzz_sdk::kind::KIND_MEETING_HUMAN_FLOOR_REQUEST,
    );
    assert_shape(
        build_meeting_v1_human_floor_withdraw(MeetingV1HumanFloorWithdrawParams {
            session_id,
            request_id: previous,
        })
        .expect("Human Withdraw"),
        &fixture,
        "human_withdraw",
        buzz_sdk::kind::KIND_MEETING_HUMAN_FLOOR_REQUEST,
    );
    assert_shape(
        build_meeting_v1_offer_ack(MeetingV1OfferAckParams {
            session_id,
            offer_id: offer,
        })
        .expect("Offer ACK"),
        &fixture,
        "offer_ack",
        buzz_sdk::kind::KIND_MEETING_OFFER_RESPONSE,
    );
    assert_shape(
        build_meeting_v1_offer_decline(MeetingV1OfferDeclineParams {
            session_id,
            offer_id: offer,
            reason: fixture["offer_decline"]["content"].as_str(),
        })
        .expect("Offer Decline"),
        &fixture,
        "offer_decline",
        buzz_sdk::kind::KIND_MEETING_OFFER_RESPONSE,
    );
    assert_shape(
        build_meeting_v1_grant_progress(MeetingV1GrantProgressParams {
            session_id,
            grant_id: grant,
            progress_seq: 3,
            stage: MeetingV1ProgressStage::ToolUse,
        })
        .expect("Grant Progress"),
        &fixture,
        "grant_progress",
        buzz_sdk::kind::KIND_MEETING_GRANT_SIGNAL,
    );
    assert_shape(
        build_meeting_v1_grant_yield(MeetingV1GrantYieldParams {
            session_id,
            grant_id: grant,
            reason_code: Some(MeetingV1GrantYieldReason::InsufficientContext),
            reason: fixture["grant_yield"]["content"].as_str(),
        })
        .expect("Grant Yield"),
        &fixture,
        "grant_yield",
        buzz_sdk::kind::KIND_MEETING_GRANT_SIGNAL,
    );
    assert_shape(
        build_meeting_v1_speech(MeetingV1SpeechParams {
            session_id,
            grant_id: grant,
            speech_revision: 5,
            content: fixture["speech"]["content"].as_str().expect("speech"),
            mentions: &[participant],
            handoff: Some(MeetingV1DirectedHandoff {
                target_pubkey: participant,
                handoff_type: MeetingV1HandoffType::Review,
                reason: "Please validate the rollback plan.",
            }),
        })
        .expect("Speech"),
        &fixture,
        "speech",
        buzz_sdk::kind::KIND_STREAM_MESSAGE,
    );
}

#[test]
fn meeting_v1_baton_builders_reject_invalid_wire_values() {
    let session_id = Uuid::new_v4();
    let id = "aa".repeat(32);
    let other = "bb".repeat(32);
    let deferral = MeetingV1IntentDeferral {
        intent_id: &other,
        previous_event_id: &id,
        reason: "Later.",
    };

    assert!(build_meeting_v1_intent_submit(MeetingV1IntentSubmitParams {
        session_id: Uuid::nil(),
        basis_speech_revision: 0,
        addressed_to: None,
        summary: "Valid",
    })
    .is_err());
    assert!(build_meeting_v1_intent_submit(MeetingV1IntentSubmitParams {
        session_id,
        basis_speech_revision: 0,
        addressed_to: None,
        summary: " leading whitespace",
    })
    .is_err());
    assert!(
        build_meeting_v1_intent_refresh(MeetingV1IntentRefreshParams {
            session_id,
            intent_id: "bad",
            previous_event_id: &id,
            basis_speech_revision: 0,
            addressed_to: None,
            summary: "Valid",
        })
        .is_err()
    );
    assert!(
        build_meeting_v1_moderator_select(MeetingV1ModeratorSelectParams {
            session_id,
            selection: MeetingV1Selection::Handoff {
                handoff_id: &id,
                expected_attempt_count: 0,
            },
            expected_control_epoch: 1,
            expected_decision_epoch: 0,
            expected_intent_revision: 0,
            expected_speech_revision: 0,
            selection_reason: None,
            deferrals: &[deferral],
            attempt_id: None,
            expected_source_event_id: None,
        })
        .is_err()
    );
    assert!(
        build_meeting_v1_moderator_recall(MeetingV1ModeratorRecallParams {
            session_id,
            control_epoch: 0,
            reason: None,
        })
        .is_err()
    );
    assert!(
        build_meeting_v1_grant_progress(MeetingV1GrantProgressParams {
            session_id,
            grant_id: &id,
            progress_seq: 0,
            stage: MeetingV1ProgressStage::Generating,
        })
        .is_err()
    );
    assert!(
        build_meeting_v1_grant_progress(MeetingV1GrantProgressParams {
            session_id,
            grant_id: &id,
            progress_seq: (i64::MAX as u64) + 1,
            stage: MeetingV1ProgressStage::Generating,
        })
        .is_err()
    );
    assert!(build_meeting_v1_speech(MeetingV1SpeechParams {
        session_id,
        grant_id: &id,
        speech_revision: 0,
        content: "Speech",
        mentions: &[],
        handoff: None,
    })
    .is_err());
    assert!(build_meeting_v1_speech(MeetingV1SpeechParams {
        session_id,
        grant_id: &id,
        speech_revision: 1,
        content: "Speech",
        mentions: &[&other, &other],
        handoff: None,
    })
    .is_err());
}
