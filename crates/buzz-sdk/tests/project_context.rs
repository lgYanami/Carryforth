use buzz_core::{CommunityId, EventId, PublicKey};
use buzz_project_context::{
    canonicalize_coordinates, reduce_project_context, ProjectContextCatalog,
    ProjectContextChangeContext, ProjectContextCommand, ProjectContextCoordinate,
    ProjectContextOperation, ProjectContextProjectionPlan, ProjectContextReceipt,
    MAX_PROJECTION_CONTENT_BYTES,
};
use buzz_project_view::ProjectViewObjectType;
use buzz_sdk::project_context::{
    aggregate_project_context_edges, build_project_context_binding_projection,
    build_project_context_command, build_project_context_meta_projection,
    changed_project_context_binding_for, legacy_v1_migration, parse_project_context_binding,
    parse_project_context_command, parse_project_context_meta, validate_signed_event_frame_size,
    verify_project_context_meta_change, verify_project_context_projection_bundle,
};
use chrono::{DateTime, Utc};
use nostr::{Event, EventBuilder, Keys, Tag, Timestamp};
use uuid::Uuid;

const PROJECT: &str = "3f2b2e8f-3f1d-4e91-91ac-5e5f1f0a2d77";
const REQUIREMENT: &str = "0fd3a16e-4da4-48c1-aa6a-63b3661091d0";
const RESOURCE: &str = "e0a286dd-4391-4a45-b843-62b2c57b014a";
const CONTEXT_DOCUMENT: &str = "9c23f672-a397-42d1-b933-104ba2674f26";
const COMMAND_ATTACH_ID: &str = "9045864ebbce4d6e1fd37ba4a35ee832823a535c107d78ad88064879dfc0eefe";
const BINDING_ACTIVE_ID: &str = "f3dc55d71e0479c6cc0ec4b805ef76b2fa4a959fb37032c47094b129d5e2a383";
const META_INCREMENTAL_ID: &str =
    "24f769eff8547238beeb1a41006c51e75be63ed84705f7de3a9be3c9ffbea306";
const COMMAND_DETACH_ID: &str = "b900a43007575986118593995b346c3c7e7257f7b39e9b9495abbb0ad48837d4";
const BINDING_DELETED_ID: &str = "96ea3a97c28289bcaef8d68213666a5c0d26701bc4b9b3e622c70c5afcb352a1";
const META_DETACH_ID: &str = "d7ce27aa7d9828bea06a63ac1af12c398879f1ca92144f86a532350551ff1b72";

fn keys(secret: u8) -> Keys {
    Keys::parse(&format!("{secret:064x}")).expect("fixed test key")
}

fn project_id() -> CommunityId {
    CommunityId::from_uuid(Uuid::parse_str(PROJECT).expect("project UUID"))
}

fn canonical_time() -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_002, 0).expect("canonical timestamp")
}

fn coordinates() -> Vec<ProjectContextCoordinate> {
    canonicalize_coordinates(vec![
        ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Resource,
            object_id: Uuid::parse_str(RESOURCE).expect("Resource UUID"),
        },
        ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Requirement,
            object_id: Uuid::parse_str(REQUIREMENT).expect("Requirement UUID"),
        },
    ])
    .expect("canonical coordinates")
}

fn command_and_transition() -> (
    ProjectContextCommand,
    buzz_project_context::ProjectContextTransition,
) {
    let command = ProjectContextCommand::new(
        0,
        ProjectContextOperation::Attach,
        coordinates(),
        Uuid::parse_str(CONTEXT_DOCUMENT).expect("Document UUID"),
    )
    .expect("command");
    let command_event = build_project_context_command(project_id(), command.clone())
        .expect("command builder")
        .custom_created_at(Timestamp::from(1_800_000_001_u64))
        .sign_with_keys(&keys(2))
        .expect("sign command");
    let initialized_at = DateTime::from_timestamp(1_800_000_000, 0).expect("initialized time");
    let catalog =
        ProjectContextCatalog::empty(project_id(), 1, initialized_at).expect("empty catalog");
    let transition = reduce_project_context(
        &catalog,
        None,
        None,
        &command,
        ProjectContextChangeContext::active(
            keys(2).public_key(),
            command_event.id,
            canonical_time(),
        ),
    )
    .expect("attach transition");
    (command, transition)
}

