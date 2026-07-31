use std::str::FromStr;

use buzz_core::kind::{
    KIND_PROJECT_DOCUMENT_COMMAND, KIND_PROJECT_DOCUMENT_HEAD, KIND_PROJECT_DOCUMENT_META,
    KIND_PROJECT_DOCUMENT_REVISION,
};
use buzz_core::{EventId, PublicKey};
use buzz_project_document::{
    document_head_coordinate, document_meta_coordinate, document_revision_coordinate,
    DocumentHeadProjection, DocumentMetaProjection, DocumentProjectionType,
    DocumentRevisionProjection, ProjectDocumentCommand, ProjectDocumentReceipt,
};
use buzz_project_view::v2::{CommunityMemberRole, RoleCommand, SchemaVersion};
use buzz_project_view::v3::{
    canonicalize_context_references, guide_snapshot_digest, legacy_resource_digest,
    manifest_digest, mapping_entry_digest, resource_cutover_payload_digest, review_digest,
    CanonicalGuideSnapshotV1, CanonicalLegacyObjectStateV1, CanonicalLegacyResourceV1,
    CanonicalProjectResourceV3, CanonicalResourceCutoverV1, CanonicalResourceMappingEntryV1,
    CanonicalResourceReviewV1, ContextAvailabilityV3, DocumentMetadataSourceV3,
    ProjectContextReference, ProjectResourceV3, ProjectViewInitializeV3, ResourceMappingManifestV1,
    ReviewSignature, ReviewedResourceMappingV1, RoleBriefContextV3, RoleBriefSourceRevisionsV3,
    RoleDefinitionV3,
};
use buzz_project_view::{Mutation, ProjectResource, ProjectViewRelations};
use chrono::{DateTime, Utc};
use nostr::secp256k1::schnorr::Signature;
use nostr::secp256k1::{Message, SECP256K1};
use nostr::{Event, JsonUtil};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

macro_rules! fixture {
    ($path:literal) => {
        include_str!(concat!(
            "../../../docs/nips/fixtures/project-document-v1/",
            $path
        ))
    };
}

const PROJECT: &str = "3f2b2e8f-3f1d-4e91-91ac-5e5f1f0a2d77";
const RELAY: &str = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

#[test]
fn commands_and_receipt_roundtrip_the_shared_bytes() {
    for raw in [
        fixture!("commands/create.json"),
        fixture!("commands/update.json"),
        fixture!("commands/delete.json"),
    ] {
        let command = ProjectDocumentCommand::from_json(raw).expect("valid command fixture");
        assert_json_roundtrip(raw, &command);
    }

    for raw in [
        fixture!("events/command-create.json"),
        fixture!("events/command-update.json"),
        fixture!("events/command-delete.json"),
    ] {
        let event = parse_event(raw);
        event.verify().expect("valid command signature");
        assert_eq!(event.kind.as_u16() as u32, KIND_PROJECT_DOCUMENT_COMMAND);
        require_exact_tags(
            &event,
            vec![
                vec!["-".to_owned()],
                vec!["t".to_owned(), "buzz-project-document-command".to_owned()],
            ],
        )
        .expect("exact command tags");
        ProjectDocumentCommand::from_json(&event.content).expect("valid signed command content");
    }

    let receipt: ProjectDocumentReceipt = parse_fixture(fixture!("receipt-update.json"));
    receipt.validate().expect("valid stable receipt");
    assert_json_roundtrip(fixture!("receipt-update.json"), &receipt);
}

#[test]
fn independent_projection_parser_verifies_signatures_tags_and_pointers() {
    let relay = PublicKey::from_hex(RELAY).expect("relay key");
    let project = Uuid::parse_str(PROJECT).expect("project UUID");
    let active_revision = parse_projection(fixture!("events/revision-active.json"), relay, project)
        .expect("active revision");
    let active_head =
        parse_projection(fixture!("events/head-active.json"), relay, project).expect("active head");
    let incremental = parse_projection(fixture!("events/meta-incremental.json"), relay, project)
        .expect("incremental meta");
    let deleted_revision =
        parse_projection(fixture!("events/revision-tombstone.json"), relay, project)
            .expect("deleted revision");
    let deleted_head = parse_projection(fixture!("events/head-tombstone.json"), relay, project)
        .expect("deleted head");
    let empty = parse_projection(fixture!("events/meta-empty.json"), relay, project)
        .expect("empty bootstrap meta");
    let reset =
        parse_projection(fixture!("events/meta-reset.json"), relay, project).expect("reset meta");

    assert_eq!(
        active_head.revision_event_id,
        Some(active_revision.event_id)
    );
    assert_eq!(
        deleted_head.revision_event_id,
        Some(deleted_revision.event_id)
    );
    assert_eq!(incremental.head_event_id, Some(active_head.event_id));
    assert_eq!(
        incremental.revision_event_id,
        Some(active_revision.event_id)
    );
    assert!(empty.source_event_id.is_none());
    assert!(reset.source_event_id.is_none());
}

