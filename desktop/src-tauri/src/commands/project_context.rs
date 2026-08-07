//! Verified, body-free Project Context Edge queries for the desktop client.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::Duration;

use buzz_core_pkg::kind::{
    KIND_PROJECT_CONTEXT_EDGE_BINDING, KIND_PROJECT_CONTEXT_META, KIND_PROJECT_DOCUMENT_HEAD,
    KIND_PROJECT_DOCUMENT_META,
};
use buzz_core_pkg::{CommunityId, PublicKey};
use buzz_project_context_pkg::{
    canonicalize_coordinates, EdgeKey, ProjectContextCoordinate, ProjectContextEdge,
    ProjectContextMetaProjection,
};
use buzz_project_document_pkg::DocumentHeadProjection;
use buzz_sdk_pkg::project_context::{
    aggregate_project_context_edges, parse_project_context_binding, parse_project_context_meta,
    project_context_edge_coordinate, verify_project_context_binding_observation,
    VerifiedProjectContextMeta,
};
use buzz_sdk_pkg::project_document::{
    document_head_coordinate, parse_document_head, parse_document_meta,
    verify_document_head_observation, VerifiedDocumentHead, VerifiedDocumentMeta,
};
use chrono::{DateTime, Utc};
use nostr::{Event, Keys};
use serde_json::{json, Value};
use tauri::State;
use uuid::Uuid;

use super::project_view::{read_identity_at, ProjectViewIdentity, ProjectViewSchema};
use crate::app_state::AppState;
use crate::relay::{query_relay_at_with_keys_typed, relay_api_base_url_with_override};

mod model;
pub use model::*;
mod project_view_hydration;
use project_view_hydration::hydrate_project_view;

const QUERY_PAGE_SIZE: u16 = 500;
const QUERY_SNAPSHOT_ATTEMPTS: usize = 3;
const HYDRATION_SNAPSHOT_ATTEMPTS: usize = 3;
const DOCUMENT_HEAD_CHUNK_SIZE: usize = 200;

#[derive(Debug, Clone)]
enum CanonicalContextQuery {
    Exact(Vec<ProjectContextCoordinate>),
    Incident(ProjectContextCoordinate),
    ContainsAll(Vec<ProjectContextCoordinate>),
}

impl CanonicalContextQuery {
    fn coordinates(&self) -> &[ProjectContextCoordinate] {
        match self {
            Self::Exact(coordinates) | Self::ContainsAll(coordinates) => coordinates,
            Self::Incident(coordinate) => std::slice::from_ref(coordinate),
        }
    }

    const fn complete_catalog(&self) -> bool {
        matches!(self, Self::ContainsAll(coordinates) if coordinates.is_empty())
    }

    fn to_dto(&self) -> ProjectContextQueryDto {
        match self {
            Self::Exact(coordinates) => ProjectContextQueryDto::Exact {
                coordinates: coordinates.iter().map(coordinate_dto).collect(),
            },
            Self::Incident(coordinate) => ProjectContextQueryDto::Incident {
                coordinate: coordinate_dto(coordinate),
            },
            Self::ContainsAll(coordinates) => ProjectContextQueryDto::ContainsAll {
                coordinates: coordinates.iter().map(coordinate_dto).collect(),
            },
        }
    }
}

#[derive(Debug, Clone)]
struct ProjectContextReadContext {
    community_key: String,
    api_base_url: String,
    keys: Keys,
    identity: ProjectViewIdentity,
}

#[derive(Debug)]
struct EdgeSnapshot {
    meta: VerifiedProjectContextMeta,
    edges: Vec<ProjectContextEdge>,
}

struct BindingPages {
    events: Vec<Event>,
    reached_empty_page: bool,
}

struct DocumentHydration {
    observation: ProjectContextDocumentObservation,
    documents: BTreeMap<Uuid, DocumentMetadata>,
}

#[derive(Debug, Clone)]
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

/// Query exact, incident, or contains-all Project Context Edges for the active Community.
#[tauri::command]
pub async fn query_project_context(
    input: QueryProjectContextInput,
    state: State<'_, AppState>,
) -> Result<ProjectContextQueryResult, ProjectContextCommandError> {
    let query = canonicalize_query(input.query)?;
    let context = capture_context(input.community_key, &state).await?;
    let snapshot = read_edge_snapshot(&state, &context, &query).await?;
    build_result(&state, &context, &query, snapshot).await
}

