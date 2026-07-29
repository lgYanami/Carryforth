//! Revision-pinned, verified Role history pagination.

use std::collections::HashSet;

use buzz_core_pkg::kind::KIND_PROJECT_VIEW_OBJECT;
use buzz_project_view_pkg::v2::{RoleContinuityChange, RoleContinuityEntity};
use buzz_sdk_pkg::project_view_v2::parse_entity_projection as parse_v2_entity_projection;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::State;

use super::{
    integrity_error, query_project_view, read_error_message, read_identity, read_v2_meta,
    validate_v2_projection_basis, AppState, ProjectViewSchema,
};

/// Revision-pinned keyset cursor for the Desktop Role history inspector.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct ProjectViewRoleHistoryCursor {
    project_revision: u64,
    entity_type: RoleContinuityEntity,
    entity_id: uuid::Uuid,
}

/// Closed request accepted by [`get_project_view_role_history`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectViewRoleHistoryInput {
    project_revision: u64,
    projection_generation: u64,
    role_id: uuid::Uuid,
    limit: u16,
    before: Option<ProjectViewRoleHistoryCursor>,
}

/// One verified, newest-first Role history page.
#[derive(Debug, Serialize)]
pub struct ProjectViewRoleHistoryPage {
    project_revision: u64,
    projection_generation: u64,
    items: Vec<RoleContinuityChange>,
    next_before: Option<ProjectViewRoleHistoryCursor>,
}

/// Load a bounded Role history page without expanding the default View
/// snapshot.
#[tauri::command]
pub async fn get_project_view_role_history(
    state: State<'_, AppState>,
    input: ProjectViewRoleHistoryInput,
) -> Result<ProjectViewRoleHistoryPage, String> {
    if !(1..=100).contains(&input.limit) {
        return Err("Project View Role history limit must be in 1..=100".to_owned());
    }
    if input.before.is_some_and(|cursor| {
        cursor.project_revision > input.project_revision
            || history_entity_order(cursor.entity_type).is_none()
    }) {
        return Err("Project View Role history cursor is invalid".to_owned());
    }
    let Some(identity) = read_identity(&state).await? else {
        return Err("Project View v2 is unsupported".to_owned());
    };
    if identity.schema != ProjectViewSchema::V2 {
        return Err("Project View Role history requires schema v2".to_owned());
    }
    let meta = read_v2_meta(&state, identity)
        .await
        .map_err(read_error_message)?
        .ok_or_else(|| "Project View v2 is uninitialized".to_owned())?;
    if meta.project_revision != input.project_revision
        || meta.projection_generation != input.projection_generation
    {
        return Err("conflict:project_view:snapshot_changed".to_owned());
    }

    let mut extension = json!({
        "scope": "role_history",
        "revision": input.project_revision,
        "projection_generation": input.projection_generation,
        "entity_types": [
            "role_assignment_proposal",
            "role_assignment",
            "role_checkpoint",
            "role_handoff",
        ],
        "role_id": input.role_id,
    });
    if let Some(before) = input.before {
        extension["after"] = json!({
            "project_revision": before.project_revision,
            "entity_type": before.entity_type.as_str(),
            "entity_id": before.entity_id,
        });
    }
    let events = query_project_view(
        &state,
        &[json!({
            "kinds": [KIND_PROJECT_VIEW_OBJECT],
            "authors": [identity.relay_pubkey.to_hex()],
            "#t": ["buzz-project-view-v2-entity"],
            "limit": input.limit,
            "buzz_project_view": extension,
        })],
    )
    .await
    .map_err(read_error_message)?;
    if events.len() > usize::from(input.limit) {
        return Err(integrity_error(
            "Role history page exceeded its requested limit",
        ));
    }

    let mut items = Vec::with_capacity(events.len());
    let mut event_ids = HashSet::with_capacity(events.len());
    let mut previous: Option<ProjectViewRoleHistoryCursor> = None;
    for event in events {
        if !event_ids.insert(event.id) {
            return Err(integrity_error(
                "Role history page contains a duplicate signed event",
            ));
        }
        let projection =
            parse_v2_entity_projection(&event, &identity.relay_pubkey, meta.project_id)
                .map_err(|error| integrity_error(error.to_string()))?;
        validate_v2_projection_basis(
            projection.projection_generation,
            projection.project_revision,
            &meta,
        )
        .map_err(read_error_message)?;
        let cursor = ProjectViewRoleHistoryCursor {
            project_revision: projection.project_revision,
            entity_type: projection.entity.entity_type(),
            entity_id: projection.entity.entity_id(),
        };
        if history_entity_order(cursor.entity_type).is_none()
            || previous.is_some_and(|previous| !history_cursor_precedes(previous, cursor))
            || history_role_id(&projection.entity) != Some(input.role_id)
        {
            return Err(integrity_error(
                "Role history page violates its requested Role or canonical order",
            ));
        }
        previous = Some(cursor);
        items.push(projection.entity);
    }

    let final_meta = read_v2_meta(&state, identity)
        .await
        .map_err(read_error_message)?
        .ok_or_else(|| "Project View v2 metadata disappeared".to_owned())?;
    if final_meta.event_id != meta.event_id {
        return Err("conflict:project_view:snapshot_changed".to_owned());
    }
    let next_before = (items.len() == usize::from(input.limit))
        .then_some(previous)
        .flatten();
    Ok(ProjectViewRoleHistoryPage {
        project_revision: meta.project_revision,
        projection_generation: meta.projection_generation,
        items,
        next_before,
    })
}

const fn history_entity_order(entity_type: RoleContinuityEntity) -> Option<u8> {
    match entity_type {
        RoleContinuityEntity::RoleAssignmentProposal => Some(0),
        RoleContinuityEntity::RoleAssignment => Some(1),
        RoleContinuityEntity::RoleCheckpoint => Some(2),
        RoleContinuityEntity::RoleHandoff => Some(3),
        RoleContinuityEntity::Role | RoleContinuityEntity::WorkCommitment => None,
    }
}

fn history_cursor_precedes(
    previous: ProjectViewRoleHistoryCursor,
    current: ProjectViewRoleHistoryCursor,
) -> bool {
    previous.project_revision > current.project_revision
        || (previous.project_revision == current.project_revision
            && (history_entity_order(previous.entity_type)
                < history_entity_order(current.entity_type)
                || (previous.entity_type == current.entity_type
                    && previous.entity_id > current.entity_id)))
}

const fn history_role_id(entity: &RoleContinuityChange) -> Option<uuid::Uuid> {
    match entity {
        RoleContinuityChange::Proposal(proposal) => Some(proposal.role_id),
        RoleContinuityChange::Assignment(assignment) => Some(assignment.role_id),
        RoleContinuityChange::Checkpoint(checkpoint) => Some(checkpoint.role_id),
        RoleContinuityChange::Handoff(handoff) => Some(handoff.role_id),
        RoleContinuityChange::Role(_) | RoleContinuityChange::Commitment(_) => None,
    }
}
