//! Pure Role Proposal and Assignment state machine for Project View v2.
//!
//! The Relay supplies current Community membership, managed-Agent ownership,
//! and canonical time. This module never performs I/O. It validates the signed
//! command, applies compound Assignment fencing, and returns the exact entity
//! and membership changes that one database transaction must persist.

use std::collections::{BTreeMap, BTreeSet};

use buzz_core::PublicKey;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{RoleLevel, SchemaVersion};
use crate::{WorkStatus, MAX_SAFE_REVISION};

const MAX_REASON_BYTES: usize = 4_096;

/// Longest Proposal lifetime accepted by the v0 role-continuity protocol.
pub const MAX_PROPOSAL_LIFETIME_DAYS: i64 = 30;

/// Community permission role visible to the Role Assignment reducer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommunityMemberRole {
    /// Unique Community owner.
    Owner,
    /// Community administrator backed by an active Leader Assignment in v2.
    Admin,
    /// Ordinary Community member.
    Member,
}

impl CommunityMemberRole {
    /// Return the stable database and wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }
}

/// Membership and managed-Agent facts supplied by the locked Community state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberGovernance {
    /// Member or candidate public key.
    pub pubkey: PublicKey,
    /// Direct Community role, absent for an eligible managed Agent that has
    /// not yet been materialized as a direct member.
    pub community_role: Option<CommunityMemberRole>,
    /// Whether this principal may currently participate in the Community.
    pub eligible: bool,
    /// Verified owner for a known managed Agent.
    pub managed_agent_owner: Option<PublicKey>,
}

impl MemberGovernance {
    /// Return whether this is the unique Community owner.
    #[must_use]
    pub const fn is_owner(&self) -> bool {
        matches!(self.community_role, Some(CommunityMemberRole::Owner))
    }

    /// Return whether this principal is a known managed Agent.
    #[must_use]
    pub const fn is_managed_agent(&self) -> bool {
        self.managed_agent_owner.is_some()
    }
}

/// Governance-relevant state of one Project Role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleSlot {
    /// Stable Project Role identifier.
    pub role_id: Uuid,
    /// Community permission level granted by an active Assignment.
    pub level: RoleLevel,
    /// Whether the Role may receive an Assignment.
    pub active: bool,
}

/// Complete v2 projection of one canonical Project Role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleDefinition {
    /// Stable Project Role identifier.
    pub role_id: Uuid,
    /// Human-readable Role name.
    pub name: String,
    /// Why this responsibility position exists.
    pub purpose: String,
    /// Responsibilities owned by the Role.
    pub responsibilities: Vec<String>,
    /// Explicit Role boundaries.
    pub boundaries: Vec<String>,
    /// Community permission level granted by an active Assignment.
    pub level: RoleLevel,
    /// Whether the Role may receive an Assignment.
    pub active: bool,
    /// Canonical Project View object revision.
    pub object_revision: u64,
    /// Project revision at which the Role was last changed.
    pub project_revision: u64,
    /// Canonical creation time.
    pub created_at: DateTime<Utc>,
    /// Canonical update time.
    pub updated_at: DateTime<Utc>,
    /// Verified Role creator.
    pub created_by: PublicKey,
    /// Verified latest Role editor.
    pub updated_by: PublicKey,
}

/// How a Role Assignment Proposal was initiated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalType {
    /// Candidate asks the Project to authorize an Assignment.
    Request,
    /// Project governor offers an already-authorized Assignment.
    Offer,
}

impl ProposalType {
    /// Return the stable database and wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Offer => "offer",
        }
    }
}

/// Durable Proposal lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    /// Waiting for candidate acceptance, Project authorization, or both.
    Open,
    /// Both confirmations completed and an Assignment was created.
    Consumed,
    /// Explicitly rejected.
    Rejected,
    /// Withdrawn by its creator.
    Withdrawn,
    /// Canonical Relay time passed its deadline.
    Expired,
}

impl ProposalStatus {
    /// Return the stable database and wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Consumed => "consumed",
            Self::Rejected => "rejected",
            Self::Withdrawn => "withdrawn",
            Self::Expired => "expired",
        }
    }
}

/// One Role Assignment Proposal and its two independent confirmations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleAssignmentProposal {
    /// Stable Proposal identifier.
    pub proposal_id: Uuid,
    /// Target Role.
    pub role_id: Uuid,
    /// Candidate who may receive the Assignment.
    pub candidate_pubkey: PublicKey,
    /// Request or offer origin.
    pub proposal_type: ProposalType,
    /// Canonical time at which the candidate accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_accepted_at: Option<DateTime<Utc>>,
    /// Governor whose authorization must still be valid at completion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorized_by: Option<PublicKey>,
    /// Canonical time of Project authorization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorized_at: Option<DateTime<Utc>>,
    /// Target Role Assignment observed when the full move was proposed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_target_assignment_id: Option<Uuid>,
    /// Candidate's other Assignment observed when the full move was proposed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_candidate_assignment_id: Option<Uuid>,
    /// Canonical deadline.
    pub expires_at: DateTime<Utc>,
    /// Durable lifecycle state.
    pub status: ProposalStatus,
    /// Optional rejection or withdrawal explanation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Verified creator.
    pub created_by: PublicKey,
    /// Canonical creation time.
    pub created_at: DateTime<Utc>,
    /// Canonical terminal time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<DateTime<Utc>>,
    /// Per-entity revision.
    pub entity_revision: u64,
    /// Project revision at which this version was written.
    pub project_revision: u64,
}

impl RoleAssignmentProposal {
    /// Return the status readers must expose at `canonical_time`, even before
    /// a cleanup transaction materializes expiration.
    #[must_use]
    pub fn effective_status(&self, canonical_time: DateTime<Utc>) -> ProposalStatus {
        if self.status == ProposalStatus::Open && canonical_time >= self.expires_at {
            ProposalStatus::Expired
        } else {
            self.status
        }
    }
}

/// Why an immutable Assignment tenure ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentEndReason {
    /// Explicit governance revocation.
    Revoked,
    /// Another Assignment superseded this tenure.
    Replaced,
    /// Trusted recovery concluded that the assignee cannot recover.
    Unrecoverable,
    /// Community membership was explicitly ended.
    MembershipEnded,
    /// The governing Role was deactivated.
    RoleDeactivated,
}

impl AssignmentEndReason {
    /// Return the stable database and wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Revoked => "revoked",
            Self::Replaced => "replaced",
            Self::Unrecoverable => "unrecoverable",
            Self::MembershipEnded => "membership_ended",
            Self::RoleDeactivated => "role_deactivated",
        }
    }
}

/// One immutable Role tenure. Updates only add lifecycle facts; an ended
/// Assignment can never become active again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleAssignment {
    /// Stable Assignment identifier.
    pub assignment_id: Uuid,
    /// Assigned Role.
    pub role_id: Uuid,
    /// Assignee.
    pub member_pubkey: PublicKey,
    /// Proposal that activated this tenure.
    pub proposal_id: Uuid,
    /// Canonical activation time.
    pub started_at: DateTime<Utc>,
    /// Governor whose authorization activated the tenure.
    pub started_by: PublicKey,
    /// Canonical replacement request time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement_requested_at: Option<DateTime<Utc>>,
    /// Optional replacement request context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement_request_reason: Option<String>,
    /// Canonical unable-to-continue report time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unable_reported_at: Option<DateTime<Utc>>,
    /// Optional unable-to-continue context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unable_report_reason: Option<String>,
    /// Canonical end time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    /// Governor that ended the tenure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_by: Option<PublicKey>,
    /// Terminal reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_reason: Option<AssignmentEndReason>,
    /// Assignment that superseded this tenure, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced_by_assignment_id: Option<Uuid>,
    /// Per-entity revision.
    pub entity_revision: u64,
    /// Project revision at which this version was written.
    pub project_revision: u64,
}

impl RoleAssignment {
    /// Return whether this tenure is still active.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.ended_at.is_none()
    }
}

/// Current responsibility relation and lifecycle facts for one Work object.
///
/// `status = None` represents a retained Work tombstone. Tombstones preserve
/// historical Commitment foreign keys but cannot receive responsibility or a
/// new Commitment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkResponsibility {
    /// Stable Work object identifier.
    pub work_id: Uuid,
    /// Current Work status, absent after deletion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<WorkStatus>,
    /// Stable Role responsible for this Work across Assignment replacement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responsible_role_id: Option<Uuid>,
    /// Canonical Project View object revision.
    pub object_revision: u64,
    /// Project revision at which responsibility or the Work last changed.
    pub project_revision: u64,
    /// Canonical update time.
    pub updated_at: DateTime<Utc>,
    /// Verified latest Work editor.
    pub updated_by: PublicKey,
}

impl WorkResponsibility {
    /// Return whether the Work can still be executed.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        matches!(
            self.status,
            Some(
                WorkStatus::Pending
                    | WorkStatus::InProgress
                    | WorkStatus::Paused
                    | WorkStatus::Submitted
            )
        )
    }
}

/// Why one Work Commitment ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitmentEndReason {
    /// The assignee explicitly released the Work.
    Released,
    /// A new Commitment superseded this one atomically.
    Replaced,
    /// The owning Assignment ended.
    AssignmentEnded,
    /// The Work reached a terminal lifecycle state.
    WorkClosed,
}

impl CommitmentEndReason {
    /// Return the stable database and wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Released => "released",
            Self::Replaced => "replaced",
            Self::AssignmentEnded => "assignment_ended",
            Self::WorkClosed => "work_closed",
        }
    }
}

/// One immutable-attribution commitment by a concrete Assignment to a Work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkCommitment {
    /// Stable Commitment identifier.
    pub commitment_id: Uuid,
    /// Work being accepted.
    pub work_id: Uuid,
    /// Assignment through which the Member accepted the Work.
    pub assignment_id: Uuid,
    /// Member attributed with this Commitment.
    pub member_pubkey: PublicKey,
    /// Canonical acceptance time.
    pub started_at: DateTime<Utc>,
    /// Signer that accepted the Work.
    pub started_by: PublicKey,
    /// Canonical terminal time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    /// Signer or governor that ended the Commitment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_by: Option<PublicKey>,
    /// Stable terminal cause.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_reason: Option<CommitmentEndReason>,
    /// Per-entity revision.
    pub entity_revision: u64,
    /// Project revision at which this version was written.
    pub project_revision: u64,
}

impl WorkCommitment {
    /// Return whether this Commitment is current.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.ended_at.is_none()
    }
}

/// Minimal system-generated record created whenever an Assignment is
/// replaced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleHandoff {
    /// Stable Handoff identifier.
    pub handoff_id: Uuid,
    /// Role whose tenure ended.
    pub role_id: Uuid,
    /// Ended Assignment.
    pub from_assignment_id: Uuid,
    /// New Assignment when the handoff is directly into the same Role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_assignment_id: Option<Uuid>,
    /// Work Commitments ended with the old tenure.
    pub affected_commitment_ids: Vec<Uuid>,
    /// Stable cause.
    pub cause: AssignmentEndReason,
    /// Canonical creation time.
    pub created_at: DateTime<Utc>,
    /// Per-entity revision; Handoffs are append-only and therefore always one.
    pub entity_revision: u64,
    /// Project revision allocated to the replacement.
    pub project_revision: u64,
}

/// Closed v2 Role command envelope carried by kind `44300`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleCommand {
    /// Must be `2`.
    pub schema_version: u16,
    /// Project revision on which the intent was based.
    pub expected_project_revision: u64,
    /// Active tenure from which a role-bearing action is performed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acting_assignment_id: Option<Uuid>,
    /// Closed operation payload.
    pub request: RoleCommandRequest,
}

impl RoleCommand {
    /// Construct a v2 command.
    #[must_use]
    pub const fn new(
        expected_project_revision: u64,
        acting_assignment_id: Option<Uuid>,
        request: RoleCommandRequest,
    ) -> Self {
        Self {
            schema_version: SchemaVersion::V2.as_u16(),
            expected_project_revision,
            acting_assignment_id,
            request,
        }
    }