#[test]
fn sdk_builds_and_strictly_verifies_the_complete_projection_bundle() {
    let (command, transition) = command_and_transition();
    let command_event = build_project_context_command(project_id(), command)
        .expect("command builder")
        .custom_created_at(Timestamp::from(1_800_000_001_u64))
        .sign_with_keys(&keys(2))
        .expect("sign command");
    let parsed =
        parse_project_context_command(&command_event, project_id()).expect("parse command");
    assert_eq!(
        parsed.context_document_id(),
        Uuid::parse_str(CONTEXT_DOCUMENT).expect("UUID")
    );

    let relay = keys(1);
    let binding = build_project_context_binding_projection(transition.projection_plan())
        .expect("binding builder")
        .sign_with_keys(&relay)
        .expect("sign binding");
    let changed = changed_project_context_binding_for(transition.projection_plan(), &binding)
        .expect("changed binding");
    let meta = build_project_context_meta_projection(transition.projection_plan(), &[changed])
        .expect("meta builder")
        .sign_with_keys(&relay)
        .expect("sign metadata");

    let verified_binding =
        parse_project_context_binding(&binding, &relay.public_key(), project_id())
            .expect("verify binding");
    let verified_meta = parse_project_context_meta(&meta, &relay.public_key(), project_id())
        .expect("verify metadata");
    verify_project_context_meta_change(&verified_meta, &verified_binding).expect("bind metadata");
    verify_project_context_projection_bundle(
        transition.projection_plan(),
        &binding,
        &meta,
        &relay.public_key(),
    )
    .expect("verify deterministic bundle");
    assert_eq!(command_event.id.to_hex(), COMMAND_ATTACH_ID);
    assert_eq!(binding.id.to_hex(), BINDING_ACTIVE_ID);
    assert_eq!(meta.id.to_hex(), META_INCREMENTAL_ID);
    assert_eq!(
        transition.binding().edge_key.to_string(),
        "5fd64dcb2a0aa7e37b696806be6c815df9dc3f3766b1613a89746269cde139fc"
    );
}

#[test]
fn reset_metadata_is_closed_and_has_no_source_or_changed_binding() {
    let initialized_at = DateTime::from_timestamp(1_800_000_000, 0).expect("initialized time");
    let catalog =
        ProjectContextCatalog::empty(project_id(), 3, initialized_at).expect("empty catalog");
    let plan = ProjectContextProjectionPlan::for_reset(&catalog).expect("reset plan");
    let relay = keys(1);
    let event = build_project_context_meta_projection(&plan, &[])
        .expect("reset meta builder")
        .sign_with_keys(&relay)
        .expect("sign reset meta");
    let verified = parse_project_context_meta(&event, &relay.public_key(), project_id())
        .expect("verify reset meta");
    assert!(verified.projection.reset);
    assert!(verified.projection.changed_bindings.is_empty());
    assert!(verified.projection.source_event_id.is_none());
}

#[test]
fn deleted_binding_bundle_is_strict() {
    let (_, attached) = command_and_transition();
    let command = ProjectContextCommand::new(
        1,
        ProjectContextOperation::Detach,
        coordinates(),
        Uuid::parse_str(CONTEXT_DOCUMENT).expect("Document UUID"),
    )
    .expect("detach command");
    let command_event = build_project_context_command(project_id(), command.clone())
        .expect("command builder")
        .custom_created_at(Timestamp::from(1_800_000_003_u64))
        .sign_with_keys(&keys(2))
        .expect("sign detach command");
    let transition = reduce_project_context(
        attached.catalog(),
        attached.edge(),
        Some(attached.binding().edge_key),
        &command,
        ProjectContextChangeContext::active(
            keys(2).public_key(),
            command_event.id,
            DateTime::from_timestamp(1_800_000_004, 0).expect("detach time"),
        ),
    )
    .expect("detach transition");
    let relay = keys(1);
    let binding = build_project_context_binding_projection(transition.projection_plan())
        .expect("deleted binding")
        .sign_with_keys(&relay)
        .expect("sign deleted binding");
    let changed = changed_project_context_binding_for(transition.projection_plan(), &binding)
        .expect("changed binding");
    let meta = build_project_context_meta_projection(transition.projection_plan(), &[changed])
        .expect("detach meta")
        .sign_with_keys(&relay)
        .expect("sign detach meta");
    verify_project_context_projection_bundle(
        transition.projection_plan(),
        &binding,
        &meta,
        &relay.public_key(),
    )
    .expect("verify deleted bundle");
    assert_eq!(command_event.id.to_hex(), COMMAND_DETACH_ID);
    assert_eq!(binding.id.to_hex(), BINDING_DELETED_ID);
    assert_eq!(meta.id.to_hex(), META_DETACH_ID);
}

