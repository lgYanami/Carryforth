//! Project View v2 role-continuity primitives.
//!
//! This module is intentionally separate from the v1 object and mutation
//! types. Adding fields to the existing closed serde shapes would make a v1
//! client accidentally accept a v2 payload. The Relay selects one schema
//! version per Community and only constructs these types for v2 state.

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::num::NonZeroI64;

mod role_continuity;

pub use role_continuity::{
    AssignmentEndReason, CommunityMemberRole, GeneratedRoleContinuityIds, MemberGovernance,
    ProposalStatus, ProposalType, RoleAssignment, RoleAssignmentProposal, RoleCommand,
    RoleCommandRequest, RoleContinuityChange, RoleContinuityEntity, RoleContinuityError,
    RoleContinuityOutcome, RoleContinuityState, RoleDefinition, RoleHandoff, RoleSlot,
    MAX_PROPOSAL_LIFETIME_DAYS,
};

const CHANGE_ID_DOMAIN: &[u8] = b"buzz-project-view-v2:change-id\0";
const REQUEST_HASH_DOMAIN: &[u8] = b"buzz-project-view-v2:request\0";
const IDEMPOTENCY_HASH_DOMAIN: &[u8] = b"buzz-project-view-v2:idempotency-key\0";

/// Project View wire and canonical schema version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub enum SchemaVersion {
    /// The original nine-object Project View schema.
    V1,
    /// Role continuity and membership-coupled Project View schema.
    V2,
}

impl SchemaVersion {
    /// Return the stable wire and database number.
    #[must_use]
    pub const fn as_u16(self) -> u16 {
        match self {
            Self::V1 => 1,
            Self::V2 => 2,
        }
    }
}

impl TryFrom<u16> for SchemaVersion {
    type Error = UnsupportedSchemaVersion;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::V1),
            2 => Ok(Self::V2),
            _ => Err(UnsupportedSchemaVersion(value)),
        }
    }
}

impl From<SchemaVersion> for u16 {
    fn from(value: SchemaVersion) -> Self {
        value.as_u16()
    }
}

/// Error returned for a Project View schema number this binary cannot use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("unsupported Project View schema version {0}")]
pub struct UnsupportedSchemaVersion(pub u16);

/// Authenticated source from which one accepted v2 change is derived.
///
/// The source is closed and typed so a non-event operator action cannot be
/// presented as a member-signed Nostr event. Raw idempotency keys never enter
/// this value; only their domain-separated digest is retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeSource {
    /// A signed Nostr command. Its event ID is already a stable content digest
    /// and therefore is the change ID.
    NostrEvent {
        /// ID of the verified source event.
        event_id: [u8; 32],
    },
    /// A NIP-98-authenticated HTTP request such as an invite claim.
    Nip98Request {
        /// ID of the verified NIP-98 authentication event.
        auth_event_id: [u8; 32],
        /// Domain-separated digest of the canonical request bytes.
        request_hash: [u8; 32],
    },
    /// A deployment-operator action linked to the Community audit chain.
    Operator {
        /// Non-zero sequence of the referenced Community audit entry.
        audit_seq: AuditSequence,
        /// Domain-separated digest of the caller's idempotency key.
        idempotency_key_hash: [u8; 32],
    },
    /// A trusted internal system action linked to the Community audit chain.
    System {
        /// Non-zero sequence of the referenced Community audit entry.
        audit_seq: AuditSequence,
        /// Domain-separated digest of the system idempotency key.
        idempotency_key_hash: [u8; 32],
    },
}

impl ChangeSource {
    /// Build a member-signed Nostr source.
    #[must_use]
    pub const fn nostr_event(event_id: [u8; 32]) -> Self {
        Self::NostrEvent { event_id }
    }

    /// Build a NIP-98 source from an authentication event and canonical
    /// request digest.
    #[must_use]
    pub const fn nip98_request(auth_event_id: [u8; 32], request_hash: [u8; 32]) -> Self {
        Self::Nip98Request {
            auth_event_id,
            request_hash,
        }
    }

    /// Build an operator source.
    pub fn operator(
        audit_seq: i64,
        idempotency_key_hash: [u8; 32],
    ) -> Result<Self, ChangeSourceError> {
        Ok(Self::Operator {
            audit_seq: required_audit_sequence(audit_seq)?,
            idempotency_key_hash,
        })
    }

    /// Build a trusted system source.
    pub fn system(
        audit_seq: i64,
        idempotency_key_hash: [u8; 32],
    ) -> Result<Self, ChangeSourceError> {
        Ok(Self::System {
            audit_seq: required_audit_sequence(audit_seq)?,
            idempotency_key_hash,
        })
    }

