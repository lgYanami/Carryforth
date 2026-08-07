//! Project View protocol adapter.
//!
//! The pure reducer lives in `buzz-project-view`, wire construction lives in
//! `buzz-sdk`, and atomic persistence lives in `buzz-db`. This module owns the
//! Relay-specific security gates, signing, error mapping, and post-commit
//! delivery policy.

use std::{sync::Arc, time::Instant};

use buzz_auth::Scope;
use buzz_core::kind::{
    is_project_view_projection_kind, is_project_view_protocol_kind, KIND_PROJECT_VIEW_META,
    KIND_PROJECT_VIEW_MUTATION, KIND_PROJECT_VIEW_OBJECT,
};
use buzz_core::{PublicKey, StoredEvent, TenantContext};
use buzz_db::project_view::PreparedObjectProjection;
use buzz_db::project_view_v2::{
    PreparedV2EntityProjection, ProjectViewV2WriteError, V2MembershipEntry,
};
use buzz_db::project_view_v3::{
    PreparedV3ProjectObjectCommit, PreparedV3ProjectObjectHead, PreparedV3RoleCommit,
    ProjectViewV3PrepareOutcome, ProjectViewV3ProjectObjectPrepareOutcome, ProjectViewV3WriteError,
};
use buzz_project_view::v2::{RoleContinuityChange, RoleContinuityError};
use buzz_project_view::v3::{
    ProjectObjectCommandV3, ProjectViewInitializeV3, RoleCommandV3, V3ContractError,
    V3ProjectObjectError, V3ReferenceError,
};
use buzz_project_view::{DomainError, MAX_MUTATION_CONTENT_BYTES};
use buzz_sdk::project_view_v3::{
    parse_entity_projection, parse_meta_projection, parse_project_object_projection,
    PROJECT_VIEW_V3_ENTITY_TAG, PROJECT_VIEW_V3_META_TAG, PROJECT_VIEW_V3_OBJECT_TAG,
};
use nostr::{Event, EventBuilder, Filter, Kind, Tag, Timestamp};
use tracing::{info, warn};

use crate::state::AppState;

use super::event::{
    dispatch_persistent_event, dispatch_persistent_event_with_options, PersistentDispatchOptions,
};
use super::ingest::{IngestAuth, IngestError, IngestResult};

const MUTATION_TAG: &str = "buzz-project-view-mutation";

/// Return whether this credential can ever read Community-global Project View
/// events, before the current member/ban lookup.
#[must_use]
pub(crate) fn credential_can_read(scopes: &[Scope], channel_ids: Option<&[uuid::Uuid]>) -> bool {
    super::community_private::credential_can_read_community_private(scopes, channel_ids)
}

/// Return whether a Nostr filter can match at least one Project View kind.
#[must_use]
pub(crate) fn filter_can_match(filter: &Filter) -> bool {
    filter.kinds.as_ref().is_none_or(|kinds| {
        kinds
            .iter()
            .any(|kind| is_project_view_protocol_kind(kind.as_u16() as u32))
    })
}

/// Return whether a filter explicitly targets only Project View kinds.
#[must_use]
pub(crate) fn filter_is_exclusively_project_view(filter: &Filter) -> bool {
    filter.kinds.as_ref().is_some_and(|kinds| {
        !kinds.is_empty()
            && kinds
                .iter()
                .all(|kind| is_project_view_protocol_kind(kind.as_u16() as u32))
    })
}

/// Return whether an explicit Project View filter asks for unsupported search.
#[must_use]
pub(crate) fn filter_is_project_view_search(filter: &Filter) -> bool {
    filter.search.is_some()
        && filter.kinds.as_ref().is_some_and(|kinds| {
            kinds
                .iter()
                .any(|kind| is_project_view_protocol_kind(kind.as_u16() as u32))
        })
}

/// Return whether an ordinary Nostr filter explicitly selects only schema-v3
/// Project View projection coordinates.
///
/// Kindless filters and filters that merely name kind 44301/44302 are not a
/// Project View reader contract: they would also match retained v1/v2
/// migration evidence. A mixed ordinary filter may continue to read its
/// non-Project-View kinds, but Project View projections are silently omitted.
#[must_use]
pub(crate) fn filter_allows_v3_projections(filter: &Filter) -> bool {
    let Some(kinds) = filter.kinds.as_ref() else {
        return false;
    };
    let requests_object = kinds
        .iter()
        .any(|kind| kind.as_u16() as u32 == KIND_PROJECT_VIEW_OBJECT);
    let requests_meta = kinds
        .iter()
        .any(|kind| kind.as_u16() as u32 == KIND_PROJECT_VIEW_META);
    if !requests_object && !requests_meta {
        return false;
    }

    let t = nostr::SingleLetterTag::lowercase(nostr::Alphabet::T);
    let Some(values) = filter.generic_tags.get(&t) else {
        return false;
    };
    if values.is_empty()
        || values.iter().any(|value| {
            !matches!(
                value.as_str(),
                PROJECT_VIEW_V3_OBJECT_TAG | PROJECT_VIEW_V3_ENTITY_TAG | PROJECT_VIEW_V3_META_TAG
            )
        })
    {
        return false;
    }

    (!requests_object
        || values.contains(PROJECT_VIEW_V3_OBJECT_TAG)
        || values.contains(PROJECT_VIEW_V3_ENTITY_TAG))
        && (!requests_meta || values.contains(PROJECT_VIEW_V3_META_TAG))
}

/// Return whether a filter can match a Project View projection but lacks the
/// closed schema-v3 projection tag contract.
#[must_use]
pub(crate) fn filter_has_unscoped_project_view_projection(filter: &Filter) -> bool {
    filter.kinds.as_ref().is_none_or(|kinds| {
        kinds
            .iter()
            .any(|kind| is_project_view_projection_kind(kind.as_u16() as u32))
    }) && !filter_allows_v3_projections(filter)
}

