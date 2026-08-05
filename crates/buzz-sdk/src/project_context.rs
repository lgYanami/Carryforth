//! Project Context Edge v1 command and Relay projection builders/verifiers.

use buzz_core::kind::{
    KIND_PROJECT_CONTEXT_COMMAND, KIND_PROJECT_CONTEXT_EDGE_BINDING, KIND_PROJECT_CONTEXT_META,
};
use buzz_core::{CommunityId, EventId, PublicKey};
use buzz_project_context::{
    context_binding_coordinate as domain_binding_coordinate,
    context_edge_coordinate as domain_edge_coordinate,
    context_meta_coordinate as domain_meta_coordinate, ChangedContextBinding, EdgeKey,
    ProjectContextBindingProjection, ProjectContextCommand, ProjectContextCoordinate,
    ProjectContextMetaProjection, ProjectContextOperation, ProjectContextProjectionPlan,
    ProjectContextProjectionType, MAX_PROJECTION_CONTENT_BYTES, MAX_SAFE_REVISION,
    PROJECT_CONTEXT_COMMAND_TAG, PROJECT_CONTEXT_PROJECTION_TAG, PROJECT_CONTEXT_SCHEMA_VERSION,
};
use chrono::{DateTime, Utc};
use nostr::{Event, EventBuilder, JsonUtil, Kind, Tag, Timestamp};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::SdkError;

const BINDING_TAG: &str = "binding";
const META_TAG: &str = "meta";

/// Build an unsigned Human-authored attach command.
pub fn build_attach_context_document(
    project_id: CommunityId,
    expected_context_revision: u64,
    coordinates: Vec<ProjectContextCoordinate>,
    context_document_id: Uuid,
) -> Result<EventBuilder, SdkError> {
    let command = ProjectContextCommand::new(
        expected_context_revision,
        ProjectContextOperation::Attach,
        coordinates,
        context_document_id,
    )
    .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    build_project_context_command(project_id, command)
}

/// Build an unsigned Human-authored detach command.
pub fn build_detach_context_document(
    project_id: CommunityId,
    expected_context_revision: u64,
    coordinates: Vec<ProjectContextCoordinate>,
    context_document_id: Uuid,
) -> Result<EventBuilder, SdkError> {
    let command = ProjectContextCommand::new(
        expected_context_revision,
        ProjectContextOperation::Detach,
        coordinates,
        context_document_id,
    )
    .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    build_project_context_command(project_id, command)
}

/// Build any validated Human or managed-Agent Project Context command.
pub fn build_project_context_command(
    project_id: CommunityId,
    command: ProjectContextCommand,
) -> Result<EventBuilder, SdkError> {
    command
        .validate_for_project(*project_id.as_uuid())
        .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    let content = canonical_json(&command, "serialize Project Context command")?;
    ProjectContextCommand::from_json(&content)
        .and_then(|parsed| parsed.validate_for_project(*project_id.as_uuid()))
        .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    Ok(
        EventBuilder::new(Kind::Custom(KIND_PROJECT_CONTEXT_COMMAND as u16), content)
            .tags([tag(["-"])?, tag(["t", PROJECT_CONTEXT_COMMAND_TAG])?]),
    )
}

/// Parse a signed member command with exact kind, tags, closed JSON, and Project identity.
pub fn parse_project_context_command(
    event: &Event,
    expected_project: CommunityId,
) -> Result<ProjectContextCommand, SdkError> {
    event
        .verify()
        .map_err(|error| SdkError::InvalidInput(format!("invalid command signature: {error}")))?;
    if u32::from(event.kind.as_u16()) != KIND_PROJECT_CONTEXT_COMMAND {
        return Err(SdkError::InvalidInput(format!(
            "Project Context command kind must be {KIND_PROJECT_CONTEXT_COMMAND}"
        )));
    }
    require_exact_tags(
        event,
        &[
            vec!["-".to_owned()],
            vec!["t".to_owned(), PROJECT_CONTEXT_COMMAND_TAG.to_owned()],
        ],
        "command",
    )?;
    let raw: Value = serde_json::from_str(&event.content)
        .map_err(|error| SdkError::InvalidInput(format!("invalid command JSON: {error}")))?;
    let command = ProjectContextCommand::from_json(&event.content)
        .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    command
        .validate_for_project(*expected_project.as_uuid())
        .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    require_canonical_value(&raw, &command, "command")?;
    Ok(command)
}

