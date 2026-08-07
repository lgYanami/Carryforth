//! Strict schema-v3 Role continuity history reader for CLI commands.

use std::collections::HashSet;

use buzz_core::kind::KIND_PROJECT_VIEW_OBJECT;
use buzz_core::PublicKey;
use buzz_project_view::v2::RoleContinuityEntity;
use buzz_sdk::project_view_v3::{
    parse_entity_projection, V3EntityChange, V3EntityProjection, V3MetaProjection,
    PROJECT_VIEW_V3_ENTITY_TAG, PROJECT_VIEW_V3_ROLE_HISTORY_SCOPE,
};
use nostr::Event;
use serde_json::{json, Value};

use crate::client::BuzzClient;
use crate::commands::project_view_snapshot::{ProjectViewIdentity, ProjectViewSchema};
use crate::error::CliError;

#[derive(Debug)]
pub(crate) struct V3RoleHistoryPage {
    pub(crate) projections: Vec<V3EntityProjection>,
    pub(crate) next_before: Option<String>,
}

pub(crate) struct V3RoleHistoryRequest<'a> {
    pub(crate) entity_types: &'a [RoleContinuityEntity],
    pub(crate) role_id: Option<uuid::Uuid>,
    pub(crate) assignment_id: Option<uuid::Uuid>,
    pub(crate) member_pubkey: Option<PublicKey>,
    pub(crate) limit: u16,
    pub(crate) before: Option<&'a str>,
}

pub(crate) async fn read_v3_role_history_page(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
    meta: &V3MetaProjection,
    request: V3RoleHistoryRequest<'_>,
) -> Result<V3RoleHistoryPage, CliError> {
    if identity.schema != ProjectViewSchema::V3 {
        return Err(CliError::Other(
            "migration_required: Role history requires Project View schema v3".to_owned(),
        ));
    }
    let entity_types = request.entity_types;
    if entity_types.is_empty() {
        return Err(CliError::Usage(
            "Role history requires at least one entity type".to_owned(),
        ));
    }
    let after = request
        .before
        .map(|cursor| parse_history_cursor(cursor, entity_types))
        .transpose()?;
    let mut extension = json!({
        "scope": PROJECT_VIEW_V3_ROLE_HISTORY_SCOPE,
        "revision": meta.project_revision,
        "projection_generation": meta.projection_generation,
        "entity_types": entity_types
            .iter()
            .map(|entity_type| entity_type.as_str())
            .collect::<Vec<_>>(),
    });
    if let Some(role_id) = request.role_id {
        extension["role_id"] = json!(role_id);
    }
    if let Some(assignment_id) = request.assignment_id {
        extension["assignment_id"] = json!(assignment_id);
    }
    if let Some(member_pubkey) = request.member_pubkey {
        extension["member_pubkey"] = json!(member_pubkey.to_hex());
    }
    if let Some(after) = after {
        extension["after"] = json!({
            "project_revision": after.project_revision,
            "entity_type": after.entity_type.as_str(),
            "entity_id": after.entity_id,
        });
    }
    let filter = json!({
        "kinds": [KIND_PROJECT_VIEW_OBJECT],
        "authors": [identity.relay_pubkey.to_hex()],
        "#t": [PROJECT_VIEW_V3_ENTITY_TAG],
        "limit": request.limit,
        "buzz_project_view": extension,
    });
    let values: Vec<Value> = serde_json::from_str(&client.query(&filter).await?)
        .map_err(|error| integrity_error(format!("invalid Role history page: {error}")))?;
    if values.len() > usize::from(request.limit) {
        return Err(integrity_error(
            "Role history page exceeded the requested limit",
        ));
    }

    let mut projections = Vec::with_capacity(values.len());
    let mut event_ids = HashSet::with_capacity(values.len());
    let mut previous = None;
    for value in values {
        let event: Event = serde_json::from_value(value)
            .map_err(|error| integrity_error(format!("invalid Role history event: {error}")))?;
        if !event_ids.insert(event.id) {
            return Err(integrity_error(
                "Role history page contains a duplicate event",
            ));
        }
        let projection = parse_entity_projection(&event, &identity.relay_pubkey, meta.project_id)
            .map_err(|error| integrity_error(error.to_string()))?;
        let cursor = RoleHistoryCursor {
            project_revision: projection.project_revision,
            entity_type: projection.entity.entity_type(),
            entity_id: projection.entity.entity_id(),
        };
        if !entity_types.contains(&cursor.entity_type)
            || projection.projection_generation != meta.projection_generation
            || cursor.project_revision > meta.project_revision
            || previous.is_some_and(|previous| !history_cursor_precedes(previous, cursor))
        {
            return Err(integrity_error(
                "Role history page violates its requested type, revision, generation, or canonical order",
            ));
        }
        validate_history_projection_filter(
            &projection,
            request.role_id,
            request.assignment_id,
            request.member_pubkey,
        )?;
        previous = Some(cursor);
        projections.push(projection);
    }
    let next_before = (projections.len() == usize::from(request.limit))
        .then(|| previous.map(format_history_cursor))
        .flatten();
    Ok(V3RoleHistoryPage {
        projections,
        next_before,
    })
}

#[derive(Debug, Clone, Copy)]
struct RoleHistoryCursor {
    project_revision: u64,
    entity_type: RoleContinuityEntity,
    entity_id: uuid::Uuid,
}

