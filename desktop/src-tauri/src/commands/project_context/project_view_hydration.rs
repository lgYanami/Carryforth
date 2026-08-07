//! Project View v3 metadata hydration for Project Context coordinates.

use std::collections::{BTreeMap, BTreeSet};

use buzz_project_context_pkg::ProjectContextCoordinate;
use buzz_project_view_pkg::v3::{ProjectViewEntryV3, ProjectViewObjectDataV3};
use serde_json::{json, Value};
use uuid::Uuid;

use super::{
    coordinate_dto, coordinate_key, unavailable_coordinate, ProjectContextCommandError,
    ProjectContextCoordinateDetail, ProjectContextDetailState,
    ProjectContextProjectViewObservation, ProjectContextReadContext, ProjectContextSourceState,
};
use crate::app_state::AppState;
use crate::commands::project_view::{
    fetch_consistent_verified_v3_snapshot_at, ProjectViewReadError,
};

pub(super) struct ProjectViewHydration {
    pub(super) observation: ProjectContextProjectViewObservation,
    pub(super) coordinates: BTreeMap<ProjectContextCoordinate, ProjectContextCoordinateDetail>,
}

pub(super) async fn hydrate_project_view(
    state: &AppState,
    context: &ProjectContextReadContext,
    project_id: Uuid,
    requested: &BTreeSet<ProjectContextCoordinate>,
    required_by_edge: &BTreeSet<ProjectContextCoordinate>,
) -> Result<ProjectViewHydration, ProjectContextCommandError> {
    if requested.is_empty() {
        return Ok(ProjectViewHydration {
            observation: empty_observation(ProjectContextSourceState::NotRequested),
            coordinates: BTreeMap::new(),
        });
    }
    let snapshot = match fetch_consistent_verified_v3_snapshot_at(
        state,
        context.identity,
        &context.api_base_url,
        &context.keys,
    )
    .await
    {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => {
            return Err(ProjectContextCommandError::verification_failed(
                "Project View metadata is absent while Context metadata exists",
            ));
        }
        Err(ProjectViewReadError::Forbidden) => {
            return Err(ProjectContextCommandError::restricted());
        }
        Err(ProjectViewReadError::Conflict(_) | ProjectViewReadError::Unavailable(_)) => {
            return Ok(ProjectViewHydration {
                observation: empty_observation(ProjectContextSourceState::Unavailable),
                coordinates: requested
                    .iter()
                    .cloned()
                    .map(|coordinate| {
                        let detail = unavailable_coordinate(&coordinate);
                        (coordinate, detail)
                    })
                    .collect(),
            });
        }
        Err(ProjectViewReadError::Other(_)) => {
            return Err(ProjectContextCommandError::verification_failed(
                "the Project View hydration snapshot is invalid",
            ));
        }
    };
    if *snapshot.meta().project_id.as_uuid() != project_id {
        return Err(ProjectContextCommandError::verification_failed(
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
            return Err(ProjectContextCommandError::internal());
        };
        let Some(entry) = snapshot.entry(*object_id) else {
            if required_by_edge.contains(coordinate) {
                return Err(ProjectContextCommandError::verification_failed(
                    "verified Project View omitted a Context Edge coordinate",
                ));
            }
            coordinates.insert(coordinate.clone(), unavailable_coordinate(coordinate));
            continue;
        };
        if entry.object_type() != *object_type {
            if required_by_edge.contains(coordinate) {
                return Err(ProjectContextCommandError::verification_failed(
                    "a Context Edge coordinate has a different Project View object type",
                ));
            }
            coordinates.insert(coordinate.clone(), unavailable_coordinate(coordinate));
            continue;
        }
        coordinates.insert(
            coordinate.clone(),
            project_view_coordinate_detail(coordinate, entry),
        );
    }
    Ok(ProjectViewHydration {
        observation: ProjectContextProjectViewObservation {
            state: ProjectContextSourceState::Observed,
            project_revision: Some(snapshot.meta().project_revision),
            projection_generation: Some(snapshot.meta().projection_generation),
            updated_at: Some(snapshot.meta().updated_at),
            meta_event_id: Some(snapshot.meta().event_id.to_hex()),
        },
        coordinates,
    })
}

fn empty_observation(state: ProjectContextSourceState) -> ProjectContextProjectViewObservation {
    ProjectContextProjectViewObservation {
        state,
        project_revision: None,
        projection_generation: None,
        updated_at: None,
        meta_event_id: None,
    }
}

pub(super) fn project_view_coordinate_detail(
    coordinate: &ProjectContextCoordinate,
    entry: &ProjectViewEntryV3,
) -> ProjectContextCoordinateDetail {
    match entry {
        ProjectViewEntryV3::Active(object) => {
            let (title, status) = title_status(&object.data);
            ProjectContextCoordinateDetail {
                coordinate_key: coordinate_key(coordinate),
                coordinate: coordinate_dto(coordinate),
                state: ProjectContextDetailState::Active,
                title: Some(title),
                status,
                object_revision: Some(object.object_revision),
                document_revision: None,
                meeting: None,
                updated_at: Some(object.updated_at),
                updated_by: Some(object.updated_by.to_hex()),
                unavailable_reason: None,
            }
        }
        ProjectViewEntryV3::Tombstone(tombstone) => ProjectContextCoordinateDetail {
            coordinate_key: coordinate_key(coordinate),
            coordinate: coordinate_dto(coordinate),
            state: ProjectContextDetailState::Tombstoned,
            title: None,
            status: None,
            object_revision: Some(tombstone.object_revision),
            document_revision: None,
            meeting: None,
            updated_at: Some(tombstone.deleted_at),
            updated_by: Some(tombstone.deleted_by.to_hex()),
            unavailable_reason: None,
        },
    }
}

fn title_status(data: &ProjectViewObjectDataV3) -> (String, Option<Value>) {
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
