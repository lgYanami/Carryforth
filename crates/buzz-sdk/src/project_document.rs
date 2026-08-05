//! Project Document v1 command and Relay projection builders/verifiers.

use std::collections::HashSet;

use buzz_core::kind::{
    KIND_PROJECT_DOCUMENT_COMMAND, KIND_PROJECT_DOCUMENT_HEAD, KIND_PROJECT_DOCUMENT_META,
    KIND_PROJECT_DOCUMENT_REVISION,
};
use buzz_core::{CommunityId, EventId, PublicKey};
use buzz_project_document::{
    document_head_coordinate as domain_head_coordinate,
    document_meta_coordinate as domain_meta_coordinate,
    document_revision_coordinate as domain_revision_coordinate, ChangedDocumentHead,
    DocumentCommandRequest, DocumentHeadProjection, DocumentMetaProjection, DocumentProjectionPlan,
    DocumentProjectionType, DocumentRevision, DocumentRevisionProjection, DocumentState,
    ProjectDocumentCommand, MAX_SAFE_REVISION, PROJECT_DOCUMENT_COMMAND_TAG,
    PROJECT_DOCUMENT_PROJECTION_TAG, PROJECT_DOCUMENT_SCHEMA_VERSION,
};
use chrono::{DateTime, Utc};
use nostr::{Event, EventBuilder, Kind, Tag, Timestamp};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::SdkError;

const HEAD_TAG: &str = "buzz-project-document-head";
const REVISION_TAG: &str = "buzz-project-document-revision";
const META_TAG: &str = "buzz-project-document-meta";
const ACTIVE_TAG: &str = "buzz-project-document-active";
const TOMBSTONE_TAG: &str = "buzz-project-document-tombstone";

/// Build an unsigned Human-authored create command.
pub fn build_create_document(
    document_id: Uuid,
    title: String,
    summary: Option<String>,
    content_markdown: String,
) -> Result<EventBuilder, SdkError> {
    build_document_command(ProjectDocumentCommand::new(
        0,
        DocumentCommandRequest::Create {
            document_id,
            title,
            summary,
            content_markdown,
        },
    ))
}

/// Build an unsigned Human-authored full-snapshot update command.
pub fn build_update_document(
    expected_document_revision: u64,
    document_id: Uuid,
    title: String,
    summary: Option<String>,
    content_markdown: String,
) -> Result<EventBuilder, SdkError> {
    build_document_command(ProjectDocumentCommand::new(
        expected_document_revision,
        DocumentCommandRequest::Update {
            document_id,
            title,
            summary,
            content_markdown,
        },
    ))
}

/// Build an unsigned Human-authored tombstone command.
pub fn build_delete_document(
    expected_document_revision: u64,
    document_id: Uuid,
) -> Result<EventBuilder, SdkError> {
    build_document_command(ProjectDocumentCommand::new(
        expected_document_revision,
        DocumentCommandRequest::Delete { document_id },
    ))
}

/// Build any validated Human or managed-Agent Document command.
pub fn build_document_command(command: ProjectDocumentCommand) -> Result<EventBuilder, SdkError> {
    command
        .validate_for_submission()
        .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    let content = canonical_json(&command, "serialize Project Document command")?;
    ProjectDocumentCommand::from_json(&content)
        .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    Ok(
        EventBuilder::new(Kind::Custom(KIND_PROJECT_DOCUMENT_COMMAND as u16), content)
            .tags([tag(["-"])?, tag(["t", PROJECT_DOCUMENT_COMMAND_TAG])?]),
    )
}

/// Parse a signed member command with exact kind, tags, closed JSON, and
/// canonical scalar spellings.
pub fn parse_document_command(event: &Event) -> Result<ProjectDocumentCommand, SdkError> {
    event
        .verify()
        .map_err(|error| SdkError::InvalidInput(format!("invalid command signature: {error}")))?;
    if u32::from(event.kind.as_u16()) != KIND_PROJECT_DOCUMENT_COMMAND {
        return Err(SdkError::InvalidInput(format!(
            "Project Document command kind must be {KIND_PROJECT_DOCUMENT_COMMAND}"
        )));
    }
    require_exact_tags(
        event,
        &[
            vec!["-".to_owned()],
            vec!["t".to_owned(), PROJECT_DOCUMENT_COMMAND_TAG.to_owned()],
        ],
        "command",
    )?;
    let raw: Value = serde_json::from_str(&event.content)
        .map_err(|error| SdkError::InvalidInput(format!("invalid command JSON: {error}")))?;
    let command = ProjectDocumentCommand::from_json(&event.content)
        .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    require_canonical_value(&raw, &command, "command")
        .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    Ok(command)
}

/// Verified current-head event and closed content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedDocumentHead {
    /// Signed event identifier.
    pub event_id: EventId,
    /// Verified Relay signer.
    pub signer: PublicKey,
    /// Strict active or tombstone head.
    pub projection: DocumentHeadProjection,
}

