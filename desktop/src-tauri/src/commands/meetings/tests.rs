//! Focused integrity tests for the Desktop Meeting read boundary.

use nostr::{EventBuilder, Keys, Kind, Tag};

use super::*;

const TEST_MEETING_ID: &str = "00000000-0000-4000-8000-000000000001";

fn tag<const N: usize>(values: [&str; N]) -> Tag {
    Tag::parse(values).unwrap_or_else(|error| panic!("parse test tag: {error}"))
}

fn test_create(host: &Keys, participant: &Keys) -> (Event, CreateProjection) {
    let meeting_id = Uuid::parse_str(TEST_MEETING_ID)
        .unwrap_or_else(|error| panic!("parse test Meeting ID: {error}"));
    let host_pubkey = host.public_key().to_hex();
    let participant_pubkey = participant.public_key().to_hex();
    let participant_refs = [participant_pubkey.as_str()];
    let event =
        buzz_sdk_pkg::build_meeting_v2_actions_create(buzz_sdk_pkg::MeetingV2CreateParams {
            session_id: meeting_id,
            title: "Design review",
            description: Some("Verify the Desktop read model"),
            source_channel_id: None,
            author_pubkey: &host_pubkey,
            participant_pubkeys: &participant_refs,
            initial_board: "# Goal\n\nVerify the signed projection.",
        })
        .unwrap_or_else(|error| panic!("build test Create: {error}"))
        .sign_with_keys(host)
        .unwrap_or_else(|error| panic!("sign test Create: {error}"));
    let projection = parse_create(&event, TEST_MEETING_ID)
        .unwrap_or_else(|_| panic!("parse test Create"))
        .unwrap_or_else(|| panic!("test Create must match"));
    (event, projection)
}

struct StateFixture<'a> {
    signer: &'a Keys,
    create: &'a CreateProjection,
    meeting_id: &'a str,
    phase: &'a str,
    state_revision_tag: u64,
    state_revision_content: u64,
    floor_revision: u64,
    intent_revision: u64,
    speech_revision: u64,
    participant_type: &'a str,
    floor_target: Option<&'a str>,
}

fn test_state(input: StateFixture<'_>) -> Event {
    let participant_pubkey = input
        .create
        .participant_pubkeys
        .iter()
        .find(|pubkey| *pubkey != &input.create.host_pubkey)
        .unwrap_or_else(|| panic!("test Create has another participant"));
    let target_type = if input.floor_target == Some(input.create.host_pubkey.as_str()) {
        "human"
    } else {
        input.participant_type
    };
    let (offer, grant) = match input.phase {
        "offered" => (
            json!({
                "offer_id": "aa".repeat(32),
                "target_pubkey": input.floor_target,
                "target_participant_type": target_type,
                "allocation_source": "fallback",
                "turn_role": "participant",
                "basis_speech_revision": input.speech_revision,
                "created_at_ms": 1_000,
                "ack_deadline_ms": 31_000
            }),
            Value::Null,
        ),
        "granted" => (
            Value::Null,
            json!({
                "grant_id": "bb".repeat(32),
                "holder_pubkey": input.floor_target,
                "allocation_source": "fallback",
                "turn_role": "participant",
                "source_offer_id": "aa".repeat(32),
                "basis_speech_revision": input.speech_revision,
                "created_at_ms": 1_000,
                "soft_lease_expires_at_ms": 31_000,
                "hard_deadline_ms": 61_000,
                "progress_seq": 0
            }),
        ),
        _ => (Value::Null, Value::Null),
    };
    let content = json!({
        "phase": input.phase,
        "state_revision": input.state_revision_content,
        "floor_revision": input.floor_revision,
        "intent_revision": input.intent_revision,
        "speech_revision": input.speech_revision,
        "moderator_pubkey": input.create.host_pubkey,
        "participants": [
            {
                "pubkey": input.create.host_pubkey,
                "participant_type": "human",
                "channel_role": "owner"
            },
            {
                "pubkey": participant_pubkey,
                "participant_type": input.participant_type,
                "channel_role": "member"
            }
        ],
        "human_queue": [],
        "offer": offer,
        "grant": grant
    });
    EventBuilder::new(
        Kind::Custom(KIND_MEETING_STATE as u16),
        serde_json::to_string(&content)
            .unwrap_or_else(|error| panic!("serialize test State: {error}")),
    )
    .tags([
        tag(["h", input.meeting_id]),
        tag(["v", buzz_sdk_pkg::MEETING_V2_SCHEMA_VERSION]),
        tag(["policy", input.create.policy.as_str()]),
        tag(["phase", input.phase]),
        tag(["state-revision", &input.state_revision_tag.to_string()]),
        tag(["floor-revision", &input.floor_revision.to_string()]),
        tag(["intent-revision", &input.intent_revision.to_string()]),
        tag(["speech-revision", &input.speech_revision.to_string()]),
        tag(["moderator", input.create.host_pubkey.as_str()]),
    ])
    .sign_with_keys(input.signer)
    .unwrap_or_else(|error| panic!("sign test State: {error}"))
}

