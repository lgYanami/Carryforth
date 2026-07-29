//! Shared verified Project View v2 snapshot reader for CLI commands.

use std::collections::HashSet;
use std::time::Duration;

use buzz_core::kind::{
    KIND_NIP43_MEMBERSHIP_LIST, KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_OBJECT,
};
use buzz_core::PublicKey;
use buzz_project_view::v2::{RoleContinuityEntity, RuntimeFence};
use buzz_sdk::project_view_v2::{
    parse_entity_projection, parse_membership_projection, parse_meta_projection,
    parse_project_object_projection, V2EntityProjection, V2MembershipProjection, V2MetaProjection,
};
use buzz_sdk::role_brief::VerifiedRoleBriefSnapshot;
use nostr::Event;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::client::BuzzClient;
use crate::error::CliError;

pub(crate) const PROJECT_VIEW_V1_EXTENSION: &str = "buzz-project-view-v1";
pub(crate) const PROJECT_VIEW_V2_EXTENSION: &str = "buzz-project-view-v2";
const SNAPSHOT_ATTEMPTS: usize = 3;
const V2_ENTITY_PAGE_SIZE: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectViewSchema {
    V1,
    V2,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProjectViewIdentity {
    pub(crate) relay_pubkey: PublicKey,
    pub(crate) schema: ProjectViewSchema,
}

#[derive(Debug)]
pub(crate) struct RoleHistoryPage {
    pub(crate) projections: Vec<V2EntityProjection>,
    pub(crate) next_before: Option<String>,
}

pub(crate) struct RoleHistoryRequest<'a> {
    pub(crate) entity_types: &'a [RoleContinuityEntity],
    pub(crate) role_id: Option<uuid::Uuid>,
    pub(crate) assignment_id: Option<uuid::Uuid>,
    pub(crate) member_pubkey: Option<PublicKey>,
    pub(crate) limit: u16,
    pub(crate) before: Option<&'a str>,
}

#[derive(Deserialize)]
struct Nip11Document {
    #[serde(default)]
    supported_extensions: Vec<String>,
    #[serde(rename = "self")]
    relay_self: Option<String>,
}

pub(crate) async fn read_identity(
    client: &BuzzClient,
) -> Result<Option<ProjectViewIdentity>, CliError> {
    let raw = client.get_public("/info").await?;
    let info: Nip11Document = serde_json::from_str(&raw)
        .map_err(|error| integrity_error(format!("invalid NIP-11 document: {error}")))?;
    let schema = if info
        .supported_extensions
        .iter()
        .any(|extension| extension == PROJECT_VIEW_V2_EXTENSION)
    {
        ProjectViewSchema::V2
    } else if info
        .supported_extensions
        .iter()
        .any(|extension| extension == PROJECT_VIEW_V1_EXTENSION)
    {
        ProjectViewSchema::V1
    } else {
        return Ok(None);
    };
    let relay_self = info.relay_self.ok_or_else(|| {
        integrity_error("NIP-11 advertises Project View without a relay `self` key")
    })?;
    let relay_pubkey = PublicKey::from_hex(&relay_self)
        .map_err(|error| integrity_error(format!("invalid NIP-11 relay `self`: {error}")))?;
    if relay_pubkey.to_hex() != relay_self {
        return Err(integrity_error(
            "NIP-11 relay `self` is not canonical lowercase hex",
        ));
    }
    Ok(Some(ProjectViewIdentity {
        relay_pubkey,
        schema,
    }))
}

pub(crate) async fn require_v2_identity(
    client: &BuzzClient,
) -> Result<ProjectViewIdentity, CliError> {
    match read_identity(client).await? {
        Some(identity) if identity.schema == ProjectViewSchema::V2 => Ok(identity),
        _ => Err(CliError::Other(format!(
            "unsupported: relay does not advertise {PROJECT_VIEW_V2_EXTENSION}"
        ))),
    }
}