/// Verified immutable revision event and closed content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedDocumentRevision {
    /// Signed event identifier.
    pub event_id: EventId,
    /// Verified Relay signer.
    pub signer: PublicKey,
    /// Strict full snapshot or bodyless tombstone.
    pub projection: DocumentRevisionProjection,
}

/// Verified catalog metadata event and closed content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedDocumentMeta {
    /// Signed event identifier.
    pub event_id: EventId,
    /// Verified Relay signer.
    pub signer: PublicKey,
    /// Strict reset or incremental catalog observation.
    pub projection: DocumentMetaProjection,
}

/// A head and immutable revision whose exact pointer and business metadata
/// have been verified together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCurrentDocument {
    /// Verified lightweight current head.
    pub head: VerifiedDocumentHead,
    /// Exact immutable revision named by the head.
    pub revision: VerifiedDocumentRevision,
}

impl VerifiedCurrentDocument {
    /// Bind a verified head to its exact verified immutable revision.
    pub fn new(
        head: VerifiedDocumentHead,
        revision: VerifiedDocumentRevision,
    ) -> Result<Self, SdkError> {
        if head.signer != revision.signer
            || head_revision_event_id(&head.projection) != revision.event_id
        {
            return Err(invalid_projection(
                "Document head does not point at the supplied Relay revision",
            ));
        }
        let head_common = head_common(&head.projection);
        let revision_common = revision_common(&revision.projection);
        if head_common != revision_common {
            return Err(invalid_projection(
                "Document head and revision identity, generation, catalog, state, or source disagree",
            ));
        }
        match (&head.projection, &revision.projection) {
            (
                DocumentHeadProjection::Active {
                    title: head_title,
                    summary: head_summary,
                    created_at: head_created_at,
                    created_by: head_created_by,
                    updated_at,
                    updated_by,
                    ..
                },
                DocumentRevisionProjection::Active {
                    title: revision_title,
                    summary: revision_summary,
                    created_at: revision_created_at,
                    created_by: revision_created_by,
                    revision_at,
                    revision_by,
                    ..
                },
            ) if head_title == revision_title
                && head_summary == revision_summary
                && head_created_at == revision_created_at
                && head_created_by == revision_created_by
                && updated_at == revision_at
                && updated_by == revision_by => {}
            (
                DocumentHeadProjection::Deleted {
                    created_at: head_created_at,
                    created_by: head_created_by,
                    deleted_at,
                    deleted_by,
                    ..
                },
                DocumentRevisionProjection::Deleted {
                    created_at: revision_created_at,
                    created_by: revision_created_by,
                    revision_at,
                    revision_by,
                    ..
                },
            ) if head_created_at == revision_created_at
                && head_created_by == revision_created_by
                && deleted_at == revision_at
                && deleted_by == revision_by => {}
            _ => {
                return Err(invalid_projection(
                    "Document head metadata does not match its immutable revision",
                ));
            }
        }
        Ok(Self { head, revision })
    }
}

/// Derive the canonical current-head coordinate.
#[must_use]
pub fn document_head_coordinate(project_id: CommunityId, document_id: Uuid) -> String {
    domain_head_coordinate(*project_id.as_uuid(), document_id)
}

/// Derive the canonical immutable-revision coordinate.
#[must_use]
pub fn document_revision_coordinate(
    project_id: CommunityId,
    document_id: Uuid,
    document_revision: u64,
) -> String {
    domain_revision_coordinate(*project_id.as_uuid(), document_id, document_revision)
}

/// Derive the canonical catalog metadata coordinate.
#[must_use]
pub fn document_meta_coordinate(project_id: CommunityId) -> String {
    domain_meta_coordinate(*project_id.as_uuid())
}

/// Build the unsigned immutable revision event for one mutation plan.
pub fn build_document_revision_projection(
    plan: &DocumentProjectionPlan,
) -> Result<EventBuilder, SdkError> {
    let projection = revision_projection_from_plan(plan)?;
    build_document_revision_reprojection(&projection)
}

/// Build an unsigned immutable revision from an explicitly reconstructed
/// canonical projection. This is reserved for generation reprojection: normal
/// writes must use [`build_document_revision_projection`] and a reducer plan.
pub fn build_document_revision_reprojection(
    projection: &DocumentRevisionProjection,
) -> Result<EventBuilder, SdkError> {
    projection
        .validate()
        .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    let common = revision_common(projection);
    let project_id = CommunityId::from_uuid(common.project_id);
    let coordinate =
        document_revision_coordinate(project_id, common.document_id, common.document_revision);
    let state_tag = lifecycle_tag(common.state);
    let source = common.source_event_id.to_hex();
    let tags = vec![
        tag(["-"])?,
        tag(["d", coordinate.as_str()])?,
        tag(["t", PROJECT_DOCUMENT_PROJECTION_TAG])?,
        tag(["t", REVISION_TAG])?,
        tag(["t", state_tag])?,
        tag([
            "projection_generation",
            &canonical_decimal(common.projection_generation),
        ])?,
        tag([
            "catalog_revision",
            &canonical_decimal(common.catalog_revision),
        ])?,
        tag([
            "document_revision",
            &canonical_decimal(common.document_revision),
        ])?,
        tag(["e", source.as_str(), "", "source"])?,
    ];
    Ok(EventBuilder::new(
        Kind::Custom(KIND_PROJECT_DOCUMENT_REVISION as u16),
        canonical_json(projection, "serialize Document revision projection")?,
    )
    .tags(tags)
    .custom_created_at(timestamp(revision_timestamp(projection))?))
}

