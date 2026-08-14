//! `cf project-context` — verified Edge discovery and explicit maintenance.

#[path = "project_context_observation.rs"]
mod observation;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::Duration;

#[cfg(test)]
use buzz_core::kind::KIND_SEMANTIC_GRAPH_QUERY_RESULT;
use buzz_core::kind::{
    KIND_PROJECT_CONTEXT_COMMAND, KIND_PROJECT_CONTEXT_EDGE_BINDING, KIND_PROJECT_CONTEXT_META,
    KIND_PROJECT_DOCUMENT_HEAD, KIND_PROJECT_DOCUMENT_META,
};
use buzz_core::{CommunityId, PublicKey, RuntimeFence};
use buzz_project_context::{
    canonicalize_coordinates, EdgeKey, ProjectContextBindingState, ProjectContextCommand,
    ProjectContextCoordinate, ProjectContextEdge, ProjectContextMetaProjection,
    ProjectContextOperation, ProjectContextReceipt,
};
use buzz_project_document::DocumentHeadProjection;
use buzz_project_view::v3::{ProjectViewEntryV3, ProjectViewObjectDataV3};
use buzz_project_view::ProjectViewObjectType;
use buzz_sdk::project_context::{
    aggregate_project_context_edges, build_project_context_command, parse_project_context_binding,
    parse_project_context_command, parse_project_context_meta, project_context_edge_coordinate,
    verify_project_context_binding_observation, VerifiedProjectContextMeta,
};
use buzz_sdk::project_document::{
    document_head_coordinate, parse_document_head, parse_document_meta,
    verify_document_head_observation, VerifiedDocumentHead, VerifiedDocumentMeta,
};
use buzz_sdk::semantic_coordinate_search::{
    parse_project_context_coordinate_search_result,
    ProjectContextCoordinateSearchHttpRequestObservation,
};
use buzz_sdk::semantic_graph::{
    parse_semantic_graph_query_result, SemanticGraphHttpRequestObservation,
};
use buzz_semantic_query::{
    LifecycleFilter, ProjectContextCoordinateSearchQuery, ProjectContextCoordinateSearchResult,
    RootStructuralEntrypoint, SemanticGraphQuery, SemanticGraphQueryBudget,
    SemanticGraphQueryResult,
};
use chrono::{DateTime, Utc};
use nostr::Event;
use serde::Serialize;
use serde_json::{json, Value};
use uuid::{Uuid, Version};

use crate::client::{CarryforthClient, ProjectCommandDelivery};
use crate::commands::meetings::{fetch_meeting_context_summaries, MeetingSummary};
use crate::commands::project_view_snapshot::{
    read_identity, read_verified_v3_snapshot, ProjectViewIdentity, ProjectViewSchema,
    COORDINATE_SEARCH_HTTP_EXTENSION, SEMANTIC_GRAPH_QUERY_HTTP_EXTENSION,
};
use crate::error::CliError;
use crate::{
    OutputFormat, ProjectContextAttributionArgs, ProjectContextCmd, ProjectContextCoordinateCmd,
    ProjectContextEdgeCmd, SemanticGraphBudgetArgs, SemanticLifecycleArg,
};

const QUERY_PAGE_SIZE: u16 = 500;
const QUERY_SNAPSHOT_ATTEMPTS: usize = 3;
const HYDRATION_SNAPSHOT_ATTEMPTS: usize = 3;
const DOCUMENT_HEAD_CHUNK_SIZE: usize = 200;

#[derive(Debug, Clone)]
enum ContextQuery {
    Exact(Vec<ProjectContextCoordinate>),
    Incident(ProjectContextCoordinate),
    ContainsAll(Vec<ProjectContextCoordinate>),
    EdgeKey(EdgeKey),
}

impl ContextQuery {
    fn coordinates(&self) -> Vec<ProjectContextCoordinate> {
        match self {
            Self::Exact(coordinates) | Self::ContainsAll(coordinates) => coordinates.clone(),
            Self::Incident(coordinate) => vec![coordinate.clone()],
            Self::EdgeKey(_) => Vec::new(),
        }
    }

    const fn query_type(&self) -> &'static str {
        match self {
            Self::Exact(_) => "exact",
            Self::Incident(_) => "incident",
            Self::ContainsAll(_) => "contains_all",
            Self::EdgeKey(_) => "edge_key",
        }
    }

    const fn complete_catalog(&self) -> bool {
        matches!(self, Self::ContainsAll(coordinates) if coordinates.is_empty())
    }
}

#[derive(Serialize)]
struct QueryDescriptor {
    query_type: &'static str,
    coordinates: Vec<ProjectContextCoordinate>,
}

#[derive(Serialize)]
struct ProjectContextQueryOutput {
    project_id: Uuid,
    context_revision: u64,
    projection_generation: u64,
    query: QueryDescriptor,
    project_view_observation: ProjectViewObservation,
    document_observation: DocumentObservation,
    meeting_observation: MeetingObservation,
    edges: Vec<EdgeOutput>,
}

#[derive(Clone, Serialize)]
struct CoordinateOutput {
    coordinate: ProjectContextCoordinate,
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    object_revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    document_revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_by: Option<PublicKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    meeting_fetch: Option<MeetingFetchCommands>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unavailable_reason: Option<&'static str>,
}

#[derive(Clone, Serialize)]
struct MeetingFetchCommands {
    metadata: String,
    board: String,
    speech: String,
}

#[derive(Clone, Serialize)]
struct ContextDocumentOutput {
    document_id: Uuid,
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    document_revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_by: Option<PublicKey>,
    fetch_command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    unavailable_reason: Option<&'static str>,
}

#[derive(Serialize)]
struct EdgeOutput {
    edge_key: EdgeKey,
    coordinates: Vec<CoordinateOutput>,
    context_documents: Vec<ContextDocumentOutput>,
}

#[derive(Clone, Serialize)]
struct ProjectViewObservation {
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    project_revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    projection_generation: Option<u64>,
}

#[derive(Clone, Serialize)]
struct DocumentObservation {
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    catalog_revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    projection_generation: Option<u64>,
}

#[derive(Clone, Serialize)]
struct MeetingObservation {
    state: &'static str,
    requested_count: usize,
    observed_count: usize,
}

struct EdgeSnapshot {
    meta: VerifiedProjectContextMeta,
    edges: Vec<ProjectContextEdge>,
}

#[derive(Serialize)]
struct SemanticGraphQueryCliOutput<'a> {
    result: &'a SemanticGraphQueryResult,
    read_commands: UnsignedSemanticReadCommands,
}

#[derive(Serialize)]
struct UnsignedSemanticReadCommands {
    signed: bool,
    derivation: &'static str,
    commands: Vec<SemanticReadCommand>,
}

#[derive(Serialize)]
struct SemanticReadCommand {
    coordinate: ProjectContextCoordinate,
    command: String,
}

struct BindingPages {
    values: Vec<Value>,
    reached_empty_page: bool,
}

struct ProjectViewHydration {
    observation: ProjectViewObservation,
    coordinates: BTreeMap<ProjectContextCoordinate, CoordinateOutput>,
}

struct DocumentHydration {
    observation: DocumentObservation,
    heads: BTreeMap<Uuid, DocumentMetadata>,
}

struct MeetingHydration {
    observation: MeetingObservation,
    summaries: BTreeMap<Uuid, MeetingSummary>,
}

#[derive(Clone)]
enum DocumentMetadata {
    Active {
        title: String,
        summary: Option<String>,
        document_revision: u64,
        updated_at: DateTime<Utc>,
        updated_by: PublicKey,
    },
    Tombstoned {
        document_revision: u64,
        deleted_at: DateTime<Utc>,
        deleted_by: PublicKey,
    },
}

/// Dispatch one Project Context query or write.
pub async fn dispatch(
    command: ProjectContextCmd,
    client: &CarryforthClient,
    format: &OutputFormat,
) -> Result<(), CliError> {
    match command {
        ProjectContextCmd::Coordinate { command } => match command {
            ProjectContextCoordinateCmd::Show { coordinate } => {
                observation::run_coordinate_show(client, &coordinate, format).await
            }
            ProjectContextCoordinateCmd::Edges {
                coordinate,
                limit,
                after_edge,
                expected_context_meta_event_id,
                expected_context_revision,
                expected_projection_generation,
            } => {
                observation::run_coordinate_edges(
                    client,
                    &coordinate,
                    limit,
                    after_edge.as_deref(),
                    expected_context_meta_event_id.as_deref(),
                    expected_context_revision,
                    expected_projection_generation,
                    format,
                )
                .await
            }
        },
        ProjectContextCmd::Edge { command } => match command {
            ProjectContextEdgeCmd::Documents {
                edge_key,
                document,
                limit,
                after_document,
                expected_context_meta_event_id,
                expected_context_revision,
                expected_projection_generation,
            } => {
                observation::run_edge_documents(
                    client,
                    &edge_key,
                    document,
                    limit,
                    after_document,
                    expected_context_meta_event_id.as_deref(),
                    expected_context_revision,
                    expected_projection_generation,
                    format,
                )
                .await
            }
            ProjectContextEdgeCmd::Coordinates { edge_key } => {
                observation::run_edge_coordinates(client, &edge_key, format).await
            }
        },
        ProjectContextCmd::CoordinateSearch { query, limit } => {
            run_coordinate_search(client, query, limit, format).await
        }
        ProjectContextCmd::SemanticQuery {
            problem,
            initial_coordinates,
            context_coordinates,
            lifecycle,
            budget,
        } => {
            run_semantic_query(
                client,
                problem,
                initial_coordinates,
                context_coordinates,
                lifecycle,
                budget,
                format,
            )
            .await
        }
        ProjectContextCmd::Exact { coordinates } => {
            let coordinates = canonicalize_edge_tokens(coordinates)?;
            run_query(client, ContextQuery::Exact(coordinates), format).await
        }
        ProjectContextCmd::Incident { coordinate } => {
            let coordinate = parse_coordinate_token(&coordinate)?;
            run_query(client, ContextQuery::Incident(coordinate), format).await
        }
        ProjectContextCmd::ContainsAll { coordinates } => {
            let coordinates = canonicalize_subset_tokens(coordinates)?;
            run_query(client, ContextQuery::ContainsAll(coordinates), format).await
        }
        ProjectContextCmd::Attach {
            context_document_id,
            coordinates,
            attribution,
        } => {
            let coordinates = canonicalize_edge_tokens(coordinates)?;
            run_write(
                client,
                ProjectContextOperation::Attach,
                context_document_id,
                coordinates,
                attribution,
            )
            .await
        }
        ProjectContextCmd::Detach {
            context_document_id,
            coordinates,
            attribution,
        } => {
            let coordinates = canonicalize_edge_tokens(coordinates)?;
            run_write(
                client,
                ProjectContextOperation::Detach,
                context_document_id,
                coordinates,
                attribution,
            )
            .await
        }
    }
}

