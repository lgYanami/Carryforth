//! Verified, read-only Project View bridge for the desktop client.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::Duration;

use buzz_core_pkg::kind::{
    KIND_NIP43_MEMBERSHIP_LIST, KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_OBJECT,
};
use buzz_core_pkg::PublicKey;
use buzz_project_view_pkg::v2::{
    CommunityMemberRole, ProposalStatus, RoleAssignment, RoleAssignmentProposal, RoleCheckpoint,
    RoleContinuityChange, RoleDefinition, RoleHandoff, RoleLevel, WorkCommitment,
};
use buzz_project_view_pkg::v3::ProjectViewObjectV3;
use buzz_project_view_pkg::{
    ProjectRole, ProjectView, ProjectViewEntry, ProjectViewObject, ProjectViewObjectData,
    ProjectViewObjectType, ProjectViewRelations, ProjectViewState,
};
use buzz_sdk_pkg::project_view::{
    parse_meta_projection, parse_object_projection, MetaProjection, ObjectProjection,
    ProjectedObject,
};
use buzz_sdk_pkg::project_view_v2::{
    parse_entity_projection as parse_v2_entity_projection,
    parse_membership_projection as parse_v2_membership_projection,
    parse_meta_projection as parse_v2_meta_projection,
    parse_project_object_projection as parse_v2_project_object_projection, V2MembershipProjection,
    V2MetaProjection, V2ProjectedObject,
};
use buzz_sdk_pkg::role_brief::{RoleBrief, VerifiedRoleBriefSnapshot};
use chrono::{DateTime, Utc};
use nostr::{Event, Keys};
use serde::Serialize;
use serde_json::json;
use tauri::State;

use crate::app_state::AppState;
use crate::relay::{query_relay, query_relay_at_with_keys_typed, RelayHttpErrorCategory};

pub(crate) const PROJECT_VIEW_V1_EXTENSION: &str = "buzz-project-view-v1";
pub(crate) const PROJECT_VIEW_V2_EXTENSION: &str = "buzz-project-view-v2";
pub(crate) const PROJECT_VIEW_V3_EXTENSION: &str = "buzz-project-view-v3";
pub(crate) const PROJECT_CONTEXT_REFERENCE_EXTENSION: &str = "buzz-project-context-v1";
const SNAPSHOT_PAGE_SIZE: usize = 500;
const SNAPSHOT_MAX_ATTEMPTS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectViewSchema {
    V1,
    V2,
    V3,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProjectViewIdentity {
    pub(crate) relay_pubkey: PublicKey,
    pub(crate) schema: ProjectViewSchema,
    pub(crate) project_document_supported: bool,
    pub(crate) project_context_reference_supported: bool,
    pub(crate) project_context_edge_supported: bool,
}

struct ProjectSnapshot {
    meta: MetaProjection,
    view: ProjectView,
}

struct V2ProjectSnapshot {
    meta: V2MetaProjection,
    view: ProjectView,
    role_continuity: ProjectViewRoleContinuity,
}

/// Verified Role continuity state returned beside a schema-v2 Project View.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectViewRoleContinuity {
    roles: Vec<RoleDefinition>,
    proposals: Vec<RoleAssignmentProposal>,
    assignments: Vec<RoleAssignment>,
    commitments: Vec<WorkCommitment>,
    work_responsibilities: Vec<ProjectViewWorkResponsibility>,
    checkpoints: Vec<RoleCheckpoint>,
    handoffs: Vec<RoleHandoff>,
    members: Vec<ProjectViewMembershipMember>,
    briefs: Vec<RoleBrief>,
}