    /// Return the stable database and wire discriminator.
    #[must_use]
    pub const fn source_type(self) -> &'static str {
        match self {
            Self::NostrEvent { .. } => "nostr_event",
            Self::Nip98Request { .. } => "nip98_request",
            Self::Operator { .. } => "operator",
            Self::System { .. } => "system",
        }
    }

    /// Compute the stable 32-byte change ID.
    ///
    /// A Nostr command reuses its event ID. Other sources use SHA-256 with a
    /// protocol-owned domain separator and fixed-width fields.
    #[must_use]
    pub fn change_id(self) -> [u8; 32] {
        match self {
            Self::NostrEvent { event_id } => event_id,
            Self::Nip98Request {
                auth_event_id,
                request_hash,
            } => digest_parts(&[
                CHANGE_ID_DOMAIN,
                b"nip98_request\0",
                &auth_event_id,
                &request_hash,
            ]),
            Self::Operator {
                audit_seq,
                idempotency_key_hash,
            } => digest_audited_source(b"operator\0", audit_seq, idempotency_key_hash),
            Self::System {
                audit_seq,
                idempotency_key_hash,
            } => digest_audited_source(b"system\0", audit_seq, idempotency_key_hash),
        }
    }
}

/// Stable validation failures for a typed v2 change source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ChangeSourceError {
    /// Audit sequences start at one inside each Community chain.
    #[error("audit sequence must be greater than zero")]
    InvalidAuditSequence,
}

/// A positive sequence in one Community's hash-chain audit log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditSequence(NonZeroI64);

impl AuditSequence {
    /// Return the database representation.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0.get()
    }
}

/// Hash canonical request bytes before retaining them in a NIP-98 source.
#[must_use]
pub fn canonical_request_hash(canonical_request: &[u8]) -> [u8; 32] {
    digest_parts(&[REQUEST_HASH_DOMAIN, canonical_request])
}

/// Hash an operator or system idempotency key without retaining the raw key.
#[must_use]
pub fn idempotency_key_hash(idempotency_key: &[u8]) -> [u8; 32] {
    digest_parts(&[IDEMPOTENCY_HASH_DOMAIN, idempotency_key])
}

fn required_audit_sequence(value: i64) -> Result<AuditSequence, ChangeSourceError> {
    NonZeroI64::new(value)
        .filter(|sequence| sequence.get() > 0)
        .map(AuditSequence)
        .ok_or(ChangeSourceError::InvalidAuditSequence)
}

fn digest_audited_source(
    source_type: &[u8],
    audit_seq: AuditSequence,
    idempotency_key_hash: [u8; 32],
) -> [u8; 32] {
    digest_parts(&[
        CHANGE_ID_DOMAIN,
        source_type,
        &audit_seq.get().to_be_bytes(),
        &idempotency_key_hash,
    ])
}

fn digest_parts(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// The Community permission level granted by one active Role Assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleLevel {
    /// A Leader Role, mapped to Community `admin`.
    Admin,
    /// An ordinary Role, mapped to Community `member`.
    Member,
}

impl RoleLevel {
    /// Return the stable wire and database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }
}

/// The governance-sensitive fields of a v2 Role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoleGovernanceState {
    /// Permission level granted by an active Assignment.
    pub level: RoleLevel,
    /// Whether the Role may receive an active Assignment.
    pub active: bool,
}

impl RoleGovernanceState {
    /// Return whether moving from `self` to `next` requires Community-owner
    /// authorization.
    ///
    /// Every lifecycle operation involving an admin Role is owner-only:
    /// creation, promotion, demotion, deactivation, and reactivation.
    #[must_use]
    pub const fn transition_requires_owner(self, next: Self) -> bool {
        matches!(self.level, RoleLevel::Admin) || matches!(next.level, RoleLevel::Admin)
    }
}

/// Reject a governance-sensitive Role transition that lacks owner authority.
pub fn authorize_role_governance_transition(
    current: RoleGovernanceState,
    next: RoleGovernanceState,
    actor_is_community_owner: bool,
) -> Result<(), RoleGovernanceError> {
    if current.transition_requires_owner(next) && !actor_is_community_owner {
        return Err(RoleGovernanceError::OwnerRequired);
    }
    Ok(())
}

/// Reject creation of an admin Role that lacks Community-owner authority.
pub fn authorize_role_creation(
    level: RoleLevel,
    actor_is_community_owner: bool,
) -> Result<(), RoleGovernanceError> {
    if matches!(level, RoleLevel::Admin) && !actor_is_community_owner {
        return Err(RoleGovernanceError::OwnerRequired);
    }
    Ok(())
}

