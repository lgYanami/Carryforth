//! `buzz roles` — verified Project View v2 Role continuity reads and writes.

use std::collections::{BTreeMap, HashSet};

use buzz_core::kind::{
    KIND_NIP43_MEMBERSHIP_LIST, KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_OBJECT,
};
use buzz_core::PublicKey;
use buzz_project_view::v2::{
    CommunityMemberRole, ProposalStatus, RoleAssignment, RoleAssignmentProposal, RoleCommand,
    RoleCommandRequest, RoleContinuityChange, RoleDefinition, RoleHandoff, RoleLevel,
};
use buzz_sdk::project_view_v2::{
    build_role_command, parse_entity_projection, parse_membership_projection,
    parse_meta_projection, V2MembershipProjection, V2MetaProjection,
};
use chrono::{Duration, Utc};
use nostr::Event;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::client::{normalize_write_response, BuzzClient};
use crate::error::CliError;
use crate::validate::sdk_err;
use crate::{OutputFormat, RoleAssignmentCmd, RoleProposalCmd, RoleProposalStatusArg, RolesCmd};

const PROJECT_VIEW_V2_EXTENSION: &str = "buzz-project-view-v2";
const SNAPSHOT_ATTEMPTS: usize = 3;

#[derive(Debug, Clone, Copy)]
struct RoleIdentity {
    relay_pubkey: PublicKey,
}

#[derive(Deserialize)]
struct Nip11Document {
    #[serde(default)]
    supported_extensions: Vec<String>,
    #[serde(rename = "self")]
    relay_self: Option<String>,
}

#[derive(Debug)]
struct RoleSnapshot {
    meta: V2MetaProjection,
    roles: Vec<RoleDefinition>,
    proposals: Vec<RoleAssignmentProposal>,
    assignments: Vec<RoleAssignment>,
    handoffs: Vec<RoleHandoff>,
}

#[derive(Serialize)]
struct RoleListItem<'a> {
    #[serde(flatten)]
    role: &'a RoleDefinition,
    vacant: bool,
    current_assignment: Option<&'a RoleAssignment>,
}

/// Dispatch one `buzz roles` command.
pub async fn dispatch(
    command: RolesCmd,
    client: &BuzzClient,
    format: &OutputFormat,
) -> Result<(), CliError> {
    match command {
        RolesCmd::List => list_roles(client, format).await,
        RolesCmd::Get { role } => get_role(client, role, format).await,
        RolesCmd::Current { member } => current_assignment(client, member.as_deref(), format).await,
        RolesCmd::Proposals { status } => list_proposals(client, status, format).await,
        RolesCmd::Request {
            role,
            expected_project_revision,
            expires_in_hours,
            reason,
            acting_assignment,
        } => {
            submit(
                client,
                RoleCommand::new(
                    expected_project_revision,
                    acting_assignment,
                    RoleCommandRequest::RequestRole {
                        proposal_id: Uuid::new_v4(),
                        role_id: role,
                        expires_at: Utc::now() + Duration::hours(i64::from(expires_in_hours)),
                        reason,
                    },
                ),
            )
            .await
        }
        RolesCmd::Offer {
            role,
            member,
            expected_project_revision,
            expires_in_hours,
            reason,
            acting_assignment,
        } => {
            submit(
                client,
                RoleCommand::new(
                    expected_project_revision,
                    acting_assignment,
                    RoleCommandRequest::OfferRole {
                        proposal_id: Uuid::new_v4(),
                        role_id: role,
                        candidate_pubkey: parse_pubkey(&member)?,
                        expires_at: Utc::now() + Duration::hours(i64::from(expires_in_hours)),
                        reason,
                    },
                ),
            )
            .await
        }
        RolesCmd::Proposal { command } => submit_proposal(client, command).await,
        RolesCmd::Assignment { command } => dispatch_assignment(client, command, format).await,
    }
}

async fn list_roles(client: &BuzzClient, format: &OutputFormat) -> Result<(), CliError> {
    let snapshot = read_snapshot(client).await?;
    let active = snapshot
        .assignments
        .iter()
        .filter(|assignment| assignment.is_active())
        .map(|assignment| (assignment.role_id, assignment))
        .collect::<BTreeMap<_, _>>();
    let mut roles = snapshot.roles.iter().collect::<Vec<_>>();
    roles.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then(left.role_id.cmp(&right.role_id))
    });
    let output = roles
        .into_iter()
        .map(|role| RoleListItem {
            role,
            vacant: !active.contains_key(&role.role_id),
            current_assignment: active.get(&role.role_id).copied(),
        })
        .collect::<Vec<_>>();
    print_json(
        &json!({
            "project_revision": snapshot.meta.project_revision,
            "roles": output,
        }),
        format,
    )
}