/// Versioned Role continuity payload. Its JSON shape stays stable while the
/// Role definition and Role Brief major are selected by Project View schema.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum ProjectViewRoleContinuityPayload {
    /// Schema-v2 continuity and Role Briefs.
    V2(ProjectViewRoleContinuity),
    /// Schema-v3 continuity and strict base RoleBriefV3 values.
    V3(ProjectViewRoleContinuityV3),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectViewWorkResponsibility {
    work_id: uuid::Uuid,
    role_id: uuid::Uuid,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectViewMembershipMember {
    pubkey: String,
    role: CommunityMemberRole,
}

/// Failures produced while assembling a verified Project View read snapshot.
#[derive(Debug)]
pub(crate) enum ProjectViewReadError {
    /// The current identity is not permitted to read the Project View.
    Forbidden,
    /// A bounded read observed incompatible source revisions.
    Conflict(String),
    /// The verified source could not be reached temporarily.
    Unavailable(String),
    /// The response was malformed, unverifiable, or otherwise invalid.
    Other(String),
}

/// Result type shared by Project View's verified native readers.
pub(crate) type ProjectViewReadResult<T> = Result<T, ProjectViewReadError>;

/// Desktop-facing state of the active Community's Project View.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProjectViewLoadResult {
    /// The Relay does not advertise Project View support.
    Unsupported,
    /// The current identity may not read this Community's Project View.
    Forbidden,
    /// Project View is supported but has not been initialized.
    Uninitialized {
        /// Canonical Relay signing identity established by NIP-11.
        relay_pubkey: String,
    },
    /// A complete, internally consistent and cryptographically verified view.
    Ready {
        /// Canonical Relay signing identity established by NIP-11.
        relay_pubkey: String,
        /// Whether the independent Context sub-capability is currently ready.
        project_context_supported: bool,
        /// Project View protocol schema selected by the Community.
        schema_version: u16,
        /// Current optimistic-concurrency revision.
        project_revision: u64,
        /// Current projection generation.
        projection_generation: u64,
        /// Number of active objects declared by the metadata projection.
        active_object_count: u32,
        /// Canonical server time of the projected state.
        updated_at: DateTime<Utc>,
        /// Deterministically assembled legacy Project View hierarchy.
        #[serde(skip_serializing_if = "Option::is_none")]
        view: Option<Box<ProjectView>>,
        /// Strict flat schema-v3 objects. TypeScript assembles the hierarchy
        /// without inventing a legacy Resource locator.
        #[serde(skip_serializing_if = "Option::is_none")]
        objects_v3: Option<Vec<ProjectViewObjectV3>>,
        /// Verified Role continuity state for schema v2 or v3.
        #[serde(skip_serializing_if = "Option::is_none")]
        role_continuity: Option<Box<ProjectViewRoleContinuityPayload>>,
    },
}

/// Load the active Community's complete, verified Project View snapshot.
#[tauri::command]
pub async fn get_project_view(state: State<'_, AppState>) -> Result<ProjectViewLoadResult, String> {
    load_project_view(&state).await
}

mod identity;
use identity::read_identity;
pub(crate) use identity::read_identity_at;
mod role_history;
pub use role_history::*;
mod v3;
pub(crate) use v3::fetch_consistent_verified_v3_snapshot_at;
pub use v3::ProjectViewRoleContinuityV3;
use v3::{fetch_consistent_v3_snapshot, read_v3_meta, V3ProjectSnapshot};

fn read_error_message(error: ProjectViewReadError) -> String {
    match error {
        ProjectViewReadError::Forbidden => {
            "restricted: Project View requires current Community membership".to_owned()
        }
        ProjectViewReadError::Conflict(message)
        | ProjectViewReadError::Unavailable(message)
        | ProjectViewReadError::Other(message) => message,
    }
}

