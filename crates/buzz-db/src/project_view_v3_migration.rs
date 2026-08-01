//! Reviewed legacy-Resource export, validation, and Project View v3 cutover.
//!
//! The JSON files handled by operator tooling are transport envelopes only.
//! Every decision is reconstructed from the frozen postcard contract, checked
//! against canonical database rows under the Community lock, and copied into
//! immutable ledgers before a schema-major cutover can commit.

use std::collections::{BTreeMap, BTreeSet};

use buzz_audit::{AuditAction, NewAuditEntry};
use buzz_core::kind::{
    KIND_NIP43_MEMBERSHIP_LIST, KIND_PROJECT_DOCUMENT_HEAD, KIND_PROJECT_DOCUMENT_META,
    KIND_PROJECT_DOCUMENT_REVISION, KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_MUTATION,
    KIND_PROJECT_VIEW_OBJECT,
};
use buzz_core::{CommunityId, EventId, PublicKey};
use buzz_project_view::v2::{ChangeSource, RoleContinuityEntity, RoleLevel};
use buzz_project_view::v3::{
    guide_snapshot_digest, legacy_resource_digest, manifest_digest, CanonicalGuideSnapshotV1,
    CanonicalLegacyObjectStateV1, CanonicalLegacyResourceV1, CanonicalResourceCutoverEnvelopeV1,
    MigrationContractError, ProjectResourceV3, ProjectViewEntryV3, ProjectViewObjectDataV3,
    ProjectViewObjectV3, ProjectViewStateV3, ProjectViewTombstoneV3, ResourceMappingManifestV1,
    ReviewedResourceMappingV1, MAX_MANIFEST_ENTRIES,
};
use buzz_project_view::{
    LocatorType, Mutation, MutationRequest, ObjectRef, ProjectResource, ProjectViewEntry,
    ProjectViewObjectData, ProjectViewObjectType, ProjectViewRelations, ResourceType,
    MAX_SAFE_REVISION,
};
use chrono::{DateTime, Utc};
use nostr::secp256k1::{schnorr::Signature, Message, SECP256K1};
use nostr::Keys;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{Db, DbError};
use buzz_sdk::project_view_v3::{
    V3EntityChange, V3EntityCounts, V3ProjectionContext, V3ProjectionSource,
};

const CUTOVER_IDEMPOTENCY_DOMAIN: &[u8] = b"buzz-pv3-cutover-idempotency-v1\0";
const CUTOVER_REQUEST_DOMAIN: &[u8] = b"buzz-pv3-cutover-request-v1\0";

/// Stable failures from reviewed Resource migration tooling.
#[derive(Debug, thiserror::Error)]
pub enum ProjectViewV3MigrationError {
    /// Database abstraction failure.
    #[error(transparent)]
    Database(#[from] DbError),
    /// Direct SQL failure.
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    /// Frozen manifest/digest contract failure.
    #[error(transparent)]
    Contract(#[from] MigrationContractError),
    /// Tamper-evident Community audit append failure.
    #[error(transparent)]
    Audit(#[from] buzz_audit::AuditError),
    /// Input is malformed independently of current state.
    #[error("invalid Project View v3 migration input: {0}")]
    Invalid(String),
    /// A pinned base, mapping, epoch, or idempotency compare-and-set failed.
    #[error("Project View v3 migration conflict: {0}")]
    Conflict(String),
    /// The Community is not in the required migration state.
    #[error("Project View v3 migration unavailable: {0}")]
    Unavailable(String),
    /// The caller or reviewer is not an eligible direct Human member.
    #[error("Project View v3 migration authorization failed: {0}")]
    Forbidden(String),
}

/// Convenient reviewed migration result.
pub type ProjectViewV3MigrationResult<T> = Result<T, ProjectViewV3MigrationError>;

/// Local, owner-only review bundle produced from one exact schema-v2 base.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceMappingDraftV1 {
    /// Draft envelope version.
    pub schema_version: u16,
    /// Community identity.
    pub community_id: Uuid,
    /// Exact schema-v2 metadata projection ID.
    pub base_meta_event_id: String,
    /// Exact schema-v2 Project revision.
    pub base_project_revision: u64,
    /// Exact schema-v2 projection generation.
    pub base_projection_generation: u64,
    /// One entry for every active legacy Resource, in UUID byte order.
    pub entries: Vec<ResourceMappingDraftEntryV1>,
}

/// One editable Resource review draft. Legacy values are inert data and must
/// never be interpolated into a shell command by consumers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceMappingDraftEntryV1 {
    /// Stable Resource identity.
    pub resource_id: Uuid,
    /// Exact legacy object revision.
    pub legacy_object_revision: u64,
    /// Lowercase legacy projection event ID.
    pub legacy_projection_event_id: String,
    /// Lowercase canonical legacy body digest.
    pub legacy_body_digest: String,
    /// Complete legacy body retained for Human review.
    pub legacy_resource: ProjectResource,
    /// Suggested open v3 kind; the reviewer may replace it.
    pub suggested_resource_kind: String,
    /// Deterministic, non-truncating Markdown draft.
    pub suggested_guide_markdown: String,
    /// Stable preallocated Guide identity reused by later exports.
    pub guide_document_id: Uuid,
    /// Final payload to complete before approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewed_v3_payload: Option<CanonicalResourceCutoverEnvelopeV1>,
    /// Exact active Guide revision to complete before approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guide_document_revision: Option<u64>,
    /// Exact Guide current-head event ID to complete before approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guide_head_event_id: Option<String>,
    /// Exact Guide immutable-revision event ID to complete before approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guide_revision_event_id: Option<String>,
    /// Stable staging state observed during export.
    pub review_status: String,
}

/// Successful server-side manifest validation result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResourceMappingValidationReceipt {
    /// Community identity.
    pub community_id: Uuid,
    /// Lowercase canonical manifest digest.
    pub manifest_digest: String,
    /// Exact reviewed Resource count.
    pub entry_count: usize,
    /// Exact pinned Project revision.
    pub base_project_revision: u64,
    /// Exact pinned projection generation.
    pub base_projection_generation: u64,
}

/// Durable v2-to-v3 cutover result.
#[derive(Debug, Clone)]
pub struct ProjectViewV3CutoverOutcome {
    /// New global Project revision.
    pub project_revision: u64,
    /// New reset projection generation.
    pub projection_generation: u64,
    /// Stable durable result body.
    pub result: Value,
    /// Relay-signed events to fan out after commit; empty for replay.
    pub events: Vec<nostr::Event>,
    /// Whether an exact immutable receipt was replayed.
    pub replayed: bool,
}

#[derive(Debug)]
struct MigrationBase {
    meta_event_id: [u8; 32],
    project_revision: u64,
    projection_generation: u64,
    projection_pubkey: PublicKey,
    membership_snapshot_event_id: [u8; 32],
    initialized_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug)]
struct LegacyResourceRow {
    resource_id: Uuid,
    object_revision: u64,
    project_revision: u64,
    projection_event_id: [u8; 32],
    resource: ProjectResource,
    relations: ProjectViewRelations,
}

#[derive(Debug, Clone)]
pub(crate) struct StoredOrigin {
    pub(crate) source_type: String,
    pub(crate) change_id: [u8; 32],
    pub(crate) event_id: Option<[u8; 32]>,
    pub(crate) actor: Option<PublicKey>,
    pub(crate) audit_seq: Option<u64>,
    pub(crate) legacy_mutation: bool,
}

impl StoredOrigin {
    pub(crate) fn projection_source(&self) -> ProjectViewV3MigrationResult<V3ProjectionSource> {
        let change_id = event_id(self.change_id, "source_change_id")?;
        match self.source_type.as_str() {
            "nostr_event" => {
                let event = self.event_id.ok_or_else(|| {
                    ProjectViewV3MigrationError::Invalid(
                        "nostr_event origin is missing source_event_id".to_owned(),
                    )
                })?;
                Ok(V3ProjectionSource::NostrEvent {
                    change_id,
                    event_id: event_id(event, "source_event_id")?,
                })
            }
            "operator" => Ok(V3ProjectionSource::Operator {
                change_id,
                audit_seq: self.audit_seq.ok_or_else(|| {
                    ProjectViewV3MigrationError::Invalid(
                        "operator origin is missing audit sequence".to_owned(),
                    )
                })?,
            }),
            "system" => Ok(V3ProjectionSource::System {
                change_id,
                audit_seq: self.audit_seq.ok_or_else(|| {
                    ProjectViewV3MigrationError::Invalid(
                        "system origin is missing audit sequence".to_owned(),
                    )
                })?,
            }),
            other => Err(ProjectViewV3MigrationError::Invalid(format!(
                "source type {other} cannot be represented by the v3 projection contract"
            ))),
        }
    }
}

#[derive(Debug, Clone)]
struct CutoverObject {
    entry: ProjectViewEntryV3,
    role_level: Option<RoleLevel>,
    responsible_role_id: Option<Uuid>,
    old_projection_event_id: [u8; 32],
    origin: StoredOrigin,
    provenance_id: Uuid,
}

#[derive(Debug, Clone)]
pub(crate) struct CutoverEntity {
    pub(crate) entity: V3EntityChange,
    pub(crate) old_projection_event_id: [u8; 32],
    pub(crate) origin: StoredOrigin,
    pub(crate) updated_at: DateTime<Utc>,
}

impl LegacyResourceRow {
    fn canonical(&self) -> CanonicalLegacyResourceV1 {
        CanonicalLegacyResourceV1 {
            schema_version: 2,
            resource_id: *self.resource_id.as_bytes(),
            object_revision: self.object_revision,
            project_revision: self.project_revision,
            state: CanonicalLegacyObjectStateV1::Active,
            resource_data: Some(self.resource.clone()),
            relations: self.relations,
        }
    }

    fn digest(&self) -> ProjectViewV3MigrationResult<[u8; 32]> {
        Ok(legacy_resource_digest(&self.canonical())?)
    }
}

impl Db {
    /// Export an exact schema-v2 Resource review draft while preserving each
    /// preallocated Guide UUID across repeated exports.
    pub async fn export_project_view_v3_resource_draft(
        &self,
        community_id: CommunityId,
        requested_by: PublicKey,
        relay_pubkey: &PublicKey,
    ) -> ProjectViewV3MigrationResult<ResourceMappingDraftV1> {
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;
        require_eligible_human_in_tx(&mut tx, community_id, requested_by, true).await?;
        let base = require_schema_v2_base_in_tx(&mut tx, community_id, relay_pubkey).await?;
        let resources = load_active_legacy_resources_in_tx(&mut tx, community_id).await?;
        if resources.len() > MAX_MANIFEST_ENTRIES {
            return Err(ProjectViewV3MigrationError::Unavailable(format!(
                "active Resource count exceeds the v1 manifest limit of {MAX_MANIFEST_ENTRIES}"
            )));
        }

        let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *tx)
            .await?;
        let mut entries = Vec::with_capacity(resources.len());
        for resource in resources {
            let legacy_digest = resource.digest()?;
            let existing = sqlx::query(
                "SELECT guide_document_id, legacy_object_revision, \
                        legacy_projection_event_id, legacy_body_digest, status \
                 FROM project_view_v3_resource_mappings \
                 WHERE community_id = $1 AND resource_id = $2 FOR UPDATE",
            )
            .bind(community_id.as_uuid())
            .bind(resource.resource_id)
            .fetch_optional(&mut *tx)
            .await?;
            let (guide_document_id, status) = if let Some(row) = existing {
                let status: String = row.try_get("status")?;
                if status != "draft"
                    && (db_u64(
                        row.try_get("legacy_object_revision")?,
                        "legacy_object_revision",
                    )? != resource.object_revision
                        || bytes32(
                            row.try_get("legacy_projection_event_id")?,
                            "legacy_projection_event_id",
                        )? != resource.projection_event_id
                        || bytes32(row.try_get("legacy_body_digest")?, "legacy_body_digest")?
                            != legacy_digest)
                {
                    return Err(ProjectViewV3MigrationError::Conflict(format!(
                        "reviewed Resource {} changed after review",
                        resource.resource_id
                    )));
                }
                let guide_document_id: Uuid = row.try_get("guide_document_id")?;
                if status == "draft" {
                    sqlx::query(
                        "UPDATE project_view_v3_resource_mappings SET \
                             legacy_object_revision = $3, legacy_projection_event_id = $4, \
                             legacy_body_digest = $5, updated_at = $6 \
                         WHERE community_id = $1 AND resource_id = $2 AND status = 'draft'",
                    )
                    .bind(community_id.as_uuid())
                    .bind(resource.resource_id)
                    .bind(revision_i64(
                        resource.object_revision,
                        "legacy_object_revision",
                    )?)
                    .bind(resource.projection_event_id.as_slice())
                    .bind(legacy_digest.as_slice())
                    .bind(now)
                    .execute(&mut *tx)
                    .await?;
                }
                (guide_document_id, status)
            } else {
                let guide_document_id = Uuid::new_v4();
                sqlx::query(
                    "INSERT INTO project_view_v3_resource_mappings \
                        (community_id, resource_id, guide_document_id, \
                         legacy_object_revision, legacy_projection_event_id, \
                         legacy_body_digest, status, created_at, updated_at) \
                     VALUES ($1,$2,$3,$4,$5,$6,'draft',$7,$7)",
                )
                .bind(community_id.as_uuid())
                .bind(resource.resource_id)
                .bind(guide_document_id)
                .bind(revision_i64(
                    resource.object_revision,
                    "legacy_object_revision",
                )?)
                .bind(resource.projection_event_id.as_slice())
                .bind(legacy_digest.as_slice())
                .bind(now)
                .execute(&mut *tx)
                .await?;
                (guide_document_id, "draft".to_owned())
            };

            entries.push(ResourceMappingDraftEntryV1 {
                resource_id: resource.resource_id,
                legacy_object_revision: resource.object_revision,
                legacy_projection_event_id: hex::encode(resource.projection_event_id),
                legacy_body_digest: hex::encode(legacy_digest),
                suggested_resource_kind: suggested_resource_kind(resource.resource.resource_type)
                    .to_owned(),
                suggested_guide_markdown: guide_markdown_draft(&resource.resource),
                legacy_resource: resource.resource,
                guide_document_id,
                reviewed_v3_payload: None,
                guide_document_revision: None,
                guide_head_event_id: None,
                guide_revision_event_id: None,
                review_status: status,
            });
        }
        tx.commit().await?;
        Ok(ResourceMappingDraftV1 {
            schema_version: 1,
            community_id: *community_id.as_uuid(),
            base_meta_event_id: hex::encode(base.meta_event_id),
            base_project_revision: base.project_revision,
            base_projection_generation: base.projection_generation,
            entries,
        })
    }

    /// Recompute every canonical digest, detached signature, base pointer,
    /// legacy Resource, Guide snapshot, membership pin, and current Human
    /// eligibility check before advancing staging rows to `reviewed`.
    pub async fn validate_project_view_v3_resource_manifest(
        &self,
        community_id: CommunityId,
        manifest: &ResourceMappingManifestV1,
        relay_pubkey: &PublicKey,
    ) -> ProjectViewV3MigrationResult<ResourceMappingValidationReceipt> {
        manifest.validate()?;
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;
        let base = require_schema_v2_base_in_tx(&mut tx, community_id, relay_pubkey).await?;
        let digest = validate_manifest_in_tx(&mut tx, community_id, manifest, &base).await?;
        persist_validated_manifest_in_tx(&mut tx, community_id, manifest, &digest).await?;
        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(ResourceMappingValidationReceipt {
            community_id: *community_id.as_uuid(),
            manifest_digest: hex::encode(digest),
            entry_count: manifest.entries.len(),
            base_project_revision: manifest.base_project_revision,
            base_projection_generation: manifest.base_projection_generation,
        })
    }

    /// Execute the replay-first, exact-epoch schema-v2-to-v3 cutover.
    ///
    /// The full implementation lives below the validation helpers so all
    /// preflight logic is shared with the standalone `validate` command.
    pub async fn cutover_project_view_v3(
        &self,
        community_id: CommunityId,
        maintenance_epoch: u64,
        requested_by: PublicKey,
        idempotency_key: &str,
        manifest: &ResourceMappingManifestV1,
        relay_keys: &Keys,
    ) -> ProjectViewV3MigrationResult<ProjectViewV3CutoverOutcome> {
        cutover_project_view_v3_impl(
            self,
            community_id,
            maintenance_epoch,
            requested_by,
            idempotency_key,
            manifest,
            relay_keys,
        )
        .await
    }
}

