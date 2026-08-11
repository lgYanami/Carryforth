//! Trusted one-shot semantic Project Context query boundary for Desktop.

use std::collections::BTreeMap;
use std::time::Duration;

use buzz_core_pkg::CommunityId;
use buzz_project_context_pkg::ProjectContextCoordinate;
use buzz_semantic_query_pkg::{
    BranchStopReason, CompletionReason, ExhaustedDimension, LifecycleFilter,
    OmittedContextCoordinateReason, OmittedInitialCoordinateReason, RootStructuralEntrypoint,
    SemanticGraphQuery, SemanticGraphQueryBudget, SemanticGraphQueryResult,
    MAX_CONTEXT_COORDINATES, MAX_INITIAL_COORDINATES, MAX_PROBLEM_BYTES,
};
use futures_util::StreamExt;
use nostr::Event;
use reqwest::{Method, StatusCode};
use serde_json::Value;
use tauri::State;
use uuid::Uuid;

use super::{coordinate_key, domain_coordinate, ProjectContextCoordinateDto};
use crate::app_state::{AppState, AppliedWorkspaceCapture, AppliedWorkspaceCaptureError};
use crate::commands::project_view::{
    read_identity_at_with_client, read_verified_v3_meta_at_with_client, ProjectViewReadError,
    ProjectViewSchema,
};
use crate::relay::build_nip98_auth_observation_for_keys;

const SEMANTIC_QUERY_TIMEOUT: Duration = Duration::from_secs(45);
const SEMANTIC_QUERY_ERROR_RESPONSE_BYTES: u64 = 16 * 1024;

#[path = "semantic_model.rs"]
mod model;
use model::{
    SemanticContextDocumentEntrypoint, SemanticCoordinateInputOutcome,
    SemanticProjectContextCoverage, SemanticProjectContextHop, SemanticProjectContextPath,
    SemanticProjectContextRoot, SemanticQueryInputOutcomes, SemanticResponseBudgetOmissions,
};
pub use model::{
    SemanticProjectContextQueryError, SemanticProjectContextQueryInput,
    SemanticProjectContextQueryResult,
};

/// Execute one exact authenticated semantic Project Context query.
#[tauri::command]
pub async fn query_project_context_semantic(
    input: SemanticProjectContextQueryInput,
    state: State<'_, AppState>,
) -> Result<SemanticProjectContextQueryResult, SemanticProjectContextQueryError> {
    query_project_context_semantic_inner(input, &state).await
}

