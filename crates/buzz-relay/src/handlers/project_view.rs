//! Project View protocol adapter.
//!
//! The pure reducer lives in `buzz-project-view`, wire construction lives in
//! `buzz-sdk`, and atomic persistence lives in `buzz-db`. This module owns the
//! Relay-specific security gates, signing, error mapping, and post-commit
//! delivery policy.

use std::sync::Arc;

use buzz_auth::Scope;
use buzz_core::kind::{
    is_project_view_protocol_kind, KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_MUTATION,
    KIND_PROJECT_VIEW_OBJECT,
};
use buzz_core::{StoredEvent, TenantContext};
use buzz_db::project_view::{
    PreparedObjectProjection, PreparedProjectViewCommit, ProjectViewWriteError,
};
use buzz_project_view::{DomainError, Mutation, ProjectViewEntry, ProjectionPlan};
use nostr::{Event, Filter};

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
    channel_ids.is_none() && (scopes.is_empty() || scopes.contains(&Scope::MessagesRead))
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
    if !status.enabled {
        return Err(IngestError::Unavailable(
            "unavailable:project_view:disabled".to_owned(),
        ));
    }
    let relay_pubkey = state.relay_keypair.public_key();
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
}
