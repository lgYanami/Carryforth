//! Project Document v1 Relay protocol adapter.
//!
//! The pure reducer owns lifecycle semantics, the SDK owns exact wire bytes,
//! and `buzz-db` owns the Community-locked atomic commit. This module supplies
//! transport credentials, current-principal checks, stable signing, error
//! classes, pre-commit response construction, and private post-commit fan-out.

use std::{sync::Arc, time::Instant};

#[cfg(test)]
use buzz_core::kind::{
    KIND_PROJECT_DOCUMENT_COMMAND, KIND_PROJECT_DOCUMENT_HEAD, KIND_PROJECT_DOCUMENT_META,
    KIND_PROJECT_DOCUMENT_REVISION,
};
use buzz_core::{StoredEvent, TenantContext};
use buzz_db::project_document::{
    PreparedProjectDocumentCommit, ProjectDocumentPrepareOutcome, ProjectDocumentWriteError,
};
#[cfg(test)]
use buzz_project_document::DocumentOperation;
use buzz_project_document::{
    reduce_document, DocumentChangeContext, DocumentError, ProjectDocumentCommand,
    ProjectDocumentReceipt,
};
use buzz_sdk::project_document::{
    build_document_head_projection, build_document_meta_projection,
    build_document_revision_projection, changed_head_for, parse_document_command,
};
use nostr::Event;
use tracing::{info, warn};

use crate::state::AppState;

use super::event::{dispatch_persistent_event_with_options, PersistentDispatchOptions};
use super::ingest::{IngestAuth, IngestError, IngestResult};