/// Return whether a stored Relay projection is a fully verified schema-v3
/// wire event.
///
/// Metadata retained from v1/v2 shares the historical `buzz-project-view-meta`
/// tag, so tag admission alone is insufficient. Every result chokepoint calls
/// this parser before delivery/counting; the strict SDK parser verifies the
/// signature, exact canonical tags, typed content, and schema version.
#[must_use]
pub(crate) fn event_is_v3_projection(event: &Event, expected_relay_pubkey: &PublicKey) -> bool {
    let kind = event.kind.as_u16() as u32;
    if !is_project_view_projection_kind(kind) {
        return true;
    }
    if kind == KIND_PROJECT_VIEW_META {
        return parse_meta_projection(event, expected_relay_pubkey).is_ok();
    }

    let Ok(content) = serde_json::from_str::<serde_json::Value>(&event.content) else {
        return false;
    };
    let Some(project_id) = content
        .get("project_id")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
        .map(buzz_core::CommunityId::from_uuid)
    else {
        return false;
    };
    match content
        .get("projection_type")
        .and_then(serde_json::Value::as_str)
    {
        Some("object") => {
            parse_project_object_projection(event, expected_relay_pubkey, project_id).is_ok()
        }
        Some("entity") => parse_entity_projection(event, expected_relay_pubkey, project_id).is_ok(),
        _ => false,
    }
}

/// Return the configured stable Relay projection signer.
///
/// `AppState` always owns a keypair, but when no private key was configured it
/// is process-ephemeral and is not a Project View authority. Callers therefore
/// receive `None` and must fail closed for projection delivery.
#[must_use]
pub(crate) fn configured_projection_signer(state: &AppState) -> Option<PublicKey> {
    state
        .config
        .relay_private_key
        .as_ref()
        .map(|_| state.relay_keypair.public_key())
}

/// Return whether an event is admissible for one ordinary Project View
/// projection filter.
///
/// Legacy metadata uses the same kind and `t` coordinate as v3 metadata, so
/// the NIP-01 match alone is not a schema boundary. Every ordinary result and
/// COUNT fallback uses this predicate to fail closed on the strict v3 parser.
#[must_use]
pub(crate) fn projection_event_visible_for_filter(
    filter: &Filter,
    event: &Event,
    expected_relay_pubkey: Option<&PublicKey>,
) -> bool {
    !is_project_view_projection_kind(event.kind.as_u16() as u32)
        || expected_relay_pubkey.is_some_and(|relay_pubkey| {
            filter_allows_v3_projections(filter) && event_is_v3_projection(event, relay_pubkey)
        })
}

/// Return whether COUNT must retain the per-event v3 projection gate.
///
/// `#t` is not SQL-pushable today, but spelling this independently prevents a
/// future tag-index optimization from silently counting retained legacy rows
/// that share the v3 metadata coordinate.
#[must_use]
pub(crate) fn filter_requires_v3_projection_post_filter(filter: &Filter) -> bool {
    filter_allows_v3_projections(filter)
}

/// Apply one signed member mutation and publish its committed projections.
pub(crate) async fn handle_mutation(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: Event,
    auth: &IngestAuth,
) -> Result<IngestResult, IngestError> {
    let started = Instant::now();
    let telemetry = MutationTelemetry::from_content(&event.content);
    let event_id = event.id.to_hex();
    let actor_pubkey = event.pubkey.to_hex();
    let result = handle_mutation_inner(tenant, state, event, auth).await;
    let result_code = mutation_result_code(&result);
    let committed_project_revision = committed_project_revision(&result);

    metrics::counter!(
        "buzz_project_view_mutations_total",
        "operation" => telemetry.operation,
        "result" => result_code
    )
    .increment(1);
    metrics::histogram!(
        "buzz_project_view_mutation_duration_seconds",
        "operation" => telemetry.operation
    )
    .record(started.elapsed().as_secs_f64());
    if matches!(&result, Err(IngestError::Conflict(_))) {
        metrics::counter!(
            "buzz_project_view_conflicts_total",
            "operation" => telemetry.operation
        )
        .increment(1);
    }

    if result.is_ok() {
        info!(
            community_host = %tenant.host(),
            command_event_id = %event_id,
            actor_pubkey = %actor_pubkey,
            operation = telemetry.operation,
            object_type = ?telemetry.object_type,
            object_id = ?telemetry.object_id,
            expected_project_revision = ?telemetry.expected_project_revision,
            committed_project_revision = ?committed_project_revision,
            result_code,
            "Project View mutation completed"
        );
    } else {
        warn!(
            community_host = %tenant.host(),
            command_event_id = %event_id,
            actor_pubkey = %actor_pubkey,
            operation = telemetry.operation,
            object_type = ?telemetry.object_type,
            object_id = ?telemetry.object_id,
            expected_project_revision = ?telemetry.expected_project_revision,
            committed_project_revision = ?committed_project_revision,
            result_code,
            "Project View mutation rejected"
        );
    }

    result
}

async fn handle_mutation_inner(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: Event,
    auth: &IngestAuth,
) -> Result<IngestResult, IngestError> {
    validate_mutation_tags(&event)?;

    if auth.channel_ids().is_some() {
        return Err(IngestError::AuthFailed(
            "restricted: Project View requires a Community-global credential".to_owned(),
        ));
    }
    if !state
        .db
        .project_view_authorized_pubkey(tenant.community(), auth.pubkey().as_bytes())
        .await
        .map_err(|error| {
            IngestError::Internal(format!(
                "error: Project View membership lookup failed: {error}"
            ))
        })?
    {
        return Err(IngestError::AuthFailed(
            "restricted: Project View requires current Community membership".to_owned(),
        ));
    }

    if state.config.relay_private_key.is_none() {
        return Err(IngestError::Unavailable(
            "unavailable:project_view:stable_signer".to_owned(),
        ));
    }
    let relay_pubkey = state.relay_keypair.public_key();
    let schema_version = state
        .db
        .project_view_schema_version(tenant.community())
        .await
        .map_err(|error| {
            IngestError::Internal(format!("error: Project View schema lookup failed: {error}"))
        })?;
    require_project_view_v3_runtime(schema_version)?;
    let status = state
        .db
        .project_view_status_by_host(tenant.host())
        .await
        .map_err(|error| {
            IngestError::Internal(format!("error: Project View status check failed: {error}"))
        })?
        .ok_or_else(|| IngestError::Unavailable("unavailable:project_view:community".to_owned()))?;
    if is_initialize_command(&event.content) {
        let command =
            ProjectViewInitializeV3::from_json(&event.content).map_err(map_v3_contract_error)?;
        return handle_v3_initialize(tenant, state, event, &command).await;
    }
    if !status.enabled {
        return Err(IngestError::Unavailable(
            "unavailable:project_view:disabled".to_owned(),
        ));
    }
    if !state
        .db
        .project_view_v3_advertised_write_ready(tenant.community(), &relay_pubkey)
        .await
        .map_err(|error| {
            IngestError::Internal(format!(
                "error: Project View v3 readiness check failed: {error}"
            ))
        })?
    {
        return Err(IngestError::Unavailable(
            "unavailable:project_view:not_ready".to_owned(),
        ));
    }
    if is_project_object_command(&event.content) {
        handle_v3_project_object_mutation(tenant, state, event).await
    } else {
        handle_v3_role_mutation(tenant, state, event).await
    }
}