/// Verified current binding event and closed content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedProjectContextBinding {
    /// Signed event identifier.
    pub event_id: EventId,
    /// Verified Relay signer.
    pub signer: PublicKey,
    /// Strict active or deleted binding projection.
    pub projection: ProjectContextBindingProjection,
}

/// Verified catalog metadata event and closed content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedProjectContextMeta {
    /// Signed event identifier.
    pub event_id: EventId,
    /// Verified Relay signer.
    pub signer: PublicKey,
    /// Strict reset or incremental catalog observation.
    pub projection: ProjectContextMetaProjection,
}

/// Derive the canonical Document binding `d` coordinate.
#[must_use]
pub fn project_context_binding_coordinate(
    project_id: CommunityId,
    context_document_id: Uuid,
) -> String {
    domain_binding_coordinate(*project_id.as_uuid(), context_document_id)
}

/// Derive the canonical exact-edge `g` query coordinate.
#[must_use]
pub fn project_context_edge_coordinate(project_id: CommunityId, edge_key: EdgeKey) -> String {
    domain_edge_coordinate(*project_id.as_uuid(), edge_key)
}

/// Derive the canonical catalog metadata `d` coordinate.
#[must_use]
pub fn project_context_meta_coordinate(project_id: CommunityId) -> String {
    domain_meta_coordinate(*project_id.as_uuid())
}

/// Build the unsigned binding projection for one transition plan.
pub fn build_project_context_binding_projection(
    plan: &ProjectContextProjectionPlan,
) -> Result<EventBuilder, SdkError> {
    let projection = binding_projection_from_plan(plan)?;
    build_project_context_binding_reprojection(&projection)
}

/// Build an unsigned binding from explicitly reconstructed canonical state.
///
/// This entry point is reserved for generation reprojection. Ordinary writes
/// should use [`build_project_context_binding_projection`].
pub fn build_project_context_binding_reprojection(
    projection: &ProjectContextBindingProjection,
) -> Result<EventBuilder, SdkError> {
    projection
        .validate()
        .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    let binding_coordinate =
        domain_binding_coordinate(projection.project_id, projection.context_document_id);
    let edge_coordinate = domain_edge_coordinate(projection.project_id, projection.edge_key);
    let source = projection.source_event_id.to_hex();
    let generation = canonical_decimal(projection.projection_generation);
    let revision = canonical_decimal(projection.context_revision);
    let mut tags = vec![
        tag(["-"])?,
        tag(["d", binding_coordinate.as_str()])?,
        tag(["t", PROJECT_CONTEXT_PROJECTION_TAG])?,
        tag(["t", BINDING_TAG])?,
        tag(["s", projection.state.as_str()])?,
        tag(["g", edge_coordinate.as_str()])?,
    ];
    for coordinate in &projection.coordinates {
        let value = coordinate.tag_value(projection.project_id);
        tags.push(tag(["c", value.as_str()])?);
    }
    tags.extend([
        tag(["projection_generation", generation.as_str()])?,
        tag(["context_revision", revision.as_str()])?,
        tag(["e", source.as_str(), "", "source"])?,
    ]);
    Ok(EventBuilder::new(
        Kind::Custom(KIND_PROJECT_CONTEXT_EDGE_BINDING as u16),
        canonical_json(projection, "serialize Project Context binding projection")?,
    )
    .tags(tags)
    .custom_created_at(timestamp(projection.updated_at)?))
}

/// Bind one signed binding event into an incremental metadata entry.
pub fn changed_project_context_binding_for(
    plan: &ProjectContextProjectionPlan,
    binding_event: &Event,
) -> Result<ChangedContextBinding, SdkError> {
    let project_id = plan.catalog().project_id();
    let verified = parse_project_context_binding(binding_event, &binding_event.pubkey, project_id)?;
    if verified.projection != binding_projection_from_plan(plan)? {
        return Err(SdkError::InvalidInput(
            "signed binding does not match the projection plan".to_owned(),
        ));
    }
    let binding = plan.binding().ok_or_else(|| {
        SdkError::InvalidInput("reset metadata has no changed binding".to_owned())
    })?;
    Ok(ChangedContextBinding {
        context_document_id: binding.context_document_id,
        edge_key: binding.edge_key,
        binding_coordinate: domain_binding_coordinate(
            *project_id.as_uuid(),
            binding.context_document_id,
        ),
        binding_event_id: binding_event.id,
        state: binding.state,
    })
}

