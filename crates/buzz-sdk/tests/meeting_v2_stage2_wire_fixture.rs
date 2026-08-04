use buzz_sdk::{
    build_meeting_v2_board_action, build_meeting_v2_end, build_meeting_v2_intent_submit,
    MeetingV1IntentSubmitParams, MeetingV2BoardActionParams, MeetingV2EndOutcome,
    MeetingV2EndParams,
};
use nostr::Keys;
use serde_json::Value;
use uuid::Uuid;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/meeting_v2_stage2_commands.json"))
        .expect("parse Meeting V2 stage-two fixture")
}

fn event_tags(event: &nostr::Event) -> Vec<Vec<String>> {
    event
        .tags
        .iter()
        .map(|tag| tag.as_slice().iter().map(ToString::to_string).collect())
        .collect()
}

fn fixture_tags(value: &Value) -> Vec<Vec<String>> {
    serde_json::from_value(value.clone()).expect("parse fixture tags")
}

fn sign(builder: nostr::EventBuilder) -> nostr::Event {
    builder
        .sign_with_keys(
            &Keys::parse("0000000000000000000000000000000000000000000000000000000000000001")
                .expect("fixture key"),
        )
        .expect("sign fixture event")
}

#[test]
fn board_update_and_unchanged_match_stage_two_fixture() {
    let fixture = fixture();
    let session_id = Uuid::parse_str(fixture["session_id"].as_str().expect("session id"))
        .expect("valid session id");
    let params = |board| MeetingV2BoardActionParams {
        session_id,
        expected_control_epoch: fixture["board"]["control_epoch"]
            .as_u64()
            .expect("control epoch"),
        board_window: fixture["board"]["board_window"]
            .as_u64()
            .expect("board window"),
        board,
    };

    let update = sign(
        build_meeting_v2_board_action(params(fixture["board"]["body"].as_str()))
            .expect("build Board update"),
    );
    assert_eq!(
        update.kind.as_u16() as u32,
        buzz_sdk::kind::KIND_MEETING_BOARD_COMMAND
    );
    assert_eq!(
        event_tags(&update),
        fixture_tags(&fixture["board"]["update_tags"])
    );
    assert_eq!(
        serde_json::from_str::<Value>(&update.content).expect("Board envelope"),
        serde_json::json!({
            "format": buzz_sdk::MEETING_V2_BOARD_FORMAT,
            "body": fixture["board"]["body"],
        })
    );

    let unchanged =
        sign(build_meeting_v2_board_action(params(None)).expect("build Board unchanged"));
    assert_eq!(
        event_tags(&unchanged),
        fixture_tags(&fixture["board"]["unchanged_tags"])
    );
    assert!(unchanged.content.is_empty());
}

#[test]
fn moderated_command_and_terminal_outcomes_match_stage_two_fixture() {
    let fixture = fixture();
    let session_id = Uuid::parse_str(fixture["session_id"].as_str().expect("session id"))
        .expect("valid session id");
    let create_event_id = fixture["create_event_id"]
        .as_str()
        .expect("Create event id");

    let intent = sign(
        build_meeting_v2_intent_submit(MeetingV1IntentSubmitParams {
            session_id,
            basis_speech_revision: 2,
            addressed_to: None,
            summary: fixture["intent"]["summary"].as_str().expect("summary"),
        })
        .expect("build V2 Intent"),
    );
    assert_eq!(
        event_tags(&intent),
        fixture_tags(&fixture["intent"]["tags"])
    );

    let close = sign(
        build_meeting_v2_end(MeetingV2EndParams {
            session_id,
            create_event_id,
            outcome: MeetingV2EndOutcome::Closed,
            reason_code: None,
            reason: None,
        })
        .expect("build V2 close"),
    );
    assert_eq!(event_tags(&close), fixture_tags(&fixture["close_tags"]));
    assert!(close.content.is_empty());

    let abort = sign(
        build_meeting_v2_end(MeetingV2EndParams {
            session_id,
            create_event_id,
            outcome: MeetingV2EndOutcome::Aborted,
            reason_code: fixture["abort"]["reason_code"].as_str(),
            reason: fixture["abort"]["reason"].as_str(),
        })
        .expect("build V2 abort"),
    );
    assert_eq!(event_tags(&abort), fixture_tags(&fixture["abort"]["tags"]));
    assert_eq!(abort.content, fixture["abort"]["reason"]);
}