/// Apply one exact member command and schedule its committed private events.
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
    let body_bytes = telemetry.body_bytes;
    let result = handle_command_inner(tenant, state, event, auth).await;
    let result_code = result_code(&result);

    metrics::counter!(
        "buzz_project_document_commands_total",
        "operation" => telemetry.operation,
        "result" => result_code,
    )
    .increment(1);
    metrics::histogram!(
        "buzz_project_document_command_duration_seconds",
        "operation" => telemetry.operation,
    )
    .record(started.elapsed().as_secs_f64());
    metrics::histogram!("buzz_project_document_body_bytes").record(body_bytes as f64);
    if matches!(result, Err(IngestError::Conflict(_))) {
        metrics::counter!(
            "buzz_project_document_conflicts_total",
            "operation" => telemetry.operation,
            "reason" => "canonical",
        )
        .increment(1);
    }

    match &result {
        Ok(_) => info!(
            community_host = %tenant.host(),
            command_event_id = %event_id,
            actor_pubkey = %actor_pubkey,
            operation = telemetry.operation,
            expected_document_revision = telemetry.expected_revision,
            result_code,
            "Project Document command completed"
        ),
        Err(_) => warn!(
            community_host = %tenant.host(),
            command_event_id = %event_id,
            actor_pubkey = %actor_pubkey,
            operation = telemetry.operation,
            expected_document_revision = telemetry.expected_revision,
            result_code,
            "Project Document command rejected"
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
    // Principal and capability checks deliberately precede detailed body
    // validation so an unauthorized caller cannot use parser errors as an
    // oracle. The DB coordinator repeats current authority under its lock
    // before consulting a durable receipt.
    if auth.channel_ids().is_some() {
        return Err(IngestError::AuthFailed(
            "restricted:project_document:global_credential_required".to_owned(),
        ));
    }
    if !state
        .db
        .project_document_authorized_pubkey(tenant.community(), auth.pubkey().as_bytes())
        .await
        .map_err(|_| IngestError::Internal("error:project_document:membership_lookup".to_owned()))?
    {
        return Err(IngestError::AuthFailed(
            "restricted:project_document:membership_required".to_owned(),
        ));
    }
    if state.config.relay_private_key.is_none() {
        return Err(IngestError::Unavailable(
            "unavailable:project_document:stable_signer".to_owned(),
        ));
    }
    let relay_pubkey = state.relay_keypair.public_key();
    let status = state
        .db
        .project_document_status(tenant.community())
        .await
        .map_err(|_| IngestError::Internal("error:project_document:status".to_owned()))?
        .ok_or_else(|| {
            IngestError::Unavailable("unavailable:project_document:not_ready".to_owned())
        })?;
    if !status.enabled {
        return Err(IngestError::Unavailable(
            "unavailable:project_document:disabled".to_owned(),
        ));
    }
    if !matches!(status.project_view_schema_version, 2 | 3) {
        return Err(IngestError::Unsupported(
            "unsupported:project_document:schema".to_owned(),
        ));
    }
    if !state
        .db
        .project_document_capability_ready(tenant.community(), &relay_pubkey)
        .await
        .map_err(|_| IngestError::Internal("error:project_document:readiness".to_owned()))?
    {
        return Err(IngestError::Unavailable(
            "unavailable:project_document:not_ready".to_owned(),
        ));
    }

    // Retain the domain's stable low-cardinality reason, then enforce the
    // SDK's signature/kind/exact-tag/canonical-JSON contract.
    let command = ProjectDocumentCommand::from_json(&event.content).map_err(map_domain_error)?;
    let strict = parse_document_command(&event).map_err(|_| {
        IngestError::Rejected("invalid:project_document:invalid_snapshot".to_owned())
    })?;
    if strict != command {
        return Err(IngestError::Rejected(
            "invalid:project_document:invalid_snapshot".to_owned(),
        ));
    }

    let mut write = state
        .db
        .begin_project_document_write(tenant.community(), relay_pubkey)
        .await
        .map_err(map_write_error)?;
    match write
        .prepare_command(&event, &command)
        .await
        .map_err(map_write_error)?
    {
        ProjectDocumentPrepareOutcome::Replayed(receipt) => {
            let message = response_message(&receipt)?;
            write.rollback().await.map_err(map_write_error)?;
            return Ok(IngestResult {
                event_id: event.id.to_hex(),
                accepted: true,
                message,
            });
        }
        ProjectDocumentPrepareOutcome::New => {}
    }

    let context = write
        .load_current(command.document_id())
        .await
        .map_err(map_write_error)?;
    let transition = reduce_document(
        &context.catalog,
        context.current.as_ref(),
        &command,
        DocumentChangeContext::new(event.pubkey, event.id, context.canonical_time)
            .with_deletion_blocked(context.deletion_blocked),
    )
    .map_err(map_domain_error)?;
    let revision_projection = build_document_revision_projection(transition.projection_plan())
        .map_err(|_| projection_failure("revision"))?
        .sign_with_keys(&state.relay_keypair)
        .map_err(|_| projection_failure("revision"))?;
    let head_projection =
        build_document_head_projection(transition.projection_plan(), &revision_projection)
            .map_err(|_| projection_failure("head"))?
            .sign_with_keys(&state.relay_keypair)
            .map_err(|_| projection_failure("head"))?;
    let changed = changed_head_for(
        transition.projection_plan(),
        &head_projection,
        &revision_projection,
    )
    .map_err(|_| projection_failure("bundle"))?;
    let meta_projection = build_document_meta_projection(transition.projection_plan(), &[changed])
        .map_err(|_| projection_failure("meta"))?
        .sign_with_keys(&state.relay_keypair)
        .map_err(|_| projection_failure("meta"))?;

    // The exact response bytes are constructed before commit. Everything
    // after commit is transport delivery only and cannot manufacture a stable
    // protocol-internal rejection for an already accepted command.
    let expected_receipt = transition.receipt().clone();
    let response = response_message(&expected_receipt)?;
    let prepared = PreparedProjectDocumentCommit {
        command_event: event.clone(),
        command,
        transition,
        revision_projection: revision_projection.clone(),
        head_projection: head_projection.clone(),
        meta_projection: meta_projection.clone(),
    };
    let committed = write.commit(prepared).await.map_err(map_write_error)?;
    // The database validates the same receipt as part of the commit. Retain a
    // debug assertion without introducing any fallible work after acceptance.
    debug_assert_eq!(committed.receipt, expected_receipt);
    if !committed.replayed {
        dispatch_committed_events(
            tenant,
            state,
            &[event, revision_projection, head_projection, meta_projection],
        )
        .await;
    }
    Ok(IngestResult {
        event_id: committed.receipt.change_id.to_hex(),
        accepted: true,
        message: response,
    })
}

fn response_message(receipt: &ProjectDocumentReceipt) -> Result<String, IngestError> {
    serde_json::to_string(receipt)
        .map(|json| format!("response:{json}"))
        .map_err(|_| IngestError::Internal("error:project_document:response".to_owned()))
}

fn projection_failure(projection_type: &'static str) -> IngestError {
    metrics::counter!(
        "buzz_project_document_projection_failures_total",
        "projection_type" => projection_type,
    )
    .increment(1);
    IngestError::Internal("error:project_document:projection".to_owned())
}

fn map_domain_error(error: DocumentError) -> IngestError {
    let code = error.code();
    match error {
        DocumentError::UnsupportedSchemaVersion { .. } => {
            IngestError::Unsupported(format!("unsupported:project_document:{code}"))
        }
        DocumentError::RevisionConflict { .. }
        | DocumentError::DocumentIdAlreadyExists { .. }
        | DocumentError::StillReferenced { .. } => {
            IngestError::Conflict(format!("conflict:project_document:{code}"))
        }
        _ => IngestError::Rejected(format!("invalid:project_document:{code}")),
    }
}

fn map_write_error(error: ProjectDocumentWriteError) -> IngestError {
    match error {
        ProjectDocumentWriteError::Unavailable { .. } => {
            IngestError::Unavailable("unavailable:project_document:not_ready".to_owned())
        }
        ProjectDocumentWriteError::Unauthorized => {
            IngestError::AuthFailed("restricted:project_document:runtime_fence".to_owned())
        }
        ProjectDocumentWriteError::Domain(error) => map_domain_error(error),
        ProjectDocumentWriteError::Database(_)
        | ProjectDocumentWriteError::Sqlx(_)
        | ProjectDocumentWriteError::Audit(_) => {
            IngestError::Internal("error:project_document:database".to_owned())
        }
        ProjectDocumentWriteError::InvalidCommit(_) => {
            IngestError::Internal("error:project_document:invalid_commit".to_owned())
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
    body_bytes: usize,
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
                "create" => "create",
                "update" => "update",
                "delete" => "delete",
                _ => "unknown",
            })
            .unwrap_or("unknown");
        let expected_revision = parsed
            .as_ref()
            .and_then(|value| value.get("expected_document_revision"))
            .and_then(serde_json::Value::as_u64);
        let body_bytes = parsed
            .as_ref()
            .and_then(|value| value.get("request"))
            .and_then(|value| value.get("content_markdown"))
            .and_then(serde_json::Value::as_str)
            .map_or(0, str::len);
        Self {
            operation,
            expected_revision,
            body_bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_is_closed_and_never_retains_document_text() {
        let telemetry = CommandTelemetry::from_content(
            r#"{"expected_document_revision":7,"request":{"type":"update","title":"private","content_markdown":"secret body"}}"#,
        );
        assert_eq!(telemetry.operation, "update");
        assert_eq!(telemetry.expected_revision, Some(7));
        assert_eq!(telemetry.body_bytes, 11);
        assert_eq!(CommandTelemetry::from_content("{").operation, "unknown");
    }

    #[test]
    fn stable_domain_classes_match_the_document_contract() {
        assert!(matches!(
            map_domain_error(DocumentError::RevisionConflict {
                expected: 1,
                actual: Some(2),
            }),
            IngestError::Conflict(_)
        ));
        assert!(matches!(
            map_domain_error(DocumentError::NoChange),
            IngestError::Rejected(_)
        ));
    }

    #[test]
    fn document_kinds_are_all_dispatched_by_the_adapter() {
        assert_eq!(KIND_PROJECT_DOCUMENT_COMMAND, 44301);
        assert_eq!(KIND_PROJECT_DOCUMENT_HEAD, 40905);
        assert_eq!(KIND_PROJECT_DOCUMENT_REVISION, 40906);
        assert_eq!(KIND_PROJECT_DOCUMENT_META, 40907);
        assert_eq!(DocumentOperation::Create.as_str(), "create");
    }
}