#[test]
fn malformed_extra_tag_cross_project_and_wrong_signer_fail_closed() {
    let relay = PublicKey::from_hex(RELAY).expect("relay key");
    let project = Uuid::parse_str(PROJECT).expect("project UUID");
    assert!(ProjectDocumentCommand::from_json(fixture!("invalid/malformed-command.json")).is_err());
    for raw in [
        fixture!("invalid/extra-tag.json"),
        fixture!("invalid/cross-project.json"),
        fixture!("invalid/wrong-signer.json"),
    ] {
        assert!(parse_projection(raw, relay, project).is_err());
    }
}

#[test]
fn project_view_v3_resource_context_initialize_role_and_brief_are_closed() {
    let resource =
        ProjectResourceV3::from_json(fixture!("v3/resource.json")).expect("valid v3 Resource");
    assert_json_roundtrip(fixture!("v3/resource.json"), &resource);

    let resource_ref: ProjectContextReference = parse_fixture(fixture!("v3/context-resource.json"));
    let live_ref: ProjectContextReference =
        parse_fixture(fixture!("v3/context-document-live.json"));
    let pinned_ref: ProjectContextReference =
        parse_fixture(fixture!("v3/context-document-pinned.json"));
    for reference in [&resource_ref, &live_ref, &pinned_ref] {
        reference.validate().expect("valid Context variant");
    }
    let canonical = canonicalize_context_references(vec![
        pinned_ref.clone(),
        live_ref.clone(),
        resource_ref.clone(),
    ])
    .expect("canonical Context set");
    assert_eq!(canonical, vec![resource_ref, live_ref, pinned_ref]);
    assert!(
        canonicalize_context_references(vec![canonical[0].clone(), canonical[0].clone()]).is_err()
    );
    assert!(serde_json::from_str::<ProjectContextReference>(fixture!(
        "invalid/context-live-null.json"
    ))
    .is_err());

    let role: RoleDefinitionV3 = parse_fixture(fixture!("v3/role-definition.json"));
    role.validate().expect("valid RoleDefinitionV3");
    assert_json_roundtrip(fixture!("v3/role-definition.json"), &role);

    let initialize = ProjectViewInitializeV3::from_json(fixture!("v3/initialize.json"))
        .expect("valid greenfield initialize");
    assert_json_roundtrip(fixture!("v3/initialize.json"), &initialize);

    let brief: BaseRoleBriefV3Fixture = parse_fixture(fixture!("v3/role-brief-base.json"));
    brief
        .validate()
        .expect("valid base Context-off RoleBriefV3");
    assert_json_roundtrip(fixture!("v3/role-brief-base.json"), &brief);
}