async fn require_schema_v2_base_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    relay_pubkey: &PublicKey,
) -> ProjectViewV3MigrationResult<MigrationBase> {
    if !crate::project_view_v2::project_view_v2_enable_ready_in_tx(tx, community_id, relay_pubkey)
        .await
        .map_err(|error| ProjectViewV3MigrationError::Unavailable(error.to_string()))?
    {
        return Err(ProjectViewV3MigrationError::Unavailable(
            "schema-v2 structural/signer readiness failed".to_owned(),
        ));
    }
    let row = sqlx::query(
        "SELECT state.meta_projection_event_id, state.project_revision, \
                state.projection_generation, state.projection_pubkey, \
                state.membership_snapshot_event_id, state.initialized_at, state.updated_at \
         FROM project_view_state state \
         JOIN communities community ON community.id = state.community_id \
         WHERE state.community_id = $1 AND state.schema_version = 2 \
           AND community.project_view_schema_version = 2 \
           AND community.archived_at IS NULL \
         FOR UPDATE OF state, community",
    )
    .bind(community_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| {
        ProjectViewV3MigrationError::Unavailable(
            "an initialized non-archived schema-v2 Project View is required".to_owned(),
        )
    })?;
    Ok(MigrationBase {
        meta_event_id: bytes32(
            row.try_get("meta_projection_event_id")?,
            "meta_projection_event_id",
        )?,
        project_revision: db_u64(row.try_get("project_revision")?, "project_revision")?,
        projection_generation: db_u64(
            row.try_get("projection_generation")?,
            "projection_generation",
        )?,
        projection_pubkey: public_key(
            &row.try_get::<Vec<u8>, _>("projection_pubkey")?,
            "projection_pubkey",
        )?,
        membership_snapshot_event_id: bytes32(
            row.try_get("membership_snapshot_event_id")?,
            "membership_snapshot_event_id",
        )?,
        initialized_at: row.try_get("initialized_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn load_active_legacy_resources_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
) -> ProjectViewV3MigrationResult<Vec<LegacyResourceRow>> {
    let rows = sqlx::query(
        "SELECT object_id, object_revision, project_revision, projection_event_id, body, \
                under_goal_id, under_plan_id, planned_in_stage_id, \
                about_object_id, about_object_type, handles_object_id, handles_object_type \
         FROM project_view_objects \
         WHERE community_id = $1 AND schema_version = 2 \
           AND object_type = 'resource' AND deleted_at IS NULL \
         ORDER BY object_id FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            let resource: ProjectResource =
                serde_json::from_value(row.try_get("body")?).map_err(|error| {
                    ProjectViewV3MigrationError::Invalid(format!(
                        "invalid canonical legacy Resource body: {error}"
                    ))
                })?;
            Ok(LegacyResourceRow {
                resource_id: row.try_get("object_id")?,
                object_revision: db_u64(row.try_get("object_revision")?, "object_revision")?,
                project_revision: db_u64(row.try_get("project_revision")?, "project_revision")?,
                projection_event_id: bytes32(
                    row.try_get("projection_event_id")?,
                    "projection_event_id",
                )?,
                resource,
                relations: relations_from_row(&row)?,
            })
        })
        .collect()
}

async fn validate_manifest_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    manifest: &ResourceMappingManifestV1,
    base: &MigrationBase,
) -> ProjectViewV3MigrationResult<[u8; 32]> {
    if manifest.community_id != *community_id.as_uuid().as_bytes()
        || manifest.base_meta_event_id != base.meta_event_id
        || manifest.base_project_revision != base.project_revision
        || manifest.base_projection_generation != base.projection_generation
    {
        return Err(ProjectViewV3MigrationError::Conflict(
            "manifest header does not match the exact current schema-v2 base".to_owned(),
        ));
    }
    let resources = load_active_legacy_resources_in_tx(tx, community_id).await?;
    let resource_ids = resources
        .iter()
        .map(|resource| *resource.resource_id.as_bytes())
        .collect::<BTreeSet<_>>();
    let manifest_ids = manifest
        .entries
        .iter()
        .map(|entry| entry.resource_id)
        .collect::<BTreeSet<_>>();
    if resource_ids != manifest_ids || resources.len() != manifest.entries.len() {
        return Err(ProjectViewV3MigrationError::Conflict(
            "manifest Resource set is not the exact active legacy Resource set".to_owned(),
        ));
    }
    let by_id = resources
        .into_iter()
        .map(|resource| (*resource.resource_id.as_bytes(), resource))
        .collect::<BTreeMap<_, _>>();
    for entry in &manifest.entries {
        let resource = by_id.get(&entry.resource_id).ok_or_else(|| {
            ProjectViewV3MigrationError::Conflict("manifest Resource disappeared".to_owned())
        })?;
        if entry.legacy_object_revision != resource.object_revision
            || entry.legacy_projection_event_id != resource.projection_event_id
            || entry.legacy_body_digest != resource.digest()?
        {
            return Err(ProjectViewV3MigrationError::Conflict(format!(
                "legacy Resource {} no longer matches its reviewed pins",
                resource.resource_id
            )));
        }
        validate_guide_in_tx(tx, community_id, entry).await?;
        let reviewer = public_key(&entry.reviewed_by_pubkey, "reviewed_by_pubkey")?;
        require_eligible_human_in_tx(tx, community_id, reviewer, false).await?;
        require_reviewer_in_base_membership_in_tx(
            tx,
            community_id,
            &base.membership_snapshot_event_id,
            reviewer,
        )
        .await?;
        verify_review_signature(entry)?;

        let staged: Option<(Uuid, String)> = sqlx::query_as(
            "SELECT guide_document_id, status \
             FROM project_view_v3_resource_mappings \
             WHERE community_id = $1 AND resource_id = $2 FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .bind(resource.resource_id)
        .fetch_optional(&mut **tx)
        .await?;
        let Some((guide_document_id, status)) = staged else {
            return Err(ProjectViewV3MigrationError::Conflict(format!(
                "Resource {} has no persistent exported mapping",
                resource.resource_id
            )));
        };
        if status == "consumed"
            || guide_document_id.as_bytes()
                != &entry.reviewed_v3_payload.resource_data.guide_document_id
        {
            return Err(ProjectViewV3MigrationError::Conflict(format!(
                "Resource {} does not use its preallocated unconsumed Guide mapping",
                resource.resource_id
            )));
        }
    }
    Ok(manifest_digest(manifest)?)
}

async fn validate_guide_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    entry: &ReviewedResourceMappingV1,
) -> ProjectViewV3MigrationResult<()> {
    let guide_id = Uuid::from_bytes(entry.reviewed_v3_payload.resource_data.guide_document_id);
    let row = sqlx::query(
        "SELECT document.current_revision, document.current_head_event_id, \
                document.current_revision_event_id, revision.state, revision.title, \
                revision.summary, revision.content_markdown \
         FROM project_documents document \
         JOIN project_document_revisions revision \
           ON revision.community_id = document.community_id \
          AND revision.document_id = document.document_id \
          AND revision.document_revision = document.current_revision \
         WHERE document.community_id = $1 AND document.document_id = $2 \
           AND document.state = 'active' FOR SHARE OF document, revision",
    )
    .bind(community_id.as_uuid())
    .bind(guide_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| {
        ProjectViewV3MigrationError::Conflict(format!(
            "Guide Document {guide_id} is missing or not active"
        ))
    })?;
    let revision = db_u64(row.try_get("current_revision")?, "guide_document_revision")?;
    let head = bytes32(row.try_get("current_head_event_id")?, "guide_head_event_id")?;
    let revision_event = bytes32(
        row.try_get("current_revision_event_id")?,
        "guide_revision_event_id",
    )?;
    if row.try_get::<String, _>("state")? != "active"
        || revision != entry.guide_document_revision
        || head != entry.guide_head_event_id
        || revision_event != entry.guide_revision_event_id
    {
        return Err(ProjectViewV3MigrationError::Conflict(format!(
            "Guide Document {guide_id} no longer matches its reviewed pointers"
        )));
    }
    let snapshot = CanonicalGuideSnapshotV1 {
        document_id: *guide_id.as_bytes(),
        document_revision: revision,
        title: row.try_get("title")?,
        summary: row.try_get("summary")?,
        content_markdown: row.try_get("content_markdown")?,
    };
    if guide_snapshot_digest(&snapshot)? != entry.guide_content_digest {
        return Err(ProjectViewV3MigrationError::Conflict(format!(
            "Guide Document {guide_id} content changed after review"
        )));
    }
    Ok(())
}

async fn persist_validated_manifest_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    manifest: &ResourceMappingManifestV1,
    digest: &[u8; 32],
) -> ProjectViewV3MigrationResult<()> {
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **tx)
        .await?;
    for entry in &manifest.entries {
        let resource_id = Uuid::from_bytes(entry.resource_id);
        let guide_id = Uuid::from_bytes(entry.reviewed_v3_payload.resource_data.guide_document_id);
        let payload = reviewed_payload_json(entry);
        let updated = sqlx::query(
            "UPDATE project_view_v3_resource_mappings SET \
                 guide_document_revision = $3, guide_head_event_id = $4, \
                 guide_revision_event_id = $5, guide_content_digest = $6, \
                 reviewed_v3_payload = $7, v3_payload_digest = $8, \
                 mapping_entry_digest = $9, reviewed_by_pubkey = $10, \
                 reviewed_at_unix_micros = $11, review_digest = $12, \
                 review_signature = $13, manifest_digest = $14, \
                 status = 'reviewed', updated_at = $15 \
             WHERE community_id = $1 AND resource_id = $2 \
               AND guide_document_id = $16 AND status IN ('draft', 'reviewed')",
        )
        .bind(community_id.as_uuid())
        .bind(resource_id)
        .bind(revision_i64(
            entry.guide_document_revision,
            "guide_document_revision",
        )?)
        .bind(entry.guide_head_event_id.as_slice())
        .bind(entry.guide_revision_event_id.as_slice())
        .bind(entry.guide_content_digest.as_slice())
        .bind(payload)
        .bind(entry.v3_payload_digest.as_slice())
        .bind(entry.mapping_entry_digest.as_slice())
        .bind(entry.reviewed_by_pubkey.as_slice())
        .bind(entry.reviewed_at_unix_micros)
        .bind(entry.review_digest.as_slice())
        .bind(entry.review_signature.as_bytes().as_slice())
        .bind(digest.as_slice())
        .bind(now)
        .bind(guide_id)
        .execute(&mut **tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(ProjectViewV3MigrationError::Conflict(format!(
                "Resource {resource_id} staging mapping changed during validation"
            )));
        }
    }
    Ok(())
}

async fn require_eligible_human_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    pubkey: PublicKey,
    owner_only: bool,
) -> ProjectViewV3MigrationResult<()> {
    let bytes = pubkey.to_bytes();
    let role: Option<String> = sqlx::query_scalar(
        "SELECT member.role \
         FROM relay_members member \
         LEFT JOIN users actor \
           ON actor.community_id = member.community_id \
          AND actor.pubkey = decode(member.pubkey, 'hex') \
         WHERE member.community_id = $1 AND member.pubkey = $2 \
           AND ($3::boolean = FALSE OR member.role = 'owner') \
           AND actor.agent_owner_pubkey IS NULL \
           AND NOT EXISTS ( \
               SELECT 1 FROM community_bans restriction \
               WHERE restriction.community_id = member.community_id \
                 AND restriction.pubkey = $4 \
                 AND ( \
                     (restriction.banned AND (restriction.ban_expires_at IS NULL \
                         OR restriction.ban_expires_at > clock_timestamp())) \
                     OR restriction.muted_until > clock_timestamp() \
                 ) \
           )",
    )
    .bind(community_id.as_uuid())
    .bind(pubkey.to_hex())
    .bind(owner_only)
    .bind(bytes.as_slice())
    .fetch_optional(&mut **tx)
    .await?;
    if role.is_none() {
        return Err(ProjectViewV3MigrationError::Forbidden(if owner_only {
            "export requires the current eligible direct Human owner".to_owned()
        } else {
            format!(
                "reviewer {} is not an eligible direct Human member",
                pubkey.to_hex()
            )
        }));
    }
    Ok(())
}

