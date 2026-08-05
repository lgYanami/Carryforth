//! Project Context Edge v1 Relay protocol adapter.
//!
//! The pure reducer owns Edge semantics, the SDK owns exact wire bytes, and
//! `buzz-db` owns the Community-locked atomic commit. This module supplies
//! transport credentials, operation-aware readiness, stable signing, response
//! construction, and private post-commit fan-out.

use std::{sync::Arc, time::Instant};

#[cfg(test)]
use buzz_core::kind::{
    KIND_PROJECT_CONTEXT_COMMAND, KIND_PROJECT_CONTEXT_EDGE_BINDING, KIND_PROJECT_CONTEXT_META,
};
use buzz_core::{StoredEvent, TenantContext};
use buzz_db::project_context::{
    PreparedProjectContextCommit, ProjectContextPrepareOutcome, ProjectContextWriteError,
};
use buzz_project_context::{
    reduce_project_context, ProjectContextChangeContext, ProjectContextCommand,
    ProjectContextError, ProjectContextOperation, ProjectContextReceipt,
};
use buzz_sdk::project_context::{
    build_project_context_binding_projection, build_project_context_meta_projection,
    changed_project_context_binding_for, parse_project_context_command,
    validate_signed_event_frame_size,
};
use nostr::Event;
use tracing::{info, warn};

use crate::state::AppState;

use super::event::{dispatch_persistent_event_with_options, PersistentDispatchOptions};
use super::ingest::{IngestAuth, IngestError, IngestResult};

/// Apply one exact member command and dispatch its committed private events.
pub(crate) async fn handle_command(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: Event,
    auth: &IngestAuth,
) -> Result<IngestResult, IngestError> {
    let started = Instant::now();
    let telemetry = CommandTelemetry::from_content(&event.content);
    let event_id = event.id.to_hex();
    let actor_pubkey = event.pubkey.to_hex();
    let result = handle_command_inner(tenant, state, event, auth).await;
    let result_code = result_code(&result);

    metrics::counter!(
        "buzz_project_context_commands_total",
        "operation" => telemetry.operation,
        "result" => result_code,
    )
    .increment(1);
    metrics::histogram!(
        "buzz_project_context_command_duration_seconds",
        "operation" => telemetry.operation,
    )
    .record(started.elapsed().as_secs_f64());
    metrics::histogram!("buzz_project_context_coordinate_count")
        .record(telemetry.coordinate_count as f64);
    if matches!(result, Err(IngestError::Conflict(_))) {
        metrics::counter!(
            "buzz_project_context_conflicts_total",
            "operation" => telemetry.operation,
        )
        .increment(1);
    }

    match &result {
        Ok(_) => info!(
            community_host = %tenant.host(),
            command_event_id = %event_id,
            actor_pubkey = %actor_pubkey,
            operation = telemetry.operation,
            expected_context_revision = telemetry.expected_revision,
            result_code,
            "Project Context command completed"
        ),
        Err(_) => warn!(
            community_host = %tenant.host(),
            command_event_id = %event_id,
            actor_pubkey = %actor_pubkey,
            operation = telemetry.operation,
            expected_context_revision = telemetry.expected_revision,
            result_code,
            "Project Context command rejected"
        ),
    }
    result
}