#[test]
fn v2_meeting_fixtures_are_normative_and_production_parseable() {
    let relay = keys(1).public_key();
    let command_attach: Event = serde_json::from_str(include_str!(
        "fixtures/project-context-edge-v2/events/command-attach.json"
    ))
    .expect("v2 attach event fixture");
    let binding_active: Event = serde_json::from_str(include_str!(
        "fixtures/project-context-edge-v2/events/binding-active.json"
    ))
    .expect("v2 active binding fixture");
    let meta_incremental: Event = serde_json::from_str(include_str!(
        "fixtures/project-context-edge-v2/events/meta-incremental.json"
    ))
    .expect("v2 incremental metadata fixture");
    let command_detach: Event = serde_json::from_str(include_str!(
        "fixtures/project-context-edge-v2/events/command-detach.json"
    ))
    .expect("v2 detach event fixture");
    let binding_deleted: Event = serde_json::from_str(include_str!(
        "fixtures/project-context-edge-v2/events/binding-deleted.json"
    ))
    .expect("v2 deleted binding fixture");
    let meta_detach: Event = serde_json::from_str(include_str!(
        "fixtures/project-context-edge-v2/events/meta-detach.json"
    ))
    .expect("v2 detach metadata fixture");
    let meta_reset: Event = serde_json::from_str(include_str!(
        "fixtures/project-context-edge-v2/events/meta-reset.json"
    ))
    .expect("v2 reset metadata fixture");
    let meta_reset_reproject: Event = serde_json::from_str(include_str!(
        "fixtures/project-context-edge-v2/events/meta-reset-reproject.json"
    ))
    .expect("v2 reproject metadata fixture");
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/project-context-edge-v2/golden.json"))
            .expect("v2 golden manifest");

    let attach = parse_project_context_command(&command_attach, project_id())
        .expect("parse v2 Meeting attach");
    assert!(matches!(
        attach.coordinates()[1],
        ProjectContextCoordinate::Meeting { meeting_id }
            if meeting_id
                == Uuid::parse_str("0ed366aa-6f94-4eff-83db-b8bf081fbf35")
                    .expect("Meeting UUID")
    ));
    parse_project_context_command(&command_detach, project_id()).expect("parse v2 Meeting detach");
    let active = parse_project_context_binding(&binding_active, &relay, project_id())
        .expect("parse v2 active binding");
    let incremental = parse_project_context_meta(&meta_incremental, &relay, project_id())
        .expect("parse v2 incremental metadata");
    verify_project_context_meta_change(&incremental, &active)
        .expect("verify v2 active observation");
    let deleted = parse_project_context_binding(&binding_deleted, &relay, project_id())
        .expect("parse v2 deleted binding");
    let detached = parse_project_context_meta(&meta_detach, &relay, project_id())
        .expect("parse v2 detach metadata");
    verify_project_context_meta_change(&detached, &deleted).expect("verify v2 deleted observation");
    assert!(
        parse_project_context_meta(&meta_reset, &relay, project_id())
            .expect("parse v2 reset metadata")
            .projection
            .reset
    );
    assert!(
        parse_project_context_meta(&meta_reset_reproject, &relay, project_id())
            .expect("parse v2 reproject metadata")
            .projection
            .reset
    );

    let raw_attach = include_str!("fixtures/project-context-edge-v2/commands/attach.json").trim();
    assert_eq!(
        serde_json::to_string(&ProjectContextCommand::from_json(raw_attach).expect("v2 attach"))
            .expect("serialize v2 attach"),
        raw_attach
    );
    let receipt_raw = include_str!("fixtures/project-context-edge-v2/receipt-detach.json").trim();
    let receipt: ProjectContextReceipt =
        serde_json::from_str(receipt_raw).expect("v2 receipt fixture");
    receipt.validate().expect("validate v2 receipt");
    assert_eq!(receipt.edge_key.to_string(), golden["edge_key"]);

    for (field, event) in [
        ("command_attach_event_id", &command_attach),
        ("binding_active_event_id", &binding_active),
        ("meta_incremental_event_id", &meta_incremental),
        ("command_detach_event_id", &command_detach),
        ("binding_deleted_event_id", &binding_deleted),
        ("meta_detach_event_id", &meta_detach),
        ("meta_reset_event_id", &meta_reset),
        ("meta_reset_reproject_event_id", &meta_reset_reproject),
    ] {
        assert_eq!(golden[field], event.id.to_hex());
    }
}