/// Build the unsigned current head after the immutable revision is signed.
pub fn build_document_head_projection(
    plan: &DocumentProjectionPlan,
    revision_event: &Event,
) -> Result<EventBuilder, SdkError> {
    let project_id = plan.catalog().project_id();
    let verified_revision =
        parse_document_revision(revision_event, &revision_event.pubkey, project_id)?;
    if verified_revision.projection != revision_projection_from_plan(plan)? {
        return Err(SdkError::InvalidInput(
            "signed revision does not match the projection plan".to_owned(),
        ));
    }
    let projection = head_projection_from_plan(plan, revision_event.id)?;
    build_document_head_reprojection(&projection, revision_event)
}

/// Build an unsigned current head from an explicitly reconstructed canonical
/// projection and its already-signed revision event. This is reserved for
/// generation reprojection.
pub fn build_document_head_reprojection(
    projection: &DocumentHeadProjection,
    revision_event: &Event,
) -> Result<EventBuilder, SdkError> {
    projection
        .validate()
        .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    let common = head_common(projection);
    let project_id = CommunityId::from_uuid(common.project_id);
    let revision = parse_document_revision(revision_event, &revision_event.pubkey, project_id)?;
    let revision_common = revision_common(&revision.projection);
    if revision.event_id != head_revision_event_id(projection) || revision_common != common {
        return Err(SdkError::InvalidInput(
            "reprojected Document head does not match its signed revision".to_owned(),
        ));
    }
    let coordinate = document_head_coordinate(project_id, common.document_id);
    let state_tag = lifecycle_tag(common.state);
    let source = common.source_event_id.to_hex();
    let revision_id = revision_event.id.to_hex();
    let tags = vec![
        tag(["-"])?,
        tag(["d", coordinate.as_str()])?,
        tag(["t", PROJECT_DOCUMENT_PROJECTION_TAG])?,
        tag(["t", HEAD_TAG])?,
        tag(["t", state_tag])?,
        tag([
            "projection_generation",
            &canonical_decimal(common.projection_generation),
        ])?,
        tag([
            "catalog_revision",
            &canonical_decimal(common.catalog_revision),
        ])?,
        tag([
            "document_revision",
            &canonical_decimal(common.document_revision),
        ])?,
        tag(["e", revision_id.as_str(), "", "revision"])?,
        tag(["e", source.as_str(), "", "source"])?,
    ];
    Ok(EventBuilder::new(
        Kind::Custom(KIND_PROJECT_DOCUMENT_HEAD as u16),
        canonical_json(projection, "serialize Document head projection")?,
    )
    .tags(tags)
    .custom_created_at(timestamp(head_timestamp(projection))?))
}

/// Bind signed head and revision events into one incremental metadata entry.
pub fn changed_head_for(
    plan: &DocumentProjectionPlan,
    head_event: &Event,
    revision_event: &Event,
) -> Result<ChangedDocumentHead, SdkError> {
    if head_event.pubkey != revision_event.pubkey {
        return Err(SdkError::InvalidInput(
            "Document head and revision use different signers".to_owned(),
        ));
    }
    let project_id = plan.catalog().project_id();
    let head = parse_document_head(head_event, &head_event.pubkey, project_id)?;
    let revision = parse_document_revision(revision_event, &head_event.pubkey, project_id)?;
    let current = VerifiedCurrentDocument::new(head, revision)?;
    if current.revision.projection != revision_projection_from_plan(plan)?
        || current.head.projection != head_projection_from_plan(plan, revision_event.id)?
    {
        return Err(SdkError::InvalidInput(
            "signed head/revision bundle does not match the projection plan".to_owned(),
        ));
    }
    let plan_current = plan.current().ok_or_else(|| {
        SdkError::InvalidInput("bootstrap has no changed Document head".to_owned())
    })?;
    Ok(ChangedDocumentHead {
        head_coordinate: document_head_coordinate(
            project_id,
            plan_current.document().document_id(),
        ),
        head_event_id: head_event.id,
        document_id: plan_current.document().document_id(),
        document_revision: plan_current.document().current_revision(),
        revision_event_id: revision_event.id,
        deleted: plan_current.document().state() == DocumentState::Deleted,
    })
}

