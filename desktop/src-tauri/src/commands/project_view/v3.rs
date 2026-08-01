//! Strict schema-v3 Project View snapshot reader.

use std::collections::{BTreeSet, HashSet};
use std::time::Duration;

use buzz_core_pkg::kind::{
    KIND_NIP43_MEMBERSHIP_LIST, KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_OBJECT,
};
use buzz_project_view_pkg::v2::{
    RoleAssignment, RoleAssignmentProposal, RoleCheckpoint, RoleHandoff, WorkCommitment,
};
use buzz_project_view_pkg::v3::{ProjectViewObjectV3, RoleDefinitionV3};
use buzz_project_view_pkg::ProjectViewObjectType;
use buzz_sdk_pkg::project_view_v2::{
    parse_membership_projection as parse_v2_membership_projection, V2MembershipProjection,
};
use buzz_sdk_pkg::project_view_v3::{
    parse_entity_projection, parse_meta_projection, parse_project_object_projection,
    V3MetaProjection, V3ProjectedObject,
};
use buzz_sdk_pkg::role_brief_v3::{RoleBriefV3, VerifiedRoleBriefSnapshotV3};
use chrono::Utc;
use nostr::Event;
use serde::Serialize;
use serde_json::json;

use crate::app_state::AppState;

use super::{
    conflict_error, integrity_read_error, query_project_view, ProjectViewIdentity,
    ProjectViewMembershipMember, ProjectViewReadError, ProjectViewReadResult,
    ProjectViewWorkResponsibility, SNAPSHOT_MAX_ATTEMPTS, SNAPSHOT_PAGE_SIZE,
};

pub(super) struct V3ProjectSnapshot {
    pub(super) meta: V3MetaProjection,
    pub(super) objects: Vec<ProjectViewObjectV3>,
    pub(super) role_continuity: ProjectViewRoleContinuityV3,
}

/// Verified Role continuity state returned beside a schema-v3 Project View.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectViewRoleContinuityV3 {
    roles: Vec<RoleDefinitionV3>,
    proposals: Vec<RoleAssignmentProposal>,
    assignments: Vec<RoleAssignment>,
    commitments: Vec<WorkCommitment>,
    work_responsibilities: Vec<ProjectViewWorkResponsibility>,
    checkpoints: Vec<RoleCheckpoint>,
    handoffs: Vec<RoleHandoff>,
    members: Vec<ProjectViewMembershipMember>,
    briefs: Vec<RoleBriefV3>,
}

pub(super) async fn fetch_consistent_v3_snapshot(
    state: &AppState,
    identity: ProjectViewIdentity,
) -> ProjectViewReadResult<Option<V3ProjectSnapshot>> {
    for attempt in 0..SNAPSHOT_MAX_ATTEMPTS {
        match fetch_v3_snapshot_once(state, identity).await {
            Ok(snapshot) => return Ok(snapshot),
            Err(ProjectViewReadError::Conflict(_)) if attempt + 1 < SNAPSHOT_MAX_ATTEMPTS => {
                let backoff_ms = 25_u64 << attempt;
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            }
            Err(error) => return Err(error),
        }
    }
    Err(conflict_error(
        "Project View v3 changed during every bounded snapshot attempt",
    ))
}