async fn load_project_view(state: &AppState) -> Result<ProjectViewLoadResult, String> {
    let Some(identity) = read_identity(state).await? else {
        return Ok(ProjectViewLoadResult::Unsupported);
    };

    let loaded = match identity.schema {
        ProjectViewSchema::V1 => fetch_consistent_snapshot(state, identity)
            .await
            .map(|snapshot| {
                snapshot.map(
                    |ProjectSnapshot { meta, view }| ProjectViewLoadResult::Ready {
                        relay_pubkey: identity.relay_pubkey.to_hex(),
                        project_context_supported: false,
                        schema_version: 1,
                        project_revision: meta.project_revision,
                        projection_generation: meta.projection_generation,
                        active_object_count: meta.active_object_count,
                        updated_at: meta.updated_at,
                        view: Some(Box::new(view)),
                        objects_v3: None,
                        role_continuity: None,
                    },
                )
            }),
        ProjectViewSchema::V2 => {
            fetch_consistent_v2_snapshot(state, identity)
                .await
                .map(|snapshot| {
                    snapshot.map(
                        |V2ProjectSnapshot {
                             meta,
                             view,
                             role_continuity,
                         }| ProjectViewLoadResult::Ready {
                            relay_pubkey: identity.relay_pubkey.to_hex(),
                            project_context_supported: false,
                            schema_version: 2,
                            project_revision: meta.project_revision,
                            projection_generation: meta.projection_generation,
                            active_object_count: meta.entity_counts.active_objects,
                            updated_at: meta.updated_at,
                            view: Some(Box::new(view)),
                            objects_v3: None,
                            role_continuity: Some(Box::new(ProjectViewRoleContinuityPayload::V2(
                                role_continuity,
                            ))),
                        },
                    )
                })
        }
        ProjectViewSchema::V3 => {
            fetch_consistent_v3_snapshot(state, identity)
                .await
                .map(|snapshot| {
                    snapshot.map(
                        |V3ProjectSnapshot {
                             meta,
                             objects,
                             role_continuity,
                         }| ProjectViewLoadResult::Ready {
                            relay_pubkey: identity.relay_pubkey.to_hex(),
                            project_context_supported: identity.project_context_reference_supported,
                            schema_version: 3,
                            project_revision: meta.project_revision,
                            projection_generation: meta.projection_generation,
                            active_object_count: meta.entity_counts.active_objects,
                            updated_at: meta.updated_at,
                            view: None,
                            objects_v3: Some(objects),
                            role_continuity: Some(Box::new(ProjectViewRoleContinuityPayload::V3(
                                role_continuity,
                            ))),
                        },
                    )
                })
        }
    };
    match loaded {
        Ok(Some(result)) => Ok(result),
        Ok(None) => Ok(ProjectViewLoadResult::Uninitialized {
            relay_pubkey: identity.relay_pubkey.to_hex(),
        }),
        Err(ProjectViewReadError::Forbidden) => Ok(ProjectViewLoadResult::Forbidden),
        Err(ProjectViewReadError::Conflict(message))
        | Err(ProjectViewReadError::Unavailable(message))
        | Err(ProjectViewReadError::Other(message)) => Err(message),
    }
}

async fn fetch_consistent_snapshot(
    state: &AppState,
    identity: ProjectViewIdentity,
) -> ProjectViewReadResult<Option<ProjectSnapshot>> {
    for attempt in 0..SNAPSHOT_MAX_ATTEMPTS {
        match fetch_snapshot_once(state, identity).await {
            Ok(snapshot) => return Ok(snapshot),
            Err(ProjectViewReadError::Conflict(_)) if attempt + 1 < SNAPSHOT_MAX_ATTEMPTS => {
                let backoff_ms = 25_u64 << attempt;
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            }
            Err(error) => return Err(error),
        }
    }
    Err(ProjectViewReadError::Conflict(
        "Project View changed during every bounded snapshot attempt".to_owned(),
    ))
}