async fn run_coordinate_search(
    client: &CarryforthClient,
    query: String,
    limit: u8,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let identity = require_coordinate_search_identity(client).await?;
    let project = read_verified_v3_snapshot(client, identity)
        .await
        .map_err(|error| {
            integrity_error(format!(
                "cannot resolve current Project identity for Coordinate search: {error}"
            ))
        })?;
    let project_id = *project.meta().project_id.as_uuid();
    let request = ProjectContextCoordinateSearchQuery {
        request_id: Uuid::new_v4(),
        project_id,
        query,
        limit,
    }
    .validate_and_canonicalize()
    .map_err(|error| CliError::Usage(format!("invalid Coordinate search: {error}")))?;
    let response = client
        .coordinate_search_once(&identity.relay_pubkey, request)
        .await?;
    let event = parse_single_semantic_result_event(&response.response_body)?;
    let result = parse_project_context_coordinate_search_result(
        &event,
        &identity.relay_pubkey,
        ProjectContextCoordinateSearchHttpRequestObservation {
            project_id: CommunityId::from_uuid(project_id),
            authenticated_caller: client.public_key(),
            request: &response.request,
            nip98_auth_event_id: response.nip98_auth_event_id,
            exact_authenticated_body: &response.exact_body,
        },
    )
    .map_err(|error| integrity_error(format!("invalid Coordinate-search result: {error}")))?;
    print_coordinate_search_result(&result, format)
}

async fn require_coordinate_search_identity(
    client: &CarryforthClient,
) -> Result<ProjectViewIdentity, CliError> {
    let identity = read_identity(client).await?.ok_or_else(|| {
        CliError::Other("unavailable:coordinate_search:project_view_v3_not_ready".to_owned())
    })?;
    if identity.schema != ProjectViewSchema::V3 {
        return Err(CliError::Other(
            "unsupported:coordinate_search:project_view_v3_required".to_owned(),
        ));
    }
    if !identity.coordinate_search_http_enabled {
        if identity.extensions_temporarily_unavailable {
            return Err(CliError::Unavailable(
                "Relay Coordinate-search capability observation could not be completed".to_owned(),
            ));
        }
        return Err(CliError::Other(format!(
            "unsupported:coordinate_search:relay_does_not_advertise_{COORDINATE_SEARCH_HTTP_EXTENSION}"
        )));
    }
    Ok(identity)
}

fn print_coordinate_search_result(
    result: &ProjectContextCoordinateSearchResult,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let serialized = match format {
        OutputFormat::Json => serde_json::to_string_pretty(result),
        OutputFormat::Compact => serde_json::to_string(result),
    }
    .map_err(|error| CliError::Other(format!("failed to serialize output: {error}")))?;
    println!("{serialized}");
    Ok(())
}

async fn run_semantic_query(
    client: &CarryforthClient,
    problem: String,
    initial_coordinate_tokens: Vec<String>,
    context_coordinate_tokens: Vec<String>,
    lifecycle: SemanticLifecycleArg,
    budget_args: SemanticGraphBudgetArgs,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let identity = require_semantic_identity(client).await?;
    let project = read_verified_v3_snapshot(client, identity)
        .await
        .map_err(|error| {
            integrity_error(format!(
                "cannot resolve current Project identity for semantic query: {error}"
            ))
        })?;
    let project_id = *project.meta().project_id.as_uuid();
    let initial_coordinates = parse_semantic_coordinate_tokens(initial_coordinate_tokens)?;
    let context_coordinates = parse_semantic_coordinate_tokens(context_coordinate_tokens)?;
    let request = SemanticGraphQuery {
        request_id: Uuid::new_v4(),
        project_id,
        problem,
        initial_coordinates,
        context_coordinates,
        lifecycle_filter: match lifecycle {
            SemanticLifecycleArg::AllCurrent => LifecycleFilter::AllCurrent,
            SemanticLifecycleArg::NonTerminal => LifecycleFilter::NonTerminal,
            SemanticLifecycleArg::TerminalOnly => LifecycleFilter::TerminalOnly,
        },
        budget: semantic_query_budget(budget_args),
    }
    .validate_and_canonicalize()
    .map_err(|error| CliError::Usage(format!("invalid semantic graph query: {error}")))?;

    let response = client
        .semantic_query_once(&identity.relay_pubkey, request)
        .await?;
    let event = parse_single_semantic_result_event(&response.response_body)?;
    let result = parse_semantic_graph_query_result(
        &event,
        &identity.relay_pubkey,
        SemanticGraphHttpRequestObservation {
            project_id: CommunityId::from_uuid(project_id),
            authenticated_caller: client.public_key(),
            request: &response.request,
            nip98_auth_event_id: response.nip98_auth_event_id,
            exact_authenticated_body: &response.exact_body,
        },
    )
    .map_err(|error| integrity_error(format!("invalid semantic virtual result: {error}")))?;
    let read_commands = UnsignedSemanticReadCommands {
        signed: false,
        derivation: "deterministic_from_verified_result_identities",
        commands: semantic_read_commands(&result),
    };
    print_semantic_query_output(
        &SemanticGraphQueryCliOutput {
            result: &result,
            read_commands,
        },
        format,
    )
}

async fn require_semantic_identity(
    client: &CarryforthClient,
) -> Result<ProjectViewIdentity, CliError> {
    let identity = read_identity(client).await?.ok_or_else(|| {
        CliError::Other("unavailable:semantic_graph_query:project_view_v3_not_ready".to_owned())
    })?;
    if identity.schema != ProjectViewSchema::V3 {
        return Err(CliError::Other(
            "unsupported:semantic_graph_query:project_view_v3_required".to_owned(),
        ));
    }
    if !identity.semantic_query_http_enabled {
        if identity.extensions_temporarily_unavailable {
            return Err(CliError::Unavailable(
                "Relay semantic capability observation could not be completed".to_owned(),
            ));
        }
        return Err(CliError::Other(format!(
            "unsupported:semantic_graph_query:relay_does_not_advertise_{SEMANTIC_GRAPH_QUERY_HTTP_EXTENSION}"
        )));
    }
    Ok(identity)
}

fn parse_semantic_coordinate_tokens(
    tokens: Vec<String>,
) -> Result<Vec<ProjectContextCoordinate>, CliError> {
    tokens
        .iter()
        .map(|token| parse_coordinate_token(token))
        .collect()
}

fn semantic_query_budget(args: SemanticGraphBudgetArgs) -> SemanticGraphQueryBudget {
    let mut budget = SemanticGraphQueryBudget::default();
    if let Some(value) = args.max_recall_per_channel {
        budget.max_recall_per_channel = value;
    }
    if let Some(value) = args.max_semantic_roots {
        budget.max_semantic_roots = value;
    }
    if let Some(value) = args.max_hops_per_path {
        budget.max_hops_per_path = value;
    }
    if let Some(value) = args.beam_width {
        budget.beam_width = value;
    }
    if let Some(value) = args.max_expanded_coordinates {
        budget.max_expanded_coordinates = value;
    }
    if let Some(value) = args.max_incident_edges_materialized {
        budget.max_incident_edges_materialized = value;
    }
    if let Some(value) = args.max_relation_options_materialized {
        budget.max_relation_options_materialized = value;
    }
    if let Some(value) = args.max_target_options_materialized {
        budget.max_target_options_materialized = value;
    }
    if let Some(value) = args.max_paths {
        budget.max_paths = value;
    }
    if let Some(value) = args.max_wall_time_ms {
        budget.max_wall_time_ms = value;
    }
    if let Some(value) = args.max_response_bytes {
        budget.max_response_bytes = value;
    }
    budget
}

fn parse_single_semantic_result_event(bytes: &[u8]) -> Result<Event, CliError> {
    let values: Vec<Value> = serde_json::from_slice(bytes).map_err(|error| {
        integrity_error(format!(
            "semantic query response is not a JSON Event array: {error}"
        ))
    })?;
    let [value] = values.as_slice() else {
        return Err(integrity_error(
            "semantic query response must contain exactly one virtual Event",
        ));
    };
    let event: Event = serde_json::from_value(value.clone())
        .map_err(|error| integrity_error(format!("invalid semantic result Event: {error}")))?;
    let canonical = serde_json::to_value(&event)
        .map_err(|error| integrity_error(format!("serialize semantic result Event: {error}")))?;
    if canonical != *value {
        return Err(integrity_error(
            "semantic result Event contains unknown or noncanonical fields",
        ));
    }
    Ok(event)
}

fn semantic_read_commands(result: &SemanticGraphQueryResult) -> Vec<SemanticReadCommand> {
    let mut coordinates = BTreeSet::new();
    for observation in &result.input_observations.accepted_initial_coordinates {
        coordinates.insert(observation.coordinate.clone());
    }
    coordinates.extend(
        result
            .input_observations
            .initial_not_in_graph
            .iter()
            .cloned(),
    );
    for observation in &result.input_observations.omitted_initial_coordinates {
        coordinates.insert(observation.coordinate.clone());
    }
    for observation in &result.input_observations.accepted_context_coordinates {
        coordinates.insert(observation.coordinate.clone());
    }
    for observation in &result.input_observations.omitted_context_coordinates {
        coordinates.insert(observation.coordinate.clone());
    }
    for root in &result.roots {
        for entrypoint in &root.structural_entrypoints {
            match entrypoint {
                RootStructuralEntrypoint::Coordinate { coordinate } => {
                    coordinates.insert(coordinate.clone());
                }
                RootStructuralEntrypoint::ContextDocument { document_id, .. } => {
                    coordinates.insert(ProjectContextCoordinate::Document {
                        document_id: *document_id,
                    });
                }
            }
        }
    }
    for path in &result.paths {
        coordinates.insert(path.terminal_coordinate.clone());
        for hop in &path.hops {
            if let Some(coordinate) = &hop.entered_from_coordinate {
                coordinates.insert(coordinate.clone());
            }
            coordinates.extend(hop.edge.complete_coordinates.iter().cloned());
            for binding in &hop.edge.current_context_document_bindings {
                coordinates.insert(ProjectContextCoordinate::Document {
                    document_id: binding.document_id,
                });
            }
            coordinates.insert(ProjectContextCoordinate::Document {
                document_id: hop.selected_relation_document.document_id,
            });
            coordinates.insert(hop.continued_to_coordinate.coordinate.clone());
        }
    }

    semantic_read_commands_for_coordinates(coordinates)
}

