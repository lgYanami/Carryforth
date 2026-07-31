//! Durable Project View v3 maintenance state-machine contract.

use serde::{Deserialize, Serialize};

/// Current operational state of one Community's Project View.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceState {
    /// Ordinary member writes, runtime admission, and capability reads may run.
    Normal,
    /// New work is stopped while exact runtime/Assignment baselines quiesce.
    Draining,
    /// Member/runtime mutation paths are fenced for cutover, verify, or repair.
    Frozen,
}

/// Closed operator actions that change the maintenance state pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceAction {
    /// Start a new immutable maintenance epoch and enter draining.
    Begin,
    /// Enter frozen after all exact baseline acknowledgements are durable.
    Freeze,
    /// Cancel a pre-commit epoch without reviving old runtime coordinates.
    Abort,
    /// Return a verified post-cutover v3 Community to normal operation.
    Resume,
}

/// A maintenance transition violated the irreversible cutover boundary.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MaintenanceContractError {
    /// The action is not an edge in the closed state machine.
    #[error("maintenance action {action:?} is invalid from state {state:?}")]
    InvalidTransition {
        /// Current durable state.
        state: MaintenanceState,
        /// Requested operator action.
        action: MaintenanceAction,
    },
    /// Abort was requested after the v3 cutover receipt committed.
    #[error("a committed v3 cutover cannot be rolled back; repair and resume forward")]
    CutoverAlreadyCommitted,
    /// Resume was requested without a committed and structurally verified v3 state.
    #[error("resume requires a committed, structurally verified v3 cutover")]
    V3NotVerified,
}

/// Apply the Stage 0 maintenance state-machine contract.
///
/// `cutover_committed` means the immutable cutover receipt exists. Once true,
/// Abort is permanently forbidden. `v3_verified` is meaningful only for
/// Resume and represents structural verification plus resolved post-cutover
/// invalidations under the exclusive Community lock.
pub fn transition_maintenance(
    state: MaintenanceState,
    action: MaintenanceAction,
    cutover_committed: bool,
    v3_verified: bool,
) -> Result<MaintenanceState, MaintenanceContractError> {
    match (state, action) {
        (MaintenanceState::Normal, MaintenanceAction::Begin) => Ok(MaintenanceState::Draining),
        (MaintenanceState::Draining, MaintenanceAction::Freeze) => Ok(MaintenanceState::Frozen),
        (MaintenanceState::Draining | MaintenanceState::Frozen, MaintenanceAction::Abort)
            if !cutover_committed =>
        {
            Ok(MaintenanceState::Normal)
        }
        (MaintenanceState::Draining | MaintenanceState::Frozen, MaintenanceAction::Abort) => {
            Err(MaintenanceContractError::CutoverAlreadyCommitted)
        }
        (MaintenanceState::Frozen, MaintenanceAction::Resume)
            if cutover_committed && v3_verified =>
        {
            Ok(MaintenanceState::Normal)
        }
        (MaintenanceState::Frozen, MaintenanceAction::Resume) => {
            Err(MaintenanceContractError::V3NotVerified)
        }
        (state, action) => Err(MaintenanceContractError::InvalidTransition { state, action }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maintenance_edges_and_irreversible_cutover_are_closed() {
        assert_eq!(
            transition_maintenance(
                MaintenanceState::Normal,
                MaintenanceAction::Begin,
                false,
                false,
            ),
            Ok(MaintenanceState::Draining)
        );
        assert_eq!(
            transition_maintenance(
                MaintenanceState::Draining,
                MaintenanceAction::Freeze,
                false,
                false,
            ),
            Ok(MaintenanceState::Frozen)
        );
        assert_eq!(
            transition_maintenance(
                MaintenanceState::Frozen,
                MaintenanceAction::Resume,
                true,
                true,
            ),
            Ok(MaintenanceState::Normal)
        );
        assert_eq!(
            transition_maintenance(
                MaintenanceState::Frozen,
                MaintenanceAction::Abort,
                true,
                false,
            ),
            Err(MaintenanceContractError::CutoverAlreadyCommitted)
        );
    }
}
