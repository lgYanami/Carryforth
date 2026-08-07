//! Canonical Project Context catalog, edge, and binding state.

use std::fmt;

use buzz_core::CommunityId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::coordinate::validate_canonical_coordinates;
use crate::validation::{
    validate_document_id, validate_nonnegative, validate_positive, validate_uuid_v4,
};
use crate::{
    ProjectContextCoordinate, ProjectContextError, ProjectContextResult, MAX_SAFE_REVISION,
};

const EDGE_KEY_DOMAIN: &[u8] = b"buzz-project-context-edge-v1\0";

/// Deterministic SHA-256 identity of one Project-scoped coordinate set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EdgeKey([u8; 32]);

impl EdgeKey {
    /// Derive a key from the host-derived Project identity and canonical coordinates.
    pub fn derive(
        project_id: Uuid,
        coordinates: &[ProjectContextCoordinate],
    ) -> ProjectContextResult<Self> {
        validate_uuid_v4(project_id, "project_id")?;
        validate_canonical_coordinates(coordinates)?;
        let count = u32::try_from(coordinates.len()).map_err(|_| {
            ProjectContextError::InvalidCoordinate {
                reason: "coordinate count exceeds the edge-key-v1 u32 identity encoding".to_owned(),
            }
        })?;
        let mut hasher = Sha256::new();
        hasher.update(EDGE_KEY_DOMAIN);
        hasher.update(project_id.as_bytes());
        hasher.update(count.to_be_bytes());
        for coordinate in coordinates {
            let mut bytes = Vec::with_capacity(18);
            coordinate.append_identity_bytes(&mut bytes);
            hasher.update(bytes);
        }
        Ok(Self(hasher.finalize().into()))
    }

    /// Parse the canonical 64-character lowercase hexadecimal wire value.
    pub fn from_hex(value: &str) -> ProjectContextResult<Self> {
        if value.len() != 64 || value.bytes().any(|byte| !byte.is_ascii_hexdigit()) {
            return Err(ProjectContextError::InvalidEdgeKey {
                reason: "edge key must contain exactly 64 hexadecimal characters".to_owned(),
            });
        }
        if value.bytes().any(|byte| byte.is_ascii_uppercase()) {
            return Err(ProjectContextError::InvalidEdgeKey {
                reason: "edge key hexadecimal must be lowercase".to_owned(),
            });
        }
        let decoded = hex::decode(value).map_err(|error| ProjectContextError::InvalidEdgeKey {
            reason: error.to_string(),
        })?;
        let bytes: [u8; 32] =
            decoded
                .try_into()
                .map_err(|_| ProjectContextError::InvalidEdgeKey {
                    reason: "edge key must decode to 32 bytes".to_owned(),
                })?;
        Ok(Self(bytes))
    }

    /// Canonical lowercase hexadecimal representation.
    #[must_use]
    pub fn to_hex(self) -> String {
        hex::encode(self.0)
    }

    /// Raw 32-byte hash value.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for EdgeKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&hex::encode(self.0))
    }
}

impl Serialize for EdgeKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for EdgeKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(&value).map_err(serde::de::Error::custom)
    }
}

/// Stable operation names used by commands, receipts, audit, and telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectContextOperation {
    /// Add one active Context Document to an edge.
    Attach,
    /// Remove one active Context Document from its exact edge.
    Detach,
}

impl ProjectContextOperation {
    /// Stable wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Attach => "attach",
            Self::Detach => "detach",
        }
    }
}

/// Lifecycle state carried by a binding projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectContextBindingState {
    /// The Document currently belongs to the edge.
    Active,
    /// The prior binding has been removed.
    Deleted,
}

impl ProjectContextBindingState {
    /// Stable wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Deleted => "deleted",
        }
    }
}

/// One active hyperedge and its non-empty, sorted Context Document membership.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectContextEdge {
    key: EdgeKey,
    coordinates: Vec<ProjectContextCoordinate>,
    context_document_ids: Vec<Uuid>,
}

impl ProjectContextEdge {
    /// Reconstruct and validate one canonical active edge.
    pub fn from_snapshot(
        project_id: Uuid,
        coordinates: Vec<ProjectContextCoordinate>,
        context_document_ids: Vec<Uuid>,
    ) -> ProjectContextResult<Self> {
        let key = EdgeKey::derive(project_id, &coordinates)?;
        let edge = Self {
            key,
            coordinates,
            context_document_ids,
        };
        edge.validate(project_id)?;
        Ok(edge)
    }

    /// Validate identity, canonical coordinates, and sorted non-empty membership.
    pub fn validate(&self, project_id: Uuid) -> ProjectContextResult<()> {
        validate_canonical_coordinates(&self.coordinates)?;
        for coordinate in &self.coordinates {
            coordinate.validate_for_project(project_id)?;
        }
        if self.key != EdgeKey::derive(project_id, &self.coordinates)? {
            return Err(ProjectContextError::InvalidCanonicalState {
                reason: "edge key does not match project and coordinates".to_owned(),
            });
        }
        if self.context_document_ids.is_empty() {
            return Err(ProjectContextError::InvalidCanonicalState {
                reason: "an active edge must contain at least one Context Document".to_owned(),
            });
        }
        for id in &self.context_document_ids {
            validate_document_id(*id)?;
        }
        if self
            .context_document_ids
            .windows(2)
            .any(|pair| pair[0].as_bytes() >= pair[1].as_bytes())
        {
            return Err(ProjectContextError::InvalidCanonicalState {
                reason: "edge Context Document ids must be strictly sorted".to_owned(),
            });
        }
        Ok(())
    }