fn canonicalize_query(
    query: ProjectContextQueryDto,
) -> Result<CanonicalContextQuery, ProjectContextCommandError> {
    match query {
        ProjectContextQueryDto::Exact { coordinates } => {
            let coordinates = canonicalize_coordinates(
                coordinates
                    .into_iter()
                    .map(domain_coordinate)
                    .collect::<Vec<_>>(),
            )
            .map_err(|error| ProjectContextCommandError::invalid_input(error.to_string()))?;
            Ok(CanonicalContextQuery::Exact(coordinates))
        }
        ProjectContextQueryDto::Incident { coordinate } => {
            let coordinate = domain_coordinate(coordinate);
            coordinate
                .validate()
                .map_err(|error| ProjectContextCommandError::invalid_input(error.to_string()))?;
            Ok(CanonicalContextQuery::Incident(coordinate))
        }
        ProjectContextQueryDto::ContainsAll { coordinates } => {
            let mut coordinates = coordinates
                .into_iter()
                .map(domain_coordinate)
                .collect::<Vec<_>>();
            for coordinate in &coordinates {
                coordinate.validate().map_err(|error| {
                    ProjectContextCommandError::invalid_input(error.to_string())
                })?;
            }
            coordinates.sort();
            if coordinates.windows(2).any(|pair| pair[0] == pair[1]) {
                return Err(ProjectContextCommandError::invalid_input(
                    "Project Context query coordinates must be distinct.",
                ));
            }
            Ok(CanonicalContextQuery::ContainsAll(coordinates))
        }
    }
}

fn domain_coordinate(coordinate: ProjectContextCoordinateDto) -> ProjectContextCoordinate {
    match coordinate {
        ProjectContextCoordinateDto::ProjectViewObject {
            object_type,
            object_id,
        } => ProjectContextCoordinate::ProjectViewObject {
            object_type,
            object_id,
        },
        ProjectContextCoordinateDto::Document { document_id } => {
            ProjectContextCoordinate::Document { document_id }
        }
    }
}

fn coordinate_dto(coordinate: &ProjectContextCoordinate) -> ProjectContextCoordinateDto {
    match coordinate {
        ProjectContextCoordinate::ProjectViewObject {
            object_type,
            object_id,
        } => ProjectContextCoordinateDto::ProjectViewObject {
            object_type: *object_type,
            object_id: *object_id,
        },
        ProjectContextCoordinate::Document { document_id } => {
            ProjectContextCoordinateDto::Document {
                document_id: *document_id,
            }
        }
    }
}

fn coordinate_key(coordinate: &ProjectContextCoordinate) -> String {
    match coordinate {
        ProjectContextCoordinate::ProjectViewObject {
            object_type,
            object_id,
        } => format!("{}:{object_id}", object_type.as_str()),
        ProjectContextCoordinate::Document { document_id } => {
            format!("document:{document_id}")
        }
    }
}

async fn capture_context(
    community_key: String,
    state: &AppState,
) -> Result<ProjectContextReadContext, ProjectContextCommandError> {
    if community_key.trim().is_empty() {
        return Err(ProjectContextCommandError::invalid_input(
            "The local Community key is empty.",
        ));
    }
    // Capture all mutable workspace inputs before identity discovery. Every
    // subsequent request remains pinned if the Human switches Community.
    let api_base_url = relay_api_base_url_with_override(state);
    let keys = state
        .signing_keys()
        .map_err(|_| ProjectContextCommandError::internal())?;
    let identity = read_identity_at(state, &api_base_url)
        .await
        .map_err(|message| ProjectContextCommandError::from_identity_error(&message))?
        .ok_or_else(|| {
            ProjectContextCommandError::unsupported("This Community does not support Project View.")
        })?;
    if identity.schema != ProjectViewSchema::V3 {
        return Err(ProjectContextCommandError::unsupported(
            "Project Context requires Project View v3.",
        ));
    }
    identity
        .require_runtime_ready("Project Context")
        .map_err(ProjectContextCommandError::unavailable)?;
    if !identity.project_document_supported {
        return Err(ProjectContextCommandError::unsupported(
            "Project Context requires Project Documents.",
        ));
    }
    Ok(ProjectContextReadContext {
        community_key,
        api_base_url,
        keys,
        identity,
    })
}