async fn require_reviewer_in_base_membership_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    membership_event_id: &[u8; 32],
    reviewer: PublicKey,
) -> ProjectViewV3MigrationResult<()> {
    let reviewer_hex = reviewer.to_hex();
    let present: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM events event, jsonb_array_elements(event.tags) tag \
             WHERE event.community_id = $1 AND event.id = $2 \
               AND event.deleted_at IS NULL \
               AND jsonb_array_length(tag) = 3 \
               AND tag->>0 = 'member' AND tag->>1 = $3 \
               AND tag->>2 IN ('owner', 'admin', 'member') \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(membership_event_id.as_slice())
    .bind(reviewer_hex)
    .fetch_one(&mut **tx)
    .await?;
    if !present {
        return Err(ProjectViewV3MigrationError::Forbidden(
            "reviewer is absent from the manifest base membership snapshot".to_owned(),
        ));
    }
    Ok(())
}

fn verify_review_signature(entry: &ReviewedResourceMappingV1) -> ProjectViewV3MigrationResult<()> {
    let reviewer = public_key(&entry.reviewed_by_pubkey, "reviewed_by_pubkey")?;
    let signature = Signature::from_slice(entry.review_signature.as_bytes()).map_err(|error| {
        ProjectViewV3MigrationError::Invalid(format!("invalid review signature bytes: {error}"))
    })?;
    let message = Message::from_digest(entry.review_digest);
    let xonly = reviewer.xonly().map_err(|error| {
        ProjectViewV3MigrationError::Invalid(format!("invalid reviewer public key: {error}"))
    })?;
    SECP256K1
        .verify_schnorr(&signature, &message, &xonly)
        .map_err(|error| {
            ProjectViewV3MigrationError::Invalid(format!(
                "review signature verification failed: {error}"
            ))
        })
}

fn reviewed_payload_json(entry: &ReviewedResourceMappingV1) -> Value {
    let resource = &entry.reviewed_v3_payload.resource_data;
    let mut resource_data = serde_json::Map::from_iter([
        ("name".to_owned(), Value::String(resource.name.clone())),
        (
            "resource_kind".to_owned(),
            Value::String(resource.resource_kind.clone()),
        ),
        (
            "guide_document_id".to_owned(),
            Value::String(Uuid::from_bytes(resource.guide_document_id).to_string()),
        ),
    ]);
    if let Some(summary) = &resource.summary {
        resource_data.insert("summary".to_owned(), Value::String(summary.clone()));
    }
    json!({
        "resource_data": Value::Object(resource_data),
        "context_references": [],
    })
}

fn relations_from_row(
    row: &sqlx::postgres::PgRow,
) -> ProjectViewV3MigrationResult<ProjectViewRelations> {
    Ok(ProjectViewRelations {
        under_goal_id: row.try_get("under_goal_id")?,
        under_plan_id: row.try_get("under_plan_id")?,
        planned_in_stage_id: row.try_get("planned_in_stage_id")?,
        about: typed_reference(
            row.try_get("about_object_id")?,
            row.try_get("about_object_type")?,
            "about",
        )?,
        handles: typed_reference(
            row.try_get("handles_object_id")?,
            row.try_get("handles_object_type")?,
            "handles",
        )?,
    })
}

fn typed_reference(
    id: Option<Uuid>,
    object_type: Option<String>,
    field: &str,
) -> ProjectViewV3MigrationResult<Option<ObjectRef>> {
    match (id, object_type) {
        (None, None) => Ok(None),
        (Some(object_id), Some(object_type)) => Ok(Some(ObjectRef {
            object_id,
            object_type: parse_object_type(&object_type)?,
        })),
        _ => Err(ProjectViewV3MigrationError::Invalid(format!(
            "stored {field} relation has an incomplete ID/type pair"
        ))),
    }
}

fn parse_object_type(value: &str) -> ProjectViewV3MigrationResult<ProjectViewObjectType> {
    match value {
        "project_profile" => Ok(ProjectViewObjectType::ProjectProfile),
        "goal" => Ok(ProjectViewObjectType::Goal),
        "role" => Ok(ProjectViewObjectType::Role),
        "plan" => Ok(ProjectViewObjectType::Plan),
        "stage" => Ok(ProjectViewObjectType::Stage),
        "requirement" => Ok(ProjectViewObjectType::Requirement),
        "issue" => Ok(ProjectViewObjectType::Issue),
        "work" => Ok(ProjectViewObjectType::Work),
        "resource" => Ok(ProjectViewObjectType::Resource),
        _ => Err(ProjectViewV3MigrationError::Invalid(format!(
            "unknown stored Project View object type {value}"
        ))),
    }
}

const fn suggested_resource_kind(resource_type: ResourceType) -> &'static str {
    match resource_type {
        ResourceType::Repository => "repository",
        ResourceType::Document => "external_document",
        ResourceType::Design => "design",
        ResourceType::Service => "service",
        ResourceType::Environment => "environment",
        ResourceType::Artifact => "artifact",
        ResourceType::Url => "external_link",
    }
}

fn guide_markdown_draft(resource: &ProjectResource) -> String {
    let locator_fence = markdown_fence(&resource.locator.value);
    let description_fence = markdown_fence(&resource.description);
    format!(
        "# {} Guide\n\n> Review this draft, remove any secrets, and verify every value before publishing.\n\n## Legacy locator\n\nType: `{}`\n\n{locator_fence}\n{}\n{locator_fence}\n\n## Legacy description\n\n{description_fence}\n{}\n{description_fence}\n",
        resource.name,
        locator_type_name(resource.locator.locator_type),
        resource.locator.value,
        resource.description,
    )
}

fn markdown_fence(value: &str) -> String {
    let longest = value
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    "`".repeat(longest.saturating_add(1).max(3))
}

const fn locator_type_name(locator_type: LocatorType) -> &'static str {
    match locator_type {
        LocatorType::Url => "url",
        LocatorType::NostrAddress => "nostr_address",
        LocatorType::NostrEvent => "nostr_event",
        LocatorType::BuzzDeepLink => "buzz_deep_link",
    }
}

fn bytes32(value: Vec<u8>, field: &str) -> ProjectViewV3MigrationResult<[u8; 32]> {
    value.try_into().map_err(|value: Vec<u8>| {
        ProjectViewV3MigrationError::Invalid(format!(
            "{field} must contain 32 bytes, found {}",
            value.len()
        ))
    })
}

fn public_key(value: &[u8], field: &str) -> ProjectViewV3MigrationResult<PublicKey> {
    PublicKey::from_slice(value)
        .map_err(|error| ProjectViewV3MigrationError::Invalid(format!("invalid {field}: {error}")))
}

fn db_u64(value: i64, field: &str) -> ProjectViewV3MigrationResult<u64> {
    u64::try_from(value)
        .map_err(|_| ProjectViewV3MigrationError::Invalid(format!("{field} is negative")))
}

fn revision_i64(value: u64, field: &str) -> ProjectViewV3MigrationResult<i64> {
    i64::try_from(value).map_err(|_| {
        ProjectViewV3MigrationError::Invalid(format!("{field} exceeds PostgreSQL BIGINT"))
    })
}

fn cutover_idempotency_hash(value: &str) -> ProjectViewV3MigrationResult<[u8; 32]> {
    if value.is_empty() || value.len() > 256 || value.contains('\0') {
        return Err(ProjectViewV3MigrationError::Invalid(
            "idempotency key must contain 1..=256 non-NUL UTF-8 bytes".to_owned(),
        ));
    }
    Ok(digest_parts(&[
        CUTOVER_IDEMPOTENCY_DOMAIN,
        value.as_bytes(),
    ]))
}

fn cutover_request_hash(
    community_id: CommunityId,
    maintenance_epoch: u64,
    manifest_digest: &[u8; 32],
) -> [u8; 32] {
    digest_parts(&[
        CUTOVER_REQUEST_DOMAIN,
        community_id.as_uuid().as_bytes(),
        &maintenance_epoch.to_be_bytes(),
        manifest_digest,
        &3_u16.to_be_bytes(),
    ])
}

fn digest_parts(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

fn event_id(value: [u8; 32], field: &str) -> ProjectViewV3MigrationResult<EventId> {
    EventId::from_slice(&value)
        .map_err(|error| ProjectViewV3MigrationError::Invalid(format!("invalid {field}: {error}")))
}

fn json_u64(value: &Value, field: &str) -> ProjectViewV3MigrationResult<u64> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .filter(|value| (1..=MAX_SAFE_REVISION).contains(value))
        .ok_or_else(|| {
            ProjectViewV3MigrationError::Invalid(format!(
                "stored cutover receipt has invalid {field}"
            ))
        })
}

async fn require_operator_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    requested_by: PublicKey,
) -> ProjectViewV3MigrationResult<()> {
    require_eligible_human_in_tx(tx, community_id, requested_by, false).await?;
    let role: Option<String> = sqlx::query_scalar(
        "SELECT role FROM relay_members \
         WHERE community_id = $1 AND pubkey = $2 AND role IN ('owner', 'admin')",
    )
    .bind(community_id.as_uuid())
    .bind(requested_by.to_hex())
    .fetch_optional(&mut **tx)
    .await?;
    if role.is_none() {
        return Err(ProjectViewV3MigrationError::Forbidden(
            "cutover requires a current eligible Human owner or admin".to_owned(),
        ));
    }
    Ok(())
}

async fn require_cutover_fence_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    epoch: i64,
) -> ProjectViewV3MigrationResult<()> {
    crate::project_view_maintenance::validate_freeze_in_tx(tx, community_id, epoch)
        .await
        .map_err(|error| ProjectViewV3MigrationError::Conflict(error.to_string()))?;
    let incompatible: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 \
             FROM project_view_maintenance_assignment_baselines baseline \
             JOIN project_view_maintenance_epochs epoch \
               ON epoch.community_id = baseline.community_id \
              AND epoch.maintenance_epoch = baseline.maintenance_epoch \
             JOIN project_view_maintenance_assignment_acks ack \
               ON ack.community_id = baseline.community_id \
              AND ack.maintenance_epoch = baseline.maintenance_epoch \
              AND ack.assignment_id = baseline.assignment_id \
             WHERE baseline.community_id = $1 AND baseline.maintenance_epoch = $2 \
               AND (ack.status <> 'quiesced' \
                    OR ack.client_protocol_version < epoch.required_client_protocol_version) \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(epoch)
    .fetch_one(&mut **tx)
    .await?;
    if incompatible {
        return Err(ProjectViewV3MigrationError::Conflict(
            "one or more Assignment acknowledgements are incompatible with this epoch".to_owned(),
        ));
    }
    Ok(())
}

async fn live_event_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    id: &[u8; 32],
) -> ProjectViewV3MigrationResult<nostr::Event> {
    let row = sqlx::query(
        "SELECT id, pubkey, created_at, kind, tags, content, sig, received_at, channel_id \
         FROM events WHERE community_id = $1 AND id = $2 AND deleted_at IS NULL",
    )
    .bind(community_id.as_uuid())
    .bind(id.as_slice())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| {
        ProjectViewV3MigrationError::Conflict(format!(
            "projection event {} is missing or retired",
            hex::encode(id)
        ))
    })?;
    let stored = crate::event::row_to_stored_event(row)?
        .ok_or_else(|| ProjectViewV3MigrationError::Invalid("stored event is malformed".into()))?;
    if stored.channel_id.is_some() {
        return Err(ProjectViewV3MigrationError::Invalid(
            "Project projection event is unexpectedly channel-scoped".to_owned(),
        ));
    }
    Ok(stored.event)
}

