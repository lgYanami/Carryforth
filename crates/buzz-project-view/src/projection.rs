//! Wire-neutral plans for relay-signed Project View projections.

use std::collections::BTreeSet;

use buzz_core::CommunityId;
use chrono::{DateTime, Utc};

use crate::{
    DomainError, DomainResult, MutationOutcome, ProjectViewEntry, ProjectViewState,
    MAX_SAFE_REVISION,
};

/// Canonical inputs shared by Relay mutation projection and maintenance
/// reprojection.
///
/// This type deliberately contains no Nostr tags, event builders, keys, or
/// signatures. The SDK turns the plan into wire events while the domain crate
/// remains pure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionPlan {
    project_id: CommunityId,
    projection_generation: u64,
    project_revision: u64,
    active_object_count: u32,
    updated_at: DateTime<Utc>,
    source_event_id: Option<[u8; 32]>,
    reset: bool,
    entries: Vec<ProjectViewEntry>,
}

impl ProjectionPlan {
    /// Build the projection plan for one accepted member mutation.
    pub fn for_mutation(
        state: &ProjectViewState,
        outcome: &MutationOutcome,
        source_event_id: [u8; 32],
        projection_generation: u64,
    ) -> DomainResult<Self> {
        validate_generation(projection_generation)?;
        state.validate()?;
        if !state.is_initialized() {
            return Err(DomainError::NotInitialized);
        }
        if state.project_revision() != outcome.project_revision {
            return Err(DomainError::InvalidFinalState {
                reason: "projection outcome revision differs from canonical state".to_owned(),
            });
        }
        if outcome.changed_entries.is_empty() {
            return Err(DomainError::InvalidFinalState {
                reason: "a mutation projection must contain at least one changed entry".to_owned(),
            });
        }

        let mut changed_ids = BTreeSet::new();
        for entry in &outcome.changed_entries {
            if !changed_ids.insert(entry.id()) {
                return Err(DomainError::InvalidFinalState {
                    reason: "mutation projection contains a duplicate changed object".to_owned(),
                });
            }
            if entry.project_revision() != outcome.project_revision
                || state.entry(entry.id()) != Some(entry)
            {
                return Err(DomainError::InvalidFinalState {
                    reason:
                        "mutation projection entry does not match the canonical resulting state"
                            .to_owned(),
                });
            }
        }

        Self::new(
            state,
            projection_generation,
            Some(source_event_id),
            false,
            outcome.changed_entries.clone(),
        )
    }

    /// Build a reset plan that re-signs every occupied object identity without
    /// changing the project revision.
    pub fn for_reprojection(
        state: &ProjectViewState,
        projection_generation: u64,
    ) -> DomainResult<Self> {
        validate_generation(projection_generation)?;
        state.validate()?;
        if !state.is_initialized() {
            return Err(DomainError::NotInitialized);
        }
        Self::new(
            state,
            projection_generation,
            None,
            true,
            state.entries().values().cloned().collect(),
        )
    }

    fn new(
        state: &ProjectViewState,
        projection_generation: u64,
        source_event_id: Option<[u8; 32]>,
        reset: bool,
        entries: Vec<ProjectViewEntry>,
    ) -> DomainResult<Self> {
        let active_object_count = u32::try_from(state.active_objects().count()).map_err(|_| {
            DomainError::InvalidFinalState {
                reason: "active object count exceeds the supported range".to_owned(),
            }
        })?;
        let updated_at = state.updated_at().ok_or(DomainError::NotInitialized)?;
        Ok(Self {
            project_id: state.project_id(),
            projection_generation,
            project_revision: state.project_revision(),
            active_object_count,
            updated_at,
            source_event_id,
            reset,
            entries,
        })
    }

    /// Return the server-resolved Project/Community identity.
    #[must_use]
    pub const fn project_id(&self) -> CommunityId {
        self.project_id
    }

    /// Return the relay projection generation.
    #[must_use]
    pub const fn projection_generation(&self) -> u64 {
        self.projection_generation
    }

    /// Return the current canonical project revision.
    #[must_use]
    pub const fn project_revision(&self) -> u64 {
        self.project_revision
    }

    /// Return the number of active canonical objects.
    #[must_use]
    pub const fn active_object_count(&self) -> u32 {
        self.active_object_count
    }

    /// Return the canonical timestamp of the current project revision.
    #[must_use]
    pub const fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    /// Return the accepted member command that caused this projection, absent
    /// for maintenance resets.
    #[must_use]
    pub const fn source_event_id(&self) -> Option<[u8; 32]> {
        self.source_event_id
    }

    /// Return whether clients must discard the preceding projection generation.
    #[must_use]
    pub const fn reset(&self) -> bool {
        self.reset
    }

    /// Return the object entries that must be re-signed for this plan.
    #[must_use]
    pub fn entries(&self) -> &[ProjectViewEntry] {
        &self.entries
    }
}

fn validate_generation(generation: u64) -> DomainResult<()> {
    if generation == 0 || generation > MAX_SAFE_REVISION {
        return Err(DomainError::RevisionOutOfRange {
            revision: generation,
            max: MAX_SAFE_REVISION,
        });
    }
    Ok(())
}
