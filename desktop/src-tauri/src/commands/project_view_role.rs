//! Typed Project View v2 Role-governance bridge for the desktop client.

use buzz_core_pkg::kind::KIND_PROJECT_VIEW_META;
use buzz_core_pkg::PublicKey;
use buzz_project_view_pkg::v2::{
    HandoffCause, RoleCheckpointContent, RoleCommand, RoleCommandRequest, RoleHandoffContent,
};
use buzz_project_view_pkg::v3::RoleCommandV3;
use buzz_sdk_pkg::project_view_v2::{
    build_role_command, parse_meta_projection as parse_v2_meta_projection, V2MetaProjection,
    V2ProjectionSource,
};
use buzz_sdk_pkg::project_view_v3::{
    build_role_command as build_v3_role_command, parse_meta_projection as parse_v3_meta_projection,
    V3MetaProjection, V3ProjectionSource,
};
use chrono::{Duration, Utc};
use nostr::{Event, Keys};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::State;
use uuid::Uuid;

use crate::app_state::AppState;
use crate::relay::{
    query_relay_at_with_keys, relay_api_base_url_with_override, submit_signed_event_at_with_keys,
};

use super::project_view::{read_identity_at, ProjectViewIdentity, ProjectViewSchema};

#[path = "project_view_role_receipt.rs"]
mod receipt;

use receipt::parse_role_receipt;

/// A closed Human Role-governance intent.
#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectViewRoleMutationInput {
    /// Ask to take one Role. Candidate confirmation is implicit.
    RequestRole {
        /// Verified Project revision on which the Human acts.
        expected_project_revision: u64,
        /// Desired Role.
        role_id: Uuid,
        /// Proposal lifetime in hours.
        expires_in_hours: u16,
        /// Optional Human explanation.
        reason: Option<String>,
        /// Active tenure when the signer is acting as a Role.
        acting_assignment_id: Option<Uuid>,
    },
    /// Offer a vacant or occupied Role to a candidate.
    ///
    /// Acceptance performs any replacement atomically.
    OfferRole {
        /// Verified Project revision on which the Human acts.
        expected_project_revision: u64,
        /// Offered Role.
        role_id: Uuid,
        /// Candidate's canonical public key.
        candidate_pubkey: String,
        /// Proposal lifetime in hours.
        expires_in_hours: u16,
        /// Optional Human explanation.
        reason: Option<String>,
        /// Active tenure when a Leader performs this action.
        acting_assignment_id: Option<Uuid>,
    },
    /// Candidate accepts an offer.
    AcceptProposal {
        /// Verified Project revision on which the Human acts.
        expected_project_revision: u64,
        /// Proposal to accept.
        proposal_id: Uuid,
        /// Active tenure when the candidate already holds another Role.
        acting_assignment_id: Option<Uuid>,
    },
    /// Candidate or governor rejects a Proposal.
    RejectProposal {
        /// Verified Project revision on which the Human acts.
        expected_project_revision: u64,
        /// Proposal to reject.
        proposal_id: Uuid,
        /// Optional explanation.
        reason: Option<String>,
        /// Active tenure when a Leader performs this action.
        acting_assignment_id: Option<Uuid>,
    },
    /// Proposal creator withdraws it.
    WithdrawProposal {
        /// Verified Project revision on which the Human acts.
        expected_project_revision: u64,
        /// Proposal to withdraw.
        proposal_id: Uuid,
        /// Optional explanation.
        reason: Option<String>,
        /// Active tenure when the creator is acting as a Role.
        acting_assignment_id: Option<Uuid>,
    },
    /// Owner or Leader authorizes a candidate request.
    AuthorizeProposal {
        /// Verified Project revision on which the Human acts.
        expected_project_revision: u64,
        /// Proposal to authorize.
        proposal_id: Uuid,
        /// Active Leader tenure; owner may omit it.
        acting_assignment_id: Option<Uuid>,
    },
    /// Owner or an authorized Leader ends an active tenure.
    EndAssignment {
        /// Verified Project revision on which the Human acts.
        expected_project_revision: u64,
        /// Assignment to end.
        assignment_id: Uuid,
        /// Optional governance explanation.
        reason: Option<String>,
        /// Active Leader tenure; owner may omit it.
        acting_assignment_id: Option<Uuid>,
    },
    /// Owner or Leader changes the stable Role responsible for Work.
    SetWorkResponsibility {
        /// Verified Project revision on which the Human acts.
        expected_project_revision: u64,
        /// Work being assigned or unassigned.
        work_id: Uuid,
        /// New responsible Role; absent clears responsibility.
        responsible_role_id: Option<Uuid>,
        /// Active Leader tenure; owner may omit it.
        acting_assignment_id: Option<Uuid>,
    },
    /// The current Role assignee explicitly accepts its Work.
    AcceptWork {
        /// Verified Project revision on which the Human acts.
        expected_project_revision: u64,
        /// Work owned by the current Assignment's Role.
        work_id: Uuid,
        /// Current Assignment tenure.
        acting_assignment_id: Option<Uuid>,
    },
    /// The current assignee releases one active Commitment.
    EndCommitment {
        /// Verified Project revision on which the Human acts.
        expected_project_revision: u64,
        /// Commitment to release.
        commitment_id: Uuid,
        /// Optional release context.
        reason: Option<String>,
        /// Current Assignment tenure.
        acting_assignment_id: Option<Uuid>,
    },
    /// The current assignee atomically recommits to the same Work.
    ReplaceCommitment {
        /// Verified Project revision on which the Human acts.
        expected_project_revision: u64,
        /// Work being recommitted.
        work_id: Uuid,
        /// Active Commitment observed by the Human.
        expected_commitment_id: Uuid,
        /// Current Assignment tenure.
        acting_assignment_id: Option<Uuid>,
    },
    /// The current assignee appends one structured continuity Checkpoint.
    AppendCheckpoint {
        /// Verified Project revision on which the Human acts.
        expected_project_revision: u64,
        /// Project revision whose facts were reviewed.
        based_on_project_revision: u64,
        /// Structured Checkpoint body.
        content: RoleCheckpointContent,
        /// Earlier Checkpoint corrected by this entry.
        supersedes_checkpoint_id: Option<Uuid>,
        /// Current Assignment tenure.
        acting_assignment_id: Option<Uuid>,
    },
    /// The current assignee appends a context-only Handoff note.
    AppendHandoff {
        /// Verified Project revision on which the Human acts.
        expected_project_revision: u64,
        /// Known successor Assignment in the same Role.
        to_assignment_id: Option<Uuid>,
        /// Checkpoint explicitly carried by this note.
        checkpoint_id: Option<Uuid>,
        /// Structured transition body.
        content: RoleHandoffContent,
        /// Member-authored cause (`planned` or `other`).
        cause: HandoffCause,
        /// Current Assignment tenure.
        acting_assignment_id: Option<Uuid>,
    },
}

