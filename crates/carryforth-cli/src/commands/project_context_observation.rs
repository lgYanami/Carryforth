//! Atomic structural observations for progressive Project Context traversal.

use std::collections::BTreeSet;
use std::io::{self, Write};

use buzz_core::{EventId, PublicKey};
use buzz_project_context::{EdgeKey, ProjectContextCoordinate, MAX_SAFE_REVISION};
use buzz_project_view::v3::{ProjectViewEntryV3, ProjectViewObjectDataV3};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use super::{
    hydrate_documents, hydrate_meetings, hydration_is_unavailable, parse_coordinate_token,
    project_view_title_status, read_edge_snapshot, read_verified_v3_snapshot, require_identity,
    ContextQuery, DocumentMetadata, EdgeSnapshot, MeetingFetchCommands, ProjectViewIdentity,
};
use crate::client::CarryforthClient;
use crate::error::CliError;
use crate::OutputFormat;

const DEFAULT_PAGE_LIMIT: u8 = 32;
const MAX_STRUCTURAL_OUTPUT_BYTES: usize = 512 * 1024;

#[derive(Clone, PartialEq, Eq, Serialize)]
struct ContextSnapshotIdentity {
    context_meta_event_id: String,
    context_revision: u64,
    projection_generation: u64,
}

#[derive(Serialize)]
struct CoordinateShowOutput {
    project_id: Uuid,
    snapshot: ContextSnapshotIdentity,
    coordinate: CoordinateObservation,
}

