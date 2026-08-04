use buzz_sdk::{
    build_meeting_v1_create, build_meeting_v1_end, MeetingV1CreateParams, MeetingV1EndParams,
    MEETING_V1_POLICY, MEETING_V1_SCHEMA_VERSION,
};
use nostr::Keys;
use serde_json::Value;
use uuid::Uuid;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/meeting_v1_create_end_v1.json"))
        .expect("parse Meeting V1 Create/End fixture")
}

fn event_tags(event: &nostr::Event) -> Vec<Vec<String>> {
    event
        .tags
        .iter()
        .map(|tag| tag.as_slice().iter().map(ToString::to_string).collect())
        .collect()
}

fn fixture_tags(value: &Value) -> Vec<Vec<String>> {
    serde_json::from_value(value.clone()).expect("fixture tag arrays")
}

#[test]
fn meeting_v1_create_and_end_match_wire_fixture() {
    let fixture = fixture();
    assert_eq!(
        fixture["protocol"]["meeting_version"].as_str(),
        Some(MEETING_V1_SCHEMA_VERSION)
    );
    assert_eq!(
        fixture["protocol"]["floor_policy"].as_str(),
        Some(MEETING_V1_POLICY)
    );
    assert_eq!(
        fixture["protocol"]["kinds"],
        serde_json::json!({
            "create": buzz_sdk::kind::KIND_MEETING_CREATE,
            "end": buzz_sdk::kind::KIND_MEETING_END,
            "state": buzz_sdk::kind::KIND_MEETING_STATE,
            "speech_intent": buzz_sdk::kind::KIND_MEETING_SPEECH_INTENT,
            "moderator_command": buzz_sdk::kind::KIND_MEETING_MODERATOR_COMMAND,
            "human_floor_request": buzz_sdk::kind::KIND_MEETING_HUMAN_FLOOR_REQUEST,
            "offer_response": buzz_sdk::kind::KIND_MEETING_OFFER_RESPONSE,
            "grant_signal": buzz_sdk::kind::KIND_MEETING_GRANT_SIGNAL,
        })
    );

    let create = &fixture["create"];
    let session_id = Uuid::parse_str(create["session_id"].as_str().expect("session id"))
        .expect("valid session id");
    let source_channel_id = Uuid::parse_str(
        create["source_channel_id"]
            .as_str()
            .expect("source channel id"),
    )
    .expect("valid source channel id");
    let participant_pubkeys: Vec<&str> = create["participant_pubkeys"]
        .as_array()
        .expect("participant pubkeys")
        .iter()
        .map(|value| value.as_str().expect("participant pubkey"))
        .collect();
    let author_pubkey = create["author_pubkey"].as_str().expect("author pubkey");
    let author_keys =
        Keys::parse("0000000000000000000000000000000000000000000000000000000000000001")
            .expect("fixture author key");
    assert_eq!(author_keys.public_key().to_hex(), author_pubkey);

    let create_event = build_meeting_v1_create(MeetingV1CreateParams {
        session_id,
        title: create["title"].as_str().expect("title"),
        description: create["description"].as_str(),
        source_channel_id: Some(source_channel_id),
        author_pubkey,
        moderator_pubkey: create["moderator_pubkey"].as_str().expect("moderator"),
        participant_pubkeys: &participant_pubkeys,
    })
    .expect("valid V1 Create")
    .sign_with_keys(&author_keys)
    .expect("sign V1 Create");
    assert_eq!(
        create_event.kind.as_u16(),
        buzz_sdk::kind::KIND_MEETING_CREATE as u16
    );
    assert!(create_event.content.is_empty());
    assert_eq!(
        event_tags(&create_event),
        fixture_tags(&create["expected_tags"])
    );

    let end = &fixture["end"];
    let end_event = build_meeting_v1_end(MeetingV1EndParams {
        session_id,
        create_event_id: end["create_event_id"].as_str().expect("create event id"),
    })
    .expect("valid V1 End")
    .sign_with_keys(&author_keys)
    .expect("sign V1 End");
    assert_eq!(
        end_event.kind.as_u16(),
        buzz_sdk::kind::KIND_MEETING_END as u16
    );
    assert!(end_event.content.is_empty());
    assert_eq!(event_tags(&end_event), fixture_tags(&end["expected_tags"]));
}

#[test]
fn meeting_v1_builders_reject_fixture_invalid_cases() {
    let fixture = fixture();
    let invalid_create_cases: Vec<&str> = fixture["invalid_create_cases"]
        .as_array()
        .expect("invalid Create cases")
        .iter()
        .map(|value| value.as_str().expect("invalid Create case name"))
        .collect();
    assert_eq!(
        invalid_create_cases,
        [
            "nil_session",
            "self_source",
            "empty_roster",
            "duplicate_author",
            "moderator_outside_roster",
        ]
    );
    let invalid_end_cases: Vec<&str> = fixture["invalid_end_cases"]
        .as_array()
        .expect("invalid End cases")
        .iter()
        .map(|value| value.as_str().expect("invalid End case name"))
        .collect();
    assert_eq!(
        invalid_end_cases,
        ["nil_session", "malformed_create_event_id"]
    );

    let session_id = Uuid::new_v4();
    let author = "11".repeat(32);
    let participant = "22".repeat(32);
    let outsider = "33".repeat(32);

    assert!(build_meeting_v1_create(MeetingV1CreateParams {
        session_id: Uuid::nil(),
        title: "review",
        description: None,
        source_channel_id: None,
        author_pubkey: &author,
        moderator_pubkey: &author,
        participant_pubkeys: &[&participant],
    })
    .is_err());
    assert!(build_meeting_v1_create(MeetingV1CreateParams {
        session_id,
        title: "review",
        description: None,
        source_channel_id: Some(session_id),
        author_pubkey: &author,
        moderator_pubkey: &author,
        participant_pubkeys: &[&participant],
    })
    .is_err());
    assert!(build_meeting_v1_create(MeetingV1CreateParams {
        session_id,
        title: "review",
        description: None,
        source_channel_id: None,
        author_pubkey: &author,
        moderator_pubkey: &author,
        participant_pubkeys: &[],
    })
    .is_err());
    assert!(build_meeting_v1_create(MeetingV1CreateParams {
        session_id,
        title: "review",
        description: None,
        source_channel_id: None,
        author_pubkey: &author,
        moderator_pubkey: &author,
        participant_pubkeys: &[&author],
    })
    .is_err());
    assert!(build_meeting_v1_create(MeetingV1CreateParams {
        session_id,
        title: "review",
        description: None,
        source_channel_id: None,
        author_pubkey: &author,
        moderator_pubkey: &outsider,
        participant_pubkeys: &[&participant],
    })
    .is_err());
    assert!(build_meeting_v1_end(MeetingV1EndParams {
        session_id: Uuid::nil(),
        create_event_id: &"44".repeat(32),
    })
    .is_err());
    assert!(build_meeting_v1_end(MeetingV1EndParams {
        session_id,
        create_event_id: "not-an-event-id",
    })
    .is_err());
}