pub(crate) async fn read_verified_v2_snapshot(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
) -> Result<VerifiedRoleBriefSnapshot, CliError> {
    if identity.schema != ProjectViewSchema::V2 {
        return Err(CliError::Other(
            "unsupported: verified Role state requires Project View v2".to_owned(),
        ));
    }
    for attempt in 0..SNAPSHOT_ATTEMPTS {
        let before = read_meta(client, identity).await?;
        let ordinary_values = client
            .query_all(json!({
                "kinds": [KIND_PROJECT_VIEW_OBJECT],
                "authors": [identity.relay_pubkey.to_hex()],
                "#t": ["buzz-project-view-v2-object"],
            }))
            .await?;
        let entity_projections = read_current_entity_projections(client, identity, &before).await?;

        let mut event_ids =
            HashSet::with_capacity(ordinary_values.len() + entity_projections.len());
        let mut object_projections = Vec::with_capacity(ordinary_values.len());
        for value in ordinary_values {
            let event: Event = serde_json::from_value(value)
                .map_err(|error| integrity_error(format!("invalid v2 object event: {error}")))?;
            if !event_ids.insert(event.id) {
                return Err(integrity_error(
                    "v2 object query returned a duplicate event",
                ));
            }
            object_projections.push(
                parse_project_object_projection(&event, &identity.relay_pubkey, before.project_id)
                    .map_err(|error| integrity_error(error.to_string()))?,
            );
        }
        for projection in &entity_projections {
            if !event_ids.insert(projection.event_id) {
                return Err(integrity_error(
                    "v2 entity query returned a duplicate event",
                ));
            }
        }
        let membership = read_membership(client, identity, &before).await?;
        let after = read_meta(client, identity).await?;
        if before.event_id != after.event_id {
            if attempt + 1 < SNAPSHOT_ATTEMPTS {
                let backoff_ms = 25_u64 << attempt;
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                continue;
            }
            return Err(CliError::Conflict(
                "Project View v2 changed during every bounded snapshot attempt".to_owned(),
            ));
        }
        return VerifiedRoleBriefSnapshot::new_with_partial_history(
            before,
            membership,
            object_projections,
            entity_projections,
        )
        .map_err(|error| integrity_error(error.to_string()));
    }
    Err(CliError::Conflict(
        "Project View v2 snapshot could not be stabilized".to_owned(),
    ))
}

async fn read_current_entity_projections(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
    meta: &V2MetaProjection,
) -> Result<Vec<V2EntityProjection>, CliError> {
    let mut projections = Vec::new();
    let mut event_ids = HashSet::new();
    let mut after: Option<Value> = None;
    loop {
        let mut extension = json!({
            "scope": "v2_current_entities",
            "revision": meta.project_revision,
            "projection_generation": meta.projection_generation,
        });
        if let Some(cursor) = &after {
            extension["after"] = cursor.clone();
        }
        let filter = json!({
            "kinds": [KIND_PROJECT_VIEW_OBJECT],
            "authors": [identity.relay_pubkey.to_hex()],
            "#t": ["buzz-project-view-v2-entity"],
            "limit": V2_ENTITY_PAGE_SIZE,
            "buzz_project_view": extension,
        });
        let values: Vec<Value> = serde_json::from_str(&client.query(&filter).await?)
            .map_err(|error| integrity_error(format!("invalid v2 entity page: {error}")))?;
        if values.len() > V2_ENTITY_PAGE_SIZE {
            return Err(integrity_error(
                "v2 current-entity page exceeded the requested limit",
            ));
        }
        let page_len = values.len();
        for value in values {
            let event: Event = serde_json::from_value(value)
                .map_err(|error| integrity_error(format!("invalid v2 entity event: {error}")))?;
            if !event_ids.insert(event.id) {
                return Err(integrity_error(
                    "v2 current-entity pages contain a duplicate event",
                ));
            }
            let projection =
                parse_entity_projection(&event, &identity.relay_pubkey, meta.project_id)
                    .map_err(|error| integrity_error(error.to_string()))?;
            after = Some(json!({
                "entity_type": projection.entity.entity_type().as_str(),
                "entity_id": projection.entity.entity_id(),
            }));
            projections.push(projection);
        }
        if page_len < V2_ENTITY_PAGE_SIZE {
            break;
        }
    }
    Ok(projections)
}