async fn get_role(
    client: &BuzzClient,
    role_id: Uuid,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let snapshot = read_snapshot(client).await?;
    let role = snapshot
        .roles
        .iter()
        .find(|role| role.role_id == role_id)
        .ok_or_else(|| CliError::NotFound(format!("Role {role_id} was not found")))?;
    let current_assignment = snapshot
        .assignments
        .iter()
        .find(|assignment| assignment.role_id == role_id && assignment.is_active());
    let history = snapshot
        .assignments
        .iter()
        .filter(|assignment| assignment.role_id == role_id)
        .collect::<Vec<_>>();
    let proposals = snapshot
        .proposals
        .iter()
        .filter(|proposal| proposal.role_id == role_id)
        .map(proposal_output)
        .collect::<Vec<_>>();
    let handoffs = snapshot
        .handoffs
        .iter()
        .filter(|handoff| handoff.role_id == role_id)
        .collect::<Vec<_>>();
    print_json(
        &json!({
            "project_revision": snapshot.meta.project_revision,
            "role": role,
            "vacant": current_assignment.is_none(),
            "current_assignment": current_assignment,
            "assignment_history": history,
            "proposals": proposals,
            "handoffs": handoffs,
        }),
        format,
    )
}

async fn current_assignment(
    client: &BuzzClient,
    member: Option<&str>,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let member = member
        .map(parse_pubkey)
        .transpose()?
        .unwrap_or_else(|| client.public_key());
    let snapshot = read_snapshot(client).await?;
    let assignment = snapshot
        .assignments
        .iter()
        .find(|assignment| assignment.member_pubkey == member && assignment.is_active());
    let role = assignment.and_then(|assignment| {
        snapshot
            .roles
            .iter()
            .find(|role| role.role_id == assignment.role_id)
    });
    print_json(
        &json!({
            "project_revision": snapshot.meta.project_revision,
            "member_pubkey": member,
            "assigned": assignment.is_some(),
            "assignment": assignment,
            "role": role,
        }),
        format,
    )
}

async fn list_proposals(
    client: &BuzzClient,
    status: Option<RoleProposalStatusArg>,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let snapshot = read_snapshot(client).await?;
    let expected_status = status.map(proposal_status);
    let proposals = snapshot
        .proposals
        .iter()
        .filter(|proposal| {
            expected_status.is_none_or(|status| proposal.effective_status(Utc::now()) == status)
        })
        .map(proposal_output)
        .collect::<Vec<_>>();
    print_json(
        &json!({
            "project_revision": snapshot.meta.project_revision,
            "proposals": proposals,
        }),
        format,
    )
}

async fn submit_proposal(client: &BuzzClient, command: RoleProposalCmd) -> Result<(), CliError> {
    let (expected, acting, request) = match command {
        RoleProposalCmd::Accept {
            proposal,
            expected_project_revision,
            acting_assignment,
        } => (
            expected_project_revision,
            acting_assignment,
            RoleCommandRequest::AcceptProposal {
                proposal_id: proposal,
            },
        ),
        RoleProposalCmd::Reject {
            proposal,
            expected_project_revision,
            reason,
            acting_assignment,
        } => (
            expected_project_revision,
            acting_assignment,
            RoleCommandRequest::RejectProposal {
                proposal_id: proposal,
                reason,
            },
        ),
        RoleProposalCmd::Withdraw {
            proposal,
            expected_project_revision,
            reason,
            acting_assignment,
        } => (
            expected_project_revision,
            acting_assignment,
            RoleCommandRequest::WithdrawProposal {
                proposal_id: proposal,
                reason,
            },
        ),
        RoleProposalCmd::Authorize {
            proposal,
            expected_project_revision,
            acting_assignment,
        } => (
            expected_project_revision,
            acting_assignment,
            RoleCommandRequest::AuthorizeProposal {
                proposal_id: proposal,
            },
        ),
        RoleProposalCmd::Expire {
            proposal,
            expected_project_revision,
            acting_assignment,
        } => (
            expected_project_revision,
            acting_assignment,
            RoleCommandRequest::ExpireProposal {
                proposal_id: proposal,
            },
        ),
    };
    submit(client, RoleCommand::new(expected, acting, request)).await
}

