//! `cf roles` — verified Project View v3 Role continuity reads and writes.

use std::collections::{BTreeMap, HashSet};

use buzz_core::PublicKey;
use buzz_project_view::v2::{
    HandoffCause, ProposalStatus, RoleActorIntent, RoleAssignment, RoleAssignmentProposal,
    RoleCheckpointContent, RoleCommandRequest, RoleContinuityEntity, RoleHandoffContent,
};
use buzz_project_view::v3::{RoleCommandV3, RoleDefinitionV3};
use buzz_sdk::project_view_v3::{
    build_role_command as build_v3_role_command, V3EntityChange, V3MetaProjection,
};
use buzz_sdk::role_brief_v3::render_role_brief_markdown_v3;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::client::{normalize_write_response, CarryforthClient};
use crate::commands::project_view_snapshot::{
    is_managed_runtime, read_identity, read_verified_v3_snapshot, ProjectViewIdentity,
    ProjectViewSchema,
};
use crate::commands::project_view_v3_context::resolve_v3_role_brief;
use crate::commands::project_view_v3_role_history::{
    read_v3_role_history_page, V3RoleHistoryRequest,
};
use crate::error::CliError;
use crate::validate::{read_file_or_stdin, sdk_err};
use crate::{
    OutputFormat, RoleAssignmentCmd, RoleCheckpointCmd, RoleHandoffCauseArg, RoleHandoffCmd,
    RoleProposalCmd, RoleProposalStatusArg, RoleWorkCmd, RolesCmd,
};

#[derive(Debug)]
struct RoleSnapshot {
    meta: V3MetaProjection,
    roles: Vec<RoleDefinitionV3>,
    assignments: Vec<RoleAssignment>,
}

#[derive(Debug, Clone, Copy, Default)]
struct ManagedRoleState {
    current_assignment: Option<Uuid>,
    actor_is_proposal_candidate: Option<bool>,
}

#[derive(Serialize)]
struct RoleListItem<'a> {
    #[serde(flatten)]
    role: &'a RoleDefinitionV3,
    vacant: bool,
    current_assignment: Option<&'a RoleAssignment>,
}