#[test]
fn legacy_v1_fixtures_are_migration_only_and_rejected_by_v2_runtime_parsers() {
    let relay = keys(1).public_key();
    let command_attach: Event = serde_json::from_str(include_str!(
        "fixtures/project-context-edge-v1/events/command-attach.json"
    ))
    .expect("command fixture");
    let binding_active: Event = serde_json::from_str(include_str!(
        "fixtures/project-context-edge-v1/events/binding-active.json"
    ))
    .expect("binding fixture");
    let meta_incremental: Event = serde_json::from_str(include_str!(
        "fixtures/project-context-edge-v1/events/meta-incremental.json"
    ))
    .expect("meta fixture");
    let command_detach: Event = serde_json::from_str(include_str!(
        "fixtures/project-context-edge-v1/events/command-detach.json"
    ))
    .expect("detach command fixture");
    let binding_deleted: Event = serde_json::from_str(include_str!(
        "fixtures/project-context-edge-v1/events/binding-deleted.json"
    ))
    .expect("deleted binding fixture");
    let meta_detach: Event = serde_json::from_str(include_str!(
        "fixtures/project-context-edge-v1/events/meta-detach.json"
    ))
    .expect("detach meta fixture");
    let meta_reset: Event = serde_json::from_str(include_str!(
        "fixtures/project-context-edge-v1/events/meta-reset.json"
    ))
    .expect("reset meta fixture");
    let meta_reset_reproject: Event = serde_json::from_str(include_str!(
        "fixtures/project-context-edge-v1/events/meta-reset-reproject.json"
    ))
    .expect("reproject reset meta fixture");
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/project-context-edge-v1/golden.json"))
            .expect("golden manifest");

    assert!(parse_project_context_command(&command_attach, project_id()).is_err());
    assert!(parse_project_context_command(&command_detach, project_id()).is_err());
    assert!(parse_project_context_binding(&binding_active, &relay, project_id()).is_err());
    assert!(parse_project_context_meta(&meta_incremental, &relay, project_id()).is_err());

    let active = legacy_v1_migration::verify_binding(&binding_active, &relay, project_id())
        .expect("migration verifies active binding");
    let incremental = legacy_v1_migration::verify_meta(&meta_incremental, &relay, project_id())
        .expect("migration verifies incremental metadata");
    assert_eq!(
        incremental.changed_bindings[0].binding_event_id,
        binding_active.id
    );
    assert_eq!(incremental.changed_bindings[0].edge_key, active.edge_key);
    let deleted = legacy_v1_migration::verify_binding(&binding_deleted, &relay, project_id())
        .expect("migration verifies deleted binding");
    let detach = legacy_v1_migration::verify_meta(&meta_detach, &relay, project_id())
        .expect("migration verifies detach metadata");
    assert_eq!(
        detach.changed_bindings[0].binding_event_id,
        binding_deleted.id
    );
    assert_eq!(detach.changed_bindings[0].edge_key, deleted.edge_key);
    let reset = legacy_v1_migration::verify_meta(&meta_reset, &relay, project_id())
        .expect("migration verifies reset metadata");
    assert!(reset.reset);
    let reproject_reset =
        legacy_v1_migration::verify_meta(&meta_reset_reproject, &relay, project_id())
            .expect("migration verifies reproject reset metadata");
    assert!(reproject_reset.reset);
    assert_eq!(reproject_reset.context_revision, active.context_revision);
    assert!(legacy_v1_migration::verify_binding(
        &binding_active,
        &keys(3).public_key(),
        project_id()
    )
    .is_err());

    for (field, event) in [
        ("command_attach_event_id", &command_attach),
        ("binding_active_event_id", &binding_active),
        ("meta_incremental_event_id", &meta_incremental),
        ("command_detach_event_id", &command_detach),
        ("binding_deleted_event_id", &binding_deleted),
        ("meta_detach_event_id", &meta_detach),
        ("meta_reset_event_id", &meta_reset),
        ("meta_reset_reproject_event_id", &meta_reset_reproject),
    ] {
        assert_eq!(golden[field], event.id.to_hex(), "golden field {field}");
    }

    let attach_raw = include_str!("fixtures/project-context-edge-v1/commands/attach.json").trim();
    let attach: serde_json::Value = serde_json::from_str(attach_raw).expect("raw attach fixture");
    assert_eq!(attach["schema_version"], 1);
    assert!(ProjectContextCommand::from_json(attach_raw).is_err());
    let detach_raw = include_str!("fixtures/project-context-edge-v1/commands/detach.json").trim();
    let detach: serde_json::Value = serde_json::from_str(detach_raw).expect("raw detach fixture");
    assert_eq!(detach["schema_version"], 1);
    assert!(ProjectContextCommand::from_json(detach_raw).is_err());

    let receipt_raw = include_str!("fixtures/project-context-edge-v1/receipt-detach.json").trim();
    let receipt: ProjectContextReceipt =
        serde_json::from_str(receipt_raw).expect("receipt fixture");
    assert!(receipt.validate().is_err());
    assert_eq!(
        serde_json::to_string(&receipt).expect("receipt JSON"),
        receipt_raw
    );
}

