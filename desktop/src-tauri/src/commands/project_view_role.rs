//! Typed Project View v2 Role-governance bridge for the desktop client.

use buzz_core_pkg::kind::KIND_PROJECT_VIEW_META;
use buzz_core_pkg::PublicKey;
use buzz_project_view_pkg::v2::{RoleCommand, RoleCommandRequest};
use buzz_sdk_pkg::project_view_v2::{
    build_role_command, parse_meta_projection, V2ProjectionSource,
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
    SubmitEventResponse,
};

use super::project_view::{read_identity_at, ProjectViewIdentity, ProjectViewSchema};

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
#[serde(rename_all = "camelCase")]
pub struct RoleChangedEntity {
    entity_type: String,
    entity_id: Uuid,
    entity_revision: u64,
}

#[derive(Debug, Deserialize)]
struct RoleReceipt {
    project_revision: u64,
    operation: String,
    changed_entities: Vec<RoleChangedEntity>,
    proposal_id: Option<Uuid>,
    assignment_id: Option<Uuid>,
    target_assignment_id: Option<Uuid>,
}

struct RoleMutationContext {
    api_base_url: String,
    identity: ProjectViewIdentity,
    keys: Keys,
}

/// Validate, sign, submit, and confirm one Project View v2 Role intent.
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
    if identity.schema != ProjectViewSchema::V2 {
        return Err("unsupported: Role continuity requires Project View schema v2".to_owned());
    }
    let context = RoleMutationContext {
        api_base_url,
        identity,
        keys,
    };
    let event = build_role_command(command)
        .map_err(|error| format!("invalid Role intent: {error}"))?
        .sign_with_keys(&context.keys)
        .map_err(|error| format!("failed to sign Role intent: {error}"))?;
    let response =
        match submit_signed_event_at_with_keys(&event, state, &context.api_base_url, &context.keys)
            .await
        {
            Ok(response) => response,
            Err(message) if message.starts_with("relay returned 409") => {
                let current_project_revision = read_v2_meta(state, &context)
                    .await
                    .ok()
                    .flatten()
                    .map(|meta| meta.project_revision);
                return Ok(ProjectViewRoleMutationResult::Conflict {
                    expected_project_revision,
                    current_project_revision,
                    message,
                });
            }
            Err(message) => return Err(message),
        };
    let receipt = parse_role_receipt(&response, &event)?;
    confirm_role_meta(state, &context, &event, receipt.project_revision).await?;
    Ok(ProjectViewRoleMutationResult::Applied {
        event_id: event.id.to_hex(),
        project_revision: receipt.project_revision,
        operation: receipt.operation,
        proposal_id: receipt.proposal_id,
        assignment_id: receipt.assignment_id,
        target_assignment_id: receipt.target_assignment_id,
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

fn parse_role_receipt(
    response: &SubmitEventResponse,
    event: &Event,
) -> Result<RoleReceipt, String> {
    if response.event_id != event.id.to_hex() {
        return Err(
            "Project View integrity error: Role response event_id differs from the submitted event"
                .to_owned(),
        );
    }
    let payload = response.message.strip_prefix("response:").ok_or_else(|| {
        "Project View integrity error: Role receipt is missing the canonical `response:` prefix"
            .to_owned()
    })?;
    serde_json::from_str(payload)
        .map_err(|error| format!("Project View integrity error: invalid Role receipt: {error}"))
}

async fn confirm_role_meta(
    state: &AppState,
    context: &RoleMutationContext,
    event: &Event,
    receipt_revision: u64,
) -> Result<(), String> {
    let meta = read_v2_meta(state, context).await?.ok_or_else(|| {
        "Project View integrity error: successful Role command has no v2 metadata".to_owned()
    })?;
    if meta.project_revision < receipt_revision {
        return Err(
            "Project View integrity error: v2 metadata is older than the Role receipt".to_owned(),
        );
    }
    if meta.project_revision == receipt_revision
        && !matches!(
            meta.source,
            V2ProjectionSource::NostrEvent {
                event_id,
                change_id,
            } if event_id == event.id && change_id == event.id
        )
    {
        return Err(
            "Project View integrity error: v2 metadata does not identify the submitted Role command"
                .to_owned(),
        );
    }
    Ok(())
}

async fn read_v2_meta(
    state: &AppState,
    context: &RoleMutationContext,
) -> Result<Option<buzz_sdk_pkg::project_view_v2::V2MetaProjection>, String> {
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
        [event] => parse_meta_projection(event, &context.identity.relay_pubkey)
            .map(Some)
            .map_err(|error| format!("Project View integrity error: {error}")),
        _ => Err(
            "Project View integrity error: v2 metadata query returned multiple current heads"
                .to_owned(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