async fn query_project_context_semantic_inner(
    input: SemanticProjectContextQueryInput,
    state: &AppState,
) -> Result<SemanticProjectContextQueryResult, SemanticProjectContextQueryError> {
    let submitted = validate_input(input)?;
    let workspace = state
        .capture_applied_workspace(&submitted.community_key, &submitted.applied_workspace_token)
        .map_err(map_workspace_capture_error)?;

    // Capability is observed afresh for every explicit run, over the pinned
    // no-redirect origin. The problem is not sent when this check fails.
    let identity = read_identity_at_with_client(
        &workspace.relay_http_origin,
        &state.semantic_query_http_client,
    )
    .await
    .map_err(|message| map_identity_error(&message))?
    .ok_or_else(SemanticProjectContextQueryError::unsupported)?;
    if identity.schema != ProjectViewSchema::V3
        || !identity.runtime_ready
        || !identity.semantic_query_http_available
    {
        return Err(SemanticProjectContextQueryError::unsupported());
    }

    let project_view_meta = read_verified_v3_meta_at_with_client(
        identity,
        &workspace.relay_http_origin,
        &workspace.keys,
        &state.semantic_query_http_client,
    )
    .await
    .map_err(map_project_view_error)?
    .ok_or_else(|| SemanticProjectContextQueryError::unavailable(None))?;
    let project_id = *project_view_meta.project_id.as_uuid();

    let request = SemanticGraphQuery {
        request_id: Uuid::new_v4(),
        project_id,
        problem: submitted.problem,
        initial_coordinates: submitted.initial_coordinates,
        context_coordinates: submitted.context_coordinates,
        lifecycle_filter: LifecycleFilter::AllCurrent,
        budget: SemanticGraphQueryBudget::default(),
    };
    let prepared = buzz_sdk_pkg::semantic_graph::build_semantic_graph_http_query_request(
        request,
        &identity.relay_pubkey,
        &workspace.caller,
    )
    .map_err(|error| match error {
        buzz_sdk_pkg::SdkError::InvalidInput(_) => {
            SemanticProjectContextQueryError::invalid_input()
        }
        _ => SemanticProjectContextQueryError::internal(),
    })?;

    crate::relay_admission::wait_for_rate_limit().await;
    let query_url = format!(
        "{}/query",
        workspace.relay_http_origin.trim_end_matches('/')
    );
    let authorization: crate::relay::Nip98AuthorizationObservation =
        build_nip98_auth_observation_for_keys(
            &workspace.keys,
            &Method::POST,
            &query_url,
            &prepared.exact_body,
        )
        .map_err(|_| SemanticProjectContextQueryError::internal())?;

    let response = state
        .semantic_query_http_client
        .post(query_url)
        .timeout(SEMANTIC_QUERY_TIMEOUT)
        .header("Authorization", authorization.authorization_header)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(prepared.exact_body.clone())
        .send()
        .await
        .map_err(map_request_error)?;
    let status = response.status();
    let limit = if status.is_success() {
        u64::from(prepared.request.budget.max_response_bytes)
    } else {
        SEMANTIC_QUERY_ERROR_RESPONSE_BYTES
    };
    let response_body =
        read_bounded_response(response, limit)
            .await
            .map_err(|error| match error {
                BoundedResponseError::TooLarge => {
                    SemanticProjectContextQueryError::too_large(Some(status.as_u16()))
                }
                BoundedResponseError::Network(error) => map_request_error(error),
            })?;
    if !status.is_success() {
        return Err(map_http_status(status, &response_body));
    }

    let event = parse_single_exact_event(&response_body)?;
    let verified = buzz_sdk_pkg::semantic_graph::parse_semantic_graph_query_result(
        &event,
        &identity.relay_pubkey,
        buzz_sdk_pkg::semantic_graph::SemanticGraphHttpRequestObservation {
            project_id: CommunityId::from_uuid(project_id),
            authenticated_caller: workspace.caller,
            request: &prepared.request,
            nip98_auth_event_id: authorization.auth_event_id,
            exact_authenticated_body: &prepared.exact_body,
        },
    )
    .map_err(|_| SemanticProjectContextQueryError::verification_failed())?;

    map_display_result(workspace, identity.relay_pubkey.to_hex(), verified)
}

struct ValidatedInput {
    community_key: String,
    applied_workspace_token: String,
    problem: String,
    initial_coordinates: Vec<ProjectContextCoordinate>,
    context_coordinates: Vec<ProjectContextCoordinate>,
}

fn validate_input(
    input: SemanticProjectContextQueryInput,
) -> Result<ValidatedInput, SemanticProjectContextQueryError> {
    if input.community_key.trim().is_empty()
        || input.community_key.as_bytes().contains(&0)
        || input.community_key.len() > 1024
        || input.applied_workspace_token.trim().is_empty()
        || input.applied_workspace_token.as_bytes().contains(&0)
        || input.applied_workspace_token.len() > 128
    {
        return Err(SemanticProjectContextQueryError::invalid_input());
    }
    let problem = input.problem.trim();
    if problem.is_empty()
        || problem.as_bytes().contains(&0)
        || problem.len() > MAX_PROBLEM_BYTES
        || input.initial_coordinates.len() > MAX_INITIAL_COORDINATES
        || input.context_coordinates.len() > MAX_CONTEXT_COORDINATES
    {
        return Err(SemanticProjectContextQueryError::invalid_input());
    }
    let initial_coordinates = input
        .initial_coordinates
        .into_iter()
        .map(domain_coordinate)
        .collect::<Vec<_>>();
    let context_coordinates = input
        .context_coordinates
        .into_iter()
        .map(domain_coordinate)
        .collect::<Vec<_>>();
    if initial_coordinates
        .iter()
        .chain(&context_coordinates)
        .any(|coordinate| coordinate.validate().is_err())
    {
        return Err(SemanticProjectContextQueryError::invalid_input());
    }
    Ok(ValidatedInput {
        community_key: input.community_key,
        applied_workspace_token: input.applied_workspace_token,
        problem: problem.to_owned(),
        initial_coordinates,
        context_coordinates,
    })
}