/// Build unsigned reset/bootstrap or incremental catalog metadata.
pub fn build_document_meta_projection(
    plan: &DocumentProjectionPlan,
    changed_heads: &[ChangedDocumentHead],
) -> Result<EventBuilder, SdkError> {
    if plan.reset() {
        if !changed_heads.is_empty() || plan.source_event_id().is_some() || plan.current().is_some()
        {
            return Err(SdkError::InvalidInput(
                "reset metadata must have no changed head, source, or current Document".to_owned(),
            ));
        }
    } else {
        let current = plan.current().ok_or_else(|| {
            SdkError::InvalidInput("mutation metadata has no current Document".to_owned())
        })?;
        if changed_heads.len() != 1
            || changed_heads[0].document_id != current.document().document_id()
            || changed_heads[0].document_revision != current.document().current_revision()
            || changed_heads[0].deleted != (current.document().state() == DocumentState::Deleted)
            || changed_heads[0].head_coordinate
                != document_head_coordinate(
                    plan.catalog().project_id(),
                    current.document().document_id(),
                )
        {
            return Err(SdkError::InvalidInput(
                "incremental metadata changed head does not match the plan".to_owned(),
            ));
        }
    }
    let catalog = plan.catalog();
    let projection = DocumentMetaProjection {
        schema_version: PROJECT_DOCUMENT_SCHEMA_VERSION,
        projection_type: DocumentProjectionType::DocumentMeta,
        project_id: *catalog.project_id().as_uuid(),
        initialized: true,
        projection_generation: catalog.projection_generation(),
        catalog_revision: catalog.catalog_revision(),
        active_document_count: catalog.active_document_count(),
        reset: plan.reset(),
        changed_heads: changed_heads.to_vec(),
        source_event_id: plan.source_event_id(),
        updated_at: catalog.updated_at(),
    };
    projection
        .validate()
        .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    let coordinate = document_meta_coordinate(catalog.project_id());
    let mut tags = vec![
        tag(["-"])?,
        tag(["d", coordinate.as_str()])?,
        tag(["t", PROJECT_DOCUMENT_PROJECTION_TAG])?,
        tag(["t", META_TAG])?,
        tag([
            "projection_generation",
            &canonical_decimal(catalog.projection_generation()),
        ])?,
        tag([
            "catalog_revision",
            &canonical_decimal(catalog.catalog_revision()),
        ])?,
    ];
    if let Some(source) = plan.source_event_id() {
        tags.push(tag(["e", &source.to_hex(), "", "source"])?);
    }
    Ok(EventBuilder::new(
        Kind::Custom(KIND_PROJECT_DOCUMENT_META as u16),
        canonical_json(&projection, "serialize Document metadata projection")?,
    )
    .tags(tags)
    .custom_created_at(timestamp(catalog.updated_at())?))
}

/// Parse and verify one current head for an expected Community and Relay.
pub fn parse_document_head(
    event: &Event,
    expected_relay: &PublicKey,
    expected_project: CommunityId,
) -> Result<VerifiedDocumentHead, SdkError> {
    verify_projection_envelope(event, expected_relay, KIND_PROJECT_DOCUMENT_HEAD)?;
    let (raw, projection) = parse_projection_content::<DocumentHeadProjection>(event, "head")?;
    projection
        .validate()
        .map_err(|error| invalid_projection(error.to_string()))?;
    require_project(head_common(&projection).project_id, expected_project)?;
    require_event_time(event, head_timestamp(&projection), "head")?;
    let common = head_common(&projection);
    let expected_tags = vec![
        vec!["-".to_owned()],
        vec![
            "d".to_owned(),
            domain_head_coordinate(common.project_id, common.document_id),
        ],
        vec!["t".to_owned(), PROJECT_DOCUMENT_PROJECTION_TAG.to_owned()],
        vec!["t".to_owned(), HEAD_TAG.to_owned()],
        vec!["t".to_owned(), lifecycle_tag(common.state).to_owned()],
        vec![
            "projection_generation".to_owned(),
            canonical_decimal(common.projection_generation),
        ],
        vec![
            "catalog_revision".to_owned(),
            canonical_decimal(common.catalog_revision),
        ],
        vec![
            "document_revision".to_owned(),
            canonical_decimal(common.document_revision),
        ],
        vec![
            "e".to_owned(),
            head_revision_event_id(&projection).to_hex(),
            String::new(),
            "revision".to_owned(),
        ],
        vec![
            "e".to_owned(),
            common.source_event_id.to_hex(),
            String::new(),
            "source".to_owned(),
        ],
    ];
    require_exact_tags(event, &expected_tags, "head")?;
    require_canonical_value(&raw, &projection, "head")?;
    Ok(VerifiedDocumentHead {
        event_id: event.id,
        signer: event.pubkey,
        projection,
    })
}