/// Result of one non-replayed desktop Role intent.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProjectViewRoleMutationResult {
    /// Relay committed the command and its signed v2 metadata was confirmed.
    Applied {
        /// Submitted member event.
        event_id: String,
        /// New canonical Project revision.
        project_revision: u64,
        /// Stable operation.
        operation: String,
        /// Proposal touched by this operation.
        #[serde(skip_serializing_if = "Option::is_none")]
        proposal_id: Option<Uuid>,
        /// Assignment created when a Proposal completed.
        #[serde(skip_serializing_if = "Option::is_none")]
        assignment_id: Option<Uuid>,
        /// Assignment explicitly targeted by an end/report operation.
        #[serde(skip_serializing_if = "Option::is_none")]
        target_assignment_id: Option<Uuid>,
        /// Work touched by a responsibility or Commitment command.
        #[serde(skip_serializing_if = "Option::is_none")]
        work_id: Option<Uuid>,
        /// New Work responsibility, absent for other commands or after clear.
        #[serde(skip_serializing_if = "Option::is_none")]
        responsible_role_id: Option<Uuid>,
        /// Commitment created or targeted by the command.
        #[serde(skip_serializing_if = "Option::is_none")]
        commitment_id: Option<Uuid>,
        /// Checkpoint created by the command.
        #[serde(skip_serializing_if = "Option::is_none")]
        checkpoint_id: Option<Uuid>,
        /// Handoff created by the command.
        #[serde(skip_serializing_if = "Option::is_none")]
        handoff_id: Option<Uuid>,
        /// Canonical changed entity coordinates from the receipt.
        changed_entities: Vec<RoleChangedEntity>,
    },
    /// Another accepted change made the Human's baseline stale.
    Conflict {
        /// Revision carried by the rejected intent.
        expected_project_revision: u64,
        /// Latest verified revision, when available.
        #[serde(skip_serializing_if = "Option::is_none")]
        current_project_revision: Option<u64>,
        /// Relay diagnostic. The desktop never auto-replays this intent.
        message: String,
    },
}

