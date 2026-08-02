use buzz_sdk::{
    build_meeting_v2_create, parse_meeting_v2_board_content, MeetingV2CreateParams,
    MEETING_V2_BOARD_FORMAT, MEETING_V2_POLICY, MEETING_V2_SCHEMA_VERSION,
};
use nostr::Keys;
use serde_json::Value;
use uuid::Uuid;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/meeting_v2_stage1_create.json"))
        .expect("parse Meeting V2 stage-one fixture")
}

fn event_tags(event: &nostr::Event) -> Vec<Vec<String>> {
    event
        .tags
        .iter()
        .map(|tag| tag.as_slice().iter().map(ToString::to_string).collect())
        .collect()
}

#[test]
fn meeting_v2_create_matches_stage_one_wire_fixture() {
    let fixture = fixture();
    assert_eq!(
        fixture["protocol"]["meeting_version"].as_str(),
        Some(MEETING_V2_SCHEMA_VERSION)
    );
    assert_eq!(
        fixture["protocol"]["floor_policy"].as_str(),
        Some(MEETING_V2_POLICY)
    );
    assert_eq!(
        fixture["protocol"]["board_format"].as_str(),
        Some(MEETING_V2_BOARD_FORMAT)
    );
    assert_eq!(
        fixture["protocol"]["kinds"],
        serde_json::json!({
            "create": buzz_sdk::kind::KIND_MEETING_CREATE,
            "board": buzz_sdk::kind::KIND_MEETING_BOARD,
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

    let event = build_meeting_v2_create(MeetingV2CreateParams {
        session_id,
        title: create["title"].as_str().expect("title"),
        description: create["description"].as_str(),
        source_channel_id: Some(source_channel_id),
        author_pubkey,
        participant_pubkeys: &participant_pubkeys,
        initial_board: create["initial_board"].as_str().expect("initial board"),
    })
    .expect("valid V2 Create")
    .sign_with_keys(&author_keys)
    .expect("sign V2 Create");

    assert_eq!(
        event.kind.as_u16(),
        buzz_sdk::kind::KIND_MEETING_CREATE as u16
    );
    assert_eq!(
        event_tags(&event),
        serde_json::from_value::<Vec<Vec<String>>>(create["expected_tags"].clone())
            .expect("fixture tags")
    );
    assert_eq!(
        serde_json::from_str::<Value>(&event.content).expect("board JSON"),
        create["expected_content"]
    );
    let parsed = parse_meeting_v2_board_content(&event.content).expect("strict board content");
    assert_eq!(parsed.format, MEETING_V2_BOARD_FORMAT);
    assert_eq!(parsed.body, create["initial_board"]);
}
