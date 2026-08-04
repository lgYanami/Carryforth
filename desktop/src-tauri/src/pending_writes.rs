use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
};

/// One already-signed Human Meeting Create retained for an exact retry after
/// an indeterminate Relay response. It contains no private key material.
#[derive(Clone)]
pub(crate) struct PendingMeetingCreate {
    pub(crate) event: nostr::Event,
    pub(crate) api_base_url: String,
    pub(crate) signer_pubkey: String,
    pub(crate) meeting_id: String,
    pub(crate) fingerprint: String,
}

/// One already-signed Human Meeting Floor, host, or action-finalization command
/// retained for exact retry. The event and its binding metadata contain no
/// private key material.
#[derive(Clone)]
pub(crate) struct PendingMeetingCommand {
    pub(crate) event: nostr::Event,
    pub(crate) api_base_url: String,
    pub(crate) signer_pubkey: String,
    pub(crate) meeting_id: String,
    pub(crate) fingerprint: String,
    pub(crate) action: String,
}

/// Process-local state for writes whose Relay projection or response can
/// arrive after the initiating command returns.
#[derive(Default)]
pub(crate) struct PendingWrites {
    /// `(creator pubkey, channel ID)` until the Relay's kind:39002 owner
    /// projection is observed. Identity binding prevents cross-user overlays.
    pub(crate) owned_channels: Mutex<HashSet<(String, String)>>,
    /// Stable submission ID to signed Meeting Create. The command layer bounds
    /// the map and validates both target Community and signer on every retry.
    pub(crate) meeting_creates: Mutex<HashMap<String, PendingMeetingCreate>>,
    /// Stable submission ID to a signed Meeting Floor, host, or
    /// action-finalization command.
    pub(crate) meeting_commands: Mutex<HashMap<String, PendingMeetingCommand>>,
}
