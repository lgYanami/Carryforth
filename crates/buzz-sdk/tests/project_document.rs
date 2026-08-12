use buzz_core::{CommunityId, RuntimeFence};
use buzz_project_document::{
    reduce_document, CurrentDocument, DocumentAttribution, DocumentCatalog, DocumentChangeContext,
    DocumentCommandRequest, DocumentRevision, DocumentSnapshot, DocumentState, ProjectDocument,
    ProjectDocumentCommand,
};
use buzz_sdk::project_document::{
    build_document_command, build_document_head_projection, build_document_meta_projection,
    build_document_revision_projection, changed_head_for, parse_document_command,
    parse_document_head, parse_document_meta, parse_document_revision,
    verify_document_head_observation, verify_document_meta_change, VerifiedCurrentDocument,
};
use buzz_sdk::SdkError;
use chrono::{DateTime, Utc};
use nostr::{Event, Keys, Timestamp};
use uuid::Uuid;

const PROJECT: &str = "3f2b2e8f-3f1d-4e91-91ac-5e5f1f0a2d77";
const DOCUMENT: &str = "9c23f672-a397-42d1-b933-104ba2674f26";
const ASSIGNMENT: &str = "151f2347-7d24-41a0-ab0d-f272e84fcf88";
const RUNTIME: &str = "74ad5e95-903b-4488-ac19-d95a73fa62d4";
const UPDATE_ID: &str = "c9c2a166554b139afe8d782425fc900b6d7af45faa1cbfe613445326d2d1178d";
const REVISION_ID: &str = "d518537232de41cf43e7e770ddf4f445da325b392ce09a58483e025f0f12e670";
const HEAD_ID: &str = "9fe56c45dbd4a17c71cfb963ecf0b5105a7adc609ac47eea00acbca4818bcc4e";
const META_ID: &str = "80cd192bae83db306a45fe84784481c9d7519e39ff60c038cffd08b3b5dcc344";

fn keys(secret: u8) -> Keys {
    Keys::parse(&format!("{secret:064x}")).expect("fixed test key")
}

fn project_id() -> CommunityId {
    CommunityId::from_uuid(Uuid::parse_str(PROJECT).expect("project UUID"))
}

fn timestamp() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-07-27T10:05:00Z")
        .expect("timestamp")
        .with_timezone(&Utc)
}

fn fixture(path: &str) -> Event {
    let content = match path {
        "head" => {
            include_str!("fixtures/project-document-v1/events/head-active.json")
        }
        "revision" => include_str!("fixtures/project-document-v1/events/revision-active.json"),
        "meta" => include_str!("fixtures/project-document-v1/events/meta-incremental.json"),
        "meta_empty" => {
            include_str!("fixtures/project-document-v1/events/meta-empty.json")
        }
        "wrong_signer" => include_str!("fixtures/project-document-v1/invalid/wrong-signer.json"),
        "cross_project" => include_str!("fixtures/project-document-v1/invalid/cross-project.json"),
        _ => panic!("unknown fixture"),
    };
    serde_json::from_str(content).expect("signed event fixture")
}

fn update_transition() -> (
    ProjectDocumentCommand,
    buzz_project_document::DocumentTransition,
) {
    let actor = keys(2).public_key();
    let document_id = Uuid::parse_str(DOCUMENT).expect("document UUID");
    let created_at = DateTime::parse_from_rfc3339("2026-07-27T10:00:00Z")
        .expect("created timestamp")
        .with_timezone(&Utc);
    let catalog = DocumentCatalog::from_snapshot(
        project_id(),
        30,
        12,
        1,
        DateTime::parse_from_rfc3339("2026-07-27T09:55:00Z")
            .expect("initialized timestamp")
            .with_timezone(&Utc),
        created_at,
    )
    .expect("catalog");
    let document = ProjectDocument::from_snapshot(
        document_id,
        7,
        DocumentState::Active,
        DocumentAttribution {
            at: created_at,
            by: actor,
        },
        DocumentAttribution {
            at: created_at,
            by: actor,
        },
    )
    .expect("document");
    let current = CurrentDocument::new(
        document,
        DocumentRevision::Active {
            schema_version: 1,
            document_id,
            document_revision: 7,
            snapshot: DocumentSnapshot {
                title: "Old guide".to_owned(),
                summary: None,
                content_markdown: "old".to_owned(),
            },
            actor,
            canonical_at: created_at,
        },
    )
    .expect("current");
    let command = ProjectDocumentCommand::new(
        7,
        DocumentCommandRequest::Update {
            document_id,
            title: "Buzz repository guide".to_owned(),
            summary: Some("Clone, initialize, and verify this repository.".to_owned()),
            content_markdown: "# Repository\n\nRun `just ci` before review.".to_owned(),
        },
    )
    .with_runtime_fence(
        Uuid::parse_str(ASSIGNMENT).expect("Assignment UUID"),
        RuntimeFence {
            runtime_id: Uuid::parse_str(RUNTIME).expect("Runtime UUID"),
            runtime_epoch: 4,
        },
    );
    let command_event = build_document_command(command.clone())
        .expect("command builder")
        .custom_created_at(Timestamp::from(1_785_146_700_u64))
        .sign_with_keys(&keys(2))
        .expect("sign command");
    assert_eq!(command_event.id.to_hex(), UPDATE_ID);
    let transition = reduce_document(
        &catalog,
        Some(&current),
        &command,
        DocumentChangeContext::new(actor, command_event.id, timestamp()),
    )
    .expect("reduce update");
    (command, transition)
}

