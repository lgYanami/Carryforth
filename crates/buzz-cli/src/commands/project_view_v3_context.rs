//! Optional, body-free Project Document enrichment for CLI Role Brief v3.

use std::collections::{BTreeSet, HashSet};
use std::time::Duration;

use buzz_core::kind::{KIND_PROJECT_DOCUMENT_HEAD, KIND_PROJECT_DOCUMENT_META};
use buzz_core::CommunityId;
use buzz_project_document::DocumentHeadProjection;
use buzz_sdk::project_document::{
    document_head_coordinate, document_meta_coordinate, parse_document_head, parse_document_meta,
    VerifiedDocumentHead, VerifiedDocumentMeta,
};
use buzz_sdk::role_brief_v3::{
    RoleBriefDocumentEnrichmentV3, RoleBriefV3, VerifiedDocumentMetadataV3,
    VerifiedRoleBriefSnapshotV3,
};
use chrono::{DateTime, Utc};
use nostr::Event;
use serde_json::json;
use uuid::Uuid;

use crate::client::BuzzClient;
use crate::commands::project_view_v2_snapshot::{
    v3_integrity_error, ProjectViewIdentity, ProjectViewSchema,
};
use crate::error::CliError;

const DOCUMENT_SNAPSHOT_ATTEMPTS: usize = 3;
const DOCUMENT_ENRICHMENT_TIMEOUT: Duration = Duration::from_secs(4);

/// Resolve the Context-capability-aware v3 Brief used by the CLI surface.
///
/// Project View authority failures remain fatal. Document metadata is an
/// independent optional enrichment: timeout, absence, or invalid metadata
/// produces an explicit `unavailable` Context without weakening Assignment
/// authority or reusing stale values.
pub(crate) async fn resolve_v3_role_brief(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
    snapshot: &VerifiedRoleBriefSnapshotV3,
    member: nostr::PublicKey,
    generated_at: DateTime<Utc>,
) -> Result<RoleBriefV3, CliError> {
    if identity.schema != ProjectViewSchema::V3 {
        return Err(v3_integrity_error(
            "Context enrichment requires Project View v3",
        ));
    }
    if !identity.context_enabled {
        return snapshot
            .brief_for(member, generated_at)
            .map_err(|error| v3_integrity_error(error.to_string()));
    }

    let required = snapshot
        .required_live_document_ids_for(member)
        .map_err(|error| v3_integrity_error(error.to_string()))?;
    if required.is_empty() {
        return snapshot
            .brief_for_with_context(
                member,
                generated_at,
                RoleBriefDocumentEnrichmentV3::NotRequired,
            )
            .map_err(|error| v3_integrity_error(error.to_string()));
    }

    let metadata = if identity.document_enabled {
        tokio::time::timeout(
            DOCUMENT_ENRICHMENT_TIMEOUT,
            read_stable_document_metadata(client, identity, snapshot, &required),
        )
        .await
        .ok()
        .and_then(Result::ok)
    } else {
        None
    };

    if let Some(metadata) = &metadata {
        if let Ok(brief) = snapshot.brief_for_with_context(
            member,
            generated_at,
            RoleBriefDocumentEnrichmentV3::Verified(metadata),
        ) {
            return Ok(brief);
        }
    }
    snapshot
        .brief_for_with_context(
            member,
            generated_at,
            RoleBriefDocumentEnrichmentV3::Unavailable,
        )
        .map_err(|error| v3_integrity_error(error.to_string()))
}

async fn read_stable_document_metadata(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
    snapshot: &VerifiedRoleBriefSnapshotV3,
    required: &BTreeSet<Uuid>,
) -> Result<VerifiedDocumentMetadataV3, CliError> {
    let project_id = snapshot.meta().project_id;
    let mut before = read_document_meta(client, identity, project_id).await?;
    for attempt in 0..DOCUMENT_SNAPSHOT_ATTEMPTS {
        let heads = read_document_heads(client, identity, project_id, required).await?;
        let after = read_document_meta(client, identity, project_id).await?;
        if document_meta_boundary_matches(&before, &after) {
            return VerifiedDocumentMetadataV3::new(before, heads)
                .map_err(|error| v3_integrity_error(error.to_string()));
        }
        if attempt + 1 < DOCUMENT_SNAPSHOT_ATTEMPTS {
            before = after;
            tokio::time::sleep(Duration::from_millis(25_u64 << attempt)).await;
            continue;
        }
        return Err(v3_integrity_error(
            "Project Document metadata changed during every bounded snapshot attempt",
        ));
    }
    Err(v3_integrity_error(
        "Project Document metadata snapshot could not be stabilized",
    ))
}