/// One changed Role-continuity entity reported by the canonical receipt.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct RoleChangedEntity {
    entity_type: String,
    entity_id: Uuid,
    entity_revision: u64,
}

struct RoleMutationContext {
    api_base_url: String,
    identity: ProjectViewIdentity,
    keys: Keys,
}

enum RoleMutationMeta {
    V2(V2MetaProjection),
    V3(V3MetaProjection),
}

impl RoleMutationMeta {
    const fn project_revision(&self) -> u64 {
        match self {
            Self::V2(meta) => meta.project_revision,
            Self::V3(meta) => meta.project_revision,
        }
    }

    fn identifies_source(&self, event: &Event) -> bool {
        match self {
            Self::V2(meta) => matches!(
                meta.source,
                V2ProjectionSource::NostrEvent {
                    event_id,
                    change_id,
                } if event_id == event.id && change_id == event.id
            ),
            Self::V3(meta) => matches!(
                meta.source,
                V3ProjectionSource::NostrEvent {
                    event_id,
                    change_id,
                } if event_id == event.id && change_id == event.id
            ),
        }
    }
}

/// Validate, sign, submit, and confirm one Project View v2/v3 Role intent.
#[tauri::command]
pub async fn mutate_project_view_role(
    input: ProjectViewRoleMutationInput,
    state: State<'_, AppState>,
) -> Result<ProjectViewRoleMutationResult, String> {
    execute_role_mutation(input, &state).await
}

async fn execute_role_mutation(
    input: ProjectViewRoleMutationInput,
    state: &AppState,
) -> Result<ProjectViewRoleMutationResult, String> {
    let (command, expected_project_revision) = prepare_role_command(input)?;
    let api_base_url = relay_api_base_url_with_override(state);
    let keys = state.signing_keys()?;
    let identity = read_identity_at(state, &api_base_url)
        .await?
        .ok_or_else(|| "unsupported: Relay does not advertise Project View".to_owned())?;
    let context = RoleMutationContext {
        api_base_url,
        identity,
        keys,
    };
    let builder = match context.identity.schema {
        ProjectViewSchema::V1 => {
            return Err(
                "unsupported: Role continuity requires Project View schema v2 or v3".to_owned(),
            )
        }
        ProjectViewSchema::V2 => build_role_command(command.clone())
            .map_err(|error| format!("invalid Role intent: {error}"))?,
        ProjectViewSchema::V3 => build_v3_role_command(RoleCommandV3::new(
            command.expected_project_revision,
            command.acting_assignment_id,
            command.request.clone(),
        ))
        .map_err(|error| format!("invalid Role v3 intent: {error}"))?,
    };
    let event = builder
        .sign_with_keys(&context.keys)
        .map_err(|error| format!("failed to sign Role intent: {error}"))?;
    let response =
        match submit_signed_event_at_with_keys(&event, state, &context.api_base_url, &context.keys)
            .await
        {
            Ok(response) => response,
            Err(message) if message.starts_with("relay returned 409") => {
                let current_project_revision = read_role_meta(state, &context)
                    .await
                    .ok()
                    .flatten()
                    .map(|meta| meta.project_revision());
                return Ok(ProjectViewRoleMutationResult::Conflict {
                    expected_project_revision,
                    current_project_revision,
                    message,
                });
            }
            Err(message) => return Err(message),
        };
    let receipt = parse_role_receipt(&response, &event, &command)?;
    confirm_role_meta(state, &context, &event, receipt.project_revision).await?;
    Ok(ProjectViewRoleMutationResult::Applied {
        event_id: event.id.to_hex(),
        project_revision: receipt.project_revision,
        operation: receipt.operation,
        proposal_id: receipt.proposal_id,
        assignment_id: receipt.assignment_id,
        target_assignment_id: receipt.target_assignment_id,
        work_id: receipt.work_id,
        responsible_role_id: receipt.responsible_role_id,
        commitment_id: receipt.commitment_id,
        checkpoint_id: receipt.checkpoint_id,
        handoff_id: receipt.handoff_id,
        changed_entities: receipt.changed_entities,
    })
}