#[test]
fn sdk_rebuilds_the_shared_command_and_projection_event_ids() {
    let (command, transition) = update_transition();
    let command_event = build_document_command(command)
        .expect("build command")
        .custom_created_at(Timestamp::from(1_785_146_700_u64))
        .sign_with_keys(&keys(2))
        .expect("sign command");
    assert_eq!(command_event.id.to_hex(), UPDATE_ID);
    assert_eq!(
        parse_document_command(&command_event)
            .expect("parse command")
            .document_id(),
        Uuid::parse_str(DOCUMENT).expect("UUID")
    );

    let relay = keys(1);
    let revision = build_document_revision_projection(transition.projection_plan())
        .expect("revision builder")
        .sign_with_keys(&relay)
        .expect("sign revision");
    assert_eq!(revision.id.to_hex(), REVISION_ID);
    let head = build_document_head_projection(transition.projection_plan(), &revision)
        .expect("head builder")
        .sign_with_keys(&relay)
        .expect("sign head");
    assert_eq!(head.id.to_hex(), HEAD_ID);
    let changed =
        changed_head_for(transition.projection_plan(), &head, &revision).expect("changed head");
    let meta = build_document_meta_projection(transition.projection_plan(), &[changed])
        .expect("meta builder")
        .sign_with_keys(&relay)
        .expect("sign metadata");
    assert_eq!(meta.id.to_hex(), META_ID);

    let verified_head =
        parse_document_head(&head, &relay.public_key(), project_id()).expect("verify head");
    let verified_revision = parse_document_revision(&revision, &relay.public_key(), project_id())
        .expect("verify revision");
    let current =
        VerifiedCurrentDocument::new(verified_head, verified_revision).expect("bind current");
    let verified_meta = parse_document_meta(&meta, &relay.public_key()).expect("verify meta");
    verify_document_head_observation(&verified_meta, &current.head)
        .expect("lightweight head observation");
    verify_document_meta_change(&verified_meta, &current).expect("bind metadata");

    let mut future_head = current.head;
    match &mut future_head.projection {
        buzz_project_document::DocumentHeadProjection::Active {
            catalog_revision, ..
        }
        | buzz_project_document::DocumentHeadProjection::Deleted {
            catalog_revision, ..
        } => *catalog_revision = verified_meta.projection.catalog_revision + 1,
    }
    assert!(verify_document_head_observation(&verified_meta, &future_head).is_err());
}

#[test]
fn production_parser_accepts_the_shared_golden_events() {
    let relay = keys(1).public_key();
    let head = parse_document_head(&fixture("head"), &relay, project_id()).expect("head");
    let revision =
        parse_document_revision(&fixture("revision"), &relay, project_id()).expect("revision");
    let current = VerifiedCurrentDocument::new(head, revision).expect("current");
    let meta = parse_document_meta(&fixture("meta"), &relay).expect("meta");
    verify_document_meta_change(&meta, &current).expect("meta binding");
    let empty = parse_document_meta(&fixture("meta_empty"), &relay).expect("empty metadata");
    assert!(empty.projection.reset);
    assert_eq!(empty.projection.catalog_revision, 0);
}

#[test]
fn wrong_signer_cross_project_and_cross_bundle_fail_closed() {
    let relay = keys(1).public_key();
    assert!(parse_document_head(&fixture("wrong_signer"), &relay, project_id()).is_err());
    assert!(parse_document_head(&fixture("cross_project"), &relay, project_id()).is_err());

    let head = parse_document_head(&fixture("head"), &relay, project_id()).expect("head");
    let tombstone: Event = serde_json::from_str(include_str!(
        "fixtures/project-document-v1/events/revision-tombstone.json"
    ))
    .expect("tombstone fixture");
    let tombstone =
        parse_document_revision(&tombstone, &relay, project_id()).expect("tombstone revision");
    assert!(VerifiedCurrentDocument::new(head, tombstone).is_err());
}

#[test]
fn invalid_projection_error_is_protocol_neutral() {
    let error = SdkError::InvalidProjection("bad pointer".to_owned());
    assert_eq!(error.to_string(), "invalid Relay projection: bad pointer");
}
