//! Strict parsing and normalization for Relay Project View Role receipts.

use std::collections::BTreeSet;

use buzz_project_view_pkg::v2::{RoleCommand, RoleCommandRequest};
use nostr::Event;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::relay::SubmitEventResponse;

use super::RoleChangedEntity;

#[derive(Debug, Deserialize)]
pub(super) struct RoleReceipt {
    pub(super) project_revision: u64,
    pub(super) operation: String,
    pub(super) changed_entities: Vec<RoleChangedEntity>,
    pub(super) proposal_id: Option<Uuid>,
    pub(super) assignment_id: Option<Uuid>,
    pub(super) target_assignment_id: Option<Uuid>,
    pub(super) work_id: Option<Uuid>,
    pub(super) responsible_role_id: Option<Uuid>,
    pub(super) commitment_id: Option<Uuid>,
    pub(super) checkpoint_id: Option<Uuid>,
    pub(super) handoff_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleReceiptV3 {
    schema_version: u16,
    operation: String,
    project_revision: u64,
    entities: Vec<RoleChangedEntity>,
    work_objects: Vec<RoleWorkObjectReceiptV3>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleWorkObjectReceiptV3 {
    object_id: Uuid,
    object_revision: u64,
    responsible_role_id: Option<Uuid>,
}

pub(super) fn parse_role_receipt(
    response: &SubmitEventResponse,
    event: &Event,
    command: &RoleCommand,
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
    let value: Value = serde_json::from_str(payload)
        .map_err(|error| format!("Project View integrity error: invalid Role receipt: {error}"))?;
    if value.get("schema_version").is_some() {
        let receipt = serde_json::from_value(value).map_err(|error| {
            format!("Project View integrity error: invalid v3 Role receipt: {error}")
        })?;
        return normalize_v3_receipt(receipt, command);
    }

    let receipt: RoleReceipt = serde_json::from_value(value)
        .map_err(|error| format!("Project View integrity error: invalid Role receipt: {error}"))?;
    if receipt.operation != command.operation() {
        return Err(
            "Project View integrity error: Role receipt operation differs from the signed command"
                .to_owned(),
        );
    }
    Ok(receipt)
}

#[allow(clippy::too_many_lines)]
fn normalize_v3_receipt(
    receipt: RoleReceiptV3,
    command: &RoleCommand,
) -> Result<RoleReceipt, String> {
    if receipt.schema_version != 3 || receipt.operation != command.operation() {
        return Err(v3_mismatch());
    }

    let mut entity_coordinates = BTreeSet::new();
    for entity in &receipt.entities {
        if entity.entity_revision == 0
            || !matches!(
                entity.entity_type.as_str(),
                "role"
                    | "role_assignment_proposal"
                    | "role_assignment"
                    | "work_commitment"
                    | "role_checkpoint"
                    | "role_handoff"
            )
            || !entity_coordinates.insert((entity.entity_type.as_str(), entity.entity_id))
        {
            return Err(v3_mismatch());
        }
    }

    let mut work_ids = BTreeSet::new();
    if receipt
        .work_objects
        .iter()
        .any(|work| work.object_revision == 0 || !work_ids.insert(work.object_id))
    {
        return Err(v3_mismatch());
    }

    let mut proposal_id = None;
    let mut target_assignment_id = None;
    let mut work_id = None;
    let mut responsible_role_id = None;
    let mut commitment_id = None;
    let mut checkpoint_id = None;
    let mut handoff_id = None;

    match &command.request {
        RoleCommandRequest::RequestRole {
            proposal_id: expected,
            ..
        }
        | RoleCommandRequest::OfferRole {
            proposal_id: expected,
            ..
        }
        | RoleCommandRequest::AcceptProposal {
            proposal_id: expected,
        }
        | RoleCommandRequest::RejectProposal {
            proposal_id: expected,
            ..
        }
        | RoleCommandRequest::WithdrawProposal {
            proposal_id: expected,
            ..
        }
        | RoleCommandRequest::ExpireProposal {
            proposal_id: expected,
        }
        | RoleCommandRequest::AuthorizeProposal {
            proposal_id: expected,
        } => {
            require_entity(&receipt, "role_assignment_proposal", *expected)?;
            proposal_id = Some(*expected);
        }
        RoleCommandRequest::EndAssignment {
            assignment_id: expected,
            ..
        }
        | RoleCommandRequest::RequestReplacement {
            assignment_id: expected,
            ..
        }
        | RoleCommandRequest::ReportUnableToContinue {
            assignment_id: expected,
            ..
        } => {
            require_entity(&receipt, "role_assignment", *expected)?;
            target_assignment_id = Some(*expected);
        }
        RoleCommandRequest::SetWorkResponsibility {
            work_id: expected_work,
            responsible_role_id: expected_role,
        } => {
            if !receipt.work_objects.iter().any(|work| {
                work.object_id == *expected_work && work.responsible_role_id == *expected_role
            }) {
                return Err(v3_mismatch());
            }
            work_id = Some(*expected_work);
            responsible_role_id = *expected_role;
        }
        RoleCommandRequest::AcceptWork {
            commitment_id: expected_commitment,
            work_id: expected_work,
        } => {
            require_entity(&receipt, "work_commitment", *expected_commitment)?;
            commitment_id = Some(*expected_commitment);
            work_id = Some(*expected_work);
        }
        RoleCommandRequest::EndCommitment {
            commitment_id: expected,
            ..
        } => {
            require_entity(&receipt, "work_commitment", *expected)?;
            commitment_id = Some(*expected);
        }
        RoleCommandRequest::ReplaceCommitment {
            commitment_id: expected,
            work_id: expected_work,
            expected_commitment_id,
        } => {
            require_entity(&receipt, "work_commitment", *expected)?;
            require_entity(&receipt, "work_commitment", *expected_commitment_id)?;
            commitment_id = Some(*expected);
            work_id = Some(*expected_work);
        }
        RoleCommandRequest::AppendCheckpoint {
            checkpoint_id: expected,
            ..
        } => {
            require_entity(&receipt, "role_checkpoint", *expected)?;
            checkpoint_id = Some(*expected);
        }
        RoleCommandRequest::AppendHandoff {
            handoff_id: expected,
            ..
        } => {
            require_entity(&receipt, "role_handoff", *expected)?;
            handoff_id = Some(*expected);
        }
    }

    let mut created_assignments = receipt
        .entities
        .iter()
        .filter(|entity| entity.entity_type == "role_assignment" && entity.entity_revision == 1)
        .map(|entity| entity.entity_id);
    let assignment_id = created_assignments.next();
    if created_assignments.next().is_some()
        || matches!(
            &command.request,
            RoleCommandRequest::AcceptProposal { .. }
                | RoleCommandRequest::AuthorizeProposal { .. }
        ) && assignment_id.is_none()
    {
        return Err(v3_mismatch());
    }

    Ok(RoleReceipt {
        project_revision: receipt.project_revision,
        operation: receipt.operation,
        changed_entities: receipt.entities,
        proposal_id,
        assignment_id,
        target_assignment_id,
        work_id,
        responsible_role_id,
        commitment_id,
        checkpoint_id,
        handoff_id,
    })
}

fn require_entity(
    receipt: &RoleReceiptV3,
    entity_type: &str,
    entity_id: Uuid,
) -> Result<(), String> {
    if receipt
        .entities
        .iter()
        .any(|entity| entity.entity_type == entity_type && entity.entity_id == entity_id)
    {
        Ok(())
    } else {
        Err(v3_mismatch())
    }
}

fn v3_mismatch() -> String {
    "Project View integrity error: v3 Role receipt does not match the signed command".to_owned()
}