/// Parse and verify one immutable revision for an expected Community and Relay.
pub fn parse_document_revision(
    event: &Event,
    expected_relay: &PublicKey,
    expected_project: CommunityId,
) -> Result<VerifiedDocumentRevision, SdkError> {
    verify_projection_envelope(event, expected_relay, KIND_PROJECT_DOCUMENT_REVISION)?;
    let (raw, projection) =
        parse_projection_content::<DocumentRevisionProjection>(event, "revision")?;
    projection
        .validate()
        .map_err(|error| invalid_projection(error.to_string()))?;
    let common = revision_common(&projection);
    require_project(common.project_id, expected_project)?;
    require_event_time(event, revision_timestamp(&projection), "revision")?;
    let expected_tags = vec![
        vec!["-".to_owned()],
        vec![
            "d".to_owned(),
            domain_revision_coordinate(
                common.project_id,
                common.document_id,
                common.document_revision,
            ),
        ],
        vec!["t".to_owned(), PROJECT_DOCUMENT_PROJECTION_TAG.to_owned()],
        vec!["t".to_owned(), REVISION_TAG.to_owned()],
        vec!["t".to_owned(), lifecycle_tag(common.state).to_owned()],
        vec![
            "projection_generation".to_owned(),
            canonical_decimal(common.projection_generation),
        ],
        vec![
            "catalog_revision".to_owned(),
            canonical_decimal(common.catalog_revision),
        ],
        vec![
            "document_revision".to_owned(),
            canonical_decimal(common.document_revision),
        ],
        vec![
            "e".to_owned(),
            common.source_event_id.to_hex(),
            String::new(),
            "source".to_owned(),
        ],
    ];
    require_exact_tags(event, &expected_tags, "revision")?;
    require_canonical_value(&raw, &projection, "revision")?;
    Ok(VerifiedDocumentRevision {
        event_id: event.id,
        signer: event.pubkey,
        projection,
    })
}

/// Parse and verify one catalog metadata observation for an expected Relay.
pub fn parse_document_meta(
    event: &Event,
    expected_relay: &PublicKey,
) -> Result<VerifiedDocumentMeta, SdkError> {
    verify_projection_envelope(event, expected_relay, KIND_PROJECT_DOCUMENT_META)?;
    let (raw, projection) = parse_projection_content::<DocumentMetaProjection>(event, "metadata")?;
    projection
        .validate()
        .map_err(|error| invalid_projection(error.to_string()))?;
    require_event_time(event, projection.updated_at, "metadata")?;
    let mut document_ids = HashSet::new();
    let mut head_event_ids = HashSet::new();
    let mut revision_event_ids = HashSet::new();
    for changed in &projection.changed_heads {
        if !document_ids.insert(changed.document_id)
            || !head_event_ids.insert(changed.head_event_id)
            || !revision_event_ids.insert(changed.revision_event_id)
        {
            return Err(invalid_projection(
                "metadata contains duplicate changed-head identity or pointer",
            ));
        }
    }
    let mut expected_tags = vec![
        vec!["-".to_owned()],
        vec![
            "d".to_owned(),
            domain_meta_coordinate(projection.project_id),
        ],
        vec!["t".to_owned(), PROJECT_DOCUMENT_PROJECTION_TAG.to_owned()],
        vec!["t".to_owned(), META_TAG.to_owned()],
        vec![
            "projection_generation".to_owned(),
            canonical_decimal(projection.projection_generation),
        ],
        vec![
            "catalog_revision".to_owned(),
            canonical_decimal(projection.catalog_revision),
        ],
    ];
    if let Some(source) = projection.source_event_id {
        expected_tags.push(vec![
            "e".to_owned(),
            source.to_hex(),
            String::new(),
            "source".to_owned(),
        ]);
    }
    require_exact_tags(event, &expected_tags, "metadata")?;
    require_canonical_value(&raw, &projection, "metadata")?;
    Ok(VerifiedDocumentMeta {
        event_id: event.id,
        signer: event.pubkey,
        projection,
    })
}

/// Verify that one incremental metadata entry binds an already verified
/// current head and immutable revision.
pub fn verify_document_meta_change(
    meta: &VerifiedDocumentMeta,
    current: &VerifiedCurrentDocument,
) -> Result<(), SdkError> {
    if meta.signer != current.head.signer
        || meta.projection.project_id != head_common(&current.head.projection).project_id
        || meta.projection.projection_generation
            != head_common(&current.head.projection).projection_generation
        || meta.projection.catalog_revision
            != head_common(&current.head.projection).catalog_revision
        || meta.projection.source_event_id
            != Some(head_common(&current.head.projection).source_event_id)
        || meta.projection.changed_heads.len() != 1
    {
        return Err(invalid_projection(
            "metadata and current Document belong to different observations",
        ));
    }
    let common = head_common(&current.head.projection);
    let expected = ChangedDocumentHead {
        head_coordinate: domain_head_coordinate(common.project_id, common.document_id),
        head_event_id: current.head.event_id,
        document_id: common.document_id,
        document_revision: common.document_revision,
        revision_event_id: current.revision.event_id,
        deleted: common.state == DocumentState::Deleted,
    };
    if meta.projection.changed_heads[0] != expected {
        return Err(invalid_projection(
            "metadata changed head does not bind the supplied current Document",
        ));
    }
    Ok(())
}

