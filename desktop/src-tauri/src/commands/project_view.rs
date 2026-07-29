//! Verified, read-only Project View bridge for the desktop client.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::Duration;

use buzz_core_pkg::kind::{
    KIND_NIP43_MEMBERSHIP_LIST, KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_OBJECT,
};
use buzz_core_pkg::PublicKey;
use buzz_project_view_pkg::v2::{
    CommunityMemberRole, ProposalStatus, RoleAssignment, RoleAssignmentProposal,
    RoleContinuityChange, RoleDefinition, RoleHandoff, RoleLevel,
};
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
use nostr::Event;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::State;

use crate::app_state::AppState;
use crate::relay::{
    classify_request_error, parse_json_response, query_relay, relay_api_base_url_with_override,
    relay_error_message,
};

pub(crate) const PROJECT_VIEW_V1_EXTENSION: &str = "buzz-project-view-v1";
pub(crate) const PROJECT_VIEW_V2_EXTENSION: &str = "buzz-project-view-v2";
const SNAPSHOT_PAGE_SIZE: usize = 500;
const SNAPSHOT_MAX_ATTEMPTS: usize = 3;

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

#[derive(Debug, Deserialize)]
struct Nip11Document {
    #[serde(default)]
    supported_extensions: Vec<String>,
    #[serde(rename = "self")]
    relay_self: Option<String>,
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
    handoffs: Vec<RoleHandoff>,
    members: Vec<ProjectViewMembershipMember>,
    briefs: Vec<RoleBrief>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectViewMembershipMember {
    pubkey: String,
    role: CommunityMemberRole,
}

#[derive(Debug)]
enum ProjectViewReadError {
    Forbidden,
    Conflict(String),
    Other(String),
}

type ProjectViewReadResult<T> = Result<T, ProjectViewReadError>;

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
        /// Deterministically assembled Project View hierarchy.
        view: Box<ProjectView>,
        /// Verified Role continuity state for schema v2.
        #[serde(skip_serializing_if = "Option::is_none")]
        role_continuity: Option<Box<ProjectViewRoleContinuity>>,
    },
}

/// Load the active Community's complete, verified Project View snapshot.
#[tauri::command]
pub async fn get_project_view(state: State<'_, AppState>) -> Result<ProjectViewLoadResult, String> {
    load_project_view(&state).await
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
                        schema_version: 1,
                        project_revision: meta.project_revision,
                        projection_generation: meta.projection_generation,
                        active_object_count: meta.active_object_count,
                        updated_at: meta.updated_at,
                        view: Box::new(view),
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
                            schema_version: 2,
                            project_revision: meta.project_revision,
                            projection_generation: meta.projection_generation,
                            active_object_count: meta.entity_counts.active_objects,
                            updated_at: meta.updated_at,
                            view: Box::new(view),
                            role_continuity: Some(Box::new(role_continuity)),
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
        | Err(ProjectViewReadError::Other(message)) => Err(message),
    }
}

async fn read_identity(state: &AppState) -> Result<Option<ProjectViewIdentity>, String> {
    read_identity_at(state, &relay_api_base_url_with_override(state)).await
}

pub(crate) async fn read_identity_at(
    state: &AppState,
    api_base_url: &str,
) -> Result<Option<ProjectViewIdentity>, String> {
    crate::relay_admission::wait_for_rate_limit().await;
    let url = format!("{}/info", api_base_url.trim_end_matches('/'));
    let response = state
        .http_client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/nostr+json")
        .send()
        .await
        .map_err(|error| classify_request_error(&error))?;
    if !response.status().is_success() {
        return Err(relay_error_message(response).await);
    }
    let info: Nip11Document = parse_json_response(response).await?;
    let has_v2 = info
        .supported_extensions
        .iter()
        .any(|extension| extension == PROJECT_VIEW_V2_EXTENSION);
    let has_v1 = info
        .supported_extensions
        .iter()
        .any(|extension| extension == PROJECT_VIEW_V1_EXTENSION);
    let schema = if has_v2 {
        ProjectViewSchema::V2
    } else if has_v1 {
        ProjectViewSchema::V1
    } else {
        return Ok(None);
    };

    let relay_self = info.relay_self.ok_or_else(|| {
        integrity_error("NIP-11 advertises Project View without a Relay `self` key")
    })?;
    let relay_pubkey = PublicKey::from_hex(&relay_self)
        .map_err(|error| integrity_error(format!("invalid NIP-11 Relay `self`: {error}")))?;
    if relay_pubkey.to_hex() != relay_self {
        return Err(integrity_error(
            "NIP-11 Relay `self` is not canonical lowercase hex",
        ));
    }
    Ok(Some(ProjectViewIdentity {
        relay_pubkey,
        schema,
    }))
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
    let entity_events = query_project_view(
        state,
        &[json!({
            "kinds": [KIND_PROJECT_VIEW_OBJECT],
            "authors": [identity.relay_pubkey.to_hex()],
            "#t": ["buzz-project-view-v2-entity"],
        })],
    )
    .await?;

    let mut event_ids = HashSet::with_capacity(ordinary_events.len() + entity_events.len());
    let mut object_ids = HashSet::new();
    let mut entries = Vec::new();
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
            entries.push(ProjectViewEntry::Active((**object).clone()));
        }
        object_projections.push(projection);
    }

    let mut roles = Vec::new();
    let mut proposals = Vec::new();
    let mut assignments = Vec::new();
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
            RoleContinuityChange::Handoff(handoff) => handoffs.push(handoff.clone()),
        }
        entity_projections.push(projection);
    }
    let membership = read_v2_membership(state, identity, &meta).await?;
    validate_v2_counts_and_membership(
        &meta,
        &roles,
        &proposals,
        &assignments,
        &handoffs,
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
    let verified = VerifiedRoleBriefSnapshot::new(
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
            handoffs,
            members,
            briefs,
        },
    }))
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

fn validate_v2_counts_and_membership(
    meta: &V2MetaProjection,
    roles: &[RoleDefinition],
    proposals: &[RoleAssignmentProposal],
    assignments: &[RoleAssignment],
    handoffs: &[RoleHandoff],
    membership: &V2MembershipProjection,
) -> ProjectViewReadResult<()> {
    let open_proposals = proposals
        .iter()
        .filter(|proposal| proposal.status == ProposalStatus::Open)
        .count();
    let active_assignments = assignments
        .iter()
        .filter(|assignment| assignment.is_active())
        .count();
    if usize::try_from(meta.entity_counts.open_proposals).ok() != Some(open_proposals)
        || usize::try_from(meta.entity_counts.active_assignments).ok() != Some(active_assignments)
        || usize::try_from(meta.entity_counts.handoffs).ok() != Some(handoffs.len())
    {
        return Err(integrity_read_error(
            "v2 metadata counts disagree with verified Role heads",
        ));
    }
    let roles_by_id = roles
        .iter()
        .map(|role| (role.role_id, role))
        .collect::<BTreeMap<_, _>>();
    let members = membership
        .members
        .iter()
        .map(|member| (member.pubkey, member.role))
        .collect::<BTreeMap<_, _>>();
    let mut assigned_members = HashSet::new();
    for assignment in assignments
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
            && !assignments.iter().any(|assignment| {
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