    /// Deterministic edge identity.
    #[must_use]
    pub const fn key(&self) -> EdgeKey {
        self.key
    }

    /// Canonical coordinate set.
    #[must_use]
    pub fn coordinates(&self) -> &[ProjectContextCoordinate] {
        &self.coordinates
    }

    /// Strictly sorted active Context Document identities.
    #[must_use]
    pub fn context_document_ids(&self) -> &[Uuid] {
        &self.context_document_ids
    }
}

/// Canonical current catalog counters and projection generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectContextCatalog {
    project_id: CommunityId,
    context_revision: u64,
    active_edge_count: u64,
    bound_document_count: u64,
    projection_generation: u64,
    initialized_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl ProjectContextCatalog {
    /// Construct an initialized but untouched empty catalog at revision zero.
    pub fn empty(
        project_id: CommunityId,
        projection_generation: u64,
        initialized_at: DateTime<Utc>,
    ) -> ProjectContextResult<Self> {
        Self::from_snapshot(
            project_id,
            0,
            0,
            0,
            projection_generation,
            initialized_at,
            initialized_at,
        )
    }

    /// Reconstruct and validate one trusted canonical catalog row.
    #[allow(clippy::too_many_arguments)]
    pub fn from_snapshot(
        project_id: CommunityId,
        context_revision: u64,
        active_edge_count: u64,
        bound_document_count: u64,
        projection_generation: u64,
        initialized_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    ) -> ProjectContextResult<Self> {
        let catalog = Self {
            project_id,
            context_revision,
            active_edge_count,
            bound_document_count,
            projection_generation,
            initialized_at,
            updated_at,
        };
        catalog.validate()?;
        Ok(catalog)
    }

    /// Validate all catalog invariants.
    pub fn validate(&self) -> ProjectContextResult<()> {
        validate_uuid_v4(*self.project_id.as_uuid(), "project_id")?;
        validate_nonnegative(self.context_revision, "context_revision")?;
        validate_nonnegative(self.active_edge_count, "active_edge_count")?;
        validate_nonnegative(self.bound_document_count, "bound_document_count")?;
        validate_positive(self.projection_generation, "projection_generation")?;
        if self.updated_at < self.initialized_at {
            return invalid_catalog("updated_at precedes initialized_at");
        }
        if self.context_revision == 0
            && (self.active_edge_count != 0
                || self.bound_document_count != 0
                || self.updated_at != self.initialized_at)
        {
            return invalid_catalog("revision zero is reserved for the untouched empty catalog");
        }
        if self.active_edge_count > self.bound_document_count {
            return invalid_catalog("active_edge_count exceeds bound_document_count");
        }
        if (self.active_edge_count == 0) != (self.bound_document_count == 0) {
            return invalid_catalog("edge and binding emptiness must agree");
        }
        Ok(())
    }

    /// Host-derived Project identity.
    #[must_use]
    pub const fn project_id(&self) -> CommunityId {
        self.project_id
    }

    /// Global canonical Context revision.
    #[must_use]
    pub const fn context_revision(&self) -> u64 {
        self.context_revision
    }

    /// Number of active edges.
    #[must_use]
    pub const fn active_edge_count(&self) -> u64 {
        self.active_edge_count
    }

    /// Number of active one-Document bindings.
    #[must_use]
    pub const fn bound_document_count(&self) -> u64 {
        self.bound_document_count
    }

    /// Active relay projection generation.
    #[must_use]
    pub const fn projection_generation(&self) -> u64 {
        self.projection_generation
    }

    /// Canonical initialization time.
    #[must_use]
    pub const fn initialized_at(&self) -> DateTime<Utc> {
        self.initialized_at
    }

    /// Canonical time of the current catalog observation.
    #[must_use]
    pub const fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

/// One canonical binding transition emitted by the reducer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectContextBinding {
    /// Deterministic edge identity.
    pub edge_key: EdgeKey,
    /// Canonical coordinate set retained by active and deleted projections.
    pub coordinates: Vec<ProjectContextCoordinate>,
    /// Document whose one-to-one binding changed.
    pub context_document_id: Uuid,
    /// New binding lifecycle state.
    pub state: ProjectContextBindingState,
    /// Global revision committed by this transition.
    pub context_revision: u64,
    /// Canonical transition time.
    pub updated_at: DateTime<Utc>,
}

impl ProjectContextBinding {
    /// Validate the binding against a host-derived Project identity.
    pub fn validate(&self, project_id: Uuid) -> ProjectContextResult<()> {
        validate_canonical_coordinates(&self.coordinates)?;
        for coordinate in &self.coordinates {
            coordinate.validate_for_project(project_id)?;
        }
        validate_document_id(self.context_document_id)?;
        validate_positive(self.context_revision, "context_revision")?;
        if self.edge_key != EdgeKey::derive(project_id, &self.coordinates)? {
            return Err(ProjectContextError::InvalidCanonicalState {
                reason: "binding edge key does not match coordinates".to_owned(),
            });
        }
        Ok(())
    }
}

fn invalid_catalog(reason: &str) -> ProjectContextResult<()> {
    Err(ProjectContextError::InvalidCanonicalState {
        reason: reason.to_owned(),
    })
}

pub(crate) fn checked_next(value: u64) -> ProjectContextResult<u64> {
    value
        .checked_add(1)
        .filter(|next| *next <= MAX_SAFE_REVISION)
        .ok_or(ProjectContextError::RevisionExhausted)
}