fn require_project_view_v3_runtime(schema_version: i16) -> Result<(), IngestError> {
    match schema_version {
        3 => Ok(()),
        1 | 2 => Err(IngestError::Unsupported(
            "unsupported:project_view:migration_required".to_owned(),
        )),
        _ => Err(IngestError::Unsupported(
            "unsupported:project_view:schema".to_owned(),
        )),
    }
}

fn is_project_object_command(content: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|value| {
            value
                .get("request")?
                .get("type")?
                .as_str()
                .map(str::to_owned)
        })
        .is_some_and(|operation| {
            matches!(
                operation.as_str(),
                "initialize" | "create" | "update" | "delete"
            )
        })
}

fn is_initialize_command(content: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|value| {
            value
                .get("request")
                .and_then(|request| request.get("type"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .as_deref()
        == Some("initialize")
}

async fn handle_v3_initialize(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: Event,
    command: &ProjectViewInitializeV3,
) -> Result<IngestResult, IngestError> {
    let outcome = state
        .db
        .initialize_project_view_v3(tenant.community(), &event, command, &state.relay_keypair)
        .await
        .map_err(map_v3_write_error)?;
    let message = response_message(&outcome.result)?;
    dispatch_project_view_committed_events(tenant, state, &outcome.events).await;
    Ok(IngestResult {
        event_id: event.id.to_hex(),
        accepted: true,
        message,
    })
}

async fn handle_v3_project_object_mutation(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: Event,
) -> Result<IngestResult, IngestError> {
    let command = ProjectObjectCommandV3::from_json(&event.content).map_err(map_v3_object_error)?;
    let mut write = state
        .db
        .begin_project_view_v3_write(tenant.community())
        .await
        .map_err(map_v3_write_error)?;
    let prepared = match write
        .prepare_project_object_command(&event, &command)
        .await
        .map_err(map_v3_write_error)?
    {
        ProjectViewV3ProjectObjectPrepareOutcome::Replayed(receipt) => {
            write.rollback().await.map_err(map_v3_write_error)?;
            return Ok(IngestResult {
                event_id: event.id.to_hex(),
                accepted: true,
                message: response_message(&receipt.result)?,
            });
        }
        ProjectViewV3ProjectObjectPrepareOutcome::Prepared(prepared) => prepared,
    };
    if prepared.projection_pubkey != state.relay_keypair.public_key() {
        write.rollback().await.map_err(map_v3_write_error)?;
        return Err(IngestError::Unavailable(
            "unavailable:project_view:signer_rotation".to_owned(),
        ));
    }

    let context = v3_projection_context(&prepared, event.id);
    let mut object_projections = Vec::with_capacity(prepared.heads.len());
    let mut entity_projections = Vec::with_capacity(prepared.entity_changes.len());
    let mut changed_heads =
        Vec::with_capacity(prepared.heads.len() + prepared.entity_changes.len());
    for head in &prepared.heads {
        let (object_id, projection, changed_head) = match head {
            PreparedV3ProjectObjectHead::Role(role) => {
                let entity = buzz_sdk::project_view_v3::V3EntityChange::Role(role.clone());
                let projection =
                    buzz_sdk::project_view_v3::build_entity_projection(&context, &entity)
                        .map_err(v3_projection_build_error)?
                        .sign_with_keys(&state.relay_keypair)
                        .map_err(v3_projection_sign_error)?;
                let changed = buzz_sdk::project_view_v3::changed_head_for_entity(
                    &context,
                    &entity,
                    &projection,
                )
                .map_err(v3_projection_bind_error)?;
                (role.role_id, projection, changed)
            }
            PreparedV3ProjectObjectHead::Object {
                entry,
                responsible_role_id,
            } => {
                let projection = buzz_sdk::project_view_v3::build_project_object_projection(
                    &context,
                    entry,
                    *responsible_role_id,
                )
                .map_err(v3_projection_build_error)?
                .sign_with_keys(&state.relay_keypair)
                .map_err(v3_projection_sign_error)?;
                let changed = buzz_sdk::project_view_v3::changed_head_for_project_object(
                    &context,
                    entry,
                    &projection,
                )
                .map_err(v3_projection_bind_error)?;
                (entry.id(), projection, changed)
            }
        };
        object_projections.push(PreparedObjectProjection::new(object_id, projection));
        changed_heads.push(changed_head);
    }
    for change in &prepared.entity_changes {
        let entity = v3_entity_change(change)?;
        let projection = buzz_sdk::project_view_v3::build_entity_projection(&context, &entity)
            .map_err(v3_projection_build_error)?
            .sign_with_keys(&state.relay_keypair)
            .map_err(v3_projection_sign_error)?;
        changed_heads.push(
            buzz_sdk::project_view_v3::changed_head_for_entity(&context, &entity, &projection)
                .map_err(v3_projection_bind_error)?,
        );
        entity_projections.push(PreparedV2EntityProjection {
            entity_type: change.entity_type(),
            entity_id: change.entity_id(),
            event: projection,
        });
    }
    let meta_projection = buzz_sdk::project_view_v3::build_meta_projection(
        &context,
        v3_counts(prepared.counts),
        prepared.membership_snapshot_event_id,
        false,
        &changed_heads,
    )
    .map_err(v3_projection_build_error)?
    .sign_with_keys(&state.relay_keypair)
    .map_err(v3_projection_sign_error)?;
    let committed = write
        .commit_project_object_command(PreparedV3ProjectObjectCommit {
            command_event: event,
            object_projections,
            entity_projections,
            meta_projection,
        })
        .await
        .map_err(map_v3_write_error)?;
    let message = response_message(&committed.receipt.result)?;
    dispatch_project_view_committed_events(tenant, state, &committed.events).await;
    Ok(IngestResult {
        event_id: nostr::EventId::from_byte_array(committed.receipt.change_id).to_hex(),
        accepted: true,
        message,
    })
}

async fn handle_v3_role_mutation(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: Event,
) -> Result<IngestResult, IngestError> {
    let command = RoleCommandV3::from_json(&event.content).map_err(map_role_continuity_error)?;
    let mut write = state
        .db
        .begin_project_view_v3_write(tenant.community())
        .await
        .map_err(map_v3_write_error)?;
    let prepared = match write
        .prepare_role_command(&event, &command)
        .await
        .map_err(map_v3_write_error)?
    {
        ProjectViewV3PrepareOutcome::Replayed(receipt) => {
            write.rollback().await.map_err(map_v3_write_error)?;
            return Ok(IngestResult {
                event_id: event.id.to_hex(),
                accepted: true,
                message: response_message(&receipt.result)?,
            });
        }
        ProjectViewV3PrepareOutcome::Prepared(prepared) => prepared,
    };
    if prepared.projection_pubkey != state.relay_keypair.public_key() {
        write.rollback().await.map_err(map_v3_write_error)?;
        return Err(IngestError::Unavailable(
            "unavailable:project_view:signer_rotation".to_owned(),
        ));
    }

    let membership_projection = if prepared.membership_changed() {
        Some(
            build_membership_projection(&prepared.membership_after, prepared.canonical_time)?
                .sign_with_keys(&state.relay_keypair)
                .map_err(|error| {
                    IngestError::Internal(format!(
                        "error: sign atomic v3 membership projection: {error}"
                    ))
                })?,
        )
    } else {
        None
    };
    let membership_event_id = membership_projection
        .as_ref()
        .map(|projection| projection.id)
        .or(prepared.membership_snapshot_event_id)
        .ok_or_else(|| {
            IngestError::Internal("error: prepared v3 change has no membership snapshot".to_owned())
        })?;
    let context = buzz_sdk::project_view_v3::V3ProjectionContext {
        project_id: prepared.community_id,
        projection_generation: prepared.projection_generation,
        project_revision: prepared.project_revision,
        source: buzz_sdk::project_view_v3::V3ProjectionSource::NostrEvent {
            change_id: event.id,
            event_id: event.id,
        },
        updated_at: prepared.canonical_time,
    };
    let mut entity_projections = Vec::with_capacity(prepared.changes.len());
    let mut object_projections = Vec::with_capacity(prepared.work_heads.len());
    let mut changed_heads = Vec::with_capacity(prepared.changes.len() + prepared.work_heads.len());
    for change in &prepared.changes {
        let entity = v3_entity_change(change)?;
        let projection = buzz_sdk::project_view_v3::build_entity_projection(&context, &entity)
            .map_err(v3_projection_build_error)?
            .sign_with_keys(&state.relay_keypair)
            .map_err(v3_projection_sign_error)?;
        changed_heads.push(
            buzz_sdk::project_view_v3::changed_head_for_entity(&context, &entity, &projection)
                .map_err(v3_projection_bind_error)?,
        );
        entity_projections.push(PreparedV2EntityProjection {
            entity_type: change.entity_type(),
            entity_id: change.entity_id(),
            event: projection,
        });
    }
    for head in &prepared.work_heads {
        let PreparedV3ProjectObjectHead::Object {
            entry,
            responsible_role_id,
        } = head
        else {
            return Err(IngestError::Internal(
                "error: v3 Role command prepared a non-Work object head".to_owned(),
            ));
        };
        let projection = buzz_sdk::project_view_v3::build_project_object_projection(
            &context,
            entry,
            *responsible_role_id,
        )
        .map_err(v3_projection_build_error)?
        .sign_with_keys(&state.relay_keypair)
        .map_err(v3_projection_sign_error)?;
        changed_heads.push(
            buzz_sdk::project_view_v3::changed_head_for_project_object(
                &context,
                entry,
                &projection,
            )
            .map_err(v3_projection_bind_error)?,
        );
        object_projections.push(PreparedObjectProjection::new(entry.id(), projection));
    }
    let meta_projection = buzz_sdk::project_view_v3::build_meta_projection(
        &context,
        v3_counts(prepared.counts),
        membership_event_id,
        false,
        &changed_heads,
    )
    .map_err(v3_projection_build_error)?
    .sign_with_keys(&state.relay_keypair)
    .map_err(v3_projection_sign_error)?;
    let committed = write
        .commit_role_command(PreparedV3RoleCommit {
            command_event: event,
            entity_projections,
            object_projections,
            meta_projection,
            membership_projection,
        })
        .await
        .map_err(map_v3_write_error)?;
    let message = response_message(&committed.receipt.result)?;
    dispatch_project_view_committed_events(tenant, state, &committed.events).await;
    Ok(IngestResult {
        event_id: nostr::EventId::from_byte_array(committed.receipt.change_id).to_hex(),
        accepted: true,
        message,
    })
}

fn v3_projection_context(
    prepared: &buzz_db::project_view_v3::PreparedV3ProjectObjectChange,
    source_event_id: nostr::EventId,
) -> buzz_sdk::project_view_v3::V3ProjectionContext {
    buzz_sdk::project_view_v3::V3ProjectionContext {
        project_id: prepared.community_id,
        projection_generation: prepared.projection_generation,
        project_revision: prepared.project_revision,
        source: buzz_sdk::project_view_v3::V3ProjectionSource::NostrEvent {
            change_id: source_event_id,
            event_id: source_event_id,
        },
        updated_at: prepared.canonical_time,
    }
}

fn v3_counts(
    counts: buzz_db::project_view_v2::V2CanonicalCounts,
) -> buzz_sdk::project_view_v3::V3EntityCounts {
    buzz_sdk::project_view_v3::V3EntityCounts {
        active_objects: counts.active_objects,
        open_proposals: counts.open_proposals,
        active_assignments: counts.active_assignments,
        active_commitments: counts.active_commitments,
        checkpoints: counts.checkpoints,
        handoffs: counts.handoffs,
    }
}

fn v3_entity_change(
    change: &RoleContinuityChange,
) -> Result<buzz_sdk::project_view_v3::V3EntityChange, IngestError> {
    Ok(match change {
        RoleContinuityChange::Role(_) => {
            return Err(IngestError::Internal(
                "error: continuity-only v3 change produced a legacy Role head".to_owned(),
            ));
        }
        RoleContinuityChange::Proposal(value) => {
            buzz_sdk::project_view_v3::V3EntityChange::Proposal(value.clone())
        }
        RoleContinuityChange::Assignment(value) => {
            buzz_sdk::project_view_v3::V3EntityChange::Assignment(value.clone())
        }
        RoleContinuityChange::Commitment(value) => {
            buzz_sdk::project_view_v3::V3EntityChange::Commitment(value.clone())
        }
        RoleContinuityChange::Checkpoint(value) => {
            buzz_sdk::project_view_v3::V3EntityChange::Checkpoint(value.clone())
        }
        RoleContinuityChange::Handoff(value) => {
            buzz_sdk::project_view_v3::V3EntityChange::Handoff(value.clone())
        }
    })
}

fn v3_projection_build_error(error: buzz_sdk::SdkError) -> IngestError {
    IngestError::Internal(format!("error: build Project View v3 projection: {error}"))
}

fn v3_projection_sign_error(error: nostr::event::builder::Error) -> IngestError {
    IngestError::Internal(format!("error: sign Project View v3 projection: {error}"))
}

fn v3_projection_bind_error(error: buzz_sdk::SdkError) -> IngestError {
    IngestError::Internal(format!("error: bind Project View v3 projection: {error}"))
}

fn build_membership_projection(
    members: &[V2MembershipEntry],
    canonical_time: chrono::DateTime<chrono::Utc>,
) -> Result<EventBuilder, IngestError> {
    let mut tags = Vec::with_capacity(members.len() + 1);
    tags.push(Tag::parse(["-"]).map_err(|error| {
        IngestError::Internal(format!("error: build membership protection tag: {error}"))
    })?);
    for member in members {
        tags.push(
            Tag::parse(["member", member.pubkey.as_str(), member.role.as_str()]).map_err(
                |error| {
                    IngestError::Internal(format!("error: build membership member tag: {error}"))
                },
            )?,
        );
    }
    let seconds = u64::try_from(canonical_time.timestamp()).map_err(|_| {
        IngestError::Internal("error: canonical membership time precedes Unix epoch".to_owned())
    })?;
    Ok(EventBuilder::new(
        Kind::Custom(buzz_core::kind::KIND_NIP43_MEMBERSHIP_LIST as u16),
        "",
    )
    .tags(tags)
    .custom_created_at(Timestamp::from(seconds)))
}

#[derive(Debug, PartialEq, Eq)]
struct MutationTelemetry {
    operation: &'static str,
    object_type: Option<&'static str>,
    object_id: Option<uuid::Uuid>,
    expected_project_revision: Option<u64>,
}

impl MutationTelemetry {
    fn from_content(content: &str) -> Self {
        if content.len() > MAX_MUTATION_CONTENT_BYTES {
            return Self::unknown();
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
            return Self::unknown();
        };
        let request = value.get("request");
        let operation = request
            .and_then(|request| request.get("type"))
            .and_then(serde_json::Value::as_str)
            .map(bounded_operation)
            .unwrap_or("unknown");
        let raw_object_type = match operation {
            "initialize" => None,
            "create" => request
                .and_then(|request| request.get("object"))
                .and_then(|object| object.get("object_type"))
                .and_then(serde_json::Value::as_str),
            "update" | "delete" => request
                .and_then(|request| request.get("object_type"))
                .and_then(serde_json::Value::as_str),
            _ => None,
        };
        let object_id = match operation {
            "create" => request
                .and_then(|request| request.get("object"))
                .and_then(|object| object.get("id")),
            "update" | "delete" => request.and_then(|request| request.get("object_id")),
            _ => None,
        }
        .and_then(serde_json::Value::as_str)
        .and_then(|id| uuid::Uuid::parse_str(id).ok());

        Self {
            operation,
            object_type: raw_object_type.map(bounded_object_type),
            object_id,
            expected_project_revision: value
                .get("expected_project_revision")
                .and_then(serde_json::Value::as_u64),
        }
    }

    const fn unknown() -> Self {
        Self {
            operation: "unknown",
            object_type: None,
            object_id: None,
            expected_project_revision: None,
        }
    }
}

fn bounded_operation(operation: &str) -> &'static str {
    match operation {
        "initialize" => "initialize",
        "create" => "create",
        "update" => "update",
        "delete" => "delete",
        "request_role" => "request_role",
        "offer_role" => "offer_role",
        "accept_proposal" => "accept_proposal",
        "reject_proposal" => "reject_proposal",
        "withdraw_proposal" => "withdraw_proposal",
        "expire_proposal" => "expire_proposal",
        "authorize_proposal" => "authorize_proposal",
        "end_assignment" => "end_assignment",
        "request_replacement" => "request_replacement",
        "report_unable_to_continue" => "report_unable_to_continue",
        "set_work_responsibility" => "set_work_responsibility",
        "accept_work" => "accept_work",
        "end_commitment" => "end_commitment",
        "replace_commitment" => "replace_commitment",
        _ => "unknown",
    }
}

fn bounded_object_type(object_type: &str) -> &'static str {
    match object_type {
        "project_profile" => "project_profile",
        "goal" => "goal",
        "role" => "role",
        "plan" => "plan",
        "stage" => "stage",
        "requirement" => "requirement",
        "issue" => "issue",
        "work" => "work",
        "resource" => "resource",
        _ => "unknown",
    }
}

fn mutation_result_code(result: &Result<IngestResult, IngestError>) -> &'static str {
    match result {
        Ok(_) => "accepted",
        Err(IngestError::Rejected(_)) => "invalid",
        Err(IngestError::AuthFailed(_)) => "restricted",
        Err(IngestError::Conflict(_)) => "conflict",
        Err(IngestError::Unsupported(_)) => "unsupported",
        Err(IngestError::Unavailable(_)) => "unavailable",
        Err(IngestError::Internal(_)) => "internal",
    }
}