fn semantic_read_commands_for_coordinates(
    coordinates: BTreeSet<ProjectContextCoordinate>,
) -> Vec<SemanticReadCommand> {
    coordinates
        .into_iter()
        .map(|coordinate| SemanticReadCommand {
            command: canonical_coordinate_read_command(&coordinate),
            coordinate,
        })
        .collect()
}

fn canonical_coordinate_read_command(coordinate: &ProjectContextCoordinate) -> String {
    match coordinate {
        ProjectContextCoordinate::ProjectViewObject {
            object_type,
            object_id,
        } => format!(
            "cf project-view get-object {} {object_id}",
            object_type.as_str()
        ),
        ProjectContextCoordinate::Document { document_id } => {
            format!("cf documents get {document_id}")
        }
        ProjectContextCoordinate::Meeting { meeting_id } => {
            format!("cf meetings show --meeting {meeting_id}")
        }
    }
}

fn print_semantic_query_output(
    output: &SemanticGraphQueryCliOutput<'_>,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let serialized = match format {
        OutputFormat::Json => serde_json::to_string_pretty(output),
        OutputFormat::Compact => serde_json::to_string(output),
    }
    .map_err(|error| CliError::Other(format!("failed to serialize output: {error}")))?;
    println!("{serialized}");
    Ok(())
}

async fn run_query(
    client: &CarryforthClient,
    query: ContextQuery,
    _format: &OutputFormat,
) -> Result<(), CliError> {
    let identity = require_identity(client).await?;
    let snapshot = read_edge_snapshot(client, identity, &query).await?;
    let project_id = snapshot.meta.projection.project_id;

    let mut project_view_coordinates = BTreeSet::new();
    let mut document_ids = BTreeSet::new();
    let mut meeting_ids = BTreeSet::new();
    let mut context_document_ids = BTreeSet::new();
    for edge in &snapshot.edges {
        for coordinate in edge.coordinates() {
            match coordinate {
                ProjectContextCoordinate::ProjectViewObject { .. } => {
                    project_view_coordinates.insert(coordinate.clone());
                }
                ProjectContextCoordinate::Document { document_id } => {
                    document_ids.insert(*document_id);
                }
                ProjectContextCoordinate::Meeting { meeting_id } => {
                    meeting_ids.insert(*meeting_id);
                }
            }
        }
        context_document_ids.extend(edge.context_document_ids().iter().copied());
    }
    document_ids.extend(context_document_ids.iter().copied());

    let project_view =
        hydrate_project_view(client, identity, project_id, &project_view_coordinates).await?;
    let documents = hydrate_documents(client, identity, project_id, &document_ids).await?;
    let meetings = hydrate_meetings(client, &meeting_ids).await?;

    let mut edges = Vec::with_capacity(snapshot.edges.len());
    for edge in snapshot.edges {
        let mut coordinates = Vec::with_capacity(edge.coordinates().len());
        for coordinate in edge.coordinates() {
            let hydrated = match coordinate {
                ProjectContextCoordinate::ProjectViewObject { .. } => project_view
                    .coordinates
                    .get(coordinate)
                    .cloned()
                    .unwrap_or_else(|| unavailable_coordinate(coordinate.clone())),
                ProjectContextCoordinate::Document { document_id } => {
                    document_coordinate_output(coordinate.clone(), documents.heads.get(document_id))
                }
                ProjectContextCoordinate::Meeting { meeting_id } => meeting_coordinate_output(
                    coordinate.clone(),
                    meetings.summaries.get(meeting_id),
                ),
            };
            coordinates.push(hydrated);
        }

        let mut context_documents = Vec::with_capacity(edge.context_document_ids().len());
        for document_id in edge.context_document_ids() {
            let output = context_document_output(*document_id, documents.heads.get(document_id))?;
            context_documents.push(output);
        }
        edges.push(EdgeOutput {
            edge_key: edge.key(),
            coordinates,
            context_documents,
        });
    }

    print_json(&ProjectContextQueryOutput {
        project_id,
        context_revision: snapshot.meta.projection.context_revision,
        projection_generation: snapshot.meta.projection.projection_generation,
        query: QueryDescriptor {
            query_type: query.query_type(),
            coordinates: query.coordinates(),
        },
        project_view_observation: project_view.observation,
        document_observation: documents.observation,
        meeting_observation: meetings.observation,
        edges,
    })
}

async fn require_identity(client: &CarryforthClient) -> Result<ProjectViewIdentity, CliError> {
    let identity = read_identity(client).await?.ok_or_else(|| {
        CliError::Other("unavailable:project_context:project_view_not_ready".to_owned())
    })?;
    if identity.schema != ProjectViewSchema::V3 {
        return Err(CliError::Other(
            "unsupported:project_context:project_view_v3_required".to_owned(),
        ));
    }
    if !identity.document_enabled {
        return Err(CliError::Other(
            "unavailable:project_context:project_document_not_ready".to_owned(),
        ));
    }
    if !identity.context_edge_enabled {
        return Err(CliError::Other(
            if identity.context_edge_migration_required {
                format!(
                    "migration_required:{}",
                    buzz_project_context::PROJECT_CONTEXT_CAPABILITY
                )
            } else {
                "unavailable:project_context:capability_disabled".to_owned()
            },
        ));
    }
    Ok(identity)
}

async fn read_edge_snapshot(
    client: &CarryforthClient,
    identity: ProjectViewIdentity,
    query: &ContextQuery,
) -> Result<EdgeSnapshot, CliError> {
    for attempt in 0..QUERY_SNAPSHOT_ATTEMPTS {
        let before = read_context_meta(client, identity).await?;
        let project_id = before.projection.project_id;
        validate_query_for_project(query, project_id)?;
        let pages = read_binding_pages(client, identity, &before, query).await?;
        let after = read_context_meta(client, identity).await?;
        if !same_context_observation(&before, &after) {
            if attempt + 1 < QUERY_SNAPSHOT_ATTEMPTS {
                tokio::time::sleep(Duration::from_millis(25_u64 << attempt)).await;
                continue;
            }
            return Err(CliError::Conflict(
                "conflict:project_context:snapshot_changed".to_owned(),
            ));
        }
        if !pages.reached_empty_page {
            return Err(integrity_error(
                "binding pagination made no progress under a stable metadata observation",
            ));
        }

        let project = CommunityId::from_uuid(project_id);
        let mut bindings = Vec::with_capacity(pages.values.len());
        let mut seen_events = HashSet::with_capacity(pages.values.len());
        for value in pages.values {
            let event: Event = serde_json::from_value(value)
                .map_err(|_| integrity_error("binding query returned an invalid event"))?;
            if !seen_events.insert(event.id) {
                continue;
            }
            let binding = parse_project_context_binding(&event, &identity.relay_pubkey, project)
                .map_err(|error| integrity_error(error.to_string()))?;
            verify_project_context_binding_observation(&before, &binding)
                .map_err(|error| integrity_error(error.to_string()))?;
            bindings.push(binding);
        }
        let mut edges =
            aggregate_project_context_edges(&before, &bindings, query.complete_catalog())
                .map_err(|error| integrity_error(error.to_string()))?;
        apply_query_semantics(query, project_id, &mut edges)?;
        return Ok(EdgeSnapshot {
            meta: before,
            edges,
        });
    }
    Err(CliError::Conflict(
        "conflict:project_context:snapshot_unstable".to_owned(),
    ))
}

async fn read_context_meta(
    client: &CarryforthClient,
    identity: ProjectViewIdentity,
) -> Result<VerifiedProjectContextMeta, CliError> {
    let values = query_values(
        client,
        json!({
            "kinds": [KIND_PROJECT_CONTEXT_META],
            "authors": [identity.relay_pubkey.to_hex()],
            "limit": 2,
        }),
        "Context metadata",
    )
    .await?;
    let [value] = values.as_slice() else {
        return Err(CliError::Other(
            "unavailable:project_context:verified_meta".to_owned(),
        ));
    };
    let event: Event = serde_json::from_value(value.clone())
        .map_err(|_| integrity_error("metadata query returned an invalid event"))?;
    let untrusted: ProjectContextMetaProjection = serde_json::from_str(&event.content)
        .map_err(|_| integrity_error("metadata content cannot identify its Project"))?;
    parse_project_context_meta(
        &event,
        &identity.relay_pubkey,
        CommunityId::from_uuid(untrusted.project_id),
    )
    .map_err(|error| integrity_error(error.to_string()))
}

async fn read_binding_pages(
    client: &CarryforthClient,
    identity: ProjectViewIdentity,
    meta: &VerifiedProjectContextMeta,
    query: &ContextQuery,
) -> Result<BindingPages, CliError> {
    let project_id = meta.projection.project_id;
    let mut filter = json!({
        "kinds": [KIND_PROJECT_CONTEXT_EDGE_BINDING],
        "authors": [identity.relay_pubkey.to_hex()],
        "#s": ["active"],
        "limit": QUERY_PAGE_SIZE,
    });
    match query {
        ContextQuery::Exact(coordinates) => {
            let edge_key = EdgeKey::derive(project_id, coordinates)
                .map_err(|error| CliError::Usage(error.to_string()))?;
            filter["#g"] = json!([project_context_edge_coordinate(
                CommunityId::from_uuid(project_id),
                edge_key,
            )]);
        }
        ContextQuery::Incident(coordinate) => {
            filter["#c"] = json!([coordinate.tag_value(project_id)]);
        }
        ContextQuery::ContainsAll(coordinates) if !coordinates.is_empty() => {
            filter["#c"] = json!([coordinates[0].tag_value(project_id)]);
        }
        ContextQuery::ContainsAll(_) => {}
        ContextQuery::EdgeKey(edge_key) => {
            filter["#g"] = json!([project_context_edge_coordinate(
                CommunityId::from_uuid(project_id),
                *edge_key,
            )]);
        }
    }

    let mut values = Vec::new();
    let mut seen_event_ids = HashSet::new();
    let mut page: u64 = 1;
    loop {
        filter["page"] = json!(page);
        let current = query_values(client, filter.clone(), "Context binding page").await?;
        if current.len() > usize::from(QUERY_PAGE_SIZE) {
            return Err(integrity_error(
                "binding page exceeded the requested bounded page size",
            ));
        }
        if current.is_empty() {
            return Ok(BindingPages {
                values,
                reached_empty_page: true,
            });
        }
        let previous_len = values.len();
        for value in current {
            let event_id = value.get("id").and_then(Value::as_str).ok_or_else(|| {
                integrity_error("binding page contains an event without a string id")
            })?;
            if seen_event_ids.insert(event_id.to_owned()) {
                values.push(value);
            }
        }
        if values.len() == previous_len {
            return Ok(BindingPages {
                values,
                reached_empty_page: false,
            });
        }
        page = page
            .checked_add(1)
            .ok_or_else(|| integrity_error("binding page number overflow"))?;
    }
}