async fn handle_command_inner(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: Event,
    auth: &IngestAuth,
) -> Result<IngestResult, IngestError> {
    // Current-principal checks precede detailed parsing so an unauthorized
    // caller cannot use the closed command parser as a Project oracle. The DB
    // coordinator repeats actor authority under the Community lock.
    if auth.channel_ids().is_some() {
        return Err(IngestError::AuthFailed(
            "restricted:project_context:global_credential_required".to_owned(),
        ));
    }
    if !state
        .db
        .project_context_authorized_pubkey(tenant.community(), auth.pubkey().as_bytes())
        .await
        .map_err(|_| IngestError::Internal("error:project_context:membership_lookup".to_owned()))?
    {
        return Err(IngestError::AuthFailed(
            "restricted:project_context:membership_required".to_owned(),
        ));
    }
    if state.config.relay_private_key.is_none() {
        return Err(IngestError::Unavailable(
            "unavailable:project_context:stable_signer".to_owned(),
        ));
    }
    validate_signed_event_frame_size(&event, state.config.max_frame_bytes)
        .map_err(|_| IngestError::Rejected("invalid:project_context:event_frame".to_owned()))?;

    let command = ProjectContextCommand::from_json(&event.content).map_err(map_domain_error)?;
    let strict = parse_project_context_command(&event, tenant.community()).map_err(|_| {
        IngestError::Rejected("invalid:project_context:invalid_snapshot".to_owned())
    })?;
    if strict != command {
        return Err(IngestError::Rejected(
            "invalid:project_context:invalid_snapshot".to_owned(),
        ));
    }

    let status = state
        .db
        .project_context_status(tenant.community())
        .await
        .map_err(|_| IngestError::Internal("error:project_context:status".to_owned()))?
        .ok_or_else(|| {
            IngestError::Unavailable("unavailable:project_context:not_ready".to_owned())
        })?;
    if status.context_revision.is_none() {
        return Err(IngestError::Unavailable(
            "unavailable:project_context:not_ready".to_owned(),
        ));
    }
    if status.project_view_schema_version != 3 {
        return Err(IngestError::Unsupported(
            "unsupported:project_context:schema".to_owned(),
        ));
    }
    if command.operation() == ProjectContextOperation::Attach && !status.enabled {
        return Err(IngestError::Unavailable(
            "unavailable:project_context:disabled".to_owned(),
        ));
    }

    let relay_pubkey = state.relay_keypair.public_key();
    let mut write = state
        .db
        .begin_project_context_write(tenant.community(), relay_pubkey, command.operation())
        .await
        .map_err(map_write_error)?;
    match write
        .prepare_command(&event, &command)
        .await
        .map_err(map_write_error)?
    {
        ProjectContextPrepareOutcome::Replayed(receipt) => {
            let message = response_message(&receipt)?;
            write.rollback().await.map_err(map_write_error)?;
            return Ok(IngestResult {
                event_id: event.id.to_hex(),
                accepted: true,
                message,
            });
        }
        ProjectContextPrepareOutcome::New => {}
    }

    let context = write
        .load_current(&command)
        .await
        .map_err(map_write_error)?;
    let transition = reduce_project_context(
        &context.catalog,
        context.current_edge.as_ref(),
        context.active_document_edge,
        &command,
        ProjectContextChangeContext::active(event.pubkey, event.id, context.canonical_time)
            .with_coordinates_active(context.all_coordinates_active)
            .with_context_document_active(context.context_document_active),
    )
    .map_err(map_domain_error)?;
    let binding_projection = build_project_context_binding_projection(transition.projection_plan())
        .map_err(|_| projection_failure("binding"))?
        .sign_with_keys(&state.relay_keypair)
        .map_err(|_| projection_failure("binding"))?;
    let changed =
        changed_project_context_binding_for(transition.projection_plan(), &binding_projection)
            .map_err(|_| projection_failure("bundle"))?;
    let meta_projection =
        build_project_context_meta_projection(transition.projection_plan(), &[changed])
            .map_err(|_| projection_failure("meta"))?
            .sign_with_keys(&state.relay_keypair)
            .map_err(|_| projection_failure("meta"))?;
    validate_signed_event_frame_size(&binding_projection, state.config.max_frame_bytes)
        .map_err(|_| projection_failure("binding_frame"))?;
    validate_signed_event_frame_size(&meta_projection, state.config.max_frame_bytes)
        .map_err(|_| projection_failure("meta_frame"))?;

    // Build the exact response before commit. Post-commit work is transport
    // delivery only and cannot turn an accepted command into a rejection.
    let expected_receipt = transition.receipt().clone();
    let response = response_message(&expected_receipt)?;
    let prepared = PreparedProjectContextCommit {
        command_event: event.clone(),
        command,
        transition,
        binding_projection: binding_projection.clone(),
        meta_projection: meta_projection.clone(),
    };
    let committed = write.commit(prepared).await.map_err(map_write_error)?;
    debug_assert_eq!(committed.receipt, expected_receipt);
    if !committed.replayed {
        dispatch_committed_events(tenant, state, &[event, binding_projection, meta_projection])
            .await;
    }
    Ok(IngestResult {
        event_id: committed.receipt.change_id.to_hex(),
        accepted: true,
        message: response,
    })
}

fn response_message(receipt: &ProjectContextReceipt) -> Result<String, IngestError> {
    serde_json::to_string(receipt)
        .map(|json| format!("response:{json}"))
        .map_err(|_| IngestError::Internal("error:project_context:response".to_owned()))
}

fn projection_failure(projection_type: &'static str) -> IngestError {
    metrics::counter!(
        "buzz_project_context_projection_failures_total",
        "projection_type" => projection_type,
    )
    .increment(1);
    IngestError::Internal("error:project_context:projection".to_owned())
}

fn map_domain_error(error: ProjectContextError) -> IngestError {
    let code = error.code();
    match error {
        ProjectContextError::UnsupportedSchemaVersion { .. } => {
            IngestError::Unsupported(format!("unsupported:project_context:{code}"))
        }
        ProjectContextError::RevisionConflict { .. }
        | ProjectContextError::DocumentAlreadyBound { .. }
        | ProjectContextError::BindingNotFound { .. }
        | ProjectContextError::BindingEdgeMismatch { .. } => {
            IngestError::Conflict(format!("conflict:project_context:{code}"))
        }
        ProjectContextError::InvalidCanonicalState { .. } => {
            IngestError::Internal("error:project_context:canonical_state".to_owned())
        }
        _ => IngestError::Rejected(format!("invalid:project_context:{code}")),
    }
}