#[allow(clippy::too_many_lines)]
async fn require_document_projection_ready_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    relay_pubkey: &PublicKey,
) -> ProjectViewV3MigrationResult<()> {
    let state = sqlx::query(
        "SELECT community.project_document_enabled, state.catalog_revision, \
                state.active_document_count, state.projection_generation, \
                state.projection_pubkey, state.meta_projection_event_id, state.updated_at \
         FROM communities community \
         JOIN project_document_state state ON state.community_id = community.id \
         WHERE community.id = $1 AND community.archived_at IS NULL \
         FOR SHARE OF community, state",
    )
    .bind(community_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| {
        ProjectViewV3MigrationError::Unavailable(
            "Project Document state is not bootstrapped".to_owned(),
        )
    })?;
    if !state.try_get::<bool, _>("project_document_enabled")?
        || public_key(
            &state.try_get::<Vec<u8>, _>("projection_pubkey")?,
            "document.projection_pubkey",
        )? != *relay_pubkey
    {
        return Err(ProjectViewV3MigrationError::Unavailable(
            "Project Document capability or stable signer is not ready".to_owned(),
        ));
    }
    let meta_id = bytes32(
        state.try_get("meta_projection_event_id")?,
        "document.meta_projection_event_id",
    )?;
    let meta_event = live_event_in_tx(tx, community_id, &meta_id).await?;
    if u32::from(meta_event.kind.as_u16()) != KIND_PROJECT_DOCUMENT_META {
        return Err(ProjectViewV3MigrationError::Invalid(
            "Project Document metadata pointer has the wrong kind".to_owned(),
        ));
    }
    let meta = buzz_sdk::project_document::parse_document_meta(&meta_event, relay_pubkey)
        .map_err(|error| ProjectViewV3MigrationError::Invalid(error.to_string()))?;
    let catalog_revision = db_u64(state.try_get("catalog_revision")?, "catalog_revision")?;
    let active_count = db_u64(
        i64::from(state.try_get::<i32, _>("active_document_count")?),
        "active_document_count",
    )?;
    let generation = db_u64(
        state.try_get("projection_generation")?,
        "document.projection_generation",
    )?;
    if meta.projection.project_id != *community_id.as_uuid()
        || meta.projection.catalog_revision != catalog_revision
        || meta.projection.active_document_count != active_count
        || meta.projection.projection_generation != generation
        || meta.projection.updated_at != state.try_get::<DateTime<Utc>, _>("updated_at")?
    {
        return Err(ProjectViewV3MigrationError::Invalid(
            "Project Document metadata projection differs from canonical state".to_owned(),
        ));
    }

    let actual_active: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM project_documents \
         WHERE community_id = $1 AND state = 'active'",
    )
    .bind(community_id.as_uuid())
    .fetch_one(&mut **tx)
    .await?;
    if db_u64(actual_active, "actual_active_document_count")? != active_count {
        return Err(ProjectViewV3MigrationError::Invalid(
            "Project Document active count differs from canonical rows".to_owned(),
        ));
    }
    let rows = sqlx::query(
        "SELECT document_id, current_revision, state, current_source_change_id, \
                current_head_event_id, current_revision_event_id \
         FROM project_documents WHERE community_id = $1 ORDER BY document_id FOR SHARE",
    )
    .bind(community_id.as_uuid())
    .fetch_all(&mut **tx)
    .await?;
    for row in rows {
        let document_id: Uuid = row.try_get("document_id")?;
        let revision = db_u64(row.try_get("current_revision")?, "document_revision")?;
        let state_name: String = row.try_get("state")?;
        let source = bytes32(row.try_get("current_source_change_id")?, "document.source")?;
        let head_id = bytes32(row.try_get("current_head_event_id")?, "document.head")?;
        let revision_id = bytes32(
            row.try_get("current_revision_event_id")?,
            "document.revision",
        )?;
        let head_event = live_event_in_tx(tx, community_id, &head_id).await?;
        let revision_event = live_event_in_tx(tx, community_id, &revision_id).await?;
        if u32::from(head_event.kind.as_u16()) != KIND_PROJECT_DOCUMENT_HEAD
            || u32::from(revision_event.kind.as_u16()) != KIND_PROJECT_DOCUMENT_REVISION
        {
            return Err(ProjectViewV3MigrationError::Invalid(format!(
                "Document {document_id} points at an event with the wrong kind"
            )));
        }
        let head = buzz_sdk::project_document::parse_document_head(
            &head_event,
            relay_pubkey,
            community_id,
        )
        .map_err(|error| ProjectViewV3MigrationError::Invalid(error.to_string()))?;
        let revision_projection = buzz_sdk::project_document::parse_document_revision(
            &revision_event,
            relay_pubkey,
            community_id,
        )
        .map_err(|error| ProjectViewV3MigrationError::Invalid(error.to_string()))?;
        let current =
            buzz_sdk::project_document::VerifiedCurrentDocument::new(head, revision_projection)
                .map_err(|error| ProjectViewV3MigrationError::Invalid(error.to_string()))?;
        let (projected_id, projected_revision, projected_state, projected_source) =
            match &current.head.projection {
                buzz_project_document::DocumentHeadProjection::Active {
                    document_id,
                    document_revision,
                    source_event_id,
                    ..
                } => (*document_id, *document_revision, "active", *source_event_id),
                buzz_project_document::DocumentHeadProjection::Deleted {
                    document_id,
                    document_revision,
                    source_event_id,
                    ..
                } => (
                    *document_id,
                    *document_revision,
                    "deleted",
                    *source_event_id,
                ),
            };
        if projected_id != document_id
            || projected_revision != revision
            || projected_state != state_name
            || projected_source.to_bytes() != source
        {
            return Err(ProjectViewV3MigrationError::Invalid(format!(
                "Document {document_id} projection differs from canonical state"
            )));
        }
    }
    Ok(())
}

async fn require_exact_reviewed_staging_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    manifest: &ResourceMappingManifestV1,
    digest: &[u8; 32],
) -> ProjectViewV3MigrationResult<()> {
    let staged_count: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM project_view_v3_resource_mappings \
         WHERE community_id = $1 AND status = 'reviewed' AND manifest_digest = $2",
    )
    .bind(community_id.as_uuid())
    .bind(digest.as_slice())
    .fetch_one(&mut **tx)
    .await?;
    if usize::try_from(staged_count).ok() != Some(manifest.entries.len()) {
        return Err(ProjectViewV3MigrationError::Conflict(
            "reviewed staging set does not exactly cover the manifest".to_owned(),
        ));
    }
    for entry in &manifest.entries {
        let row = sqlx::query(
            "SELECT guide_document_id, legacy_object_revision, legacy_projection_event_id, \
                    legacy_body_digest, guide_document_revision, guide_head_event_id, \
                    guide_revision_event_id, guide_content_digest, reviewed_v3_payload, \
                    v3_payload_digest, mapping_entry_digest, reviewed_by_pubkey, \
                    reviewed_at_unix_micros, review_digest, review_signature, \
                    manifest_digest, status \
             FROM project_view_v3_resource_mappings \
             WHERE community_id = $1 AND resource_id = $2 FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .bind(Uuid::from_bytes(entry.resource_id))
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| {
            ProjectViewV3MigrationError::Conflict("reviewed staging entry is missing".to_owned())
        })?;
        let exact = row.try_get::<String, _>("status")? == "reviewed"
            && row.try_get::<Uuid, _>("guide_document_id")?.as_bytes()
                == &entry.reviewed_v3_payload.resource_data.guide_document_id
            && db_u64(
                row.try_get("legacy_object_revision")?,
                "legacy_object_revision",
            )? == entry.legacy_object_revision
            && bytes32(
                row.try_get("legacy_projection_event_id")?,
                "legacy_projection_event_id",
            )? == entry.legacy_projection_event_id
            && bytes32(row.try_get("legacy_body_digest")?, "legacy_body_digest")?
                == entry.legacy_body_digest
            && db_u64(
                row.try_get("guide_document_revision")?,
                "guide_document_revision",
            )? == entry.guide_document_revision
            && bytes32(row.try_get("guide_head_event_id")?, "guide_head_event_id")?
                == entry.guide_head_event_id
            && bytes32(
                row.try_get("guide_revision_event_id")?,
                "guide_revision_event_id",
            )? == entry.guide_revision_event_id
            && bytes32(row.try_get("guide_content_digest")?, "guide_content_digest")?
                == entry.guide_content_digest
            && row.try_get::<Value, _>("reviewed_v3_payload")? == reviewed_payload_json(entry)
            && bytes32(row.try_get("v3_payload_digest")?, "v3_payload_digest")?
                == entry.v3_payload_digest
            && bytes32(row.try_get("mapping_entry_digest")?, "mapping_entry_digest")?
                == entry.mapping_entry_digest
            && bytes32(row.try_get("reviewed_by_pubkey")?, "reviewed_by_pubkey")?
                == entry.reviewed_by_pubkey
            && row.try_get::<i64, _>("reviewed_at_unix_micros")? == entry.reviewed_at_unix_micros
            && bytes32(row.try_get("review_digest")?, "review_digest")? == entry.review_digest
            && row.try_get::<Vec<u8>, _>("review_signature")?.as_slice()
                == entry.review_signature.as_bytes()
            && bytes32(row.try_get("manifest_digest")?, "manifest_digest")? == *digest;
        if !exact {
            return Err(ProjectViewV3MigrationError::Conflict(format!(
                "reviewed staging entry {} differs from the exact manifest",
                Uuid::from_bytes(entry.resource_id)
            )));
        }
    }
    Ok(())
}

async fn cutover_canonical_time_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    base_updated_at: DateTime<Utc>,
) -> ProjectViewV3MigrationResult<DateTime<Utc>> {
    Ok(sqlx::query_scalar(
        "SELECT GREATEST( \
             clock_timestamp(), \
             $2::timestamptz + interval '1 microsecond', \
             COALESCE((SELECT max(created_at) + interval '1 second' FROM events \
                       WHERE community_id = $1 AND kind = $3 AND deleted_at IS NULL), \
                      '-infinity'::timestamptz) \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(base_updated_at)
    .bind(i32::try_from(KIND_NIP43_MEMBERSHIP_LIST).map_err(|_| {
        ProjectViewV3MigrationError::Invalid("membership kind does not fit i32".to_owned())
    })?)
    .fetch_one(&mut **tx)
    .await?)
}

fn parse_role_level(value: &str) -> ProjectViewV3MigrationResult<RoleLevel> {
    match value {
        "admin" => Ok(RoleLevel::Admin),
        "member" => Ok(RoleLevel::Member),
        _ => Err(ProjectViewV3MigrationError::Invalid(format!(
            "unknown stored Role level {value}"
        ))),
    }
}

fn legacy_data_to_v3(
    data: ProjectViewObjectData,
) -> ProjectViewV3MigrationResult<ProjectViewObjectDataV3> {
    Ok(match data {
        ProjectViewObjectData::ProjectProfile(value) => {
            ProjectViewObjectDataV3::ProjectProfile(value)
        }
        ProjectViewObjectData::Goal(value) => ProjectViewObjectDataV3::Goal(value),
        ProjectViewObjectData::Role(value) => ProjectViewObjectDataV3::Role(value),
        ProjectViewObjectData::Plan(value) => ProjectViewObjectDataV3::Plan(value),
        ProjectViewObjectData::Stage(value) => ProjectViewObjectDataV3::Stage(value),
        ProjectViewObjectData::Requirement(value) => ProjectViewObjectDataV3::Requirement(value),
        ProjectViewObjectData::Issue(value) => ProjectViewObjectDataV3::Issue(value),
        ProjectViewObjectData::Work(value) => ProjectViewObjectDataV3::Work(value),
        ProjectViewObjectData::Resource(_) => {
            return Err(ProjectViewV3MigrationError::Invalid(
                "legacy Resource requires a reviewed mapping".to_owned(),
            ));
        }
    })
}

pub(crate) async fn load_change_origin_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    change_id: [u8; 32],
    expected_project_revision: u64,
) -> ProjectViewV3MigrationResult<StoredOrigin> {
    let row = sqlx::query(
        "SELECT source_type, source_event_id, source_audit_seq, actor_pubkey, \
                project_revision \
         FROM project_view_changes WHERE community_id = $1 AND change_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(change_id.as_slice())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| {
        ProjectViewV3MigrationError::Invalid(format!(
            "typed source change {} is missing",
            hex::encode(change_id)
        ))
    })?;
    if db_u64(row.try_get("project_revision")?, "source.project_revision")?
        != expected_project_revision
    {
        return Err(ProjectViewV3MigrationError::Invalid(
            "typed source change revision differs from its canonical head".to_owned(),
        ));
    }
    let source_type: String = row.try_get("source_type")?;
    if !matches!(source_type.as_str(), "nostr_event" | "operator" | "system") {
        return Err(ProjectViewV3MigrationError::Unavailable(format!(
            "legacy source type {source_type} has no Project View v3 projection representation"
        )));
    }
    let source_event_id = row
        .try_get::<Option<Vec<u8>>, _>("source_event_id")?
        .map(|value| bytes32(value, "source_event_id"))
        .transpose()?;
    let actor = row
        .try_get::<Option<Vec<u8>>, _>("actor_pubkey")?
        .map(|value| public_key(&value, "source.actor_pubkey"))
        .transpose()?;
    let audit_seq = row
        .try_get::<Option<i64>, _>("source_audit_seq")?
        .map(|value| db_u64(value, "source_audit_seq"))
        .transpose()?;
    match source_type.as_str() {
        "nostr_event" if source_event_id != Some(change_id) || actor.is_none() => {
            return Err(ProjectViewV3MigrationError::Invalid(
                "stored Nostr source change has invalid event/actor linkage".to_owned(),
            ));
        }
        "operator" | "system" if source_event_id.is_some() || audit_seq.is_none() => {
            return Err(ProjectViewV3MigrationError::Invalid(
                "stored audited source change has invalid event/audit linkage".to_owned(),
            ));
        }
        _ => {}
    }
    Ok(StoredOrigin {
        source_type,
        change_id,
        event_id: source_event_id,
        actor,
        audit_seq,
        legacy_mutation: false,
    })
}

fn mutation_targets_object(
    mutation: &Mutation,
    community_id: CommunityId,
    object_id: Uuid,
    object_type: ProjectViewObjectType,
) -> bool {
    match &mutation.request {
        MutationRequest::Initialize(initial) => {
            (object_type == ProjectViewObjectType::ProjectProfile
                && object_id == *community_id.as_uuid())
                || (object_type == ProjectViewObjectType::Goal
                    && initial.goals.iter().any(|goal| goal.id == object_id))
        }
        MutationRequest::Create(create) => {
            create.object.id() == object_id && create.object.object_type() == object_type
        }
        MutationRequest::Update(update) => {
            update.object_id() == object_id && update.object_type() == object_type
        }
        MutationRequest::Delete(delete) => {
            delete.object_id == object_id && delete.object_type == object_type
        }
    }
}