fn apply_query_semantics(
    query: &ContextQuery,
    project_id: Uuid,
    edges: &mut Vec<ProjectContextEdge>,
) -> Result<(), CliError> {
    match query {
        ContextQuery::Exact(coordinates) => {
            let expected_key = EdgeKey::derive(project_id, coordinates)
                .map_err(|error| CliError::Usage(error.to_string()))?;
            if edges.iter().any(|edge| {
                edge.key() != expected_key || edge.coordinates() != coordinates.as_slice()
            }) || edges.len() > 1
            {
                return Err(integrity_error(
                    "exact query returned a hash collision, superset, or multiple Edges",
                ));
            }
        }
        ContextQuery::Incident(coordinate) => {
            if edges
                .iter()
                .any(|edge| !edge.coordinates().contains(coordinate))
            {
                return Err(integrity_error(
                    "incident query returned an Edge that omits its coordinate",
                ));
            }
        }
        ContextQuery::ContainsAll(required) => {
            edges.retain(|edge| {
                required
                    .iter()
                    .all(|coordinate| edge.coordinates().binary_search(coordinate).is_ok())
            });
        }
        ContextQuery::EdgeKey(expected_edge_key) => {
            if edges.len() > 1 || edges.iter().any(|edge| edge.key() != *expected_edge_key) {
                return Err(integrity_error(
                    "edge-key query returned a different or duplicate Edge",
                ));
            }
        }
    }
    Ok(())
}

fn same_context_observation(
    left: &VerifiedProjectContextMeta,
    right: &VerifiedProjectContextMeta,
) -> bool {
    left.event_id == right.event_id
        && left.projection.context_revision == right.projection.context_revision
        && left.projection.projection_generation == right.projection.projection_generation
}

fn validate_query_for_project(query: &ContextQuery, project_id: Uuid) -> Result<(), CliError> {
    for coordinate in query.coordinates() {
        coordinate
            .validate_for_project(project_id)
            .map_err(|error| CliError::Usage(error.to_string()))?;
    }
    Ok(())
}

async fn hydrate_project_view(
    client: &CarryforthClient,
    identity: ProjectViewIdentity,
    project_id: Uuid,
    requested: &BTreeSet<ProjectContextCoordinate>,
) -> Result<ProjectViewHydration, CliError> {
    if requested.is_empty() {
        return Ok(ProjectViewHydration {
            observation: ProjectViewObservation {
                state: "not_requested",
                project_revision: None,
                projection_generation: None,
            },
            coordinates: BTreeMap::new(),
        });
    }

    match read_verified_v3_snapshot(client, identity).await {
        Ok(snapshot) => {
            if *snapshot.meta().project_id.as_uuid() != project_id {
                return Err(integrity_error(
                    "Project View and Context metadata identify different Projects",
                ));
            }
            let mut coordinates = BTreeMap::new();
            for coordinate in requested {
                let ProjectContextCoordinate::ProjectViewObject {
                    object_type,
                    object_id,
                } = coordinate
                else {
                    return Err(integrity_error(
                        "Project View hydration received a non-Project-View coordinate",
                    ));
                };
                let entry = snapshot.entry(*object_id).ok_or_else(|| {
                    integrity_error("verified Project View snapshot omitted a Context coordinate")
                })?;
                if entry.object_type() != *object_type {
                    return Err(integrity_error(
                        "verified Project View coordinate has a different object type",
                    ));
                }
                coordinates.insert(
                    coordinate.clone(),
                    project_view_coordinate_output(coordinate, entry),
                );
            }
            Ok(ProjectViewHydration {
                observation: ProjectViewObservation {
                    state: "observed",
                    project_revision: Some(snapshot.meta().project_revision),
                    projection_generation: Some(snapshot.meta().projection_generation),
                },
                coordinates,
            })
        }
        Err(error) if hydration_is_unavailable(&error) => Ok(ProjectViewHydration {
            observation: ProjectViewObservation {
                state: "unavailable",
                project_revision: None,
                projection_generation: None,
            },
            coordinates: requested
                .iter()
                .cloned()
                .map(|coordinate| {
                    let unavailable = unavailable_coordinate(coordinate.clone());
                    (coordinate, unavailable)
                })
                .collect(),
        }),
        Err(error) => Err(error),
    }
}

fn project_view_coordinate_output(
    coordinate: &ProjectContextCoordinate,
    entry: &ProjectViewEntryV3,
) -> CoordinateOutput {
    match entry {
        ProjectViewEntryV3::Active(object) => {
            let (title, status) = project_view_title_status(&object.data);
            CoordinateOutput {
                coordinate: coordinate.clone(),
                state: "active",
                title: Some(title),
                description: None,
                summary: object.data.summary().map(ToOwned::to_owned),
                status,
                object_revision: Some(object.object_revision),
                document_revision: None,
                updated_at: Some(object.updated_at),
                updated_by: Some(object.updated_by),
                meeting_fetch: None,
                unavailable_reason: None,
            }
        }
        ProjectViewEntryV3::Tombstone(tombstone) => CoordinateOutput {
            coordinate: coordinate.clone(),
            state: "tombstoned",
            title: None,
            description: None,
            summary: None,
            status: None,
            object_revision: Some(tombstone.object_revision),
            document_revision: None,
            updated_at: Some(tombstone.deleted_at),
            updated_by: Some(tombstone.deleted_by),
            meeting_fetch: None,
            unavailable_reason: None,
        },
    }
}

fn project_view_title_status(data: &ProjectViewObjectDataV3) -> (String, Option<Value>) {
    match data {
        ProjectViewObjectDataV3::ProjectProfile(value) => (value.name.clone(), None),
        ProjectViewObjectDataV3::Goal(value) => (value.title.clone(), None),
        ProjectViewObjectDataV3::Role(value) => (
            value.name.clone(),
            Some(json!(if value.active { "active" } else { "inactive" })),
        ),
        ProjectViewObjectDataV3::Plan(value) => (value.title.clone(), Some(json!(value.status))),
        ProjectViewObjectDataV3::Stage(value) => (value.title.clone(), Some(json!(value.status))),
        ProjectViewObjectDataV3::Requirement(value) => {
            (value.title.clone(), Some(json!(value.status)))
        }
        ProjectViewObjectDataV3::Issue(value) => (value.title.clone(), Some(json!(value.status))),
        ProjectViewObjectDataV3::Work(value) => (value.title.clone(), Some(json!(value.status))),
        ProjectViewObjectDataV3::Resource(value) => (value.name.clone(), None),
    }
}

async fn hydrate_documents(
    client: &CarryforthClient,
    identity: ProjectViewIdentity,
    project_id: Uuid,
    requested: &BTreeSet<Uuid>,
) -> Result<DocumentHydration, CliError> {
    if requested.is_empty() {
        return Ok(DocumentHydration {
            observation: DocumentObservation {
                state: "not_requested",
                catalog_revision: None,
                projection_generation: None,
            },
            heads: BTreeMap::new(),
        });
    }

    match read_document_heads_snapshot(client, identity, project_id, requested).await {
        Ok((meta, heads)) => Ok(DocumentHydration {
            observation: DocumentObservation {
                state: "observed",
                catalog_revision: Some(meta.projection.catalog_revision),
                projection_generation: Some(meta.projection.projection_generation),
            },
            heads,
        }),
        Err(error) if hydration_is_unavailable(&error) => Ok(DocumentHydration {
            observation: DocumentObservation {
                state: "unavailable",
                catalog_revision: None,
                projection_generation: None,
            },
            heads: BTreeMap::new(),
        }),
        Err(error) => Err(error),
    }
}

async fn read_document_heads_snapshot(
    client: &CarryforthClient,
    identity: ProjectViewIdentity,
    project_id: Uuid,
    requested: &BTreeSet<Uuid>,
) -> Result<(VerifiedDocumentMeta, BTreeMap<Uuid, DocumentMetadata>), CliError> {
    for attempt in 0..HYDRATION_SNAPSHOT_ATTEMPTS {
        let before = read_document_meta(client, identity, project_id).await?;
        let mut heads = BTreeMap::new();
        let requested_ids = requested.iter().copied().collect::<Vec<_>>();
        for chunk in requested_ids.chunks(DOCUMENT_HEAD_CHUNK_SIZE) {
            let coordinates = chunk
                .iter()
                .map(|document_id| {
                    document_head_coordinate(CommunityId::from_uuid(project_id), *document_id)
                })
                .collect::<Vec<_>>();
            let values = query_values(
                client,
                json!({
                    "kinds": [KIND_PROJECT_DOCUMENT_HEAD],
                    "authors": [identity.relay_pubkey.to_hex()],
                    "#d": coordinates,
                    "limit": chunk.len() + 1,
                }),
                "Document metadata hydration",
            )
            .await?;
            if values.len() > chunk.len() {
                return Err(integrity_error(
                    "Document hydration returned multiple current heads for one coordinate",
                ));
            }
            for value in values {
                let event: Event = serde_json::from_value(value)
                    .map_err(|_| integrity_error("Document hydration returned an invalid event"))?;
                let head = parse_document_head(
                    &event,
                    &identity.relay_pubkey,
                    CommunityId::from_uuid(project_id),
                )
                .map_err(|error| integrity_error(error.to_string()))?;
                let document_id = document_head_id(&head);
                if !requested.contains(&document_id) || heads.insert(document_id, head).is_some() {
                    return Err(integrity_error(
                        "Document hydration returned a duplicate or unrequested head",
                    ));
                }
            }
        }
        let after = read_document_meta(client, identity, project_id).await?;
        if before.event_id == after.event_id
            && before.projection.catalog_revision == after.projection.catalog_revision
            && before.projection.projection_generation == after.projection.projection_generation
        {
            let verified = heads
                .into_iter()
                .map(|(document_id, head)| {
                    verify_document_head_observation(&before, &head)
                        .map_err(|error| integrity_error(error.to_string()))?;
                    Ok((document_id, document_metadata(&head)))
                })
                .collect::<Result<BTreeMap<_, _>, CliError>>()?;
            return Ok((before, verified));
        }
        if attempt + 1 < HYDRATION_SNAPSHOT_ATTEMPTS {
            tokio::time::sleep(Duration::from_millis(25_u64 << attempt)).await;
        }
    }
    Err(CliError::Conflict(
        "conflict:project_document:hydration_snapshot_changed".to_owned(),
    ))
}