async fn fetch_snapshot_once(
    state: &AppState,
    identity: ProjectViewIdentity,
) -> ProjectViewReadResult<Option<ProjectSnapshot>> {
    let Some(meta) = read_meta(state, identity).await? else {
        return Ok(None);
    };

    let mut after: Option<(String, String)> = None;
    let mut entries = Vec::new();
    let mut object_ids = HashSet::new();

    loop {
        let mut extension = json!({
            "revision": meta.project_revision,
            "projection_generation": meta.projection_generation,
        });
        if let Some((object_type, object_id)) = &after {
            extension["after"] = json!({
                "object_type": object_type,
                "object_id": object_id,
            });
        }
        let filter = json!({
            "kinds": [KIND_PROJECT_VIEW_OBJECT],
            "authors": [identity.relay_pubkey.to_hex()],
            "#t": ["buzz-project-view-active"],
            "limit": SNAPSHOT_PAGE_SIZE,
            "buzz_project_view": extension,
        });
        let page = query_project_view(state, &[filter]).await?;
        if page.len() > SNAPSHOT_PAGE_SIZE {
            return Err(integrity_read_error(
                "snapshot page exceeded the requested page size",
            ));
        }

        for event in &page {
            let projection =
                parse_object_projection(event, &identity.relay_pubkey, meta.project_id)
                    .map_err(|error| integrity_read_error(error.to_string()))?;
            validate_object_against_meta(&projection, &meta)?;
            let object = match projection.object {
                ProjectedObject::Active(object) => *object,
                ProjectedObject::Tombstone(_) => {
                    return Err(integrity_read_error(
                        "active snapshot query returned a tombstone",
                    ));
                }
            };
            let cursor = (
                object.object_type.as_str().to_owned(),
                object.id.to_string(),
            );
            if after.as_ref().is_some_and(|previous| cursor <= *previous) {
                return Err(integrity_read_error(
                    "snapshot page order is not strictly increasing",
                ));
            }
            if !object_ids.insert(object.id) {
                return Err(integrity_read_error(
                    "snapshot contains a duplicate active object id",
                ));
            }
            after = Some(cursor);
            entries.push(ProjectViewEntry::Active(object));
            if entries.len() > meta.active_object_count as usize {
                return Err(integrity_read_error(
                    "snapshot contains more objects than metadata declares",
                ));
            }
        }

        if page.len() < SNAPSHOT_PAGE_SIZE {
            break;
        }
    }

    let final_meta = read_meta(state, identity)
        .await?
        .ok_or_else(|| conflict_error("Project View metadata disappeared"))?;
    if final_meta.projection_generation != meta.projection_generation
        || final_meta.project_revision != meta.project_revision
        || final_meta.event_id != meta.event_id
    {
        return Err(conflict_error(
            "Project View changed while assembling the snapshot",
        ));
    }
    if entries.len() != meta.active_object_count as usize {
        return Err(integrity_read_error(format!(
            "snapshot contains {} active objects but metadata declares {}",
            entries.len(),
            meta.active_object_count
        )));
    }

    let initialized_at = entries.iter().find_map(|entry| match entry {
        ProjectViewEntry::Active(object)
            if object.object_type == ProjectViewObjectType::ProjectProfile =>
        {
            Some(object.created_at)
        }
        _ => None,
    });
    let state = ProjectViewState::from_snapshot(
        meta.project_id,
        meta.project_revision,
        initialized_at,
        Some(meta.updated_at),
        entries,
    )
    .map_err(|error| integrity_read_error(format!("invalid Project View snapshot: {error}")))?;
    let view = ProjectView::assemble(&state)
        .map_err(|error| integrity_read_error(format!("cannot assemble Project View: {error}")))?;
    Ok(Some(ProjectSnapshot { meta, view }))
}

async fn fetch_consistent_v2_snapshot(
    state: &AppState,
    identity: ProjectViewIdentity,
) -> ProjectViewReadResult<Option<V2ProjectSnapshot>> {
    for attempt in 0..SNAPSHOT_MAX_ATTEMPTS {
        match fetch_v2_snapshot_once(state, identity).await {
            Ok(snapshot) => return Ok(snapshot),
            Err(ProjectViewReadError::Conflict(_)) if attempt + 1 < SNAPSHOT_MAX_ATTEMPTS => {
                let backoff_ms = 25_u64 << attempt;
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            }
            Err(error) => return Err(error),
        }
    }
    Err(conflict_error(
        "Project View v2 changed during every bounded snapshot attempt",
    ))
}