fn prepare_role_command(input: ProjectViewRoleMutationInput) -> Result<(RoleCommand, u64), String> {
    let (expected_project_revision, acting_assignment_id, request) = match input {
        ProjectViewRoleMutationInput::RequestRole {
            expected_project_revision,
            role_id,
            expires_in_hours,
            reason,
            acting_assignment_id,
        } => (
            expected_project_revision,
            acting_assignment_id,
            RoleCommandRequest::RequestRole {
                proposal_id: Uuid::new_v4(),
                role_id,
                expires_at: proposal_deadline(expires_in_hours)?,
                reason,
            },
        ),
        ProjectViewRoleMutationInput::OfferRole {
            expected_project_revision,
            role_id,
            candidate_pubkey,
            expires_in_hours,
            reason,
            acting_assignment_id,
        } => (
            expected_project_revision,
            acting_assignment_id,
            RoleCommandRequest::OfferRole {
                proposal_id: Uuid::new_v4(),
                role_id,
                candidate_pubkey: parse_candidate_pubkey(&candidate_pubkey)?,
                expires_at: proposal_deadline(expires_in_hours)?,
                reason,
            },
        ),
        ProjectViewRoleMutationInput::AcceptProposal {
            expected_project_revision,
            proposal_id,
            acting_assignment_id,
        } => (
            expected_project_revision,
            acting_assignment_id,
            RoleCommandRequest::AcceptProposal { proposal_id },
        ),
        ProjectViewRoleMutationInput::RejectProposal {
            expected_project_revision,
            proposal_id,
            reason,
            acting_assignment_id,
        } => (
            expected_project_revision,
            acting_assignment_id,
            RoleCommandRequest::RejectProposal {
                proposal_id,
                reason,
            },
        ),
        ProjectViewRoleMutationInput::WithdrawProposal {
            expected_project_revision,
            proposal_id,
            reason,
            acting_assignment_id,
        } => (
            expected_project_revision,
            acting_assignment_id,
            RoleCommandRequest::WithdrawProposal {
                proposal_id,
                reason,
            },
        ),
        ProjectViewRoleMutationInput::AuthorizeProposal {
            expected_project_revision,
            proposal_id,
            acting_assignment_id,
        } => (
            expected_project_revision,
            acting_assignment_id,
            RoleCommandRequest::AuthorizeProposal { proposal_id },
        ),
        ProjectViewRoleMutationInput::EndAssignment {
            expected_project_revision,
            assignment_id,
            reason,
            acting_assignment_id,
        } => (
            expected_project_revision,
            acting_assignment_id,
            RoleCommandRequest::EndAssignment {
                assignment_id,
                reason,
            },
        ),
        ProjectViewRoleMutationInput::SetWorkResponsibility {
            expected_project_revision,
            work_id,
            responsible_role_id,
            acting_assignment_id,
        } => (
            expected_project_revision,
            acting_assignment_id,
            RoleCommandRequest::SetWorkResponsibility {
                work_id,
                responsible_role_id,
            },
        ),
        ProjectViewRoleMutationInput::AcceptWork {
            expected_project_revision,
            work_id,
            acting_assignment_id,
        } => (
            expected_project_revision,
            acting_assignment_id,
            RoleCommandRequest::AcceptWork {
                commitment_id: Uuid::new_v4(),
                work_id,
            },
        ),
        ProjectViewRoleMutationInput::EndCommitment {
            expected_project_revision,
            commitment_id,
            reason,
            acting_assignment_id,
        } => (
            expected_project_revision,
            acting_assignment_id,
            RoleCommandRequest::EndCommitment {
                commitment_id,
                reason,
            },
        ),
        ProjectViewRoleMutationInput::ReplaceCommitment {
            expected_project_revision,
            work_id,
            expected_commitment_id,
            acting_assignment_id,
        } => (
            expected_project_revision,
            acting_assignment_id,
            RoleCommandRequest::ReplaceCommitment {
                commitment_id: Uuid::new_v4(),
                work_id,
                expected_commitment_id,
            },
        ),
        ProjectViewRoleMutationInput::AppendCheckpoint {
            expected_project_revision,
            based_on_project_revision,
            content,
            supersedes_checkpoint_id,
            acting_assignment_id,
        } => (
            expected_project_revision,
            acting_assignment_id,
            RoleCommandRequest::AppendCheckpoint {
                checkpoint_id: Uuid::new_v4(),
                based_on_project_revision,
                content,
                supersedes_checkpoint_id,
            },
        ),
        ProjectViewRoleMutationInput::AppendHandoff {
            expected_project_revision,
            to_assignment_id,
            checkpoint_id,
            content,
            cause,
            acting_assignment_id,
        } => (
            expected_project_revision,
            acting_assignment_id,
            RoleCommandRequest::AppendHandoff {
                handoff_id: Uuid::new_v4(),
                to_assignment_id,
                checkpoint_id,
                content,
                cause,
            },
        ),
    };
    let command = RoleCommand::new(expected_project_revision, acting_assignment_id, request);
    command
        .validate_for_submission()
        .map_err(|error| format!("invalid Role intent: {error}"))?;
    Ok((command, expected_project_revision))
}