async fn read_document_meta(
    client: &CarryforthClient,
    identity: ProjectViewIdentity,
    project_id: Uuid,
) -> Result<VerifiedDocumentMeta, CliError> {
    let values = query_values(
        client,
        json!({
            "kinds": [KIND_PROJECT_DOCUMENT_META],
            "authors": [identity.relay_pubkey.to_hex()],
            "limit": 2,
        }),
        "Document catalog metadata",
    )
    .await?;
    let [value] = values.as_slice() else {
        return Err(CliError::Other(
            "unavailable:project_document:verified_meta".to_owned(),
        ));
    };
    let event: Event = serde_json::from_value(value.clone())
        .map_err(|_| integrity_error("Document metadata response contains an invalid event"))?;
    let meta = parse_document_meta(&event, &identity.relay_pubkey)
        .map_err(|error| integrity_error(error.to_string()))?;
    if meta.projection.project_id != project_id {
        return Err(integrity_error(
            "Document and Context metadata identify different Projects",
        ));
    }
    Ok(meta)
}

fn document_head_id(head: &VerifiedDocumentHead) -> Uuid {
    match &head.projection {
        DocumentHeadProjection::Active { document_id, .. }
        | DocumentHeadProjection::Deleted { document_id, .. } => *document_id,
    }
}

fn document_metadata(head: &VerifiedDocumentHead) -> DocumentMetadata {
    match &head.projection {
        DocumentHeadProjection::Active {
            title,
            summary,
            document_revision,
            updated_at,
            updated_by,
            ..
        } => DocumentMetadata::Active {
            title: title.clone(),
            summary: summary.clone(),
            document_revision: *document_revision,
            updated_at: *updated_at,
            updated_by: *updated_by,
        },
        DocumentHeadProjection::Deleted {
            document_revision,
            deleted_at,
            deleted_by,
            ..
        } => DocumentMetadata::Tombstoned {
            document_revision: *document_revision,
            deleted_at: *deleted_at,
            deleted_by: *deleted_by,
        },
    }
}

fn document_coordinate_output(
    coordinate: ProjectContextCoordinate,
    metadata: Option<&DocumentMetadata>,
) -> CoordinateOutput {
    match metadata {
        Some(DocumentMetadata::Active {
            title,
            document_revision,
            updated_at,
            updated_by,
            ..
        }) => CoordinateOutput {
            coordinate,
            state: "active",
            title: Some(title.clone()),
            description: None,
            summary: None,
            status: None,
            object_revision: None,
            document_revision: Some(*document_revision),
            updated_at: Some(*updated_at),
            updated_by: Some(*updated_by),
            meeting_fetch: None,
            unavailable_reason: None,
        },
        Some(DocumentMetadata::Tombstoned {
            document_revision,
            deleted_at,
            deleted_by,
        }) => CoordinateOutput {
            coordinate,
            state: "tombstoned",
            title: None,
            description: None,
            summary: None,
            status: None,
            object_revision: None,
            document_revision: Some(*document_revision),
            updated_at: Some(*deleted_at),
            updated_by: Some(*deleted_by),
            meeting_fetch: None,
            unavailable_reason: None,
        },
        None => unavailable_coordinate(coordinate),
    }
}

async fn hydrate_meetings(
    client: &CarryforthClient,
    requested: &BTreeSet<Uuid>,
) -> Result<MeetingHydration, CliError> {
    if requested.is_empty() {
        return Ok(MeetingHydration {
            observation: MeetingObservation {
                state: "not_requested",
                requested_count: 0,
                observed_count: 0,
            },
            summaries: BTreeMap::new(),
        });
    }

    match fetch_meeting_context_summaries(client, requested).await {
        Ok(summaries) => {
            let observed_count = summaries.len();
            Ok(MeetingHydration {
                observation: MeetingObservation {
                    state: if observed_count == requested.len() {
                        "observed"
                    } else {
                        "partial"
                    },
                    requested_count: requested.len(),
                    observed_count,
                },
                summaries,
            })
        }
        Err(error) if hydration_is_unavailable(&error) => Ok(MeetingHydration {
            observation: MeetingObservation {
                state: "unavailable",
                requested_count: requested.len(),
                observed_count: 0,
            },
            summaries: BTreeMap::new(),
        }),
        Err(error) => Err(error),
    }
}

fn meeting_coordinate_output(
    coordinate: ProjectContextCoordinate,
    summary: Option<&MeetingSummary>,
) -> CoordinateOutput {
    let ProjectContextCoordinate::Meeting { meeting_id } = coordinate else {
        return unavailable_coordinate(coordinate);
    };
    let Some(summary) = summary else {
        return unavailable_coordinate(ProjectContextCoordinate::Meeting { meeting_id });
    };
    CoordinateOutput {
        coordinate: ProjectContextCoordinate::Meeting { meeting_id },
        state: if summary.status == "ended" {
            "terminal"
        } else {
            "active"
        },
        title: Some(summary.title.clone()),
        description: summary.description.clone(),
        summary: summary.summary.clone(),
        status: Some(json!(summary.status)),
        object_revision: None,
        document_revision: None,
        updated_at: i64::try_from(summary.updated_at)
            .ok()
            .and_then(|timestamp| DateTime::from_timestamp(timestamp, 0)),
        updated_by: None,
        meeting_fetch: Some(MeetingFetchCommands {
            metadata: format!("cf meetings show --meeting {meeting_id}"),
            board: format!("cf meetings board get --meeting {meeting_id}"),
            speech: format!(
                "cf --format compact meetings history --meeting {meeting_id} --limit 200"
            ),
        }),
        unavailable_reason: None,
    }
}

fn context_document_output(
    document_id: Uuid,
    metadata: Option<&DocumentMetadata>,
) -> Result<ContextDocumentOutput, CliError> {
    let fetch_command = format!("cf documents get {document_id} --content-only");
    match metadata {
        Some(DocumentMetadata::Active {
            title,
            summary,
            document_revision,
            updated_at,
            updated_by,
        }) => Ok(ContextDocumentOutput {
            document_id,
            state: "active",
            title: Some(title.clone()),
            summary: summary.clone(),
            document_revision: Some(*document_revision),
            updated_at: Some(*updated_at),
            updated_by: Some(*updated_by),
            fetch_command,
            unavailable_reason: None,
        }),
        Some(DocumentMetadata::Tombstoned { .. }) => Err(integrity_error(
            "an active Context binding points to a verified tombstoned Document",
        )),
        None => Ok(ContextDocumentOutput {
            document_id,
            state: "unavailable",
            title: None,
            summary: None,
            document_revision: None,
            updated_at: None,
            updated_by: None,
            fetch_command,
            unavailable_reason: Some("metadata_unavailable"),
        }),
    }
}

fn unavailable_coordinate(coordinate: ProjectContextCoordinate) -> CoordinateOutput {
    CoordinateOutput {
        coordinate,
        state: "unavailable",
        title: None,
        description: None,
        summary: None,
        status: None,
        object_revision: None,
        document_revision: None,
        updated_at: None,
        updated_by: None,
        meeting_fetch: None,
        unavailable_reason: Some("metadata_unavailable"),
    }
}

fn hydration_is_unavailable(error: &CliError) -> bool {
    match error {
        CliError::Network(_) | CliError::Conflict(_) => true,
        CliError::Relay { status, .. } => matches!(*status, 408 | 409 | 429 | 500..=504),
        CliError::Other(message) => message.starts_with("unavailable:"),
        _ => false,
    }
}

async fn run_write(
    client: &CarryforthClient,
    operation: ProjectContextOperation,
    context_document_id: Uuid,
    coordinates: Vec<ProjectContextCoordinate>,
    attribution: ProjectContextAttributionArgs,
) -> Result<(), CliError> {
    // Validate the optional attribution tuple before any network request or Event signing. An
    // ordinary Community write omits the tuple; an explicit supervised write must prove all of
    // it. Keeping this local makes a partial tuple safe to correct without an ambiguous delivery.
    let (assignment_id, runtime_fence) = attribution.into_runtime_fence()?;
    let identity = require_identity(client).await?;
    if operation == ProjectContextOperation::Attach && !identity.context_edge_enabled {
        return Err(CliError::Other(
            "unavailable:project_context:capability_disabled".to_owned(),
        ));
    }
    let meta = read_context_meta(client, identity).await?;
    let project_id = meta.projection.project_id;
    for coordinate in &coordinates {
        coordinate
            .validate_for_project(project_id)
            .map_err(|error| CliError::Usage(error.to_string()))?;
    }
    let mut command = ProjectContextCommand::new(
        meta.projection.context_revision,
        operation,
        coordinates.clone(),
        context_document_id,
    )
    .map_err(|error| CliError::Usage(error.to_string()))?;
    if let (Some(assignment_id), Some(runtime_fence)) = (assignment_id, runtime_fence) {
        command = command.with_runtime_fence(assignment_id, runtime_fence);
    }
    command
        .validate_for_project(project_id)
        .map_err(|error| CliError::Usage(error.to_string()))?;
    let expected_revision = command.expected_context_revision;
    let expected_edge_key = EdgeKey::derive(project_id, &coordinates)
        .map_err(|error| CliError::Usage(error.to_string()))?;
    let event = client.sign_event_exact(
        build_project_context_command(CommunityId::from_uuid(project_id), command.clone())
            .map_err(|error| CliError::Usage(error.to_string()))?,
    )?;

    match client.submit_project_command(&event).await? {
        ProjectCommandDelivery::Accepted { receipt, .. } => {
            let receipt: ProjectContextReceipt = serde_json::from_value(receipt).map_err(|_| {
                integrity_error("Relay returned a receipt for another Project protocol")
            })?;
            validate_write_receipt(
                &receipt,
                &event,
                &command,
                expected_edge_key,
                context_document_id,
            )?;
            print_json(&json!({
                "event_id": event.id.to_hex(),
                "accepted": true,
                "confirmation": "receipt",
                "receipt": receipt,
            }))
        }
        ProjectCommandDelivery::Ambiguous { reason } => {
            if !confirm_committed_command(client, &event, project_id).await? {
                return Err(CliError::DeliveryUnknown(format!(
                    "Project Context command {} may have reached the Relay ({reason}); exact command read-back did not prove acceptance",
                    event.id.to_hex()
                )));
            }
            print_json(&json!({
                "event_id": event.id.to_hex(),
                "accepted": true,
                "confirmation": "command_readback",
                "operation": operation,
                "expected_context_revision": expected_revision,
                "context_revision": expected_revision + 1,
                "edge_key": expected_edge_key,
                "context_document_id": context_document_id,
            }))
        }
    }
}