fn map_workspace_capture_error(
    error: AppliedWorkspaceCaptureError,
) -> SemanticProjectContextQueryError {
    match error {
        AppliedWorkspaceCaptureError::NotApplied | AppliedWorkspaceCaptureError::Mismatch => {
            SemanticProjectContextQueryError::conflict(None)
        }
        AppliedWorkspaceCaptureError::KeyringLocked
        | AppliedWorkspaceCaptureError::IdentityLost
        | AppliedWorkspaceCaptureError::ResetFailed
        | AppliedWorkspaceCaptureError::StateUnavailable => {
            SemanticProjectContextQueryError::internal()
        }
    }
}

fn map_identity_error(message: &str) -> SemanticProjectContextQueryError {
    if message.starts_with("relay rate-limited:") {
        SemanticProjectContextQueryError::busy(parse_retry_hint(message))
    } else if message.starts_with("relay returned 401") {
        SemanticProjectContextQueryError::restricted(401)
    } else if message.starts_with("relay returned 403") {
        SemanticProjectContextQueryError::restricted(403)
    } else if message.starts_with("relay returned 409") {
        SemanticProjectContextQueryError::conflict(Some(409))
    } else if message.starts_with("relay returned 504") {
        SemanticProjectContextQueryError::timeout(Some(504))
    } else if message.starts_with("relay unreachable:") || message.starts_with("relay returned 5") {
        SemanticProjectContextQueryError::unavailable(None)
    } else if message.starts_with("Project View integrity error:") {
        SemanticProjectContextQueryError::verification_failed()
    } else {
        SemanticProjectContextQueryError::internal()
    }
}

fn map_project_view_error(error: ProjectViewReadError) -> SemanticProjectContextQueryError {
    match error {
        ProjectViewReadError::Forbidden => SemanticProjectContextQueryError::restricted(403),
        ProjectViewReadError::Conflict(_) => SemanticProjectContextQueryError::conflict(Some(409)),
        ProjectViewReadError::Unavailable(message)
            if message.starts_with("relay rate-limited:") =>
        {
            SemanticProjectContextQueryError::busy(parse_retry_hint(&message))
        }
        ProjectViewReadError::Unavailable(message) if message.contains("timed out") => {
            SemanticProjectContextQueryError::timeout(None)
        }
        ProjectViewReadError::Unavailable(_) => SemanticProjectContextQueryError::unavailable(None),
        ProjectViewReadError::Other(_) => SemanticProjectContextQueryError::verification_failed(),
    }
}

fn map_request_error(error: reqwest::Error) -> SemanticProjectContextQueryError {
    if error.is_timeout() {
        SemanticProjectContextQueryError::timeout(None)
    } else {
        SemanticProjectContextQueryError::unavailable(None)
    }
}

fn map_http_status(status: StatusCode, body: &[u8]) -> SemanticProjectContextQueryError {
    match status.as_u16() {
        400 => {
            let mut error = SemanticProjectContextQueryError::invalid_input();
            error.status = Some(400);
            error
        }
        401 | 403 => SemanticProjectContextQueryError::restricted(status.as_u16()),
        409 => SemanticProjectContextQueryError::conflict(Some(409)),
        413 => SemanticProjectContextQueryError::too_large(Some(413)),
        429 => {
            let hint = parse_retry_hint(&String::from_utf8_lossy(body))
                .map(|seconds| seconds.min(crate::relay_admission::MAX_HINT_SECONDS));
            crate::relay_admission::activate_rate_limit(hint);
            SemanticProjectContextQueryError::busy(hint)
        }
        504 => SemanticProjectContextQueryError::timeout(Some(504)),
        500 | 502 | 503 => SemanticProjectContextQueryError::unavailable(Some(status.as_u16())),
        _ if status.is_redirection() => SemanticProjectContextQueryError::verification_failed(),
        _ => SemanticProjectContextQueryError::unavailable(Some(status.as_u16())),
    }
}