#[test]
fn wrong_relay_project_and_cross_observation_fail_closed() {
    let (_, transition) = command_and_transition();
    let relay = keys(1);
    let binding = build_project_context_binding_projection(transition.projection_plan())
        .expect("binding builder")
        .sign_with_keys(&relay)
        .expect("binding");
    assert!(parse_project_context_binding(&binding, &keys(3).public_key(), project_id()).is_err());
    let other_project = CommunityId::from_uuid(
        Uuid::parse_str("825a0671-d1b8-4472-9e7e-405c186d1575").expect("other project"),
    );
    assert!(parse_project_context_binding(&binding, &relay.public_key(), other_project).is_err());

    let changed = changed_project_context_binding_for(transition.projection_plan(), &binding)
        .expect("changed binding");
    let meta = build_project_context_meta_projection(transition.projection_plan(), &[changed])
        .expect("meta")
        .sign_with_keys(&relay)
        .expect("signed meta");
    let verified_binding =
        parse_project_context_binding(&binding, &relay.public_key(), project_id())
            .expect("binding");
    let verified_meta =
        parse_project_context_meta(&meta, &relay.public_key(), project_id()).expect("meta");
    verify_project_context_meta_change(&verified_meta, &verified_binding)
        .expect("same observation");

    let mut wrong_binding = verified_binding;
    wrong_binding.event_id = EventId::from_hex(&"ff".repeat(32)).expect("event ID");
    assert!(verify_project_context_meta_change(&verified_meta, &wrong_binding).is_err());
}

#[test]
fn complete_signed_event_frame_has_an_explicit_precommit_limit() {
    let (_, transition) = command_and_transition();
    let event = build_project_context_binding_projection(transition.projection_plan())
        .expect("binding")
        .sign_with_keys(&keys(1))
        .expect("signed binding");
    validate_signed_event_frame_size(&event, 512 * 1024).expect("ordinary frame");
    assert!(validate_signed_event_frame_size(&event, 32).is_err());
}

#[test]
fn verified_binding_aggregation_is_deterministic_and_fail_closed() {
    let (_, transition) = command_and_transition();
    let relay = keys(1);
    let binding_event = build_project_context_binding_projection(transition.projection_plan())
        .expect("binding")
        .sign_with_keys(&relay)
        .expect("signed binding");
    let changed = changed_project_context_binding_for(transition.projection_plan(), &binding_event)
        .expect("changed binding");
    let meta_event =
        build_project_context_meta_projection(transition.projection_plan(), &[changed])
            .expect("meta")
            .sign_with_keys(&relay)
            .expect("signed meta");
    let binding = parse_project_context_binding(&binding_event, &relay.public_key(), project_id())
        .expect("verified binding");
    let meta = parse_project_context_meta(&meta_event, &relay.public_key(), project_id())
        .expect("verified meta");

    let edges = aggregate_project_context_edges(&meta, std::slice::from_ref(&binding), true)
        .expect("complete one-edge catalog");
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].coordinates(), coordinates());
    assert_eq!(
        edges[0].context_document_ids(),
        &[Uuid::parse_str(CONTEXT_DOCUMENT).expect("Document UUID")]
    );

    assert!(
        aggregate_project_context_edges(&meta, &[binding.clone(), binding.clone()], false,)
            .is_err()
    );

    let mut wrong_counts = meta;
    wrong_counts.projection.bound_document_count = 2;
    assert!(
        aggregate_project_context_edges(&wrong_counts, std::slice::from_ref(&binding), true,)
            .is_err()
    );
    aggregate_project_context_edges(&wrong_counts, &[binding], false)
        .expect("subset queries must not apply global catalog counts");
}

