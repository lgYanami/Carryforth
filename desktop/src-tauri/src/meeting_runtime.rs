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

/// Exact canonical Human Speech Grant owned by one Desktop renewal task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MeetingGrantRenewalBinding {
    pub(crate) api_base_url: String,
    pub(crate) signer_pubkey: String,
    pub(crate) meeting_id: String,
    pub(crate) grant_id: String,
    pub(crate) hard_deadline_ms: i64,
}

struct MeetingGrantRenewalClaim {
    generation: Uuid,
    binding: MeetingGrantRenewalBinding,
    cancel: watch::Sender<bool>,
}

/// Registration returned only to the task that won the current Grant binding.
pub(crate) struct MeetingGrantRenewalRegistration {
    pub(crate) key: String,
    pub(crate) generation: Uuid,
    pub(crate) binding: MeetingGrantRenewalBinding,
    pub(crate) cancel: watch::Receiver<bool>,
}

pub(crate) enum RegisterMeetingGrantRenewal {
    Existing,
    Started(MeetingGrantRenewalRegistration),
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

/// Community/identity-bound Human Speech Grant renewal registry.
#[derive(Default)]
pub(crate) struct MeetingGrantRenewalRuntime {
    claims: Mutex<HashMap<String, MeetingGrantRenewalClaim>>,
}

impl MeetingGrantRenewalRuntime {
    pub(crate) fn register(
        &self,
        binding: MeetingGrantRenewalBinding,
    ) -> Result<RegisterMeetingGrantRenewal, String> {
        let key = format!(
            "{}\u{0}{}\u{0}{}",
            binding.api_base_url, binding.signer_pubkey, binding.meeting_id
        );
        let mut claims = self.claims.lock().map_err(|error| error.to_string())?;
        if claims
            .get(&key)
            .is_some_and(|claim| claim.binding == binding)
        {
            return Ok(RegisterMeetingGrantRenewal::Existing);
        }
        if let Some(previous) = claims.remove(&key) {
            let _ = previous.cancel.send(true);
        }
        let generation = Uuid::new_v4();
        let (cancel, cancel_rx) = watch::channel(false);
        claims.insert(
            key.clone(),
            MeetingGrantRenewalClaim {
                generation,
                binding: binding.clone(),
                cancel,
            },
        );
        Ok(RegisterMeetingGrantRenewal::Started(
            MeetingGrantRenewalRegistration {
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

    fn grant_binding(grant_byte: &str) -> MeetingGrantRenewalBinding {
        MeetingGrantRenewalBinding {
            api_base_url: "http://localhost:3000".to_string(),
            signer_pubkey: "11".repeat(32),
            meeting_id: Uuid::new_v4().to_string(),
            grant_id: grant_byte.repeat(32),
            hard_deadline_ms: 300_000,
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

    #[test]
    fn exact_grant_binding_registration_is_idempotent() {
        let runtime = MeetingGrantRenewalRuntime::default();
        let binding = grant_binding("33");
        let first = runtime.register(binding.clone());
        assert!(matches!(first, Ok(RegisterMeetingGrantRenewal::Started(_))));
        assert!(matches!(
            runtime.register(binding),
            Ok(RegisterMeetingGrantRenewal::Existing)
        ));
    }

    #[test]
    fn replacement_grant_cancels_old_generation_without_losing_new_claim() {
        let runtime = MeetingGrantRenewalRuntime::default();
        let first_binding = grant_binding("33");
        let first = match runtime.register(first_binding.clone()) {
            Ok(RegisterMeetingGrantRenewal::Started(registration)) => registration,
            _ => panic!("first Grant binding must start a renewal claim"),
        };
        let mut replacement = first_binding;
        replacement.grant_id = "44".repeat(32);
        let second = match runtime.register(replacement.clone()) {
            Ok(RegisterMeetingGrantRenewal::Started(registration)) => registration,
            _ => panic!("replacement Grant must start a renewal claim"),
        };

        assert!(*first.cancel.borrow());
        runtime.finish(&first.key, first.generation);
        assert!(matches!(
            runtime.register(replacement),
            Ok(RegisterMeetingGrantRenewal::Existing)
        ));
        assert!(!*second.cancel.borrow());
    }

    #[test]
    fn grant_cancel_all_releases_every_community_claim() {
        let runtime = MeetingGrantRenewalRuntime::default();
        let first = match runtime.register(grant_binding("33")) {
            Ok(RegisterMeetingGrantRenewal::Started(registration)) => registration,
            _ => panic!("first Grant renewal claim must start"),
        };
        let mut other = grant_binding("44");
        other.api_base_url = "https://relay.example".to_string();
        let second = match runtime.register(other.clone()) {
            Ok(RegisterMeetingGrantRenewal::Started(registration)) => registration,
            _ => panic!("second Grant renewal claim must start"),
        };

        runtime.cancel_all();

        assert!(*first.cancel.borrow());
        assert!(*second.cancel.borrow());
        assert!(matches!(
            runtime.register(other),
            Ok(RegisterMeetingGrantRenewal::Started(_))
        ));
    }
}