async fn load_legacy_mutation_origin_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    source_event_id: [u8; 32],
    object_id: Uuid,
    object_type: ProjectViewObjectType,
    expected_project_revision: u64,
    expected_actor: PublicKey,
) -> ProjectViewV3MigrationResult<StoredOrigin> {
    let row = sqlx::query(
        "SELECT project_revision, actor_pubkey, operation, object_type, object_id \
         FROM project_view_mutations WHERE community_id = $1 AND event_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(source_event_id.as_slice())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| {
        ProjectViewV3MigrationError::Invalid(format!(
            "legacy mutation {} is missing",
            hex::encode(source_event_id)
        ))
    })?;
    let actor = public_key(
        &row.try_get::<Vec<u8>, _>("actor_pubkey")?,
        "legacy.actor_pubkey",
    )?;
    let stored_type = row
        .try_get::<Option<String>, _>("object_type")?
        .map(|value| parse_object_type(&value))
        .transpose()?;
    let stored_id: Option<Uuid> = row.try_get("object_id")?;
    let operation: String = row.try_get("operation")?;
    if db_u64(row.try_get("project_revision")?, "legacy.project_revision")?
        != expected_project_revision
        || actor != expected_actor
        || (operation == "initialize" && (stored_type.is_some() || stored_id.is_some()))
        || (operation != "initialize"
            && (stored_type != Some(object_type) || stored_id != Some(object_id)))
    {
        return Err(ProjectViewV3MigrationError::Invalid(format!(
            "legacy mutation {} does not match object {object_id}",
            hex::encode(source_event_id)
        )));
    }
    let event = live_event_in_tx(tx, community_id, &source_event_id).await?;
    event.verify().map_err(|error| {
        ProjectViewV3MigrationError::Invalid(format!(
            "legacy mutation signature verification failed: {error}"
        ))
    })?;
    let expected_tags = [
        vec!["-".to_owned()],
        vec!["t".to_owned(), "buzz-project-view-mutation".to_owned()],
    ];
    let actual_tags = event
        .tags
        .iter()
        .map(nostr::Tag::as_slice)
        .collect::<Vec<_>>();
    if event.id.to_bytes() != source_event_id
        || event.pubkey != actor
        || u32::from(event.kind.as_u16()) != KIND_PROJECT_VIEW_MUTATION
        || actual_tags.len() != expected_tags.len()
        || actual_tags
            .iter()
            .zip(&expected_tags)
            .any(|(actual, expected)| *actual != expected.as_slice())
    {
        return Err(ProjectViewV3MigrationError::Invalid(
            "legacy mutation event envelope is not canonical".to_owned(),
        ));
    }
    let mutation = Mutation::from_json(&event.content).map_err(|error| {
        ProjectViewV3MigrationError::Invalid(format!(
            "legacy mutation cannot be parsed by the frozen schema: {error}"
        ))
    })?;
    if !mutation_targets_object(&mutation, community_id, object_id, object_type) {
        return Err(ProjectViewV3MigrationError::Invalid(format!(
            "legacy mutation does not contain object {object_id}"
        )));
    }
    Ok(StoredOrigin {
        source_type: "nostr_event".to_owned(),
        change_id: source_event_id,
        event_id: Some(source_event_id),
        actor: Some(actor),
        audit_seq: None,
        legacy_mutation: true,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn load_object_origin_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    source_type: &str,
    source_change_id: [u8; 32],
    source_event_id: Option<[u8; 32]>,
    object_id: Uuid,
    object_type: ProjectViewObjectType,
    project_revision: u64,
    updated_by: PublicKey,
) -> ProjectViewV3MigrationResult<StoredOrigin> {
    let typed_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM project_view_changes \
         WHERE community_id = $1 AND change_id = $2)",
    )
    .bind(community_id.as_uuid())
    .bind(source_change_id.as_slice())
    .fetch_one(&mut **tx)
    .await?;
    if typed_exists {
        let origin =
            load_change_origin_in_tx(tx, community_id, source_change_id, project_revision).await?;
        if origin.source_type != source_type
            || origin.event_id != source_event_id
            || (origin.source_type == "nostr_event" && origin.actor != Some(updated_by))
        {
            return Err(ProjectViewV3MigrationError::Invalid(format!(
                "typed source provenance differs from object {object_id}"
            )));
        }
        return Ok(origin);
    }
    if source_type != "nostr_event" || source_event_id != Some(source_change_id) {
        return Err(ProjectViewV3MigrationError::Invalid(format!(
            "object {object_id} has no valid typed or legacy source"
        )));
    }
    load_legacy_mutation_origin_in_tx(
        tx,
        community_id,
        source_change_id,
        object_id,
        object_type,
        project_revision,
        updated_by,
    )
    .await
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn load_cutover_objects_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    manifest: &ResourceMappingManifestV1,
    cutover_project_revision: u64,
    cutover_time: DateTime<Utc>,
    operator_origin: &StoredOrigin,
) -> ProjectViewV3MigrationResult<Vec<CutoverObject>> {
    let reviewed = manifest
        .entries
        .iter()
        .map(|entry| (Uuid::from_bytes(entry.resource_id), entry))
        .collect::<BTreeMap<_, _>>();
    let rows = sqlx::query(
        "SELECT object_id, object_type, object_revision, project_revision, body, \
                under_goal_id, under_plan_id, planned_in_stage_id, \
                about_object_id, about_object_type, handles_object_id, \
                handles_object_type, created_at, updated_at, created_by, updated_by, \
                deleted_at, role_level, responsible_role_id, source_type, \
                source_change_id, source_event_id, projection_event_id \
         FROM project_view_objects \
         WHERE community_id = $1 AND schema_version = 2 \
         ORDER BY object_id FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .fetch_all(&mut **tx)
    .await?;
    let mut objects = Vec::with_capacity(rows.len());
    let mut seen_resources = BTreeSet::new();
    for row in rows {
        let object_id: Uuid = row.try_get("object_id")?;
        parse_object_type(&row.try_get::<String, _>("object_type")?)?;
        let old_projection_event_id = bytes32(
            row.try_get("projection_event_id")?,
            "object.projection_event_id",
        )?;
        let source_type: String = row.try_get("source_type")?;
        let source_change_id = bytes32(row.try_get("source_change_id")?, "source_change_id")?;
        let source_event_id = row
            .try_get::<Option<Vec<u8>>, _>("source_event_id")?
            .map(|value| bytes32(value, "source_event_id"))
            .transpose()?;
        let role_level = row
            .try_get::<Option<String>, _>("role_level")?
            .map(|value| parse_role_level(&value))
            .transpose()?;
        let responsible_role_id: Option<Uuid> = row.try_get("responsible_role_id")?;
        let legacy = crate::project_view::entry_from_row(row).map_err(|error| {
            ProjectViewV3MigrationError::Invalid(format!("load legacy object {object_id}: {error}"))
        })?;
        let project_revision = legacy.project_revision();
        let updated_by = match &legacy {
            ProjectViewEntry::Active(object) => object.updated_by,
            ProjectViewEntry::Tombstone(tombstone) => tombstone.deleted_by,
        };
        let (entry, origin) = match legacy {
            ProjectViewEntry::Active(object)
                if object.object_type == ProjectViewObjectType::Resource =>
            {
                let mapping = reviewed.get(&object.id).ok_or_else(|| {
                    ProjectViewV3MigrationError::Conflict(format!(
                        "Resource {} has no reviewed manifest entry",
                        object.id
                    ))
                })?;
                seen_resources.insert(object.id);
                let reviewer = public_key(&mapping.reviewed_by_pubkey, "reviewed_by_pubkey")?;
                let object_revision = object
                    .object_revision
                    .checked_add(1)
                    .filter(|value| *value <= MAX_SAFE_REVISION)
                    .ok_or_else(|| {
                        ProjectViewV3MigrationError::Unavailable(format!(
                            "Resource {} object revision overflow",
                            object.id
                        ))
                    })?;
                let resource = &mapping.reviewed_v3_payload.resource_data;
                let data = ProjectViewObjectDataV3::Resource(ProjectResourceV3 {
                    name: resource.name.clone(),
                    resource_kind: resource.resource_kind.clone(),
                    summary: resource.summary.clone(),
                    guide_document_id: Uuid::from_bytes(resource.guide_document_id),
                });
                (
                    ProjectViewEntryV3::Active(Box::new(ProjectViewObjectV3 {
                        id: object.id,
                        object_type: object.object_type,
                        object_revision,
                        project_revision: cutover_project_revision,
                        created_at: object.created_at,
                        updated_at: cutover_time,
                        created_by: object.created_by,
                        updated_by: reviewer,
                        data,
                        relations: object.relations,
                        context_references: Vec::new(),
                    })),
                    operator_origin.clone(),
                )
            }
            ProjectViewEntry::Active(object) => {
                let origin = load_object_origin_in_tx(
                    tx,
                    community_id,
                    &source_type,
                    source_change_id,
                    source_event_id,
                    object.id,
                    object.object_type,
                    object.project_revision,
                    object.updated_by,
                )
                .await?;
                (
                    ProjectViewEntryV3::Active(Box::new(ProjectViewObjectV3 {
                        id: object.id,
                        object_type: object.object_type,
                        object_revision: object.object_revision,
                        project_revision: object.project_revision,
                        created_at: object.created_at,
                        updated_at: object.updated_at,
                        created_by: object.created_by,
                        updated_by: object.updated_by,
                        data: legacy_data_to_v3(object.data)?,
                        relations: object.relations,
                        context_references: Vec::new(),
                    })),
                    origin,
                )
            }
            ProjectViewEntry::Tombstone(tombstone) => {
                let origin = load_object_origin_in_tx(
                    tx,
                    community_id,
                    &source_type,
                    source_change_id,
                    source_event_id,
                    tombstone.id,
                    tombstone.object_type,
                    tombstone.project_revision,
                    tombstone.deleted_by,
                )
                .await?;
                (
                    ProjectViewEntryV3::Tombstone(ProjectViewTombstoneV3 {
                        id: tombstone.id,
                        object_type: tombstone.object_type,
                        object_revision: tombstone.object_revision,
                        project_revision: tombstone.project_revision,
                        created_at: tombstone.created_at,
                        deleted_at: tombstone.deleted_at,
                        created_by: tombstone.created_by,
                        deleted_by: tombstone.deleted_by,
                    }),
                    origin,
                )
            }
        };
        if entry.object_type() == ProjectViewObjectType::Role && role_level.is_none() {
            return Err(ProjectViewV3MigrationError::Invalid(format!(
                "Role {object_id} is missing its governance level"
            )));
        }
        if entry.object_type() != ProjectViewObjectType::Role && role_level.is_some() {
            return Err(ProjectViewV3MigrationError::Invalid(format!(
                "non-Role object {object_id} carries a Role level"
            )));
        }
        if responsible_role_id.is_some()
            && !(entry.object_type() == ProjectViewObjectType::Work
                && matches!(entry, ProjectViewEntryV3::Active(_)))
        {
            return Err(ProjectViewV3MigrationError::Invalid(format!(
                "object {object_id} has an invalid responsible Role pointer"
            )));
        }
        if origin.source_type == "nostr_event" && origin.actor != Some(updated_by) {
            return Err(ProjectViewV3MigrationError::Invalid(format!(
                "object {object_id} business actor differs from its Nostr source"
            )));
        }
        if entry.project_revision() != project_revision
            && entry.object_type() != ProjectViewObjectType::Resource
        {
            return Err(ProjectViewV3MigrationError::Invalid(format!(
                "object {object_id} unexpectedly changed revision during conversion"
            )));
        }
        objects.push(CutoverObject {
            entry,
            role_level,
            responsible_role_id,
            old_projection_event_id,
            origin,
            provenance_id: Uuid::new_v4(),
        });
    }
    if seen_resources.len() != reviewed.len() {
        return Err(ProjectViewV3MigrationError::Conflict(
            "manifest Resource set is not exactly represented by canonical objects".to_owned(),
        ));
    }
    Ok(objects)
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn load_current_entities_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    schema_version: i16,
) -> ProjectViewV3MigrationResult<Vec<CutoverEntity>> {
    let continuity =
        crate::project_view_v2::load_continuity_state(tx, community_id, schema_version)
            .await
            .map_err(|error| ProjectViewV3MigrationError::Invalid(error.to_string()))?;
    let rows = sqlx::query(
        "WITH current_entities AS ( \
             SELECT 1::smallint AS sort_order, 'role_assignment_proposal'::text AS entity_type, \
                    proposal_id AS entity_id, projection_event_id, last_change_id, updated_at \
             FROM project_role_assignment_proposals p \
             WHERE p.community_id = $1 \
               AND (p.status = 'open' OR EXISTS ( \
                    SELECT 1 FROM project_role_assignments a \
                    WHERE a.community_id = p.community_id \
                      AND a.proposal_id = p.proposal_id AND a.ended_at IS NULL)) \
             UNION ALL \
             SELECT 2::smallint, 'role_assignment', assignment_id, projection_event_id, \
                    last_change_id, updated_at \
             FROM project_role_assignments \
             WHERE community_id = $1 AND ended_at IS NULL \
             UNION ALL \
             SELECT 3::smallint, 'work_commitment', commitment_id, projection_event_id, \
                    last_change_id, updated_at \
             FROM project_work_commitments \
             WHERE community_id = $1 AND ended_at IS NULL \
             UNION ALL \
             SELECT 4::smallint, 'role_checkpoint', checkpoint_id, projection_event_id, \
                    last_change_id, created_at \
             FROM ( \
                 SELECT checkpoint.*, row_number() OVER ( \
                     PARTITION BY checkpoint.role_id \
                     ORDER BY checkpoint.project_revision DESC, checkpoint.checkpoint_id DESC \
                 ) AS history_rank \
                 FROM project_role_checkpoints checkpoint \
                 JOIN project_view_objects role \
                   ON role.community_id = checkpoint.community_id \
                  AND role.object_id = checkpoint.role_id \
                  AND role.object_type = 'role' AND role.deleted_at IS NULL \
                 WHERE checkpoint.community_id = $1 \
             ) current_checkpoint WHERE history_rank = 1 \
             UNION ALL \
             SELECT 5::smallint, 'role_handoff', handoff_id, projection_event_id, \
                    last_change_id, created_at \
             FROM ( \
                 SELECT handoff.*, row_number() OVER ( \
                     PARTITION BY handoff.role_id \
                     ORDER BY handoff.project_revision DESC, handoff.handoff_id DESC \
                 ) AS history_rank \
                 FROM project_role_handoffs handoff \
                 JOIN project_view_objects role \
                   ON role.community_id = handoff.community_id \
                  AND role.object_id = handoff.role_id \
                  AND role.object_type = 'role' AND role.deleted_at IS NULL \
                 WHERE handoff.community_id = $1 \
             ) current_handoff WHERE history_rank <= 3 \
         ) \
         SELECT sort_order, entity_type, entity_id, projection_event_id, \
                last_change_id, updated_at \
         FROM current_entities ORDER BY sort_order, entity_id",
    )
    .bind(community_id.as_uuid())
    .fetch_all(&mut **tx)
    .await?;
    let mut entities = Vec::with_capacity(rows.len());
    for row in rows {
        let entity_type: String = row.try_get("entity_type")?;
        let entity_id: Uuid = row.try_get("entity_id")?;
        let entity = match entity_type.as_str() {
            "role_assignment_proposal" => V3EntityChange::Proposal(
                continuity
                    .state
                    .proposals()
                    .find(|value| value.proposal_id == entity_id)
                    .cloned()
                    .ok_or_else(|| {
                        ProjectViewV3MigrationError::Invalid(format!(
                            "current Proposal {entity_id} is absent from canonical state"
                        ))
                    })?,
            ),
            "role_assignment" => V3EntityChange::Assignment(
                continuity
                    .state
                    .assignments()
                    .find(|value| value.assignment_id == entity_id)
                    .cloned()
                    .ok_or_else(|| {
                        ProjectViewV3MigrationError::Invalid(format!(
                            "current Assignment {entity_id} is absent from canonical state"
                        ))
                    })?,
            ),
            "work_commitment" => V3EntityChange::Commitment(
                continuity
                    .state
                    .commitments()
                    .find(|value| value.commitment_id == entity_id)
                    .cloned()
                    .ok_or_else(|| {
                        ProjectViewV3MigrationError::Invalid(format!(
                            "current Commitment {entity_id} is absent from canonical state"
                        ))
                    })?,
            ),
            "role_checkpoint" => V3EntityChange::Checkpoint(
                continuity
                    .state
                    .checkpoints()
                    .find(|value| value.checkpoint_id == entity_id)
                    .cloned()
                    .ok_or_else(|| {
                        ProjectViewV3MigrationError::Invalid(format!(
                            "current Checkpoint {entity_id} is absent from canonical state"
                        ))
                    })?,
            ),
            "role_handoff" => V3EntityChange::Handoff(
                continuity
                    .state
                    .handoffs()
                    .find(|value| value.handoff_id == entity_id)
                    .cloned()
                    .ok_or_else(|| {
                        ProjectViewV3MigrationError::Invalid(format!(
                            "current Handoff {entity_id} is absent from canonical state"
                        ))
                    })?,
            ),
            _ => {
                return Err(ProjectViewV3MigrationError::Invalid(format!(
                    "unsupported current continuity entity {entity_type}"
                )));
            }
        };
        let old_projection_event_id = bytes32(
            row.try_get::<Option<Vec<u8>>, _>("projection_event_id")?
                .ok_or_else(|| {
                    ProjectViewV3MigrationError::Invalid(format!(
                        "current {entity_type} {entity_id} has no projection pointer"
                    ))
                })?,
            "entity.projection_event_id",
        )?;
        let change_id = bytes32(row.try_get("last_change_id")?, "entity.last_change_id")?;
        let origin = load_change_origin_in_tx(
            tx,
            community_id,
            change_id,
            entity_project_revision(&entity),
        )
        .await?;
        entities.push(CutoverEntity {
            entity,
            old_projection_event_id,
            origin,
            updated_at: row.try_get("updated_at")?,
        });
    }
    Ok(entities)
}

const fn entity_project_revision(entity: &V3EntityChange) -> u64 {
    match entity {
        V3EntityChange::Role(value) => value.project_revision,
        V3EntityChange::Proposal(value) => value.project_revision,
        V3EntityChange::Assignment(value) => value.project_revision,
        V3EntityChange::Commitment(value) => value.project_revision,
        V3EntityChange::Checkpoint(value) => value.project_revision,
        V3EntityChange::Handoff(value) => value.project_revision,
    }
}

fn entry_updated_at(entry: &ProjectViewEntryV3) -> DateTime<Utc> {
    match entry {
        ProjectViewEntryV3::Active(object) => object.updated_at,
        ProjectViewEntryV3::Tombstone(tombstone) => tombstone.deleted_at,
    }
}

fn projected_object_matches(
    projected: &buzz_sdk::project_view_v3::V3ProjectedObject,
    entry: &ProjectViewEntryV3,
) -> bool {
    match (projected, entry) {
        (
            buzz_sdk::project_view_v3::V3ProjectedObject::Active(projected),
            ProjectViewEntryV3::Active(entry),
        ) => projected.as_ref() == entry.as_ref(),
        (
            buzz_sdk::project_view_v3::V3ProjectedObject::Tombstone(projected),
            ProjectViewEntryV3::Tombstone(entry),
        ) => projected == entry,
        _ => false,
    }
}

fn sign_cutover_objects(
    community_id: CommunityId,
    projection_generation: u64,
    objects: &[CutoverObject],
    relay_keys: &Keys,
) -> ProjectViewV3MigrationResult<Vec<(Uuid, nostr::Event)>> {
    let mut signed = Vec::with_capacity(objects.len());
    for object in objects {
        let context = V3ProjectionContext {
            project_id: community_id,
            projection_generation,
            project_revision: object.entry.project_revision(),
            source: object.origin.projection_source()?,
            updated_at: entry_updated_at(&object.entry),
        };
        let event = match &object.entry {
            ProjectViewEntryV3::Active(value)
                if value.object_type == ProjectViewObjectType::Role =>
            {
                let role = value
                    .role_definition(object.role_level.ok_or_else(|| {
                        ProjectViewV3MigrationError::Invalid(format!(
                            "Role {} is missing its level",
                            value.id
                        ))
                    })?)
                    .map_err(|error| ProjectViewV3MigrationError::Invalid(error.to_string()))?;
                let event = buzz_sdk::project_view_v3::build_entity_projection(
                    &context,
                    &V3EntityChange::Role(role.clone()),
                )
                .map_err(|error| ProjectViewV3MigrationError::Invalid(error.to_string()))?
                .sign_with_keys(relay_keys)
                .map_err(|error| {
                    ProjectViewV3MigrationError::Invalid(format!(
                        "sign v3 Role projection: {error}"
                    ))
                })?;
                let parsed = buzz_sdk::project_view_v3::parse_entity_projection(
                    &event,
                    &relay_keys.public_key(),
                    community_id,
                )
                .map_err(|error| ProjectViewV3MigrationError::Invalid(error.to_string()))?;
                if parsed.projection_generation != projection_generation
                    || parsed.project_revision != context.project_revision
                    || parsed.source != context.source
                    || parsed.updated_at != context.updated_at
                    || parsed.entity != V3EntityChange::Role(role)
                {
                    return Err(ProjectViewV3MigrationError::Invalid(format!(
                        "signed Role {} differs from canonical cutover state",
                        value.id
                    )));
                }
                event
            }
            ProjectViewEntryV3::Active(_) | ProjectViewEntryV3::Tombstone(_) => {
                let event = buzz_sdk::project_view_v3::build_project_object_projection(
                    &context,
                    &object.entry,
                    object.responsible_role_id,
                )
                .map_err(|error| ProjectViewV3MigrationError::Invalid(error.to_string()))?
                .sign_with_keys(relay_keys)
                .map_err(|error| {
                    ProjectViewV3MigrationError::Invalid(format!(
                        "sign v3 object projection: {error}"
                    ))
                })?;
                let parsed = buzz_sdk::project_view_v3::parse_project_object_projection(
                    &event,
                    &relay_keys.public_key(),
                    community_id,
                )
                .map_err(|error| ProjectViewV3MigrationError::Invalid(error.to_string()))?;
                if parsed.projection_generation != projection_generation
                    || parsed.project_revision != context.project_revision
                    || parsed.source != context.source
                    || parsed.updated_at != context.updated_at
                    || parsed.responsible_role_id != object.responsible_role_id
                    || !projected_object_matches(&parsed.object, &object.entry)
                {
                    return Err(ProjectViewV3MigrationError::Invalid(format!(
                        "signed object {} differs from canonical cutover state",
                        object.entry.id()
                    )));
                }
                event
            }
        };
        signed.push((object.entry.id(), event));
    }
    Ok(signed)
}

fn sign_cutover_entities(
    community_id: CommunityId,
    projection_generation: u64,
    entities: &[CutoverEntity],
    relay_keys: &Keys,
) -> ProjectViewV3MigrationResult<Vec<(RoleContinuityEntity, Uuid, nostr::Event)>> {
    let mut signed = Vec::with_capacity(entities.len());
    for entity in entities {
        let context = V3ProjectionContext {
            project_id: community_id,
            projection_generation,
            project_revision: entity_project_revision(&entity.entity),
            source: entity.origin.projection_source()?,
            updated_at: entity.updated_at,
        };
        let event = buzz_sdk::project_view_v3::build_entity_projection(&context, &entity.entity)
            .map_err(|error| ProjectViewV3MigrationError::Invalid(error.to_string()))?
            .sign_with_keys(relay_keys)
            .map_err(|error| {
                ProjectViewV3MigrationError::Invalid(format!(
                    "sign v3 continuity projection: {error}"
                ))
            })?;
        let parsed = buzz_sdk::project_view_v3::parse_entity_projection(
            &event,
            &relay_keys.public_key(),
            community_id,
        )
        .map_err(|error| ProjectViewV3MigrationError::Invalid(error.to_string()))?;
        if parsed.projection_generation != projection_generation
            || parsed.project_revision != context.project_revision
            || parsed.source != context.source
            || parsed.updated_at != context.updated_at
            || parsed.entity != entity.entity
        {
            return Err(ProjectViewV3MigrationError::Invalid(format!(
                "signed continuity entity {} differs from canonical cutover state",
                entity.entity.entity_id()
            )));
        }
        signed.push((
            entity.entity.entity_type(),
            entity.entity.entity_id(),
            event,
        ));
    }
    Ok(signed)
}

async fn insert_committed_entries_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    cutover_change_id: &[u8; 32],
    manifest: &ResourceMappingManifestV1,
    committed_at: DateTime<Utc>,
) -> ProjectViewV3MigrationResult<()> {
    for entry in &manifest.entries {
        sqlx::query(
            "INSERT INTO project_view_v3_committed_resource_entries \
                (community_id, cutover_change_id, resource_id, guide_document_id, \
                 legacy_object_revision, legacy_projection_event_id, legacy_body_digest, \
                 mapping_entry_digest, reviewed_v3_payload, v3_payload_digest, \
                 guide_document_revision, guide_head_event_id, guide_revision_event_id, \
                 guide_content_digest, reviewed_by_pubkey, reviewed_at_unix_micros, \
                 review_digest, review_signature, committed_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19)",
        )
        .bind(community_id.as_uuid())
        .bind(cutover_change_id.as_slice())
        .bind(Uuid::from_bytes(entry.resource_id))
        .bind(Uuid::from_bytes(
            entry.reviewed_v3_payload.resource_data.guide_document_id,
        ))
        .bind(revision_i64(
            entry.legacy_object_revision,
            "legacy_object_revision",
        )?)
        .bind(entry.legacy_projection_event_id.as_slice())
        .bind(entry.legacy_body_digest.as_slice())
        .bind(entry.mapping_entry_digest.as_slice())
        .bind(reviewed_payload_json(entry))
        .bind(entry.v3_payload_digest.as_slice())
        .bind(revision_i64(
            entry.guide_document_revision,
            "guide_document_revision",
        )?)
        .bind(entry.guide_head_event_id.as_slice())
        .bind(entry.guide_revision_event_id.as_slice())
        .bind(entry.guide_content_digest.as_slice())
        .bind(entry.reviewed_by_pubkey.as_slice())
        .bind(entry.reviewed_at_unix_micros)
        .bind(entry.review_digest.as_slice())
        .bind(entry.review_signature.as_bytes().as_slice())
        .bind(committed_at)
        .execute(&mut **tx)
        .await?;
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM project_view_v3_committed_resource_entries \
         WHERE community_id = $1 AND cutover_change_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(cutover_change_id.as_slice())
    .fetch_one(&mut **tx)
    .await?;
    if usize::try_from(count).ok() != Some(manifest.entries.len()) {
        return Err(ProjectViewV3MigrationError::Invalid(
            "immutable committed Resource set is incomplete".to_owned(),
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn persist_cutover_objects_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    cutover_change_id: &[u8; 32],
    objects: &[CutoverObject],
    signed: &[(Uuid, nostr::Event)],
) -> ProjectViewV3MigrationResult<()> {
    let events = signed
        .iter()
        .map(|(object_id, event)| (*object_id, event))
        .collect::<BTreeMap<_, _>>();
    if events.len() != objects.len() {
        return Err(ProjectViewV3MigrationError::Invalid(
            "signed cutover object set is not exact".to_owned(),
        ));
    }
    let mut retired = BTreeSet::new();
    for object in objects {
        let event = events.get(&object.entry.id()).ok_or_else(|| {
            ProjectViewV3MigrationError::Invalid(format!(
                "missing signed head for object {}",
                object.entry.id()
            ))
        })?;
        if !retired.insert(object.old_projection_event_id) {
            return Err(ProjectViewV3MigrationError::Invalid(
                "legacy object projection pointers are not unique".to_owned(),
            ));
        }
        if !crate::event::retire_projection_head_in_tx(
            tx,
            community_id,
            &object.old_projection_event_id,
            KIND_PROJECT_VIEW_OBJECT,
        )
        .await?
        {
            return Err(ProjectViewV3MigrationError::Conflict(format!(
                "legacy object projection {} is no longer live",
                hex::encode(object.old_projection_event_id)
            )));
        }

        let source_actor = object.origin.actor.map(PublicKey::to_bytes);
        let legacy_mutation_event_id = object
            .origin
            .legacy_mutation
            .then_some(object.origin.change_id);
        let project_view_change_id =
            (!object.origin.legacy_mutation).then_some(object.origin.change_id);
        sqlx::query(
            "INSERT INTO project_view_object_provenance \
                (community_id, provenance_id, object_id, object_type, source_type, \
                 source_change_id, source_event_id, source_project_revision, \
                 source_actor_pubkey, legacy_mutation_event_id, project_view_change_id) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
        )
        .bind(community_id.as_uuid())
        .bind(object.provenance_id)
        .bind(object.entry.id())
        .bind(object.entry.object_type().as_str())
        .bind(&object.origin.source_type)
        .bind(object.origin.change_id.as_slice())
        .bind(object.origin.event_id.as_ref().map(<[u8; 32]>::as_slice))
        .bind(revision_i64(
            object.entry.project_revision(),
            "source_project_revision",
        )?)
        .bind(source_actor.as_ref().map(<[u8; 32]>::as_slice))
        .bind(legacy_mutation_event_id.as_ref().map(<[u8; 32]>::as_slice))
        .bind(project_view_change_id.as_ref().map(<[u8; 32]>::as_slice))
        .execute(&mut **tx)
        .await?;

        let (
            body,
            relations,
            created_at,
            updated_at,
            created_by,
            updated_by,
            deleted_at,
            guide_document_id,
        ) = match &object.entry {
            ProjectViewEntryV3::Active(entry) => {
                let mut body =
                    crate::project_view_v3::v3_object_body(&entry.data, &entry.context_references)
                        .map_err(|error| ProjectViewV3MigrationError::Invalid(error.to_string()))?;
                if entry.object_type == ProjectViewObjectType::Role {
                    let level = object.role_level.ok_or_else(|| {
                        ProjectViewV3MigrationError::Invalid(format!(
                            "Role {} is missing its stored level",
                            entry.id
                        ))
                    })?;
                    body.as_object_mut()
                        .ok_or_else(|| {
                            ProjectViewV3MigrationError::Invalid(
                                "serialized Role body is not an object".to_owned(),
                            )
                        })?
                        .insert("level".to_owned(), Value::String(level.as_str().to_owned()));
                }
                let guide = match &entry.data {
                    ProjectViewObjectDataV3::Resource(resource) => Some(resource.guide_document_id),
                    _ => None,
                };
                (
                    Some(body),
                    entry.relations,
                    entry.created_at,
                    entry.updated_at,
                    entry.created_by,
                    entry.updated_by,
                    None,
                    guide,
                )
            }
            ProjectViewEntryV3::Tombstone(entry) => (
                None,
                ProjectViewRelations::default(),
                entry.created_at,
                entry.deleted_at,
                entry.created_by,
                entry.deleted_by,
                Some(entry.deleted_at),
                None,
            ),
        };
        let created_by = created_by.to_bytes();
        let updated_by = updated_by.to_bytes();
        let about_id = relations.about.map(|reference| reference.object_id);
        let about_type = relations
            .about
            .map(|reference| reference.object_type.as_str());
        let handles_id = relations.handles.map(|reference| reference.object_id);
        let handles_type = relations
            .handles
            .map(|reference| reference.object_type.as_str());
        let updated = sqlx::query(
            "UPDATE project_view_objects SET \
                 schema_version = 3, object_revision = $3, project_revision = $4, \
                 body = $5, under_goal_id = $6, under_plan_id = $7, \
                 planned_in_stage_id = $8, about_object_id = $9, about_object_type = $10, \
                 handles_object_id = $11, handles_object_type = $12, created_at = $13, \
                 updated_at = $14, created_by = $15, updated_by = $16, \
                 source_event_id = $17, projection_event_id = $18, deleted_at = $19, \
                 role_level = $20, responsible_role_id = $21, guide_document_id = $22, \
                 source_type = $23, source_change_id = $24, source_provenance_id = $25 \
             WHERE community_id = $1 AND object_id = $2 AND schema_version = 2 \
               AND projection_event_id = $26",
        )
        .bind(community_id.as_uuid())
        .bind(object.entry.id())
        .bind(revision_i64(
            object.entry.object_revision(),
            "object_revision",
        )?)
        .bind(revision_i64(
            object.entry.project_revision(),
            "project_revision",
        )?)
        .bind(body)
        .bind(relations.under_goal_id)
        .bind(relations.under_plan_id)
        .bind(relations.planned_in_stage_id)
        .bind(about_id)
        .bind(about_type)
        .bind(handles_id)
        .bind(handles_type)
        .bind(created_at)
        .bind(updated_at)
        .bind(created_by.as_slice())
        .bind(updated_by.as_slice())
        .bind(object.origin.event_id.as_ref().map(<[u8; 32]>::as_slice))
        .bind(event.id.as_bytes().as_slice())
        .bind(deleted_at)
        .bind(object.role_level.map(RoleLevel::as_str))
        .bind(object.responsible_role_id)
        .bind(guide_document_id)
        .bind(&object.origin.source_type)
        .bind(object.origin.change_id.as_slice())
        .bind(object.provenance_id)
        .bind(object.old_projection_event_id.as_slice())
        .execute(&mut **tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(ProjectViewV3MigrationError::Conflict(format!(
                "legacy object {} changed during cutover",
                object.entry.id()
            )));
        }
        sqlx::query(
            "DELETE FROM project_view_resource_context_references \
             WHERE community_id = $1 AND source_object_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(object.entry.id())
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            "DELETE FROM project_view_document_context_references \
             WHERE community_id = $1 AND source_object_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(object.entry.id())
        .execute(&mut **tx)
        .await?;
    }

    let resource_count: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM project_view_objects \
         WHERE community_id = $1 AND schema_version = 3 \
           AND object_type = 'resource' AND deleted_at IS NULL \
           AND source_change_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(cutover_change_id.as_slice())
    .fetch_one(&mut **tx)
    .await?;
    let expected_resources = objects
        .iter()
        .filter(|object| {
            object.entry.object_type() == ProjectViewObjectType::Resource
                && matches!(object.entry, ProjectViewEntryV3::Active(_))
        })
        .count();
    if usize::try_from(resource_count).ok() != Some(expected_resources) {
        return Err(ProjectViewV3MigrationError::Invalid(
            "converted Resource source linkage is incomplete".to_owned(),
        ));
    }
    Ok(())
}

async fn persist_cutover_entity_pointers_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    entities: &[CutoverEntity],
    signed: &[(RoleContinuityEntity, Uuid, nostr::Event)],
) -> ProjectViewV3MigrationResult<()> {
    let events = signed
        .iter()
        .map(|(entity_type, entity_id, event)| ((*entity_type, *entity_id), event))
        .collect::<BTreeMap<_, _>>();
    if events.len() != entities.len() {
        return Err(ProjectViewV3MigrationError::Invalid(
            "signed cutover continuity set is not exact".to_owned(),
        ));
    }
    let mut retired = BTreeSet::new();
    for entity in entities {
        let key = (entity.entity.entity_type(), entity.entity.entity_id());
        let event = events.get(&key).ok_or_else(|| {
            ProjectViewV3MigrationError::Invalid(format!(
                "missing signed continuity head for {}",
                entity.entity.entity_id()
            ))
        })?;
        if !retired.insert(entity.old_projection_event_id) {
            return Err(ProjectViewV3MigrationError::Invalid(
                "legacy continuity projection pointers are not unique".to_owned(),
            ));
        }
        if !crate::event::retire_projection_head_in_tx(
            tx,
            community_id,
            &entity.old_projection_event_id,
            KIND_PROJECT_VIEW_OBJECT,
        )
        .await?
        {
            return Err(ProjectViewV3MigrationError::Conflict(format!(
                "legacy continuity projection {} is no longer live",
                hex::encode(entity.old_projection_event_id)
            )));
        }
        crate::project_view_v2::update_projection_pointer(
            tx,
            community_id,
            key.0,
            key.1,
            event.id.as_bytes(),
        )
        .await
        .map_err(|error| ProjectViewV3MigrationError::Invalid(error.to_string()))?;
    }
    Ok(())
}

async fn cutover_project_view_v3_impl(
    db: &Db,
    community_id: CommunityId,
    maintenance_epoch: u64,
    requested_by: PublicKey,
    idempotency_key: &str,
    manifest: &ResourceMappingManifestV1,
    relay_keys: &Keys,
) -> ProjectViewV3MigrationResult<ProjectViewV3CutoverOutcome> {
    manifest.validate()?;
    if maintenance_epoch == 0 || maintenance_epoch > MAX_SAFE_REVISION {
        return Err(ProjectViewV3MigrationError::Invalid(
            "maintenance_epoch must be JavaScript-safe and positive".to_owned(),
        ));
    }
    let epoch = revision_i64(maintenance_epoch, "maintenance_epoch")?;
    let idempotency_key_hash = cutover_idempotency_hash(idempotency_key)?;
    let canonical_manifest_digest = manifest_digest(manifest)?;
    let request_hash =
        cutover_request_hash(community_id, maintenance_epoch, &canonical_manifest_digest);
    let mut tx = db.pool.begin().await?;
    crate::community_lock::acquire(&mut tx, community_id, false).await?;
    require_operator_in_tx(&mut tx, community_id, requested_by).await?;

    if let Some(row) = sqlx::query(
        "SELECT maintenance_epoch, canonical_request_hash, manifest_digest, \
                target_schema_version, result_receipt \
         FROM project_view_v3_cutovers \
         WHERE community_id = $1 AND idempotency_key_hash = $2",
    )
    .bind(community_id.as_uuid())
    .bind(idempotency_key_hash.as_slice())
    .fetch_optional(&mut *tx)
    .await?
    {
        if row.try_get::<i64, _>("maintenance_epoch")? != epoch
            || bytes32(
                row.try_get("canonical_request_hash")?,
                "canonical_request_hash",
            )? != request_hash
            || bytes32(row.try_get("manifest_digest")?, "manifest_digest")?
                != canonical_manifest_digest
            || row.try_get::<i16, _>("target_schema_version")? != 3
        {
            return Err(ProjectViewV3MigrationError::Conflict(
                "cutover idempotency key was reused for a different request".to_owned(),
            ));
        }
        let result: Value = row.try_get("result_receipt")?;
        let project_revision = json_u64(&result, "project_revision")?;
        let projection_generation = json_u64(&result, "projection_generation")?;
        tx.rollback().await?;
        return Ok(ProjectViewV3CutoverOutcome {
            project_revision,
            projection_generation,
            result,
            events: Vec::new(),
            replayed: true,
        });
    }
    let reused_operation: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM project_view_maintenance_operations \
         WHERE community_id = $1 AND idempotency_key_hash = $2)",
    )
    .bind(community_id.as_uuid())
    .bind(idempotency_key_hash.as_slice())
    .fetch_one(&mut *tx)
    .await?;
    if reused_operation {
        return Err(ProjectViewV3MigrationError::Conflict(
            "cutover idempotency key was already used by another maintenance operation".to_owned(),
        ));
    }

    let pointer = sqlx::query(
        "SELECT community.project_view_schema_version, community.project_view_enabled, \
                community.project_context_enabled, community.project_document_enabled, \
                community.archived_at, maintenance.state, maintenance.current_epoch, \
                epoch.outcome, epoch.base_meta_event_id, epoch.base_project_revision, \
                epoch.base_projection_generation \
         FROM communities community \
         JOIN project_view_maintenance maintenance \
           ON maintenance.community_id = community.id \
         JOIN project_view_maintenance_epochs epoch \
           ON epoch.community_id = community.id AND epoch.maintenance_epoch = $2 \
         WHERE community.id = $1 FOR UPDATE OF community, maintenance, epoch",
    )
    .bind(community_id.as_uuid())
    .bind(epoch)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| {
        ProjectViewV3MigrationError::Conflict("maintenance epoch is missing".to_owned())
    })?;
    if pointer.try_get::<i16, _>("project_view_schema_version")? != 2
        || pointer.try_get::<bool, _>("project_view_enabled")?
        || pointer.try_get::<bool, _>("project_context_enabled")?
        || !pointer.try_get::<bool, _>("project_document_enabled")?
        || pointer
            .try_get::<Option<DateTime<Utc>>, _>("archived_at")?
            .is_some()
        || pointer.try_get::<String, _>("state")? != "frozen"
        || pointer.try_get::<Option<i64>, _>("current_epoch")? != Some(epoch)
        || pointer.try_get::<String, _>("outcome")? != "active"
    {
        return Err(ProjectViewV3MigrationError::Unavailable(
            "fresh cutover requires a non-archived, disabled schema-v2 Community in the exact frozen epoch with Documents enabled"
                .to_owned(),
        ));
    }
    if bytes32(
        pointer.try_get("base_meta_event_id")?,
        "epoch.base_meta_event_id",
    )? != manifest.base_meta_event_id
        || db_u64(
            pointer.try_get("base_project_revision")?,
            "epoch.base_project_revision",
        )? != manifest.base_project_revision
        || db_u64(
            pointer.try_get("base_projection_generation")?,
            "epoch.base_projection_generation",
        )? != manifest.base_projection_generation
    {
        return Err(ProjectViewV3MigrationError::Conflict(
            "manifest does not match the exact maintenance begin base".to_owned(),
        ));
    }
    require_cutover_fence_in_tx(&mut tx, community_id, epoch).await?;
    require_document_projection_ready_in_tx(&mut tx, community_id, &relay_keys.public_key())
        .await?;
    let base =
        require_schema_v2_base_in_tx(&mut tx, community_id, &relay_keys.public_key()).await?;
    if base.projection_pubkey != relay_keys.public_key() {
        return Err(ProjectViewV3MigrationError::Unavailable(
            "supplied Relay signer differs from the stable schema-v2 signer".to_owned(),
        ));
    }
    let revalidated_digest =
        validate_manifest_in_tx(&mut tx, community_id, manifest, &base).await?;
    if revalidated_digest != canonical_manifest_digest {
        return Err(ProjectViewV3MigrationError::Invalid(
            "manifest digest changed during canonical validation".to_owned(),
        ));
    }
    require_exact_reviewed_staging_in_tx(
        &mut tx,
        community_id,
        manifest,
        &canonical_manifest_digest,
    )
    .await?;

    let next_revision = base
        .project_revision
        .checked_add(1)
        .filter(|value| *value <= MAX_SAFE_REVISION)
        .ok_or_else(|| {
            ProjectViewV3MigrationError::Unavailable(
                "Project revision overflow during v3 cutover".to_owned(),
            )
        })?;
    let next_generation = base
        .projection_generation
        .checked_add(1)
        .filter(|value| *value <= MAX_SAFE_REVISION)
        .ok_or_else(|| {
            ProjectViewV3MigrationError::Unavailable(
                "projection generation overflow during v3 cutover".to_owned(),
            )
        })?;
    let canonical_time =
        cutover_canonical_time_in_tx(&mut tx, community_id, base.updated_at).await?;
    let requester = requested_by.to_bytes();
    let audit = buzz_audit::append_in_transaction(
        &mut tx,
        NewAuditEntry {
            community_id,
            action: AuditAction::ProjectViewCutover,
            actor_pubkey: Some(requester.to_vec()),
            object_id: Some(community_id.to_string()),
            detail: json!({
                "from_schema_version": 2,
                "to_schema_version": 3,
                "maintenance_epoch": maintenance_epoch,
                "manifest_digest": hex::encode(canonical_manifest_digest),
                "manifest_entry_count": manifest.entries.len(),
                "idempotency_key_hash": hex::encode(idempotency_key_hash),
            }),
        },
    )
    .await?;
    let source = ChangeSource::operator(audit.seq, idempotency_key_hash).map_err(|error| {
        ProjectViewV3MigrationError::Invalid(format!("invalid cutover source: {error}"))
    })?;
    let change_id = source.change_id();
    let change_event_id = event_id(change_id, "cutover_change_id")?;
    let audit_seq = u64::try_from(audit.seq).map_err(|_| {
        ProjectViewV3MigrationError::Invalid("audit sequence must be positive".to_owned())
    })?;
    let operator_origin = StoredOrigin {
        source_type: "operator".to_owned(),
        change_id,
        event_id: None,
        actor: None,
        audit_seq: Some(audit_seq),
        legacy_mutation: false,
    };
    let operator_projection_source = V3ProjectionSource::Operator {
        change_id: change_event_id,
        audit_seq,
    };

    let objects = load_cutover_objects_in_tx(
        &mut tx,
        community_id,
        manifest,
        next_revision,
        canonical_time,
        &operator_origin,
    )
    .await?;
    let role_levels = objects
        .iter()
        .filter_map(|object| object.role_level.map(|level| (object.entry.id(), level)));
    ProjectViewStateV3::from_snapshot(
        community_id,
        next_revision,
        Some(base.initialized_at),
        Some(canonical_time),
        objects.iter().map(|object| object.entry.clone()),
        role_levels,
    )
    .map_err(|error| {
        ProjectViewV3MigrationError::Invalid(format!(
            "converted v3 object snapshot is invalid: {error}"
        ))
    })?;
    let entities = load_current_entities_in_tx(&mut tx, community_id, 2).await?;
    let membership = crate::project_view_v2::load_membership(&mut tx, community_id)
        .await
        .map_err(|error| ProjectViewV3MigrationError::Invalid(error.to_string()))?;
    let membership_event = crate::project_view_v2::build_cutover_membership_event(
        &membership,
        canonical_time,
        relay_keys,
    )
    .map_err(|error| ProjectViewV3MigrationError::Invalid(error.to_string()))?;
    crate::project_view_v2::verify_membership_projection(
        &membership_event,
        relay_keys.public_key(),
        &membership,
        canonical_time,
    )
    .map_err(|error| ProjectViewV3MigrationError::Invalid(error.to_string()))?;

    let signed_objects = sign_cutover_objects(community_id, next_generation, &objects, relay_keys)?;
    let signed_entities =
        sign_cutover_entities(community_id, next_generation, &entities, relay_keys)?;
    let counts = crate::project_view_v2::load_counts(&mut tx, community_id)
        .await
        .map_err(|error| ProjectViewV3MigrationError::Invalid(error.to_string()))?;
    let meta_context = V3ProjectionContext {
        project_id: community_id,
        projection_generation: next_generation,
        project_revision: next_revision,
        source: operator_projection_source,
        updated_at: canonical_time,
    };
    let entity_counts = V3EntityCounts {
        active_objects: counts.active_objects,
        open_proposals: counts.open_proposals,
        active_assignments: counts.active_assignments,
        active_commitments: counts.active_commitments,
        checkpoints: counts.checkpoints,
        handoffs: counts.handoffs,
    };
    let meta_event = buzz_sdk::project_view_v3::build_meta_projection(
        &meta_context,
        entity_counts,
        membership_event.id,
        true,
        &[],
    )
    .map_err(|error| ProjectViewV3MigrationError::Invalid(error.to_string()))?
    .sign_with_keys(relay_keys)
    .map_err(|error| {
        ProjectViewV3MigrationError::Invalid(format!("sign v3 cutover metadata: {error}"))
    })?;
    let verified_meta =
        buzz_sdk::project_view_v3::parse_meta_projection(&meta_event, &relay_keys.public_key())
            .map_err(|error| ProjectViewV3MigrationError::Invalid(error.to_string()))?;
    if verified_meta.project_id != community_id
        || verified_meta.project_revision != next_revision
        || verified_meta.projection_generation != next_generation
        || verified_meta.entity_counts != entity_counts
        || verified_meta.membership_snapshot_event_id != membership_event.id
        || !verified_meta.reset
        || !verified_meta.changed_heads.is_empty()
        || verified_meta.source != meta_context.source
        || verified_meta.updated_at != canonical_time
    {
        return Err(ProjectViewV3MigrationError::Invalid(
            "signed v3 cutover metadata differs from canonical state".to_owned(),
        ));
    }

    let result = json!({
        "operation": "cutover_v3",
        "community_id": community_id.to_string(),
        "maintenance_epoch": maintenance_epoch,
        "change_id": hex::encode(change_id),
        "manifest_digest": hex::encode(canonical_manifest_digest),
        "manifest_entry_count": manifest.entries.len(),
        "base_meta_event_id": hex::encode(base.meta_event_id),
        "base_project_revision": base.project_revision,
        "base_projection_generation": base.projection_generation,
        "project_revision": next_revision,
        "projection_generation": next_generation,
        "meta_projection_event_id": meta_event.id.to_hex(),
        "membership_snapshot_event_id": membership_event.id.to_hex(),
        "state": "frozen",
    });
    let subject = json!({
        "from_schema_version": 2,
        "to_schema_version": 3,
        "maintenance_epoch": maintenance_epoch,
        "manifest_digest": hex::encode(canonical_manifest_digest),
        "manifest_entry_count": manifest.entries.len(),
    });
    sqlx::query(
        "INSERT INTO project_view_changes \
            (community_id, change_id, source_type, source_event_id, \
             source_request_hash, source_audit_seq, idempotency_key_hash, \
             actor_pubkey, acting_assignment_id, operation, subject, \
             project_revision, result, accepted_at) \
         VALUES ($1,$2,'operator',NULL,NULL,$3,$4,NULL,NULL,'cutover_v3',$5,$6,$7,$8)",
    )
    .bind(community_id.as_uuid())
    .bind(change_id.as_slice())
    .bind(audit.seq)
    .bind(idempotency_key_hash.as_slice())
    .bind(subject)
    .bind(revision_i64(next_revision, "project_revision")?)
    .bind(&result)
    .bind(canonical_time)
    .execute(&mut *tx)
    .await?;

    let operation_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO project_view_maintenance_operations \
            (community_id, maintenance_epoch, operation_id, operation, \
             idempotency_key_hash, canonical_request_hash, requested_by, \
             audit_seq, result_receipt, accepted_at) \
         VALUES ($1,$2,$3,'cutover',$4,$5,$6,$7,$8,$9)",
    )
    .bind(community_id.as_uuid())
    .bind(epoch)
    .bind(operation_id)
    .bind(idempotency_key_hash.as_slice())
    .bind(request_hash.as_slice())
    .bind(requester.as_slice())
    .bind(audit.seq)
    .bind(&result)
    .bind(canonical_time)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO project_view_v3_cutovers \
            (community_id, cutover_change_id, maintenance_epoch, \
             idempotency_key_hash, canonical_request_hash, manifest_digest, \
             manifest_entry_count, base_meta_event_id, base_project_revision, \
             base_projection_generation, target_schema_version, result_receipt, accepted_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,3,$11,$12)",
    )
    .bind(community_id.as_uuid())
    .bind(change_id.as_slice())
    .bind(epoch)
    .bind(idempotency_key_hash.as_slice())
    .bind(request_hash.as_slice())
    .bind(canonical_manifest_digest.as_slice())
    .bind(i32::try_from(manifest.entries.len()).map_err(|_| {
        ProjectViewV3MigrationError::Invalid("manifest entry count exceeds INTEGER".to_owned())
    })?)
    .bind(base.meta_event_id.as_slice())
    .bind(revision_i64(
        base.project_revision,
        "base_project_revision",
    )?)
    .bind(revision_i64(
        base.projection_generation,
        "base_projection_generation",
    )?)
    .bind(&result)
    .bind(canonical_time)
    .execute(&mut *tx)
    .await?;
    insert_committed_entries_in_tx(&mut tx, community_id, &change_id, manifest, canonical_time)
        .await?;
    persist_cutover_objects_in_tx(&mut tx, community_id, &change_id, &objects, &signed_objects)
        .await?;
    persist_cutover_entity_pointers_in_tx(&mut tx, community_id, &entities, &signed_entities)
        .await?;

    if !crate::event::retire_projection_head_in_tx(
        &mut tx,
        community_id,
        &base.meta_event_id,
        KIND_PROJECT_VIEW_META,
    )
    .await?
    {
        return Err(ProjectViewV3MigrationError::Conflict(
            "schema-v2 metadata projection is no longer live".to_owned(),
        ));
    }
    crate::project_view_v2::retire_membership_heads(&mut tx, community_id, relay_keys.public_key())
        .await
        .map_err(|error| ProjectViewV3MigrationError::Invalid(error.to_string()))?;
    let (_, membership_inserted) =
        crate::event::insert_event_in_tx(&mut tx, community_id, &membership_event, None).await?;
    if !membership_inserted {
        return Err(ProjectViewV3MigrationError::Conflict(
            "v3 membership projection already exists".to_owned(),
        ));
    }
    for (_, event) in &signed_objects {
        let (_, inserted) =
            crate::event::insert_event_in_tx(&mut tx, community_id, event, None).await?;
        if !inserted {
            return Err(ProjectViewV3MigrationError::Conflict(format!(
                "v3 object projection {} already exists",
                event.id
            )));
        }
    }
    for (_, _, event) in &signed_entities {
        let (_, inserted) =
            crate::event::insert_event_in_tx(&mut tx, community_id, event, None).await?;
        if !inserted {
            return Err(ProjectViewV3MigrationError::Conflict(format!(
                "v3 continuity projection {} already exists",
                event.id
            )));
        }
    }
    let (_, meta_inserted) =
        crate::event::insert_event_in_tx(&mut tx, community_id, &meta_event, None).await?;
    if !meta_inserted {
        return Err(ProjectViewV3MigrationError::Conflict(
            "v3 metadata projection already exists".to_owned(),
        ));
    }

    let relay = relay_keys.public_key().to_bytes();
    let state_updated = sqlx::query(
        "UPDATE project_view_state SET schema_version = 3, project_revision = $2, \
             updated_at = $3, last_event_id = $4, last_actor_pubkey = $5, \
             meta_projection_event_id = $6, projection_pubkey = $7, \
             projection_generation = $8, last_change_id = $4, \
             last_source_event_id = NULL, membership_snapshot_event_id = $9 \
         WHERE community_id = $1 AND schema_version = 2 \
           AND project_revision = $10 AND projection_generation = $11 \
           AND meta_projection_event_id = $12",
    )
    .bind(community_id.as_uuid())
    .bind(revision_i64(next_revision, "project_revision")?)
    .bind(canonical_time)
    .bind(change_id.as_slice())
    .bind(requester.as_slice())
    .bind(meta_event.id.as_bytes().as_slice())
    .bind(relay.as_slice())
    .bind(revision_i64(next_generation, "projection_generation")?)
    .bind(membership_event.id.as_bytes().as_slice())
    .bind(revision_i64(
        base.project_revision,
        "base_project_revision",
    )?)
    .bind(revision_i64(
        base.projection_generation,
        "base_projection_generation",
    )?)
    .bind(base.meta_event_id.as_slice())
    .execute(&mut *tx)
    .await?;
    if state_updated.rows_affected() != 1 {
        return Err(ProjectViewV3MigrationError::Conflict(
            "Project View state changed during cutover".to_owned(),
        ));
    }
    let community_updated = sqlx::query(
        "UPDATE communities SET project_view_schema_version = 3, \
             project_context_enabled = FALSE, project_view_enabled = FALSE \
         WHERE id = $1 AND project_view_schema_version = 2 \
           AND NOT project_view_enabled",
    )
    .bind(community_id.as_uuid())
    .execute(&mut *tx)
    .await?;
    if community_updated.rows_affected() != 1 {
        return Err(ProjectViewV3MigrationError::Conflict(
            "Community schema changed during cutover".to_owned(),
        ));
    }
    let epoch_updated = sqlx::query(
        "UPDATE project_view_maintenance_epochs SET outcome = 'cutover_committed', \
             completed_at = $3, updated_at = $3 \
         WHERE community_id = $1 AND maintenance_epoch = $2 AND outcome = 'active'",
    )
    .bind(community_id.as_uuid())
    .bind(epoch)
    .bind(canonical_time)
    .execute(&mut *tx)
    .await?;
    if epoch_updated.rows_affected() != 1 {
        return Err(ProjectViewV3MigrationError::Conflict(
            "maintenance epoch changed during cutover".to_owned(),
        ));
    }
    let mappings_updated = sqlx::query(
        "UPDATE project_view_v3_resource_mappings SET status = 'consumed', updated_at = $3 \
         WHERE community_id = $1 AND status = 'reviewed' AND manifest_digest = $2",
    )
    .bind(community_id.as_uuid())
    .bind(canonical_manifest_digest.as_slice())
    .bind(canonical_time)
    .execute(&mut *tx)
    .await?;
    if usize::try_from(mappings_updated.rows_affected()).ok() != Some(manifest.entries.len()) {
        return Err(ProjectViewV3MigrationError::Conflict(
            "reviewed staging set changed during cutover".to_owned(),
        ));
    }
    sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    let mut events = vec![membership_event];
    events.extend(signed_objects.into_iter().map(|(_, event)| event));
    events.extend(signed_entities.into_iter().map(|(_, _, event)| event));
    events.push(meta_event);
    Ok(ProjectViewV3CutoverOutcome {
        project_revision: next_revision,
        projection_generation: next_generation,
        result,
        events,
        replayed: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guide_draft_uses_a_fence_longer_than_untrusted_backticks() {
        let resource = ProjectResource {
            name: "Repository".to_owned(),
            resource_type: ResourceType::Repository,
            locator: buzz_project_view::ResourceLocator {
                locator_type: LocatorType::Url,
                value: "https://example.test/```/repo".to_owned(),
            },
            description: "run ``nothing`` automatically".to_owned(),
        };
        let draft = guide_markdown_draft(&resource);
        assert!(draft.contains("````\nhttps://example.test/```/repo\n````"));
        assert!(draft.contains("```\nrun ``nothing`` automatically\n```"));
    }

    #[test]
    fn cutover_request_hash_binds_epoch_and_manifest() {
        let community = CommunityId::from_uuid(
            Uuid::parse_str("018f7797-7b69-7cc4-98a1-40585f5dd2fa").expect("fixture UUID"),
        );
        let first = cutover_request_hash(community, 1, &[1; 32]);
        assert_ne!(first, cutover_request_hash(community, 2, &[1; 32]));
        assert_ne!(first, cutover_request_hash(community, 1, &[2; 32]));
    }
}
