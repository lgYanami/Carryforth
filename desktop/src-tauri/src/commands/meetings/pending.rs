//! Shared exact-retry storage for signed Meeting commands.

use nostr::Event;

use crate::{app_state::AppState, pending_writes::PendingMeetingCommand};

const MAX_PENDING_MEETING_COMMANDS: usize = 64;

pub(super) struct PendingBinding<'a> {
    pub(super) submission_id: &'a str,
    pub(super) meeting_id: &'a str,
    pub(super) fingerprint: &'a str,
    pub(super) api_base_url: &'a str,
    pub(super) signer_pubkey: &'a str,
    pub(super) context: &'a str,
}

pub(super) fn find_pending(
    state: &AppState,
    binding: &PendingBinding<'_>,
) -> Result<Option<PendingMeetingCommand>, String> {
    let pending = state
        .pending_writes
        .meeting_commands
        .lock()
        .map_err(|error| error.to_string())?;
    let Some(existing) = pending.get(binding.submission_id) else {
        return Ok(None);
    };
    validate_pending_binding(existing, binding)?;
    Ok(Some(existing.clone()))
}

pub(super) fn insert_or_reuse_pending(
    state: &AppState,
    prepared: PendingMeetingCommand,
    binding: &PendingBinding<'_>,
) -> Result<PendingMeetingCommand, String> {
    let mut pending = state
        .pending_writes
        .meeting_commands
        .lock()
        .map_err(|error| error.to_string())?;
    if let Some(existing) = pending.get(binding.submission_id) {
        validate_pending_binding(existing, binding)?;
        return Ok(existing.clone());
    }
    if pending.len() >= MAX_PENDING_MEETING_COMMANDS {
        return Err(format!(
            "too many unresolved {} submissions; retry an existing submission first",
            binding.context
        ));
    }
    pending.insert(binding.submission_id.to_string(), prepared.clone());
    Ok(prepared)
}

pub(super) fn validate_pending_binding(
    pending: &PendingMeetingCommand,
    binding: &PendingBinding<'_>,
) -> Result<(), String> {
    if pending.fingerprint != binding.fingerprint {
        return Err(format!(
            "{} submission ID is already bound to a different action",
            binding.context
        ));
    }
    if pending.meeting_id != binding.meeting_id || pending.api_base_url != binding.api_base_url {
        return Err(format!(
            "{} submission belongs to a different Community or Meeting; switch back before retrying",
            binding.context
        ));
    }
    if pending.signer_pubkey != binding.signer_pubkey {
        return Err(format!(
            "{} submission belongs to a different identity; restore that identity before retrying",
            binding.context
        ));
    }
    Ok(())
}

pub(super) fn remove_pending(state: &AppState, submission_id: &str, event: &Event) {
    if let Ok(mut pending) = state.pending_writes.meeting_commands.lock() {
        if pending
            .get(submission_id)
            .is_some_and(|candidate| candidate.event.id == event.id)
        {
            pending.remove(submission_id);
        }
    }
}

pub(super) fn is_indeterminate_submit_error(message: &str) -> bool {
    message.starts_with("relay unreachable:")
        || message.starts_with("relay returned malformed response:")
        || message.starts_with("relay returned 408")
        || message.starts_with("relay returned 5")
}

pub(super) fn canonical_uuid(value: &str, context: &str) -> Result<String, String> {
    let parsed = uuid::Uuid::parse_str(value).map_err(|_| format!("{context} must be a UUID"))?;
    if parsed.is_nil() || parsed.to_string() != value {
        return Err(format!("{context} must be a canonical non-nil UUID"));
    }
    Ok(value.to_string())
}

pub(super) fn canonical_hex64(value: &str, context: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!("{context} must be canonical lowercase hex"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_identifiers_reject_noncanonical_values() {
        assert!(canonical_uuid("00000000-0000-4000-8000-000000000001", "submission").is_ok());
        assert!(canonical_uuid("00000000-0000-0000-0000-000000000000", "submission").is_err());
        assert!(canonical_hex64(&"ab".repeat(32), "event").is_ok());
        assert!(canonical_hex64(&"AB".repeat(32), "event").is_err());
    }
}