async fn read_document_meta(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
    project_id: CommunityId,
) -> Result<VerifiedDocumentMeta, CliError> {
    let filter = json!({
        "kinds": [KIND_PROJECT_DOCUMENT_META],
        "authors": [identity.relay_pubkey.to_hex()],
        "#d": [document_meta_coordinate(project_id)],
        "limit": 2,
    });
    let events = parse_events(&client.query(&filter).await?, "Document metadata")?;
    let [event] = events.as_slice() else {
        return Err(v3_integrity_error(
            "Document metadata query did not return exactly one current head",
        ));
    };
    let meta = parse_document_meta(event, &identity.relay_pubkey)
        .map_err(|error| v3_integrity_error(error.to_string()))?;
    if meta.projection.project_id != *project_id.as_uuid() {
        return Err(v3_integrity_error(
            "Document metadata belongs to a different Project",
        ));
    }
    Ok(meta)
}

async fn read_document_heads(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
    project_id: CommunityId,
    required: &BTreeSet<Uuid>,
) -> Result<Vec<VerifiedDocumentHead>, CliError> {
    if required.len() > 128 {
        return Err(v3_integrity_error(
            "Role Brief requested too many Document metadata heads",
        ));
    }
    let coordinates = required
        .iter()
        .map(|document_id| document_head_coordinate(project_id, *document_id))
        .collect::<Vec<_>>();
    let filter = json!({
        "kinds": [KIND_PROJECT_DOCUMENT_HEAD],
        "authors": [identity.relay_pubkey.to_hex()],
        "#d": coordinates,
        "limit": required.len(),
    });
    let events = parse_events(&client.query(&filter).await?, "Document head")?;
    if events.len() != required.len() {
        return Err(v3_integrity_error(
            "Document head query did not resolve every required coordinate",
        ));
    }

    let mut missing = required.clone();
    let mut event_ids = HashSet::with_capacity(events.len());
    let mut heads = Vec::with_capacity(events.len());
    for event in events {
        if !event_ids.insert(event.id) {
            return Err(v3_integrity_error(
                "Document head query returned a duplicate event",
            ));
        }
        let head = parse_document_head(&event, &identity.relay_pubkey, project_id)
            .map_err(|error| v3_integrity_error(error.to_string()))?;
        let document_id = document_head_id(&head);
        if !missing.remove(&document_id) {
            return Err(v3_integrity_error(
                "Document head query returned an unexpected or duplicate coordinate",
            ));
        }
        heads.push(head);
    }
    if !missing.is_empty() {
        return Err(v3_integrity_error(
            "Document head query omitted a required coordinate",
        ));
    }
    Ok(heads)
}

fn parse_events(raw: &str, label: &str) -> Result<Vec<Event>, CliError> {
    serde_json::from_str(raw)
        .map_err(|error| v3_integrity_error(format!("invalid {label} response: {error}")))
}

fn document_head_id(head: &VerifiedDocumentHead) -> Uuid {
    match &head.projection {
        DocumentHeadProjection::Active { document_id, .. }
        | DocumentHeadProjection::Deleted { document_id, .. } => *document_id,
    }
}

fn document_meta_boundary_matches(
    before: &VerifiedDocumentMeta,
    after: &VerifiedDocumentMeta,
) -> bool {
    before.event_id == after.event_id
        && before.signer == after.signer
        && before.projection.project_id == after.projection.project_id
        && before.projection.projection_generation == after.projection.projection_generation
        && before.projection.catalog_revision == after.projection.catalog_revision
}