fn parse_retry_hint(text: &str) -> Option<u64> {
    let after = text.split_once("retry in ")?.1;
    let digits = after
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    if digits.is_empty() || !after[digits.len()..].starts_with('s') {
        return None;
    }
    digits.parse().ok()
}

enum BoundedResponseError {
    TooLarge,
    Network(reqwest::Error),
}

async fn read_bounded_response(
    response: reqwest::Response,
    maximum: u64,
) -> Result<Vec<u8>, BoundedResponseError> {
    if response
        .content_length()
        .is_some_and(|length| length > maximum)
    {
        return Err(BoundedResponseError::TooLarge);
    }
    let initial_capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or(0);
    let mut body = Vec::with_capacity(initial_capacity);
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(BoundedResponseError::Network)?;
        let next_len = body
            .len()
            .checked_add(chunk.len())
            .ok_or(BoundedResponseError::TooLarge)?;
        if u64::try_from(next_len).map_or(true, |length| length > maximum) {
            return Err(BoundedResponseError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn parse_single_exact_event(body: &[u8]) -> Result<Event, SemanticProjectContextQueryError> {
    let values: Vec<Value> = serde_json::from_slice(body)
        .map_err(|_| SemanticProjectContextQueryError::verification_failed())?;
    let [value]: [Value; 1] = values
        .try_into()
        .map_err(|_| SemanticProjectContextQueryError::verification_failed())?;
    let event: Event = serde_json::from_value(value.clone())
        .map_err(|_| SemanticProjectContextQueryError::verification_failed())?;
    let canonical =
        serde_json::to_value(&event).map_err(|_| SemanticProjectContextQueryError::internal())?;
    if canonical != value {
        return Err(SemanticProjectContextQueryError::verification_failed());
    }
    Ok(event)
}

fn map_display_result(
    workspace: AppliedWorkspaceCapture,
    relay_pubkey: String,
    result: SemanticGraphQueryResult,
) -> Result<SemanticProjectContextQueryResult, SemanticProjectContextQueryError> {
    let initial_outcomes = map_initial_outcomes(&result)?;
    let context_outcomes = map_context_outcomes(&result)?;
    let omitted_initial_coordinates =
        u64::try_from(result.input_observations.omitted_initial_coordinates.len())
            .map_err(|_| SemanticProjectContextQueryError::internal())?;
    let omitted_context_coordinates =
        u64::try_from(result.input_observations.omitted_context_coordinates.len())
            .map_err(|_| SemanticProjectContextQueryError::internal())?;
    let roots = result.roots.iter().map(map_root).collect();
    let paths = result.paths.iter().map(map_path).collect();

    Ok(SemanticProjectContextQueryResult {
        community_key: workspace.community_key,
        applied_workspace_token: workspace.applied_workspace_token,
        caller_pubkey: workspace.caller.to_hex(),
        request_id: result.request_id,
        project_id: result.project_id,
        relay_pubkey,
        project_context_revision: result.observations.project_context_revision,
        snapshot_observed_at: result.observations.snapshot_observed_at,
        completion_reason: completion_reason(result.completion_reason),
        exhausted_dimensions: result
            .exhausted_dimensions
            .iter()
            .copied()
            .map(exhausted_dimension)
            .collect(),
        coverage: SemanticProjectContextCoverage {
            authorized_graph_sources: result.coverage.authorized_graph_sources,
            current_indexed_graph_sources: result.coverage.current_indexed_graph_sources,
            title_only_sources: result.coverage.title_only_sources,
            roots_returned: result.coverage.roots_returned,
            paths_returned: result.coverage.paths_returned,
            omitted_initial_coordinates,
            omitted_context_coordinates,
            index_coverage_partial: result.coverage.degraded_mode_counts.index_coverage_partial,
            omitted_for_response_budget: SemanticResponseBudgetOmissions {
                automatic_roots: result.coverage.omitted_for_response_budget.automatic_roots,
                paths: result.coverage.omitted_for_response_budget.paths,
                summaries: result.coverage.omitted_for_response_budget.summaries,
            },
        },
        input_outcomes: SemanticQueryInputOutcomes {
            initial: initial_outcomes,
            context: context_outcomes,
        },
        roots,
        paths,
    })
}

fn map_initial_outcomes(
    result: &SemanticGraphQueryResult,
) -> Result<Vec<SemanticCoordinateInputOutcome>, SemanticProjectContextQueryError> {
    let mut outcomes = BTreeMap::new();
    for accepted in &result.input_observations.accepted_initial_coordinates {
        outcomes.insert(
            accepted.coordinate.clone(),
            SemanticCoordinateInputOutcome {
                coordinate_key: coordinate_key(&accepted.coordinate),
                state: "accepted",
                reason: None,
            },
        );
    }
    for coordinate in &result.input_observations.initial_not_in_graph {
        outcomes.insert(
            coordinate.clone(),
            SemanticCoordinateInputOutcome {
                coordinate_key: coordinate_key(coordinate),
                state: "not_in_graph",
                reason: None,
            },
        );
    }
    for omitted in &result.input_observations.omitted_initial_coordinates {
        outcomes.insert(
            omitted.coordinate.clone(),
            SemanticCoordinateInputOutcome {
                coordinate_key: coordinate_key(&omitted.coordinate),
                state: "omitted",
                reason: Some(initial_omission_reason(omitted.reason)),
            },
        );
    }
    checked_outcomes(outcomes, "initial")
}

fn map_context_outcomes(
    result: &SemanticGraphQueryResult,
) -> Result<Vec<SemanticCoordinateInputOutcome>, SemanticProjectContextQueryError> {
    let mut outcomes = BTreeMap::new();
    for accepted in &result.input_observations.accepted_context_coordinates {
        outcomes.insert(
            accepted.coordinate.clone(),
            SemanticCoordinateInputOutcome {
                coordinate_key: coordinate_key(&accepted.coordinate),
                state: "accepted",
                reason: None,
            },
        );
    }
    for omitted in &result.input_observations.omitted_context_coordinates {
        outcomes.insert(
            omitted.coordinate.clone(),
            SemanticCoordinateInputOutcome {
                coordinate_key: coordinate_key(&omitted.coordinate),
                state: "omitted",
                reason: Some(context_omission_reason(omitted.reason)),
            },
        );
    }
    checked_outcomes(outcomes, "context")
}

fn checked_outcomes(
    outcomes: BTreeMap<ProjectContextCoordinate, SemanticCoordinateInputOutcome>,
    _kind: &'static str,
) -> Result<Vec<SemanticCoordinateInputOutcome>, SemanticProjectContextQueryError> {
    // The SDK verifier has already proved exact input-observation accounting;
    // BTreeMap provides the same canonical Coordinate order as the request.
    Ok(outcomes.into_values().collect())
}

fn map_root(root: &buzz_semantic_query_pkg::SemanticRoot) -> SemanticProjectContextRoot {
    let mut coordinate_entrypoints = Vec::new();
    let mut context_document_entrypoints = Vec::new();
    for entrypoint in &root.structural_entrypoints {
        match entrypoint {
            RootStructuralEntrypoint::Coordinate { coordinate } => {
                coordinate_entrypoints.push(coordinate_key(coordinate));
            }
            RootStructuralEntrypoint::ContextDocument {
                edge_key,
                document_id,
                ..
            } => context_document_entrypoints.push(SemanticContextDocumentEntrypoint {
                edge_key: edge_key.to_hex(),
                document_id: *document_id,
            }),
        }
    }
    SemanticProjectContextRoot {
        root_id: root.root_id.to_hex(),
        coordinate_entrypoints,
        context_document_entrypoints,
    }
}

fn map_path(path: &buzz_semantic_query_pkg::SemanticPath) -> SemanticProjectContextPath {
    SemanticProjectContextPath {
        path_id: path.path_id.to_hex(),
        root_id: path.root_id.to_hex(),
        branch_stop_reason: branch_stop_reason(path.branch_stop_reason),
        hops: path
            .hops
            .iter()
            .map(|hop| SemanticProjectContextHop {
                ordinal: hop.ordinal,
                edge_key: hop.edge.edge_key.to_hex(),
                complete_coordinate_keys: hop
                    .edge
                    .complete_coordinates
                    .iter()
                    .map(coordinate_key)
                    .collect(),
                current_context_document_ids: hop
                    .edge
                    .current_context_document_bindings
                    .iter()
                    .map(|binding| binding.document_id)
                    .collect(),
                entered_from_coordinate_key: hop
                    .entered_from_coordinate
                    .as_ref()
                    .map(coordinate_key),
                selected_context_document_id: hop.selected_relation_document.document_id,
                continued_to_coordinate_key: coordinate_key(
                    &hop.continued_to_coordinate.coordinate,
                ),
            })
            .collect(),
    }
}

const fn completion_reason(reason: CompletionReason) -> &'static str {
    match reason {
        CompletionReason::FrontierExhausted => "frontier_exhausted",
        CompletionReason::BudgetExhausted => "budget_exhausted",
        CompletionReason::WallTimeExhausted => "wall_time_exhausted",
    }
}

const fn exhausted_dimension(dimension: ExhaustedDimension) -> &'static str {
    match dimension {
        ExhaustedDimension::RecallPerChannel => "recall_per_channel",
        ExhaustedDimension::SemanticRoots => "semantic_roots",
        ExhaustedDimension::HopsPerPath => "hops_per_path",
        ExhaustedDimension::BeamWidth => "beam_width",
        ExhaustedDimension::ExpandedCoordinates => "expanded_coordinates",
        ExhaustedDimension::IncidentEdgesMaterialized => "incident_edges_materialized",
        ExhaustedDimension::RelationOptionsMaterialized => "relation_options_materialized",
        ExhaustedDimension::TargetOptionsMaterialized => "target_options_materialized",
        ExhaustedDimension::Paths => "paths",
        ExhaustedDimension::ResponseBytes => "response_bytes",
    }
}

const fn branch_stop_reason(reason: BranchStopReason) -> &'static str {
    match reason {
        BranchStopReason::FrontierExhausted => "frontier_exhausted",
        BranchStopReason::BelowRelevanceThreshold => "below_relevance_threshold",
        BranchStopReason::CycleOrDuplicate => "cycle_or_duplicate",
        BranchStopReason::MaxHopsReached => "max_hops_reached",
        BranchStopReason::HyperedgeTooLarge => "hyperedge_too_large",
        BranchStopReason::GlobalBudgetExhausted => "global_budget_exhausted",
        BranchStopReason::WallTimeExhausted => "wall_time_exhausted",
    }
}

const fn initial_omission_reason(reason: OmittedInitialCoordinateReason) -> &'static str {
    match reason {
        OmittedInitialCoordinateReason::SourceNotFound => "source_not_found",
        OmittedInitialCoordinateReason::SourceDeleted => "source_deleted",
        OmittedInitialCoordinateReason::SourceTombstoned => "source_tombstoned",
        OmittedInitialCoordinateReason::SourceIneligible => "source_ineligible",
    }
}

const fn context_omission_reason(reason: OmittedContextCoordinateReason) -> &'static str {
    match reason {
        OmittedContextCoordinateReason::SourceNotFound => "source_not_found",
        OmittedContextCoordinateReason::SourceIneligible => "source_ineligible",
        OmittedContextCoordinateReason::SemanticHeadMissing => "semantic_head_missing",
        OmittedContextCoordinateReason::SemanticHeadBuilding => "semantic_head_building",
        OmittedContextCoordinateReason::SemanticHeadFailed => "semantic_head_failed",
        OmittedContextCoordinateReason::ConditionedInputUnsupported => {
            "conditioned_input_unsupported"
        }
    }
}

#[cfg(test)]
#[path = "semantic_tests.rs"]
mod tests;
