//! Project View protocol adapter.
//!
//! The pure reducer lives in `buzz-project-view`, wire construction lives in
//! `buzz-sdk`, and atomic persistence lives in `buzz-db`. This module owns the
//! Relay-specific security gates, signing, error mapping, and post-commit
//! delivery policy.

use std::{sync::Arc, time::Instant};

use buzz_auth::Scope;
use buzz_core::kind::{
    is_project_view_protocol_kind, KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_MUTATION,
    KIND_PROJECT_VIEW_OBJECT,
};
use buzz_core::{StoredEvent, TenantContext};
use buzz_db::project_view::{
    PreparedObjectProjection, PreparedProjectViewCommit, ProjectViewWriteError,
};
use buzz_db::project_view_v2::{
    PreparedV2EntityProjection, PreparedV2ProjectObjectCommit, PreparedV2ProjectObjectHead,
    PreparedV2RoleCommit, ProjectViewV2PrepareOutcome, ProjectViewV2ProjectObjectPrepareOutcome,
    ProjectViewV2WriteError, V2MembershipEntry,
};
use buzz_db::project_view_v3::{
    PreparedV3ProjectObjectCommit, PreparedV3ProjectObjectHead, PreparedV3RoleCommit,
    ProjectViewV3PrepareOutcome, ProjectViewV3ProjectObjectPrepareOutcome, ProjectViewV3WriteError,
};
use buzz_project_view::v2::{
    ProjectObjectCommand, RoleCommand, RoleContinuityChange, RoleContinuityError,
};
use buzz_project_view::v3::{
    ProjectObjectCommandV3, ProjectViewInitializeV3, RoleCommandV3, V3ContractError,
    V3ProjectObjectError, V3ReferenceError,
};
use buzz_project_view::{
    DomainError, Mutation, ProjectViewEntry, ProjectionPlan, MAX_MUTATION_CONTENT_BYTES,
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
    let status = state
        .db
        .project_view_status_by_host(tenant.host())
        .await
        .map_err(|error| {
            IngestError::Internal(format!("error: Project View status check failed: {error}"))
        })?
        .ok_or_else(|| IngestError::Unavailable("unavailable:project_view:community".to_owned()))?;
    let relay_pubkey = state.relay_keypair.public_key();
    let schema_version = state
        .db
        .project_view_schema_version(tenant.community())
        .await
        .map_err(|error| {
            IngestError::Internal(format!("error: Project View schema lookup failed: {error}"))
        })?;
    if schema_version == 3 {
        if is_initialize_command(&event.content) {
            let command = ProjectViewInitializeV3::from_json(&event.content)
                .map_err(map_v3_contract_error)?;
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
        return if is_project_object_command(&event.content) {
            handle_v3_project_object_mutation(tenant, state, event).await
        } else {
            handle_v3_role_mutation(tenant, state, event).await
        };
    }
    if !status.enabled {
        return Err(IngestError::Unavailable(
            "unavailable:project_view:disabled".to_owned(),
        ));
    }
    if schema_version == 2 {
        if !state
            .db
            .project_view_v2_capability_ready(tenant.community(), &relay_pubkey)
            .await
            .map_err(|error| {
                IngestError::Internal(format!(
                    "error: Project View v2 readiness check failed: {error}"
                ))
            })?
        {
            return Err(IngestError::Unavailable(
                "unavailable:project_view:not_ready".to_owned(),
            ));
        }
        return if is_project_object_command(&event.content) {
            handle_v2_project_object_mutation(tenant, state, event).await
        } else {
            handle_v2_role_mutation(tenant, state, event).await
        };
    }
    if schema_version != 1 {
        return Err(IngestError::Unsupported(
            "unsupported:project_view:schema".to_owned(),
        ));
    }
    if !state
        .db
        .project_view_capability_ready(tenant.community(), &relay_pubkey)
        .await
        .map_err(|error| {
            IngestError::Internal(format!(
                "error: Project View readiness check failed: {error}"
            ))
        })?
    {
        return Err(IngestError::Unavailable(
            "unavailable:project_view:not_ready".to_owned(),
        ));
    }

    let mutation = Mutation::from_json(&event.content).map_err(map_domain_error)?;
    let mut write = state
        .db
        .begin_project_view_write(tenant.community())
        .await
        .map_err(map_write_error)?;

    // Current security/readiness gates intentionally precede receipt lookup.
    // A once-authorized event cannot use idempotency to bypass a later member
    // removal, ban, timeout, feature disable, or signer-readiness failure.
    if let Some(receipt) = write
        .find_receipt(event.id.as_bytes())
        .await
        .map_err(map_write_error)?
    {
        write.rollback().await.map_err(map_write_error)?;
        return Ok(IngestResult {
            event_id: event.id.to_hex(),
            accepted: true,
            message: response_message(&receipt.result)?,
        });
    }

    let context = write.load_current().await.map_err(map_write_error)?;
    let projection_generation = match context.metadata.as_ref() {
        Some(metadata) if metadata.projection_pubkey == relay_pubkey => {
            metadata.projection_generation
        }
        Some(_) => {
            write.rollback().await.map_err(map_write_error)?;
            return Err(IngestError::Unavailable(
                "unavailable:project_view:signer_rotation".to_owned(),
            ));
        }
        None => 1,
    };
    let (next_state, outcome) = context
        .state
        .reduce(&mutation, event.pubkey, context.canonical_time)
        .map_err(map_domain_error)?;
    let plan = ProjectionPlan::for_mutation(
        &next_state,
        &outcome,
        event.id.to_bytes(),
        projection_generation,
    )
    .map_err(map_domain_error)?;

    let mut object_projections = Vec::with_capacity(plan.entries().len());
    let mut object_events = Vec::with_capacity(plan.entries().len());
    let mut changed_heads = Vec::with_capacity(plan.entries().len());
    for entry in plan.entries() {
        let projection = buzz_sdk::project_view::build_object_projection(&plan, entry)
            .map_err(|error| {
                IngestError::Internal(format!("error: build Project View projection: {error}"))
            })?
            .sign_with_keys(&state.relay_keypair)
            .map_err(|error| {
                IngestError::Internal(format!("error: sign Project View projection: {error}"))
            })?;
        changed_heads.push(
            buzz_sdk::project_view::changed_head_for(&plan, entry, &projection).map_err(
                |error| {
                    IngestError::Internal(format!("error: bind Project View changed head: {error}"))
                },
            )?,
        );
        object_projections.push(PreparedObjectProjection::new(
            entry.id(),
            projection.clone(),
        ));
        object_events.push(projection);
    }
    let meta_projection = buzz_sdk::project_view::build_meta_projection(&plan, &changed_heads)
        .map_err(|error| {
            IngestError::Internal(format!("error: build Project View metadata: {error}"))
        })?
        .sign_with_keys(&state.relay_keypair)
        .map_err(|error| {
            IngestError::Internal(format!("error: sign Project View metadata: {error}"))
        })?;
    let receipt_result = mutation_receipt(&outcome.changed_entries, outcome.project_revision);

    let commit = PreparedProjectViewCommit {
        command_event: event.clone(),
        mutation,
        next_state,
        outcome,
        object_projections,
        meta_projection: meta_projection.clone(),
        projection_generation,
        receipt_result,
    };
    let committed = write
        .commit_mutation(commit)
        .await
        .map_err(map_write_error)?;
    let message = response_message(&committed.receipt.result)?;
    if !committed.replayed {
        dispatch_committed_events(tenant, state, &event, &object_events, &meta_projection).await;
    }

    Ok(IngestResult {
        event_id: event.id.to_hex(),
        accepted: true,
        message,
    })
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
    dispatch_v2_committed_events(tenant, state, &outcome.events).await;
    Ok(IngestResult {
        event_id: event.id.to_hex(),
        accepted: true,
        message,
    })
}

async fn handle_v2_project_object_mutation(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: Event,
) -> Result<IngestResult, IngestError> {
    let command = ProjectObjectCommand::from_json(&event.content).map_err(map_domain_error)?;
    let mut write = state
        .db
        .begin_project_view_v2_write(tenant.community())
        .await
        .map_err(map_v2_write_error)?;
    let preparation = write
        .prepare_project_object_command(&event, &command)
        .await
        .map_err(map_v2_write_error)?;
    let prepared = match preparation {
        ProjectViewV2ProjectObjectPrepareOutcome::Replayed(receipt) => {
            write.rollback().await.map_err(map_v2_write_error)?;
            return Ok(IngestResult {
                event_id: event.id.to_hex(),
                accepted: true,
                message: response_message(&receipt.result)?,
            });
        }
        ProjectViewV2ProjectObjectPrepareOutcome::Prepared(prepared) => prepared,
    };
    if prepared.projection_pubkey != state.relay_keypair.public_key() {
        write.rollback().await.map_err(map_v2_write_error)?;
        return Err(IngestError::Unavailable(
            "unavailable:project_view:signer_rotation".to_owned(),
        ));
    }
    let context = buzz_sdk::project_view_v2::V2ProjectionContext {
        project_id: prepared.community_id,
        projection_generation: prepared.projection_generation,
        project_revision: prepared.project_revision,
        source: buzz_sdk::project_view_v2::V2ProjectionSource::NostrEvent {
            change_id: event.id,
            event_id: event.id,
        },
        updated_at: prepared.canonical_time,
    };
    let mut projections = Vec::with_capacity(prepared.heads.len());
    let mut entity_projections = Vec::with_capacity(prepared.entity_changes.len());
    let mut changed_heads =
        Vec::with_capacity(prepared.heads.len() + prepared.entity_changes.len());
    for head in &prepared.heads {
        let (object_id, projection, changed_head) = match head {
            PreparedV2ProjectObjectHead::Role(role) => {
                let entity = RoleContinuityChange::Role(role.clone());
                let projection =
                    buzz_sdk::project_view_v2::build_entity_projection(&context, &entity)
                        .map_err(|error| {
                            IngestError::Internal(format!(
                                "error: build v2 Role projection: {error}"
                            ))
                        })?
                        .sign_with_keys(&state.relay_keypair)
                        .map_err(|error| {
                            IngestError::Internal(format!(
                                "error: sign v2 Role projection: {error}"
                            ))
                        })?;
                let changed_head =
                    buzz_sdk::project_view_v2::changed_head_for(&context, &entity, &projection)
                        .map_err(|error| {
                            IngestError::Internal(format!(
                                "error: bind v2 Role changed head: {error}"
                            ))
                        })?;
                (role.role_id, projection, changed_head)
            }
            PreparedV2ProjectObjectHead::Object {
                entry,
                responsible_role_id,
            } => {
                let projection =
                    buzz_sdk::project_view_v2::build_project_object_projection_with_responsibility(
                        &context,
                        entry,
                        *responsible_role_id,
                    )
                    .map_err(|error| {
                        IngestError::Internal(format!(
                            "error: build v2 Project object projection: {error}"
                        ))
                    })?
                    .sign_with_keys(&state.relay_keypair)
                    .map_err(|error| {
                        IngestError::Internal(format!(
                            "error: sign v2 Project object projection: {error}"
                        ))
                    })?;
                let changed_head = buzz_sdk::project_view_v2::changed_head_for_project_object(
                    &context,
                    entry,
                    &projection,
                )
                .map_err(|error| {
                    IngestError::Internal(format!(
                        "error: bind v2 Project object changed head: {error}"
                    ))
                })?;
                (entry.id(), projection, changed_head)
            }
        };
        projections.push(PreparedObjectProjection::new(object_id, projection));
        changed_heads.push(changed_head);
    }
    for entity in &prepared.entity_changes {
        let projection = buzz_sdk::project_view_v2::build_entity_projection(&context, entity)
            .map_err(|error| {
                IngestError::Internal(format!(
                    "error: build terminal Work side-effect projection: {error}"
                ))
            })?
            .sign_with_keys(&state.relay_keypair)
            .map_err(|error| {
                IngestError::Internal(format!(
                    "error: sign terminal Work side-effect projection: {error}"
                ))
            })?;
        changed_heads.push(
            buzz_sdk::project_view_v2::changed_head_for(&context, entity, &projection).map_err(
                |error| {
                    IngestError::Internal(format!(
                        "error: bind terminal Work side-effect head: {error}"
                    ))
                },
            )?,
        );
        entity_projections.push(PreparedV2EntityProjection {
            entity_type: entity.entity_type(),
            entity_id: entity.entity_id(),
            event: projection,
        });
    }
    let counts = buzz_sdk::project_view_v2::V2EntityCounts {
        active_objects: prepared.counts.active_objects,
        open_proposals: prepared.counts.open_proposals,
        active_assignments: prepared.counts.active_assignments,
        active_commitments: prepared.counts.active_commitments,
        checkpoints: prepared.counts.checkpoints,
        handoffs: prepared.counts.handoffs,
    };
    let meta_projection = buzz_sdk::project_view_v2::build_meta_projection(
        &context,
        counts,
        prepared.membership_snapshot_event_id,
        false,
        &changed_heads,
    )
    .map_err(|error| {
        IngestError::Internal(format!("error: build v2 metadata projection: {error}"))
    })?
    .sign_with_keys(&state.relay_keypair)
    .map_err(|error| {
        IngestError::Internal(format!("error: sign v2 metadata projection: {error}"))
    })?;
    let committed = write
        .commit_project_object_command(PreparedV2ProjectObjectCommit {
            command_event: event.clone(),
            object_projections: projections,
            entity_projections,
            meta_projection,
        })
        .await
        .map_err(map_v2_write_error)?;
    let message = response_message(&committed.receipt.result)?;
    dispatch_v2_committed_events(tenant, state, &committed.events).await;
    Ok(IngestResult {
        event_id: event.id.to_hex(),
        accepted: true,
        message,
    })
}

async fn handle_v2_role_mutation(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: Event,
) -> Result<IngestResult, IngestError> {
    let command = RoleCommand::from_json(&event.content).map_err(map_v2_domain_error)?;
    let mut write = state
        .db
        .begin_project_view_v2_write(tenant.community())
        .await
        .map_err(map_v2_write_error)?;
    let preparation = write
        .prepare_role_command(&event, &command)
        .await
        .map_err(map_v2_write_error)?;
    let prepared = match preparation {
        ProjectViewV2PrepareOutcome::Replayed(receipt) => {
            write.rollback().await.map_err(map_v2_write_error)?;
            return Ok(IngestResult {
                event_id: event.id.to_hex(),
                accepted: true,
                message: response_message(&receipt.result)?,
            });
        }
        ProjectViewV2PrepareOutcome::Prepared(prepared) => prepared,
    };
    if prepared.projection_pubkey != state.relay_keypair.public_key() {
        write.rollback().await.map_err(map_v2_write_error)?;
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
                        "error: sign atomic membership projection: {error}"
                    ))
                })?,
        )
    } else {
        None
    };
    let membership_event_id = membership_projection
        .as_ref()
        .map(|event| event.id)
        .or(prepared.membership_snapshot_event_id)
        .ok_or_else(|| {
            IngestError::Internal("error: prepared v2 change has no membership snapshot".to_owned())
        })?;
    let context = buzz_sdk::project_view_v2::V2ProjectionContext {
        project_id: prepared.community_id,
        projection_generation: prepared.projection_generation,
        project_revision: prepared.project_revision,
        source: buzz_sdk::project_view_v2::V2ProjectionSource::NostrEvent {
            change_id: event.id,
            event_id: event.id,
        },
        updated_at: prepared.canonical_time,
    };
    let mut entity_projections = Vec::with_capacity(prepared.changes.len());
    let mut object_projections = Vec::with_capacity(prepared.work_heads.len());
    let mut changed_heads = Vec::with_capacity(prepared.changes.len() + prepared.work_heads.len());
    for entity in &prepared.changes {
        let projection = buzz_sdk::project_view_v2::build_entity_projection(&context, entity)
            .map_err(|error| {
                IngestError::Internal(format!("error: build v2 entity projection: {error}"))
            })?
            .sign_with_keys(&state.relay_keypair)
            .map_err(|error| {
                IngestError::Internal(format!("error: sign v2 entity projection: {error}"))
            })?;
        changed_heads.push(
            buzz_sdk::project_view_v2::changed_head_for(&context, entity, &projection).map_err(
                |error| IngestError::Internal(format!("error: bind v2 changed head: {error}")),
            )?,
        );
        entity_projections.push(PreparedV2EntityProjection {
            entity_type: entity.entity_type(),
            entity_id: entity.entity_id(),
            event: projection,
        });
    }
    for head in &prepared.work_heads {
        let PreparedV2ProjectObjectHead::Object {
            entry,
            responsible_role_id,
        } = head
        else {
            return Err(IngestError::Internal(
                "error: Role command prepared a non-Work object head".to_owned(),
            ));
        };
        let projection =
            buzz_sdk::project_view_v2::build_project_object_projection_with_responsibility(
                &context,
                entry,
                *responsible_role_id,
            )
            .map_err(|error| {
                IngestError::Internal(format!(
                    "error: build Work responsibility projection: {error}"
                ))
            })?
            .sign_with_keys(&state.relay_keypair)
            .map_err(|error| {
                IngestError::Internal(format!(
                    "error: sign Work responsibility projection: {error}"
                ))
            })?;
        changed_heads.push(
            buzz_sdk::project_view_v2::changed_head_for_project_object(
                &context,
                entry,
                &projection,
            )
            .map_err(|error| {
                IngestError::Internal(format!(
                    "error: bind Work responsibility changed head: {error}"
                ))
            })?,
        );
        object_projections.push(PreparedObjectProjection::new(entry.id(), projection));
    }
    let counts = buzz_sdk::project_view_v2::V2EntityCounts {
        active_objects: prepared.counts.active_objects,
        open_proposals: prepared.counts.open_proposals,
        active_assignments: prepared.counts.active_assignments,
        active_commitments: prepared.counts.active_commitments,
        checkpoints: prepared.counts.checkpoints,
        handoffs: prepared.counts.handoffs,
    };
    let meta_projection = buzz_sdk::project_view_v2::build_meta_projection(
        &context,
        counts,
        membership_event_id,
        false,
        &changed_heads,
    )
    .map_err(|error| {
        IngestError::Internal(format!("error: build v2 metadata projection: {error}"))
    })?
    .sign_with_keys(&state.relay_keypair)
    .map_err(|error| {
        IngestError::Internal(format!("error: sign v2 metadata projection: {error}"))
    })?;
    let committed = write
        .commit_role_command(PreparedV2RoleCommit {
            command_event: event.clone(),
            entity_projections,
            object_projections,
            meta_projection,
            membership_projection,
        })
        .await
        .map_err(map_v2_write_error)?;
    let message = response_message(&committed.receipt.result)?;
    dispatch_v2_committed_events(tenant, state, &committed.events).await;
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
    dispatch_v2_committed_events(tenant, state, &committed.events).await;
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
    let command = RoleCommandV3::from_json(&event.content).map_err(map_v2_domain_error)?;
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
    dispatch_v2_committed_events(tenant, state, &committed.events).await;
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