fn test_identity(relay: &Keys) -> MeetingIdentity {
    let relay_pubkey = relay.public_key();
    MeetingIdentity {
        relay_pubkey,
        capability: MeetingCapability {
            status: MeetingCapabilityStatus::Readable,
            relay_pubkey: Some(relay_pubkey.to_hex()),
            supports_direct_actions: true,
            can_create_direct_actions: false,
        },
    }
}

fn signed_channel(room_kind: Option<&str>) -> Event {
    let mut tags = vec![
        Tag::parse(["d", "00000000-0000-4000-8000-000000000001"])
            .map_err(|error| error.to_string())
            .ok(),
        Tag::parse(["name", "Design review"])
            .map_err(|error| error.to_string())
            .ok(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if let Some(room_kind) = room_kind {
        if let Ok(tag) = Tag::parse(["room_kind", room_kind]) {
            tags.push(tag);
        }
    }
    EventBuilder::new(Kind::Custom(39000), "")
        .tags(tags)
        .sign_with_keys(&Keys::generate())
        .unwrap_or_else(|error| panic!("sign test channel: {error}"))
}

#[test]
fn single_tag_rejects_duplicate_values() {
    let event = EventBuilder::new(Kind::Custom(39000), "")
        .tags([
            Tag::parse(["room_kind", "meeting"])
                .unwrap_or_else(|error| panic!("test tag: {error}")),
            Tag::parse(["room_kind", "channel"])
                .unwrap_or_else(|error| panic!("test tag: {error}")),
        ])
        .sign_with_keys(&Keys::generate())
        .unwrap_or_else(|error| panic!("sign test event: {error}"));
    assert_eq!(single_tag(&event, "room_kind"), None);
}

#[test]
fn room_kind_is_never_inferred_from_title() {
    assert_eq!(single_tag(&signed_channel(None), "room_kind"), None);
    assert_eq!(
        single_tag(&signed_channel(Some("meeting")), "room_kind"),
        Some("meeting")
    );
    assert_eq!(
        single_tag(&signed_channel(Some("future-room")), "room_kind"),
        Some("future-room")
    );
}

#[test]
fn canonical_meeting_ids_reject_aliases_and_nil() {
    assert!(canonical_meeting_id("00000000-0000-0000-0000-000000000000").is_err());
    assert!(canonical_meeting_id("00000000-0000-4000-8000-000000000001").is_ok());
    assert!(canonical_meeting_id("00000000-0000-4000-8000-000000000001 ").is_err());
}

#[test]
fn create_protocol_is_explicit_and_unknown_versions_are_not_interpreted() {
    let host = Keys::generate();
    let participant = Keys::generate();
    let (_, create) = test_create(&host, &participant);
    assert_eq!(create.policy, buzz_sdk_pkg::MEETING_V2_ACTIONS_POLICY);

    let unknown = EventBuilder::new(
        Kind::Custom(KIND_MEETING_CREATE as u16),
        r##"{"format":"markdown","body":"# Goal"}"##,
    )
    .tags([
        tag(["h", TEST_MEETING_ID]),
        tag(["name", "Future Meeting"]),
        tag(["v", "4"]),
        tag(["policy", "future-policy"]),
        tag(["p", participant.public_key().to_hex().as_str()]),
    ])
    .sign_with_keys(&host)
    .unwrap_or_else(|error| panic!("sign unknown Create: {error}"));
    assert!(parse_create(&unknown, TEST_MEETING_ID)
        .unwrap_or_else(|_| panic!("unknown protocol should not be an integrity error"))
        .is_none());
}

#[test]
fn state_requires_the_active_relay_signer_and_exact_meeting_scope() {
    let host = Keys::generate();
    let participant = Keys::generate();
    let relay = Keys::generate();
    let outsider = Keys::generate();
    let (_, create) = test_create(&host, &participant);
    let identity = test_identity(&relay);
    let participant_pubkey = participant.public_key().to_hex();
    let valid = test_state(StateFixture {
        signer: &relay,
        create: &create,
        meeting_id: TEST_MEETING_ID,
        phase: "granted",
        state_revision_tag: 2,
        state_revision_content: 2,
        floor_revision: 2,
        intent_revision: 1,
        speech_revision: 1,
        participant_type: "agent",
        floor_target: Some(&participant_pubkey),
    });
    assert!(parse_state(&valid, &identity, &create)
        .unwrap_or_else(|_| panic!("valid Relay State"))
        .is_some());

    let wrong_signer = test_state(StateFixture {
        signer: &outsider,
        create: &create,
        meeting_id: TEST_MEETING_ID,
        phase: "moderator_idle",
        state_revision_tag: 1,
        state_revision_content: 1,
        floor_revision: 1,
        intent_revision: 0,
        speech_revision: 0,
        participant_type: "agent",
        floor_target: None,
    });
    assert!(parse_state(&wrong_signer, &identity, &create).is_err());

    let cross_scope = test_state(StateFixture {
        signer: &relay,
        create: &create,
        meeting_id: "00000000-0000-4000-8000-000000000002",
        phase: "moderator_idle",
        state_revision_tag: 1,
        state_revision_content: 1,
        floor_revision: 1,
        intent_revision: 0,
        speech_revision: 0,
        participant_type: "agent",
        floor_target: None,
    });
    assert!(parse_state(&cross_scope, &identity, &create)
        .unwrap_or_else(|_| panic!("cross-scope State is ignored"))
        .is_none());
}

#[test]
fn state_rejects_revision_mismatch_unknown_participant_and_outside_floor_target() {
    let host = Keys::generate();
    let participant = Keys::generate();
    let relay = Keys::generate();
    let outsider = Keys::generate();
    let (_, create) = test_create(&host, &participant);
    let identity = test_identity(&relay);

    let revision_mismatch = test_state(StateFixture {
        signer: &relay,
        create: &create,
        meeting_id: TEST_MEETING_ID,
        phase: "moderator_idle",
        state_revision_tag: 2,
        state_revision_content: 1,
        floor_revision: 1,
        intent_revision: 0,
        speech_revision: 0,
        participant_type: "human",
        floor_target: None,
    });
    assert!(parse_state(&revision_mismatch, &identity, &create).is_err());

    let unknown_participant = test_state(StateFixture {
        signer: &relay,
        create: &create,
        meeting_id: TEST_MEETING_ID,
        phase: "moderator_idle",
        state_revision_tag: 1,
        state_revision_content: 1,
        floor_revision: 1,
        intent_revision: 0,
        speech_revision: 0,
        participant_type: "unknown",
        floor_target: None,
    });
    assert!(parse_state(&unknown_participant, &identity, &create).is_err());

    let outsider_pubkey = outsider.public_key().to_hex();
    let outside_floor = test_state(StateFixture {
        signer: &relay,
        create: &create,
        meeting_id: TEST_MEETING_ID,
        phase: "offered",
        state_revision_tag: 2,
        state_revision_content: 2,
        floor_revision: 2,
        intent_revision: 0,
        speech_revision: 0,
        participant_type: "human",
        floor_target: Some(&outsider_pubkey),
    });
    assert!(parse_state(&outside_floor, &identity, &create).is_err());
}

#[test]
fn current_state_rejects_conflicts_regressions_and_frozen_type_changes() {
    let host = Keys::generate();
    let participant = Keys::generate();
    let relay = Keys::generate();
    let (_, create) = test_create(&host, &participant);
    let identity = test_identity(&relay);

    let parse = |event: Event| {
        parse_state(&event, &identity, &create)
            .unwrap_or_else(|_| panic!("parse test State"))
            .unwrap_or_else(|| panic!("test State must match"))
    };
    let first = || {
        parse(test_state(StateFixture {
            signer: &relay,
            create: &create,
            meeting_id: TEST_MEETING_ID,
            phase: "moderator_idle",
            state_revision_tag: 1,
            state_revision_content: 1,
            floor_revision: 2,
            intent_revision: 1,
            speech_revision: 1,
            participant_type: "human",
            floor_target: None,
        }))
    };

    let conflicting = parse(test_state(StateFixture {
        signer: &relay,
        create: &create,
        meeting_id: TEST_MEETING_ID,
        phase: "moderator_control",
        state_revision_tag: 1,
        state_revision_content: 1,
        floor_revision: 2,
        intent_revision: 1,
        speech_revision: 1,
        participant_type: "human",
        floor_target: None,
    }));
    assert!(select_current_state(vec![first(), conflicting], &create).is_err());

    let regressed = parse(test_state(StateFixture {
        signer: &relay,
        create: &create,
        meeting_id: TEST_MEETING_ID,
        phase: "moderator_control",
        state_revision_tag: 2,
        state_revision_content: 2,
        floor_revision: 1,
        intent_revision: 1,
        speech_revision: 1,
        participant_type: "human",
        floor_target: None,
    }));
    assert!(select_current_state(vec![first(), regressed], &create).is_err());

    let changed_type = parse(test_state(StateFixture {
        signer: &relay,
        create: &create,
        meeting_id: TEST_MEETING_ID,
        phase: "moderator_control",
        state_revision_tag: 2,
        state_revision_content: 2,
        floor_revision: 2,
        intent_revision: 1,
        speech_revision: 1,
        participant_type: "agent",
        floor_target: None,
    }));
    assert!(select_current_state(vec![first(), changed_type], &create).is_err());
}

#[test]
fn action_policy_end_distinguishes_direct_close_from_recorded_actions() {
    let host = Keys::generate();
    let participant = Keys::generate();
    let (_, create) = test_create(&host, &participant);
    let session_id = Uuid::parse_str(TEST_MEETING_ID)
        .unwrap_or_else(|error| panic!("parse test Meeting ID: {error}"));

    let direct_close =
        buzz_sdk_pkg::build_meeting_v2_actions_end(buzz_sdk_pkg::MeetingV2ActionsEndParams {
            session_id,
            create_event_id: &create.event_id,
            outcome: buzz_sdk_pkg::MeetingV2EndOutcome::Closed,
            reason_code: None,
            reason: None,
            action_fence: None,
        })
        .unwrap_or_else(|error| panic!("build direct close: {error}"))
        .sign_with_keys(&host)
        .unwrap_or_else(|error| panic!("sign direct close: {error}"));
    let direct = parse_current_end(&[direct_close], &create)
        .unwrap_or_else(|error| panic!("parse direct close: {error:?}"))
        .unwrap_or_else(|| panic!("direct close must be present"));
    assert!(!direct.actions_attested);

    let board_event_id = "cd".repeat(32);
    let recorded_close =
        buzz_sdk_pkg::build_meeting_v2_actions_end(buzz_sdk_pkg::MeetingV2ActionsEndParams {
            session_id,
            create_event_id: &create.event_id,
            outcome: buzz_sdk_pkg::MeetingV2EndOutcome::Closed,
            reason_code: None,
            reason: None,
            action_fence: Some(buzz_sdk_pkg::MeetingV2ActionsEndFence {
                action_run_id: Uuid::new_v4(),
                action_window: 1,
                board_event_id: &board_event_id,
            }),
        })
        .unwrap_or_else(|error| panic!("build recorded-actions close: {error}"))
        .sign_with_keys(&host)
        .unwrap_or_else(|error| panic!("sign recorded-actions close: {error}"));
    let recorded = parse_current_end(&[recorded_close], &create)
        .unwrap_or_else(|error| panic!("parse recorded-actions close: {error:?}"))
        .unwrap_or_else(|| panic!("recorded-actions close must be present"));
    assert!(recorded.actions_attested);
}

#[test]
fn speech_requires_signature_roster_scope_and_authoritative_revision() {
    let participant = Keys::generate();
    let outsider = Keys::generate();
    let participant_pubkey = participant.public_key().to_hex();
    let roster = BTreeSet::from([participant_pubkey]);
    let speech = EventBuilder::new(Kind::Custom(KIND_STREAM_MESSAGE as u16), "Ready")
        .tags([
            tag(["h", TEST_MEETING_ID]),
            tag(["v", buzz_sdk_pkg::MEETING_V2_SCHEMA_VERSION]),
            tag(["meeting-grant", &"ab".repeat(32)]),
            tag(["speech-revision", "1"]),
        ])
        .sign_with_keys(&participant)
        .unwrap_or_else(|error| panic!("sign test Speech: {error}"));
    assert!(parse_speech(&speech, TEST_MEETING_ID, &roster, 1)
        .unwrap_or_else(|error| panic!("valid Speech: {error}"))
        .is_some());
    assert!(parse_speech(&speech, TEST_MEETING_ID, &roster, 0)
        .unwrap_or_else(|error| panic!("future Speech is ignored: {error}"))
        .is_none());

    let outside_speech = EventBuilder::new(Kind::Custom(KIND_STREAM_MESSAGE as u16), "No")
        .tags([
            tag(["h", TEST_MEETING_ID]),
            tag(["v", buzz_sdk_pkg::MEETING_V2_SCHEMA_VERSION]),
            tag(["meeting-grant", &"cd".repeat(32)]),
            tag(["speech-revision", "1"]),
        ])
        .sign_with_keys(&outsider)
        .unwrap_or_else(|error| panic!("sign outsider Speech: {error}"));
    assert!(parse_speech(&outside_speech, TEST_MEETING_ID, &roster, 1)
        .unwrap_or_else(|error| panic!("outsider Speech is ignored: {error}"))
        .is_none());

    let mut tampered = serde_json::to_value(&speech)
        .unwrap_or_else(|error| panic!("serialize test Speech: {error}"));
    tampered["content"] = json!("Tampered");
    let tampered: Event = serde_json::from_value(tampered)
        .unwrap_or_else(|error| panic!("deserialize tampered Speech: {error}"));
    assert!(parse_speech(&tampered, TEST_MEETING_ID, &roster, 1).is_err());
}

#[test]
fn speech_history_uses_the_relays_composite_cursor() {
    let filter = build_meeting_speech_filter(
        TEST_MEETING_ID,
        Some(1_765_000_000),
        Some(&"ab".repeat(32)),
        200,
    );

    assert_eq!(filter["kinds"], json!([KIND_STREAM_MESSAGE]));
    assert_eq!(filter["#h"], json!([TEST_MEETING_ID]));
    assert_eq!(filter["until"], json!(1_765_000_000_u64));
    assert_eq!(filter["before_id"], json!("ab".repeat(32)));
    assert_eq!(filter["limit"], json!(200));
}