fn validate_write_receipt(
    receipt: &ProjectContextReceipt,
    event: &Event,
    command: &ProjectContextCommand,
    expected_edge_key: EdgeKey,
    context_document_id: Uuid,
) -> Result<(), CliError> {
    receipt
        .validate()
        .map_err(|error| integrity_error(error.to_string()))?;
    if receipt.change_id != event.id
        || receipt.actor != event.pubkey
        || receipt.acting_assignment_id != command.acting_assignment_id
        || receipt.operation != command.operation()
        || receipt.expected_context_revision != command.expected_context_revision
        || receipt.edge_key != expected_edge_key
        || receipt.context_document_id != context_document_id
        || (command.operation() == ProjectContextOperation::Attach
            && receipt.edge_state != ProjectContextBindingState::Active)
    {
        return Err(integrity_error(
            "Project Context receipt does not match the submitted command",
        ));
    }
    Ok(())
}

async fn confirm_committed_command(
    client: &CarryforthClient,
    event: &Event,
    project_id: Uuid,
) -> Result<bool, CliError> {
    let values = query_values(
        client,
        json!({
            "ids": [event.id.to_hex()],
            "kinds": [KIND_PROJECT_CONTEXT_COMMAND],
            "authors": [event.pubkey.to_hex()],
            "limit": 2,
        }),
        "Project Context command read-back",
    )
    .await?;
    match values.as_slice() {
        [] => Ok(false),
        [value] => {
            let read_back: Event = serde_json::from_value(value.clone())
                .map_err(|_| integrity_error("command read-back returned an invalid event"))?;
            if read_back != *event {
                return Err(integrity_error(
                    "command read-back returned different signed bytes",
                ));
            }
            parse_project_context_command(&read_back, CommunityId::from_uuid(project_id))
                .map_err(|error| integrity_error(error.to_string()))?;
            Ok(true)
        }
        _ => Err(integrity_error(
            "command read-back returned multiple events for one ID",
        )),
    }
}

impl ProjectContextAttributionArgs {
    fn into_runtime_fence(self) -> Result<(Option<Uuid>, Option<RuntimeFence>), CliError> {
        match (
            self.acting_assignment_id,
            self.runtime_id,
            self.runtime_epoch,
        ) {
            (None, None, None) => Ok((None, None)),
            (Some(assignment_id), Some(runtime_id), Some(runtime_epoch)) => {
                let fence = RuntimeFence {
                    runtime_id,
                    runtime_epoch,
                };
                fence.validate().map_err(CliError::Usage)?;
                Ok((Some(assignment_id), Some(fence)))
            }
            _ => Err(CliError::Usage(
                "ordinary Community Context writes: omit --acting-assignment, --runtime-id, and --runtime-epoch; supervised attribution requires all three options together"
                    .to_owned(),
            )),
        }
    }
}

fn canonicalize_edge_tokens(
    tokens: Vec<String>,
) -> Result<Vec<ProjectContextCoordinate>, CliError> {
    let coordinates = tokens
        .iter()
        .map(|token| parse_coordinate_token(token))
        .collect::<Result<Vec<_>, _>>()?;
    canonicalize_coordinates(coordinates).map_err(|error| CliError::Usage(error.to_string()))
}

fn canonicalize_subset_tokens(
    tokens: Vec<String>,
) -> Result<Vec<ProjectContextCoordinate>, CliError> {
    let mut coordinates = tokens
        .iter()
        .map(|token| parse_coordinate_token(token))
        .collect::<Result<Vec<_>, _>>()?;
    coordinates.sort();
    if coordinates.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(CliError::Usage(
            "duplicate Project Context coordinate".to_owned(),
        ));
    }
    Ok(coordinates)
}

fn parse_coordinate_token(token: &str) -> Result<ProjectContextCoordinate, CliError> {
    let (kind, id) = token
        .split_once(':')
        .ok_or_else(|| coordinate_usage(token))?;
    if id.contains(':') {
        return Err(coordinate_usage(token));
    }
    let id = Uuid::parse_str(id).map_err(|_| coordinate_usage(token))?;
    if id.get_version() != Some(Version::Random) || id.to_string() != token_id(token) {
        return Err(CliError::Usage(format!(
            "Project Context coordinate {token:?} must use a canonical lowercase UUID v4"
        )));
    }
    let coordinate = match kind {
        "project_profile" => ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::ProjectProfile,
            object_id: id,
        },
        "goal" => ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Goal,
            object_id: id,
        },
        "role" => ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Role,
            object_id: id,
        },
        "plan" => ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Plan,
            object_id: id,
        },
        "stage" => ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Stage,
            object_id: id,
        },
        "requirement" => ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Requirement,
            object_id: id,
        },
        "issue" => ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Issue,
            object_id: id,
        },
        "work" => ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Work,
            object_id: id,
        },
        "resource" => ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Resource,
            object_id: id,
        },
        "document" => ProjectContextCoordinate::Document { document_id: id },
        "meeting" => ProjectContextCoordinate::Meeting { meeting_id: id },
        _ => return Err(coordinate_usage(token)),
    };
    coordinate
        .validate()
        .map_err(|error| CliError::Usage(error.to_string()))?;
    Ok(coordinate)
}

fn token_id(token: &str) -> &str {
    token.split_once(':').map_or("", |(_, id)| id)
}

fn coordinate_usage(token: &str) -> CliError {
    CliError::Usage(format!(
        "invalid Project Context coordinate {token:?}; expected TYPE:<uuid-v4>, where TYPE is project_profile, goal, role, plan, stage, requirement, issue, work, resource, document, or meeting"
    ))
}

async fn query_values(
    client: &CarryforthClient,
    filter: Value,
    context: &str,
) -> Result<Vec<Value>, CliError> {
    let raw = client.query(&filter).await?;
    serde_json::from_str(&raw)
        .map_err(|_| integrity_error(format!("{context} response is not a JSON event array")))
}

fn print_json(value: &impl Serialize) -> Result<(), CliError> {
    println!(
        "{}",
        serde_json::to_string(value)
            .map_err(|error| CliError::Other(format!("failed to serialize output: {error}")))?
    );
    Ok(())
}