fn mutation_receipt(entries: &[ProjectViewEntry], project_revision: u64) -> serde_json::Value {
    let mut result = serde_json::Map::new();
    result.insert(
        "project_revision".to_owned(),
        serde_json::Value::from(project_revision),
    );
    if let [entry] = entries {
        result.insert(
            "object_id".to_owned(),
            serde_json::Value::String(entry.id().to_string()),
        );
        result.insert(
            "object_revision".to_owned(),
            serde_json::Value::from(entry.object_revision()),
        );
        result.insert(
            "deleted".to_owned(),
            serde_json::Value::Bool(matches!(entry, ProjectViewEntry::Tombstone(_))),
        );
    }
    serde_json::Value::Object(result)
}

fn response_message(result: &serde_json::Value) -> Result<String, IngestError> {
    serde_json::to_string(result)
        .map(|json| format!("response:{json}"))
        .map_err(|error| {
            IngestError::Internal(format!("error: serialize Project View receipt: {error}"))
        })
}

async fn dispatch_committed_events(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    command: &Event,
    object_events: &[Event],
    meta_event: &Event,
) {
    let actor = command.pubkey.to_hex();
    let command_stored = StoredEvent::new(command.clone(), None);
    dispatch_persistent_event(
        tenant,
        state,
        &command_stored,
        KIND_PROJECT_VIEW_MUTATION,
        &actor,
        None,
    )
    .await;

    let projection_options = PersistentDispatchOptions {
        audit: false,
        workflow: false,
    };
    let relay_actor = state.relay_keypair.public_key().to_hex();
    for event in object_events {
        let stored = StoredEvent::new(event.clone(), None);
        dispatch_persistent_event_with_options(
            tenant,
            state,
            &stored,
            KIND_PROJECT_VIEW_OBJECT,
            &relay_actor,
            None,
            projection_options,
        )
        .await;
    }
    let stored = StoredEvent::new(meta_event.clone(), None);
    dispatch_persistent_event_with_options(
        tenant,
        state,
        &stored,
        KIND_PROJECT_VIEW_META,
        &relay_actor,
        None,
        projection_options,
    )
    .await;
}

