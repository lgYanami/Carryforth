//! Trusted managed-runtime supervision HTTP surface.
//!
//! Supervisor evidence is tenant-bound and NIP-98 authenticated, but remains
//! operational state outside the Project revision/event stream. Only the final
//! policy-fenced system action emits Project View projections.

use std::sync::Arc;

use axum::{
    extract::{Query, RawQuery, State},
    http::{HeaderMap, StatusCode},
    response::Json,
};
use buzz_project_view::v2::RuntimeEvidenceRequest;
use buzz_project_view::v3::MaintenanceAckCommand;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::state::AppState;

use super::{api_error, bridge, internal_error};

/// Assignment selector for the operational runtime status read.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeStatusQuery {
    assignment_id: Uuid,
}

/// Optional exact-epoch selector and monotonic supervisor poll diagnostics.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceStatusQuery {
    epoch: Option<u64>,
    client_protocol_version: Option<u64>,
    client_build: Option<String>,
}

/// Record one immutable observation from an operator-registered supervisor.
pub async fn record_evidence(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    const PATH: &str = "/api/project-runtime/evidence";
    let (tenant, supervisor, auth_event_id) =
        authenticate_tenant_request(&state, &headers, "POST", PATH, None, Some(&body)).await?;
    let request: RuntimeEvidenceRequest = serde_json::from_slice(&body).map_err(|error| {
        api_error(
            StatusCode::BAD_REQUEST,
            &format!("invalid runtime evidence JSON: {error}"),
        )
    })?;
    let evidence_type = request.evidence.as_str();
    let receipt = state
        .db
        .record_runtime_evidence(tenant.community(), supervisor, auth_event_id, &request)
        .await
        .map_err(map_runtime_error)?;
    metrics::counter!(
        "buzz_project_runtime_evidence_total",
        "community" => tenant.host().to_owned(),
        "evidence_type" => evidence_type.to_owned(),
        "availability" => receipt.availability.as_str()
    )
    .increment(1);
    let recovery_result = match evidence_type {
        "abnormal_exit" => Some("recovering"),
        "recovery_attempt" => Some("attempted"),
        "recovery_succeeded" => Some("recovered"),
        "recovery_failed" if receipt.availability.as_str() == "unavailable" => Some("exhausted"),
        "recovery_failed" => Some("attempt_failed"),
        _ => None,
    };
    if let Some(result) = recovery_result {
        metrics::counter!("buzz_role_runtime_recovery_total", "result" => result).increment(1);
    }
    Ok(Json(serde_json::to_value(receipt).map_err(|error| {
        internal_error(&format!("serialize runtime evidence receipt: {error}"))
    })?))
}

/// Return current operational availability to an authorized Project member.
pub async fn assignment_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
    Query(query): Query<RuntimeStatusQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    const PATH: &str = "/api/project-runtime/status";
    let (tenant, actor, _) =
        authenticate_tenant_request(&state, &headers, "GET", PATH, raw_query.as_deref(), None)
            .await?;
    if !state
        .db
        .project_view_authorized_pubkey(tenant.community(), actor.as_bytes())
        .await
        .map_err(|error| internal_error(&format!("authorize runtime status: {error}")))?
    {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "actor is not authorized to read this Project View",
        ));
    }
    let status = state
        .db
        .assignment_runtime_status(tenant.community(), query.assignment_id)
        .await
        .map_err(map_runtime_error)?;
    Ok(Json(serde_json::to_value(status).map_err(|error| {
        internal_error(&format!("serialize runtime status: {error}"))
    })?))
}

/// Return this authenticated supervisor's exact durable drain baselines and
/// ACK receipts. Project View capability availability is intentionally not a
/// prerequisite: supervisors must be able to drain while it is hidden.
pub async fn maintenance_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
    Query(query): Query<MaintenanceStatusQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    const PATH: &str = "/api/project-runtime/maintenance";
    let (tenant, supervisor, _) =
        authenticate_tenant_request(&state, &headers, "GET", PATH, raw_query.as_deref(), None)
            .await?;
    let status = state
        .db
        .project_view_maintenance_supervisor_status(
            tenant.community(),
            supervisor,
            query.epoch,
            query.client_protocol_version,
            query.client_build.as_deref(),
        )
        .await
        .map_err(map_maintenance_error)?;
    Ok(Json(status))
}