/// Build unsigned reset/bootstrap or incremental catalog metadata.
pub fn build_project_context_meta_projection(
    plan: &ProjectContextProjectionPlan,
    changed_bindings: &[ChangedContextBinding],
) -> Result<EventBuilder, SdkError> {
    if plan.reset() {
        if !changed_bindings.is_empty()
            || plan.source_event_id().is_some()
            || plan.binding().is_some()
        {
            return Err(SdkError::InvalidInput(
                "reset metadata must have no changed binding, source, or transition binding"
                    .to_owned(),
            ));
        }
    } else {
        let binding = plan.binding().ok_or_else(|| {
            SdkError::InvalidInput("ordinary metadata has no changed binding".to_owned())
        })?;
        if changed_bindings.len() != 1
            || changed_bindings[0].context_document_id != binding.context_document_id
            || changed_bindings[0].edge_key != binding.edge_key
            || changed_bindings[0].state != binding.state
            || changed_bindings[0].binding_coordinate
                != domain_binding_coordinate(
                    *plan.catalog().project_id().as_uuid(),
                    binding.context_document_id,
                )
        {
            return Err(SdkError::InvalidInput(
                "incremental metadata changed binding does not match the plan".to_owned(),
            ));
        }
    }
    let catalog = plan.catalog();
    let projection = ProjectContextMetaProjection {
        schema_version: PROJECT_CONTEXT_SCHEMA_VERSION,
        projection_type: ProjectContextProjectionType::ContextMeta,
        project_id: *catalog.project_id().as_uuid(),
        projection_generation: catalog.projection_generation(),
        context_revision: catalog.context_revision(),
        active_edge_count: catalog.active_edge_count(),
        bound_document_count: catalog.bound_document_count(),
        reset: plan.reset(),
        changed_bindings: changed_bindings.to_vec(),
        source_event_id: plan.source_event_id(),
        updated_at: catalog.updated_at(),
    };
    projection
        .validate()
        .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    let coordinate = domain_meta_coordinate(*catalog.project_id().as_uuid());
    let generation = canonical_decimal(catalog.projection_generation());
    let revision = canonical_decimal(catalog.context_revision());
    let mut tags = vec![
        tag(["-"])?,
        tag(["d", coordinate.as_str()])?,
        tag(["t", PROJECT_CONTEXT_PROJECTION_TAG])?,
        tag(["t", META_TAG])?,
        tag(["projection_generation", generation.as_str()])?,
        tag(["context_revision", revision.as_str()])?,
    ];
    if let Some(source) = plan.source_event_id() {
        tags.push(tag(["e", &source.to_hex(), "", "source"])?);
    }
    Ok(EventBuilder::new(
        Kind::Custom(KIND_PROJECT_CONTEXT_META as u16),
        canonical_json(&projection, "serialize Project Context metadata projection")?,
    )
    .tags(tags)
    .custom_created_at(timestamp(catalog.updated_at())?))
}

/// Parse and verify one current binding for an expected Community and Relay.
pub fn parse_project_context_binding(
    event: &Event,
    expected_relay: &PublicKey,
    expected_project: CommunityId,
) -> Result<VerifiedProjectContextBinding, SdkError> {
    verify_projection_envelope(event, expected_relay, KIND_PROJECT_CONTEXT_EDGE_BINDING)?;
    let (raw, projection) =
        parse_projection_content::<ProjectContextBindingProjection>(event, "binding")?;
    projection
        .validate()
        .map_err(|error| invalid_projection(error.to_string()))?;
    require_project(projection.project_id, expected_project)?;
    require_event_time(event, projection.updated_at, "binding")?;
    let mut expected_tags = vec![
        vec!["-".to_owned()],
        vec![
            "d".to_owned(),
            domain_binding_coordinate(projection.project_id, projection.context_document_id),
        ],
        vec!["t".to_owned(), PROJECT_CONTEXT_PROJECTION_TAG.to_owned()],
        vec!["t".to_owned(), BINDING_TAG.to_owned()],
        vec!["s".to_owned(), projection.state.as_str().to_owned()],
        vec![
            "g".to_owned(),
            domain_edge_coordinate(projection.project_id, projection.edge_key),
        ],
    ];
    for coordinate in &projection.coordinates {
        expected_tags.push(vec![
            "c".to_owned(),
            coordinate.tag_value(projection.project_id),
        ]);
    }
    expected_tags.extend([
        vec![
            "projection_generation".to_owned(),
            canonical_decimal(projection.projection_generation),
        ],
        vec![
            "context_revision".to_owned(),
            canonical_decimal(projection.context_revision),
        ],
        vec![
            "e".to_owned(),
            projection.source_event_id.to_hex(),
            String::new(),
            "source".to_owned(),
        ],
    ]);
    require_exact_tags(event, &expected_tags, "binding")?;
    require_canonical_value(&raw, &projection, "binding")?;
    Ok(VerifiedProjectContextBinding {
        event_id: event.id,
        signer: event.pubkey,
        projection,
    })
}