pub(crate) async fn dispatch_v2_committed_events(
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

fn map_v2_domain_error(error: RoleContinuityError) -> IngestError {
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

fn map_v2_write_error(error: ProjectViewV2WriteError) -> IngestError {
    match error {
        ProjectViewV2WriteError::Unavailable { .. } => {
            IngestError::Unavailable("unavailable:project_view:not_ready".to_owned())
        }
        ProjectViewV2WriteError::Domain(error) => map_v2_domain_error(error),
        ProjectViewV2WriteError::ObjectDomain(error) => map_domain_error(error),
        ProjectViewV2WriteError::Database(error) => {
            IngestError::Internal(format!("error: Project View v2 database failure: {error}"))
        }
        ProjectViewV2WriteError::Sqlx(error) => {
            IngestError::Internal(format!("error: Project View v2 SQL failure: {error}"))
        }
        ProjectViewV2WriteError::Audit(error) => {
            IngestError::Internal(format!("error: Project View v2 audit failure: {error}"))
        }
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
        ProjectViewV2WriteError::InvalidCommit(reason) => {
            IngestError::Internal(format!("error: invalid Project View v2 commit: {reason}"))
        }
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
        ProjectViewV3WriteError::RoleDomain(error) => map_v2_domain_error(error),
        ProjectViewV3WriteError::ContinuityStorage(error) => map_v2_write_error(error),
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

fn map_write_error(error: ProjectViewWriteError) -> IngestError {
    match error {
        ProjectViewWriteError::Unavailable { .. } => {
            IngestError::Unavailable("unavailable:project_view:disabled".to_owned())
        }
        ProjectViewWriteError::RevisionConflict { .. } => {
            IngestError::Conflict("conflict:project_view:revision".to_owned())
        }
        ProjectViewWriteError::Domain(error) => map_domain_error(error),
        ProjectViewWriteError::Database(error) => {
            IngestError::Internal(format!("error: Project View database failure: {error}"))
        }
        ProjectViewWriteError::Sqlx(error) => {
            IngestError::Internal(format!("error: Project View SQL failure: {error}"))
        }
        ProjectViewWriteError::InvalidCommit(reason) => {
            IngestError::Internal(format!("error: invalid Project View commit: {reason}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};

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
