//! Process-local lifecycle for Human-hosted Meeting Action renewal claims.
//!
//! Claims contain no secret material. The renewal task re-reads the active
//! identity and Community before every signed operation, while workspace and
//! identity transitions synchronously cancel all existing claims.

use std::{collections::HashMap, sync::Mutex};

use tokio::sync::watch;
use uuid::Uuid;

/// Exact canonical Action window owned by one Human renewal task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MeetingActionRenewalBinding {
    pub(crate) api_base_url: String,
    pub(crate) signer_pubkey: String,
    pub(crate) meeting_id: String,
    pub(crate) action_run_id: String,
    pub(crate) action_window_epoch: u64,
    pub(crate) board_event_id: String,
}

struct MeetingActionRenewalClaim {
    generation: Uuid,
    binding: MeetingActionRenewalBinding,
    cancel: watch::Sender<bool>,
}

/// Registration returned only to the task that won the current binding.
pub(crate) struct MeetingActionRenewalRegistration {
    pub(crate) key: String,
    pub(crate) generation: Uuid,
    pub(crate) binding: MeetingActionRenewalBinding,
    pub(crate) cancel: watch::Receiver<bool>,
}

pub(crate) enum RegisterMeetingActionRenewal {
    Existing,
    Started(MeetingActionRenewalRegistration),
}

/// Community/identity-bound Human Action renewal registry.
#[derive(Default)]
pub(crate) struct MeetingActionRenewalRuntime {
    claims: Mutex<HashMap<String, MeetingActionRenewalClaim>>,
}

impl MeetingActionRenewalRuntime {
    pub(crate) fn register(
        &self,
        binding: MeetingActionRenewalBinding,
    ) -> Result<RegisterMeetingActionRenewal, String> {
        let key = format!(
            "{}\u{0}{}\u{0}{}",
            binding.api_base_url, binding.signer_pubkey, binding.meeting_id
        );
        let mut claims = self.claims.lock().map_err(|error| error.to_string())?;
        if claims
            .get(&key)
            .is_some_and(|claim| claim.binding == binding)
        {
            return Ok(RegisterMeetingActionRenewal::Existing);
        }
        if let Some(previous) = claims.remove(&key) {
            let _ = previous.cancel.send(true);
        }
        let generation = Uuid::new_v4();
        let (cancel, cancel_rx) = watch::channel(false);
        claims.insert(
            key.clone(),
            MeetingActionRenewalClaim {
                generation,
                binding: binding.clone(),
                cancel,
            },
        );
        Ok(RegisterMeetingActionRenewal::Started(
            MeetingActionRenewalRegistration {
                key,
                generation,
                binding,
                cancel: cancel_rx,
            },
        ))
    }

    pub(crate) fn finish(&self, key: &str, generation: Uuid) {
        if let Ok(mut claims) = self.claims.lock() {
            if claims
                .get(key)
                .is_some_and(|claim| claim.generation == generation)
            {
                claims.remove(key);
            }
        }
    }

    pub(crate) fn cancel_all(&self) {
        if let Ok(mut claims) = self.claims.lock() {
            for (_, claim) in claims.drain() {
                let _ = claim.cancel.send(true);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(window: u64) -> MeetingActionRenewalBinding {
        MeetingActionRenewalBinding {
            api_base_url: "http://localhost:3000".to_string(),
            signer_pubkey: "11".repeat(32),
            meeting_id: Uuid::new_v4().to_string(),
            action_run_id: Uuid::new_v4().to_string(),
            action_window_epoch: window,
            board_event_id: "22".repeat(32),
        }
    }

    #[test]
    fn exact_binding_registration_is_idempotent() {
        let runtime = MeetingActionRenewalRuntime::default();
        let binding = binding(1);
        let first = runtime.register(binding.clone());
        assert!(matches!(
            first,
            Ok(RegisterMeetingActionRenewal::Started(_))
        ));
        assert!(matches!(
            runtime.register(binding),
            Ok(RegisterMeetingActionRenewal::Existing)
        ));
    }

    #[test]
    fn newer_window_cancels_the_previous_claim_and_old_finish_is_fenced() {
        let runtime = MeetingActionRenewalRuntime::default();
        let first_binding = binding(1);
        let first = match runtime.register(first_binding.clone()) {
            Ok(RegisterMeetingActionRenewal::Started(registration)) => registration,
            _ => panic!("first exact binding must start a renewal claim"),
        };
        let mut second_binding = first_binding;
        second_binding.action_window_epoch = 2;
        let second = match runtime.register(second_binding.clone()) {
            Ok(RegisterMeetingActionRenewal::Started(registration)) => registration,
            _ => panic!("a newer window must replace the old renewal claim"),
        };

        assert!(*first.cancel.borrow());
        runtime.finish(&first.key, first.generation);
        assert!(matches!(
            runtime.register(second_binding),
            Ok(RegisterMeetingActionRenewal::Existing)
        ));
        assert!(!*second.cancel.borrow());
    }

    #[test]
    fn cancel_all_releases_every_community_claim() {
        let runtime = MeetingActionRenewalRuntime::default();
        let first = match runtime.register(binding(1)) {
            Ok(RegisterMeetingActionRenewal::Started(registration)) => registration,
            _ => panic!("first renewal claim must start"),
        };
        let mut other = binding(1);
        other.api_base_url = "https://relay.example".to_string();
        let second = match runtime.register(other.clone()) {
            Ok(RegisterMeetingActionRenewal::Started(registration)) => registration,
            _ => panic!("second renewal claim must start"),
        };

        runtime.cancel_all();

        assert!(*first.cancel.borrow());
        assert!(*second.cancel.borrow());
        assert!(matches!(
            runtime.register(other),
            Ok(RegisterMeetingActionRenewal::Started(_))
        ));
    }
}