fn map_write_error(error: ProjectContextWriteError) -> IngestError {
    if matches!(
        error,
        ProjectContextWriteError::Database(_)
            | ProjectContextWriteError::Sqlx(_)
            | ProjectContextWriteError::Audit(_)
            | ProjectContextWriteError::InvalidCommit(_)
    ) {
        warn!(error = %error, "Project Context storage coordinator failed");
    }
    match error {
        ProjectContextWriteError::Unavailable { .. } => {
            IngestError::Unavailable("unavailable:project_context:not_ready".to_owned())
        }
        ProjectContextWriteError::NotAuthorized => {
            IngestError::AuthFailed("restricted:project_context:not_authorized".to_owned())
        }
        ProjectContextWriteError::ActingAssignmentInvalid => {
            IngestError::Conflict("conflict:project_context:acting_assignment".to_owned())
        }
        ProjectContextWriteError::RuntimeFence => {
            IngestError::AuthFailed("restricted:project_context:runtime_fence".to_owned())
        }
        ProjectContextWriteError::Domain(error) => map_domain_error(error),
        ProjectContextWriteError::Database(_)
        | ProjectContextWriteError::Sqlx(_)
        | ProjectContextWriteError::Audit(_) => {
            IngestError::Internal("error:project_context:database".to_owned())
        }
        ProjectContextWriteError::InvalidCommit(_) => {
            IngestError::Internal("error:project_context:invalid_commit".to_owned())
        }
    }
}

async fn dispatch_committed_events(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    events: &[Event],
) {
    let options = PersistentDispatchOptions {
        audit: false,
        workflow: false,
    };
    for event in events {
        let kind = u32::from(event.kind.as_u16());
        let stored = StoredEvent::new(event.clone(), None);
        dispatch_persistent_event_with_options(
            tenant,
            state,
            &stored,
            kind,
            &event.pubkey.to_hex(),
            None,
            options,
        )
        .await;
    }
}

fn result_code(result: &Result<IngestResult, IngestError>) -> &'static str {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CommandTelemetry {
    operation: &'static str,
    expected_revision: Option<u64>,
    coordinate_count: usize,
}

impl CommandTelemetry {
    fn from_content(content: &str) -> Self {
        let parsed = serde_json::from_str::<serde_json::Value>(content).ok();
        let operation = parsed
            .as_ref()
            .and_then(|value| value.get("request"))
            .and_then(|value| value.get("type"))
            .and_then(serde_json::Value::as_str)
            .map(|value| match value {
                "attach" => "attach",
                "detach" => "detach",
                _ => "unknown",
            })
            .unwrap_or("unknown");
        let expected_revision = parsed
            .as_ref()
            .and_then(|value| value.get("expected_context_revision"))
            .and_then(serde_json::Value::as_u64);
        let coordinate_count = parsed
            .as_ref()
            .and_then(|value| value.get("request"))
            .and_then(|value| value.get("coordinates"))
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len);
        Self {
            operation,
            expected_revision,
            coordinate_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_is_bounded_to_shape_and_counts() {
        let telemetry = CommandTelemetry::from_content(
            r#"{"expected_context_revision":7,"request":{"type":"attach","coordinates":[{"secret":"not retained"},{"secret":"not retained"}]}}"#,
        );
        assert_eq!(telemetry.operation, "attach");
        assert_eq!(telemetry.expected_revision, Some(7));
        assert_eq!(telemetry.coordinate_count, 2);
        assert_eq!(CommandTelemetry::from_content("{").operation, "unknown");
    }

    #[test]
    fn stable_domain_classes_match_the_context_contract() {
        assert!(matches!(
            map_domain_error(ProjectContextError::RevisionConflict {
                expected: 1,
                actual: 2,
            }),
            IngestError::Conflict(_)
        ));
        assert!(matches!(
            map_domain_error(ProjectContextError::NoChange),
            IngestError::Rejected(message) if message == "invalid:project_context:no_change"
        ));
    }

    #[test]
    fn writer_authority_failures_keep_distinct_stable_classes() {
        assert!(matches!(
            map_write_error(ProjectContextWriteError::NotAuthorized),
            IngestError::AuthFailed(message)
                if message == "restricted:project_context:not_authorized"
        ));
        assert!(matches!(
            map_write_error(ProjectContextWriteError::ActingAssignmentInvalid),
            IngestError::Conflict(message)
                if message == "conflict:project_context:acting_assignment"
        ));
        assert!(matches!(
            map_write_error(ProjectContextWriteError::RuntimeFence),
            IngestError::AuthFailed(message)
                if message == "restricted:project_context:runtime_fence"
        ));
    }

    #[test]
    fn context_kinds_are_dispatched_by_the_adapter() {
        assert_eq!(KIND_PROJECT_CONTEXT_COMMAND, 44302);
        assert_eq!(KIND_PROJECT_CONTEXT_EDGE_BINDING, 40908);
        assert_eq!(KIND_PROJECT_CONTEXT_META, 40909);
        assert_eq!(ProjectContextOperation::Attach.as_str(), "attach");
    }
}