async fn query_relay(
    state: &AppState,
    context: &ProjectContextReadContext,
    filter: Value,
) -> Result<Vec<Event>, ProjectContextCommandError> {
    query_relay_at_with_keys_typed(state, &context.api_base_url, &[filter], &context.keys, None)
        .await
        .map_err(ProjectContextCommandError::from_http)
}

async fn read_edge_snapshot(
    state: &AppState,
    context: &ProjectContextReadContext,
    query: &CanonicalContextQuery,
) -> Result<EdgeSnapshot, ProjectContextCommandError> {
    for attempt in 0..QUERY_SNAPSHOT_ATTEMPTS {
        match read_edge_snapshot_once(state, context, query).await {
            Ok(snapshot) => return Ok(snapshot),
            Err(error)
                if error.code == "snapshot_conflict" && attempt + 1 < QUERY_SNAPSHOT_ATTEMPTS =>
            {
                tokio::time::sleep(Duration::from_millis(25_u64 << attempt)).await;
            }
            Err(error) => return Err(error),
        }
    }
    Err(ProjectContextCommandError::snapshot_conflict(
        "Project Context changed during every bounded snapshot attempt.",
    ))
}

async fn read_edge_snapshot_once(
    state: &AppState,
    context: &ProjectContextReadContext,
    query: &CanonicalContextQuery,
) -> Result<EdgeSnapshot, ProjectContextCommandError> {
    let before = read_context_meta(state, context).await?;
    validate_query_for_project(query, before.projection.project_id)?;
    let pages = read_binding_pages(state, context, &before, query).await?;
    let after = read_context_meta(state, context).await?;
    if !same_context_observation(&before, &after) {
        return Err(ProjectContextCommandError::snapshot_conflict(
            "The signed Project Context snapshot changed while it was being read.",
        ));
    }
    if !pages.reached_empty_page {
        return Err(ProjectContextCommandError::verification_failed(
            "binding pagination made no progress under stable metadata",
        ));
    }

    let project_id = CommunityId::from_uuid(before.projection.project_id);
    let mut bindings = Vec::with_capacity(pages.events.len());
    let mut seen_event_ids = HashSet::with_capacity(pages.events.len());
    for event in pages.events {
        if !seen_event_ids.insert(event.id) {
            continue;
        }
        let binding =
            parse_project_context_binding(&event, &context.identity.relay_pubkey, project_id)
                .map_err(|error| {
                    ProjectContextCommandError::verification_failed(error.to_string())
                })?;
        verify_project_context_binding_observation(&before, &binding)
            .map_err(|error| ProjectContextCommandError::verification_failed(error.to_string()))?;
        bindings.push(binding);
    }
    let mut edges =
        aggregate_project_context_edges(&before, &bindings, query.complete_catalog())
            .map_err(|error| ProjectContextCommandError::verification_failed(error.to_string()))?;
    apply_query_semantics(query, before.projection.project_id, &mut edges)?;
    Ok(EdgeSnapshot {
        meta: before,
        edges,
    })
}

async fn read_context_meta(
    state: &AppState,
    context: &ProjectContextReadContext,
) -> Result<VerifiedProjectContextMeta, ProjectContextCommandError> {
    let events = query_relay(
        state,
        context,
        json!({
            "kinds": [KIND_PROJECT_CONTEXT_META],
            "authors": [context.identity.relay_pubkey.to_hex()],
            "limit": 2,
        }),
    )
    .await?;
    let [event] = events.as_slice() else {
        return if events.is_empty() {
            Err(ProjectContextCommandError::unavailable(
                "No verified Project Context projection is currently available.",
            ))
        } else {
            Err(ProjectContextCommandError::verification_failed(
                "metadata query returned multiple current heads",
            ))
        };
    };
    let untrusted: ProjectContextMetaProjection =
        serde_json::from_str(&event.content).map_err(|_| {
            ProjectContextCommandError::verification_failed(
                "metadata content cannot identify its Project",
            )
        })?;
    parse_project_context_meta(
        event,
        &context.identity.relay_pubkey,
        CommunityId::from_uuid(untrusted.project_id),
    )
    .map_err(|error| ProjectContextCommandError::verification_failed(error.to_string()))
}