pub(crate) async fn read_role_history_page(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
    meta: &V2MetaProjection,
    request: RoleHistoryRequest<'_>,
) -> Result<RoleHistoryPage, CliError> {
    let entity_types = request.entity_types;
    let role_id = request.role_id;
    let assignment_id = request.assignment_id;
    let member_pubkey = request.member_pubkey;
    let limit = request.limit;
    let before = request.before;
    if entity_types.is_empty() {
        return Err(CliError::Usage(
            "Role history requires at least one entity type".to_owned(),
        ));
    }
    let after = before
        .map(|cursor| parse_history_cursor(cursor, entity_types))
        .transpose()?;
    let mut extension = json!({
        "scope": "role_history",
        "revision": meta.project_revision,
        "projection_generation": meta.projection_generation,
        "entity_types": entity_types
            .iter()
            .map(|entity_type| entity_type.as_str())
            .collect::<Vec<_>>(),
    });
    if let Some(role_id) = role_id {
        extension["role_id"] = json!(role_id);
    }
    if let Some(assignment_id) = assignment_id {
        extension["assignment_id"] = json!(assignment_id);
    }
    if let Some(member_pubkey) = member_pubkey {
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
        "#t": ["buzz-project-view-v2-entity"],
        "limit": limit,
        "buzz_project_view": extension,
    });
    let values: Vec<Value> = serde_json::from_str(&client.query(&filter).await?)
        .map_err(|error| integrity_error(format!("invalid Role history page: {error}")))?;
    if values.len() > usize::from(limit) {
        return Err(integrity_error(
            "Role history page exceeded the requested limit",
        ));
    }
    let mut projections = Vec::with_capacity(values.len());
    let mut event_ids = HashSet::with_capacity(values.len());
    let mut previous: Option<RoleHistoryCursor> = None;
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
                "Role history page violates its requested type, revision, or canonical order",
            ));
        }
        validate_history_projection_filter(&projection, role_id, assignment_id, member_pubkey)?;
        previous = Some(cursor);
        projections.push(projection);
    }
    let next_before = (projections.len() == usize::from(limit))
        .then(|| previous.map(format_history_cursor))
        .flatten();
    Ok(RoleHistoryPage {
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
    projection: &V2EntityProjection,
    role_id: Option<uuid::Uuid>,
    assignment_id: Option<uuid::Uuid>,
    member_pubkey: Option<PublicKey>,
) -> Result<(), CliError> {
    let (actual_role, actual_assignment, actual_member) = match &projection.entity {
        buzz_project_view::v2::RoleContinuityChange::Proposal(proposal) => {
            (proposal.role_id, None, Some(proposal.candidate_pubkey))
        }
        buzz_project_view::v2::RoleContinuityChange::Assignment(assignment) => (
            assignment.role_id,
            Some(assignment.assignment_id),
            Some(assignment.member_pubkey),
        ),
        buzz_project_view::v2::RoleContinuityChange::Checkpoint(checkpoint) => (
            checkpoint.role_id,
            Some(checkpoint.assignment_id),
            Some(checkpoint.created_by),
        ),
        buzz_project_view::v2::RoleContinuityChange::Handoff(handoff) => (
            handoff.role_id,
            Some(handoff.from_assignment_id),
            handoff.created_by,
        ),
        buzz_project_view::v2::RoleContinuityChange::Role(_)
        | buzz_project_view::v2::RoleContinuityChange::Commitment(_) => {
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

pub(crate) async fn read_current_v2_snapshot(
    client: &BuzzClient,
) -> Result<VerifiedRoleBriefSnapshot, CliError> {
    let identity = require_v2_identity(client).await?;
    read_verified_v2_snapshot(client, identity).await
}

pub(crate) fn is_managed_runtime() -> bool {
    std::env::var("BUZZ_MANAGED_AGENT").as_deref() == Ok("1")
}

pub(crate) fn runtime_fence_from_env() -> Result<Option<RuntimeFence>, CliError> {
    let runtime_id = std::env::var("BUZZ_RUNTIME_ID").ok();
    let runtime_epoch = std::env::var("BUZZ_RUNTIME_EPOCH").ok();
    match (runtime_id, runtime_epoch) {
        (None, None) => Ok(None),
        (Some(runtime_id), Some(runtime_epoch)) => {
            let runtime_fence = RuntimeFence {
                runtime_id: runtime_id
                    .parse()
                    .map_err(|error| CliError::Auth(format!("invalid BUZZ_RUNTIME_ID: {error}")))?,
                runtime_epoch: runtime_epoch.parse().map_err(|error| {
                    CliError::Auth(format!("invalid BUZZ_RUNTIME_EPOCH: {error}"))
                })?,
            };
            runtime_fence.validate().map_err(|error| {
                CliError::Auth(format!("invalid managed runtime fence: {error}"))
            })?;
            Ok(Some(runtime_fence))
        }
        _ => Err(CliError::Auth(
            "BUZZ_RUNTIME_ID and BUZZ_RUNTIME_EPOCH must be supplied together".to_owned(),
        )),
    }
}

async fn read_meta(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
) -> Result<V2MetaProjection, CliError> {
    let filter = json!({
        "kinds": [KIND_PROJECT_VIEW_META],
        "authors": [identity.relay_pubkey.to_hex()],
        "limit": 2,
    });
    let values: Vec<Value> = serde_json::from_str(&client.query(&filter).await?)
        .map_err(|error| integrity_error(format!("invalid v2 metadata response: {error}")))?;
    let [value] = values.as_slice() else {
        return Err(integrity_error(
            "v2 metadata query did not return exactly one current head",
        ));
    };
    let event: Event = serde_json::from_value(value.clone())
        .map_err(|error| integrity_error(format!("invalid v2 metadata event: {error}")))?;
    parse_meta_projection(&event, &identity.relay_pubkey)
        .map_err(|error| integrity_error(error.to_string()))
}

async fn read_membership(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
    meta: &V2MetaProjection,
) -> Result<V2MembershipProjection, CliError> {
    let filter = json!({
        "ids": [meta.membership_snapshot_event_id.to_hex()],
        "kinds": [KIND_NIP43_MEMBERSHIP_LIST],
        "authors": [identity.relay_pubkey.to_hex()],
        "limit": 2,
    });
    let values: Vec<Value> = serde_json::from_str(&client.query(&filter).await?)
        .map_err(|error| integrity_error(format!("invalid membership response: {error}")))?;
    let [value] = values.as_slice() else {
        return Err(integrity_error(
            "metadata membership pointer did not resolve to exactly one snapshot",
        ));
    };
    let event: Event = serde_json::from_value(value.clone())
        .map_err(|error| integrity_error(format!("invalid membership event: {error}")))?;
    if event.id != meta.membership_snapshot_event_id {
        return Err(integrity_error(
            "membership query returned an event other than the metadata pointer",
        ));
    }
    parse_membership_projection(&event, &identity.relay_pubkey)
        .map_err(|error| integrity_error(error.to_string()))
}

pub(crate) fn integrity_error(message: impl Into<String>) -> CliError {
    CliError::Other(format!(
        "Project View v2 integrity error: {}",
        message.into()
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        format_history_cursor, history_cursor_precedes, parse_history_cursor, RoleHistoryCursor,
    };
    use buzz_project_view::v2::RoleContinuityEntity;
    use uuid::Uuid;

    #[test]
    fn role_history_cursor_round_trips_and_is_type_scoped() {
        let cursor = RoleHistoryCursor {
            project_revision: 42,
            entity_type: RoleContinuityEntity::RoleCheckpoint,
            entity_id: Uuid::new_v4(),
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
            entity_id: Uuid::from_u128(2),
        };
        let low_id = RoleHistoryCursor {
            entity_id: Uuid::from_u128(1),
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
}