#[test]
fn project_view_v1_v2_v3_and_document_parsers_do_not_fallback() {
    let v1 = r#"{
      "schema_version":1,
      "expected_project_revision":0,
      "request":{"type":"initialize","profile":{
        "name":"Buzz","positioning":"Nostr-first","purpose":"Collaborate",
        "problem":"Fragmented context","scope":"Relay"
      },"goals":[]}
    }"#;
    let v2 = r#"{
      "schema_version":2,
      "expected_project_revision":1,
      "request":{"type":"accept_proposal","proposal_id":"eafab35e-745f-4d4a-bfbc-46d512904f06"}
    }"#;
    let v3 = fixture!("v3/initialize.json");
    let document = fixture!("commands/create.json");

    Mutation::from_json(v1).expect("v1 parser accepts v1");
    RoleCommand::from_json(v2).expect("v2 parser accepts v2");
    ProjectViewInitializeV3::from_json(v3).expect("v3 parser accepts v3");
    ProjectDocumentCommand::from_json(document).expect("Document parser accepts Document v1");

    for raw in [v2, v3, document] {
        assert!(
            Mutation::from_json(raw).is_err(),
            "v1 accepted foreign wire"
        );
    }
    for raw in [v1, v3, document] {
        assert!(
            RoleCommand::from_json(raw).is_err(),
            "v2 accepted foreign wire"
        );
    }
    for raw in [v1, v2, document] {
        assert!(
            ProjectViewInitializeV3::from_json(raw).is_err(),
            "v3 accepted foreign wire"
        );
    }
    for raw in [v1, v2, v3] {
        assert!(
            ProjectDocumentCommand::from_json(raw).is_err(),
            "Document accepted Project View wire"
        );
    }

    assert_eq!(SchemaVersion::try_from(1), Ok(SchemaVersion::V1));
    assert_eq!(SchemaVersion::try_from(2), Ok(SchemaVersion::V2));
    assert_eq!(SchemaVersion::try_from(3), Ok(SchemaVersion::V3));
    assert!(SchemaVersion::try_from(4).is_err());
}

#[test]
fn migration_postcard_digests_and_review_signature_match_golden_bytes() {
    let envelope: ManifestEnvelope = parse_fixture(fixture!("migration/manifest.json"));
    let golden: MigrationGolden = parse_fixture(fixture!("migration/golden.json"));
    let legacy: LegacyResourceEnvelope = parse_fixture(fixture!("migration/legacy-resource.json"));
    let manifest = envelope
        .into_canonical()
        .expect("canonical manifest envelope");
    manifest.validate().expect("manifest digest parity");

    let legacy_value = CanonicalLegacyResourceV1 {
        schema_version: legacy.schema_version,
        resource_id: *legacy.resource_id.as_bytes(),
        object_revision: legacy.object_revision,
        project_revision: legacy.project_revision,
        state: legacy.state,
        resource_data: legacy.resource_data,
        relations: legacy.relations,
    };
    assert_eq!(
        hex::encode(legacy_resource_digest(&legacy_value).expect("legacy digest")),
        golden.legacy_body_digest
    );

    let revision: DocumentRevisionProjection =
        projection_content(fixture!("events/revision-active.json"));
    let guide = match revision {
        DocumentRevisionProjection::Active {
            document_id,
            document_revision,
            title,
            summary,
            content_markdown,
            ..
        } => CanonicalGuideSnapshotV1 {
            document_id: *document_id.as_bytes(),
            document_revision,
            title,
            summary,
            content_markdown,
        },
        DocumentRevisionProjection::Deleted { .. } => panic!("Guide fixture is deleted"),
    };
    assert_eq!(
        hex::encode(guide_snapshot_digest(&guide).expect("Guide digest")),
        golden.guide_content_digest
    );

    let entry = &manifest.entries[0];
    assert_eq!(
        hex::encode(entry.v3_payload_digest),
        golden.v3_payload_digest
    );
    assert_eq!(
        hex::encode(entry.mapping_entry_digest),
        golden.mapping_entry_digest
    );
    assert_eq!(hex::encode(entry.review_digest), golden.review_digest);
    assert_eq!(
        hex::encode(entry.review_signature.as_bytes()),
        golden.review_signature
    );
    assert_eq!(
        hex::encode(postcard::to_stdvec(&manifest).expect("postcard manifest")),
        golden.postcard_manifest_hex
    );
    assert_eq!(
        hex::encode(manifest_digest(&manifest).expect("manifest digest")),
        golden.manifest_digest
    );

    let signature = Signature::from_str(&golden.review_signature).expect("BIP-340 signature");
    let reviewer = PublicKey::from_slice(&entry.reviewed_by_pubkey).expect("reviewer pubkey");
    let xonly = reviewer.xonly().expect("x-only reviewer key");
    let message = Message::from_digest(entry.review_digest);
    SECP256K1
        .verify_schnorr(&signature, &message, &xonly)
        .expect("valid detached review signature");
}

#[derive(Debug)]
struct ParsedProjection {
    event_id: EventId,
    source_event_id: Option<EventId>,
    revision_event_id: Option<EventId>,
    head_event_id: Option<EventId>,
}