/// Commit one exact Assignment or Runtime maintenance ACK from its registered
/// supervisor. The NIP-98 auth event ID is retained as immutable provenance.
pub async fn acknowledge_maintenance(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    const PATH: &str = "/api/project-runtime/maintenance/ack";
    let (tenant, supervisor, auth_event_id) =
        authenticate_tenant_request(&state, &headers, "POST", PATH, None, Some(&body)).await?;
    let content = std::str::from_utf8(&body).map_err(|_| {
        api_error(
            StatusCode::BAD_REQUEST,
            "maintenance ACK body must be UTF-8",
        )
    })?;
    let command = MaintenanceAckCommand::from_json(content)
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, &error.to_string()))?;
    let receipt = state
        .db
        .acknowledge_project_view_maintenance(
            tenant.community(),
            supervisor,
            auth_event_id,
            &command,
        )
        .await
        .map_err(map_maintenance_error)?;
    Ok(Json(serde_json::to_value(receipt).map_err(|error| {
        internal_error(&format!("serialize maintenance ACK receipt: {error}"))
    })?))
}

async fn authenticate_tenant_request(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    method: &str,
    path: &str,
    raw_query: Option<&str>,
    body: Option<&[u8]>,
) -> Result<(buzz_core::TenantContext, nostr::PublicKey, [u8; 32]), (StatusCode, Json<Value>)> {
    let raw_host = headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let tenant = crate::tenant::bind_community(&state.db, raw_host)
        .await
        .map_err(|error| crate::api::host_lookup_api_error(&error))?;
    let path_with_query = match raw_query {
        Some(query) if !query.is_empty() => format!("{path}?{query}"),
        _ => path.to_owned(),
    };
    let url = bridge::nip98_expected_url(&state.config.relay_url, &tenant, &path_with_query);
    let (pubkey, auth_event_id) =
        bridge::verify_bridge_auth_with_options(headers, method, &url, body, true, body.is_some())?;
    bridge::check_nip98_replay(state, &tenant, auth_event_id).await?;
    Ok((tenant, pubkey, auth_event_id))
}

fn map_runtime_error(
    error: buzz_db::project_runtime::RuntimeSupervisionError,
) -> (StatusCode, Json<Value>) {
    use buzz_db::project_runtime::RuntimeSupervisionError;

    let message = error.to_string();
    match error {
        RuntimeSupervisionError::Invalid(_) => api_error(StatusCode::BAD_REQUEST, &message),
        RuntimeSupervisionError::NotRegistered => api_error(
            StatusCode::FORBIDDEN,
            "runtime supervisor is not registered",
        ),
        RuntimeSupervisionError::AssignmentEnded | RuntimeSupervisionError::BindingConflict => {
            api_error(StatusCode::CONFLICT, &message)
        }
        RuntimeSupervisionError::StaleEpoch | RuntimeSupervisionError::CommandFence => {
            api_error(StatusCode::CONFLICT, &message)
        }
        RuntimeSupervisionError::Database(_)
        | RuntimeSupervisionError::Sqlx(_)
        | RuntimeSupervisionError::Audit(_) => {
            internal_error(&format!("runtime supervision: {message}"))
        }
    }
}

fn map_maintenance_error(
    error: buzz_db::project_view_maintenance::ProjectViewMaintenanceError,
) -> (StatusCode, Json<Value>) {
    use buzz_db::project_view_maintenance::ProjectViewMaintenanceError;

    let message = error.to_string();
    match error {
        ProjectViewMaintenanceError::Invalid(_) => api_error(StatusCode::BAD_REQUEST, &message),
        ProjectViewMaintenanceError::Forbidden(_) => api_error(StatusCode::FORBIDDEN, &message),
        ProjectViewMaintenanceError::Conflict(_) => api_error(StatusCode::CONFLICT, &message),
        ProjectViewMaintenanceError::Unavailable(_) => {
            api_error(StatusCode::SERVICE_UNAVAILABLE, &message)
        }
        ProjectViewMaintenanceError::Database(_)
        | ProjectViewMaintenanceError::Sqlx(_)
        | ProjectViewMaintenanceError::Audit(_) => {
            internal_error(&format!("Project View maintenance: {message}"))
        }
    }
}