async fn read_binding_pages(
    state: &AppState,
    context: &ProjectContextReadContext,
    meta: &VerifiedProjectContextMeta,
    query: &CanonicalContextQuery,
) -> Result<BindingPages, ProjectContextCommandError> {
    let project_id = meta.projection.project_id;
    let mut filter = binding_filter(&context.identity.relay_pubkey, project_id, query)?;

    let mut events = Vec::new();
    let mut seen_event_ids = HashSet::new();
    let mut page = 1_u64;
    loop {
        filter["page"] = json!(page);
        let current = query_relay(state, context, filter.clone()).await?;
        if current.len() > usize::from(QUERY_PAGE_SIZE) {
            return Err(ProjectContextCommandError::verification_failed(
                "binding page exceeded its requested limit",
            ));
        }
        if current.is_empty() {
            return Ok(BindingPages {
                events,
                reached_empty_page: true,
            });
        }
        let previous_len = events.len();
        for event in current {
            if seen_event_ids.insert(event.id) {
                events.push(event);
            }
        }
        if events.len() == previous_len {
            return Ok(BindingPages {
                events,
                reached_empty_page: false,
            });
        }
        page = page.checked_add(1).ok_or_else(|| {
            ProjectContextCommandError::verification_failed("binding page number overflow")
        })?;
    }
}

fn binding_filter(
    relay_pubkey: &PublicKey,
    project_id: Uuid,
    query: &CanonicalContextQuery,
) -> Result<Value, ProjectContextCommandError> {
    let mut filter = json!({
        "kinds": [KIND_PROJECT_CONTEXT_EDGE_BINDING],
        "authors": [relay_pubkey.to_hex()],
        "#s": ["active"],
        "limit": QUERY_PAGE_SIZE,
    });
    match query {
        CanonicalContextQuery::Exact(coordinates) => {
            let edge_key = EdgeKey::derive(project_id, coordinates)
                .map_err(|error| ProjectContextCommandError::invalid_input(error.to_string()))?;
            filter["#g"] = json!([project_context_edge_coordinate(
                CommunityId::from_uuid(project_id),
                edge_key,
            )]);
        }
        CanonicalContextQuery::Incident(coordinate) => {
            filter["#c"] = json!([coordinate.tag_value(project_id)]);
        }
        CanonicalContextQuery::ContainsAll(coordinates) if !coordinates.is_empty() => {
            filter["#c"] = json!([coordinates[0].tag_value(project_id)]);
        }
        CanonicalContextQuery::ContainsAll(_) => {}
    }
    Ok(filter)
}