async fn dispatch_assignment(
    client: &BuzzClient,
    command: RoleAssignmentCmd,
    format: &OutputFormat,
) -> Result<(), CliError> {
    match command {
        RoleAssignmentCmd::List {
            role,
            member,
            include_ended,
        } => {
            let member = member.as_deref().map(parse_pubkey).transpose()?;
            let snapshot = read_snapshot(client).await?;
            let assignments = snapshot
                .assignments
                .iter()
                .filter(|assignment| role.is_none_or(|role| assignment.role_id == role))
                .filter(|assignment| member.is_none_or(|member| assignment.member_pubkey == member))
                .filter(|assignment| include_ended || assignment.is_active())
                .collect::<Vec<_>>();
            print_json(
                &json!({
                    "project_revision": snapshot.meta.project_revision,
                    "assignments": assignments,
                }),
                format,
            )
        }
        RoleAssignmentCmd::Get { assignment } => {
            let snapshot = read_snapshot(client).await?;
            let assignment = snapshot
                .assignments
                .iter()
                .find(|candidate| candidate.assignment_id == assignment)
                .ok_or_else(|| {
                    CliError::NotFound(format!("Assignment {assignment} was not found"))
                })?;
            let role = snapshot
                .roles
                .iter()
                .find(|role| role.role_id == assignment.role_id);
            print_json(
                &json!({
                    "project_revision": snapshot.meta.project_revision,
                    "assignment": assignment,
                    "role": role,
                }),
                format,
            )
        }
        RoleAssignmentCmd::End {
            assignment,
            expected_project_revision,
            reason,
            acting_assignment,
        } => {
            submit(
                client,
                RoleCommand::new(
                    expected_project_revision,
                    acting_assignment,
                    RoleCommandRequest::EndAssignment {
                        assignment_id: assignment,
                        reason,
                    },
                ),
            )
            .await
        }
        RoleAssignmentCmd::RequestReplacement {
            assignment,
            expected_project_revision,
            reason,
        } => {
            submit(
                client,
                RoleCommand::new(
                    expected_project_revision,
                    Some(assignment),
                    RoleCommandRequest::RequestReplacement {
                        assignment_id: assignment,
                        reason,
                    },
                ),
            )
            .await
        }
        RoleAssignmentCmd::ReportUnableToContinue {
            assignment,
            expected_project_revision,
            reason,
        } => {
            submit(
                client,
                RoleCommand::new(
                    expected_project_revision,
                    Some(assignment),
                    RoleCommandRequest::ReportUnableToContinue {
                        assignment_id: assignment,
                        reason,
                    },
                ),
            )
            .await
        }
    }
}

async fn submit(client: &BuzzClient, command: RoleCommand) -> Result<(), CliError> {
    require_identity(client).await?;
    let event = client.sign_event_exact(build_role_command(command).map_err(sdk_err)?)?;
    let response = client.submit_event(event).await?;
    println!("{}", normalize_write_response(&response));
    Ok(())
}