fn parse_projection(
    raw: &str,
    expected_relay: PublicKey,
    expected_project: Uuid,
) -> Result<ParsedProjection, String> {
    let event = Event::from_json(raw).map_err(|error| error.to_string())?;
    event.verify().map_err(|error| error.to_string())?;
    if event.pubkey != expected_relay {
        return Err("wrong Relay signer".to_owned());
    }
    match event.kind.as_u16() as u32 {
        KIND_PROJECT_DOCUMENT_HEAD => parse_head(event, expected_project),
        KIND_PROJECT_DOCUMENT_REVISION => parse_revision(event, expected_project),
        KIND_PROJECT_DOCUMENT_META => parse_meta(event, expected_project),
        _ => Err("unexpected projection kind".to_owned()),
    }
}

fn parse_head(event: Event, expected_project: Uuid) -> Result<ParsedProjection, String> {
    let content: DocumentHeadProjection =
        serde_json::from_str(&event.content).map_err(|error| error.to_string())?;
    content.validate().map_err(|error| error.to_string())?;
    assert_content_roundtrip(&event.content, &content)?;
    let (
        project,
        generation,
        catalog_revision,
        document_id,
        document_revision,
        state_tag,
        revision_event_id,
        source_event_id,
        canonical_at,
    ) = match &content {
        DocumentHeadProjection::Active {
            project_id,
            projection_generation,
            catalog_revision,
            document_id,
            document_revision,
            revision_event_id,
            source_event_id,
            updated_at,
            ..
        } => (
            *project_id,
            *projection_generation,
            *catalog_revision,
            *document_id,
            *document_revision,
            "buzz-project-document-active",
            *revision_event_id,
            *source_event_id,
            *updated_at,
        ),
        DocumentHeadProjection::Deleted {
            project_id,
            projection_generation,
            catalog_revision,
            document_id,
            document_revision,
            revision_event_id,
            source_event_id,
            deleted_at,
            ..
        } => (
            *project_id,
            *projection_generation,
            *catalog_revision,
            *document_id,
            *document_revision,
            "buzz-project-document-tombstone",
            *revision_event_id,
            *source_event_id,
            *deleted_at,
        ),
    };
    require_project_time(&event, project, expected_project, canonical_at)?;
    require_exact_tags(
        &event,
        vec![
            vec!["-".to_owned()],
            vec![
                "d".to_owned(),
                document_head_coordinate(project, document_id),
            ],
            vec!["t".to_owned(), "buzz-project-document".to_owned()],
            vec!["t".to_owned(), "buzz-project-document-head".to_owned()],
            vec!["t".to_owned(), state_tag.to_owned()],
            vec!["projection_generation".to_owned(), generation.to_string()],
            vec!["catalog_revision".to_owned(), catalog_revision.to_string()],
            vec![
                "document_revision".to_owned(),
                document_revision.to_string(),
            ],
            vec![
                "e".to_owned(),
                revision_event_id.to_hex(),
                String::new(),
                "revision".to_owned(),
            ],
            vec![
                "e".to_owned(),
                source_event_id.to_hex(),
                String::new(),
                "source".to_owned(),
            ],
        ],
    )?;
    Ok(ParsedProjection {
        event_id: event.id,
        source_event_id: Some(source_event_id),
        revision_event_id: Some(revision_event_id),
        head_event_id: None,
    })
}