fn committed_project_revision(result: &Result<IngestResult, IngestError>) -> Option<u64> {
    let message = result.as_ref().ok()?.message.strip_prefix("response:")?;
    serde_json::from_str::<serde_json::Value>(message)
        .ok()?
        .get("project_revision")?
        .as_u64()
}

fn validate_mutation_tags(event: &Event) -> Result<(), IngestError> {
    let mut tags = event.tags.iter();
    let exact = event.tags.len() == 2
        && tags.next().is_some_and(|tag| tag.as_slice() == ["-"])
        && tags
            .next()
            .is_some_and(|tag| tag.as_slice() == ["t", MUTATION_TAG]);
    if exact {
        Ok(())
    } else {
        Err(IngestError::Rejected(
            "invalid:project_view:tags".to_owned(),
        ))
    }
}

fn response_message(result: &serde_json::Value) -> Result<String, IngestError> {
    serde_json::to_string(result)
        .map(|json| format!("response:{json}"))
        .map_err(|error| {
            IngestError::Internal(format!("error: serialize Project View receipt: {error}"))
        })
}

pub(crate) async fn dispatch_project_view_committed_events(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    events: &[Event],
) {
    let projection_options = PersistentDispatchOptions {
        audit: false,
        workflow: false,
    };
    for event in events {
        let kind = u32::from(event.kind.as_u16());
        let stored = StoredEvent::new(event.clone(), None);
        let actor = event.pubkey.to_hex();
        if kind == KIND_PROJECT_VIEW_MUTATION {
            dispatch_persistent_event(tenant, state, &stored, kind, &actor, None).await;
        } else {
            dispatch_persistent_event_with_options(
                tenant,
                state,
                &stored,
                kind,
                &actor,
                None,
                projection_options,
            )
            .await;
        }
    }
}