async fn fetch_v2_snapshot_once(
    state: &AppState,
    identity: ProjectViewIdentity,
) -> ProjectViewReadResult<Option<V2ProjectSnapshot>> {
    let Some(meta) = read_v2_meta(state, identity).await? else {
        return Ok(None);
    };
    let ordinary_events = query_project_view(
        state,
        &[json!({
            "kinds": [KIND_PROJECT_VIEW_OBJECT],
            "authors": [identity.relay_pubkey.to_hex()],
            "#t": ["buzz-project-view-v2-object"],
        })],
    )
    .await?;
    let entity_events = query_v2_current_entities(state, identity, &meta).await?;

    let mut event_ids = HashSet::with_capacity(ordinary_events.len() + entity_events.len());
    let mut object_ids = HashSet::new();
    let mut entries = Vec::new();
    let mut work_responsibilities = Vec::new();
    let mut object_projections = Vec::with_capacity(ordinary_events.len());
    for event in &ordinary_events {
        if !event_ids.insert(event.id) {
            return Err(integrity_read_error(
                "v2 object query returned a duplicate event",
            ));
        }
        let projection =
            parse_v2_project_object_projection(event, &identity.relay_pubkey, meta.project_id)
                .map_err(|error| integrity_read_error(error.to_string()))?;
        validate_v2_projection_basis(
            projection.projection_generation,
            projection.project_revision,
            &meta,
        )?;
        if let V2ProjectedObject::Active(object) = &projection.object {
            if !object_ids.insert(object.id) {
                return Err(integrity_read_error(
                    "v2 snapshot contains a duplicate active object ID",
                ));
            }
            if object.object_type == ProjectViewObjectType::Work {
                if let Some(role_id) = projection.responsible_role_id {
                    work_responsibilities.push(ProjectViewWorkResponsibility {
                        work_id: object.id,
                        role_id,
                    });
                }
            }
            entries.push(ProjectViewEntry::Active((**object).clone()));
        }
        object_projections.push(projection);
    }

    let mut roles = Vec::new();
    let mut proposals = Vec::new();
    let mut assignments = Vec::new();
    let mut commitments = Vec::new();
    let mut checkpoints = Vec::new();
    let mut handoffs = Vec::new();
    let mut entity_projections = Vec::with_capacity(entity_events.len());
    for event in &entity_events {
        if !event_ids.insert(event.id) {
            return Err(integrity_read_error(
                "v2 entity query returned a duplicate event",
            ));
        }
        let projection = parse_v2_entity_projection(event, &identity.relay_pubkey, meta.project_id)
            .map_err(|error| integrity_read_error(error.to_string()))?;
        validate_v2_projection_basis(
            projection.projection_generation,
            projection.project_revision,
            &meta,
        )?;
        match &projection.entity {
            RoleContinuityChange::Role(role) => {
                if !object_ids.insert(role.role_id) {
                    return Err(integrity_read_error(
                        "v2 Role head collides with another active object ID",
                    ));
                }
                entries.push(ProjectViewEntry::Active(project_object_from_role(role)));
                roles.push(role.clone());
            }
            RoleContinuityChange::Proposal(proposal) => proposals.push(proposal.clone()),
            RoleContinuityChange::Assignment(assignment) => assignments.push(assignment.clone()),
            RoleContinuityChange::Commitment(commitment) => commitments.push(commitment.clone()),
            RoleContinuityChange::Checkpoint(checkpoint) => checkpoints.push(checkpoint.clone()),
            RoleContinuityChange::Handoff(handoff) => handoffs.push(handoff.clone()),
        }
        entity_projections.push(projection);
    }
    let membership = read_v2_membership(state, identity, &meta).await?;
    validate_v2_counts_and_membership(
        &meta,
        V2ContinuitySlices {
            roles: &roles,
            proposals: &proposals,
            assignments: &assignments,
            commitments: &commitments,
            checkpoints: &checkpoints,
            handoffs: &handoffs,
        },
        &membership,
    )?;

    if entries.len() != meta.entity_counts.active_objects as usize {
        return Err(integrity_read_error(format!(
            "v2 snapshot contains {} active objects but metadata declares {}",
            entries.len(),
            meta.entity_counts.active_objects
        )));
    }
    let initialized_at = entries.iter().find_map(|entry| match entry {
        ProjectViewEntry::Active(object)
            if object.object_type == ProjectViewObjectType::ProjectProfile =>
        {
            Some(object.created_at)
        }
        _ => None,
    });
    let project_state = ProjectViewState::from_snapshot(
        meta.project_id,
        meta.project_revision,
        initialized_at,
        Some(meta.updated_at),
        entries,
    )
    .map_err(|error| integrity_read_error(format!("invalid v2 Project snapshot: {error}")))?;
    ProjectView::assemble(&project_state).map_err(|error| {
        integrity_read_error(format!("cannot assemble v2 Project View: {error}"))
    })?;

    let final_meta = read_v2_meta(state, identity)
        .await?
        .ok_or_else(|| conflict_error("Project View v2 metadata disappeared"))?;
    if final_meta.event_id != meta.event_id {
        return Err(conflict_error(
            "Project View v2 changed while assembling the snapshot",
        ));
    }
    let verified = VerifiedRoleBriefSnapshot::new_with_partial_history(
        meta.clone(),
        membership.clone(),
        object_projections,
        entity_projections,
    )
    .map_err(|error| integrity_read_error(error.to_string()))?;
    let view = verified.project_view().clone();
    let brief_members = assignments
        .iter()
        .filter(|assignment| assignment.is_active())
        .map(|assignment| assignment.member_pubkey)
        .collect::<BTreeSet<_>>();
    let brief_generated_at = Utc::now();
    let briefs = brief_members
        .into_iter()
        .map(|member| {
            verified
                .brief_for(member, brief_generated_at)
                .map_err(|error| integrity_read_error(error.to_string()))
        })
        .collect::<ProjectViewReadResult<Vec<_>>>()?;
    roles.sort_by_key(|role| role.role_id);
    proposals.sort_by_key(|proposal| proposal.proposal_id);
    assignments.sort_by_key(|assignment| assignment.assignment_id);
    commitments.sort_by_key(|commitment| commitment.commitment_id);
    work_responsibilities.sort_by_key(|responsibility| responsibility.work_id);
    checkpoints.sort_by_key(|checkpoint| checkpoint.checkpoint_id);
    handoffs.sort_by_key(|handoff| handoff.handoff_id);
    let members = membership
        .members
        .into_iter()
        .map(|member| ProjectViewMembershipMember {
            pubkey: member.pubkey.to_hex(),
            role: member.role,
        })
        .collect();
    Ok(Some(V2ProjectSnapshot {
        meta,
        view,
        role_continuity: ProjectViewRoleContinuity {
            roles,
            proposals,
            assignments,
            commitments,
            work_responsibilities,
            checkpoints,
            handoffs,
            members,
            briefs,
        },
    }))
}