    /// Parse a closed command from JSON and apply submission-shape checks.
    pub fn from_json(json: &str) -> Result<Self, RoleContinuityError> {
        let command: Self = serde_json::from_str(json)
            .map_err(|error| RoleContinuityError::InvalidCommand(error.to_string()))?;
        command.validate_for_submission()?;
        Ok(command)
    }

    /// Validate command fields that do not require canonical state.
    pub fn validate_for_submission(&self) -> Result<(), RoleContinuityError> {
        if self.schema_version != SchemaVersion::V2.as_u16() {
            return Err(RoleContinuityError::UnsupportedSchema);
        }
        if self.expected_project_revision == 0 || self.expected_project_revision > MAX_SAFE_REVISION
        {
            return Err(RoleContinuityError::InvalidCommand(
                "expected_project_revision must be in the JavaScript-safe positive range"
                    .to_owned(),
            ));
        }
        if self
            .acting_assignment_id
            .is_some_and(|assignment_id| assignment_id.is_nil())
        {
            return Err(RoleContinuityError::InvalidCommand(
                "acting_assignment_id cannot be nil".to_owned(),
            ));
        }
        match &self.request {
            RoleCommandRequest::RequestRole {
                proposal_id,
                role_id,
                reason,
                ..
            }
            | RoleCommandRequest::OfferRole {
                proposal_id,
                role_id,
                reason,
                ..
            } => {
                require_id(*proposal_id, "proposal_id")?;
                require_id(*role_id, "role_id")?;
                validate_optional_reason(reason)?;
            }
            RoleCommandRequest::AcceptProposal { proposal_id }
            | RoleCommandRequest::AuthorizeProposal { proposal_id }
            | RoleCommandRequest::ExpireProposal { proposal_id } => {
                require_id(*proposal_id, "proposal_id")?;
            }
            RoleCommandRequest::RejectProposal {
                proposal_id,
                reason,
            }
            | RoleCommandRequest::WithdrawProposal {
                proposal_id,
                reason,
            } => {
                require_id(*proposal_id, "proposal_id")?;
                validate_optional_reason(reason)?;
            }
            RoleCommandRequest::EndAssignment {
                assignment_id,
                reason,
            }
            | RoleCommandRequest::RequestReplacement {
                assignment_id,
                reason,
            }
            | RoleCommandRequest::ReportUnableToContinue {
                assignment_id,
                reason,
            } => {
                require_id(*assignment_id, "assignment_id")?;
                validate_optional_reason(reason)?;
            }
            RoleCommandRequest::SetWorkResponsibility {
                work_id,
                responsible_role_id,
            } => {
                require_id(*work_id, "work_id")?;
                if let Some(role_id) = responsible_role_id {
                    require_id(*role_id, "responsible_role_id")?;
                }
            }
            RoleCommandRequest::AcceptWork {
                commitment_id,
                work_id,
            } => {
                require_id(*commitment_id, "commitment_id")?;
                require_id(*work_id, "work_id")?;
            }
            RoleCommandRequest::EndCommitment {
                commitment_id,
                reason,
            } => {
                require_id(*commitment_id, "commitment_id")?;
                validate_optional_reason(reason)?;
            }
            RoleCommandRequest::ReplaceCommitment {
                commitment_id,
                work_id,
                expected_commitment_id,
            } => {
                require_id(*commitment_id, "commitment_id")?;
                require_id(*work_id, "work_id")?;
                require_id(*expected_commitment_id, "expected_commitment_id")?;
                if commitment_id == expected_commitment_id {
                    return Err(RoleContinuityError::InvalidCommand(
                        "replacement Commitment must use a new identifier".to_owned(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Stable operation name used by receipts and metrics.
    #[must_use]
    pub const fn operation(&self) -> &'static str {
        self.request.operation()
    }
}

/// Closed Role continuity operation set through stage 5.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RoleCommandRequest {
    /// Candidate requests a Role; candidate acceptance is implicit.
    RequestRole {
        /// Client-generated Proposal UUID.
        proposal_id: Uuid,
        /// Desired Role.
        role_id: Uuid,
        /// Signed absolute deadline.
        expires_at: DateTime<Utc>,
        /// Optional context.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Governor offers a Role; Project authorization is implicit.
    OfferRole {
        /// Client-generated Proposal UUID.
        proposal_id: Uuid,
        /// Offered Role.
        role_id: Uuid,
        /// Candidate public key.
        candidate_pubkey: PublicKey,
        /// Signed absolute deadline.
        expires_at: DateTime<Utc>,
        /// Optional context.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Candidate confirms an offer.
    AcceptProposal {
        /// Proposal being accepted.
        proposal_id: Uuid,
    },
    /// Candidate or governor rejects a Proposal.
    RejectProposal {
        /// Proposal being rejected.
        proposal_id: Uuid,
        /// Optional explanation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Creator withdraws an open Proposal.
    WithdrawProposal {
        /// Proposal being withdrawn.
        proposal_id: Uuid,
        /// Optional explanation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Materialize an already effective expiration.
    ExpireProposal {
        /// Proposal being expired.
        proposal_id: Uuid,
    },
    /// Governor authorizes a candidate request.
    AuthorizeProposal {
        /// Proposal being authorized.
        proposal_id: Uuid,
    },
    /// Governor explicitly ends an Assignment.
    EndAssignment {
        /// Active Assignment to end.
        assignment_id: Uuid,
        /// Optional governance explanation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Assignee asks governance to arrange a replacement without self-ending.
    RequestReplacement {
        /// Caller's active Assignment.
        assignment_id: Uuid,
        /// Optional context.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Assignee reports that it cannot continue without self-ending.
    ReportUnableToContinue {
        /// Caller's active Assignment.
        assignment_id: Uuid,
        /// Optional context.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Assign or clear the stable Role responsible for one Work.
    SetWorkResponsibility {
        /// Target Work.
        work_id: Uuid,
        /// New responsible Role, or `null` to leave the Work unassigned.
        responsible_role_id: Option<Uuid>,
    },
    /// Accept one Work through the signer's current Assignment.
    AcceptWork {
        /// Client-generated Commitment UUID.
        commitment_id: Uuid,
        /// Work being accepted.
        work_id: Uuid,
    },
    /// Release the signer's active Commitment without changing Work status.
    EndCommitment {
        /// Active Commitment to end.
        commitment_id: Uuid,
        /// Optional release context retained by the signed command.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Atomically supersede the signer's current Commitment to the same Work.
    ReplaceCommitment {
        /// Client-generated replacement Commitment UUID.
        commitment_id: Uuid,
        /// Work being recommitted.
        work_id: Uuid,
        /// Active Commitment observed by the caller.
        expected_commitment_id: Uuid,
    },
}

impl RoleCommandRequest {
    /// Stable operation spelling.
    #[must_use]
    pub const fn operation(&self) -> &'static str {
        match self {
            Self::RequestRole { .. } => "request_role",
            Self::OfferRole { .. } => "offer_role",
            Self::AcceptProposal { .. } => "accept_proposal",
            Self::RejectProposal { .. } => "reject_proposal",
            Self::WithdrawProposal { .. } => "withdraw_proposal",
            Self::ExpireProposal { .. } => "expire_proposal",
            Self::AuthorizeProposal { .. } => "authorize_proposal",
            Self::EndAssignment { .. } => "end_assignment",
            Self::RequestReplacement { .. } => "request_replacement",
            Self::ReportUnableToContinue { .. } => "report_unable_to_continue",
            Self::SetWorkResponsibility { .. } => "set_work_responsibility",
            Self::AcceptWork { .. } => "accept_work",
            Self::EndCommitment { .. } => "end_commitment",
            Self::ReplaceCommitment { .. } => "replace_commitment",
        }
    }
}

/// Relay-generated identifiers needed by a deterministic reduction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedRoleContinuityIds {
    /// Assignment ID to use if this command completes a Proposal.
    pub assignment_id: Uuid,
    /// Handoff IDs, consumed in deterministic ended-Assignment order.
    pub handoff_ids: Vec<Uuid>,
}

/// Canonical in-memory state required by Role continuity commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleContinuityState {
    project_revision: u64,
    roles: BTreeMap<Uuid, RoleSlot>,
    works: BTreeMap<Uuid, WorkResponsibility>,
    members: BTreeMap<PublicKey, MemberGovernance>,
    proposals: BTreeMap<Uuid, RoleAssignmentProposal>,
    assignments: BTreeMap<Uuid, RoleAssignment>,
    commitments: BTreeMap<Uuid, WorkCommitment>,
    handoffs: BTreeMap<Uuid, RoleHandoff>,
}

impl RoleContinuityState {
    /// Reconstruct state loaded under the Community Project lock.
    pub fn from_snapshot(
        project_revision: u64,
        roles: Vec<RoleSlot>,
        members: Vec<MemberGovernance>,
        proposals: Vec<RoleAssignmentProposal>,
        assignments: Vec<RoleAssignment>,
        handoffs: Vec<RoleHandoff>,
    ) -> Result<Self, RoleContinuityError> {
        Self::from_complete_snapshot(
            project_revision,
            roles,
            Vec::new(),
            members,
            proposals,
            assignments,
            Vec::new(),
            handoffs,
        )
    }

    /// Reconstruct the complete stage-5 state loaded under the Community
    /// Project lock.
    #[allow(clippy::too_many_arguments)]
    pub fn from_complete_snapshot(
        project_revision: u64,
        roles: Vec<RoleSlot>,
        works: Vec<WorkResponsibility>,
        members: Vec<MemberGovernance>,
        proposals: Vec<RoleAssignmentProposal>,
        assignments: Vec<RoleAssignment>,
        commitments: Vec<WorkCommitment>,
        handoffs: Vec<RoleHandoff>,
    ) -> Result<Self, RoleContinuityError> {
        let state = Self {
            project_revision,
            roles: collect_unique(roles, |role| role.role_id, "Role")?,
            works: collect_unique(works, |work| work.work_id, "Work")?,
            members: collect_unique(members, |member| member.pubkey, "Member")?,
            proposals: collect_unique(proposals, |proposal| proposal.proposal_id, "Proposal")?,
            assignments: collect_unique(
                assignments,
                |assignment| assignment.assignment_id,
                "Assignment",
            )?,
            commitments: collect_unique(
                commitments,
                |commitment| commitment.commitment_id,
                "Commitment",
            )?,
            handoffs: collect_unique(handoffs, |handoff| handoff.handoff_id, "Handoff")?,
        };
        state.validate()?;
        Ok(state)
    }

    /// Current project revision.
    #[must_use]
    pub const fn project_revision(&self) -> u64 {
        self.project_revision
    }

    /// All Proposals in stable ID order.
    pub fn proposals(&self) -> impl Iterator<Item = &RoleAssignmentProposal> {
        self.proposals.values()
    }

    /// All Assignments in stable ID order.
    pub fn assignments(&self) -> impl Iterator<Item = &RoleAssignment> {
        self.assignments.values()
    }

    /// All Work responsibility rows in stable Work ID order.
    pub fn works(&self) -> impl Iterator<Item = &WorkResponsibility> {
        self.works.values()
    }

    /// All Work Commitment heads in stable Commitment ID order.
    pub fn commitments(&self) -> impl Iterator<Item = &WorkCommitment> {
        self.commitments.values()
    }

    /// Re-run the current membership and active-Assignment security fence
    /// before consulting an idempotency receipt.
    ///
    /// This intentionally does not compare project revisions or mutate state:
    /// a successfully accepted old command may be replayed only while its
    /// signer and optional acting tenure remain eligible now.
    pub fn validate_actor_for_replay(
        &self,
        command: &RoleCommand,
        actor: PublicKey,
    ) -> Result<(), RoleContinuityError> {
        command.validate_for_submission()?;
        self.validate_actor_fence(command, actor)
    }

    /// Apply one signed command using Relay canonical time.
    pub fn reduce(
        &self,
        command: &RoleCommand,
        actor: PublicKey,
        canonical_time: DateTime<Utc>,
        generated_ids: &GeneratedRoleContinuityIds,
    ) -> Result<(Self, RoleContinuityOutcome), RoleContinuityError> {
        command.validate_for_submission()?;
        if command.expected_project_revision != self.project_revision {
            return Err(RoleContinuityError::RevisionConflict {
                expected: command.expected_project_revision,
                current: self.project_revision,
            });
        }
        let next_revision = self
            .project_revision
            .checked_add(1)
            .filter(|revision| *revision <= MAX_SAFE_REVISION)
            .ok_or(RoleContinuityError::RevisionOverflow)?;
        self.validate_actor_fence(command, actor)?;

        let mut next = self.clone();
        let before = self.entity_map();
        let before_works = self.works.clone();
        let mut membership_roles = BTreeMap::new();
        let mut affected_commitments = BTreeMap::<Uuid, Vec<Uuid>>::new();

        match &command.request {
            RoleCommandRequest::RequestRole {
                proposal_id,
                role_id,
                expires_at,
                reason,
            } => {
                next.create_proposal(
                    *proposal_id,
                    *role_id,
                    actor,
                    ProposalType::Request,
                    *expires_at,
                    reason.clone(),
                    actor,
                    canonical_time,
                    next_revision,
                    None,
                )?;
            }
            RoleCommandRequest::OfferRole {
                proposal_id,
                role_id,
                candidate_pubkey,
                expires_at,
                reason,
            } => {
                next.require_governor_fence(command, actor)?;
                next.create_proposal(
                    *proposal_id,
                    *role_id,
                    *candidate_pubkey,
                    ProposalType::Offer,
                    *expires_at,
                    reason.clone(),
                    actor,
                    canonical_time,
                    next_revision,
                    Some(actor),
                )?;
                next.complete_proposal_if_ready(
                    *proposal_id,
                    canonical_time,
                    next_revision,
                    generated_ids,
                    &mut membership_roles,
                    &mut affected_commitments,
                )?;
            }
            RoleCommandRequest::AcceptProposal { proposal_id } => {
                let proposal = next.open_proposal(*proposal_id, canonical_time)?.clone();
                if proposal.candidate_pubkey != actor {
                    return Err(RoleContinuityError::CandidateRequired);
                }
                if proposal.candidate_accepted_at.is_some() {
                    return Err(RoleContinuityError::AlreadyConfirmed);
                }
                // An offer's stored authorization is deliberately revalidated
                // before candidate acceptance is written.
                if let Some(authorizer) = proposal.authorized_by {
                    next.authorize_complete_move(&proposal, authorizer)?;
                }
                let proposal = next
                    .proposals
                    .get_mut(proposal_id)
                    .ok_or(RoleContinuityError::ProposalNotFound)?;
                proposal.candidate_accepted_at = Some(canonical_time);
                touch_proposal(proposal, next_revision)?;
                next.complete_proposal_if_ready(
                    *proposal_id,
                    canonical_time,
                    next_revision,
                    generated_ids,
                    &mut membership_roles,
                    &mut affected_commitments,
                )?;
            }
            RoleCommandRequest::AuthorizeProposal { proposal_id } => {
                next.require_governor_fence(command, actor)?;
                let proposal = next.open_proposal(*proposal_id, canonical_time)?.clone();
                next.authorize_complete_move(&proposal, actor)?;
                if proposal.authorized_at.is_some() {
                    return Err(RoleContinuityError::AlreadyConfirmed);
                }
                let proposal = next
                    .proposals
                    .get_mut(proposal_id)
                    .ok_or(RoleContinuityError::ProposalNotFound)?;
                proposal.authorized_by = Some(actor);
                proposal.authorized_at = Some(canonical_time);
                touch_proposal(proposal, next_revision)?;
                next.complete_proposal_if_ready(
                    *proposal_id,
                    canonical_time,
                    next_revision,
                    generated_ids,
                    &mut membership_roles,
                    &mut affected_commitments,
                )?;
            }
            RoleCommandRequest::RejectProposal {
                proposal_id,
                reason,
            } => {
                let proposal = next.open_proposal(*proposal_id, canonical_time)?.clone();
                if proposal.candidate_pubkey != actor {
                    next.require_governor_fence(command, actor)?;
                    next.authorize_complete_move(&proposal, actor)?;
                }
                resolve_proposal(
                    next.proposals
                        .get_mut(proposal_id)
                        .ok_or(RoleContinuityError::ProposalNotFound)?,
                    ProposalStatus::Rejected,
                    reason.clone(),
                    canonical_time,
                    next_revision,
                )?;
            }
            RoleCommandRequest::WithdrawProposal {
                proposal_id,
                reason,
            } => {
                let proposal = next.open_proposal(*proposal_id, canonical_time)?;
                if proposal.created_by != actor {
                    return Err(RoleContinuityError::CreatorRequired);
                }
                resolve_proposal(
                    next.proposals
                        .get_mut(proposal_id)
                        .ok_or(RoleContinuityError::ProposalNotFound)?,
                    ProposalStatus::Withdrawn,
                    reason.clone(),
                    canonical_time,
                    next_revision,
                )?;
            }
            RoleCommandRequest::ExpireProposal { proposal_id } => {
                let proposal = next
                    .proposals
                    .get(proposal_id)
                    .ok_or(RoleContinuityError::ProposalNotFound)?;
                if proposal.status != ProposalStatus::Open {
                    return Err(RoleContinuityError::ProposalNotOpen);
                }
                if canonical_time < proposal.expires_at {
                    return Err(RoleContinuityError::ProposalNotExpired);
                }
                resolve_proposal(
                    next.proposals
                        .get_mut(proposal_id)
                        .ok_or(RoleContinuityError::ProposalNotFound)?,
                    ProposalStatus::Expired,
                    None,
                    canonical_time,
                    next_revision,
                )?;
            }
            RoleCommandRequest::EndAssignment { assignment_id, .. } => {
                let assignment = next.active_assignment(*assignment_id)?.clone();
                next.authorize_assignment_end(actor, &assignment)?;
                if next.assignment_end_requires_role_authority(actor, &assignment)? {
                    next.require_governor_fence(command, actor)?;
                }
                let commitment_ids = next.end_assignment(
                    *assignment_id,
                    actor,
                    AssignmentEndReason::Revoked,
                    None,
                    canonical_time,
                    next_revision,
                )?;
                affected_commitments.insert(*assignment_id, commitment_ids);
                next.record_desired_member_role(assignment.member_pubkey, &mut membership_roles)?;
            }
            RoleCommandRequest::RequestReplacement {
                assignment_id,
                reason,
            } => {
                next.require_assignee_action(command, actor, *assignment_id)?;
                let assignment = next
                    .assignments
                    .get_mut(assignment_id)
                    .ok_or(RoleContinuityError::AssignmentNotFound)?;
                if assignment.replacement_requested_at.is_some() {
                    return Err(RoleContinuityError::AlreadyReported);
                }
                assignment.replacement_requested_at = Some(canonical_time);
                assignment.replacement_request_reason = reason.clone();
                touch_assignment(assignment, next_revision)?;
            }
            RoleCommandRequest::ReportUnableToContinue {
                assignment_id,
                reason,
            } => {
                next.require_assignee_action(command, actor, *assignment_id)?;
                let assignment = next
                    .assignments
                    .get_mut(assignment_id)
                    .ok_or(RoleContinuityError::AssignmentNotFound)?;
                if assignment.unable_reported_at.is_some() {
                    return Err(RoleContinuityError::AlreadyReported);
                }
                assignment.unable_reported_at = Some(canonical_time);
                assignment.unable_report_reason = reason.clone();
                touch_assignment(assignment, next_revision)?;
            }
            RoleCommandRequest::SetWorkResponsibility {
                work_id,
                responsible_role_id,
            } => {
                next.require_governor_fence(command, actor)?;
                if next.active_commitment_for_work(*work_id).is_some() {
                    return Err(RoleContinuityError::ActiveCommitmentConflict);
                }
                if let Some(role_id) = responsible_role_id {
                    let role = next
                        .roles
                        .get(role_id)
                        .ok_or(RoleContinuityError::RoleNotFound)?;
                    if !role.active {
                        return Err(RoleContinuityError::RoleInactive);
                    }
                }
                let work = next
                    .works
                    .get_mut(work_id)
                    .filter(|work| work.status.is_some())
                    .ok_or(RoleContinuityError::WorkNotFound)?;
                if work.responsible_role_id == *responsible_role_id {
                    return Err(RoleContinuityError::ResponsibilityUnchanged);
                }
                work.responsible_role_id = *responsible_role_id;
                touch_work(work, actor, canonical_time, next_revision)?;
            }
            RoleCommandRequest::AcceptWork {
                commitment_id,
                work_id,
            } => {
                let assignment_id = command
                    .acting_assignment_id
                    .ok_or(RoleContinuityError::ActingAssignmentRequired)?;
                next.require_assignee_action(command, actor, assignment_id)?;
                next.create_commitment(
                    *commitment_id,
                    *work_id,
                    assignment_id,
                    actor,
                    canonical_time,
                    next_revision,
                )?;
            }
            RoleCommandRequest::EndCommitment { commitment_id, .. } => {
                let assignment_id = command
                    .acting_assignment_id
                    .ok_or(RoleContinuityError::ActingAssignmentRequired)?;
                next.require_assignee_action(command, actor, assignment_id)?;
                let commitment = next.active_commitment(*commitment_id)?.clone();
                if commitment.assignment_id != assignment_id || commitment.member_pubkey != actor {
                    return Err(RoleContinuityError::CommitmentAssigneeRequired);
                }
                next.end_commitment(
                    *commitment_id,
                    actor,
                    CommitmentEndReason::Released,
                    canonical_time,
                    next_revision,
                )?;
            }
            RoleCommandRequest::ReplaceCommitment {
                commitment_id,
                work_id,
                expected_commitment_id,
            } => {
                let assignment_id = command
                    .acting_assignment_id
                    .ok_or(RoleContinuityError::ActingAssignmentRequired)?;
                next.require_assignee_action(command, actor, assignment_id)?;
                let current = next.active_commitment(*expected_commitment_id)?.clone();
                if current.work_id != *work_id
                    || current.assignment_id != assignment_id
                    || current.member_pubkey != actor
                {
                    return Err(RoleContinuityError::CommitmentFenceConflict);
                }
                next.end_commitment(
                    *expected_commitment_id,
                    actor,
                    CommitmentEndReason::Replaced,
                    canonical_time,
                    next_revision,
                )?;
                next.create_commitment(
                    *commitment_id,
                    *work_id,
                    assignment_id,
                    actor,
                    canonical_time,
                    next_revision,
                )?;
            }
        }

        next.project_revision = next_revision;
        next.validate()?;
        let changes = changed_entities(before, &next.entity_map());
        let work_changes = changed_works(before_works, &next.works);
        if changes.is_empty() && work_changes.is_empty() {
            return Err(RoleContinuityError::InvalidState(
                "accepted command produced no canonical change".to_owned(),
            ));
        }
        Ok((
            next,
            RoleContinuityOutcome {
                project_revision: next_revision,
                changes,
                work_changes,
                membership_roles,
                ended_commitments: affected_commitments,
            },
        ))
    }

    fn validate(&self) -> Result<(), RoleContinuityError> {
        if self.project_revision == 0 || self.project_revision > MAX_SAFE_REVISION {
            return Err(RoleContinuityError::InvalidState(
                "v2 Role continuity requires an initialized safe project revision".to_owned(),
            ));
        }
        let mut active_roles = BTreeSet::new();
        let mut active_members = BTreeSet::new();
        for assignment in self.assignments.values() {
            let role = self
                .roles
                .get(&assignment.role_id)
                .ok_or(RoleContinuityError::RoleNotFound)?;
            if assignment.is_active() {
                if !role.active {
                    return Err(RoleContinuityError::RoleInactive);
                }
                if !active_roles.insert(assignment.role_id) {
                    return Err(RoleContinuityError::InvalidState(
                        "a Role has multiple active Assignments".to_owned(),
                    ));
                }
                if !active_members.insert(assignment.member_pubkey) {
                    return Err(RoleContinuityError::InvalidState(
                        "a Member has multiple active Assignments".to_owned(),
                    ));
                }
                let member = self
                    .members
                    .get(&assignment.member_pubkey)
                    .filter(|member| member.eligible && member.community_role.is_some())
                    .ok_or(RoleContinuityError::CandidateIneligible)?;
                if !member.is_owner()
                    && member.community_role.map(CommunityMemberRole::as_str)
                        != Some(role.level.as_str())
                {
                    return Err(RoleContinuityError::InvalidState(
                        "active Assignment and Community role disagree".to_owned(),
                    ));
                }
            } else if assignment.ended_by.is_none() || assignment.ended_reason.is_none() {
                return Err(RoleContinuityError::InvalidState(
                    "ended Assignment is missing terminal attribution".to_owned(),
                ));
            }
        }
        for work in self.works.values() {
            if let Some(role_id) = work.responsible_role_id {
                let role = self
                    .roles
                    .get(&role_id)
                    .ok_or(RoleContinuityError::RoleNotFound)?;
                if work.status.is_none() || !role.active {
                    return Err(RoleContinuityError::InvalidState(
                        "responsible Work must be active and reference an active Role".to_owned(),
                    ));
                }
            }
        }
        let mut active_work = BTreeSet::new();
        for commitment in self.commitments.values() {
            let assignment = self
                .assignments
                .get(&commitment.assignment_id)
                .ok_or(RoleContinuityError::AssignmentNotFound)?;
            let work = self
                .works
                .get(&commitment.work_id)
                .ok_or(RoleContinuityError::WorkNotFound)?;
            if commitment.member_pubkey != assignment.member_pubkey {
                return Err(RoleContinuityError::InvalidState(
                    "Commitment Member differs from its Assignment".to_owned(),
                ));
            }
            if commitment.is_active() {
                if !assignment.is_active() {
                    return Err(RoleContinuityError::InvalidState(
                        "ended Assignment retains an active Commitment".to_owned(),
                    ));
                }
                if !work.is_open() {
                    return Err(RoleContinuityError::WorkClosed);
                }
                if work.responsible_role_id != Some(assignment.role_id) {
                    return Err(RoleContinuityError::WorkRoleMismatch);
                }
                if !active_work.insert(commitment.work_id) {
                    return Err(RoleContinuityError::InvalidState(
                        "a Work has multiple active Commitments".to_owned(),
                    ));
                }
            } else if commitment.ended_by.is_none() || commitment.ended_reason.is_none() {
                return Err(RoleContinuityError::InvalidState(
                    "ended Commitment is missing terminal attribution".to_owned(),
                ));
            }
        }
        Ok(())
    }

    fn validate_actor_fence(
        &self,
        command: &RoleCommand,
        actor: PublicKey,
    ) -> Result<(), RoleContinuityError> {
        let member = self
            .members
            .get(&actor)
            .filter(|member| member.eligible)
            .ok_or(RoleContinuityError::CandidateIneligible)?;
        if let Some(assignment_id) = command.acting_assignment_id {
            let assignment = self
                .assignments
                .get(&assignment_id)
                .ok_or(RoleContinuityError::ActingAssignmentInvalid)?;
            if !assignment.is_active() || assignment.member_pubkey != actor {
                return Err(RoleContinuityError::ActingAssignmentInvalid);
            }
        }

        let active = self.active_assignment_for_member(actor);
        let role_action_requires_fence = matches!(
            &command.request,
            RoleCommandRequest::RequestReplacement { .. }
                | RoleCommandRequest::ReportUnableToContinue { .. }
                | RoleCommandRequest::AcceptWork { .. }
                | RoleCommandRequest::EndCommitment { .. }
                | RoleCommandRequest::ReplaceCommitment { .. }
        );
        if role_action_requires_fence && command.acting_assignment_id.is_none() {
            return Err(RoleContinuityError::ActingAssignmentRequired);
        }
        if member.is_managed_agent()
            && active.is_some()
            && command.acting_assignment_id.is_none()
            && !matches!(
                &command.request,
                RoleCommandRequest::AcceptProposal { .. }
                    | RoleCommandRequest::RejectProposal { .. }
                    | RoleCommandRequest::WithdrawProposal { .. }
            )
        {
            return Err(RoleContinuityError::ActingAssignmentRequired);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn create_proposal(
        &mut self,
        proposal_id: Uuid,
        role_id: Uuid,
        candidate: PublicKey,
        proposal_type: ProposalType,
        expires_at: DateTime<Utc>,
        reason: Option<String>,
        actor: PublicKey,
        canonical_time: DateTime<Utc>,
        next_revision: u64,
        authorizer: Option<PublicKey>,
    ) -> Result<(), RoleContinuityError> {
        if self.proposals.contains_key(&proposal_id) {
            return Err(RoleContinuityError::IdCollision);
        }
        let role = self
            .roles
            .get(&role_id)
            .ok_or(RoleContinuityError::RoleNotFound)?;
        if !role.active {
            return Err(RoleContinuityError::RoleInactive);
        }
        if expires_at <= canonical_time
            || expires_at > canonical_time + Duration::days(MAX_PROPOSAL_LIFETIME_DAYS)
        {
            return Err(RoleContinuityError::InvalidProposalDeadline);
        }
        self.members
            .get(&candidate)
            .filter(|member| member.eligible)
            .ok_or(RoleContinuityError::CandidateIneligible)?;
        if self.proposals.values().any(|proposal| {
            proposal.status == ProposalStatus::Open
                && proposal.role_id == role_id
                && proposal.candidate_pubkey == candidate
                && canonical_time < proposal.expires_at
        }) {
            return Err(RoleContinuityError::DuplicateProposal);
        }

        let target_assignment = self.active_assignment_for_role(role_id);
        let candidate_assignment = self
            .active_assignment_for_member(candidate)
            .filter(|assignment_id| Some(*assignment_id) != target_assignment);
        let proposal = RoleAssignmentProposal {
            proposal_id,
            role_id,
            candidate_pubkey: candidate,
            proposal_type,
            candidate_accepted_at: (proposal_type == ProposalType::Request)
                .then_some(canonical_time),
            authorized_by: authorizer,
            authorized_at: authorizer.map(|_| canonical_time),
            expected_target_assignment_id: target_assignment,
            expected_candidate_assignment_id: candidate_assignment,
            expires_at,
            status: ProposalStatus::Open,
            reason,
            created_by: actor,
            created_at: canonical_time,
            resolved_at: None,
            entity_revision: 1,
            project_revision: next_revision,
        };
        if authorizer.is_some() {
            self.authorize_complete_move(&proposal, actor)?;
        }
        self.proposals.insert(proposal_id, proposal);
        Ok(())
    }

    fn complete_proposal_if_ready(
        &mut self,
        proposal_id: Uuid,
        canonical_time: DateTime<Utc>,
        next_revision: u64,
        generated_ids: &GeneratedRoleContinuityIds,
        membership_roles: &mut BTreeMap<PublicKey, CommunityMemberRole>,
        affected_commitments: &mut BTreeMap<Uuid, Vec<Uuid>>,
    ) -> Result<(), RoleContinuityError> {
        let proposal = self.open_proposal(proposal_id, canonical_time)?.clone();
        let (Some(_), Some(authorizer)) = (proposal.candidate_accepted_at, proposal.authorized_by)
        else {
            return Ok(());
        };
        self.authorize_complete_move(&proposal, authorizer)?;
        self.validate_compound_fence(&proposal)?;
        require_id(generated_ids.assignment_id, "generated assignment_id")?;
        if self.assignments.contains_key(&generated_ids.assignment_id) {
            return Err(RoleContinuityError::IdCollision);
        }

        let mut ended_ids = [
            proposal.expected_target_assignment_id,
            proposal.expected_candidate_assignment_id,
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        ended_ids.sort_unstable();
        ended_ids.dedup();
        if generated_ids.handoff_ids.len() < ended_ids.len() {
            return Err(RoleContinuityError::MissingGeneratedId);
        }
        let handoff_ids = generated_ids
            .handoff_ids
            .iter()
            .copied()
            .take(ended_ids.len())
            .collect::<Vec<_>>();
        let unique_handoff_ids = handoff_ids.iter().copied().collect::<BTreeSet<_>>();
        if unique_handoff_ids.len() != handoff_ids.len()
            || handoff_ids
                .iter()
                .any(|id| id.is_nil() || self.handoffs.contains_key(id))
        {
            return Err(RoleContinuityError::IdCollision);
        }

        for (assignment_id, handoff_id) in ended_ids.iter().zip(handoff_ids) {
            let old = self.active_assignment(*assignment_id)?.clone();
            let commitment_ids = self.end_assignment(
                *assignment_id,
                authorizer,
                AssignmentEndReason::Replaced,
                Some(generated_ids.assignment_id),
                canonical_time,
                next_revision,
            )?;
            let to_assignment_id =
                (old.role_id == proposal.role_id).then_some(generated_ids.assignment_id);
            affected_commitments.insert(*assignment_id, commitment_ids.clone());
            self.handoffs.insert(
                handoff_id,
                RoleHandoff {
                    handoff_id,
                    role_id: old.role_id,
                    from_assignment_id: old.assignment_id,
                    to_assignment_id,
                    affected_commitment_ids: commitment_ids,
                    cause: AssignmentEndReason::Replaced,
                    created_at: canonical_time,
                    entity_revision: 1,
                    project_revision: next_revision,
                },
            );
            self.record_desired_member_role(old.member_pubkey, membership_roles)?;
        }

        self.assignments.insert(
            generated_ids.assignment_id,
            RoleAssignment {
                assignment_id: generated_ids.assignment_id,
                role_id: proposal.role_id,
                member_pubkey: proposal.candidate_pubkey,
                proposal_id,
                started_at: canonical_time,
                started_by: authorizer,
                replacement_requested_at: None,
                replacement_request_reason: None,
                unable_reported_at: None,
                unable_report_reason: None,
                ended_at: None,
                ended_by: None,
                ended_reason: None,
                replaced_by_assignment_id: None,
                entity_revision: 1,
                project_revision: next_revision,
            },
        );
        let candidate_pubkey = proposal.candidate_pubkey;
        {
            let proposal = self
                .proposals
                .get_mut(&proposal_id)
                .ok_or(RoleContinuityError::ProposalNotFound)?;
            proposal.status = ProposalStatus::Consumed;
            proposal.resolved_at = Some(canonical_time);
            touch_proposal(proposal, next_revision)?;
        }
        self.record_desired_member_role(candidate_pubkey, membership_roles)?;
        Ok(())
    }

    fn authorize_complete_move(
        &self,
        proposal: &RoleAssignmentProposal,
        actor: PublicKey,
    ) -> Result<(), RoleContinuityError> {
        let actor_member = self
            .members
            .get(&actor)
            .filter(|member| member.eligible)
            .ok_or(RoleContinuityError::NotAuthorized)?;
        if actor_member.is_owner() {
            return Ok(());
        }
        let acting_assignment = self
            .active_assignment_for_member(actor)
            .and_then(|assignment| self.assignments.get(&assignment));
        let Some(acting_assignment) = acting_assignment else {
            return Err(RoleContinuityError::NotAuthorized);
        };
        let acting_role = self
            .roles
            .get(&acting_assignment.role_id)
            .ok_or(RoleContinuityError::RoleNotFound)?;
        if acting_role.level != RoleLevel::Admin
            || actor_member.community_role != Some(CommunityMemberRole::Admin)
        {
            return Err(RoleContinuityError::NotAuthorized);
        }
        let target_role = self
            .roles
            .get(&proposal.role_id)
            .ok_or(RoleContinuityError::RoleNotFound)?;
        if target_role.level == RoleLevel::Admin {
            return Err(RoleContinuityError::OwnerRequired);
        }

        for assignment_id in [
            proposal.expected_target_assignment_id,
            proposal.expected_candidate_assignment_id,
        ]
        .into_iter()
        .flatten()
        {
            let assignment = self.active_assignment(assignment_id)?;
            if assignment.member_pubkey == actor {
                return Err(RoleContinuityError::SelfEndForbidden);
            }
            let role = self
                .roles
                .get(&assignment.role_id)
                .ok_or(RoleContinuityError::RoleNotFound)?;
            if role.level == RoleLevel::Admin {
                return Err(RoleContinuityError::PeerLeaderForbidden);
            }
            if actor_member.is_managed_agent()
                && !self
                    .members
                    .get(&assignment.member_pubkey)
                    .is_some_and(MemberGovernance::is_managed_agent)
            {
                return Err(RoleContinuityError::ManagedLeaderTargetUnknown);
            }
        }
        Ok(())
    }

    fn authorize_assignment_end(
        &self,
        actor: PublicKey,
        target: &RoleAssignment,
    ) -> Result<(), RoleContinuityError> {
        if actor == target.member_pubkey {
            return Err(RoleContinuityError::SelfEndForbidden);
        }
        let actor_member = self
            .members
            .get(&actor)
            .filter(|member| member.eligible)
            .ok_or(RoleContinuityError::NotAuthorized)?;
        if actor_member.is_owner() {
            return Ok(());
        }
        let target_member = self
            .members
            .get(&target.member_pubkey)
            .ok_or(RoleContinuityError::CandidateIneligible)?;
        if target_member.managed_agent_owner == Some(actor) {
            return Ok(());
        }
        let actor_assignment = self
            .active_assignment_for_member(actor)
            .and_then(|id| self.assignments.get(&id))
            .ok_or(RoleContinuityError::NotAuthorized)?;
        let actor_role = self
            .roles
            .get(&actor_assignment.role_id)
            .ok_or(RoleContinuityError::RoleNotFound)?;
        let target_role = self
            .roles
            .get(&target.role_id)
            .ok_or(RoleContinuityError::RoleNotFound)?;
        if actor_role.level != RoleLevel::Admin
            || actor_member.community_role != Some(CommunityMemberRole::Admin)
        {
            return Err(RoleContinuityError::NotAuthorized);
        }
        if target_role.level == RoleLevel::Admin {
            return Err(RoleContinuityError::PeerLeaderForbidden);
        }
        if actor_member.is_managed_agent() && !target_member.is_managed_agent() {
            return Err(RoleContinuityError::ManagedLeaderTargetUnknown);
        }
        Ok(())
    }

    fn assignment_end_requires_role_authority(
        &self,
        actor: PublicKey,
        target: &RoleAssignment,
    ) -> Result<bool, RoleContinuityError> {
        let actor_member = self
            .members
            .get(&actor)
            .filter(|member| member.eligible)
            .ok_or(RoleContinuityError::NotAuthorized)?;
        if actor_member.is_owner() {
            return Ok(false);
        }
        let target_member = self
            .members
            .get(&target.member_pubkey)
            .ok_or(RoleContinuityError::CandidateIneligible)?;
        Ok(target_member.managed_agent_owner != Some(actor))
    }

    fn require_governor_fence(
        &self,
        command: &RoleCommand,
        actor: PublicKey,
    ) -> Result<(), RoleContinuityError> {
        let member = self
            .members
            .get(&actor)
            .filter(|member| member.eligible)
            .ok_or(RoleContinuityError::NotAuthorized)?;
        if member.is_owner() {
            return Ok(());
        }
        let active_assignment = self
            .active_assignment_for_member(actor)
            .ok_or(RoleContinuityError::ActingAssignmentRequired)?;
        if command.acting_assignment_id != Some(active_assignment) {
            return Err(if command.acting_assignment_id.is_some() {
                RoleContinuityError::ActingAssignmentInvalid
            } else {
                RoleContinuityError::ActingAssignmentRequired
            });
        }
        let assignment = self
            .assignments
            .get(&active_assignment)
            .ok_or(RoleContinuityError::ActingAssignmentInvalid)?;
        let role = self
            .roles
            .get(&assignment.role_id)
            .ok_or(RoleContinuityError::RoleNotFound)?;
        if member.community_role != Some(CommunityMemberRole::Admin)
            || role.level != RoleLevel::Admin
        {
            return Err(RoleContinuityError::NotAuthorized);
        }
        Ok(())
    }

    fn validate_compound_fence(
        &self,
        proposal: &RoleAssignmentProposal,
    ) -> Result<(), RoleContinuityError> {
        if self.active_assignment_for_role(proposal.role_id)
            != proposal.expected_target_assignment_id
        {
            return Err(RoleContinuityError::CompoundFenceConflict);
        }
        let candidate_assignment = self
            .active_assignment_for_member(proposal.candidate_pubkey)
            .filter(|id| Some(*id) != proposal.expected_target_assignment_id);
        if candidate_assignment != proposal.expected_candidate_assignment_id {
            return Err(RoleContinuityError::CompoundFenceConflict);
        }
        self.members
            .get(&proposal.candidate_pubkey)
            .filter(|member| member.eligible)
            .ok_or(RoleContinuityError::CandidateIneligible)?;
        Ok(())
    }

    fn end_assignment(
        &mut self,
        assignment_id: Uuid,
        actor: PublicKey,
        reason: AssignmentEndReason,
        replaced_by_assignment_id: Option<Uuid>,
        canonical_time: DateTime<Utc>,
        next_revision: u64,
    ) -> Result<Vec<Uuid>, RoleContinuityError> {
        let assignment = self
            .assignments
            .get_mut(&assignment_id)
            .ok_or(RoleContinuityError::AssignmentNotFound)?;
        if !assignment.is_active() {
            return Err(RoleContinuityError::AssignmentEnded);
        }
        assignment.ended_at = Some(canonical_time);
        assignment.ended_by = Some(actor);
        assignment.ended_reason = Some(reason);
        assignment.replaced_by_assignment_id = replaced_by_assignment_id;
        touch_assignment(assignment, next_revision)?;

        let commitment_ids = self
            .commitments
            .values()
            .filter(|commitment| {
                commitment.assignment_id == assignment_id && commitment.is_active()
            })
            .map(|commitment| commitment.commitment_id)
            .collect::<Vec<_>>();
        for commitment_id in &commitment_ids {
            self.end_commitment(
                *commitment_id,
                actor,
                CommitmentEndReason::AssignmentEnded,
                canonical_time,
                next_revision,
            )?;
        }
        Ok(commitment_ids)
    }

    fn create_commitment(
        &mut self,
        commitment_id: Uuid,
        work_id: Uuid,
        assignment_id: Uuid,
        actor: PublicKey,
        canonical_time: DateTime<Utc>,
        next_revision: u64,
    ) -> Result<(), RoleContinuityError> {
        if self.commitments.contains_key(&commitment_id) {
            return Err(RoleContinuityError::IdCollision);
        }
        if self.active_commitment_for_work(work_id).is_some() {
            return Err(RoleContinuityError::ActiveCommitmentConflict);
        }
        let assignment = self.active_assignment(assignment_id)?;
        if assignment.member_pubkey != actor {
            return Err(RoleContinuityError::CommitmentAssigneeRequired);
        }
        let work = self
            .works
            .get(&work_id)
            .filter(|work| work.status.is_some())
            .ok_or(RoleContinuityError::WorkNotFound)?;
        if !work.is_open() {
            return Err(RoleContinuityError::WorkClosed);
        }
        let Some(responsible_role_id) = work.responsible_role_id else {
            return Err(RoleContinuityError::ResponsibilityRequired);
        };
        if responsible_role_id != assignment.role_id {
            return Err(RoleContinuityError::WorkRoleMismatch);
        }
        self.commitments.insert(
            commitment_id,
            WorkCommitment {
                commitment_id,
                work_id,
                assignment_id,
                member_pubkey: actor,
                started_at: canonical_time,
                started_by: actor,
                ended_at: None,
                ended_by: None,
                ended_reason: None,
                entity_revision: 1,
                project_revision: next_revision,
            },
        );
        Ok(())
    }

    fn end_commitment(
        &mut self,
        commitment_id: Uuid,
        actor: PublicKey,
        reason: CommitmentEndReason,
        canonical_time: DateTime<Utc>,
        next_revision: u64,
    ) -> Result<(), RoleContinuityError> {
        let commitment = self
            .commitments
            .get_mut(&commitment_id)
            .ok_or(RoleContinuityError::CommitmentNotFound)?;
        if !commitment.is_active() {
            return Err(RoleContinuityError::CommitmentEnded);
        }
        commitment.ended_at = Some(canonical_time);
        commitment.ended_by = Some(actor);
        commitment.ended_reason = Some(reason);
        touch_commitment(commitment, next_revision)
    }

    fn require_assignee_action(
        &self,
        command: &RoleCommand,
        actor: PublicKey,
        assignment_id: Uuid,
    ) -> Result<(), RoleContinuityError> {
        if command.acting_assignment_id != Some(assignment_id) {
            return Err(RoleContinuityError::ActingAssignmentInvalid);
        }
        let assignment = self.active_assignment(assignment_id)?;
        if assignment.member_pubkey != actor {
            return Err(RoleContinuityError::AssigneeRequired);
        }
        Ok(())
    }

    fn open_proposal(
        &self,
        proposal_id: Uuid,
        canonical_time: DateTime<Utc>,
    ) -> Result<&RoleAssignmentProposal, RoleContinuityError> {
        let proposal = self
            .proposals
            .get(&proposal_id)
            .ok_or(RoleContinuityError::ProposalNotFound)?;
        if proposal.status != ProposalStatus::Open {
            return Err(RoleContinuityError::ProposalNotOpen);
        }
        if canonical_time >= proposal.expires_at {
            return Err(RoleContinuityError::ProposalExpired);
        }
        Ok(proposal)
    }

    fn active_assignment(
        &self,
        assignment_id: Uuid,
    ) -> Result<&RoleAssignment, RoleContinuityError> {
        self.assignments
            .get(&assignment_id)
            .ok_or(RoleContinuityError::AssignmentNotFound)
            .and_then(|assignment| {
                assignment
                    .is_active()
                    .then_some(assignment)
                    .ok_or(RoleContinuityError::AssignmentEnded)
            })
    }

    fn active_assignment_for_role(&self, role_id: Uuid) -> Option<Uuid> {
        self.assignments
            .values()
            .find(|assignment| assignment.role_id == role_id && assignment.is_active())
            .map(|assignment| assignment.assignment_id)
    }

    fn active_assignment_for_member(&self, member: PublicKey) -> Option<Uuid> {
        self.assignments
            .values()
            .find(|assignment| assignment.member_pubkey == member && assignment.is_active())
            .map(|assignment| assignment.assignment_id)
    }

    fn active_commitment(
        &self,
        commitment_id: Uuid,
    ) -> Result<&WorkCommitment, RoleContinuityError> {
        self.commitments
            .get(&commitment_id)
            .ok_or(RoleContinuityError::CommitmentNotFound)
            .and_then(|commitment| {
                commitment
                    .is_active()
                    .then_some(commitment)
                    .ok_or(RoleContinuityError::CommitmentEnded)
            })
    }

    fn active_commitment_for_work(&self, work_id: Uuid) -> Option<Uuid> {
        self.commitments
            .values()
            .find(|commitment| commitment.work_id == work_id && commitment.is_active())
            .map(|commitment| commitment.commitment_id)
    }

    fn desired_member_role(
        &self,
        member_pubkey: PublicKey,
    ) -> Result<CommunityMemberRole, RoleContinuityError> {
        let member = self
            .members
            .get(&member_pubkey)
            .ok_or(RoleContinuityError::CandidateIneligible)?;
        if member.is_owner() {
            return Ok(CommunityMemberRole::Owner);
        }
        let role = self
            .active_assignment_for_member(member_pubkey)
            .and_then(|assignment_id| self.assignments.get(&assignment_id))
            .and_then(|assignment| self.roles.get(&assignment.role_id));
        Ok(match role.map(|role| role.level) {
            Some(RoleLevel::Admin) => CommunityMemberRole::Admin,
            Some(RoleLevel::Member) | None => CommunityMemberRole::Member,
        })
    }

    fn record_desired_member_role(
        &mut self,
        member_pubkey: PublicKey,
        membership_roles: &mut BTreeMap<PublicKey, CommunityMemberRole>,
    ) -> Result<(), RoleContinuityError> {
        let role = self.desired_member_role(member_pubkey)?;
        let member = self
            .members
            .get_mut(&member_pubkey)
            .ok_or(RoleContinuityError::CandidateIneligible)?;
        member.community_role = Some(role);
        membership_roles.insert(member_pubkey, role);
        Ok(())
    }

    fn entity_map(&self) -> BTreeMap<(RoleContinuityEntity, Uuid), RoleContinuityChange> {
        let mut entities = BTreeMap::new();
        for proposal in self.proposals.values() {
            entities.insert(
                (
                    RoleContinuityEntity::RoleAssignmentProposal,
                    proposal.proposal_id,
                ),
                RoleContinuityChange::Proposal(proposal.clone()),
            );
        }
        for assignment in self.assignments.values() {
            entities.insert(
                (
                    RoleContinuityEntity::RoleAssignment,
                    assignment.assignment_id,
                ),
                RoleContinuityChange::Assignment(assignment.clone()),
            );
        }
        for commitment in self.commitments.values() {
            entities.insert(
                (
                    RoleContinuityEntity::WorkCommitment,
                    commitment.commitment_id,
                ),
                RoleContinuityChange::Commitment(commitment.clone()),
            );
        }
        for handoff in self.handoffs.values() {
            entities.insert(
                (RoleContinuityEntity::RoleHandoff, handoff.handoff_id),
                RoleContinuityChange::Handoff(handoff.clone()),
            );
        }
        entities
    }
}

/// Entity discriminator used by v2 projection coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleContinuityEntity {
    /// Canonical Project Role.
    Role,
    /// Role Assignment Proposal.
    RoleAssignmentProposal,
    /// Role Assignment tenure.
    RoleAssignment,
    /// Work Commitment tenure.
    WorkCommitment,
    /// Append-only Role Handoff.
    RoleHandoff,
}

impl RoleContinuityEntity {
    /// Stable coordinate and database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Role => "role",
            Self::RoleAssignmentProposal => "role_assignment_proposal",
            Self::RoleAssignment => "role_assignment",
            Self::WorkCommitment => "work_commitment",
            Self::RoleHandoff => "role_handoff",
        }
    }
}

/// One complete changed entity head.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "entity_type", content = "entity", rename_all = "snake_case")]
pub enum RoleContinuityChange {
    /// Canonical Project Role head.
    Role(RoleDefinition),
    /// Proposal head.
    Proposal(RoleAssignmentProposal),
    /// Assignment head.
    Assignment(RoleAssignment),
    /// Work Commitment head.
    Commitment(WorkCommitment),
    /// Handoff head.
    Handoff(RoleHandoff),
}

impl RoleContinuityChange {
    /// Entity type.
    #[must_use]
    pub const fn entity_type(&self) -> RoleContinuityEntity {
        match self {
            Self::Role(_) => RoleContinuityEntity::Role,
            Self::Proposal(_) => RoleContinuityEntity::RoleAssignmentProposal,
            Self::Assignment(_) => RoleContinuityEntity::RoleAssignment,
            Self::Commitment(_) => RoleContinuityEntity::WorkCommitment,
            Self::Handoff(_) => RoleContinuityEntity::RoleHandoff,
        }
    }

    /// Stable entity ID.
    #[must_use]
    pub const fn entity_id(&self) -> Uuid {
        match self {
            Self::Role(role) => role.role_id,
            Self::Proposal(proposal) => proposal.proposal_id,
            Self::Assignment(assignment) => assignment.assignment_id,
            Self::Commitment(commitment) => commitment.commitment_id,
            Self::Handoff(handoff) => handoff.handoff_id,
        }
    }

    /// Per-entity revision.
    #[must_use]
    pub const fn entity_revision(&self) -> u64 {
        match self {
            Self::Role(role) => role.object_revision,
            Self::Proposal(proposal) => proposal.entity_revision,
            Self::Assignment(assignment) => assignment.entity_revision,
            Self::Commitment(commitment) => commitment.entity_revision,
            Self::Handoff(handoff) => handoff.entity_revision,
        }
    }
}

/// Pure result that the database coordinator persists atomically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleContinuityOutcome {
    /// New project revision.
    pub project_revision: u64,
    /// Complete changed entity heads.
    pub changes: Vec<RoleContinuityChange>,
    /// Work objects whose stable responsible Role changed.
    pub work_changes: Vec<WorkResponsibility>,
    /// Final Community role for every affected Member.
    pub membership_roles: BTreeMap<PublicKey, CommunityMemberRole>,
    /// Commitments ended for each ended Assignment, also embedded into any
    /// system-generated Handoff from the same transition.
    pub ended_commitments: BTreeMap<Uuid, Vec<Uuid>>,
}

/// Stable failures from the pure role-continuity protocol.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RoleContinuityError {
    /// The content is not schema v2.
    #[error("unsupported Role command schema")]
    UnsupportedSchema,
    /// Closed JSON or scalar validation failed.
    #[error("invalid Role command: {0}")]
    InvalidCommand(String),
    /// Optimistic concurrency failed.
    #[error("project revision conflict: expected {expected}, current {current}")]
    RevisionConflict {
        /// Signed expected revision.
        expected: u64,
        /// Locked current revision.
        current: u64,
    },
    /// Project revision cannot advance safely.
    #[error("project revision overflow")]
    RevisionOverflow,
    /// Role does not exist.
    #[error("Role was not found")]
    RoleNotFound,
    /// Role is inactive.
    #[error("Role is inactive")]
    RoleInactive,
    /// Candidate is not currently eligible for this Community.
    #[error("candidate is not eligible for this Community")]
    CandidateIneligible,
    /// Proposal does not exist.
    #[error("Proposal was not found")]
    ProposalNotFound,
    /// Proposal is terminal.
    #[error("Proposal is not open")]
    ProposalNotOpen,
    /// Proposal is already effectively expired.
    #[error("Proposal has expired")]
    ProposalExpired,
    /// Expire was requested before the canonical deadline.
    #[error("Proposal has not expired")]
    ProposalNotExpired,
    /// Deadline is not after canonical time or is too far in the future.
    #[error("invalid Proposal deadline")]
    InvalidProposalDeadline,
    /// An equivalent open Proposal already exists.
    #[error("an open Proposal already exists for this Role and candidate")]
    DuplicateProposal,
    /// Candidate-only action used by another signer.
    #[error("Proposal candidate authorization is required")]
    CandidateRequired,
    /// Creator-only withdrawal used by another signer.
    #[error("Proposal creator authorization is required")]
    CreatorRequired,
    /// The confirmation already exists.
    #[error("Proposal confirmation already exists")]
    AlreadyConfirmed,
    /// Assignment does not exist.
    #[error("Assignment was not found")]
    AssignmentNotFound,
    /// Assignment is already terminal.
    #[error("Assignment has ended")]
    AssignmentEnded,
    /// Role-bearing action omitted its tenure fence.
    #[error("acting_assignment_id is required")]
    ActingAssignmentRequired,
    /// Tenure fence is missing, stale, or belongs to another signer.
    #[error("acting_assignment_id is not the signer's active Assignment")]
    ActingAssignmentInvalid,
    /// Assignment action used by a non-assignee.
    #[error("active Assignment assignee authorization is required")]
    AssigneeRequired,
    /// Work does not exist or has been deleted.
    #[error("Work was not found")]
    WorkNotFound,
    /// Work is already completed or cancelled.
    #[error("Work is closed")]
    WorkClosed,
    /// Work has no responsible Role and therefore cannot be accepted.
    #[error("Work requires a responsible Role before it can be accepted")]
    ResponsibilityRequired,
    /// Requested responsibility is identical to current state.
    #[error("Work already has the requested responsible Role")]
    ResponsibilityUnchanged,
    /// One active Commitment prevents responsibility from changing or another
    /// Commitment from being created.
    #[error("Work already has an active Commitment")]
    ActiveCommitmentConflict,
    /// Work responsible Role differs from the acting Assignment Role.
    #[error("Work responsible Role does not match the acting Assignment")]
    WorkRoleMismatch,
    /// Commitment does not exist.
    #[error("Work Commitment was not found")]
    CommitmentNotFound,
    /// Commitment is already terminal.
    #[error("Work Commitment has ended")]
    CommitmentEnded,
    /// Commitment action used by another Assignment or Member.
    #[error("active Commitment assignee authorization is required")]
    CommitmentAssigneeRequired,
    /// Replacement did not fence the exact active Commitment.
    #[error("expected Work Commitment no longer matches")]
    CommitmentFenceConflict,
    /// Governor authority is insufficient.
    #[error("actor is not authorized for this Role change")]
    NotAuthorized,
    /// Only the Community owner may govern this Leader transition.
    #[error("Community owner authorization is required")]
    OwnerRequired,
    /// No Member may end their own Assignment in v0.
    #[error("an assignee cannot end its own Assignment")]
    SelfEndForbidden,
    /// Leader attempted to end a Leader tenure.
    #[error("a Leader cannot end a peer Leader Assignment")]
    PeerLeaderForbidden,
    /// Managed Agent Leader target could not be verified as another managed
    /// Agent.
    #[error("managed Agent Leader cannot end an unverified target")]
    ManagedLeaderTargetUnknown,
    /// Target Role or candidate tenure changed since Proposal creation.
    #[error("Proposal Assignment fence no longer matches")]
    CompoundFenceConflict,
    /// Client or Relay generated an occupied identifier.
    #[error("entity identifier is already occupied")]
    IdCollision,
    /// Relay did not supply enough Handoff IDs.
    #[error("Relay did not supply all required generated identifiers")]
    MissingGeneratedId,
    /// Replacement or unable report was already recorded.
    #[error("Assignment report already exists")]
    AlreadyReported,
    /// Reconstructed state violates a core invariant.
    #[error("invalid Role continuity state: {0}")]
    InvalidState(String),
}

impl RoleContinuityError {
    /// Stable protocol code used by Relay errors.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedSchema => "schema",
            Self::InvalidCommand(_) => "command",
            Self::RevisionConflict { .. } | Self::CompoundFenceConflict => "revision",
            Self::RevisionOverflow => "revision_overflow",
            Self::RoleNotFound => "role_not_found",
            Self::RoleInactive => "role_inactive",
            Self::CandidateIneligible => "candidate_ineligible",
            Self::ProposalNotFound => "proposal_not_found",
            Self::ProposalNotOpen => "proposal_not_open",
            Self::ProposalExpired => "proposal_expired",
            Self::ProposalNotExpired => "proposal_not_expired",
            Self::InvalidProposalDeadline => "proposal_deadline",
            Self::DuplicateProposal => "proposal_exists",
            Self::CandidateRequired => "candidate_required",
            Self::CreatorRequired => "creator_required",
            Self::AlreadyConfirmed => "already_confirmed",
            Self::AssignmentNotFound => "assignment_not_found",
            Self::AssignmentEnded => "assignment_ended",
            Self::ActingAssignmentRequired => "acting_assignment_required",
            Self::ActingAssignmentInvalid => "acting_assignment",
            Self::AssigneeRequired => "assignee_required",
            Self::WorkNotFound => "work_not_found",
            Self::WorkClosed => "work_closed",
            Self::ResponsibilityRequired => "responsibility_required",
            Self::ResponsibilityUnchanged => "responsibility_unchanged",
            Self::ActiveCommitmentConflict => "commitment_active",
            Self::WorkRoleMismatch => "work_role_mismatch",
            Self::CommitmentNotFound => "commitment_not_found",
            Self::CommitmentEnded => "commitment_ended",
            Self::CommitmentAssigneeRequired => "commitment_assignee_required",
            Self::CommitmentFenceConflict => "commitment_fence",
            Self::NotAuthorized => "authorization",
            Self::OwnerRequired => "owner_required",
            Self::SelfEndForbidden => "self_end",
            Self::PeerLeaderForbidden => "peer_leader",
            Self::ManagedLeaderTargetUnknown => "target_identity",
            Self::IdCollision => "id_collision",
            Self::MissingGeneratedId => "generated_id",
            Self::AlreadyReported => "already_reported",
            Self::InvalidState(_) => "state",
        }
    }
}

fn resolve_proposal(
    proposal: &mut RoleAssignmentProposal,
    status: ProposalStatus,
    reason: Option<String>,
    canonical_time: DateTime<Utc>,
    next_revision: u64,
) -> Result<(), RoleContinuityError> {
    proposal.status = status;
    proposal.reason = reason;
    proposal.resolved_at = Some(canonical_time);
    touch_proposal(proposal, next_revision)
}

fn touch_proposal(
    proposal: &mut RoleAssignmentProposal,
    next_revision: u64,
) -> Result<(), RoleContinuityError> {
    if proposal.project_revision != next_revision {
        proposal.entity_revision = proposal
            .entity_revision
            .checked_add(1)
            .filter(|revision| *revision <= MAX_SAFE_REVISION)
            .ok_or(RoleContinuityError::RevisionOverflow)?;
    }
    proposal.project_revision = next_revision;
    Ok(())
}

fn touch_assignment(
    assignment: &mut RoleAssignment,
    next_revision: u64,
) -> Result<(), RoleContinuityError> {
    if assignment.project_revision != next_revision {
        assignment.entity_revision = assignment
            .entity_revision
            .checked_add(1)
            .filter(|revision| *revision <= MAX_SAFE_REVISION)
            .ok_or(RoleContinuityError::RevisionOverflow)?;
    }
    assignment.project_revision = next_revision;
    Ok(())
}

fn touch_commitment(
    commitment: &mut WorkCommitment,
    next_revision: u64,
) -> Result<(), RoleContinuityError> {
    if commitment.project_revision != next_revision {
        commitment.entity_revision = commitment
            .entity_revision
            .checked_add(1)
            .filter(|revision| *revision <= MAX_SAFE_REVISION)
            .ok_or(RoleContinuityError::RevisionOverflow)?;
    }
    commitment.project_revision = next_revision;
    Ok(())
}

fn touch_work(
    work: &mut WorkResponsibility,
    actor: PublicKey,
    canonical_time: DateTime<Utc>,
    next_revision: u64,
) -> Result<(), RoleContinuityError> {
    work.object_revision = work
        .object_revision
        .checked_add(1)
        .filter(|revision| *revision <= MAX_SAFE_REVISION)
        .ok_or(RoleContinuityError::RevisionOverflow)?;
    work.project_revision = next_revision;
    work.updated_at = canonical_time;
    work.updated_by = actor;
    Ok(())
}

fn require_id(id: Uuid, field: &str) -> Result<(), RoleContinuityError> {
    if id.is_nil() {
        return Err(RoleContinuityError::InvalidCommand(format!(
            "{field} cannot be nil"
        )));
    }
    Ok(())
}

fn validate_optional_reason(reason: &Option<String>) -> Result<(), RoleContinuityError> {
    if let Some(reason) = reason {
        if reason.trim().is_empty() || reason.len() > MAX_REASON_BYTES {
            return Err(RoleContinuityError::InvalidCommand(format!(
                "reason must contain 1..={MAX_REASON_BYTES} bytes"
            )));
        }
    }
    Ok(())
}

fn collect_unique<T, K, F>(
    values: Vec<T>,
    key: F,
    entity: &str,
) -> Result<BTreeMap<K, T>, RoleContinuityError>
where
    K: Ord,
    F: Fn(&T) -> K,
{
    let mut map = BTreeMap::new();
    for value in values {
        if map.insert(key(&value), value).is_some() {
            return Err(RoleContinuityError::InvalidState(format!(
                "duplicate {entity} identifier"
            )));
        }
    }
    Ok(map)
}

fn changed_entities(
    before: BTreeMap<(RoleContinuityEntity, Uuid), RoleContinuityChange>,
    after: &BTreeMap<(RoleContinuityEntity, Uuid), RoleContinuityChange>,
) -> Vec<RoleContinuityChange> {
    after
        .iter()
        .filter_map(|(key, value)| (before.get(key) != Some(value)).then_some(value.clone()))
        .collect()
}

fn changed_works(
    before: BTreeMap<Uuid, WorkResponsibility>,
    after: &BTreeMap<Uuid, WorkResponsibility>,
) -> Vec<WorkResponsibility> {
    after
        .iter()
        .filter_map(|(work_id, work)| (before.get(work_id) != Some(work)).then_some(work.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_core::Keys;

    fn pubkey() -> PublicKey {
        Keys::generate().public_key()
    }

    fn member(pubkey: PublicKey, role: Option<CommunityMemberRole>) -> MemberGovernance {
        MemberGovernance {
            pubkey,
            community_role: role,
            eligible: true,
            managed_agent_owner: None,
        }
    }

    fn state(
        roles: Vec<RoleSlot>,
        members: Vec<MemberGovernance>,
        assignments: Vec<RoleAssignment>,
    ) -> RoleContinuityState {
        RoleContinuityState::from_snapshot(7, roles, members, Vec::new(), assignments, Vec::new())
            .expect("valid test state")
    }

    fn assignment(assignment_id: Uuid, role_id: Uuid, member_pubkey: PublicKey) -> RoleAssignment {
        RoleAssignment {
            assignment_id,
            role_id,
            member_pubkey,
            proposal_id: Uuid::new_v4(),
            started_at: DateTime::from_timestamp(1_800_000_000, 0).expect("timestamp"),
            started_by: pubkey(),
            replacement_requested_at: None,
            replacement_request_reason: None,
            unable_reported_at: None,
            unable_report_reason: None,
            ended_at: None,
            ended_by: None,
            ended_reason: None,
            replaced_by_assignment_id: None,
            entity_revision: 1,
            project_revision: 6,
        }
    }

    fn ids(handoffs: usize) -> GeneratedRoleContinuityIds {
        GeneratedRoleContinuityIds {
            assignment_id: Uuid::new_v4(),
            handoff_ids: (0..handoffs).map(|_| Uuid::new_v4()).collect(),
        }
    }

    fn work(
        work_id: Uuid,
        responsible_role_id: Option<Uuid>,
        actor: PublicKey,
    ) -> WorkResponsibility {
        WorkResponsibility {
            work_id,
            status: Some(WorkStatus::Pending),
            responsible_role_id,
            object_revision: 1,
            project_revision: 6,
            updated_at: DateTime::from_timestamp(1_800_000_000, 0).expect("timestamp"),
            updated_by: actor,
        }
    }

    fn complete_state(
        roles: Vec<RoleSlot>,
        works: Vec<WorkResponsibility>,
        members: Vec<MemberGovernance>,
        assignments: Vec<RoleAssignment>,
        commitments: Vec<WorkCommitment>,
    ) -> RoleContinuityState {
        RoleContinuityState::from_complete_snapshot(
            7,
            roles,
            works,
            members,
            Vec::new(),
            assignments,
            commitments,
            Vec::new(),
        )
        .expect("valid complete test state")
    }

    #[test]
    fn offer_completion_atomically_replaces_both_old_tenures() {
        let owner = pubkey();
        let incumbent = pubkey();
        let candidate = pubkey();
        let target_role = Uuid::new_v4();
        let old_role = Uuid::new_v4();
        let target_assignment = Uuid::new_v4();
        let candidate_assignment = Uuid::new_v4();
        let roles = vec![
            RoleSlot {
                role_id: target_role,
                level: RoleLevel::Member,
                active: true,
            },
            RoleSlot {
                role_id: old_role,
                level: RoleLevel::Member,
                active: true,
            },
        ];
        let current = state(
            roles,
            vec![
                member(owner, Some(CommunityMemberRole::Owner)),
                member(incumbent, Some(CommunityMemberRole::Member)),
                member(candidate, Some(CommunityMemberRole::Member)),
            ],
            vec![
                assignment(target_assignment, target_role, incumbent),
                assignment(candidate_assignment, old_role, candidate),
            ],
        );
        let proposal_id = Uuid::new_v4();
        let now = DateTime::from_timestamp(1_800_000_100, 0).expect("timestamp");
        let offer = RoleCommand::new(
            7,
            None,
            RoleCommandRequest::OfferRole {
                proposal_id,
                role_id: target_role,
                candidate_pubkey: candidate,
                expires_at: now + Duration::days(2),
                reason: None,
            },
        );
        let (offered, _) = current
            .reduce(&offer, owner, now, &ids(0))
            .expect("owner offer");
        let generated = ids(2);
        let accept = RoleCommand::new(8, None, RoleCommandRequest::AcceptProposal { proposal_id });
        let (completed, outcome) = offered
            .reduce(&accept, candidate, now + Duration::seconds(1), &generated)
            .expect("candidate acceptance");

        assert_eq!(
            completed
                .assignments
                .get(&target_assignment)
                .and_then(|assignment| assignment.ended_reason),
            Some(AssignmentEndReason::Replaced)
        );
        assert_eq!(
            completed
                .assignments
                .get(&candidate_assignment)
                .and_then(|assignment| assignment.ended_reason),
            Some(AssignmentEndReason::Replaced)
        );
        assert_eq!(
            completed
                .active_assignment_for_member(candidate)
                .expect("new assignment"),
            generated.assignment_id
        );
        assert_eq!(completed.handoffs.len(), 2);
        assert_eq!(outcome.project_revision, 9);
    }

    #[test]
    fn stale_authorizer_leaves_offer_open_and_unaccepted() {
        let owner = pubkey();
        let leader = pubkey();
        let candidate = pubkey();
        let leader_role = Uuid::new_v4();
        let target_role = Uuid::new_v4();
        let leader_assignment = Uuid::new_v4();
        let roles = vec![
            RoleSlot {
                role_id: leader_role,
                level: RoleLevel::Admin,
                active: true,
            },
            RoleSlot {
                role_id: target_role,
                level: RoleLevel::Member,
                active: true,
            },
        ];
        let current = state(
            roles.clone(),
            vec![
                member(owner, Some(CommunityMemberRole::Owner)),
                member(leader, Some(CommunityMemberRole::Admin)),
                member(candidate, Some(CommunityMemberRole::Member)),
            ],
            vec![assignment(leader_assignment, leader_role, leader)],
        );
        let proposal_id = Uuid::new_v4();
        let now = DateTime::from_timestamp(1_800_000_100, 0).expect("timestamp");
        let offer = RoleCommand::new(
            7,
            Some(leader_assignment),
            RoleCommandRequest::OfferRole {
                proposal_id,
                role_id: target_role,
                candidate_pubkey: candidate,
                expires_at: now + Duration::days(2),
                reason: None,
            },
        );
        let (offered, _) = current
            .reduce(&offer, leader, now, &ids(0))
            .expect("leader offer");

        let mut ended_leader = offered
            .assignments
            .get(&leader_assignment)
            .expect("leader assignment")
            .clone();
        ended_leader.ended_at = Some(now + Duration::seconds(1));
        ended_leader.ended_by = Some(owner);
        ended_leader.ended_reason = Some(AssignmentEndReason::Revoked);
        ended_leader.entity_revision += 1;
        ended_leader.project_revision = 9;
        let stale = RoleContinuityState::from_snapshot(
            9,
            roles,
            vec![
                member(owner, Some(CommunityMemberRole::Owner)),
                member(leader, Some(CommunityMemberRole::Member)),
                member(candidate, Some(CommunityMemberRole::Member)),
            ],
            offered.proposals.values().cloned().collect(),
            vec![ended_leader],
            Vec::new(),
        )
        .expect("state after leader removal");
        let accept = RoleCommand::new(9, None, RoleCommandRequest::AcceptProposal { proposal_id });
        assert_eq!(
            stale.reduce(&accept, candidate, now + Duration::seconds(2), &ids(0)),
            Err(RoleContinuityError::NotAuthorized)
        );
        assert!(stale
            .proposals
            .get(&proposal_id)
            .expect("proposal")
            .candidate_accepted_at
            .is_none());
    }

    #[test]
    fn assignee_can_request_replacement_but_cannot_end_itself() {
        let owner = pubkey();
        let agent = pubkey();
        let role_id = Uuid::new_v4();
        let assignment_id = Uuid::new_v4();
        let current = state(
            vec![RoleSlot {
                role_id,
                level: RoleLevel::Member,
                active: true,
            }],
            vec![
                member(owner, Some(CommunityMemberRole::Owner)),
                member(agent, Some(CommunityMemberRole::Member)),
            ],
            vec![assignment(assignment_id, role_id, agent)],
        );
        let now = DateTime::from_timestamp(1_800_000_100, 0).expect("timestamp");
        let end = RoleCommand::new(
            7,
            Some(assignment_id),
            RoleCommandRequest::EndAssignment {
                assignment_id,
                reason: None,
            },
        );
        assert_eq!(
            current.reduce(&end, agent, now, &ids(0)),
            Err(RoleContinuityError::SelfEndForbidden)
        );

        let request = RoleCommand::new(
            7,
            Some(assignment_id),
            RoleCommandRequest::RequestReplacement {
                assignment_id,
                reason: Some("planned handoff".to_owned()),
            },
        );
        let (next, _) = current
            .reduce(&request, agent, now, &ids(0))
            .expect("replacement request");
        assert_eq!(
            next.assignments
                .get(&assignment_id)
                .and_then(|assignment| assignment.replacement_requested_at),
            Some(now)
        );
        assert!(next
            .assignments
            .get(&assignment_id)
            .is_some_and(RoleAssignment::is_active));
    }

    #[test]
    fn leader_governance_requires_the_exact_active_assignment_fence() {
        let owner = pubkey();
        let leader = pubkey();
        let candidate = pubkey();
        let leader_role = Uuid::new_v4();
        let target_role = Uuid::new_v4();
        let leader_assignment = Uuid::new_v4();
        let current = state(
            vec![
                RoleSlot {
                    role_id: leader_role,
                    level: RoleLevel::Admin,
                    active: true,
                },
                RoleSlot {
                    role_id: target_role,
                    level: RoleLevel::Member,
                    active: true,
                },
            ],
            vec![
                member(owner, Some(CommunityMemberRole::Owner)),
                member(leader, Some(CommunityMemberRole::Admin)),
                member(candidate, Some(CommunityMemberRole::Member)),
            ],
            vec![assignment(leader_assignment, leader_role, leader)],
        );
        let now = DateTime::from_timestamp(1_800_000_100, 0).expect("timestamp");
        let proposal_id = Uuid::new_v4();
        let request = RoleCommandRequest::OfferRole {
            proposal_id,
            role_id: target_role,
            candidate_pubkey: candidate,
            expires_at: now + Duration::days(2),
            reason: None,
        };

        assert_eq!(
            current.reduce(
                &RoleCommand::new(7, None, request.clone()),
                leader,
                now,
                &ids(0),
            ),
            Err(RoleContinuityError::ActingAssignmentRequired)
        );
        assert_eq!(
            current.reduce(
                &RoleCommand::new(7, Some(Uuid::new_v4()), request.clone()),
                leader,
                now,
                &ids(0),
            ),
            Err(RoleContinuityError::ActingAssignmentInvalid)
        );
        let (next, _) = current
            .reduce(
                &RoleCommand::new(7, Some(leader_assignment), request),
                leader,
                now,
                &ids(0),
            )
            .expect("active Leader Assignment authorizes ordinary Role offer");
        assert!(next.proposals.contains_key(&proposal_id));
    }

    #[test]
    fn only_governance_can_assign_work_and_only_its_role_can_accept() {
        let owner = pubkey();
        let agent_a = pubkey();
        let agent_b = pubkey();
        let role_a = Uuid::new_v4();
        let role_b = Uuid::new_v4();
        let assignment_a = Uuid::new_v4();
        let assignment_b = Uuid::new_v4();
        let work_id = Uuid::new_v4();
        let current = complete_state(
            vec![
                RoleSlot {
                    role_id: role_a,
                    level: RoleLevel::Member,
                    active: true,
                },
                RoleSlot {
                    role_id: role_b,
                    level: RoleLevel::Member,
                    active: true,
                },
            ],
            vec![work(work_id, None, owner)],
            vec![
                member(owner, Some(CommunityMemberRole::Owner)),
                member(agent_a, Some(CommunityMemberRole::Member)),
                member(agent_b, Some(CommunityMemberRole::Member)),
            ],
            vec![
                assignment(assignment_a, role_a, agent_a),
                assignment(assignment_b, role_b, agent_b),
            ],
            Vec::new(),
        );
        let now = DateTime::from_timestamp(1_800_000_100, 0).expect("timestamp");
        let set_responsibility = RoleCommandRequest::SetWorkResponsibility {
            work_id,
            responsible_role_id: Some(role_a),
        };
        assert_eq!(
            current.reduce(
                &RoleCommand::new(7, Some(assignment_a), set_responsibility.clone()),
                agent_a,
                now,
                &ids(0),
            ),
            Err(RoleContinuityError::NotAuthorized)
        );
        let (assigned, outcome) = current
            .reduce(
                &RoleCommand::new(7, None, set_responsibility),
                owner,
                now,
                &ids(0),
            )
            .expect("owner assigns Work");
        assert_eq!(outcome.work_changes.len(), 1);
        assert_eq!(outcome.work_changes[0].responsible_role_id, Some(role_a));

        let other_commitment = Uuid::new_v4();
        assert_eq!(
            assigned.reduce(
                &RoleCommand::new(
                    8,
                    Some(assignment_b),
                    RoleCommandRequest::AcceptWork {
                        commitment_id: other_commitment,
                        work_id,
                    },
                ),
                agent_b,
                now + Duration::seconds(1),
                &ids(0),
            ),
            Err(RoleContinuityError::WorkRoleMismatch)
        );

        let commitment_id = Uuid::new_v4();
        let (accepted, outcome) = assigned
            .reduce(
                &RoleCommand::new(
                    8,
                    Some(assignment_a),
                    RoleCommandRequest::AcceptWork {
                        commitment_id,
                        work_id,
                    },
                ),
                agent_a,
                now + Duration::seconds(1),
                &ids(0),
            )
            .expect("responsible Role assignee accepts Work");
        assert!(accepted
            .commitments
            .get(&commitment_id)
            .is_some_and(WorkCommitment::is_active));
        assert!(matches!(
            outcome.changes.as_slice(),
            [RoleContinuityChange::Commitment(commitment)]
                if commitment.commitment_id == commitment_id
        ));
    }

    #[test]
    fn ending_assignment_ends_commitment_without_changing_work() {
        let owner = pubkey();
        let agent = pubkey();
        let role_id = Uuid::new_v4();
        let assignment_id = Uuid::new_v4();
        let work_id = Uuid::new_v4();
        let commitment_id = Uuid::new_v4();
        let now = DateTime::from_timestamp(1_800_000_100, 0).expect("timestamp");
        let current = complete_state(
            vec![RoleSlot {
                role_id,
                level: RoleLevel::Member,
                active: true,
            }],
            vec![work(work_id, Some(role_id), owner)],
            vec![
                member(owner, Some(CommunityMemberRole::Owner)),
                member(agent, Some(CommunityMemberRole::Member)),
            ],
            vec![assignment(assignment_id, role_id, agent)],
            vec![WorkCommitment {
                commitment_id,
                work_id,
                assignment_id,
                member_pubkey: agent,
                started_at: now - Duration::seconds(5),
                started_by: agent,
                ended_at: None,
                ended_by: None,
                ended_reason: None,
                entity_revision: 1,
                project_revision: 7,
            }],
        );
        assert_eq!(
            current.reduce(
                &RoleCommand::new(
                    7,
                    None,
                    RoleCommandRequest::SetWorkResponsibility {
                        work_id,
                        responsible_role_id: None,
                    },
                ),
                owner,
                now,
                &ids(0),
            ),
            Err(RoleContinuityError::ActiveCommitmentConflict)
        );
        let (ended, outcome) = current
            .reduce(
                &RoleCommand::new(
                    7,
                    None,
                    RoleCommandRequest::EndAssignment {
                        assignment_id,
                        reason: None,
                    },
                ),
                owner,
                now,
                &ids(0),
            )
            .expect("owner ends Assignment");

        let commitment = ended.commitments.get(&commitment_id).expect("commitment");
        assert_eq!(
            commitment.ended_reason,
            Some(CommitmentEndReason::AssignmentEnded)
        );
        assert_eq!(
            ended.works.get(&work_id).expect("work").status,
            Some(WorkStatus::Pending)
        );
        assert_eq!(
            ended.works.get(&work_id).expect("work").responsible_role_id,
            Some(role_id)
        );
        assert!(outcome.work_changes.is_empty());
        assert_eq!(
            outcome.ended_commitments.get(&assignment_id),
            Some(&vec![commitment_id])
        );
    }

    #[test]
    fn recommit_preserves_predecessor_attribution() {
        let owner = pubkey();
        let agent = pubkey();
        let role_id = Uuid::new_v4();
        let assignment_id = Uuid::new_v4();
        let work_id = Uuid::new_v4();
        let old_commitment_id = Uuid::new_v4();
        let new_commitment_id = Uuid::new_v4();
        let now = DateTime::from_timestamp(1_800_000_100, 0).expect("timestamp");
        let current = complete_state(
            vec![RoleSlot {
                role_id,
                level: RoleLevel::Member,
                active: true,
            }],
            vec![work(work_id, Some(role_id), owner)],
            vec![
                member(owner, Some(CommunityMemberRole::Owner)),
                member(agent, Some(CommunityMemberRole::Member)),
            ],
            vec![assignment(assignment_id, role_id, agent)],
            vec![WorkCommitment {
                commitment_id: old_commitment_id,
                work_id,
                assignment_id,
                member_pubkey: agent,
                started_at: now - Duration::seconds(5),
                started_by: agent,
                ended_at: None,
                ended_by: None,
                ended_reason: None,
                entity_revision: 1,
                project_revision: 7,
            }],
        );
        let (recommitted, outcome) = current
            .reduce(
                &RoleCommand::new(
                    7,
                    Some(assignment_id),
                    RoleCommandRequest::ReplaceCommitment {
                        commitment_id: new_commitment_id,
                        work_id,
                        expected_commitment_id: old_commitment_id,
                    },
                ),
                agent,
                now,
                &ids(0),
            )
            .expect("assignee recommits atomically");

        let old = recommitted
            .commitments
            .get(&old_commitment_id)
            .expect("old Commitment retained");
        assert_eq!(old.ended_reason, Some(CommitmentEndReason::Replaced));
        assert_eq!(old.member_pubkey, agent);
        assert!(recommitted
            .commitments
            .get(&new_commitment_id)
            .is_some_and(WorkCommitment::is_active));
        assert_eq!(
            outcome
                .changes
                .iter()
                .filter(|change| matches!(change, RoleContinuityChange::Commitment(_)))
                .count(),
            2
        );
    }
}