fn map_domain_error(error: DomainError) -> IngestError {
    let code = error.code();
    match error {
        DomainError::UnsupportedSchemaVersion { .. } => {
            IngestError::Unsupported(format!("unsupported:project_view:{code}"))
        }
        DomainError::NotInitialized
        | DomainError::AlreadyInitialized
        | DomainError::RevisionConflict { .. } => {
            IngestError::Conflict(format!("conflict:project_view:{code}"))
        }
        _ => IngestError::Rejected(format!("invalid:project_view:{code}")),
    }
}

fn map_role_continuity_error(error: RoleContinuityError) -> IngestError {
    let code = error.code();
    match error {
        RoleContinuityError::UnsupportedSchema => {
            IngestError::Unsupported(format!("unsupported:project_view:{code}"))
        }
        RoleContinuityError::RevisionConflict { .. }
        | RoleContinuityError::CompoundFenceConflict
        | RoleContinuityError::AssignmentEnded
        | RoleContinuityError::ActingAssignmentInvalid
        | RoleContinuityError::ProposalExpired
        | RoleContinuityError::ProposalNotOpen
        | RoleContinuityError::AlreadyConfirmed
        | RoleContinuityError::AlreadyReported => {
            IngestError::Conflict(format!("conflict:project_view:{code}"))
        }
        RoleContinuityError::NotAuthorized
        | RoleContinuityError::OwnerRequired
        | RoleContinuityError::SelfEndForbidden
        | RoleContinuityError::PeerLeaderForbidden
        | RoleContinuityError::ManagedLeaderTargetUnknown
        | RoleContinuityError::CandidateRequired
        | RoleContinuityError::CreatorRequired
        | RoleContinuityError::AssigneeRequired
        | RoleContinuityError::ActingAssignmentRequired => {
            IngestError::AuthFailed(format!("restricted:project_view:{code}"))
        }
        _ => IngestError::Rejected(format!("invalid:project_view:{code}")),
    }
}

