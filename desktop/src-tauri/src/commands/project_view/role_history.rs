//! Revision-pinned, verified Role history pagination.

use std::collections::HashSet;

use buzz_core_pkg::kind::KIND_PROJECT_VIEW_OBJECT;
use buzz_core_pkg::{CommunityId, EventId};
use buzz_project_view_pkg::v2::{RoleContinuityChange, RoleContinuityEntity};
use buzz_sdk_pkg::project_view_v3::{
    parse_entity_projection as parse_v3_entity_projection, V3EntityChange,
    PROJECT_VIEW_V3_ENTITY_TAG, PROJECT_VIEW_V3_ROLE_HISTORY_SCOPE,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::State;

use super::{
    integrity_error, query_project_view, read_error_message, read_identity, read_v3_meta, AppState,
    ProjectViewIdentity, ProjectViewSchema,
};

struct HistoryMeta {
    event_id: EventId,
    project_id: CommunityId,
    project_revision: u64,
    projection_generation: u64,
}

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
        return Err("Project View Role history is unsupported".to_owned());
    };
    if identity.schema != ProjectViewSchema::V3 {
        return Err("Project View Role history requires schema v3".to_owned());
    }
    identity.require_runtime_ready("Project View Role history")?;
    let meta = read_history_meta(&state, identity)
        .await?
        .ok_or_else(|| "Project View is uninitialized".to_owned())?;
    if meta.project_revision != input.project_revision
        || meta.projection_generation != input.projection_generation
    {
        return Err("conflict:project_view:snapshot_changed".to_owned());
    }

    let mut extension = json!({
        "scope": PROJECT_VIEW_V3_ROLE_HISTORY_SCOPE,
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
            "#t": [PROJECT_VIEW_V3_ENTITY_TAG],
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
            parse_v3_entity_projection(&event, &identity.relay_pubkey, meta.project_id)
                .map_err(|error| integrity_error(error.to_string()))?;
        let projection_generation = projection.projection_generation;
        let project_revision = projection.project_revision;
        let entity = v3_history_change(projection.entity)?;
        if projection_generation != meta.projection_generation
            || project_revision > meta.project_revision
        {
            return Err(integrity_error(
                "Role history head does not match the verified metadata basis",
            ));
        }
        let cursor = ProjectViewRoleHistoryCursor {
            project_revision,
            entity_type: entity.entity_type(),
            entity_id: entity.entity_id(),
        };
        if history_entity_order(cursor.entity_type).is_none()
            || previous.is_some_and(|previous| !history_cursor_precedes(previous, cursor))
            || history_role_id(&entity) != Some(input.role_id)
        {
            return Err(integrity_error(
                "Role history page violates its requested Role or canonical order",
            ));
        }
        previous = Some(cursor);
        items.push(entity);
    }

    let final_meta = read_history_meta(&state, identity)
        .await?
        .ok_or_else(|| "Project View metadata disappeared".to_owned())?;
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

async fn read_history_meta(
    state: &AppState,
    identity: ProjectViewIdentity,
) -> Result<Option<HistoryMeta>, String> {
    if identity.schema != ProjectViewSchema::V3 {
        return Err("Project View Role history requires schema v3".to_owned());
    }
    read_v3_meta(state, identity)
        .await
        .map_err(read_error_message)
        .map(|meta| {
            meta.map(|meta| HistoryMeta {
                event_id: meta.event_id,
                project_id: meta.project_id,
                project_revision: meta.project_revision,
                projection_generation: meta.projection_generation,
            })
        })
}

fn v3_history_change(entity: V3EntityChange) -> Result<RoleContinuityChange, String> {
    match entity {
        V3EntityChange::Proposal(value) => Ok(RoleContinuityChange::Proposal(value)),
        V3EntityChange::Assignment(value) => Ok(RoleContinuityChange::Assignment(value)),
        V3EntityChange::Commitment(value) => Ok(RoleContinuityChange::Commitment(value)),
        V3EntityChange::Checkpoint(value) => Ok(RoleContinuityChange::Checkpoint(value)),
        V3EntityChange::Handoff(value) => Ok(RoleContinuityChange::Handoff(value)),
        V3EntityChange::Role(_) => Err(integrity_error(
            "Role history query returned a v3 Role definition",
        )),
    }
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