/// Stable authorization failures for Role governance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RoleGovernanceError {
    /// An admin Role lifecycle change was attempted by a non-owner.
    #[error("Community owner authorization is required for an admin Role lifecycle change")]
    OwnerRequired,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(level: RoleLevel, active: bool) -> RoleGovernanceState {
        RoleGovernanceState { level, active }
    }

    #[test]
    fn schema_version_is_closed() {
        assert_eq!(SchemaVersion::try_from(1), Ok(SchemaVersion::V1));
        assert_eq!(SchemaVersion::try_from(2), Ok(SchemaVersion::V2));
        assert_eq!(SchemaVersion::try_from(3), Err(UnsupportedSchemaVersion(3)));
    }

    #[test]
    fn change_sources_have_stable_domain_separated_ids() {
        let event_id = [0x11; 32];
        assert_eq!(ChangeSource::nostr_event(event_id).change_id(), event_id);

        let request_hash = canonical_request_hash(br#"{"role":"member"}"#);
        assert_eq!(
            request_hash,
            [
                0xe4, 0x2f, 0xdf, 0x4f, 0x66, 0xd5, 0xfa, 0xcb, 0x2f, 0x1a, 0x45, 0x87, 0xed, 0xdc,
                0xef, 0xdd, 0x42, 0x82, 0xa5, 0x3b, 0x4d, 0xc7, 0x42, 0x12, 0xe8, 0x1e, 0x3a, 0x3d,
                0xec, 0x58, 0x57, 0xc5,
            ]
        );
        let nip98 = ChangeSource::nip98_request([0x22; 32], request_hash);
        assert_eq!(
            nip98.change_id(),
            [
                0x3d, 0x10, 0x15, 0x4d, 0xc1, 0x9f, 0xf2, 0x4c, 0x46, 0xb5, 0x43, 0x9c, 0xfa, 0xbc,
                0xdd, 0x2d, 0x88, 0xe5, 0xe1, 0x5e, 0x03, 0xcb, 0x54, 0x1a, 0xd8, 0x02, 0xfb, 0xbe,
                0x1d, 0x05, 0x5d, 0x28,
            ]
        );

        let key_hash = idempotency_key_hash(b"operator-request-7");
        let operator = ChangeSource::operator(7, key_hash).expect("non-zero audit sequence");
        let system = ChangeSource::system(7, key_hash).expect("non-zero audit sequence");
        assert_eq!(
            operator.change_id(),
            [
                0x6b, 0x41, 0xc8, 0x41, 0xcc, 0x58, 0x1a, 0xcf, 0xc3, 0xa8, 0x44, 0xc0, 0x2d, 0x1e,
                0xf3, 0xee, 0x47, 0xcd, 0xc2, 0xe1, 0x38, 0xa3, 0xd3, 0x76, 0x89, 0x71, 0x51, 0x42,
                0x11, 0x1a, 0x44, 0x0a,
            ]
        );
        assert_ne!(operator.change_id(), system.change_id());
        assert_ne!(operator.change_id(), nip98.change_id());
        assert_eq!(
            ChangeSource::operator(0, key_hash),
            Err(ChangeSourceError::InvalidAuditSequence)
        );
        assert_eq!(
            ChangeSource::operator(-1, key_hash),
            Err(ChangeSourceError::InvalidAuditSequence)
        );
    }

    #[test]
    fn all_admin_lifecycle_transitions_are_owner_only() {
        let member_active = state(RoleLevel::Member, true);
        let admin_active = state(RoleLevel::Admin, true);
        let admin_inactive = state(RoleLevel::Admin, false);

        for (current, next) in [
            (member_active, admin_active),
            (admin_active, member_active),
            (admin_active, admin_inactive),
            (admin_inactive, admin_active),
        ] {
            assert_eq!(
                authorize_role_governance_transition(current, next, false),
                Err(RoleGovernanceError::OwnerRequired)
            );
            assert!(
                authorize_role_governance_transition(current, next, true).is_ok(),
                "owner must be allowed to perform {current:?} -> {next:?}"
            );
        }
    }

    #[test]
    fn admin_role_creation_is_owner_only() {
        assert_eq!(
            authorize_role_creation(RoleLevel::Admin, false),
            Err(RoleGovernanceError::OwnerRequired)
        );
        assert!(authorize_role_creation(RoleLevel::Admin, true).is_ok());
        assert!(authorize_role_creation(RoleLevel::Member, false).is_ok());
    }

    #[test]
    fn member_role_lifecycle_does_not_require_owner() {
        let active = state(RoleLevel::Member, true);
        let inactive = state(RoleLevel::Member, false);
        assert!(authorize_role_governance_transition(active, inactive, false).is_ok());
        assert!(authorize_role_governance_transition(inactive, active, false).is_ok());
    }
}