/// Dispatch one `cf roles` command.
pub async fn dispatch(
    command: RolesCmd,
    client: &CarryforthClient,
    format: &OutputFormat,
) -> Result<(), CliError> {
    match command {
        RolesCmd::List => list_roles(client, format).await,
        RolesCmd::Brief { member, markdown } => {
            show_brief(client, member.as_deref(), markdown, format).await
        }
        RolesCmd::Get { role } => get_role(client, role, format).await,
        RolesCmd::Current { member } => current_assignment(client, member.as_deref(), format).await,
        RolesCmd::Proposals {
            status,
            limit,
            before,
        } => list_proposals(client, status, limit, before.as_deref(), format).await,
        RolesCmd::Request {
            role,
            expected_project_revision,
            expires_in_hours,
            reason,
            acting_assignment,
        } => {
            submit(
                client,
                RoleCommandV3::new(
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
                RoleCommandV3::new(
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
        RolesCmd::Work { command } => dispatch_work(client, command).await,
        RolesCmd::Checkpoint { command } => dispatch_checkpoint(client, command, format).await,
        RolesCmd::Handoff { command } => dispatch_handoff(client, command, format).await,
    }
}

async fn show_brief(
    client: &CarryforthClient,
    member: Option<&str>,
    markdown: bool,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let member = member
        .map(parse_pubkey)
        .transpose()?
        .unwrap_or_else(|| client.public_key());
    let identity = require_role_identity(client).await?;
    let snapshot = read_verified_v3_snapshot(client, identity).await?;
    let brief = resolve_v3_role_brief(client, identity, &snapshot, member, Utc::now()).await?;
    if markdown {
        print!("{}", render_role_brief_markdown_v3(&brief));
        return Ok(());
    }
    print_json(
        &serde_json::to_value(brief)
            .map_err(|error| CliError::Other(format!("serialize Role Brief v3: {error}")))?,
        format,
    )
}

async fn list_roles(client: &CarryforthClient, format: &OutputFormat) -> Result<(), CliError> {
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
    client: &CarryforthClient,
    role_id: Uuid,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let identity = require_role_identity(client).await?;
    let snapshot = read_verified_v3_snapshot(client, identity).await?;
    let role = snapshot
        .roles()
        .find(|role| role.role_id == role_id)
        .ok_or_else(|| CliError::NotFound(format!("Role {role_id} was not found")))?;
    let current_assignment = snapshot
        .assignments()
        .find(|assignment| assignment.role_id == role_id && assignment.is_active());
    let history = read_complete_role_history(client, identity, snapshot.meta(), role_id).await?;
    let mut assignments = Vec::new();
    let mut proposals = Vec::new();
    let mut checkpoints = Vec::new();
    let mut handoffs = Vec::new();
    for change in history {
        match change {
            V3EntityChange::Proposal(proposal) => {
                proposals.push(proposal_output(&proposal));
            }
            V3EntityChange::Assignment(assignment) => assignments.push(assignment),
            V3EntityChange::Checkpoint(checkpoint) => checkpoints.push(checkpoint),
            V3EntityChange::Handoff(handoff) => handoffs.push(handoff),
            V3EntityChange::Role(_) | V3EntityChange::Commitment(_) => {
                return Err(integrity_error(
                    "Role history returned an entity outside the requested history set",
                ));
            }
        }
    }
    if current_assignment
        .is_some_and(|current| !assignments.iter().any(|historical| historical == current))
    {
        return Err(integrity_error(
            "current Assignment is missing from Role history",
        ));
    }
    print_json(
        &json!({
            "project_revision": snapshot.meta().project_revision,
            "role": role,
            "vacant": current_assignment.is_none(),
            "current_assignment": current_assignment,
            "assignment_history": assignments,
            "proposals": proposals,
            "checkpoints": checkpoints,
            "handoffs": handoffs,
        }),
        format,
    )
}

async fn read_complete_role_history(
    client: &CarryforthClient,
    identity: ProjectViewIdentity,
    meta: &V3MetaProjection,
    role_id: Uuid,
) -> Result<Vec<V3EntityChange>, CliError> {
    const PAGE_SIZE: u16 = 500;

    let entity_types = [
        RoleContinuityEntity::RoleAssignmentProposal,
        RoleContinuityEntity::RoleAssignment,
        RoleContinuityEntity::RoleCheckpoint,
        RoleContinuityEntity::RoleHandoff,
    ];
    let mut history = Vec::new();
    let mut event_ids = HashSet::new();
    let mut before = None;
    loop {
        let page = read_v3_role_history_page(
            client,
            identity,
            meta,
            V3RoleHistoryRequest {
                entity_types: &entity_types,
                role_id: Some(role_id),
                assignment_id: None,
                member_pubkey: None,
                limit: PAGE_SIZE,
                before: before.as_deref(),
            },
        )
        .await?;
        for projection in page.projections {
            if !event_ids.insert(projection.event_id) {
                return Err(integrity_error(
                    "Role history pages contain a duplicate event",
                ));
            }
            history.push(projection.entity);
        }
        let Some(next_before) = page.next_before else {
            return Ok(history);
        };
        if before.as_deref() == Some(next_before.as_str()) {
            return Err(integrity_error("Role history cursor did not advance"));
        }
        before = Some(next_before);
    }
}

async fn current_assignment(
    client: &CarryforthClient,
    member: Option<&str>,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let member = member
        .map(parse_pubkey)
        .transpose()?
        .unwrap_or_else(|| client.public_key());
    let identity = require_role_identity(client).await?;
    let snapshot = read_verified_v3_snapshot(client, identity).await?;
    let assignment = snapshot
        .assignments()
        .find(|assignment| assignment.member_pubkey == member && assignment.is_active());
    let role = assignment.and_then(|assignment| {
        snapshot
            .roles()
            .find(|role| role.role_id == assignment.role_id)
    });
    print_json(
        &json!({
            "project_view_schema_version": 3,
            "project_revision": snapshot.meta().project_revision,
            "member_pubkey": member,
            "assigned": assignment.is_some(),
            "assignment": assignment,
            "role": role,
        }),
        format,
    )
}

async fn list_proposals(
    client: &CarryforthClient,
    status: Option<RoleProposalStatusArg>,
    limit: u16,
    before: Option<&str>,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let identity = require_role_identity(client).await?;
    let snapshot = read_verified_v3_snapshot(client, identity).await?;
    let page = read_v3_role_history_page(
        client,
        identity,
        snapshot.meta(),
        V3RoleHistoryRequest {
            entity_types: &[RoleContinuityEntity::RoleAssignmentProposal],
            role_id: None,
            assignment_id: None,
            member_pubkey: None,
            limit,
            before,
        },
    )
    .await?;
    let expected_status = status.map(proposal_status);
    let proposals = page
        .projections
        .into_iter()
        .filter_map(|projection| match projection.entity {
            V3EntityChange::Proposal(proposal) => Some(proposal),
            _ => None,
        })
        .filter(|proposal| {
            expected_status.is_none_or(|status| proposal.effective_status(Utc::now()) == status)
        })
        .map(|proposal| proposal_output(&proposal))
        .collect::<Vec<_>>();
    print_json(
        &json!({
            "project_revision": snapshot.meta().project_revision,
            "proposals": proposals,
            "page": {
                "limit": limit,
                "has_more": page.next_before.is_some(),
                "next_before": page.next_before,
            },
        }),
        format,
    )
}

async fn submit_proposal(
    client: &CarryforthClient,
    command: RoleProposalCmd,
) -> Result<(), CliError> {
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
    submit(client, RoleCommandV3::new(expected, acting, request)).await
}

async fn dispatch_assignment(
    client: &CarryforthClient,
    command: RoleAssignmentCmd,
    format: &OutputFormat,
) -> Result<(), CliError> {
    match command {
        RoleAssignmentCmd::List {
            role,
            member,
            include_ended,
            limit,
            before,
        } => {
            let member = member.as_deref().map(parse_pubkey).transpose()?;
            let identity = require_role_identity(client).await?;
            let snapshot = read_verified_v3_snapshot(client, identity).await?;
            if !include_ended {
                if before.is_some() {
                    return Err(CliError::Usage(
                        "--before requires --include-ended for Assignment history".to_owned(),
                    ));
                }
                let mut assignments = snapshot
                    .assignments()
                    .filter(|assignment| assignment.is_active())
                    .filter(|assignment| role.is_none_or(|role| assignment.role_id == role))
                    .filter(|assignment| {
                        member.is_none_or(|member| assignment.member_pubkey == member)
                    })
                    .collect::<Vec<_>>();
                assignments.sort_by_key(|assignment| assignment.assignment_id);
                return print_json(
                    &json!({
                        "project_revision": snapshot.meta().project_revision,
                        "assignments": assignments,
                        "page": {
                            "limit": limit,
                            "has_more": false,
                            "next_before": Value::Null,
                        },
                    }),
                    format,
                );
            }
            let page = read_v3_role_history_page(
                client,
                identity,
                snapshot.meta(),
                V3RoleHistoryRequest {
                    entity_types: &[RoleContinuityEntity::RoleAssignment],
                    role_id: role,
                    assignment_id: None,
                    member_pubkey: member,
                    limit,
                    before: before.as_deref(),
                },
            )
            .await?;
            let assignments = page
                .projections
                .into_iter()
                .filter_map(|projection| match projection.entity {
                    V3EntityChange::Assignment(assignment) => Some(assignment),
                    _ => None,
                })
                .collect::<Vec<_>>();
            print_json(
                &json!({
                    "project_revision": snapshot.meta().project_revision,
                    "assignments": assignments,
                    "page": {
                        "limit": limit,
                        "has_more": page.next_before.is_some(),
                        "next_before": page.next_before,
                    },
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
                RoleCommandV3::new(
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
                RoleCommandV3::new(
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
                RoleCommandV3::new(
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

async fn dispatch_work(client: &CarryforthClient, command: RoleWorkCmd) -> Result<(), CliError> {
    let (expected, acting, request) = match command {
        RoleWorkCmd::Assign {
            work,
            role,
            expected_project_revision,
            acting_assignment,
        } => (
            expected_project_revision,
            acting_assignment,
            RoleCommandRequest::SetWorkResponsibility {
                work_id: work,
                responsible_role_id: Some(role),
            },
        ),
        RoleWorkCmd::Unassign {
            work,
            expected_project_revision,
            acting_assignment,
        } => (
            expected_project_revision,
            acting_assignment,
            RoleCommandRequest::SetWorkResponsibility {
                work_id: work,
                responsible_role_id: None,
            },
        ),
        RoleWorkCmd::Accept {
            work,
            expected_project_revision,
            acting_assignment,
        } => (
            expected_project_revision,
            acting_assignment,
            RoleCommandRequest::AcceptWork {
                commitment_id: Uuid::new_v4(),
                work_id: work,
            },
        ),
        RoleWorkCmd::Release {
            commitment,
            expected_project_revision,
            reason,
            acting_assignment,
        } => (
            expected_project_revision,
            acting_assignment,
            RoleCommandRequest::EndCommitment {
                commitment_id: commitment,
                reason,
            },
        ),
        RoleWorkCmd::Recommit {
            work,
            commitment,
            expected_project_revision,
            acting_assignment,
        } => (
            expected_project_revision,
            acting_assignment,
            RoleCommandRequest::ReplaceCommitment {
                commitment_id: Uuid::new_v4(),
                work_id: work,
                expected_commitment_id: commitment,
            },
        ),
    };
    submit(client, RoleCommandV3::new(expected, acting, request)).await
}

async fn dispatch_checkpoint(
    client: &CarryforthClient,
    command: RoleCheckpointCmd,
    format: &OutputFormat,
) -> Result<(), CliError> {
    match command {
        RoleCheckpointCmd::Append {
            input,
            expected_project_revision,
            based_on_project_revision,
            supersedes,
            acting_assignment,
        } => {
            let content: RoleCheckpointContent = serde_json::from_str(&read_file_or_stdin(&input)?)
                .map_err(|error| {
                    CliError::Usage(format!(
                        "invalid Role Checkpoint JSON in {input:?}: {error}"
                    ))
                })?;
            submit(
                client,
                RoleCommandV3::new(
                    expected_project_revision,
                    acting_assignment,
                    RoleCommandRequest::AppendCheckpoint {
                        checkpoint_id: Uuid::new_v4(),
                        based_on_project_revision: based_on_project_revision
                            .unwrap_or(expected_project_revision),
                        content,
                        supersedes_checkpoint_id: supersedes,
                    },
                ),
            )
            .await
        }
        RoleCheckpointCmd::List {
            role,
            assignment,
            limit,
            before,
        } => {
            let identity = require_role_identity(client).await?;
            let snapshot = read_verified_v3_snapshot(client, identity).await?;
            let page = read_v3_role_history_page(
                client,
                identity,
                snapshot.meta(),
                V3RoleHistoryRequest {
                    entity_types: &[RoleContinuityEntity::RoleCheckpoint],
                    role_id: role,
                    assignment_id: assignment,
                    member_pubkey: None,
                    limit,
                    before: before.as_deref(),
                },
            )
            .await?;
            let checkpoints = page
                .projections
                .into_iter()
                .filter_map(|projection| match projection.entity {
                    V3EntityChange::Checkpoint(checkpoint) => Some(checkpoint),
                    _ => None,
                })
                .collect::<Vec<_>>();
            print_json(
                &json!({
                    "project_revision": snapshot.meta().project_revision,
                    "checkpoints": checkpoints,
                    "page": {
                        "limit": limit,
                        "has_more": page.next_before.is_some(),
                        "next_before": page.next_before,
                    },
                }),
                format,
            )
        }
    }
}

async fn dispatch_handoff(
    client: &CarryforthClient,
    command: RoleHandoffCmd,
    format: &OutputFormat,
) -> Result<(), CliError> {
    match command {
        RoleHandoffCmd::Append {
            input,
            expected_project_revision,
            to_assignment,
            checkpoint,
            cause,
            acting_assignment,
        } => {
            let content: RoleHandoffContent = serde_json::from_str(&read_file_or_stdin(&input)?)
                .map_err(|error| {
                    CliError::Usage(format!("invalid Role Handoff JSON in {input:?}: {error}"))
                })?;
            submit(
                client,
                RoleCommandV3::new(
                    expected_project_revision,
                    acting_assignment,
                    RoleCommandRequest::AppendHandoff {
                        handoff_id: Uuid::new_v4(),
                        to_assignment_id: to_assignment,
                        checkpoint_id: checkpoint,
                        content,
                        cause: match cause {
                            RoleHandoffCauseArg::Planned => HandoffCause::Planned,
                            RoleHandoffCauseArg::Other => HandoffCause::Other,
                        },
                    },
                ),
            )
            .await
        }
        RoleHandoffCmd::List {
            role,
            assignment,
            limit,
            before,
        } => {
            let identity = require_role_identity(client).await?;
            let snapshot = read_verified_v3_snapshot(client, identity).await?;
            let page = read_v3_role_history_page(
                client,
                identity,
                snapshot.meta(),
                V3RoleHistoryRequest {
                    entity_types: &[RoleContinuityEntity::RoleHandoff],
                    role_id: role,
                    assignment_id: assignment,
                    member_pubkey: None,
                    limit,
                    before: before.as_deref(),
                },
            )
            .await?;
            let handoffs = page
                .projections
                .into_iter()
                .filter_map(|projection| match projection.entity {
                    V3EntityChange::Handoff(handoff) => Some(handoff),
                    _ => None,
                })
                .collect::<Vec<_>>();
            print_json(
                &json!({
                    "project_revision": snapshot.meta().project_revision,
                    "handoffs": handoffs,
                    "page": {
                        "limit": limit,
                        "has_more": page.next_before.is_some(),
                        "next_before": page.next_before,
                    },
                }),
                format,
            )
        }
    }
}

async fn submit(client: &CarryforthClient, mut command: RoleCommandV3) -> Result<(), CliError> {
    let identity = require_role_identity(client).await?;
    if is_managed_runtime() {
        let actor_intent = command.request.actor_intent();
        let proposal_id = match &command.request {
            RoleCommandRequest::RejectProposal { proposal_id, .. } => Some(*proposal_id),
            _ => None,
        };
        let supplied_assignment = command.acting_assignment_id;
        let needs_state =
            supplied_assignment.is_some() || actor_intent != RoleActorIntent::CommunityIdentity;
        let managed_state = if needs_state {
            read_managed_role_state(client, identity, proposal_id).await?
        } else {
            ManagedRoleState::default()
        };
        if command
            .acting_assignment_id
            .is_some_and(|provided| Some(provided) != managed_state.current_assignment)
        {
            return Err(CliError::Conflict(
                "provided acting Assignment is not the verified current Assignment".to_owned(),
            ));
        }
        match &command.request {
            RoleCommandRequest::RequestReplacement { assignment_id, .. }
            | RoleCommandRequest::ReportUnableToContinue { assignment_id, .. }
                if Some(*assignment_id) != managed_state.current_assignment =>
            {
                return Err(CliError::Conflict(
                    "target Assignment is not the verified current Assignment".to_owned(),
                ));
            }
            _ => {}
        }
        if supplied_assignment.is_some()
            || managed_command_requires_assignment(
                actor_intent,
                managed_state.actor_is_proposal_candidate,
            )
        {
            let assignment_id = managed_state.current_assignment.ok_or_else(|| {
                CliError::Auth(
                    "assignment_unavailable: managed Role action has no active Assignment"
                        .to_owned(),
                )
            })?;
            command.acting_assignment_id = Some(assignment_id);
            command.runtime_fence = None;
        } else {
            command.acting_assignment_id = None;
            command.runtime_fence = None;
        }
    }
    let responsibility = match &command.request {
        RoleCommandRequest::SetWorkResponsibility {
            work_id,
            responsible_role_id,
        } => Some((*work_id, *responsible_role_id)),
        _ => None,
    };
    let event = client.sign_event_exact(build_v3_role_command(command).map_err(sdk_err)?)?;
    let response = client.submit_event(event.clone()).await?;
    if let Some((work_id, responsible_role_id)) = responsibility {
        return print_responsibility_write(
            client,
            identity,
            &event,
            &response,
            work_id,
            responsible_role_id,
        )
        .await;
    }
    println!("{}", normalize_write_response(&response));
    Ok(())
}

async fn read_managed_role_state(
    client: &CarryforthClient,
    identity: ProjectViewIdentity,
    proposal_id: Option<Uuid>,
) -> Result<ManagedRoleState, CliError> {
    let actor = client.public_key();
    let snapshot = read_verified_v3_snapshot(client, identity)
        .await
        .map_err(assignment_read_error)?;
    Ok(managed_role_state(
        actor,
        proposal_id,
        snapshot.assignments(),
        snapshot.proposals(),
    ))
}

fn managed_role_state<'a>(
    actor: PublicKey,
    proposal_id: Option<Uuid>,
    assignments: impl Iterator<Item = &'a RoleAssignment>,
    mut proposals: impl Iterator<Item = &'a RoleAssignmentProposal>,
) -> ManagedRoleState {
    ManagedRoleState {
        current_assignment: assignments
            .filter(|assignment| assignment.member_pubkey == actor && assignment.is_active())
            .map(|assignment| assignment.assignment_id)
            .next(),
        actor_is_proposal_candidate: proposal_id.and_then(|proposal_id| {
            proposals
                .find(|proposal| proposal.proposal_id == proposal_id)
                .map(|proposal| proposal.candidate_pubkey == actor)
        }),
    }
}

const fn managed_command_requires_assignment(
    actor_intent: RoleActorIntent,
    actor_is_proposal_candidate: Option<bool>,
) -> bool {
    match actor_intent {
        RoleActorIntent::CommunityIdentity => false,
        RoleActorIntent::CandidateOrGovernor => {
            matches!(actor_is_proposal_candidate, Some(false))
        }
        RoleActorIntent::Governor | RoleActorIntent::RoleBearing => true,
    }
}

async fn require_role_identity(client: &CarryforthClient) -> Result<ProjectViewIdentity, CliError> {
    match read_identity(client).await? {
        Some(identity) if identity.schema == ProjectViewSchema::V3 => Ok(identity),
        Some(_) => Err(CliError::Other(
            "migration_required: Role continuity requires Project View schema v3".to_owned(),
        )),
        None => Err(CliError::Other(
            "unsupported: relay does not advertise Project View v3".to_owned(),
        )),
    }
}

fn assignment_read_error(error: CliError) -> CliError {
    CliError::Other(format!(
        "assignment_unavailable: current Assignment could not be verified: {error}"
    ))
}

#[derive(Deserialize)]
struct RoleWriteResponse {
    event_id: String,
    accepted: bool,
    message: String,
}

#[derive(Deserialize)]
struct ResponsibilityReceipt {
    #[serde(default)]
    schema_version: Option<u16>,
    project_revision: u64,
    operation: String,
    #[serde(alias = "work_objects")]
    changed_objects: Vec<ResponsibilityChangedObject>,
}

#[derive(Deserialize)]
struct ResponsibilityChangedObject {
    #[serde(default)]
    object_type: Option<String>,
    object_id: Uuid,
    object_revision: u64,
    responsible_role_id: Option<Uuid>,
}

async fn print_responsibility_write(
    client: &CarryforthClient,
    identity: ProjectViewIdentity,
    event: &nostr::Event,
    raw: &str,
    work_id: Uuid,
    responsible_role_id: Option<Uuid>,
) -> Result<(), CliError> {
    let response: RoleWriteResponse = serde_json::from_str(raw)
        .map_err(|error| integrity_error(format!("invalid Role write response: {error}")))?;
    if !response.accepted || response.event_id != event.id.to_hex() {
        return Err(integrity_error(
            "Role write response does not confirm the submitted event",
        ));
    }
    let receipt: ResponsibilityReceipt = serde_json::from_str(
        response
            .message
            .strip_prefix("response:")
            .ok_or_else(|| integrity_error("Role write response has no canonical receipt"))?,
    )
    .map_err(|error| integrity_error(format!("invalid responsibility receipt: {error}")))?;
    let changed = receipt
        .changed_objects
        .iter()
        .find(|changed| changed.object_id == work_id)
        .ok_or_else(|| {
            integrity_error("responsibility receipt does not contain the target Work")
        })?;
    if receipt.project_revision == 0
        || receipt.operation != "set_work_responsibility"
        || receipt.changed_objects.len() != 1
        || changed.responsible_role_id != responsible_role_id
        || changed.object_revision == 0
        || receipt.schema_version != Some(3)
        || changed
            .object_type
            .as_deref()
            .is_some_and(|object_type| object_type != "work")
    {
        return Err(integrity_error(
            "responsibility receipt differs from the submitted operation",
        ));
    }
    let snapshot = read_verified_v3_snapshot(client, identity).await?;
    let work = snapshot.active_object(work_id).ok_or_else(|| {
        integrity_error("accepted responsibility has no verified active v3 Work projection")
    })?;
    if snapshot.meta().project_revision < receipt.project_revision
        || work.responsible_role_id != responsible_role_id
        || work.source.change_id.to_hex() != event.id.to_hex()
    {
        return Err(integrity_error(
            "verified v3 Work projection does not confirm the responsibility receipt",
        ));
    }
    let projection_source = work.source;
    println!(
        "{}",
        json!({
            "event_id": event.id.to_hex(),
            "accepted": true,
            "operation": receipt.operation,
            "work_id": work_id,
            "object_revision": changed.object_revision,
            "responsible_role_id": responsible_role_id,
            "accepted_project_revision": receipt.project_revision,
            "projection_source": projection_source,
        })
    );
    Ok(())
}

async fn read_snapshot(client: &CarryforthClient) -> Result<RoleSnapshot, CliError> {
    let identity = require_role_identity(client).await?;
    let snapshot = read_verified_v3_snapshot(client, identity).await?;
    Ok(RoleSnapshot {
        meta: snapshot.meta().clone(),
        roles: snapshot.roles().cloned().collect(),
        assignments: snapshot.assignments().cloned().collect(),
    })
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
        "Project View v3 integrity error: {}",
        message.into()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_core::Keys;

    #[test]
    fn managed_role_fences_follow_shared_operation_intent() {
        let proposal_id = Uuid::new_v4();
        let role_id = Uuid::new_v4();
        let identity_requests = [
            RoleCommandRequest::RequestRole {
                proposal_id,
                role_id,
                expires_at: Utc::now() + Duration::hours(1),
                reason: None,
            },
            RoleCommandRequest::AcceptProposal { proposal_id },
            RoleCommandRequest::WithdrawProposal {
                proposal_id,
                reason: None,
            },
            RoleCommandRequest::ExpireProposal { proposal_id },
        ];
        for request in identity_requests {
            assert_eq!(request.actor_intent(), RoleActorIntent::CommunityIdentity);
            assert!(!managed_command_requires_assignment(
                request.actor_intent(),
                None
            ));
        }

        let reject = RoleCommandRequest::RejectProposal {
            proposal_id,
            reason: None,
        };
        assert_eq!(reject.actor_intent(), RoleActorIntent::CandidateOrGovernor);
        assert!(!managed_command_requires_assignment(
            reject.actor_intent(),
            Some(true)
        ));
        assert!(managed_command_requires_assignment(
            reject.actor_intent(),
            Some(false)
        ));
        assert!(!managed_command_requires_assignment(
            reject.actor_intent(),
            None
        ));

        let offer = RoleCommandRequest::OfferRole {
            proposal_id,
            role_id,
            candidate_pubkey: Keys::generate().public_key(),
            expires_at: Utc::now() + Duration::hours(1),
            reason: None,
        };
        assert!(managed_command_requires_assignment(
            offer.actor_intent(),
            None
        ));

        let replacement = RoleCommandRequest::RequestReplacement {
            assignment_id: Uuid::new_v4(),
            reason: None,
        };
        assert!(managed_command_requires_assignment(
            replacement.actor_intent(),
            None
        ));
    }
}