async fn query_v2_current_entities(
    state: &AppState,
    identity: ProjectViewIdentity,
    meta: &V2MetaProjection,
) -> ProjectViewReadResult<Vec<Event>> {
    let mut events = Vec::new();
    let mut event_ids = HashSet::new();
    let mut after: Option<serde_json::Value> = None;
    loop {
        let mut extension = json!({
            "scope": "v2_current_entities",
            "revision": meta.project_revision,
            "projection_generation": meta.projection_generation,
        });
        if let Some(cursor) = &after {
            extension["after"] = cursor.clone();
        }
        let page = query_project_view(
            state,
            &[json!({
                "kinds": [KIND_PROJECT_VIEW_OBJECT],
                "authors": [identity.relay_pubkey.to_hex()],
                "#t": ["buzz-project-view-v2-entity"],
                "limit": SNAPSHOT_PAGE_SIZE,
                "buzz_project_view": extension,
            })],
        )
        .await?;
        if page.len() > SNAPSHOT_PAGE_SIZE {
            return Err(integrity_read_error(
                "v2 current-entity page exceeded its requested limit",
            ));
        }
        let page_len = page.len();
        for event in page {
            if !event_ids.insert(event.id) {
                return Err(integrity_read_error(
                    "v2 current-entity pages contain a duplicate event",
                ));
            }
            let projection =
                parse_v2_entity_projection(&event, &identity.relay_pubkey, meta.project_id)
                    .map_err(|error| integrity_read_error(error.to_string()))?;
            after = Some(json!({
                "entity_type": projection.entity.entity_type().as_str(),
                "entity_id": projection.entity.entity_id(),
            }));
            events.push(event);
        }
        if page_len < SNAPSHOT_PAGE_SIZE {
            break;
        }
    }
    Ok(events)
}

fn project_object_from_role(role: &RoleDefinition) -> ProjectViewObject {
    ProjectViewObject {
        id: role.role_id,
        object_type: ProjectViewObjectType::Role,
        object_revision: role.object_revision,
        project_revision: role.project_revision,
        created_at: role.created_at,
        updated_at: role.updated_at,
        created_by: role.created_by,
        updated_by: role.updated_by,
        data: ProjectViewObjectData::Role(ProjectRole {
            name: role.name.clone(),
            purpose: role.purpose.clone(),
            responsibilities: role.responsibilities.clone(),
            boundaries: role.boundaries.clone(),
            active: role.active,
        }),
        relations: ProjectViewRelations::default(),
    }
}

fn validate_v2_projection_basis(
    projection_generation: u64,
    project_revision: u64,
    meta: &V2MetaProjection,
) -> ProjectViewReadResult<()> {
    if projection_generation != meta.projection_generation {
        return Err(conflict_error(
            "v2 head generation differs from current metadata",
        ));
    }
    if project_revision > meta.project_revision {
        return Err(integrity_read_error(
            "v2 head is newer than current metadata",
        ));
    }
    Ok(())
}