/// Verify that one lightweight current head is visible within a signed
/// Document catalog observation without fetching its Markdown revision body.
///
/// Older heads only need to share the Project, signer, and projection
/// generation and remain at or before the observed catalog revision. When an
/// incremental metadata event and head share the current revision, the exact
/// changed-head pointer must bind that head.
pub fn verify_document_head_observation(
    meta: &VerifiedDocumentMeta,
    head: &VerifiedDocumentHead,
) -> Result<(), SdkError> {
    let common = head_common(&head.projection);
    if meta.signer != head.signer
        || meta.projection.project_id != common.project_id
        || meta.projection.projection_generation != common.projection_generation
        || common.catalog_revision > meta.projection.catalog_revision
    {
        return Err(invalid_projection(
            "Document head is outside the supplied metadata observation boundary",
        ));
    }
    if !meta.projection.reset && common.catalog_revision == meta.projection.catalog_revision {
        let expected = ChangedDocumentHead {
            head_coordinate: domain_head_coordinate(common.project_id, common.document_id),
            head_event_id: head.event_id,
            document_id: common.document_id,
            document_revision: common.document_revision,
            revision_event_id: head_revision_event_id(&head.projection),
            deleted: common.state == DocumentState::Deleted,
        };
        if meta.projection.source_event_id != Some(common.source_event_id)
            || meta.projection.changed_heads.as_slice() != [expected]
        {
            return Err(invalid_projection(
                "Document metadata changed-head entry does not bind the supplied head",
            ));
        }
    }
    Ok(())
}