#[test]
fn extra_reordered_tags_and_mismatched_edge_hash_fail_closed() {
    let (command, transition) = command_and_transition();
    let relay = keys(1);
    let binding = build_project_context_binding_projection(transition.projection_plan())
        .expect("binding")
        .sign_with_keys(&relay)
        .expect("signed binding");

    let mut extra_tags: Vec<Tag> = binding.tags.iter().cloned().collect();
    extra_tags.push(Tag::parse(["x", "extra"]).expect("extra tag"));
    let extra = EventBuilder::new(binding.kind, binding.content.clone())
        .tags(extra_tags)
        .custom_created_at(binding.created_at)
        .sign_with_keys(&relay)
        .expect("sign extra-tag binding");
    assert!(parse_project_context_binding(&extra, &relay.public_key(), project_id()).is_err());

    let mut reordered_tags: Vec<Tag> = binding.tags.iter().cloned().collect();
    reordered_tags.swap(2, 3);
    let reordered = EventBuilder::new(binding.kind, binding.content.clone())
        .tags(reordered_tags)
        .custom_created_at(binding.created_at)
        .sign_with_keys(&relay)
        .expect("sign reordered binding");
    assert!(parse_project_context_binding(&reordered, &relay.public_key(), project_id()).is_err());

    let mut content: serde_json::Value =
        serde_json::from_str(&binding.content).expect("binding content");
    content["edge_key"] = serde_json::Value::String("00".repeat(32));
    let mismatched = EventBuilder::new(binding.kind, content.to_string())
        .tags(binding.tags.iter().cloned())
        .custom_created_at(binding.created_at)
        .sign_with_keys(&relay)
        .expect("sign mismatched binding");
    assert!(parse_project_context_binding(&mismatched, &relay.public_key(), project_id()).is_err());

    for (index, replacement) in [
        (1, Tag::parse(["d", "wrong"]).expect("wrong d")),
        (5, Tag::parse(["g", "wrong"]).expect("wrong g")),
        (6, Tag::parse(["c", "wrong"]).expect("wrong c")),
        (
            binding.tags.len() - 2,
            Tag::parse(["context_revision", "01"]).expect("noncanonical revision"),
        ),
    ] {
        let mut tags: Vec<Tag> = binding.tags.iter().cloned().collect();
        tags[index] = replacement;
        let event = EventBuilder::new(binding.kind, binding.content.clone())
            .tags(tags)
            .custom_created_at(binding.created_at)
            .sign_with_keys(&relay)
            .expect("sign wrong-tag binding");
        assert!(parse_project_context_binding(&event, &relay.public_key(), project_id()).is_err());
    }

    let oversized_content = format!(
        "{}{}",
        binding.content,
        " ".repeat(MAX_PROJECTION_CONTENT_BYTES)
    );
    let oversized = EventBuilder::new(binding.kind, oversized_content)
        .tags(binding.tags.iter().cloned())
        .custom_created_at(binding.created_at)
        .sign_with_keys(&relay)
        .expect("sign oversized binding");
    assert!(parse_project_context_binding(&oversized, &relay.public_key(), project_id()).is_err());

    let command_event = build_project_context_command(project_id(), command)
        .expect("command builder")
        .custom_created_at(Timestamp::from(1_800_000_001_u64))
        .sign_with_keys(&keys(2))
        .expect("sign command");
    let mut noncanonical_command: serde_json::Value =
        serde_json::from_str(&command_event.content).expect("command content");
    let object_id = noncanonical_command["request"]["coordinates"][0]["object_id"]
        .as_str()
        .expect("object id")
        .to_ascii_uppercase();
    noncanonical_command["request"]["coordinates"][0]["object_id"] =
        serde_json::Value::String(object_id);
    let noncanonical_command =
        EventBuilder::new(command_event.kind, noncanonical_command.to_string())
            .tags(command_event.tags.iter().cloned())
            .custom_created_at(command_event.created_at)
            .sign_with_keys(&keys(2))
            .expect("sign noncanonical command");
    assert!(parse_project_context_command(&noncanonical_command, project_id()).is_err());
}

#[test]
fn verified_projection_types_retain_expected_relay_identity() {
    let (_, transition) = command_and_transition();
    let relay = keys(1);
    let binding = build_project_context_binding_projection(transition.projection_plan())
        .expect("binding")
        .sign_with_keys(&relay)
        .expect("signed binding");
    let verified = parse_project_context_binding(&binding, &relay.public_key(), project_id())
        .expect("verified binding");
    let expected: PublicKey = relay.public_key();
    assert_eq!(verified.signer, expected);
}