async fn read_v2_membership(
    state: &AppState,
    identity: ProjectViewIdentity,
    meta: &V2MetaProjection,
) -> ProjectViewReadResult<V2MembershipProjection> {
    let events = query_project_view(
        state,
        &[json!({
            "ids": [meta.membership_snapshot_event_id.to_hex()],
            "kinds": [KIND_NIP43_MEMBERSHIP_LIST],
            "authors": [identity.relay_pubkey.to_hex()],
            "limit": 2,
        })],
    )
    .await?;
    let [event] = events.as_slice() else {
        return Err(integrity_read_error(
            "v2 metadata membership pointer did not resolve exactly once",
        ));
    };
    if event.id != meta.membership_snapshot_event_id {
        return Err(integrity_read_error(
            "membership query returned an event other than the metadata pointer",
        ));
    }
    parse_v2_membership_projection(event, &identity.relay_pubkey)
        .map_err(|error| integrity_read_error(error.to_string()))
}

struct V2ContinuitySlices<'a> {
    roles: &'a [RoleDefinition],
    proposals: &'a [RoleAssignmentProposal],
    assignments: &'a [RoleAssignment],
    commitments: &'a [WorkCommitment],
    checkpoints: &'a [RoleCheckpoint],
    handoffs: &'a [RoleHandoff],
}

fn validate_v2_counts_and_membership(
    meta: &V2MetaProjection,
    continuity: V2ContinuitySlices<'_>,
    membership: &V2MembershipProjection,
) -> ProjectViewReadResult<()> {
    let open_proposals = continuity
        .proposals
        .iter()
        .filter(|proposal| proposal.status == ProposalStatus::Open)
        .count();
    let active_assignments = continuity
        .assignments
        .iter()
        .filter(|assignment| assignment.is_active())
        .count();
    let active_commitments = continuity
        .commitments
        .iter()
        .filter(|commitment| commitment.is_active())
        .count();
    if usize::try_from(meta.entity_counts.open_proposals).ok() != Some(open_proposals)
        || usize::try_from(meta.entity_counts.active_assignments).ok() != Some(active_assignments)
        || usize::try_from(meta.entity_counts.active_commitments).ok() != Some(active_commitments)
        || usize::try_from(meta.entity_counts.checkpoints)
            .ok()
            .is_none_or(|count| continuity.checkpoints.len() > count)
        || usize::try_from(meta.entity_counts.handoffs)
            .ok()
            .is_none_or(|count| continuity.handoffs.len() > count)
    {
        return Err(integrity_read_error(
            "v2 metadata counts disagree with verified Role heads",
        ));
    }
    let roles_by_id = continuity
        .roles
        .iter()
        .map(|role| (role.role_id, role))
        .collect::<BTreeMap<_, _>>();
    let members = membership
        .members
        .iter()
        .map(|member| (member.pubkey, member.role))
        .collect::<BTreeMap<_, _>>();
    let mut assigned_members = HashSet::new();
    for assignment in continuity
        .assignments
        .iter()
        .filter(|assignment| assignment.is_active())
    {
        if !assigned_members.insert(assignment.member_pubkey) {
            return Err(integrity_read_error(
                "one Member has multiple active Assignment heads",
            ));
        }
        let role = roles_by_id.get(&assignment.role_id).ok_or_else(|| {
            integrity_read_error("an active Assignment references a missing Role")
        })?;
        let actual_role = members
            .get(&assignment.member_pubkey)
            .ok_or_else(|| integrity_read_error("an active assignee is absent from membership"))?;
        let expected_role = match role.level {
            RoleLevel::Admin => CommunityMemberRole::Admin,
            RoleLevel::Member => CommunityMemberRole::Member,
        };
        if *actual_role != CommunityMemberRole::Owner && *actual_role != expected_role {
            return Err(integrity_read_error(
                "an active Assignment disagrees with Community membership",
            ));
        }
    }
    for (pubkey, role) in members {
        if role == CommunityMemberRole::Admin
            && !continuity.assignments.iter().any(|assignment| {
                assignment.is_active()
                    && assignment.member_pubkey == pubkey
                    && roles_by_id
                        .get(&assignment.role_id)
                        .is_some_and(|role| role.level == RoleLevel::Admin)
            })
        {
            return Err(integrity_read_error(
                "a non-owner admin has no active Leader Assignment",
            ));
        }
    }
    Ok(())
}