async fn read_snapshot(client: &BuzzClient) -> Result<RoleSnapshot, CliError> {
    let identity = require_identity(client).await?;
    for attempt in 0..SNAPSHOT_ATTEMPTS {
        let before = read_meta(client, identity).await?;
        let filter = json!({
            "kinds": [KIND_PROJECT_VIEW_OBJECT],
            "authors": [identity.relay_pubkey.to_hex()],
            "#t": ["buzz-project-view-v2-entity"],
        });
        let raw_events = client.query_all(filter).await?;
        let mut event_ids = HashSet::with_capacity(raw_events.len());
        let mut roles = Vec::new();
        let mut proposals = Vec::new();
        let mut assignments = Vec::new();
        let mut handoffs = Vec::new();
        for raw in raw_events {
            let event: Event = serde_json::from_value(raw)
                .map_err(|error| integrity_error(format!("invalid entity event: {error}")))?;
            if !event_ids.insert(event.id) {
                return Err(integrity_error("entity query returned a duplicate event"));
            }
            let projection =
                parse_entity_projection(&event, &identity.relay_pubkey, before.project_id)
                    .map_err(|error| integrity_error(error.to_string()))?;
            if projection.projection_generation != before.projection_generation
                || projection.project_revision > before.project_revision
            {
                return Err(CliError::Conflict(
                    "Role projection snapshot changed during read".to_owned(),
                ));
            }
            match projection.entity {
                RoleContinuityChange::Role(role) => roles.push(role),
                RoleContinuityChange::Proposal(proposal) => proposals.push(proposal),
                RoleContinuityChange::Assignment(assignment) => assignments.push(assignment),
                RoleContinuityChange::Handoff(handoff) => handoffs.push(handoff),
            }
        }
        let membership = read_membership(client, identity, &before).await?;
        let after = read_meta(client, identity).await?;
        if before.event_id != after.event_id {
            if attempt + 1 < SNAPSHOT_ATTEMPTS {
                continue;
            }
            return Err(CliError::Conflict(
                "Role projection changed during every bounded snapshot attempt".to_owned(),
            ));
        }
        let active_assignments = assignments
            .iter()
            .filter(|assignment| assignment.is_active())
            .count();
        let open_proposals = proposals
            .iter()
            .filter(|proposal| proposal.status == ProposalStatus::Open)
            .count();
        if usize::try_from(before.entity_counts.active_assignments).ok() != Some(active_assignments)
            || usize::try_from(before.entity_counts.open_proposals).ok() != Some(open_proposals)
            || usize::try_from(before.entity_counts.handoffs).ok() != Some(handoffs.len())
        {
            return Err(integrity_error(
                "v2 metadata counts disagree with verified entity heads",
            ));
        }
        validate_membership_assignment_consistency(&roles, &assignments, &membership)?;
        roles.sort_by_key(|role| role.role_id);
        proposals.sort_by_key(|proposal| proposal.proposal_id);
        assignments.sort_by_key(|assignment| assignment.assignment_id);
        handoffs.sort_by_key(|handoff| handoff.handoff_id);
        return Ok(RoleSnapshot {
            meta: before,
            roles,
            proposals,
            assignments,
            handoffs,
        });
    }
    Err(CliError::Conflict(
        "Role projection snapshot could not be stabilized".to_owned(),
    ))
}

async fn read_membership(
    client: &BuzzClient,
    identity: RoleIdentity,
    meta: &V2MetaProjection,
) -> Result<V2MembershipProjection, CliError> {
    let filter = json!({
        "ids": [meta.membership_snapshot_event_id.to_hex()],
        "kinds": [KIND_NIP43_MEMBERSHIP_LIST],
        "authors": [identity.relay_pubkey.to_hex()],
        "limit": 2,
    });
    let raw = client.query(&filter).await?;
    let values: Vec<Value> = serde_json::from_str(&raw)
        .map_err(|error| integrity_error(format!("invalid membership response: {error}")))?;
    if values.len() != 1 {
        return Err(integrity_error(
            "metadata membership pointer did not resolve to exactly one live snapshot",
        ));
    }
    let event: Event = serde_json::from_value(
        values
            .into_iter()
            .next()
            .ok_or_else(|| integrity_error("membership response is empty"))?,
    )
    .map_err(|error| integrity_error(format!("invalid membership event: {error}")))?;
    if event.id != meta.membership_snapshot_event_id {
        return Err(integrity_error(
            "membership query returned an event other than the metadata pointer",
        ));
    }
    parse_membership_projection(&event, &identity.relay_pubkey)
        .map_err(|error| integrity_error(error.to_string()))
}

fn validate_membership_assignment_consistency(
    roles: &[RoleDefinition],
    assignments: &[RoleAssignment],
    membership: &V2MembershipProjection,
) -> Result<(), CliError> {
    let roles = roles
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
            return Err(integrity_error(
                "one Member has multiple active Assignment heads",
            ));
        }
        let role = roles.get(&assignment.role_id).ok_or_else(|| {
            integrity_error("an active Assignment references a missing Role head")
        })?;
        let community_role = members.get(&assignment.member_pubkey).ok_or_else(|| {
            integrity_error("an active Assignment assignee is absent from membership")
        })?;
        let expected = match role.level {
            RoleLevel::Admin => CommunityMemberRole::Admin,
            RoleLevel::Member => CommunityMemberRole::Member,
        };
        if *community_role != CommunityMemberRole::Owner && *community_role != expected {
            return Err(integrity_error(
                "an active Assignment disagrees with the assignee's Community role",
            ));
        }
    }
    for (pubkey, role) in members {
        if role == CommunityMemberRole::Admin
            && !assignments.iter().any(|assignment| {
                assignment.is_active()
                    && assignment.member_pubkey == pubkey
                    && roles
                        .get(&assignment.role_id)
                        .is_some_and(|role| role.level == RoleLevel::Admin)
            })
        {
            return Err(integrity_error(
                "a non-owner Community admin has no active Leader Assignment",
            ));
        }
    }
    Ok(())
}