fn parse_history_cursor(
    value: &str,
    allowed: &[RoleContinuityEntity],
) -> Result<RoleHistoryCursor, CliError> {
    let mut parts = value.splitn(3, ':');
    let project_revision = parts
        .next()
        .and_then(|part| part.parse::<u64>().ok())
        .filter(|revision| *revision > 0)
        .ok_or_else(|| CliError::Usage("invalid Role history cursor revision".to_owned()))?;
    let entity_type = match parts.next() {
        Some("role_assignment_proposal") => RoleContinuityEntity::RoleAssignmentProposal,
        Some("role_assignment") => RoleContinuityEntity::RoleAssignment,
        Some("role_checkpoint") => RoleContinuityEntity::RoleCheckpoint,
        Some("role_handoff") => RoleContinuityEntity::RoleHandoff,
        _ => {
            return Err(CliError::Usage(
                "invalid Role history cursor entity type".to_owned(),
            ));
        }
    };
    if !allowed.contains(&entity_type) {
        return Err(CliError::Usage(
            "Role history cursor type is outside this command".to_owned(),
        ));
    }
    let entity_id = parts
        .next()
        .and_then(|part| part.parse::<uuid::Uuid>().ok())
        .ok_or_else(|| CliError::Usage("invalid Role history cursor UUID".to_owned()))?;
    let cursor = RoleHistoryCursor {
        project_revision,
        entity_type,
        entity_id,
    };
    if format_history_cursor(cursor) != value {
        return Err(CliError::Usage(
            "Role history cursor is not canonical".to_owned(),
        ));
    }
    Ok(cursor)
}

fn format_history_cursor(cursor: RoleHistoryCursor) -> String {
    format!(
        "{}:{}:{}",
        cursor.project_revision,
        cursor.entity_type.as_str(),
        cursor.entity_id
    )
}

fn history_cursor_precedes(previous: RoleHistoryCursor, current: RoleHistoryCursor) -> bool {
    previous.project_revision > current.project_revision
        || (previous.project_revision == current.project_revision
            && (history_entity_order(previous.entity_type)
                < history_entity_order(current.entity_type)
                || (previous.entity_type == current.entity_type
                    && previous.entity_id > current.entity_id)))
}

const fn history_entity_order(entity_type: RoleContinuityEntity) -> u8 {
    match entity_type {
        RoleContinuityEntity::RoleAssignmentProposal => 0,
        RoleContinuityEntity::RoleAssignment => 1,
        RoleContinuityEntity::RoleCheckpoint => 2,
        RoleContinuityEntity::RoleHandoff => 3,
        RoleContinuityEntity::Role | RoleContinuityEntity::WorkCommitment => u8::MAX,
    }
}

fn validate_history_projection_filter(
    projection: &V3EntityProjection,
    role_id: Option<uuid::Uuid>,
    assignment_id: Option<uuid::Uuid>,
    member_pubkey: Option<PublicKey>,
) -> Result<(), CliError> {
    let (actual_role, actual_assignment, actual_member) = match &projection.entity {
        V3EntityChange::Proposal(proposal) => {
            (proposal.role_id, None, Some(proposal.candidate_pubkey))
        }
        V3EntityChange::Assignment(assignment) => (
            assignment.role_id,
            Some(assignment.assignment_id),
            Some(assignment.member_pubkey),
        ),
        V3EntityChange::Checkpoint(checkpoint) => (
            checkpoint.role_id,
            Some(checkpoint.assignment_id),
            Some(checkpoint.created_by),
        ),
        V3EntityChange::Handoff(handoff) => (
            handoff.role_id,
            Some(handoff.from_assignment_id),
            handoff.created_by,
        ),
        V3EntityChange::Role(_) | V3EntityChange::Commitment(_) => {
            return Err(integrity_error(
                "non-history entity appeared in a Role history page",
            ));
        }
    };
    if role_id.is_some_and(|expected| expected != actual_role)
        || assignment_id.is_some_and(|expected| Some(expected) != actual_assignment)
        || member_pubkey.is_some_and(|expected| Some(expected) != actual_member)
    {
        return Err(integrity_error(
            "Role history event is outside the requested Role or Assignment",
        ));
    }
    Ok(())
}

fn integrity_error(message: impl Into<String>) -> CliError {
    CliError::Other(format!(
        "Project View v3 integrity error: {}",
        message.into()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_history_cursor_round_trips_and_is_type_scoped() {
        let cursor = RoleHistoryCursor {
            project_revision: 42,
            entity_type: RoleContinuityEntity::RoleCheckpoint,
            entity_id: uuid::Uuid::new_v4(),
        };
        let encoded = format_history_cursor(cursor);
        let parsed = parse_history_cursor(&encoded, &[RoleContinuityEntity::RoleCheckpoint])
            .expect("parse canonical cursor");
        assert_eq!(parsed.project_revision, cursor.project_revision);
        assert_eq!(parsed.entity_type, cursor.entity_type);
        assert_eq!(parsed.entity_id, cursor.entity_id);
        assert!(parse_history_cursor(&encoded, &[RoleContinuityEntity::RoleHandoff]).is_err());
    }

    #[test]
    fn role_history_order_is_revision_type_then_descending_uuid() {
        let high = RoleHistoryCursor {
            project_revision: 7,
            entity_type: RoleContinuityEntity::RoleCheckpoint,
            entity_id: uuid::Uuid::from_u128(2),
        };
        let low_id = RoleHistoryCursor {
            entity_id: uuid::Uuid::from_u128(1),
            ..high
        };
        let handoff = RoleHistoryCursor {
            entity_type: RoleContinuityEntity::RoleHandoff,
            ..high
        };
        assert!(history_cursor_precedes(high, low_id));
        assert!(history_cursor_precedes(high, handoff));
        assert!(!history_cursor_precedes(low_id, high));
    }

    #[test]
    fn wire_contract_is_explicitly_v3_only() {
        assert_eq!(PROJECT_VIEW_V3_ROLE_HISTORY_SCOPE, "v3_role_history");
        assert_eq!(PROJECT_VIEW_V3_ENTITY_TAG, "buzz-project-view-v3-entity");
    }
}