async fn read_meta(
    state: &AppState,
    identity: ProjectViewIdentity,
) -> ProjectViewReadResult<Option<MetaProjection>> {
    let events = query_project_view(
        state,
        &[json!({
            "kinds": [KIND_PROJECT_VIEW_META],
            "authors": [identity.relay_pubkey.to_hex()],
            "limit": 2,
        })],
    )
    .await?;
    match events.as_slice() {
        [] => Ok(None),
        [event] => parse_meta_projection(event, &identity.relay_pubkey)
            .map(Some)
            .map_err(|error| integrity_read_error(error.to_string())),
        _ => Err(integrity_read_error(
            "metadata query returned multiple current heads",
        )),
    }
}

async fn read_v2_meta(
    state: &AppState,
    identity: ProjectViewIdentity,
) -> ProjectViewReadResult<Option<V2MetaProjection>> {
    let events = query_project_view(
        state,
        &[json!({
            "kinds": [KIND_PROJECT_VIEW_META],
            "authors": [identity.relay_pubkey.to_hex()],
            "limit": 2,
        })],
    )
    .await?;
    match events.as_slice() {
        [] => Ok(None),
        [event] => parse_v2_meta_projection(event, &identity.relay_pubkey)
            .map(Some)
            .map_err(|error| integrity_read_error(error.to_string())),
        _ => Err(integrity_read_error(
            "v2 metadata query returned multiple current heads",
        )),
    }
}

async fn query_project_view(
    state: &AppState,
    filters: &[serde_json::Value],
) -> ProjectViewReadResult<Vec<Event>> {
    query_relay(state, filters).await.map_err(|message| {
        if message.starts_with("relay returned 403") {
            ProjectViewReadError::Forbidden
        } else if message.starts_with("relay returned 409") {
            conflict_error("Project View changed during snapshot pagination")
        } else {
            ProjectViewReadError::Other(message)
        }
    })
}

/// Query Project View through a Relay URL and signer captured before any await.
pub(crate) async fn query_project_view_at_with_keys(
    state: &AppState,
    api_base_url: &str,
    keys: &Keys,
    filters: &[serde_json::Value],
) -> ProjectViewReadResult<Vec<Event>> {
    query_relay_at_with_keys_typed(state, api_base_url, filters, keys, None)
        .await
        .map_err(|error| match error.category {
            RelayHttpErrorCategory::Forbidden => ProjectViewReadError::Forbidden,
            RelayHttpErrorCategory::Conflict => {
                conflict_error("Project View changed during snapshot pagination")
            }
            RelayHttpErrorCategory::Connect
            | RelayHttpErrorCategory::Timeout
            | RelayHttpErrorCategory::RateLimited
            | RelayHttpErrorCategory::Unavailable => {
                ProjectViewReadError::Unavailable(error.message)
            }
            RelayHttpErrorCategory::Http
                if error
                    .status
                    .is_some_and(|status| (500..=504).contains(&status)) =>
            {
                ProjectViewReadError::Unavailable(error.message)
            }
            RelayHttpErrorCategory::Http
            | RelayHttpErrorCategory::Malformed
            | RelayHttpErrorCategory::Internal => ProjectViewReadError::Other(error.message),
        })
}

fn validate_object_against_meta(
    projection: &ObjectProjection,
    meta: &MetaProjection,
) -> ProjectViewReadResult<()> {
    if projection.project_id != meta.project_id {
        return Err(integrity_read_error(
            "object projection belongs to a different project than metadata",
        ));
    }
    if projection.projection_generation != meta.projection_generation {
        return Err(conflict_error(
            "object projection generation differs from current metadata",
        ));
    }
    if projection.project_revision > meta.project_revision {
        return Err(integrity_read_error(
            "object projection is newer than current metadata",
        ));
    }
    Ok(())
}

fn conflict_error(message: impl Into<String>) -> ProjectViewReadError {
    ProjectViewReadError::Conflict(message.into())
}

fn integrity_read_error(message: impl Into<String>) -> ProjectViewReadError {
    ProjectViewReadError::Other(integrity_error(message))
}

fn integrity_error(message: impl Into<String>) -> String {
    format!("Project View integrity error: {}", message.into())
}

#[cfg(test)]
#[path = "project_view_tests.rs"]
mod tests;