fn map_continuity_storage_error(error: ProjectViewV2WriteError) -> IngestError {
    match error {
        ProjectViewV2WriteError::Unavailable { .. } => {
            IngestError::Unavailable("unavailable:project_view:not_ready".to_owned())
        }
        ProjectViewV2WriteError::Domain(error) => map_role_continuity_error(error),
        ProjectViewV2WriteError::ObjectDomain(error) => map_domain_error(error),
        ProjectViewV2WriteError::Database(buzz_db::DbError::AccessDenied(_)) => {
            IngestError::AuthFailed("restricted:project_view:access_denied".to_owned())
        }
        ProjectViewV2WriteError::Database(error) => IngestError::Internal(format!(
            "error: Project View continuity database failure: {error}"
        )),
        ProjectViewV2WriteError::Sqlx(error) => IngestError::Internal(format!(
            "error: Project View continuity SQL failure: {error}"
        )),
        ProjectViewV2WriteError::Audit(error) => IngestError::Internal(format!(
            "error: Project View continuity audit failure: {error}"
        )),
        ProjectViewV2WriteError::RuntimeSupervision(error) => {
            use buzz_db::project_runtime::RuntimeSupervisionError;

            match error {
                RuntimeSupervisionError::CommandFence
                | RuntimeSupervisionError::StaleEpoch
                | RuntimeSupervisionError::AssignmentEnded
                | RuntimeSupervisionError::BindingConflict => {
                    IngestError::Conflict("conflict:project_view:runtime_fence".to_owned())
                }
                RuntimeSupervisionError::Invalid(reason) => IngestError::Rejected(format!(
                    "invalid:project_view:runtime_supervision:{reason}"
                )),
                RuntimeSupervisionError::NotRegistered => IngestError::Rejected(
                    "invalid:project_view:runtime_supervision:not_registered".to_owned(),
                ),
                RuntimeSupervisionError::Database(error) => IngestError::Internal(format!(
                    "error: Project View runtime database failure: {error}"
                )),
                RuntimeSupervisionError::Sqlx(error) => IngestError::Internal(format!(
                    "error: Project View runtime SQL failure: {error}"
                )),
                RuntimeSupervisionError::Audit(error) => IngestError::Internal(format!(
                    "error: Project View runtime audit failure: {error}"
                )),
            }
        }
        ProjectViewV2WriteError::InvalidCommit(reason) => IngestError::Internal(format!(
            "error: invalid Project View continuity commit: {reason}"
        )),
    }
}

fn map_v3_object_error(error: V3ProjectObjectError) -> IngestError {
    match error {
        V3ProjectObjectError::Object(error) => map_domain_error(error),
        V3ProjectObjectError::Reference(V3ReferenceError::ContextCapabilityUnavailable) => {
            IngestError::Unavailable("unavailable:project_view:context_capability".to_owned())
        }
        V3ProjectObjectError::Reference(V3ReferenceError::DocumentCapabilityUnavailable) => {
            IngestError::Unavailable("unavailable:project_view:document_capability".to_owned())
        }
        V3ProjectObjectError::Reference(
            V3ReferenceError::MissingDocumentProof { .. }
            | V3ReferenceError::InactiveDocumentTarget { .. },
        )
        | V3ProjectObjectError::InvalidResourceTarget { .. } => {
            IngestError::Conflict("conflict:project_view:reference_target".to_owned())
        }
        V3ProjectObjectError::ResourceStillContextReferenced { .. } => {
            IngestError::Conflict("conflict:project_view:object_still_referenced".to_owned())
        }
        V3ProjectObjectError::Contract(_)
        | V3ProjectObjectError::Reference(
            V3ReferenceError::Contract(_) | V3ReferenceError::DuplicateProof { .. },
        )
        | V3ProjectObjectError::ResourceSourceReferenceForbidden => {
            IngestError::Rejected("invalid:project_view:context_reference".to_owned())
        }
        V3ProjectObjectError::InvalidRoleLevels(reason) => IngestError::Internal(format!(
            "error: invalid Project View v3 Role state: {reason}"
        )),
    }
}

fn map_v3_contract_error(error: V3ContractError) -> IngestError {
    IngestError::Rejected(format!("invalid:project_view:initialize:{error}"))
}