/// Strictly verify a complete Relay-signed mutation projection bundle against
/// the deterministic pure-domain plan that produced it.
///
/// This is the database commit seam: callers cannot persist a signed bundle
/// that is internally valid but belongs to another transition.
pub fn verify_document_projection_bundle(
    plan: &DocumentProjectionPlan,
    revision_event: &Event,
    head_event: &Event,
    meta_event: &Event,
    expected_relay: &PublicKey,
) -> Result<VerifiedCurrentDocument, SdkError> {
    if plan.reset() || plan.current().is_none() || plan.source_event_id().is_none() {
        return Err(SdkError::InvalidInput(
            "a mutation projection bundle requires a non-reset transition plan".to_owned(),
        ));
    }
    let project_id = plan.catalog().project_id();
    let revision = parse_document_revision(revision_event, expected_relay, project_id)?;
    let head = parse_document_head(head_event, expected_relay, project_id)?;
    let current = VerifiedCurrentDocument::new(head, revision)?;
    let changed = changed_head_for(plan, head_event, revision_event)?;
    let meta = parse_document_meta(meta_event, expected_relay)?;
    verify_document_meta_change(&meta, &current)?;

    let expected_meta = DocumentMetaProjection {
        schema_version: PROJECT_DOCUMENT_SCHEMA_VERSION,
        projection_type: DocumentProjectionType::DocumentMeta,
        project_id: *project_id.as_uuid(),
        initialized: true,
        projection_generation: plan.catalog().projection_generation(),
        catalog_revision: plan.catalog().catalog_revision(),
        active_document_count: plan.catalog().active_document_count(),
        reset: false,
        changed_heads: vec![changed],
        source_event_id: plan.source_event_id(),
        updated_at: plan.catalog().updated_at(),
    };
    if meta.projection != expected_meta {
        return Err(invalid_projection(
            "metadata projection does not match the deterministic transition plan",
        ));
    }
    Ok(current)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProjectionCommon {
    project_id: Uuid,
    projection_generation: u64,
    catalog_revision: u64,
    document_id: Uuid,
    document_revision: u64,
    state: DocumentState,
    source_event_id: EventId,
}

fn revision_projection_from_plan(
    plan: &DocumentProjectionPlan,
) -> Result<DocumentRevisionProjection, SdkError> {
    let current = plan.current().ok_or_else(|| {
        SdkError::InvalidInput("bootstrap has no Document revision projection".to_owned())
    })?;
    let source_event_id = plan.source_event_id().ok_or_else(|| {
        SdkError::InvalidInput("mutation projection has no source event".to_owned())
    })?;
    let catalog = plan.catalog();
    let document = current.document();
    let created = document.created();
    let projection = match current.revision() {
        DocumentRevision::Active {
            snapshot,
            actor,
            canonical_at,
            ..
        } => DocumentRevisionProjection::Active {
            schema_version: PROJECT_DOCUMENT_SCHEMA_VERSION,
            projection_type: DocumentProjectionType::DocumentRevision,
            project_id: *catalog.project_id().as_uuid(),
            projection_generation: catalog.projection_generation(),
            catalog_revision: catalog.catalog_revision(),
            document_id: document.document_id(),
            document_revision: document.current_revision(),
            title: snapshot.title.clone(),
            summary: snapshot.summary.clone(),
            content_markdown: snapshot.content_markdown.clone(),
            created_at: created.at,
            created_by: created.by,
            revision_at: *canonical_at,
            revision_by: *actor,
            source_event_id,
        },
        DocumentRevision::Deleted {
            actor,
            canonical_at,
            ..
        } => DocumentRevisionProjection::Deleted {
            schema_version: PROJECT_DOCUMENT_SCHEMA_VERSION,
            projection_type: DocumentProjectionType::DocumentRevision,
            project_id: *catalog.project_id().as_uuid(),
            projection_generation: catalog.projection_generation(),
            catalog_revision: catalog.catalog_revision(),
            document_id: document.document_id(),
            document_revision: document.current_revision(),
            created_at: created.at,
            created_by: created.by,
            revision_at: *canonical_at,
            revision_by: *actor,
            source_event_id,
        },
    };
    projection
        .validate()
        .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    Ok(projection)
}

fn head_projection_from_plan(
    plan: &DocumentProjectionPlan,
    revision_event_id: EventId,
) -> Result<DocumentHeadProjection, SdkError> {
    let current = plan.current().ok_or_else(|| {
        SdkError::InvalidInput("bootstrap has no Document head projection".to_owned())
    })?;
    let source_event_id = plan.source_event_id().ok_or_else(|| {
        SdkError::InvalidInput("mutation projection has no source event".to_owned())
    })?;
    let catalog = plan.catalog();
    let document = current.document();
    let created = document.created();
    let updated = document.updated();
    let revision_coordinate = document_revision_coordinate(
        catalog.project_id(),
        document.document_id(),
        document.current_revision(),
    );
    let projection = match current.revision() {
        DocumentRevision::Active { snapshot, .. } => DocumentHeadProjection::Active {
            schema_version: PROJECT_DOCUMENT_SCHEMA_VERSION,
            projection_type: DocumentProjectionType::DocumentHead,
            project_id: *catalog.project_id().as_uuid(),
            projection_generation: catalog.projection_generation(),
            catalog_revision: catalog.catalog_revision(),
            document_id: document.document_id(),
            document_revision: document.current_revision(),
            title: snapshot.title.clone(),
            summary: snapshot.summary.clone(),
            created_at: created.at,
            created_by: created.by,
            updated_at: updated.at,
            updated_by: updated.by,
            revision_coordinate,
            revision_event_id,
            source_event_id,
        },
        DocumentRevision::Deleted { .. } => DocumentHeadProjection::Deleted {
            schema_version: PROJECT_DOCUMENT_SCHEMA_VERSION,
            projection_type: DocumentProjectionType::DocumentHead,
            project_id: *catalog.project_id().as_uuid(),
            projection_generation: catalog.projection_generation(),
            catalog_revision: catalog.catalog_revision(),
            document_id: document.document_id(),
            document_revision: document.current_revision(),
            created_at: created.at,
            created_by: created.by,
            deleted_at: updated.at,
            deleted_by: updated.by,
            revision_coordinate,
            revision_event_id,
            source_event_id,
        },
    };
    projection
        .validate()
        .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    Ok(projection)
}

fn head_common(projection: &DocumentHeadProjection) -> ProjectionCommon {
    match projection {
        DocumentHeadProjection::Active {
            project_id,
            projection_generation,
            catalog_revision,
            document_id,
            document_revision,
            source_event_id,
            ..
        } => ProjectionCommon {
            project_id: *project_id,
            projection_generation: *projection_generation,
            catalog_revision: *catalog_revision,
            document_id: *document_id,
            document_revision: *document_revision,
            state: DocumentState::Active,
            source_event_id: *source_event_id,
        },
        DocumentHeadProjection::Deleted {
            project_id,
            projection_generation,
            catalog_revision,
            document_id,
            document_revision,
            source_event_id,
            ..
        } => ProjectionCommon {
            project_id: *project_id,
            projection_generation: *projection_generation,
            catalog_revision: *catalog_revision,
            document_id: *document_id,
            document_revision: *document_revision,
            state: DocumentState::Deleted,
            source_event_id: *source_event_id,
        },
    }
}

fn revision_common(projection: &DocumentRevisionProjection) -> ProjectionCommon {
    match projection {
        DocumentRevisionProjection::Active {
            project_id,
            projection_generation,
            catalog_revision,
            document_id,
            document_revision,
            source_event_id,
            ..
        } => ProjectionCommon {
            project_id: *project_id,
            projection_generation: *projection_generation,
            catalog_revision: *catalog_revision,
            document_id: *document_id,
            document_revision: *document_revision,
            state: DocumentState::Active,
            source_event_id: *source_event_id,
        },
        DocumentRevisionProjection::Deleted {
            project_id,
            projection_generation,
            catalog_revision,
            document_id,
            document_revision,
            source_event_id,
            ..
        } => ProjectionCommon {
            project_id: *project_id,
            projection_generation: *projection_generation,
            catalog_revision: *catalog_revision,
            document_id: *document_id,
            document_revision: *document_revision,
            state: DocumentState::Deleted,
            source_event_id: *source_event_id,
        },
    }
}

fn head_revision_event_id(projection: &DocumentHeadProjection) -> EventId {
    match projection {
        DocumentHeadProjection::Active {
            revision_event_id, ..
        }
        | DocumentHeadProjection::Deleted {
            revision_event_id, ..
        } => *revision_event_id,
    }
}

fn head_timestamp(projection: &DocumentHeadProjection) -> DateTime<Utc> {
    match projection {
        DocumentHeadProjection::Active { updated_at, .. } => *updated_at,
        DocumentHeadProjection::Deleted { deleted_at, .. } => *deleted_at,
    }
}

fn revision_timestamp(projection: &DocumentRevisionProjection) -> DateTime<Utc> {
    match projection {
        DocumentRevisionProjection::Active { revision_at, .. }
        | DocumentRevisionProjection::Deleted { revision_at, .. } => *revision_at,
    }
}

fn lifecycle_tag(state: DocumentState) -> &'static str {
    match state {
        DocumentState::Active => ACTIVE_TAG,
        DocumentState::Deleted => TOMBSTONE_TAG,
    }
}