fn integrity_error(message: impl Into<String>) -> CliError {
    CliError::Other(format!(
        "Project Context integrity error: {}",
        message.into()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use axum::extract::State;
    use axum::routing::post;
    use axum::{Json, Router};
    use buzz_core::EventId;
    use buzz_project_context::{
        ProjectContextBindingProjection, ProjectContextCatalog, ProjectContextProjectionPlan,
        ProjectContextProjectionType, PROJECT_CONTEXT_SCHEMA_VERSION,
    };
    use buzz_sdk::project_context::{
        build_project_context_binding_reprojection, build_project_context_meta_projection,
    };
    use nostr::{EventBuilder, Keys, Kind, Timestamp};
    use tokio::net::TcpListener;

    #[test]
    fn attribution_accepts_ordinary_and_complete_supervised_writes() {
        let ordinary = ProjectContextAttributionArgs {
            acting_assignment_id: None,
            runtime_id: None,
            runtime_epoch: None,
        }
        .into_runtime_fence()
        .expect("ordinary Community attribution");
        assert_eq!(ordinary, (None, None));

        let assignment_id = Uuid::new_v4();
        let runtime_id = Uuid::new_v4();
        let supervised = ProjectContextAttributionArgs {
            acting_assignment_id: Some(assignment_id),
            runtime_id: Some(runtime_id),
            runtime_epoch: Some(7),
        }
        .into_runtime_fence()
        .expect("complete supervised attribution");
        assert_eq!(supervised.0, Some(assignment_id));
        assert_eq!(
            supervised.1,
            Some(RuntimeFence {
                runtime_id,
                runtime_epoch: 7,
            })
        );
    }

    #[test]
    fn partial_attribution_fails_locally_with_both_legal_corrections() {
        let assignment_id = Uuid::new_v4();
        let runtime_id = Uuid::new_v4();
        for attribution in [
            ProjectContextAttributionArgs {
                acting_assignment_id: Some(assignment_id),
                runtime_id: None,
                runtime_epoch: None,
            },
            ProjectContextAttributionArgs {
                acting_assignment_id: None,
                runtime_id: Some(runtime_id),
                runtime_epoch: None,
            },
            ProjectContextAttributionArgs {
                acting_assignment_id: None,
                runtime_id: None,
                runtime_epoch: Some(7),
            },
            ProjectContextAttributionArgs {
                acting_assignment_id: Some(assignment_id),
                runtime_id: Some(runtime_id),
                runtime_epoch: None,
            },
            ProjectContextAttributionArgs {
                acting_assignment_id: Some(assignment_id),
                runtime_id: None,
                runtime_epoch: Some(7),
            },
            ProjectContextAttributionArgs {
                acting_assignment_id: None,
                runtime_id: Some(runtime_id),
                runtime_epoch: Some(7),
            },
        ] {
            let error = attribution
                .into_runtime_fence()
                .expect_err("partial attribution must fail before Event signing");
            let CliError::Usage(message) = error else {
                panic!("partial attribution must be a user error");
            };
            assert!(message.contains("ordinary Community Context writes"));
            assert!(message.contains("omit --acting-assignment"));
            assert!(message.contains("supervised attribution requires all three"));
        }
    }

    #[derive(Clone)]
    struct QueryServerState {
        meta_events: Arc<Vec<Event>>,
        meta_calls: Arc<AtomicUsize>,
        binding_attempt_pages: Arc<Vec<Vec<Vec<Event>>>>,
        binding_page_calls: Arc<Mutex<Vec<u64>>>,
    }

    async fn project_context_query_handler(
        State(state): State<QueryServerState>,
        Json(filters): Json<Vec<Value>>,
    ) -> Json<Value> {
        let filter = filters.first().expect("one query filter");
        let kind = filter["kinds"][0].as_u64().expect("numeric kind");
        if kind == u64::from(KIND_PROJECT_CONTEXT_META) {
            let call = state.meta_calls.fetch_add(1, Ordering::SeqCst);
            let event = state
                .meta_events
                .get(call)
                .or_else(|| state.meta_events.last())
                .expect("at least one metadata event");
            return Json(json!([event]));
        }

        assert_eq!(kind, u64::from(KIND_PROJECT_CONTEXT_EDGE_BINDING));
        let page = filter["page"].as_u64().expect("numeric page");
        let attempt = {
            let mut calls = state
                .binding_page_calls
                .lock()
                .expect("binding page call lock");
            calls.push(page);
            calls
                .iter()
                .filter(|&&called_page| called_page == 1)
                .count()
                .saturating_sub(1)
        };
        let pages = state
            .binding_attempt_pages
            .get(attempt)
            .or_else(|| state.binding_attempt_pages.last())
            .expect("at least one binding-page attempt");
        let events = page
            .checked_sub(1)
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| pages.get(index))
            .cloned()
            .unwrap_or_default();
        Json(json!(events))
    }

    async fn query_test_server(state: QueryServerState) -> String {
        let app = Router::new()
            .route("/query", post(project_context_query_handler))
            .with_state(state);
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind query test server");
        let address = listener.local_addr().expect("query server address");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve query test requests");
        });
        format!("http://{address}")
    }

    struct QueryFixture {
        project_id: Uuid,
        relay: Keys,
        coordinates: Vec<ProjectContextCoordinate>,
        old_meta: Event,
        new_meta: Event,
        binding_one: Event,
        binding_two: Event,
    }

    fn query_fixture() -> QueryFixture {
        let project_id = Uuid::new_v4();
        let project = CommunityId::from_uuid(project_id);
        let relay = Keys::generate();
        let coordinates = canonicalize_coordinates(vec![
            ProjectContextCoordinate::ProjectViewObject {
                object_type: ProjectViewObjectType::ProjectProfile,
                object_id: project_id,
            },
            ProjectContextCoordinate::ProjectViewObject {
                object_type: ProjectViewObjectType::Goal,
                object_id: Uuid::new_v4(),
            },
        ])
        .expect("canonical fixture coordinates");
        let edge_key = EdgeKey::derive(project_id, &coordinates).expect("fixture Edge key");
        let initialized_at = DateTime::from_timestamp(1_800_000_000, 0).expect("fixture time");
        let old_catalog = ProjectContextCatalog::from_snapshot(
            project,
            1,
            1,
            1,
            1,
            initialized_at,
            DateTime::from_timestamp(1_800_000_001, 0).expect("old catalog time"),
        )
        .expect("old catalog");
        let new_catalog = ProjectContextCatalog::from_snapshot(
            project,
            2,
            1,
            2,
            1,
            initialized_at,
            DateTime::from_timestamp(1_800_000_002, 0).expect("new catalog time"),
        )
        .expect("new catalog");
        let old_meta = build_project_context_meta_projection(
            &ProjectContextProjectionPlan::for_reset(&old_catalog).expect("old reset plan"),
            &[],
        )
        .expect("old metadata builder")
        .sign_with_keys(&relay)
        .expect("sign old metadata");
        let new_meta = build_project_context_meta_projection(
            &ProjectContextProjectionPlan::for_reset(&new_catalog).expect("new reset plan"),
            &[],
        )
        .expect("new metadata builder")
        .sign_with_keys(&relay)
        .expect("sign new metadata");

        let source_keys = Keys::generate();
        let binding_event = |document_id: Uuid, revision: u64| {
            let source_event_id: EventId =
                EventBuilder::new(Kind::TextNote, format!("fixture Context source {revision}"))
                    .custom_created_at(Timestamp::from(1_799_999_990 + revision))
                    .sign_with_keys(&source_keys)
                    .expect("sign fixture source")
                    .id;
            build_project_context_binding_reprojection(&ProjectContextBindingProjection {
                schema_version: PROJECT_CONTEXT_SCHEMA_VERSION,
                projection_type: ProjectContextProjectionType::ContextEdgeBinding,
                project_id,
                projection_generation: 1,
                context_revision: revision,
                edge_key,
                coordinates: coordinates.clone(),
                context_document_id: document_id,
                state: ProjectContextBindingState::Active,
                source_event_id,
                updated_at: DateTime::from_timestamp(
                    1_800_000_000 + i64::try_from(revision).expect("small fixture revision"),
                    0,
                )
                .expect("binding time"),
            })
            .expect("binding builder")
            .sign_with_keys(&relay)
            .expect("sign binding")
        };
        let binding_one = binding_event(Uuid::new_v4(), 1);
        let binding_two = binding_event(Uuid::new_v4(), 2);
        QueryFixture {
            project_id,
            relay,
            coordinates,
            old_meta,
            new_meta,
            binding_one,
            binding_two,
        }
    }

    fn query_identity(fixture: &QueryFixture) -> ProjectViewIdentity {
        ProjectViewIdentity {
            relay_pubkey: fixture.relay.public_key(),
            schema: ProjectViewSchema::V3,
            context_enabled: false,
            context_edge_enabled: true,
            context_edge_migration_required: false,
            document_enabled: true,
            semantic_query_http_enabled: false,
            coordinate_search_http_enabled: false,
            extensions_temporarily_unavailable: false,
        }
    }

    fn query_client(base_url: String) -> CarryforthClient {
        CarryforthClient::new(base_url, Keys::generate(), None, None).expect("query test client")
    }

    fn goal(project_id: Uuid, object_id: Uuid) -> ProjectContextCoordinate {
        let coordinate = ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Goal,
            object_id,
        };
        coordinate
            .validate_for_project(project_id)
            .expect("valid Goal coordinate");
        coordinate
    }

    fn edge(project_id: Uuid, coordinates: Vec<ProjectContextCoordinate>) -> ProjectContextEdge {
        ProjectContextEdge::from_snapshot(project_id, coordinates, vec![Uuid::new_v4()])
            .expect("valid test Edge")
    }

    #[test]
    fn coordinate_tokens_cover_the_closed_v2_union() {
        let id = Uuid::new_v4();
        let requirement =
            parse_coordinate_token(&format!("requirement:{id}")).expect("Requirement token parses");
        assert!(matches!(
            requirement,
            ProjectContextCoordinate::ProjectViewObject {
                object_type: ProjectViewObjectType::Requirement,
                object_id,
            } if object_id == id
        ));
        assert!(matches!(
            parse_coordinate_token(&format!("document:{id}")),
            Ok(ProjectContextCoordinate::Document { document_id }) if document_id == id
        ));
        assert!(matches!(
            parse_coordinate_token(&format!("meeting:{id}")),
            Ok(ProjectContextCoordinate::Meeting { meeting_id }) if meeting_id == id
        ));
        assert!(parse_coordinate_token(&format!("unknown:{id}")).is_err());
        assert!(parse_coordinate_token("requirement:not-a-uuid").is_err());
        assert!(parse_coordinate_token(&format!("requirement:{}", Uuid::nil())).is_err());
    }

    #[test]
    fn semantic_budget_overrides_are_closed_and_validated_by_the_query_contract() {
        let budget = semantic_query_budget(SemanticGraphBudgetArgs {
            max_semantic_roots: Some(4),
            max_hops_per_path: Some(2),
            max_response_bytes: Some(64 * 1024),
            ..SemanticGraphBudgetArgs::default()
        });
        assert_eq!(budget.max_semantic_roots, 4);
        assert_eq!(budget.max_hops_per_path, 2);
        assert_eq!(budget.max_response_bytes, 64 * 1024);
        assert!(budget.validate().is_ok());

        let invalid = semantic_query_budget(SemanticGraphBudgetArgs {
            max_hops_per_path: Some(0),
            ..SemanticGraphBudgetArgs::default()
        });
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn semantic_read_commands_are_canonical_deduplicated_and_sorted() {
        let object_id = Uuid::new_v4();
        let document_id = Uuid::new_v4();
        let meeting_id = Uuid::new_v4();
        let object = ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Requirement,
            object_id,
        };
        let document = ProjectContextCoordinate::Document { document_id };
        let meeting = ProjectContextCoordinate::Meeting { meeting_id };
        let coordinates =
            BTreeSet::from([meeting.clone(), document.clone(), object.clone(), document]);
        let commands = semantic_read_commands_for_coordinates(coordinates);
        let value = serde_json::to_value(commands).expect("serialize read commands");
        assert_eq!(value.as_array().expect("command array").len(), 3);
        assert_eq!(
            value[0]["coordinate"],
            serde_json::to_value(object).unwrap()
        );
        assert_eq!(
            value[0]["command"],
            format!("cf project-view get-object requirement {object_id}")
        );
        assert_eq!(
            value[1]["command"],
            format!("cf documents get {document_id}")
        );
        assert_eq!(
            value[2]["command"],
            format!("cf meetings show --meeting {meeting_id}")
        );
    }

    #[test]
    fn semantic_read_command_projection_is_explicitly_unsigned() {
        let projection = UnsignedSemanticReadCommands {
            signed: false,
            derivation: "deterministic_from_verified_result_identities",
            commands: Vec::new(),
        };
        let value = serde_json::to_value(projection).expect("serialize read command projection");
        assert_eq!(value["signed"], false);
        assert_eq!(
            value["derivation"],
            "deterministic_from_verified_result_identities"
        );
        assert_eq!(value["commands"], serde_json::json!([]));
    }

    #[test]
    fn semantic_response_parser_is_single_event_and_closed() {
        let event = EventBuilder::new(Kind::Custom(KIND_SEMANTIC_GRAPH_QUERY_RESULT as u16), "{}")
            .sign_with_keys(&Keys::generate())
            .expect("sign virtual fixture");
        let exact = serde_json::to_vec(&[&event]).expect("serialize Event array");
        assert_eq!(
            parse_single_semantic_result_event(&exact).expect("parse exact Event"),
            event
        );
        assert!(parse_single_semantic_result_event(b"[]").is_err());
        let doubled = serde_json::to_vec(&[&event, &event]).expect("serialize two Events");
        assert!(parse_single_semantic_result_event(&doubled).is_err());

        let mut value = serde_json::to_value(&event).expect("Event value");
        value
            .as_object_mut()
            .expect("Event object")
            .insert("semantic_extra".to_owned(), Value::Bool(true));
        let unknown = serde_json::to_vec(&[value]).expect("serialize unknown Event field");
        assert!(parse_single_semantic_result_event(&unknown).is_err());
    }

    #[test]
    fn contains_all_subset_allows_zero_or_one_but_rejects_duplicates() {
        assert!(canonicalize_subset_tokens(Vec::new()).is_ok());
        let id = Uuid::new_v4();
        assert_eq!(
            canonicalize_subset_tokens(vec![format!("requirement:{id}")])
                .expect("one-coordinate subset")
                .len(),
            1
        );
        assert!(canonicalize_subset_tokens(vec![
            format!("requirement:{id}"),
            format!("requirement:{id}"),
        ])
        .is_err());
    }

    #[test]
    fn compact_query_shapes_cannot_contain_document_body() {
        let document_id = Uuid::new_v4();
        let actor = Keys::generate().public_key();
        let metadata = DocumentMetadata::Active {
            title: "Context".to_owned(),
            summary: Some("Summary".to_owned()),
            document_revision: 2,
            updated_at: Utc::now(),
            updated_by: actor,
        };
        let legacy_relation = context_document_output(document_id, Some(&metadata))
            .expect("legacy Context Document output");
        let legacy_relation_json =
            serde_json::to_value(legacy_relation).expect("serialize legacy relation");
        assert_eq!(
            legacy_relation_json["fetch_command"],
            format!("cf documents get {document_id} --content-only")
        );

        let legacy_coordinate = document_coordinate_output(
            ProjectContextCoordinate::Document { document_id },
            Some(&metadata),
        );
        let legacy_coordinate_json =
            serde_json::to_value(legacy_coordinate).expect("serialize legacy Coordinate");
        assert!(legacy_coordinate_json.get("summary").is_none());
        assert!(legacy_coordinate_json.get("fetch_command").is_none());

        let output = ContextDocumentOutput {
            document_id,
            state: "active",
            title: Some("Context".to_owned()),
            summary: Some("Summary".to_owned()),
            document_revision: Some(2),
            updated_at: None,
            updated_by: None,
            fetch_command: format!("cf documents get {document_id} --content-only"),
            unavailable_reason: None,
        };
        let json = serde_json::to_value(output).expect("serialize metadata-only output");
        assert!(json.get("content_markdown").is_none());
        assert!(json.get("fetch_command").is_some());
    }

    #[test]
    fn project_view_coordinate_output_hydrates_the_source_owned_summary() {
        let object_id = Uuid::new_v4();
        let actor = Keys::generate().public_key();
        let now = Utc::now();
        let coordinate = ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Requirement,
            object_id,
        };
        let entry =
            ProjectViewEntryV3::Active(Box::new(buzz_project_view::v3::ProjectViewObjectV3 {
                id: object_id,
                object_type: ProjectViewObjectType::Requirement,
                object_revision: 2,
                project_revision: 7,
                created_at: now,
                updated_at: now,
                created_by: actor,
                updated_by: actor,
                data: ProjectViewObjectDataV3::Requirement(buzz_project_view::Requirement {
                    title: "Progressive retrieval".to_owned(),
                    description: "Expose source metadata before full content.".to_owned(),
                    status: buzz_project_view::RequirementStatus::Ready,
                    priority: buzz_project_view::Priority::High,
                    summary: Some(
                        "Relevant when deciding how graph coordinates are loaded.".to_owned(),
                    ),
                }),
                relations: buzz_project_view::ProjectViewRelations::default(),
                context_references: Vec::new(),
            }));

        let output = project_view_coordinate_output(&coordinate, &entry);
        let value = serde_json::to_value(output).expect("serialize Coordinate preview");
        assert_eq!(
            value["summary"],
            "Relevant when deciding how graph coordinates are loaded."
        );
        assert_eq!(value["object_revision"], 2);
        assert!(value.get("content").is_none());
    }

    #[test]
    fn meeting_coordinate_output_is_typed_metadata_first_and_on_demand() {
        let meeting_id = Uuid::new_v4();
        let output = meeting_coordinate_output(
            ProjectContextCoordinate::Meeting { meeting_id },
            Some(&MeetingSummary {
                meeting_id: meeting_id.to_string(),
                title: "Architecture review".to_owned(),
                description: Some("Set the first delivery boundary".to_owned()),
                summary: Some("Decision and materialized architecture context.".to_owned()),
                room_kind: "meeting".to_owned(),
                status: "ended",
                updated_at: 1_800_000_000,
            }),
        );
        let value = serde_json::to_value(output).expect("serialize Meeting coordinate output");
        assert_eq!(value["coordinate"]["coordinate_type"], "meeting");
        assert_eq!(value["coordinate"]["meeting_id"], meeting_id.to_string());
        assert_eq!(value["state"], "terminal");
        assert_eq!(value["title"], "Architecture review");
        assert_eq!(
            value["summary"],
            "Decision and materialized architecture context."
        );
        assert_eq!(value["status"], "ended");
        assert_eq!(
            value["meeting_fetch"]["metadata"],
            format!("cf meetings show --meeting {meeting_id}")
        );
        assert_eq!(
            value["meeting_fetch"]["board"],
            format!("cf meetings board get --meeting {meeting_id}")
        );
        assert!(value.get("content").is_none());
        assert!(value.get("board").is_none());
        assert!(value.get("speech").is_none());
    }

    #[test]
    fn exact_incident_and_contains_all_keep_their_distinct_set_semantics() {
        let project_id = Uuid::new_v4();
        let a = goal(project_id, Uuid::new_v4());
        let b = goal(project_id, Uuid::new_v4());
        let c = goal(project_id, Uuid::new_v4());
        let ab = canonicalize_coordinates(vec![a.clone(), b.clone()]).expect("canonical AB");
        let abc = canonicalize_coordinates(vec![a.clone(), b.clone(), c]).expect("canonical ABC");
        let binary = edge(project_id, ab.clone());
        let hyperedge = edge(project_id, abc);

        let mut exact = vec![binary.clone()];
        apply_query_semantics(&ContextQuery::Exact(ab.clone()), project_id, &mut exact)
            .expect("exact accepts the exact Edge");
        assert_eq!(exact, vec![binary.clone()]);

        let mut exact_superset = vec![hyperedge.clone()];
        assert!(apply_query_semantics(
            &ContextQuery::Exact(ab.clone()),
            project_id,
            &mut exact_superset,
        )
        .is_err());

        let mut incident = vec![binary.clone(), hyperedge.clone()];
        apply_query_semantics(
            &ContextQuery::Incident(a.clone()),
            project_id,
            &mut incident,
        )
        .expect("incident accepts binary and hyperedge matches");
        assert_eq!(incident.len(), 2);

        let mut supersets = vec![binary.clone(), hyperedge.clone()];
        apply_query_semantics(&ContextQuery::ContainsAll(ab), project_id, &mut supersets)
            .expect("contains-all filters locally");
        assert_eq!(supersets, vec![binary, hyperedge]);

        let mut all = supersets.clone();
        apply_query_semantics(&ContextQuery::ContainsAll(Vec::new()), project_id, &mut all)
            .expect("empty subset keeps all Edges");
        assert_eq!(all, supersets);
    }

    #[tokio::test]
    async fn every_loader_retries_the_complete_paginated_snapshot_and_deduplicates_events() {
        for query_case in 0..3 {
            let fixture = query_fixture();
            let query = match query_case {
                0 => ContextQuery::Exact(fixture.coordinates.clone()),
                1 => ContextQuery::Incident(fixture.coordinates[0].clone()),
                _ => ContextQuery::ContainsAll(vec![fixture.coordinates[0].clone()]),
            };
            let meta_calls = Arc::new(AtomicUsize::new(0));
            let binding_page_calls = Arc::new(Mutex::new(Vec::new()));
            let server = query_test_server(QueryServerState {
                meta_events: Arc::new(vec![
                    fixture.old_meta.clone(),
                    fixture.new_meta.clone(),
                    fixture.new_meta.clone(),
                    fixture.new_meta.clone(),
                ]),
                meta_calls: meta_calls.clone(),
                binding_attempt_pages: Arc::new(vec![
                    vec![
                        vec![fixture.binding_one.clone()],
                        vec![fixture.binding_one.clone()],
                    ],
                    vec![
                        vec![fixture.binding_one.clone()],
                        vec![fixture.binding_one.clone(), fixture.binding_two.clone()],
                        Vec::new(),
                    ],
                ]),
                binding_page_calls: binding_page_calls.clone(),
            })
            .await;

            let snapshot =
                read_edge_snapshot(&query_client(server), query_identity(&fixture), &query)
                    .await
                    .expect("second complete snapshot attempt stabilizes");

            assert_eq!(snapshot.meta.event_id, fixture.new_meta.id);
            assert_eq!(snapshot.meta.projection.project_id, fixture.project_id);
            assert_eq!(snapshot.edges.len(), 1);
            assert_eq!(snapshot.edges[0].context_document_ids().len(), 2);
            assert_eq!(meta_calls.load(Ordering::SeqCst), 4);
            assert_eq!(
                *binding_page_calls.lock().expect("binding page call lock"),
                vec![1, 2, 1, 2, 3]
            );
        }
    }

    #[tokio::test]
    async fn loader_fails_with_snapshot_conflict_after_three_complete_attempts() {
        let fixture = query_fixture();
        let meta_calls = Arc::new(AtomicUsize::new(0));
        let binding_page_calls = Arc::new(Mutex::new(Vec::new()));
        let server = query_test_server(QueryServerState {
            meta_events: Arc::new(vec![
                fixture.old_meta.clone(),
                fixture.new_meta.clone(),
                fixture.old_meta.clone(),
                fixture.new_meta.clone(),
                fixture.old_meta.clone(),
                fixture.new_meta.clone(),
            ]),
            meta_calls: meta_calls.clone(),
            binding_attempt_pages: Arc::new(vec![vec![Vec::new()]]),
            binding_page_calls: binding_page_calls.clone(),
        })
        .await;

        let result = read_edge_snapshot(
            &query_client(server),
            query_identity(&fixture),
            &ContextQuery::ContainsAll(Vec::new()),
        )
        .await;
        let Err(error) = result else {
            panic!("three changing observations must fail closed");
        };

        assert!(
            matches!(error, CliError::Conflict(message) if message == "conflict:project_context:snapshot_changed")
        );
        assert_eq!(meta_calls.load(Ordering::SeqCst), 6);
        assert_eq!(
            *binding_page_calls.lock().expect("binding page call lock"),
            vec![1, 1, 1]
        );
    }

    #[tokio::test]
    async fn loader_never_accepts_nonprogress_as_a_complete_stable_snapshot() {
        let fixture = query_fixture();
        let meta_calls = Arc::new(AtomicUsize::new(0));
        let binding_page_calls = Arc::new(Mutex::new(Vec::new()));
        let server = query_test_server(QueryServerState {
            meta_events: Arc::new(vec![fixture.new_meta.clone(), fixture.new_meta.clone()]),
            meta_calls: meta_calls.clone(),
            binding_attempt_pages: Arc::new(vec![vec![
                vec![fixture.binding_one.clone()],
                vec![fixture.binding_one.clone()],
            ]]),
            binding_page_calls: binding_page_calls.clone(),
        })
        .await;

        let result = read_edge_snapshot(
            &query_client(server),
            query_identity(&fixture),
            &ContextQuery::Incident(fixture.coordinates[0].clone()),
        )
        .await;
        let Err(CliError::Other(message)) = result else {
            panic!("stable nonprogress must be an integrity failure");
        };

        assert!(message.contains("pagination made no progress"));
        assert_eq!(meta_calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            *binding_page_calls.lock().expect("binding page call lock"),
            vec![1, 2]
        );
    }
}
