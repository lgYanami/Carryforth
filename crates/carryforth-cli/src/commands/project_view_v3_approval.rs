//! Human-only detached approval for reviewed legacy Resource mappings.

use std::io::Write as _;
use std::path::Path;

use buzz_project_view::v3::{
    guide_snapshot_digest, legacy_resource_digest, mapping_entry_digest,
    resource_cutover_payload_digest, review_digest, CanonicalGuideSnapshotV1,
    CanonicalLegacyObjectStateV1, CanonicalLegacyResourceV1, CanonicalProjectResourceEnvelopeV1,
    CanonicalProjectResourceV3, CanonicalResourceCutoverEnvelopeV1, CanonicalResourceCutoverV1,
    CanonicalResourceMappingEntryV1, CanonicalResourceReviewV1, ResourceMappingManifestEnvelopeV1,
    ResourceMappingManifestV1, ReviewSignature, ReviewedResourceMappingV1, MAX_MANIFEST_JSON_BYTES,
};
use buzz_project_view::{ProjectViewEntry, ProjectViewObjectData, ProjectViewObjectType};
use chrono::Utc;
use nostr::secp256k1::Message;
use serde::Deserialize;
use uuid::Uuid;

use crate::client::CarryforthClient;
use crate::commands::documents::read_verified_document;
use crate::commands::project_view_snapshot::{
    is_managed_runtime, read_legacy_v2_identity, read_legacy_v2_migration_snapshot,
};
use crate::error::CliError;
use crate::validate::read_bounded_file_or_stdin;
use crate::ProjectViewV3ResourcesClientCmd;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceMappingDraft {
    schema_version: u16,
    community_id: Uuid,
    base_meta_event_id: String,
    base_project_revision: u64,
    base_projection_generation: u64,
    entries: Vec<ResourceMappingDraftEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceMappingDraftEntry {
    resource_id: Uuid,
    legacy_object_revision: u64,
    legacy_projection_event_id: String,
    legacy_body_digest: String,
    legacy_resource: buzz_project_view::ProjectResource,
    suggested_resource_kind: String,
    suggested_guide_markdown: String,
    guide_document_id: Uuid,
    reviewed_v3_payload: Option<CanonicalResourceCutoverEnvelopeV1>,
    guide_document_revision: Option<u64>,
    guide_head_event_id: Option<String>,
    guide_revision_event_id: Option<String>,
    review_status: String,
}

/// Run one local v3 approval command. No Relay mutation is submitted.
pub(crate) async fn dispatch(
    command: ProjectViewV3ResourcesClientCmd,
    client: &CarryforthClient,
) -> Result<(), CliError> {
    match command {
        ProjectViewV3ResourcesClientCmd::Approve { manifest, out } => {
            approve(client, &manifest, &out).await
        }
    }
}

async fn approve(client: &CarryforthClient, draft_path: &str, out: &str) -> Result<(), CliError> {
    if is_managed_runtime() {
        return Err(CliError::Auth(
            "Resource migration approval requires a direct Human member key".to_owned(),
        ));
    }
    let bytes = read_bounded_file_or_stdin(draft_path, MAX_MANIFEST_JSON_BYTES)?;
    let draft: ResourceMappingDraft = serde_json::from_str(&bytes)
        .map_err(|error| CliError::Usage(format!("invalid Resource review draft: {error}")))?;
    if draft.schema_version != 1 {
        return Err(CliError::Usage(
            "Resource review draft schema_version must be one".to_owned(),
        ));
    }
    let identity = read_legacy_v2_identity(client).await?;
    let snapshot = read_legacy_v2_migration_snapshot(client, identity).await?;
    let expected_meta = lower_hex::<32>(&draft.base_meta_event_id, "base_meta_event_id")?;
    if snapshot.meta().project_id.as_uuid() != &draft.community_id
        || snapshot.meta().event_id.to_bytes() != expected_meta
        || snapshot.meta().project_revision != draft.base_project_revision
        || snapshot.meta().projection_generation != draft.base_projection_generation
    {
        return Err(CliError::Conflict(
            "Resource review draft no longer matches the current v2 base".to_owned(),
        ));
    }
    let reviewer = client.public_key();
    if !snapshot
        .membership()
        .members
        .iter()
        .any(|member| member.pubkey == reviewer)
    {
        return Err(CliError::Auth(
            "Resource reviewer is not present in the exact membership snapshot".to_owned(),
        ));
    }

    let mut entries = Vec::with_capacity(draft.entries.len());
    for draft_entry in draft.entries {
        // Suggestions and legacy values remain review material only. Reading
        // them must never execute or interpolate their contents.
        let _review_material = (
            &draft_entry.suggested_resource_kind,
            &draft_entry.suggested_guide_markdown,
            &draft_entry.review_status,
        );
        let entry = snapshot.entry(draft_entry.resource_id).ok_or_else(|| {
            CliError::Conflict(format!(
                "legacy Resource {} is no longer present",
                draft_entry.resource_id
            ))
        })?;
        let ProjectViewEntry::Active(object) = entry else {
            return Err(CliError::Conflict(format!(
                "legacy Resource {} is deleted",
                draft_entry.resource_id
            )));
        };
        let ProjectViewObjectData::Resource(resource) = &object.data else {
            return Err(CliError::Usage(format!(
                "draft object {} is not a Resource",
                draft_entry.resource_id
            )));
        };
        if object.object_type != ProjectViewObjectType::Resource
            || object.object_revision != draft_entry.legacy_object_revision
            || resource != &draft_entry.legacy_resource
        {
            return Err(CliError::Conflict(format!(
                "legacy Resource {} changed after export",
                draft_entry.resource_id
            )));
        }
        let source = snapshot.object_source(object.id).ok_or_else(|| {
            CliError::Other(format!(
                "Project View v2 integrity error: Resource {} has no signed source",
                object.id
            ))
        })?;
        let legacy_projection_event_id = lower_hex::<32>(
            &draft_entry.legacy_projection_event_id,
            "legacy_projection_event_id",
        )?;
        let legacy = CanonicalLegacyResourceV1 {
            schema_version: 2,
            resource_id: *object.id.as_bytes(),
            object_revision: object.object_revision,
            project_revision: object.project_revision,
            state: CanonicalLegacyObjectStateV1::Active,
            resource_data: Some(resource.clone()),
            relations: object.relations,
        };
        let legacy_body_digest =
            legacy_resource_digest(&legacy).map_err(|error| CliError::Usage(error.to_string()))?;
        if source.event_id.to_bytes() != legacy_projection_event_id
            || lower_hex::<32>(&draft_entry.legacy_body_digest, "legacy_body_digest")?
                != legacy_body_digest
        {
            return Err(CliError::Conflict(format!(
                "legacy Resource {} projection or digest changed after export",
                object.id
            )));
        }

        let reviewed = draft_entry.reviewed_v3_payload.ok_or_else(|| {
            CliError::Usage(format!(
                "Resource {} is missing reviewed_v3_payload",
                object.id
            ))
        })?;
        if !reviewed.context_references.is_empty()
            || reviewed.resource_data.guide_document_id != draft_entry.guide_document_id
        {
            return Err(CliError::Usage(format!(
                "Resource {} must use its preallocated Guide and empty Context",
                object.id
            )));
        }
        let guide = read_verified_document(client, draft_entry.guide_document_id, None).await?;
        let guide_head = guide.head_event_id.ok_or_else(|| {
            CliError::Other(format!(
                "Project Document integrity error: Guide {} has no current head",
                draft_entry.guide_document_id
            ))
        })?;
        require_draft_pin(
            draft_entry.guide_document_revision,
            guide.document_revision,
            "guide_document_revision",
            object.id,
        )?;
        require_draft_hex_pin(
            draft_entry.guide_head_event_id.as_deref(),
            guide_head.to_bytes(),
            "guide_head_event_id",
            object.id,
        )?;
        require_draft_hex_pin(
            draft_entry.guide_revision_event_id.as_deref(),
            guide.revision_event_id.to_bytes(),
            "guide_revision_event_id",
            object.id,
        )?;
        let guide_snapshot = CanonicalGuideSnapshotV1 {
            document_id: *guide.document_id.as_bytes(),
            document_revision: guide.document_revision,
            title: guide.title.clone().ok_or_else(|| {
                CliError::Other("active Guide is missing its verified title".to_owned())
            })?,
            summary: guide.summary.clone(),
            content_markdown: guide.content_markdown.clone().ok_or_else(|| {
                CliError::Other("active Guide is missing verified Markdown".to_owned())
            })?,
        };
        let guide_content_digest = guide_snapshot_digest(&guide_snapshot)
            .map_err(|error| CliError::Usage(error.to_string()))?;
        let payload = canonical_payload(reviewed.resource_data);
        let v3_payload_digest = resource_cutover_payload_digest(&payload)
            .map_err(|error| CliError::Usage(error.to_string()))?;
        let mapping = CanonicalResourceMappingEntryV1 {
            community_id: *draft.community_id.as_bytes(),
            base_meta_event_id: expected_meta,
            base_project_revision: draft.base_project_revision,
            base_projection_generation: draft.base_projection_generation,
            resource_id: *object.id.as_bytes(),
            legacy_object_revision: object.object_revision,
            legacy_projection_event_id,
            legacy_body_digest,
            v3_payload_digest,
            guide_document_id: *guide.document_id.as_bytes(),
            guide_document_revision: guide.document_revision,
            guide_head_event_id: guide_head.to_bytes(),
            guide_revision_event_id: guide.revision_event_id.to_bytes(),
            guide_content_digest,
        };
        let mapping_entry_digest =
            mapping_entry_digest(&mapping).map_err(|error| CliError::Usage(error.to_string()))?;
        let reviewed_at_unix_micros = Utc::now().timestamp_micros();
        let review = CanonicalResourceReviewV1 {
            mapping_entry_digest,
            reviewed_by_pubkey: *reviewer.as_bytes(),
            reviewed_at_unix_micros,
        };
        let review_digest =
            review_digest(&review).map_err(|error| CliError::Usage(error.to_string()))?;
        let message = Message::from_digest(review_digest);
        let signature = client.keys().sign_schnorr(&message);
        let signature_bytes = *signature.as_ref();
        entries.push(ReviewedResourceMappingV1 {
            resource_id: *object.id.as_bytes(),
            legacy_object_revision: object.object_revision,
            legacy_projection_event_id,
            legacy_body_digest,
            reviewed_v3_payload: payload,
            v3_payload_digest,
            guide_document_revision: guide.document_revision,
            guide_head_event_id: guide_head.to_bytes(),
            guide_revision_event_id: guide.revision_event_id.to_bytes(),
            guide_content_digest,
            mapping_entry_digest,
            reviewed_by_pubkey: *reviewer.as_bytes(),
            reviewed_at_unix_micros,
            review_digest,
            review_signature: ReviewSignature::from_bytes(signature_bytes),
        });
    }
    entries.sort_by_key(|entry| entry.resource_id);
    let manifest = ResourceMappingManifestV1 {
        schema_version: 1,
        community_id: *draft.community_id.as_bytes(),
        base_meta_event_id: expected_meta,
        base_project_revision: draft.base_project_revision,
        base_projection_generation: draft.base_projection_generation,
        entries,
    };
    manifest
        .validate()
        .map_err(|error| CliError::Usage(error.to_string()))?;
    let output = ResourceMappingManifestEnvelopeV1::to_pretty_json(&manifest)
        .map_err(|error| CliError::Usage(error.to_string()))?;
    write_owner_only_new(Path::new(out), &output)?;
    println!("{out}");
    Ok(())
}

fn canonical_payload(resource: CanonicalProjectResourceEnvelopeV1) -> CanonicalResourceCutoverV1 {
    CanonicalResourceCutoverV1 {
        resource_data: CanonicalProjectResourceV3 {
            name: resource.name,
            resource_kind: resource.resource_kind,
            summary: resource.summary,
            guide_document_id: *resource.guide_document_id.as_bytes(),
        },
        context_references: Vec::new(),
    }
}

fn require_draft_pin(
    supplied: Option<u64>,
    actual: u64,
    field: &str,
    resource_id: Uuid,
) -> Result<(), CliError> {
    if supplied != Some(actual) {
        return Err(CliError::Conflict(format!(
            "Resource {resource_id} {field} does not match the current Guide"
        )));
    }
    Ok(())
}

fn require_draft_hex_pin(
    supplied: Option<&str>,
    actual: [u8; 32],
    field: &str,
    resource_id: Uuid,
) -> Result<(), CliError> {
    let supplied = supplied
        .ok_or_else(|| CliError::Usage(format!("Resource {resource_id} is missing {field}")))?;
    if lower_hex::<32>(supplied, field)? != actual {
        return Err(CliError::Conflict(format!(
            "Resource {resource_id} {field} does not match the current Guide"
        )));
    }
    Ok(())
}

fn lower_hex<const N: usize>(value: &str, field: &str) -> Result<[u8; N], CliError> {
    if value.len() != N * 2
        || value
            .bytes()
            .any(|byte| !matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(CliError::Usage(format!(
            "{field} must contain {} lowercase hex characters",
            N * 2
        )));
    }
    hex::decode(value)
        .map_err(|error| CliError::Usage(format!("invalid {field}: {error}")))?
        .try_into()
        .map_err(|bytes: Vec<u8>| {
            CliError::Usage(format!("{field} decoded to {} bytes", bytes.len()))
        })
}

fn write_owner_only_new(path: &Path, bytes: &[u8]) -> Result<(), CliError> {
    if path.as_os_str().is_empty() {
        return Err(CliError::Usage("--out cannot be empty".to_owned()));
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|error| {
        CliError::Other(format!(
            "create owner-only reviewed manifest {}: {error}",
            path.display()
        ))
    })?;
    file.write_all(bytes).map_err(|error| {
        CliError::Other(format!(
            "write reviewed manifest {}: {error}",
            path.display()
        ))
    })
}