#[derive(Serialize)]
struct CoordinateEdgesOutput {
    project_id: Uuid,
    snapshot: ContextSnapshotIdentity,
    coordinate: ProjectContextCoordinate,
    edges: Vec<EdgeIdentityOutput>,
    page: EdgePageOutput,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
struct EdgeIdentityOutput {
    edge_key: EdgeKey,
    binding_document_count: usize,
}

#[derive(PartialEq, Eq, Serialize)]
struct EdgePageOutput {
    limit: u8,
    next_after_edge_key: Option<EdgeKey>,
    truncated: bool,
}

#[derive(Serialize)]
struct EdgeDocumentsOutput {
    project_id: Uuid,
    snapshot: ContextSnapshotIdentity,
    edge_key: EdgeKey,
    documents: Vec<ContextDocumentObservation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    page: Option<DocumentPageOutput>,
}

#[derive(PartialEq, Eq, Serialize)]
struct DocumentPageOutput {
    limit: u8,
    next_after_document_id: Option<Uuid>,
    truncated: bool,
}

#[derive(Serialize)]
struct EdgeCoordinatesOutput {
    project_id: Uuid,
    snapshot: ContextSnapshotIdentity,
    edge_key: EdgeKey,
    coordinates: Vec<CoordinateObservation>,
}

#[derive(Clone, Serialize)]
struct CoordinateObservation {
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
    read_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fetch_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    meeting_fetch: Option<MeetingFetchCommands>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unavailable_reason: Option<&'static str>,
}

#[derive(Serialize)]
struct ContextDocumentObservation {
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
    #[serde(skip_serializing_if = "Option::is_none")]
    fetch_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unavailable_reason: Option<&'static str>,
}

pub(super) async fn run_coordinate_show(
    client: &CarryforthClient,
    coordinate: &str,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let coordinate = parse_coordinate_token(coordinate)?;
    let identity = require_identity(client).await?;
    let snapshot = read_edge_snapshot(
        client,
        identity,
        &ContextQuery::Incident(coordinate.clone()),
    )
    .await?;
    require_nonempty_scope(
        &snapshot,
        "Coordinate is not a member of any current active Edge",
    )?;
    let project_id = snapshot.meta.projection.project_id;
    let mut coordinates = hydrate_coordinates(client, identity, project_id, &[coordinate]).await?;
    let coordinate = coordinates
        .pop()
        .ok_or_else(|| integrity_error("Coordinate hydration returned no observation"))?;
    write_bounded_json(
        &CoordinateShowOutput {
            project_id,
            snapshot: snapshot_identity(&snapshot),
            coordinate,
        },
        format,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_coordinate_edges(
    client: &CarryforthClient,
    coordinate: &str,
    limit: Option<u8>,
    after_edge: Option<&str>,
    expected_context_meta_event_id: Option<&str>,
    expected_context_revision: Option<u64>,
    expected_projection_generation: Option<u64>,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let coordinate = parse_coordinate_token(coordinate)?;
    validate_page_limit(limit)?;
    validate_continuation_input(
        after_edge.is_some(),
        expected_context_meta_event_id,
        expected_context_revision,
        expected_projection_generation,
    )?;
    if let Some(cursor) = after_edge {
        parse_edge_key(cursor)?;
    }
    let identity = require_identity(client).await?;
    let snapshot = read_edge_snapshot(
        client,
        identity,
        &ContextQuery::Incident(coordinate.clone()),
    )
    .await?;
    require_nonempty_scope(
        &snapshot,
        "Coordinate is not a member of any current active Edge",
    )?;
    let snapshot_identity = snapshot_identity(&snapshot);
    let mut edges = snapshot
        .edges
        .iter()
        .map(|edge| EdgeIdentityOutput {
            edge_key: edge.key(),
            binding_document_count: edge.context_document_ids().len(),
        })
        .collect::<Vec<_>>();
    edges.sort_by_key(|edge| edge.edge_key);
    let limit = limit.unwrap_or(DEFAULT_PAGE_LIMIT);
    let start = validate_edge_continuation(
        &snapshot_identity,
        &edges,
        after_edge,
        expected_context_meta_event_id,
        expected_context_revision,
        expected_projection_generation,
    )?;
    let (page_edges, page) = edge_page(&edges, start, limit);
    write_bounded_json(
        &CoordinateEdgesOutput {
            project_id: snapshot.meta.projection.project_id,
            snapshot: snapshot_identity,
            coordinate,
            edges: page_edges,
            page,
        },
        format,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_edge_documents(
    client: &CarryforthClient,
    edge_key: &str,
    exact_document: Option<Uuid>,
    limit: Option<u8>,
    after_document: Option<Uuid>,
    expected_context_meta_event_id: Option<&str>,
    expected_context_revision: Option<u64>,
    expected_projection_generation: Option<u64>,
    format: &OutputFormat,
) -> Result<(), CliError> {
    validate_page_limit(limit)?;
    if exact_document.is_some()
        && (limit.is_some()
            || after_document.is_some()
            || expected_context_meta_event_id.is_some()
            || expected_context_revision.is_some()
            || expected_projection_generation.is_some())
    {
        return Err(CliError::Usage(
            "--document cannot be combined with pagination options".to_owned(),
        ));
    }
    validate_continuation_input(
        after_document.is_some(),
        expected_context_meta_event_id,
        expected_context_revision,
        expected_projection_generation,
    )?;
    let edge_key = parse_edge_key(edge_key)?;
    let identity = require_identity(client).await?;
    let snapshot = read_edge_snapshot(client, identity, &ContextQuery::EdgeKey(edge_key)).await?;
    let edge = require_one_edge(&snapshot, edge_key)?;
    let snapshot_identity = snapshot_identity(&snapshot);
    let mut document_ids = edge.context_document_ids().to_vec();
    document_ids.sort();

    let (selected, page) = if let Some(document_id) = exact_document {
        if document_ids.binary_search(&document_id).is_err() {
            return Err(CliError::NotFound(
                "Project Context Document binding was not found".to_owned(),
            ));
        }
        (vec![document_id], None)
    } else {
        let limit = limit.unwrap_or(DEFAULT_PAGE_LIMIT);
        let start = validate_document_continuation(
            &snapshot_identity,
            &document_ids,
            after_document,
            expected_context_meta_event_id,
            expected_context_revision,
            expected_projection_generation,
        )?;
        let (selected, page) = document_page(&document_ids, start, limit);
        (selected, Some(page))
    };

    let requested = selected.iter().copied().collect::<BTreeSet<_>>();
    let hydration = hydrate_documents(
        client,
        identity,
        snapshot.meta.projection.project_id,
        &requested,
    )
    .await?;
    let mut documents = Vec::with_capacity(selected.len());
    for document_id in selected {
        documents.push(context_document_observation(
            document_id,
            hydration.heads.get(&document_id),
        )?);
    }
    write_bounded_json(
        &EdgeDocumentsOutput {
            project_id: snapshot.meta.projection.project_id,
            snapshot: snapshot_identity,
            edge_key,
            documents,
            page,
        },
        format,
    )
}

pub(super) async fn run_edge_coordinates(
    client: &CarryforthClient,
    edge_key: &str,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let edge_key = parse_edge_key(edge_key)?;
    let identity = require_identity(client).await?;
    let snapshot = read_edge_snapshot(client, identity, &ContextQuery::EdgeKey(edge_key)).await?;
    let edge = require_one_edge(&snapshot, edge_key)?;
    let coordinates = hydrate_coordinates(
        client,
        identity,
        snapshot.meta.projection.project_id,
        edge.coordinates(),
    )
    .await?;
    write_bounded_json(
        &EdgeCoordinatesOutput {
            project_id: snapshot.meta.projection.project_id,
            snapshot: snapshot_identity(&snapshot),
            edge_key,
            coordinates,
        },
        format,
    )
}

async fn hydrate_coordinates(
    client: &CarryforthClient,
    identity: ProjectViewIdentity,
    project_id: Uuid,
    coordinates: &[ProjectContextCoordinate],
) -> Result<Vec<CoordinateObservation>, CliError> {
    let mut project_view_coordinates = BTreeSet::new();
    let mut document_ids = BTreeSet::new();
    let mut meeting_ids = BTreeSet::new();
    for coordinate in coordinates {
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
    let project_view =
        hydrate_project_view_observations(client, identity, project_id, &project_view_coordinates)
            .await?;
    let documents = hydrate_documents(client, identity, project_id, &document_ids).await?;
    let meetings = hydrate_meetings(client, &meeting_ids).await?;
    let mut output = Vec::with_capacity(coordinates.len());
    for coordinate in coordinates {
        output.push(match coordinate {
            ProjectContextCoordinate::ProjectViewObject { .. } => project_view
                .get(coordinate)
                .cloned()
                .ok_or_else(|| integrity_error("Project View hydration omitted a Coordinate"))?,
            ProjectContextCoordinate::Document { document_id } => document_coordinate_observation(
                coordinate.clone(),
                documents.heads.get(document_id),
            ),
            ProjectContextCoordinate::Meeting { meeting_id } => meeting_coordinate_observation(
                coordinate.clone(),
                meetings.summaries.get(meeting_id),
            ),
        });
    }
    Ok(output)
}

async fn hydrate_project_view_observations(
    client: &CarryforthClient,
    identity: ProjectViewIdentity,
    project_id: Uuid,
    requested: &BTreeSet<ProjectContextCoordinate>,
) -> Result<std::collections::BTreeMap<ProjectContextCoordinate, CoordinateObservation>, CliError> {
    if requested.is_empty() {
        return Ok(std::collections::BTreeMap::new());
    }
    match read_verified_v3_snapshot(client, identity).await {
        Ok(snapshot) => {
            if *snapshot.meta().project_id.as_uuid() != project_id {
                return Err(integrity_error(
                    "Project View and Context metadata identify different Projects",
                ));
            }
            let mut observations = std::collections::BTreeMap::new();
            for coordinate in requested {
                let ProjectContextCoordinate::ProjectViewObject {
                    object_type,
                    object_id,
                } = coordinate
                else {
                    return Err(integrity_error(
                        "Project View hydration received a non-Project-View Coordinate",
                    ));
                };
                let entry = snapshot.entry(*object_id).ok_or_else(|| {
                    integrity_error("verified Project View snapshot omitted a Context Coordinate")
                })?;
                if entry.object_type() != *object_type {
                    return Err(integrity_error(
                        "verified Project View Coordinate has a different object type",
                    ));
                }
                observations.insert(
                    coordinate.clone(),
                    project_view_coordinate_observation(coordinate, entry),
                );
            }
            Ok(observations)
        }
        Err(error) if hydration_is_unavailable(&error) => Ok(requested
            .iter()
            .cloned()
            .map(|coordinate| {
                let unavailable = unavailable_coordinate_observation(coordinate.clone());
                (coordinate, unavailable)
            })
            .collect()),
        Err(error) => Err(error),
    }
}

fn project_view_coordinate_observation(
    coordinate: &ProjectContextCoordinate,
    entry: &ProjectViewEntryV3,
) -> CoordinateObservation {
    match entry {
        ProjectViewEntryV3::Active(object) => {
            let (title, status) = project_view_title_status(&object.data);
            let ProjectContextCoordinate::ProjectViewObject {
                object_type,
                object_id,
            } = coordinate
            else {
                return unavailable_coordinate_observation(coordinate.clone());
            };
            CoordinateObservation {
                coordinate: coordinate.clone(),
                state: "active",
                title: Some(title),
                description: project_view_description(&object.data).map(ToOwned::to_owned),
                summary: object.data.summary().map(ToOwned::to_owned),
                status,
                object_revision: Some(object.object_revision),
                document_revision: None,
                updated_at: Some(object.updated_at),
                updated_by: Some(object.updated_by),
                read_command: Some(format!(
                    "cf project-view get-object {} {object_id}",
                    object_type.as_str()
                )),
                fetch_command: None,
                meeting_fetch: None,
                unavailable_reason: None,
            }
        }
        ProjectViewEntryV3::Tombstone(tombstone) => CoordinateObservation {
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
            read_command: None,
            fetch_command: None,
            meeting_fetch: None,
            unavailable_reason: None,
        },
    }
}

fn project_view_description(data: &ProjectViewObjectDataV3) -> Option<&str> {
    match data {
        ProjectViewObjectDataV3::Plan(value) => Some(&value.description),
        ProjectViewObjectDataV3::Stage(value) => Some(&value.description),
        ProjectViewObjectDataV3::Requirement(value) => Some(&value.description),
        ProjectViewObjectDataV3::Issue(value) => Some(&value.description),
        ProjectViewObjectDataV3::Work(value) => Some(&value.description),
        ProjectViewObjectDataV3::ProjectProfile(_)
        | ProjectViewObjectDataV3::Goal(_)
        | ProjectViewObjectDataV3::Role(_)
        | ProjectViewObjectDataV3::Resource(_) => None,
    }
}

fn document_coordinate_observation(
    coordinate: ProjectContextCoordinate,
    metadata: Option<&DocumentMetadata>,
) -> CoordinateObservation {
    match metadata {
        Some(DocumentMetadata::Active {
            title,
            summary,
            document_revision,
            updated_at,
            updated_by,
        }) => {
            let fetch_command = match &coordinate {
                ProjectContextCoordinate::Document { document_id } => Some(format!(
                    "cf documents get {document_id} --revision {document_revision} --content-only"
                )),
                _ => None,
            };
            CoordinateObservation {
                coordinate,
                state: "active",
                title: Some(title.clone()),
                description: None,
                summary: summary.clone(),
                status: None,
                object_revision: None,
                document_revision: Some(*document_revision),
                updated_at: Some(*updated_at),
                updated_by: Some(*updated_by),
                read_command: None,
                fetch_command,
                meeting_fetch: None,
                unavailable_reason: None,
            }
        }
        Some(DocumentMetadata::Tombstoned {
            document_revision,
            deleted_at,
            deleted_by,
        }) => CoordinateObservation {
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
            read_command: None,
            fetch_command: None,
            meeting_fetch: None,
            unavailable_reason: None,
        },
        None => unavailable_coordinate_observation(coordinate),
    }
}

fn meeting_coordinate_observation(
    coordinate: ProjectContextCoordinate,
    summary: Option<&super::MeetingSummary>,
) -> CoordinateObservation {
    let ProjectContextCoordinate::Meeting { meeting_id } = coordinate else {
        return unavailable_coordinate_observation(coordinate);
    };
    let Some(summary) = summary else {
        return unavailable_coordinate_observation(ProjectContextCoordinate::Meeting {
            meeting_id,
        });
    };
    CoordinateObservation {
        coordinate: ProjectContextCoordinate::Meeting { meeting_id },
        state: if summary.status == "ended" {
            "terminal"
        } else {
            "active"
        },
        title: Some(summary.title.clone()),
        description: summary.description.clone(),
        summary: summary.summary.clone(),
        status: Some(serde_json::json!(summary.status)),
        object_revision: None,
        document_revision: None,
        updated_at: i64::try_from(summary.updated_at)
            .ok()
            .and_then(|timestamp| DateTime::from_timestamp(timestamp, 0)),
        updated_by: None,
        read_command: None,
        fetch_command: None,
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

fn context_document_observation(
    document_id: Uuid,
    metadata: Option<&DocumentMetadata>,
) -> Result<ContextDocumentObservation, CliError> {
    match metadata {
        Some(DocumentMetadata::Active {
            title,
            summary,
            document_revision,
            updated_at,
            updated_by,
        }) => Ok(ContextDocumentObservation {
            document_id,
            state: "active",
            title: Some(title.clone()),
            summary: summary.clone(),
            document_revision: Some(*document_revision),
            updated_at: Some(*updated_at),
            updated_by: Some(*updated_by),
            fetch_command: Some(format!(
                "cf documents get {document_id} --revision {document_revision} --content-only"
            )),
            unavailable_reason: None,
        }),
        Some(DocumentMetadata::Tombstoned { .. }) => Err(integrity_error(
            "an active Context binding points to a verified tombstoned Document",
        )),
        None => Ok(ContextDocumentObservation {
            document_id,
            state: "unavailable",
            title: None,
            summary: None,
            document_revision: None,
            updated_at: None,
            updated_by: None,
            fetch_command: None,
            unavailable_reason: Some("metadata_unavailable"),
        }),
    }
}

fn unavailable_coordinate_observation(
    coordinate: ProjectContextCoordinate,
) -> CoordinateObservation {
    CoordinateObservation {
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
        read_command: None,
        fetch_command: None,
        meeting_fetch: None,
        unavailable_reason: Some("metadata_unavailable"),
    }
}

fn snapshot_identity(snapshot: &EdgeSnapshot) -> ContextSnapshotIdentity {
    ContextSnapshotIdentity {
        context_meta_event_id: snapshot.meta.event_id.to_hex(),
        context_revision: snapshot.meta.projection.context_revision,
        projection_generation: snapshot.meta.projection.projection_generation,
    }
}

fn require_nonempty_scope(snapshot: &EdgeSnapshot, message: &str) -> Result<(), CliError> {
    if snapshot.edges.is_empty() {
        return Err(CliError::NotFound(message.to_owned()));
    }
    Ok(())
}

fn require_one_edge(
    snapshot: &EdgeSnapshot,
    expected: EdgeKey,
) -> Result<&buzz_project_context::ProjectContextEdge, CliError> {
    let [edge] = snapshot.edges.as_slice() else {
        if snapshot.edges.is_empty() {
            return Err(CliError::NotFound(
                "Project Context Edge was not found".to_owned(),
            ));
        }
        return Err(integrity_error("Edge lookup returned multiple Edges"));
    };
    if edge.key() != expected {
        return Err(integrity_error("Edge lookup returned a different Edge"));
    }
    Ok(edge)
}

fn validate_page_limit(limit: Option<u8>) -> Result<(), CliError> {
    if limit.is_some_and(|value| !(1..=DEFAULT_PAGE_LIMIT).contains(&value)) {
        return Err(CliError::Usage(
            "Project Context page limit must be between 1 and 32".to_owned(),
        ));
    }
    Ok(())
}

fn validate_continuation_input(
    has_cursor: bool,
    expected_context_meta_event_id: Option<&str>,
    expected_context_revision: Option<u64>,
    expected_projection_generation: Option<u64>,
) -> Result<(), CliError> {
    match (
        has_cursor,
        expected_context_meta_event_id,
        expected_context_revision,
        expected_projection_generation,
    ) {
        (false, None, None, None) => Ok(()),
        (true, Some(meta), Some(revision), Some(generation)) => {
            validate_snapshot_fields(meta, revision, generation)
        }
        _ => Err(CliError::Usage(
            "Project Context continuation requires cursor, meta Event, revision, and projection generation"
                .to_owned(),
        )),
    }
}

fn edge_page(
    edges: &[EdgeIdentityOutput],
    start: usize,
    limit: u8,
) -> (Vec<EdgeIdentityOutput>, EdgePageOutput) {
    let remaining = &edges[start..];
    let take = remaining.len().min(usize::from(limit));
    let page_edges = remaining[..take]
        .iter()
        .map(|edge| EdgeIdentityOutput {
            edge_key: edge.edge_key,
            binding_document_count: edge.binding_document_count,
        })
        .collect::<Vec<_>>();
    let truncated = remaining.len() > take;
    let next_after_edge_key = if truncated {
        page_edges.last().map(|edge| edge.edge_key)
    } else {
        None
    };
    (
        page_edges,
        EdgePageOutput {
            limit,
            next_after_edge_key,
            truncated,
        },
    )
}

fn document_page(documents: &[Uuid], start: usize, limit: u8) -> (Vec<Uuid>, DocumentPageOutput) {
    let remaining = &documents[start..];
    let take = remaining.len().min(usize::from(limit));
    let page_documents = remaining[..take].to_vec();
    let truncated = remaining.len() > take;
    let next_after_document_id = if truncated {
        page_documents.last().copied()
    } else {
        None
    };
    (
        page_documents,
        DocumentPageOutput {
            limit,
            next_after_document_id,
            truncated,
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn validate_edge_continuation(
    snapshot: &ContextSnapshotIdentity,
    edges: &[EdgeIdentityOutput],
    after_edge: Option<&str>,
    expected_context_meta_event_id: Option<&str>,
    expected_context_revision: Option<u64>,
    expected_projection_generation: Option<u64>,
) -> Result<usize, CliError> {
    match (
        after_edge,
        expected_context_meta_event_id,
        expected_context_revision,
        expected_projection_generation,
    ) {
        (None, None, None, None) => Ok(0),
        (Some(cursor), Some(meta), Some(revision), Some(generation)) => {
            validate_snapshot_expectation(snapshot, meta, revision, generation)?;
            let cursor = parse_edge_key(cursor)?;
            edges
                .binary_search_by_key(&cursor, |edge| edge.edge_key)
                .map(|index| index + 1)
                .map_err(|_| {
                    CliError::NotFound(
                        "Project Context Edge cursor was not found in the Coordinate scope"
                            .to_owned(),
                    )
                })
        }
        _ => Err(CliError::Usage(
            "Edge continuation requires cursor, meta Event, revision, and projection generation"
                .to_owned(),
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_document_continuation(
    snapshot: &ContextSnapshotIdentity,
    documents: &[Uuid],
    after_document: Option<Uuid>,
    expected_context_meta_event_id: Option<&str>,
    expected_context_revision: Option<u64>,
    expected_projection_generation: Option<u64>,
) -> Result<usize, CliError> {
    match (
        after_document,
        expected_context_meta_event_id,
        expected_context_revision,
        expected_projection_generation,
    ) {
        (None, None, None, None) => Ok(0),
        (Some(cursor), Some(meta), Some(revision), Some(generation)) => {
            validate_snapshot_expectation(snapshot, meta, revision, generation)?;
            documents
                .binary_search(&cursor)
                .map(|index| index + 1)
                .map_err(|_| {
                    CliError::NotFound(
                        "Project Context Document cursor was not found in the Edge scope"
                            .to_owned(),
                    )
                })
        }
        _ => Err(CliError::Usage(
            "Document continuation requires cursor, meta Event, revision, and projection generation"
                .to_owned(),
        )),
    }
}

fn validate_snapshot_expectation(
    snapshot: &ContextSnapshotIdentity,
    meta_event_id: &str,
    context_revision: u64,
    projection_generation: u64,
) -> Result<(), CliError> {
    validate_snapshot_fields(meta_event_id, context_revision, projection_generation)?;
    if snapshot.context_meta_event_id != meta_event_id
        || snapshot.context_revision != context_revision
        || snapshot.projection_generation != projection_generation
    {
        return Err(CliError::Conflict(
            "conflict:project_context:continuation_snapshot_changed".to_owned(),
        ));
    }
    Ok(())
}

fn validate_snapshot_fields(
    meta_event_id: &str,
    context_revision: u64,
    projection_generation: u64,
) -> Result<(), CliError> {
    validate_event_id(meta_event_id)?;
    if context_revision == 0
        || context_revision > MAX_SAFE_REVISION
        || projection_generation == 0
        || projection_generation > MAX_SAFE_REVISION
    {
        return Err(CliError::Usage(
            "Project Context continuation revisions are out of range".to_owned(),
        ));
    }
    Ok(())
}

fn validate_event_id(value: &str) -> Result<(), CliError> {
    if value.len() != 64
        || value.bytes().any(|byte| !byte.is_ascii_hexdigit())
        || value.bytes().any(|byte| byte.is_ascii_uppercase())
        || EventId::from_hex(value).is_err()
    {
        return Err(CliError::Usage(
            "Context meta Event ID must be canonical lowercase hex64".to_owned(),
        ));
    }
    Ok(())
}

fn parse_edge_key(value: &str) -> Result<EdgeKey, CliError> {
    EdgeKey::from_hex(value).map_err(|error| CliError::Usage(error.to_string()))
}

fn write_bounded_json(value: &impl Serialize, format: &OutputFormat) -> Result<(), CliError> {
    let stdout = io::stdout();
    let mut lock = stdout.lock();
    write_bounded_json_to(value, format, MAX_STRUCTURAL_OUTPUT_BYTES, &mut lock)
}

fn write_bounded_json_to(
    value: &impl Serialize,
    format: &OutputFormat,
    max_bytes: usize,
    writer: &mut impl Write,
) -> Result<(), CliError> {
    let mut bytes = match format {
        OutputFormat::Json => serde_json::to_vec_pretty(value),
        OutputFormat::Compact => serde_json::to_vec(value),
    }
    .map_err(|error| CliError::Other(format!("failed to serialize output: {error}")))?;
    bytes.push(b'\n');
    if bytes.len() > max_bytes {
        return Err(CliError::Usage(format!(
            "Project Context response_too_large: output exceeds {max_bytes} bytes"
        )));
    }
    writer
        .write_all(&bytes)
        .map_err(|error| CliError::Other(format!("failed to write stdout: {error}")))
}

fn integrity_error(message: impl Into<String>) -> CliError {
    CliError::Other(format!(
        "Project Context integrity error: {}",
        message.into()
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use clap::Parser;
    use nostr::Keys;
    use serde::ser::SerializeStruct;

    use super::*;
    use crate::{Cli, Cmd, ProjectContextCmd, ProjectContextCoordinateCmd};

    struct CountingWriter {
        writes: Arc<AtomicUsize>,
        bytes: Vec<u8>,
    }

    impl Write for CountingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct Payload<'a>(&'a str);

    impl Serialize for Payload<'_> {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            let mut state = serializer.serialize_struct("Payload", 1)?;
            state.serialize_field("value", self.0)?;
            state.end()
        }
    }

    fn snapshot() -> ContextSnapshotIdentity {
        ContextSnapshotIdentity {
            context_meta_event_id: "11".repeat(32),
            context_revision: 7,
            projection_generation: 9,
        }
    }

    #[test]
    fn continuation_compares_snapshot_before_cursor_membership() {
        let edge = EdgeKey::from_hex(&"22".repeat(32)).expect("EdgeKey");
        let edges = vec![EdgeIdentityOutput {
            edge_key: edge,
            binding_document_count: 1,
        }];
        let error = validate_edge_continuation(
            &snapshot(),
            &edges,
            Some(&"33".repeat(32)),
            Some(&"44".repeat(32)),
            Some(7),
            Some(9),
        )
        .expect_err("snapshot mismatch wins");
        assert!(matches!(error, CliError::Conflict(_)));
    }

    #[test]
    fn serialized_first_page_round_trips_through_clap_without_overlap() {
        let edges = ["11", "22", "33"]
            .into_iter()
            .map(|pair| EdgeIdentityOutput {
                edge_key: EdgeKey::from_hex(&pair.repeat(32)).expect("EdgeKey"),
                binding_document_count: 1,
            })
            .collect::<Vec<_>>();
        let (first, first_page) = edge_page(&edges, 0, 2);
        assert!(first_page.truncated);
        let output = CoordinateEdgesOutput {
            project_id: Uuid::new_v4(),
            snapshot: snapshot(),
            coordinate: ProjectContextCoordinate::Document {
                document_id: Uuid::new_v4(),
            },
            edges: first.clone(),
            page: first_page,
        };
        let value = serde_json::to_value(output).expect("serialize first page");
        let cursor = value["page"]["next_after_edge_key"]
            .as_str()
            .expect("serialized cursor");
        let meta = value["snapshot"]["context_meta_event_id"]
            .as_str()
            .expect("serialized meta");
        let revision = value["snapshot"]["context_revision"]
            .as_u64()
            .expect("serialized revision");
        let generation = value["snapshot"]["projection_generation"]
            .as_u64()
            .expect("serialized generation");
        let cli = Cli::try_parse_from([
            "cf",
            "project-context",
            "coordinate",
            "edges",
            "document:10000000-0000-4000-8000-000000000001",
            "--limit",
            "2",
            "--after-edge",
            cursor,
            "--expected-context-meta-event-id",
            meta,
            "--expected-context-revision",
            &revision.to_string(),
            "--expected-projection-generation",
            &generation.to_string(),
        ])
        .expect("parse serialized continuation");
        let Cmd::ProjectContext(ProjectContextCmd::Coordinate {
            command:
                ProjectContextCoordinateCmd::Edges {
                    after_edge,
                    expected_context_meta_event_id,
                    expected_context_revision,
                    expected_projection_generation,
                    ..
                },
        }) = cli.command
        else {
            panic!("expected Coordinate Edges continuation");
        };
        let start = validate_edge_continuation(
            &snapshot(),
            &edges,
            after_edge.as_deref(),
            expected_context_meta_event_id.as_deref(),
            expected_context_revision,
            expected_projection_generation,
        )
        .expect("accept same snapshot continuation");
        let (second, second_page) = edge_page(&edges, start, 2);
        assert!(!second_page.truncated);
        assert!(first
            .iter()
            .all(|left| second.iter().all(|right| left.edge_key != right.edge_key)));
        let union = first
            .iter()
            .chain(&second)
            .map(|edge| edge.edge_key)
            .collect::<Vec<_>>();
        assert_eq!(
            union,
            edges.iter().map(|edge| edge.edge_key).collect::<Vec<_>>()
        );

        let changed = ContextSnapshotIdentity {
            context_meta_event_id: "44".repeat(32),
            ..snapshot()
        };
        let error = validate_edge_continuation(
            &changed,
            &edges,
            after_edge.as_deref(),
            expected_context_meta_event_id.as_deref(),
            expected_context_revision,
            expected_projection_generation,
        )
        .expect_err("meta replacement conflicts at the same revision and generation");
        assert!(matches!(error, CliError::Conflict(_)));
    }

    #[test]
    fn structural_result_variants_are_field_isolated() {
        let edge_key = EdgeKey::from_hex(&"22".repeat(32)).expect("EdgeKey");
        let coordinate = ProjectContextCoordinate::Document {
            document_id: Uuid::new_v4(),
        };
        let coordinate_edges = serde_json::to_value(CoordinateEdgesOutput {
            project_id: Uuid::new_v4(),
            snapshot: snapshot(),
            coordinate: coordinate.clone(),
            edges: vec![EdgeIdentityOutput {
                edge_key,
                binding_document_count: 2,
            }],
            page: EdgePageOutput {
                limit: 32,
                next_after_edge_key: None,
                truncated: false,
            },
        })
        .expect("coordinate edges");
        assert!(coordinate_edges.get("documents").is_none());
        assert!(coordinate_edges.get("coordinates").is_none());

        let edge_documents = serde_json::to_value(EdgeDocumentsOutput {
            project_id: Uuid::new_v4(),
            snapshot: snapshot(),
            edge_key,
            documents: Vec::new(),
            page: Some(DocumentPageOutput {
                limit: 32,
                next_after_document_id: None,
                truncated: false,
            }),
        })
        .expect("edge documents");
        assert!(edge_documents.get("coordinates").is_none());

        let edge_coordinates = serde_json::to_value(EdgeCoordinatesOutput {
            project_id: Uuid::new_v4(),
            snapshot: snapshot(),
            edge_key,
            coordinates: vec![unavailable_coordinate_observation(coordinate)],
        })
        .expect("edge coordinates");
        assert!(edge_coordinates.get("documents").is_none());
    }

    #[test]
    fn new_document_observations_are_summary_first_and_revision_pinned() {
        let document_id = Uuid::new_v4();
        let actor = Keys::generate().public_key();
        let metadata = DocumentMetadata::Active {
            title: "Frontend authorization".to_owned(),
            summary: Some("Client-side checks and error presentation.".to_owned()),
            document_revision: 7,
            updated_at: Utc::now(),
            updated_by: actor,
        };
        let relation = context_document_observation(document_id, Some(&metadata))
            .expect("active relation Document");
        let relation_json = serde_json::to_value(relation).expect("serialize relation");
        assert_eq!(
            relation_json["fetch_command"],
            format!("cf documents get {document_id} --revision 7 --content-only")
        );
        assert_eq!(
            relation_json["summary"],
            "Client-side checks and error presentation."
        );

        let coordinate = document_coordinate_observation(
            ProjectContextCoordinate::Document { document_id },
            Some(&metadata),
        );
        let coordinate_json = serde_json::to_value(coordinate).expect("serialize Coordinate");
        assert_eq!(coordinate_json["document_revision"], 7);
        assert_eq!(
            coordinate_json["fetch_command"],
            format!("cf documents get {document_id} --revision 7 --content-only")
        );
        assert_eq!(
            coordinate_json["summary"],
            "Client-side checks and error presentation."
        );

        let unavailable = serde_json::to_value(
            context_document_observation(document_id, None).expect("unavailable relation"),
        )
        .expect("serialize unavailable relation");
        assert_eq!(unavailable["state"], "unavailable");
        assert!(unavailable.get("fetch_command").is_none());
    }

    #[test]
    fn bounded_writer_counts_the_single_lf_and_never_writes_oversize_output() {
        for format in [OutputFormat::Json, OutputFormat::Compact] {
            let serialized = match format {
                OutputFormat::Json => serde_json::to_vec_pretty(&Payload("abc")),
                OutputFormat::Compact => serde_json::to_vec(&Payload("abc")),
            }
            .expect("serialize");
            let writes = Arc::new(AtomicUsize::new(0));
            let mut exact = CountingWriter {
                writes: Arc::clone(&writes),
                bytes: Vec::new(),
            };
            write_bounded_json_to(&Payload("abc"), &format, serialized.len() + 1, &mut exact)
                .expect("exact boundary");
            assert_eq!(writes.load(Ordering::SeqCst), 1);
            assert_eq!(exact.bytes.last(), Some(&b'\n'));

            let writes = Arc::new(AtomicUsize::new(0));
            let mut over = CountingWriter {
                writes: Arc::clone(&writes),
                bytes: Vec::new(),
            };
            let error =
                write_bounded_json_to(&Payload("abc"), &format, serialized.len(), &mut over)
                    .expect_err("one byte over");
            assert!(matches!(error, CliError::Usage(_)));
            assert_eq!(writes.load(Ordering::SeqCst), 0);
            assert!(over.bytes.is_empty());
        }
    }

    #[test]
    fn pretty_and_compact_apply_their_actual_output_bytes() {
        let mut compact = Vec::new();
        write_bounded_json_to(
            &Payload("abc"),
            &OutputFormat::Compact,
            MAX_STRUCTURAL_OUTPUT_BYTES,
            &mut compact,
        )
        .expect("compact");
        let mut pretty = Vec::new();
        write_bounded_json_to(
            &Payload("abc"),
            &OutputFormat::Json,
            MAX_STRUCTURAL_OUTPUT_BYTES,
            &mut pretty,
        )
        .expect("pretty");
        assert_ne!(compact, pretty);
        assert_eq!(compact.iter().filter(|byte| **byte == b'\n').count(), 1);
        assert!(pretty.iter().filter(|byte| **byte == b'\n').count() > 1);
    }
}