async fn fetch_v3_snapshot_once(
    state: &AppState,
    identity: ProjectViewIdentity,
) -> ProjectViewReadResult<Option<V3ProjectSnapshot>> {
    let Some(meta) = read_v3_meta(state, identity).await? else {
        return Ok(None);
    };
    let ordinary_events = query_project_view(
        state,
        &[json!({
            "kinds": [KIND_PROJECT_VIEW_OBJECT],
            "authors": [identity.relay_pubkey.to_hex()],
            "#t": ["buzz-project-view-v3-object"],
        })],
    )
    .await?;
    let entity_events = query_v3_current_entities(state, identity, &meta).await?;

    let object_projections = ordinary_events
        .iter()
        .map(|event| {
            parse_project_object_projection(event, &identity.relay_pubkey, meta.project_id)
                .map_err(|error| integrity_read_error(error.to_string()))
        })
        .collect::<ProjectViewReadResult<Vec<_>>>()?;
    let mut work_responsibilities = object_projections
        .iter()
        .filter_map(|projection| match &projection.object {
            V3ProjectedObject::Active(object)
                if object.object_type == ProjectViewObjectType::Work =>
            {
                projection
                    .responsible_role_id
                    .map(|role_id| ProjectViewWorkResponsibility {
                        work_id: object.id,
                        role_id,
                    })
            }
            V3ProjectedObject::Active(_) | V3ProjectedObject::Tombstone(_) => None,
        })
        .collect::<Vec<_>>();
    let entity_projections = entity_events
        .iter()
        .map(|event| {
            parse_entity_projection(event, &identity.relay_pubkey, meta.project_id)
                .map_err(|error| integrity_read_error(error.to_string()))
        })
        .collect::<ProjectViewReadResult<Vec<_>>>()?;
    let membership = read_v3_membership(state, identity, &meta).await?;

    let final_meta = read_v3_meta(state, identity)
        .await?
        .ok_or_else(|| conflict_error("Project View v3 metadata disappeared"))?;
    if final_meta.event_id != meta.event_id {
        return Err(conflict_error(
            "Project View v3 changed while assembling the snapshot",
        ));
    }
    let verified = VerifiedRoleBriefSnapshotV3::new_with_partial_history(
        meta.clone(),
        membership,
        object_projections,
        entity_projections,
    )
    .map_err(|error| integrity_read_error(error.to_string()))?;

    let brief_members = verified
        .assignments()
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
    let mut objects = verified
        .state()
        .active_objects()
        .cloned()
        .collect::<Vec<_>>();
    objects.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then(left.id.cmp(&right.id))
    });
    let mut roles = verified.roles().cloned().collect::<Vec<_>>();
    let mut proposals = verified.proposals().cloned().collect::<Vec<_>>();
    let mut assignments = verified.assignments().cloned().collect::<Vec<_>>();
    let mut commitments = verified.commitments().cloned().collect::<Vec<_>>();
    let mut checkpoints = verified.checkpoints().cloned().collect::<Vec<_>>();
    let mut handoffs = verified.handoffs().cloned().collect::<Vec<_>>();
    roles.sort_by_key(|role| role.role_id);
    proposals.sort_by_key(|proposal| proposal.proposal_id);
    assignments.sort_by_key(|assignment| assignment.assignment_id);
    commitments.sort_by_key(|commitment| commitment.commitment_id);
    work_responsibilities.sort_by_key(|responsibility| responsibility.work_id);
    checkpoints.sort_by_key(|checkpoint| checkpoint.checkpoint_id);
    handoffs.sort_by_key(|handoff| handoff.handoff_id);
    let members = verified
        .membership()
        .members
        .iter()
        .map(|member| ProjectViewMembershipMember {
            pubkey: member.pubkey.to_hex(),
            role: member.role,
        })
        .collect();
    Ok(Some(V3ProjectSnapshot {
        meta,
        objects,
        role_continuity: ProjectViewRoleContinuityV3 {
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

async fn query_v3_current_entities(
    state: &AppState,
    identity: ProjectViewIdentity,
    meta: &V3MetaProjection,
) -> ProjectViewReadResult<Vec<Event>> {
    let mut events = Vec::new();
    let mut event_ids = HashSet::new();
    let mut after: Option<serde_json::Value> = None;
    loop {
        let mut extension = json!({
            "scope": "v3_current_entities",
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
                "#t": ["buzz-project-view-v3-entity"],
                "limit": SNAPSHOT_PAGE_SIZE,
                "buzz_project_view": extension,
            })],
        )
        .await?;
        if page.len() > SNAPSHOT_PAGE_SIZE {
            return Err(integrity_read_error(
                "v3 current-entity page exceeded its requested limit",
            ));
        }
        let page_len = page.len();
        for event in page {
            if !event_ids.insert(event.id) {
                return Err(integrity_read_error(
                    "v3 current-entity pages contain a duplicate event",
                ));
            }
            let projection =
                parse_entity_projection(&event, &identity.relay_pubkey, meta.project_id)
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

async fn read_v3_membership(
    state: &AppState,
    identity: ProjectViewIdentity,
    meta: &V3MetaProjection,
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
            "v3 metadata membership pointer did not resolve exactly once",
        ));
    };
    if event.id != meta.membership_snapshot_event_id {
        return Err(integrity_read_error(
            "v3 membership query returned an event other than the metadata pointer",
        ));
    }
    parse_v2_membership_projection(event, &identity.relay_pubkey)
        .map_err(|error| integrity_read_error(error.to_string()))
}

pub(super) async fn read_v3_meta(
    state: &AppState,
    identity: ProjectViewIdentity,
) -> ProjectViewReadResult<Option<V3MetaProjection>> {
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
            "v3 metadata query returned multiple current heads",
        )),
    }
}