async fn read_meta(
    client: &BuzzClient,
    identity: RoleIdentity,
) -> Result<V2MetaProjection, CliError> {
    let filter = json!({
        "kinds": [KIND_PROJECT_VIEW_META],
        "authors": [identity.relay_pubkey.to_hex()],
        "limit": 2,
    });
    let raw = client.query(&filter).await?;
    let values: Vec<Value> = serde_json::from_str(&raw)
        .map_err(|error| integrity_error(format!("invalid metadata response: {error}")))?;
    if values.len() != 1 {
        return Err(integrity_error(
            "v2 metadata query did not return exactly one current head",
        ));
    }
    let event: Event = serde_json::from_value(
        values
            .into_iter()
            .next()
            .ok_or_else(|| integrity_error("v2 metadata response is empty"))?,
    )
    .map_err(|error| integrity_error(format!("invalid metadata event: {error}")))?;
    parse_meta_projection(&event, &identity.relay_pubkey)
        .map_err(|error| integrity_error(error.to_string()))
}

async fn require_identity(client: &BuzzClient) -> Result<RoleIdentity, CliError> {
    let raw = client.get_public("/info").await?;
    let info: Nip11Document = serde_json::from_str(&raw)
        .map_err(|error| integrity_error(format!("invalid NIP-11 document: {error}")))?;
    if !info
        .supported_extensions
        .iter()
        .any(|extension| extension == PROJECT_VIEW_V2_EXTENSION)
    {
        return Err(CliError::Other(format!(
            "unsupported: relay does not advertise {PROJECT_VIEW_V2_EXTENSION}"
        )));
    }
    let relay_self = info.relay_self.ok_or_else(|| {
        integrity_error("NIP-11 advertises Project View v2 without a relay `self` key")
    })?;
    let relay_pubkey = parse_pubkey(&relay_self)?;
    if relay_pubkey.to_hex() != relay_self {
        return Err(integrity_error(
            "NIP-11 relay `self` is not canonical lowercase hex",
        ));
    }
    Ok(RoleIdentity { relay_pubkey })
}

fn parse_pubkey(value: &str) -> Result<PublicKey, CliError> {
    PublicKey::parse(value)
        .map_err(|error| CliError::Usage(format!("invalid member public key: {error}")))
}

fn proposal_status(value: RoleProposalStatusArg) -> ProposalStatus {
    match value {
        RoleProposalStatusArg::Open => ProposalStatus::Open,
        RoleProposalStatusArg::Consumed => ProposalStatus::Consumed,
        RoleProposalStatusArg::Rejected => ProposalStatus::Rejected,
        RoleProposalStatusArg::Withdrawn => ProposalStatus::Withdrawn,
        RoleProposalStatusArg::Expired => ProposalStatus::Expired,
    }
}

fn proposal_output(proposal: &RoleAssignmentProposal) -> Value {
    let mut value = serde_json::to_value(proposal).unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "effective_status".to_owned(),
            serde_json::to_value(proposal.effective_status(Utc::now())).unwrap_or(Value::Null),
        );
    }
    value
}

fn print_json(value: &Value, format: &OutputFormat) -> Result<(), CliError> {
    let output = match format {
        OutputFormat::Json => serde_json::to_string_pretty(value),
        OutputFormat::Compact => serde_json::to_string(value),
    }
    .map_err(|error| CliError::Other(format!("serialize Role output: {error}")))?;
    println!("{output}");
    Ok(())
}

fn integrity_error(message: impl Into<String>) -> CliError {
    CliError::Other(format!(
        "Project View v2 integrity error: {}",
        message.into()
    ))
}