fn parse_revision(event: Event, expected_project: Uuid) -> Result<ParsedProjection, String> {
    let content: DocumentRevisionProjection =
        serde_json::from_str(&event.content).map_err(|error| error.to_string())?;
    content.validate().map_err(|error| error.to_string())?;
    assert_content_roundtrip(&event.content, &content)?;
    let (
        project,
        generation,
        catalog_revision,
        document_id,
        document_revision,
        state_tag,
        source_event_id,
        canonical_at,
    ) = match &content {
        DocumentRevisionProjection::Active {
            project_id,
            projection_generation,
            catalog_revision,
            document_id,
            document_revision,
            source_event_id,
            revision_at,
            ..
        } => (
            *project_id,
            *projection_generation,
            *catalog_revision,
            *document_id,
            *document_revision,
            "buzz-project-document-active",
            *source_event_id,
            *revision_at,
        ),
        DocumentRevisionProjection::Deleted {
            project_id,
            projection_generation,
            catalog_revision,
            document_id,
            document_revision,
            source_event_id,
            revision_at,
            ..
        } => (
            *project_id,
            *projection_generation,
            *catalog_revision,
            *document_id,
            *document_revision,
            "buzz-project-document-tombstone",
            *source_event_id,
            *revision_at,
        ),
    };
    require_project_time(&event, project, expected_project, canonical_at)?;
    require_exact_tags(
        &event,
        vec![
            vec!["-".to_owned()],
            vec![
                "d".to_owned(),
                document_revision_coordinate(project, document_id, document_revision),
            ],
            vec!["t".to_owned(), "buzz-project-document".to_owned()],
            vec!["t".to_owned(), "buzz-project-document-revision".to_owned()],
            vec!["t".to_owned(), state_tag.to_owned()],
            vec!["projection_generation".to_owned(), generation.to_string()],
            vec!["catalog_revision".to_owned(), catalog_revision.to_string()],
            vec![
                "document_revision".to_owned(),
                document_revision.to_string(),
            ],
            vec![
                "e".to_owned(),
                source_event_id.to_hex(),
                String::new(),
                "source".to_owned(),
            ],
        ],
    )?;
    Ok(ParsedProjection {
        event_id: event.id,
        source_event_id: Some(source_event_id),
        revision_event_id: None,
        head_event_id: None,
    })
}

fn parse_meta(event: Event, expected_project: Uuid) -> Result<ParsedProjection, String> {
    let content: DocumentMetaProjection =
        serde_json::from_str(&event.content).map_err(|error| error.to_string())?;
    content.validate().map_err(|error| error.to_string())?;
    assert_content_roundtrip(&event.content, &content)?;
    require_project_time(
        &event,
        content.project_id,
        expected_project,
        content.updated_at,
    )?;
    let mut expected = vec![
        vec!["-".to_owned()],
        vec!["d".to_owned(), document_meta_coordinate(content.project_id)],
        vec!["t".to_owned(), "buzz-project-document".to_owned()],
        vec!["t".to_owned(), "buzz-project-document-meta".to_owned()],
        vec![
            "projection_generation".to_owned(),
            content.projection_generation.to_string(),
        ],
        vec![
            "catalog_revision".to_owned(),
            content.catalog_revision.to_string(),
        ],
    ];
    if let Some(source) = content.source_event_id {
        expected.push(vec![
            "e".to_owned(),
            source.to_hex(),
            String::new(),
            "source".to_owned(),
        ]);
    }
    require_exact_tags(&event, expected)?;
    let changed = content.changed_heads.first();
    Ok(ParsedProjection {
        event_id: event.id,
        source_event_id: content.source_event_id,
        revision_event_id: changed.map(|head| head.revision_event_id),
        head_event_id: changed.map(|head| head.head_event_id),
    })
}

fn require_project_time(
    event: &Event,
    project: Uuid,
    expected_project: Uuid,
    canonical_at: DateTime<Utc>,
) -> Result<(), String> {
    if project != expected_project {
        return Err("cross-Project projection".to_owned());
    }
    let seconds = u64::try_from(canonical_at.timestamp()).map_err(|error| error.to_string())?;
    if event.created_at.as_secs() != seconds {
        return Err("event created_at does not match canonical projection time".to_owned());
    }
    Ok(())
}

fn require_exact_tags(event: &Event, expected: Vec<Vec<String>>) -> Result<(), String> {
    let actual = event
        .tags
        .iter()
        .map(|tag| tag.as_slice().to_vec())
        .collect::<Vec<_>>();
    if actual == expected {
        Ok(())
    } else {
        Err("tags are not the exact canonical sequence".to_owned())
    }
}

fn assert_content_roundtrip<T: Serialize>(raw: &str, value: &T) -> Result<(), String> {
    let before: Value = serde_json::from_str(raw).map_err(|error| error.to_string())?;
    let after = serde_json::to_value(value).map_err(|error| error.to_string())?;
    if before == after {
        Ok(())
    } else {
        Err("content did not roundtrip".to_owned())
    }
}

fn assert_json_roundtrip<T: Serialize>(raw: &str, value: &T) {
    assert_content_roundtrip(raw, value).expect("JSON semantic roundtrip");
}