fn verify_projection_envelope(
    event: &Event,
    expected_relay: &PublicKey,
    expected_kind: u32,
) -> Result<(), SdkError> {
    event
        .verify()
        .map_err(|error| invalid_projection(format!("invalid event signature: {error}")))?;
    if event.pubkey != *expected_relay {
        return Err(invalid_projection(
            "projection signer does not match the expected Relay identity",
        ));
    }
    if u32::from(event.kind.as_u16()) != expected_kind {
        return Err(invalid_projection(format!(
            "projection kind must be {expected_kind}"
        )));
    }
    Ok(())
}

fn parse_projection_content<T>(event: &Event, label: &str) -> Result<(Value, T), SdkError>
where
    T: serde::de::DeserializeOwned,
{
    let raw: Value = serde_json::from_str(&event.content)
        .map_err(|error| invalid_projection(format!("invalid {label} content: {error}")))?;
    let projection = serde_json::from_value(raw.clone())
        .map_err(|error| invalid_projection(format!("invalid {label} content: {error}")))?;
    Ok((raw, projection))
}

fn require_canonical_value<T>(raw: &Value, parsed: &T, label: &str) -> Result<(), SdkError>
where
    T: Serialize,
{
    let canonical = serde_json::to_value(parsed)
        .map_err(|error| invalid_projection(format!("serialize {label}: {error}")))?;
    if *raw != canonical {
        return Err(invalid_projection(format!(
            "{label} contains a noncanonical scalar spelling"
        )));
    }
    Ok(())
}

fn require_project(project_id: Uuid, expected: CommunityId) -> Result<(), SdkError> {
    if project_id != *expected.as_uuid() {
        return Err(invalid_projection(
            "projection belongs to a different Project/Community",
        ));
    }
    Ok(())
}

fn require_event_time(
    event: &Event,
    canonical_at: DateTime<Utc>,
    label: &str,
) -> Result<(), SdkError> {
    let seconds = u64::try_from(canonical_at.timestamp())
        .map_err(|_| invalid_projection(format!("{label} time precedes the Unix epoch")))?;
    if event.created_at.as_secs() != seconds {
        return Err(invalid_projection(format!(
            "{label} event timestamp does not match canonical content time"
        )));
    }
    Ok(())
}

fn require_exact_tags(
    event: &Event,
    expected: &[Vec<String>],
    label: &str,
) -> Result<(), SdkError> {
    let actual: Vec<&[String]> = event.tags.iter().map(Tag::as_slice).collect();
    if actual.len() != expected.len()
        || actual
            .iter()
            .zip(expected)
            .any(|(actual, expected)| *actual != expected.as_slice())
    {
        return Err(invalid_projection(format!(
            "{label} tags are not the exact canonical tag sequence"
        )));
    }
    Ok(())
}

fn canonical_json<T: Serialize>(value: &T, context: &str) -> Result<String, SdkError> {
    serde_json::to_string(value)
        .map_err(|error| SdkError::InvalidInput(format!("{context}: {error}")))
}

fn tag<const N: usize>(parts: [&str; N]) -> Result<Tag, SdkError> {
    Tag::parse(parts).map_err(|error| SdkError::InvalidTag(error.to_string()))
}

fn timestamp(value: DateTime<Utc>) -> Result<Timestamp, SdkError> {
    let seconds = u64::try_from(value.timestamp())
        .map_err(|_| SdkError::InvalidInput("Document time precedes the Unix epoch".to_owned()))?;
    Ok(Timestamp::from(seconds))
}

fn canonical_decimal(value: u64) -> String {
    debug_assert!(value <= MAX_SAFE_REVISION);
    value.to_string()
}

fn invalid_projection(message: impl Into<String>) -> SdkError {
    SdkError::InvalidProjection(message.into())
}