fn proposal_deadline(expires_in_hours: u16) -> Result<chrono::DateTime<Utc>, String> {
    if !(1..=720).contains(&expires_in_hours) {
        return Err("Proposal lifetime must be between 1 and 720 hours".to_owned());
    }
    Ok(Utc::now() + Duration::hours(i64::from(expires_in_hours)))
}

fn parse_candidate_pubkey(value: &str) -> Result<PublicKey, String> {
    let candidate = PublicKey::parse(value)
        .map_err(|error| format!("invalid candidate public key: {error}"))?;
    if value.len() == 64 && candidate.to_hex() != value {
        return Err("candidate public key hex must be canonical lowercase".to_owned());
    }
    Ok(candidate)
}

async fn confirm_role_meta(
    state: &AppState,
    context: &RoleMutationContext,
    event: &Event,
    receipt_revision: u64,
) -> Result<(), String> {
    let meta = read_role_meta(state, context).await?.ok_or_else(|| {
        "Project View integrity error: successful Role command has no metadata".to_owned()
    })?;
    if meta.project_revision() < receipt_revision {
        return Err(
            "Project View integrity error: metadata is older than the Role receipt".to_owned(),
        );
    }
    if meta.project_revision() == receipt_revision && !meta.identifies_source(event) {
        return Err(
            "Project View integrity error: metadata does not identify the submitted Role command"
                .to_owned(),
        );
    }
    Ok(())
}