fn parse_event(raw: &str) -> Event {
    Event::from_json(raw).expect("event fixture")
}

fn parse_fixture<T: DeserializeOwned>(raw: &str) -> T {
    serde_json::from_str(raw).expect("JSON fixture")
}

fn projection_content<T: DeserializeOwned>(raw: &str) -> T {
    let event = parse_event(raw);
    serde_json::from_str(&event.content).expect("projection content")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BaseRoleBriefV3Fixture {
    project_view_schema_version: u16,
    generated_at: DateTime<Utc>,
    project_id: Uuid,
    project_revision: u64,
    projection_generation: u64,
    member_pubkey: PublicKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    community_role: Option<CommunityMemberRole>,
    project: Value,
    state: Value,
    responsible_work: Vec<Value>,
    related_objects: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    latest_checkpoint: Option<Value>,
    recent_handoffs: Vec<Value>,
    context: RoleBriefContextV3,
    source_revisions: RoleBriefSourceRevisionsV3,
}

impl BaseRoleBriefV3Fixture {
    fn validate(&self) -> Result<(), String> {
        if self.project_view_schema_version != 3 {
            return Err("RoleBriefV3 schema must be three".to_owned());
        }
        if !matches!(
            self.context.availability,
            ContextAvailabilityV3::NotAdvertisedEmpty
        ) || !self.context.resources.is_empty()
            || !self.context.live_documents.is_empty()
            || !self.context.pinned_documents.is_empty()
            || self.context.truncation.truncated
            || self.context.truncation.omitted_resources != 0
            || self.context.truncation.omitted_live_documents != 0
            || self.context.truncation.omitted_pinned_documents != 0
            || !matches!(
                self.source_revisions.document_metadata,
                DocumentMetadataSourceV3::NotRequired
            )
        {
            return Err("base Context-off RoleBriefV3 gate is inconsistent".to_owned());
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyResourceEnvelope {
    schema_version: u16,
    resource_id: Uuid,
    object_revision: u64,
    project_revision: u64,
    state: CanonicalLegacyObjectStateV1,
    resource_data: Option<ProjectResource>,
    relations: ProjectViewRelations,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestEnvelope {
    schema_version: u16,
    community_id: String,
    base_meta_event_id: String,
    base_project_revision: u64,
    base_projection_generation: u64,
    entries: Vec<ManifestEntryEnvelope>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestEntryEnvelope {
    resource_id: String,
    legacy_object_revision: u64,
    legacy_projection_event_id: String,
    legacy_body_digest: String,
    reviewed_v3_payload: ManifestPayloadEnvelope,
    v3_payload_digest: String,
    guide_document_revision: u64,
    guide_head_event_id: String,
    guide_revision_event_id: String,
    guide_content_digest: String,
    mapping_entry_digest: String,
    reviewed_by_pubkey: String,
    reviewed_at_unix_micros: i64,
    review_digest: String,
    review_signature: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestPayloadEnvelope {
    resource_data: ManifestResourceEnvelope,
    context_references: Vec<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestResourceEnvelope {
    name: String,
    resource_kind: String,
    summary: Option<String>,
    guide_document_id: String,
}

impl ManifestEnvelope {
    fn into_canonical(self) -> Result<ResourceMappingManifestV1, String> {
        let community_id = decode_array(&self.community_id)?;
        let base_meta_event_id = decode_array(&self.base_meta_event_id)?;
        let entries = self
            .entries
            .into_iter()
            .map(|entry| {
                if !entry.reviewed_v3_payload.context_references.is_empty() {
                    return Err("cutover Context must be empty".to_owned());
                }
                Ok(ReviewedResourceMappingV1 {
                    resource_id: decode_array(&entry.resource_id)?,
                    legacy_object_revision: entry.legacy_object_revision,
                    legacy_projection_event_id: decode_array(&entry.legacy_projection_event_id)?,
                    legacy_body_digest: decode_array(&entry.legacy_body_digest)?,
                    reviewed_v3_payload: CanonicalResourceCutoverV1 {
                        resource_data: CanonicalProjectResourceV3 {
                            name: entry.reviewed_v3_payload.resource_data.name,
                            resource_kind: entry.reviewed_v3_payload.resource_data.resource_kind,
                            summary: entry.reviewed_v3_payload.resource_data.summary,
                            guide_document_id: decode_array(
                                &entry.reviewed_v3_payload.resource_data.guide_document_id,
                            )?,
                        },
                        context_references: Vec::new(),
                    },
                    v3_payload_digest: decode_array(&entry.v3_payload_digest)?,
                    guide_document_revision: entry.guide_document_revision,
                    guide_head_event_id: decode_array(&entry.guide_head_event_id)?,
                    guide_revision_event_id: decode_array(&entry.guide_revision_event_id)?,
                    guide_content_digest: decode_array(&entry.guide_content_digest)?,
                    mapping_entry_digest: decode_array(&entry.mapping_entry_digest)?,
                    reviewed_by_pubkey: decode_array(&entry.reviewed_by_pubkey)?,
                    reviewed_at_unix_micros: entry.reviewed_at_unix_micros,
                    review_digest: decode_array(&entry.review_digest)?,
                    review_signature: ReviewSignature::from_bytes(decode_array(
                        &entry.review_signature,
                    )?),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(ResourceMappingManifestV1 {
            schema_version: self.schema_version,
            community_id,
            base_meta_event_id,
            base_project_revision: self.base_project_revision,
            base_projection_generation: self.base_projection_generation,
            entries,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MigrationGolden {
    postcard_manifest_hex: String,
    legacy_body_digest: String,
    v3_payload_digest: String,
    guide_content_digest: String,
    mapping_entry_digest: String,
    review_digest: String,
    review_signature: String,
    manifest_digest: String,
}

fn decode_array<const N: usize>(value: &str) -> Result<[u8; N], String> {
    hex::decode(value)
        .map_err(|error| error.to_string())?
        .try_into()
        .map_err(|bytes: Vec<u8>| format!("expected {N} bytes, got {}", bytes.len()))
}

#[test]
fn migration_digest_helpers_recompute_every_entry_boundary() {
    let envelope: ManifestEnvelope = parse_fixture(fixture!("migration/manifest.json"));
    let manifest = envelope.into_canonical().expect("canonical manifest");
    let entry = &manifest.entries[0];
    assert_eq!(
        resource_cutover_payload_digest(&entry.reviewed_v3_payload).expect("payload digest"),
        entry.v3_payload_digest
    );
    let mapping = CanonicalResourceMappingEntryV1 {
        community_id: manifest.community_id,
        base_meta_event_id: manifest.base_meta_event_id,
        base_project_revision: manifest.base_project_revision,
        base_projection_generation: manifest.base_projection_generation,
        resource_id: entry.resource_id,
        legacy_object_revision: entry.legacy_object_revision,
        legacy_projection_event_id: entry.legacy_projection_event_id,
        legacy_body_digest: entry.legacy_body_digest,
        v3_payload_digest: entry.v3_payload_digest,
        guide_document_id: entry.reviewed_v3_payload.resource_data.guide_document_id,
        guide_document_revision: entry.guide_document_revision,
        guide_head_event_id: entry.guide_head_event_id,
        guide_revision_event_id: entry.guide_revision_event_id,
        guide_content_digest: entry.guide_content_digest,
    };
    assert_eq!(
        mapping_entry_digest(&mapping).expect("mapping digest"),
        entry.mapping_entry_digest
    );
    let review = CanonicalResourceReviewV1 {
        mapping_entry_digest: entry.mapping_entry_digest,
        reviewed_by_pubkey: entry.reviewed_by_pubkey,
        reviewed_at_unix_micros: entry.reviewed_at_unix_micros,
    };
    assert_eq!(
        review_digest(&review).expect("review digest"),
        entry.review_digest
    );
}

#[test]
fn document_projection_types_reject_cross_subtype_content() {
    let raw = parse_event(fixture!("events/head-active.json")).content;
    assert!(serde_json::from_str::<DocumentRevisionProjection>(&raw).is_err());
    let raw = parse_event(fixture!("events/revision-active.json")).content;
    assert!(serde_json::from_str::<DocumentHeadProjection>(&raw).is_err());
    assert!(serde_json::from_str::<DocumentMetaProjection>(&raw).is_err());
    let value: Value = serde_json::from_str(&raw).expect("revision JSON");
    assert_eq!(
        value["projection_type"],
        serde_json::to_value(DocumentProjectionType::DocumentRevision)
            .expect("projection discriminator")
    );
}
