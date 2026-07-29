//! `buzz roles` — verified Project View v2 Role continuity reads and writes.

use std::collections::BTreeMap;

use buzz_core::PublicKey;
use buzz_project_view::v2::{
    HandoffCause, ProposalStatus, RoleAssignment, RoleAssignmentProposal, RoleCheckpoint,
    RoleCheckpointContent, RoleCommand, RoleCommandRequest, RoleContinuityChange,
    RoleContinuityEntity, RoleDefinition, RoleHandoff, RoleHandoffContent,
};
use buzz_sdk::project_view_v2::{build_role_command, V2MetaProjection};
use buzz_sdk::role_brief::render_role_brief_markdown;
use chrono::{Duration, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::client::{normalize_write_response, BuzzClient};
use crate::commands::project_view_v2_snapshot::{
    is_managed_runtime, read_current_v2_snapshot, read_role_history_page,
    read_verified_v2_snapshot, require_v2_identity, runtime_fence_from_env, RoleHistoryRequest,
};
use crate::error::CliError;
use crate::validate::{read_file_or_stdin, sdk_err};
use crate::{
    OutputFormat, RoleAssignmentCmd, RoleCheckpointCmd, RoleHandoffCauseArg, RoleHandoffCmd,
    RoleProposalCmd, RoleProposalStatusArg, RoleWorkCmd, RolesCmd,
};

#[derive(Debug)]
struct RoleSnapshot {
    meta: V2MetaProjection,
    roles: Vec<RoleDefinition>,
    proposals: Vec<RoleAssignmentProposal>,
    assignments: Vec<RoleAssignment>,
    checkpoints: Vec<RoleCheckpoint>,
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
        RolesCmd::Work { command } => dispatch_work(client, command).await,
        RolesCmd::Checkpoint { command } => dispatch_checkpoint(client, command, format).await,
        RolesCmd::Handoff { command } => dispatch_handoff(client, command, format).await,
    }
}

async fn show_brief(
    client: &BuzzClient,
    member: Option<&str>,
    markdown: bool,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let member = member
        .map(parse_pubkey)
        .transpose()?
        .unwrap_or_else(|| client.public_key());
    let snapshot = read_current_v2_snapshot(client).await?;
    let brief = snapshot
        .brief_for(member, Utc::now())
        .map_err(|error| integrity_error(error.to_string()))?;
    if markdown {
        print!("{}", render_role_brief_markdown(&brief));
        return Ok(());
    }
    print_json(
        &serde_json::to_value(brief)
            .map_err(|error| CliError::Other(format!("serialize Role Brief: {error}")))?,
        format,
    )
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
    let checkpoints = snapshot
        .checkpoints
        .iter()
        .filter(|checkpoint| checkpoint.role_id == role_id)
        .collect::<Vec<_>>();
    print_json(
        &json!({
            "project_revision": snapshot.meta.project_revision,
            "role": role,
            "vacant": current_assignment.is_none(),
            "current_assignment": current_assignment,
            "assignment_history": history,
            "proposals": proposals,
            "checkpoints": checkpoints,
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
    limit: u16,
    before: Option<&str>,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let identity = require_v2_identity(client).await?;
    let snapshot = read_verified_v2_snapshot(client, identity).await?;
    let page = read_role_history_page(
        client,
        identity,
        snapshot.meta(),
        RoleHistoryRequest {
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
            RoleContinuityChange::Proposal(proposal) => Some(proposal),
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
            limit,
            before,
        } => {
            let member = member.as_deref().map(parse_pubkey).transpose()?;
            let identity = require_v2_identity(client).await?;
            let snapshot = read_verified_v2_snapshot(client, identity).await?;
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
            let page = read_role_history_page(
                client,
                identity,
                snapshot.meta(),
                RoleHistoryRequest {
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
                    RoleContinuityChange::Assignment(assignment) => Some(assignment),
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

async fn dispatch_work(client: &BuzzClient, command: RoleWorkCmd) -> Result<(), CliError> {
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
    submit(client, RoleCommand::new(expected, acting, request)).await
}

async fn dispatch_checkpoint(
    client: &BuzzClient,
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
                RoleCommand::new(
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
            let identity = require_v2_identity(client).await?;
            let snapshot = read_verified_v2_snapshot(client, identity).await?;
            let page = read_role_history_page(
                client,
                identity,
                snapshot.meta(),
                RoleHistoryRequest {
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
                    RoleContinuityChange::Checkpoint(checkpoint) => Some(checkpoint),
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
    client: &BuzzClient,
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
                RoleCommand::new(
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
            let identity = require_v2_identity(client).await?;
            let snapshot = read_verified_v2_snapshot(client, identity).await?;
            let page = read_role_history_page(
                client,
                identity,
                snapshot.meta(),
                RoleHistoryRequest {
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
                    RoleContinuityChange::Handoff(handoff) => Some(handoff),
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

async fn submit(client: &BuzzClient, mut command: RoleCommand) -> Result<(), CliError> {
    let identity = require_v2_identity(client).await?;
    if is_managed_runtime() {
        let snapshot = read_verified_v2_snapshot(client, identity)
            .await
            .map_err(|error| {
                CliError::Other(format!(
                    "assignment_unavailable: current Assignment could not be verified: {error}"
                ))
            })?;
        let current_assignment = snapshot
            .assignments()
            .find(|assignment| {
                assignment.member_pubkey == client.public_key() && assignment.is_active()
            })
            .map(|assignment| assignment.assignment_id);
        if command
            .acting_assignment_id
            .is_some_and(|provided| Some(provided) != current_assignment)
        {
            return Err(CliError::Conflict(
                "provided acting Assignment is not the verified current Assignment".to_owned(),
            ));
        }
        match &command.request {
            RoleCommandRequest::RequestReplacement { assignment_id, .. }
            | RoleCommandRequest::ReportUnableToContinue { assignment_id, .. }
                if Some(*assignment_id) != current_assignment =>
            {
                return Err(CliError::Conflict(
                    "target Assignment is not the verified current Assignment".to_owned(),
                ));
            }
            _ => {}
        }
        command.acting_assignment_id = current_assignment;
        command.runtime_fence = runtime_fence_from_env()?;
    }
    let event = client.sign_event_exact(build_role_command(command).map_err(sdk_err)?)?;
    let response = client.submit_event(event).await?;
    println!("{}", normalize_write_response(&response));
    Ok(())
}

async fn read_snapshot(client: &BuzzClient) -> Result<RoleSnapshot, CliError> {
    let snapshot = read_current_v2_snapshot(client).await?;
    Ok(RoleSnapshot {
        meta: snapshot.meta().clone(),
        roles: snapshot.roles().cloned().collect(),
        proposals: snapshot.proposals().cloned().collect(),
        assignments: snapshot.assignments().cloned().collect(),
        checkpoints: snapshot.checkpoints().cloned().collect(),
        handoffs: snapshot.handoffs().cloned().collect(),
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
        "Project View v2 integrity error: {}",
        message.into()
    ))
}