fn validate_query_for_project(
    query: &CanonicalContextQuery,
    project_id: Uuid,
) -> Result<(), ProjectContextCommandError> {
    for coordinate in query.coordinates() {
        coordinate
            .validate_for_project(project_id)
            .map_err(|error| ProjectContextCommandError::invalid_input(error.to_string()))?;
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

fn apply_query_semantics(
    query: &CanonicalContextQuery,
    project_id: Uuid,
    edges: &mut Vec<ProjectContextEdge>,
) -> Result<(), ProjectContextCommandError> {
    match query {
        CanonicalContextQuery::Exact(coordinates) => {
            let expected_key = EdgeKey::derive(project_id, coordinates)
                .map_err(|error| ProjectContextCommandError::invalid_input(error.to_string()))?;
            if edges.iter().any(|edge| {
                edge.key() != expected_key || edge.coordinates() != coordinates.as_slice()
            }) || edges.len() > 1
            {
                return Err(ProjectContextCommandError::verification_failed(
                    "exact query returned a collision, superset, or multiple Edges",
                ));
            }
        }
        CanonicalContextQuery::Incident(coordinate) => {
            if edges
                .iter()
                .any(|edge| !edge.coordinates().contains(coordinate))
            {
                return Err(ProjectContextCommandError::verification_failed(
                    "incident query returned an Edge that omits its coordinate",
                ));
            }
        }
        CanonicalContextQuery::ContainsAll(required) => {
            edges.retain(|edge| {
                required
                    .iter()
                    .all(|coordinate| edge.coordinates().binary_search(coordinate).is_ok())
            });
        }
    }
    Ok(())
}

async fn build_result(
    state: &AppState,
    context: &ProjectContextReadContext,
    query: &CanonicalContextQuery,
    snapshot: EdgeSnapshot,
) -> Result<ProjectContextQueryResult, ProjectContextCommandError> {
    let project_id = snapshot.meta.projection.project_id;
    let mut requested_coordinates = query.coordinates().iter().cloned().collect::<BTreeSet<_>>();
    let mut edge_coordinates = BTreeSet::new();
    let mut document_ids = BTreeSet::new();
    let mut context_document_ids = BTreeSet::new();
    for edge in &snapshot.edges {
        for coordinate in edge.coordinates() {
            requested_coordinates.insert(coordinate.clone());
            edge_coordinates.insert(coordinate.clone());
            if let ProjectContextCoordinate::Document { document_id } = coordinate {
                document_ids.insert(*document_id);
            }
        }
        context_document_ids.extend(edge.context_document_ids().iter().copied());
    }
    for coordinate in &requested_coordinates {
        if let ProjectContextCoordinate::Document { document_id } = coordinate {
            document_ids.insert(*document_id);
        }
    }
    document_ids.extend(context_document_ids.iter().copied());
    let project_view_coordinates = requested_coordinates
        .iter()
        .filter(|coordinate| {
            matches!(
                coordinate,
                ProjectContextCoordinate::ProjectViewObject { .. }
            )
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    let required_project_view_coordinates = edge_coordinates
        .iter()
        .filter(|coordinate| {
            matches!(
                coordinate,
                ProjectContextCoordinate::ProjectViewObject { .. }
            )
        })
        .cloned()
        .collect::<BTreeSet<_>>();

    let project_view = hydrate_project_view(
        state,
        context,
        project_id,
        &project_view_coordinates,
        &required_project_view_coordinates,
    )
    .await?;
    let documents = hydrate_documents(state, context, project_id, &document_ids).await?;
    validate_context_documents_active(&context_document_ids, &documents.documents)?;

    let mut coordinate_details = Vec::with_capacity(requested_coordinates.len());
    for coordinate in requested_coordinates {
        let detail = match &coordinate {
            ProjectContextCoordinate::ProjectViewObject { .. } => project_view
                .coordinates
                .get(&coordinate)
                .cloned()
                .unwrap_or_else(|| unavailable_coordinate(&coordinate)),
            ProjectContextCoordinate::Document { document_id } => {
                document_coordinate_detail(&coordinate, documents.documents.get(document_id))
            }
        };
        coordinate_details.push(detail);
    }
    let document_details = document_ids
        .iter()
        .map(|document_id| document_detail(*document_id, documents.documents.get(document_id)))
        .collect();
    let edges = snapshot
        .edges
        .iter()
        .map(|edge| ProjectContextEdgeDto {
            edge_key: edge.key().to_hex(),
            coordinate_keys: edge.coordinates().iter().map(coordinate_key).collect(),
            context_document_ids: edge.context_document_ids().to_vec(),
        })
        .collect();

    Ok(ProjectContextQueryResult {
        community_key: context.community_key.clone(),
        project_id,
        relay_pubkey: context.identity.relay_pubkey.to_hex(),
        context: ProjectContextObservation {
            context_revision: snapshot.meta.projection.context_revision,
            projection_generation: snapshot.meta.projection.projection_generation,
            active_edge_count: snapshot.meta.projection.active_edge_count,
            bound_document_count: snapshot.meta.projection.bound_document_count,
            updated_at: snapshot.meta.projection.updated_at,
            meta_event_id: snapshot.meta.event_id.to_hex(),
            capability_enabled: context.identity.project_context_edge_supported,
        },
        query: query.to_dto(),
        project_view_observation: project_view.observation,
        document_observation: documents.observation,
        edges,
        coordinate_details,
        document_details,
    })
}

fn validate_context_documents_active(
    context_document_ids: &BTreeSet<Uuid>,
    documents: &BTreeMap<Uuid, DocumentMetadata>,
) -> Result<(), ProjectContextCommandError> {
    if context_document_ids.iter().any(|document_id| {
        matches!(
            documents.get(document_id),
            Some(DocumentMetadata::Tombstoned { .. })
        )
    }) {
        return Err(ProjectContextCommandError::verification_failed(
            "an active Context binding points to a verified tombstoned Document",
        ));
    }
    Ok(())
}

async fn hydrate_documents(
    state: &AppState,
    context: &ProjectContextReadContext,
    project_id: Uuid,
    requested: &BTreeSet<Uuid>,
) -> Result<DocumentHydration, ProjectContextCommandError> {
    if requested.is_empty() {
        return Ok(DocumentHydration {
            observation: empty_document_observation(ProjectContextSourceState::NotRequested),
            documents: BTreeMap::new(),
        });
    }
    match read_document_heads_snapshot(state, context, project_id, requested).await {
        Ok((meta, documents)) => Ok(DocumentHydration {
            observation: ProjectContextDocumentObservation {
                state: ProjectContextSourceState::Observed,
                catalog_revision: Some(meta.projection.catalog_revision),
                projection_generation: Some(meta.projection.projection_generation),
                updated_at: Some(meta.projection.updated_at),
                meta_event_id: Some(meta.event_id.to_hex()),
            },
            documents,
        }),
        Err(error) if error.hydration_can_degrade() => Ok(DocumentHydration {
            observation: empty_document_observation(ProjectContextSourceState::Unavailable),
            documents: BTreeMap::new(),
        }),
        Err(error) => Err(error),
    }
}

fn empty_document_observation(
    state: ProjectContextSourceState,
) -> ProjectContextDocumentObservation {
    ProjectContextDocumentObservation {
        state,
        catalog_revision: None,
        projection_generation: None,
        updated_at: None,
        meta_event_id: None,
    }
}

async fn read_document_heads_snapshot(
    state: &AppState,
    context: &ProjectContextReadContext,
    project_id: Uuid,
    requested: &BTreeSet<Uuid>,
) -> Result<(VerifiedDocumentMeta, BTreeMap<Uuid, DocumentMetadata>), ProjectContextCommandError> {
    for attempt in 0..HYDRATION_SNAPSHOT_ATTEMPTS {
        match read_document_heads_snapshot_once(state, context, project_id, requested).await {
            Ok(snapshot) => return Ok(snapshot),
            Err(error)
                if error.code == "snapshot_conflict"
                    && attempt + 1 < HYDRATION_SNAPSHOT_ATTEMPTS =>
            {
                tokio::time::sleep(Duration::from_millis(25_u64 << attempt)).await;
            }
            Err(error) => return Err(error),
        }
    }
    Err(ProjectContextCommandError::snapshot_conflict(
        "Project Documents changed during every bounded hydration attempt.",
    ))
}

async fn read_document_heads_snapshot_once(
    state: &AppState,
    context: &ProjectContextReadContext,
    project_id: Uuid,
    requested: &BTreeSet<Uuid>,
) -> Result<(VerifiedDocumentMeta, BTreeMap<Uuid, DocumentMetadata>), ProjectContextCommandError> {
    let before = read_document_meta(state, context, project_id).await?;
    let requested_ids = requested.iter().copied().collect::<Vec<_>>();
    let mut heads = BTreeMap::new();
    for chunk in requested_ids.chunks(DOCUMENT_HEAD_CHUNK_SIZE) {
        let coordinates = chunk
            .iter()
            .map(|document_id| {
                document_head_coordinate(CommunityId::from_uuid(project_id), *document_id)
            })
            .collect::<Vec<_>>();
        let events = query_relay(
            state,
            context,
            json!({
                "kinds": [KIND_PROJECT_DOCUMENT_HEAD],
                "authors": [context.identity.relay_pubkey.to_hex()],
                "#d": coordinates,
                "limit": chunk.len() + 1,
            }),
        )
        .await?;
        if events.len() > chunk.len() {
            return Err(ProjectContextCommandError::verification_failed(
                "Document hydration returned multiple current heads for one coordinate",
            ));
        }
        for event in events {
            let head = parse_document_head(
                &event,
                &context.identity.relay_pubkey,
                CommunityId::from_uuid(project_id),
            )
            .map_err(|error| ProjectContextCommandError::verification_failed(error.to_string()))?;
            let document_id = document_head_id(&head);
            if !requested.contains(&document_id) || heads.insert(document_id, head).is_some() {
                return Err(ProjectContextCommandError::verification_failed(
                    "Document hydration returned a duplicate or unrequested head",
                ));
            }
        }
    }
    let after = read_document_meta(state, context, project_id).await?;
    if before.event_id != after.event_id
        || before.projection.catalog_revision != after.projection.catalog_revision
        || before.projection.projection_generation != after.projection.projection_generation
    {
        return Err(ProjectContextCommandError::snapshot_conflict(
            "The signed Document catalog changed during hydration.",
        ));
    }
    let documents = heads
        .into_iter()
        .map(|(document_id, head)| {
            verify_document_head_observation(&before, &head).map_err(|error| {
                ProjectContextCommandError::verification_failed(error.to_string())
            })?;
            Ok((document_id, document_metadata(&head)))
        })
        .collect::<Result<BTreeMap<_, _>, ProjectContextCommandError>>()?;
    Ok((before, documents))
}

async fn read_document_meta(
    state: &AppState,
    context: &ProjectContextReadContext,
    project_id: Uuid,
) -> Result<VerifiedDocumentMeta, ProjectContextCommandError> {
    let events = query_relay(
        state,
        context,
        json!({
            "kinds": [KIND_PROJECT_DOCUMENT_META],
            "authors": [context.identity.relay_pubkey.to_hex()],
            "limit": 2,
        }),
    )
    .await?;
    let [event] = events.as_slice() else {
        return if events.is_empty() {
            Err(ProjectContextCommandError::unavailable(
                "Project Document metadata is temporarily unavailable.",
            ))
        } else {
            Err(ProjectContextCommandError::verification_failed(
                "Document metadata query returned multiple current heads",
            ))
        };
    };
    let meta = parse_document_meta(event, &context.identity.relay_pubkey)
        .map_err(|error| ProjectContextCommandError::verification_failed(error.to_string()))?;
    if meta.projection.project_id != project_id {
        return Err(ProjectContextCommandError::verification_failed(
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

fn document_coordinate_detail(
    coordinate: &ProjectContextCoordinate,
    metadata: Option<&DocumentMetadata>,
) -> ProjectContextCoordinateDetail {
    match metadata {
        Some(DocumentMetadata::Active {
            title,
            document_revision,
            updated_at,
            updated_by,
            ..
        }) => ProjectContextCoordinateDetail {
            coordinate_key: coordinate_key(coordinate),
            coordinate: coordinate_dto(coordinate),
            state: ProjectContextDetailState::Active,
            title: Some(title.clone()),
            status: None,
            object_revision: None,
            document_revision: Some(*document_revision),
            updated_at: Some(*updated_at),
            updated_by: Some(updated_by.to_hex()),
            unavailable_reason: None,
        },
        Some(DocumentMetadata::Tombstoned {
            document_revision,
            deleted_at,
            deleted_by,
        }) => ProjectContextCoordinateDetail {
            coordinate_key: coordinate_key(coordinate),
            coordinate: coordinate_dto(coordinate),
            state: ProjectContextDetailState::Tombstoned,
            title: None,
            status: None,
            object_revision: None,
            document_revision: Some(*document_revision),
            updated_at: Some(*deleted_at),
            updated_by: Some(deleted_by.to_hex()),
            unavailable_reason: None,
        },
        None => unavailable_coordinate(coordinate),
    }
}

fn unavailable_coordinate(coordinate: &ProjectContextCoordinate) -> ProjectContextCoordinateDetail {
    ProjectContextCoordinateDetail {
        coordinate_key: coordinate_key(coordinate),
        coordinate: coordinate_dto(coordinate),
        state: ProjectContextDetailState::Unavailable,
        title: None,
        status: None,
        object_revision: None,
        document_revision: None,
        updated_at: None,
        updated_by: None,
        unavailable_reason: Some("metadata_unavailable"),
    }
}

fn document_detail(
    document_id: Uuid,
    metadata: Option<&DocumentMetadata>,
) -> ProjectContextDocumentDetail {
    match metadata {
        Some(DocumentMetadata::Active {
            title,
            summary,
            document_revision,
            updated_at,
            updated_by,
        }) => ProjectContextDocumentDetail {
            document_id,
            state: ProjectContextDetailState::Active,
            title: Some(title.clone()),
            summary: summary.clone(),
            document_revision: Some(*document_revision),
            updated_at: Some(*updated_at),
            updated_by: Some(updated_by.to_hex()),
            unavailable_reason: None,
        },
        Some(DocumentMetadata::Tombstoned {
            document_revision,
            deleted_at,
            deleted_by,
        }) => ProjectContextDocumentDetail {
            document_id,
            state: ProjectContextDetailState::Tombstoned,
            title: None,
            summary: None,
            document_revision: Some(*document_revision),
            updated_at: Some(*deleted_at),
            updated_by: Some(deleted_by.to_hex()),
            unavailable_reason: None,
        },
        None => ProjectContextDocumentDetail {
            document_id,
            state: ProjectContextDetailState::Unavailable,
            title: None,
            summary: None,
            document_revision: None,
            updated_at: None,
            updated_by: None,
            unavailable_reason: Some("metadata_unavailable"),
        },
    }
}

#[cfg(test)]
#[path = "project_context_tests.rs"]
mod tests;