/// Parse and verify one Context metadata observation for an expected Community and Relay.
pub fn parse_project_context_meta(
    event: &Event,
    expected_relay: &PublicKey,
    expected_project: CommunityId,
) -> Result<VerifiedProjectContextMeta, SdkError> {
    verify_projection_envelope(event, expected_relay, KIND_PROJECT_CONTEXT_META)?;
    let (raw, projection) =
        parse_projection_content::<ProjectContextMetaProjection>(event, "metadata")?;
    projection
        .validate()
        .map_err(|error| invalid_projection(error.to_string()))?;
    require_project(projection.project_id, expected_project)?;
    require_event_time(event, projection.updated_at, "metadata")?;
    let mut expected_tags = vec![
        vec!["-".to_owned()],
        vec![
            "d".to_owned(),
            domain_meta_coordinate(projection.project_id),
        ],
        vec!["t".to_owned(), PROJECT_CONTEXT_PROJECTION_TAG.to_owned()],
        vec!["t".to_owned(), META_TAG.to_owned()],
        vec![
            "projection_generation".to_owned(),
            canonical_decimal(projection.projection_generation),
        ],
        vec![
            "context_revision".to_owned(),
            canonical_decimal(projection.context_revision),
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
    Ok(VerifiedProjectContextMeta {
        event_id: event.id,
        signer: event.pubkey,
        projection,
    })
}

/// Verify that one incremental metadata entry binds one verified binding head.
pub fn verify_project_context_meta_change(
    meta: &VerifiedProjectContextMeta,
    binding: &VerifiedProjectContextBinding,
) -> Result<(), SdkError> {
    let projection = &binding.projection;
    let expected = ChangedContextBinding {
        context_document_id: projection.context_document_id,
        edge_key: projection.edge_key,
        binding_coordinate: domain_binding_coordinate(
            projection.project_id,
            projection.context_document_id,
        ),
        binding_event_id: binding.event_id,
        state: projection.state,
    };
    if meta.signer != binding.signer
        || meta.projection.project_id != projection.project_id
        || meta.projection.projection_generation != projection.projection_generation
        || meta.projection.context_revision != projection.context_revision
        || meta.projection.source_event_id != Some(projection.source_event_id)
        || meta.projection.changed_bindings.as_slice() != [expected]
    {
        return Err(invalid_projection(
            "metadata changed binding does not bind the supplied projection",
        ));
    }
    Ok(())
}

/// Verify that one binding is visible within a metadata observation boundary.
///
/// Binding revisions may precede the current metadata revision. When an
/// incremental metadata event has the same revision, its changed-binding
/// pointer must bind the exact event. Reset metadata instead establishes a
/// complete generation boundary and intentionally carries no per-binding
/// pointer, including when a binding has the same revision.
pub fn verify_project_context_binding_observation(
    meta: &VerifiedProjectContextMeta,
    binding: &VerifiedProjectContextBinding,
) -> Result<(), SdkError> {
    if meta.signer != binding.signer
        || meta.projection.project_id != binding.projection.project_id
        || meta.projection.projection_generation != binding.projection.projection_generation
        || binding.projection.context_revision > meta.projection.context_revision
    {
        return Err(invalid_projection(
            "binding is outside the supplied metadata observation boundary",
        ));
    }
    if !meta.projection.reset
        && binding.projection.context_revision == meta.projection.context_revision
    {
        verify_project_context_meta_change(meta, binding)?;
    }
    Ok(())
}

/// Strictly verify a complete Relay-signed ordinary projection bundle.
pub fn verify_project_context_projection_bundle(
    plan: &ProjectContextProjectionPlan,
    binding_event: &Event,
    meta_event: &Event,
    expected_relay: &PublicKey,
) -> Result<VerifiedProjectContextBinding, SdkError> {
    if plan.reset() || plan.binding().is_none() || plan.source_event_id().is_none() {
        return Err(SdkError::InvalidInput(
            "an ordinary projection bundle requires a non-reset transition plan".to_owned(),
        ));
    }
    let project_id = plan.catalog().project_id();
    let binding = parse_project_context_binding(binding_event, expected_relay, project_id)?;
    if binding.projection != binding_projection_from_plan(plan)? {
        return Err(invalid_projection(
            "binding projection does not match the deterministic transition plan",
        ));
    }
    let changed = changed_project_context_binding_for(plan, binding_event)?;
    let meta = parse_project_context_meta(meta_event, expected_relay, project_id)?;
    verify_project_context_meta_change(&meta, &binding)?;
    let expected_meta = ProjectContextMetaProjection {
        schema_version: PROJECT_CONTEXT_SCHEMA_VERSION,
        projection_type: ProjectContextProjectionType::ContextMeta,
        project_id: *project_id.as_uuid(),
        projection_generation: plan.catalog().projection_generation(),
        context_revision: plan.catalog().context_revision(),
        active_edge_count: plan.catalog().active_edge_count(),
        bound_document_count: plan.catalog().bound_document_count(),
        reset: false,
        changed_bindings: vec![changed],
        source_event_id: plan.source_event_id(),
        updated_at: plan.catalog().updated_at(),
    };
    if meta.projection != expected_meta {
        return Err(invalid_projection(
            "metadata projection does not match the deterministic transition plan",
        ));
    }
    Ok(binding)
}

/// Reject a signed event whose complete Nostr `EVENT` frame exceeds a Relay limit.
///
/// Call this for every derived projection after signing and before opening the
/// canonical commit transaction. Content-only limits do not account for the
/// repeated coordinate tags, signature, or frame envelope.
pub fn validate_signed_event_frame_size(
    event: &Event,
    max_frame_bytes: usize,
) -> Result<(), SdkError> {
    let frame = nostr::ClientMessage::event(event.clone()).as_json();
    if frame.len() > max_frame_bytes {
        return Err(SdkError::ContentTooLarge {
            max: max_frame_bytes,
            got: frame.len(),
        });
    }
    Ok(())
}

fn binding_projection_from_plan(
    plan: &ProjectContextProjectionPlan,
) -> Result<ProjectContextBindingProjection, SdkError> {
    let binding = plan
        .binding()
        .ok_or_else(|| SdkError::InvalidInput("reset plan has no binding projection".to_owned()))?;
    let source_event_id = plan.source_event_id().ok_or_else(|| {
        SdkError::InvalidInput("ordinary projection has no source event".to_owned())
    })?;
    let catalog = plan.catalog();
    let projection = ProjectContextBindingProjection {
        schema_version: PROJECT_CONTEXT_SCHEMA_VERSION,
        projection_type: ProjectContextProjectionType::ContextEdgeBinding,
        project_id: *catalog.project_id().as_uuid(),
        projection_generation: catalog.projection_generation(),
        context_revision: catalog.context_revision(),
        edge_key: binding.edge_key,
        coordinates: binding.coordinates.clone(),
        context_document_id: binding.context_document_id,
        state: binding.state,
        source_event_id,
        updated_at: binding.updated_at,
    };
    projection
        .validate()
        .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    Ok(projection)
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
    if event.content.len() > MAX_PROJECTION_CONTENT_BYTES {
        return Err(SdkError::ContentTooLarge {
            max: MAX_PROJECTION_CONTENT_BYTES,
            got: event.content.len(),
        });
    }
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
    let seconds = u64::try_from(value.timestamp()).map_err(|_| {
        SdkError::InvalidInput("Project Context time precedes the Unix epoch".to_owned())
    })?;
    Ok(Timestamp::from(seconds))
}

fn canonical_decimal(value: u64) -> String {
    debug_assert!(value <= MAX_SAFE_REVISION);
    value.to_string()
}

fn invalid_projection(message: impl Into<String>) -> SdkError {
    SdkError::InvalidProjection(message.into())
}