async fn read_role_meta(
    state: &AppState,
    context: &RoleMutationContext,
) -> Result<Option<RoleMutationMeta>, String> {
    let events = query_relay_at_with_keys(
        state,
        &context.api_base_url,
        &[json!({
            "kinds": [KIND_PROJECT_VIEW_META],
            "authors": [context.identity.relay_pubkey.to_hex()],
            "limit": 2,
        })],
        &context.keys,
        None,
    )
    .await?;
    match events.as_slice() {
        [] => Ok(None),
        [event] => match context.identity.schema {
            ProjectViewSchema::V1 => Err(
                "Project View integrity error: Role continuity is unavailable in schema v1"
                    .to_owned(),
            ),
            ProjectViewSchema::V2 => {
                parse_v2_meta_projection(event, &context.identity.relay_pubkey)
                    .map(RoleMutationMeta::V2)
                    .map(Some)
                    .map_err(|error| format!("Project View integrity error: {error}"))
            }
            ProjectViewSchema::V3 => {
                parse_v3_meta_projection(event, &context.identity.relay_pubkey)
                    .map(RoleMutationMeta::V3)
                    .map(Some)
                    .map_err(|error| format!("Project View integrity error: {error}"))
            }
        },
        _ => Err(
            "Project View integrity error: metadata query returned multiple current heads"
                .to_owned(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::SubmitEventResponse;

    #[test]
    fn offer_is_closed_and_generates_a_fresh_proposal() {
        let candidate =
            PublicKey::parse("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
                .expect("candidate");
        let role_id = Uuid::new_v4();
        let (command, revision) = prepare_role_command(ProjectViewRoleMutationInput::OfferRole {
            expected_project_revision: 9,
            role_id,
            candidate_pubkey: candidate.to_hex(),
            expires_in_hours: 72,
            reason: Some("Take over the module".to_owned()),
            acting_assignment_id: None,
        })
        .expect("prepare offer");

        assert_eq!(revision, 9);
        assert!(matches!(
            command.request,
            RoleCommandRequest::OfferRole {
                role_id: actual,
                candidate_pubkey: actual_candidate,
                ..
            } if actual == role_id && actual_candidate == candidate
        ));
    }

    #[test]
    fn proposal_lifetime_is_bounded() {
        assert!(proposal_deadline(0).is_err());
        assert!(proposal_deadline(721).is_err());
        assert!(proposal_deadline(72).is_ok());
    }

    #[test]
    fn v3_offer_receipt_is_normalized_for_role_confirmation() {
        let candidate =
            PublicKey::parse("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
                .expect("candidate");
        let (command, _) = prepare_role_command(ProjectViewRoleMutationInput::OfferRole {
            expected_project_revision: 10,
            role_id: Uuid::new_v4(),
            candidate_pubkey: candidate.to_hex(),
            expires_in_hours: 72,
            reason: None,
            acting_assignment_id: None,
        })
        .expect("prepare offer");
        let proposal_id = match &command.request {
            RoleCommandRequest::OfferRole { proposal_id, .. } => *proposal_id,
            _ => panic!("expected offer"),
        };
        let event = build_v3_role_command(RoleCommandV3::new(
            command.expected_project_revision,
            command.acting_assignment_id,
            command.request.clone(),
        ))
        .expect("build v3 Role command")
        .sign_with_keys(&Keys::generate())
        .expect("sign v3 Role command");
        let response = SubmitEventResponse {
            event_id: event.id.to_hex(),
            accepted: true,
            message: format!(
                "response:{}",
                json!({
                    "schema_version": 3,
                    "operation": "offer_role",
                    "project_revision": 11,
                    "entities": [{
                        "entity_type": "role_assignment_proposal",
                        "entity_id": proposal_id,
                        "entity_revision": 1,
                    }],
                    "work_objects": [],
                })
            ),
        };

        let receipt =
            parse_role_receipt(&response, &event, &command).expect("normalize v3 receipt");

        assert_eq!(receipt.project_revision, 11);
        assert_eq!(receipt.operation, "offer_role");
        assert_eq!(receipt.proposal_id, Some(proposal_id));
        assert_eq!(receipt.changed_entities.len(), 1);
    }

    #[test]
    fn v3_role_receipt_rejects_a_different_operation() {
        let candidate =
            PublicKey::parse("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
                .expect("candidate");
        let (command, _) = prepare_role_command(ProjectViewRoleMutationInput::OfferRole {
            expected_project_revision: 10,
            role_id: Uuid::new_v4(),
            candidate_pubkey: candidate.to_hex(),
            expires_in_hours: 72,
            reason: None,
            acting_assignment_id: None,
        })
        .expect("prepare offer");
        let proposal_id = match &command.request {
            RoleCommandRequest::OfferRole { proposal_id, .. } => *proposal_id,
            _ => panic!("expected offer"),
        };
        let event = build_v3_role_command(RoleCommandV3::new(
            command.expected_project_revision,
            command.acting_assignment_id,
            command.request.clone(),
        ))
        .expect("build v3 Role command")
        .sign_with_keys(&Keys::generate())
        .expect("sign v3 Role command");
        let response = SubmitEventResponse {
            event_id: event.id.to_hex(),
            accepted: true,
            message: format!(
                "response:{}",
                json!({
                    "schema_version": 3,
                    "operation": "request_role",
                    "project_revision": 11,
                    "entities": [{
                        "entity_type": "role_assignment_proposal",
                        "entity_id": proposal_id,
                        "entity_revision": 1,
                    }],
                    "work_objects": [],
                })
            ),
        };

        let error = parse_role_receipt(&response, &event, &command)
            .expect_err("receipt operation must match the signed Role command");

        assert!(error.contains("v3 Role receipt does not match"));
    }

    #[test]
    fn work_acceptance_and_recommit_generate_new_commitment_ids() {
        let work_id = Uuid::new_v4();
        let assignment_id = Uuid::new_v4();
        let observed_commitment_id = Uuid::new_v4();
        let (accept, revision) = prepare_role_command(ProjectViewRoleMutationInput::AcceptWork {
            expected_project_revision: 12,
            work_id,
            acting_assignment_id: Some(assignment_id),
        })
        .expect("prepare Work acceptance");
        let RoleCommandRequest::AcceptWork {
            commitment_id: accepted_id,
            work_id: accepted_work_id,
        } = accept.request
        else {
            panic!("expected Work acceptance");
        };
        assert_eq!(revision, 12);
        assert_eq!(accept.acting_assignment_id, Some(assignment_id));
        assert_eq!(accepted_work_id, work_id);
        assert!(!accepted_id.is_nil());

        let (replacement, _) =
            prepare_role_command(ProjectViewRoleMutationInput::ReplaceCommitment {
                expected_project_revision: 13,
                work_id,
                expected_commitment_id: observed_commitment_id,
                acting_assignment_id: Some(assignment_id),
            })
            .expect("prepare Work recommit");
        assert!(matches!(
            replacement.request,
            RoleCommandRequest::ReplaceCommitment {
                commitment_id,
                work_id: actual_work_id,
                expected_commitment_id,
            } if !commitment_id.is_nil()
                && commitment_id != observed_commitment_id
                && actual_work_id == work_id
                && expected_commitment_id == observed_commitment_id
        ));
    }

    #[test]
    fn continuity_writes_generate_append_only_ids_and_keep_the_assignment_fence() {
        let assignment_id = Uuid::new_v4();
        let superseded_id = Uuid::new_v4();
        let checkpoint_content = RoleCheckpointContent {
            summary: "The release path is verified".to_owned(),
            current_focus: vec!["finish migration tests".to_owned()],
            progress: vec![],
            blockers: vec!["waiting for review".to_owned()],
            risks: vec![],
            open_questions: vec![],
            next_steps: vec!["publish the handoff".to_owned()],
            references: vec![],
        };
        let (checkpoint, revision) =
            prepare_role_command(ProjectViewRoleMutationInput::AppendCheckpoint {
                expected_project_revision: 21,
                based_on_project_revision: 21,
                content: checkpoint_content.clone(),
                supersedes_checkpoint_id: Some(superseded_id),
                acting_assignment_id: Some(assignment_id),
            })
            .expect("prepare Checkpoint");
        let RoleCommandRequest::AppendCheckpoint {
            checkpoint_id,
            based_on_project_revision,
            content,
            supersedes_checkpoint_id,
        } = checkpoint.request
        else {
            panic!("expected append Checkpoint");
        };
        assert_eq!(revision, 21);
        assert_eq!(checkpoint.acting_assignment_id, Some(assignment_id));
        assert!(!checkpoint_id.is_nil());
        assert_eq!(based_on_project_revision, 21);
        assert_eq!(content, checkpoint_content);
        assert_eq!(supersedes_checkpoint_id, Some(superseded_id));

        let successor_id = Uuid::new_v4();
        let handoff_content = RoleHandoffContent {
            summary: Some("Continue the release".to_owned()),
            unresolved_items: vec!["confirm rollout".to_owned()],
            references: vec![],
        };
        let (handoff, _) = prepare_role_command(ProjectViewRoleMutationInput::AppendHandoff {
            expected_project_revision: 22,
            to_assignment_id: Some(successor_id),
            checkpoint_id: Some(checkpoint_id),
            content: handoff_content.clone(),
            cause: HandoffCause::Planned,
            acting_assignment_id: Some(assignment_id),
        })
        .expect("prepare Handoff");
        assert_eq!(handoff.acting_assignment_id, Some(assignment_id));
        assert!(matches!(
            handoff.request,
            RoleCommandRequest::AppendHandoff {
                handoff_id,
                to_assignment_id: Some(actual_successor),
                checkpoint_id: Some(actual_checkpoint),
                content,
                cause: HandoffCause::Planned,
            } if !handoff_id.is_nil()
                && actual_successor == successor_id
                && actual_checkpoint == checkpoint_id
                && content == handoff_content
        ));
    }

    #[test]
    fn human_handoff_cannot_claim_a_system_cause() {
        let result = prepare_role_command(ProjectViewRoleMutationInput::AppendHandoff {
            expected_project_revision: 3,
            to_assignment_id: None,
            checkpoint_id: None,
            content: RoleHandoffContent {
                summary: Some("invalid system claim".to_owned()),
                unresolved_items: vec![],
                references: vec![],
            },
            cause: HandoffCause::Revoked,
            acting_assignment_id: Some(Uuid::new_v4()),
        });
        assert!(result.is_err());
    }
}