fn map_v3_write_error(error: ProjectViewV3WriteError) -> IngestError {
    match error {
        ProjectViewV3WriteError::Unavailable { .. } => {
            IngestError::Unavailable("unavailable:project_view:not_ready".to_owned())
        }
        ProjectViewV3WriteError::ObjectDomain(error) => map_v3_object_error(error),
        ProjectViewV3WriteError::Contract(error) => map_v3_contract_error(error),
        ProjectViewV3WriteError::RoleDomain(error) => map_role_continuity_error(error),
        ProjectViewV3WriteError::ContinuityStorage(error) => map_continuity_storage_error(error),
        ProjectViewV3WriteError::RuntimeSupervision(error) => match error {
            buzz_db::project_runtime::RuntimeSupervisionError::CommandFence
            | buzz_db::project_runtime::RuntimeSupervisionError::StaleEpoch
            | buzz_db::project_runtime::RuntimeSupervisionError::AssignmentEnded
            | buzz_db::project_runtime::RuntimeSupervisionError::BindingConflict => {
                IngestError::Conflict("conflict:project_view:runtime_fence".to_owned())
            }
            buzz_db::project_runtime::RuntimeSupervisionError::Invalid(reason) => {
                IngestError::Rejected(format!("invalid:project_view:runtime_supervision:{reason}"))
            }
            buzz_db::project_runtime::RuntimeSupervisionError::NotRegistered => {
                IngestError::Rejected(
                    "invalid:project_view:runtime_supervision:not_registered".to_owned(),
                )
            }
            buzz_db::project_runtime::RuntimeSupervisionError::Database(error) => {
                IngestError::Internal(format!(
                    "error: Project View v3 runtime database failure: {error}"
                ))
            }
            buzz_db::project_runtime::RuntimeSupervisionError::Sqlx(error) => {
                IngestError::Internal(format!(
                    "error: Project View v3 runtime SQL failure: {error}"
                ))
            }
            buzz_db::project_runtime::RuntimeSupervisionError::Audit(error) => {
                IngestError::Internal(format!(
                    "error: Project View v3 runtime audit failure: {error}"
                ))
            }
        },
        ProjectViewV3WriteError::Database(error) => {
            IngestError::Internal(format!("error: Project View v3 database failure: {error}"))
        }
        ProjectViewV3WriteError::Sqlx(error) => {
            IngestError::Internal(format!("error: Project View v3 SQL failure: {error}"))
        }
        ProjectViewV3WriteError::InvalidCommit(reason) => {
            IngestError::Internal(format!("error: invalid Project View v3 commit: {reason}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_core::kind::{KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_OBJECT};
    use buzz_sdk::{
        project_view_v2::{
            build_meta_projection as build_v2_meta_projection, V2EntityCounts, V2ProjectionContext,
            V2ProjectionSource,
        },
        project_view_v3::{
            build_meta_projection as build_v3_meta_projection, V3EntityCounts, V3ProjectionContext,
            V3ProjectionSource,
        },
    };
    use nostr::{EventBuilder, EventId, Keys, Kind, Tag};

    fn mutation_event(tags: Vec<Tag>) -> Event {
        EventBuilder::new(Kind::Custom(KIND_PROJECT_VIEW_MUTATION as u16), "{}")
            .tags(tags)
            .sign_with_keys(&Keys::generate())
            .expect("sign test event")
    }

    #[test]
    fn mutation_outer_tags_are_exact_and_ordered() {
        let valid = mutation_event(vec![
            Tag::parse(["-"]).expect("protected"),
            Tag::parse(["t", MUTATION_TAG]).expect("type"),
        ]);
        assert!(validate_mutation_tags(&valid).is_ok());

        let reversed = mutation_event(vec![
            Tag::parse(["t", MUTATION_TAG]).expect("type"),
            Tag::parse(["-"]).expect("protected"),
        ]);
        assert!(validate_mutation_tags(&reversed).is_err());

        let channel_id = uuid::Uuid::new_v4().to_string();
        let extra = mutation_event(vec![
            Tag::parse(["-"]).expect("protected"),
            Tag::parse(["t", MUTATION_TAG]).expect("type"),
            Tag::parse(["h", channel_id.as_str()]).expect("h"),
        ]);
        assert!(validate_mutation_tags(&extra).is_err());
    }

    #[test]
    fn ordinary_runtime_requires_schema_v3() {
        assert!(require_project_view_v3_runtime(3).is_ok());

        for legacy_schema in [1, 2] {
            assert!(matches!(
                require_project_view_v3_runtime(legacy_schema),
                Err(IngestError::Unsupported(reason))
                    if reason == "unsupported:project_view:migration_required"
            ));
        }

        assert!(matches!(
            require_project_view_v3_runtime(4),
            Err(IngestError::Unsupported(reason))
                if reason == "unsupported:project_view:schema"
        ));
    }

    #[test]
    fn filter_classification_distinguishes_exclusive_mixed_and_wildcard() {
        let exclusive = Filter::new().kinds([
            nostr::Kind::Custom(KIND_PROJECT_VIEW_OBJECT as u16),
            nostr::Kind::Custom(KIND_PROJECT_VIEW_META as u16),
        ]);
        assert!(filter_can_match(&exclusive));
        assert!(filter_is_exclusively_project_view(&exclusive));

        let mixed = Filter::new().kinds([
            nostr::Kind::Custom(KIND_PROJECT_VIEW_OBJECT as u16),
            nostr::Kind::TextNote,
        ]);
        assert!(filter_can_match(&mixed));
        assert!(!filter_is_exclusively_project_view(&mixed));

        assert!(filter_can_match(&Filter::new()));
        assert!(!filter_is_exclusively_project_view(&Filter::new()));
    }

    #[test]
    fn ordinary_projection_filters_require_closed_v3_tags() {
        let t = nostr::SingleLetterTag::lowercase(nostr::Alphabet::T);
        let object = Filter::new()
            .kind(nostr::Kind::Custom(KIND_PROJECT_VIEW_OBJECT as u16))
            .custom_tags(t, [PROJECT_VIEW_V3_OBJECT_TAG]);
        assert!(filter_allows_v3_projections(&object));
        assert!(!filter_has_unscoped_project_view_projection(&object));

        let t = nostr::SingleLetterTag::lowercase(nostr::Alphabet::T);
        let entity = Filter::new()
            .kind(nostr::Kind::Custom(KIND_PROJECT_VIEW_OBJECT as u16))
            .custom_tags(t, [PROJECT_VIEW_V3_ENTITY_TAG]);
        assert!(filter_allows_v3_projections(&entity));

        let t = nostr::SingleLetterTag::lowercase(nostr::Alphabet::T);
        let meta = Filter::new()
            .kind(nostr::Kind::Custom(KIND_PROJECT_VIEW_META as u16))
            .custom_tags(t, [PROJECT_VIEW_V3_META_TAG]);
        assert!(filter_allows_v3_projections(&meta));

        let unscoped = Filter::new().kind(nostr::Kind::Custom(KIND_PROJECT_VIEW_META as u16));
        assert!(!filter_allows_v3_projections(&unscoped));
        assert!(filter_has_unscoped_project_view_projection(&unscoped));

        let t = nostr::SingleLetterTag::lowercase(nostr::Alphabet::T);
        let legacy = Filter::new()
            .kind(nostr::Kind::Custom(KIND_PROJECT_VIEW_OBJECT as u16))
            .custom_tags(t, ["buzz-project-view-v2-object"]);
        assert!(!filter_allows_v3_projections(&legacy));
        assert!(filter_has_unscoped_project_view_projection(&legacy));

        let ordinary = Filter::new().kind(nostr::Kind::TextNote);
        assert!(!filter_has_unscoped_project_view_projection(&ordinary));
    }

    #[test]
    fn shared_meta_coordinate_excludes_real_v2_projection_from_v3_results_and_counts() {
        let relay = Keys::generate();
        let project_id = buzz_core::CommunityId::from_uuid(uuid::Uuid::new_v4());
        let updated_at =
            chrono::DateTime::from_timestamp(1_800_000_000, 0).expect("valid projection timestamp");
        let change_id = EventId::all_zeros();

        let v2 = build_v2_meta_projection(
            &V2ProjectionContext {
                project_id,
                projection_generation: 1,
                project_revision: 1,
                source: V2ProjectionSource::System {
                    change_id,
                    audit_seq: 1,
                },
                updated_at,
            },
            V2EntityCounts {
                active_objects: 0,
                open_proposals: 0,
                active_assignments: 0,
                active_commitments: 0,
                checkpoints: 0,
                handoffs: 0,
            },
            EventId::all_zeros(),
            true,
            &[],
        )
        .expect("build v2 metadata")
        .sign_with_keys(&relay)
        .expect("sign v2 metadata");
        let v3_context = V3ProjectionContext {
            project_id,
            projection_generation: 2,
            project_revision: 2,
            source: V3ProjectionSource::System {
                change_id,
                audit_seq: 2,
            },
            updated_at,
        };
        let v3_counts = V3EntityCounts {
            active_objects: 0,
            open_proposals: 0,
            active_assignments: 0,
            active_commitments: 0,
            checkpoints: 0,
            handoffs: 0,
        };
        let v3 = build_v3_meta_projection(&v3_context, v3_counts, EventId::all_zeros(), true, &[])
            .expect("build v3 metadata")
            .sign_with_keys(&relay)
            .expect("sign v3 metadata");
        let foreign_signer = Keys::generate();
        let foreign_v3 =
            build_v3_meta_projection(&v3_context, v3_counts, EventId::all_zeros(), true, &[])
                .expect("build foreign v3 metadata")
                .sign_with_keys(&foreign_signer)
                .expect("sign foreign v3 metadata");

        let t = nostr::SingleLetterTag::lowercase(nostr::Alphabet::T);
        let filter = Filter::new()
            .kind(Kind::Custom(KIND_PROJECT_VIEW_META as u16))
            .custom_tags(t, [PROJECT_VIEW_V3_META_TAG]);
        assert!(filter_requires_v3_projection_post_filter(&filter));
        assert!(buzz_core::filter::filters_match(
            std::slice::from_ref(&filter),
            &buzz_core::StoredEvent::new(v2.clone(), None),
        ));
        assert!(buzz_core::filter::filters_match(
            std::slice::from_ref(&filter),
            &buzz_core::StoredEvent::new(v3.clone(), None),
        ));

        let visible = [v2, v3]
            .into_iter()
            .filter(|event| {
                projection_event_visible_for_filter(&filter, event, Some(&relay.public_key()))
            })
            .collect::<Vec<_>>();
        assert_eq!(visible.len(), 1);
        let visible_content: serde_json::Value =
            serde_json::from_str(&visible[0].content).expect("valid metadata JSON");
        assert_eq!(visible_content["schema_version"], 3);
        assert!(event_is_v3_projection(&visible[0], &relay.public_key()));
        assert!(event_is_v3_projection(
            &foreign_v3,
            &foreign_signer.public_key()
        ));
        assert!(!event_is_v3_projection(&foreign_v3, &relay.public_key()));
        assert!(!projection_event_visible_for_filter(
            &filter,
            &foreign_v3,
            Some(&relay.public_key()),
        ));
        assert!(!projection_event_visible_for_filter(
            &filter,
            &visible[0],
            None,
        ));
    }

    #[test]
    fn mutation_telemetry_is_bounded_and_omits_payloads() {
        let object_id = uuid::Uuid::new_v4();
        let telemetry = MutationTelemetry::from_content(
            &serde_json::json!({
                "schema_version": 1,
                "expected_project_revision": 7,
                "request": {
                    "type": "update",
                    "object_type": "issue",
                    "object_id": object_id,
                    "patch": {
                        "title": "must not become a metric label",
                        "locator": "must not become a log field"
                    }
                }
            })
            .to_string(),
        );
        assert_eq!(
            telemetry,
            MutationTelemetry {
                operation: "update",
                object_type: Some("issue"),
                object_id: Some(object_id),
                expected_project_revision: Some(7),
            }
        );

        let unknown = MutationTelemetry::from_content(
            r#"{"request":{"type":"attacker-operation","object_type":"attacker-type"}}"#,
        );
        assert_eq!(unknown.operation, "unknown");
        assert_eq!(unknown.object_type, None);
        assert_eq!(MutationTelemetry::from_content("{").operation, "unknown");
    }

    #[test]
    fn mutation_result_labels_are_closed() {
        let accepted = Ok(IngestResult {
            event_id: "event".to_owned(),
            accepted: true,
            message: r#"response:{"project_revision":9}"#.to_owned(),
        });
        assert_eq!(mutation_result_code(&accepted), "accepted");
        assert_eq!(committed_project_revision(&accepted), Some(9));

        let conflict = Err(IngestError::Conflict("details".to_owned()));
        assert_eq!(mutation_result_code(&conflict), "conflict");
        assert_eq!(committed_project_revision(&conflict), None);
    }
}
