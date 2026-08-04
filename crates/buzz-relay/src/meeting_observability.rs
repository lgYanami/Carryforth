//! Low-cardinality Meeting V2 read observations shared by HTTP and WebSocket.

use std::time::Instant;

use nostr::{Filter, Kind};

const BOARD_KIND: Kind = Kind::Custom(buzz_core::kind::KIND_MEETING_BOARD as u16);

/// One request-scoped current-Board metric guard.
///
/// The guard defaults to `error`, so parser failures, early connection loss,
/// panics and newly-added return paths cannot silently disappear from the
/// denominator. Callers explicitly mark authorization denials and successful
/// empty/non-empty responses.
pub(crate) struct MeetingV2BoardReadObservation {
    enabled: bool,
    transport: &'static str,
    outcome: &'static str,
    started_at: Instant,
}

impl MeetingV2BoardReadObservation {
    pub(crate) fn for_filters(transport: &'static str, filters: &[Filter]) -> Self {
        Self {
            enabled: filters_target_only_current_board(filters),
            transport,
            outcome: "error",
            started_at: Instant::now(),
        }
    }

    pub(crate) fn for_raw_filters(transport: &'static str, body: &[u8]) -> Self {
        let enabled = serde_json::from_slice::<Vec<Filter>>(body)
            .ok()
            .is_some_and(|filters| filters_target_only_current_board(&filters));
        Self {
            enabled,
            transport,
            outcome: "error",
            started_at: Instant::now(),
        }
    }

    pub(crate) fn denied(&mut self) {
        self.outcome = "denied";
    }

    pub(crate) fn completed(&mut self, result_count: usize) {
        self.outcome = if result_count == 0 {
            "not_found"
        } else {
            "success"
        };
    }

    pub(crate) fn failed(&mut self) {
        self.outcome = "error";
    }
}

impl Drop for MeetingV2BoardReadObservation {
    fn drop(&mut self) {
        if !self.enabled {
            return;
        }
        metrics::counter!(
            "meeting_v2_board_read_total",
            "transport" => self.transport,
            "outcome" => self.outcome
        )
        .increment(1);
        metrics::histogram!(
            "meeting_v2_board_read_latency_seconds",
            "transport" => self.transport,
            "outcome" => self.outcome
        )
        .record(self.started_at.elapsed().as_secs_f64());
    }
}

fn filters_target_only_current_board(filters: &[Filter]) -> bool {
    !filters.is_empty()
        && filters.iter().all(|filter| {
            filter
                .kinds
                .as_ref()
                .is_some_and(|kinds| kinds.len() == 1 && kinds.contains(&BOARD_KIND))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{Alphabet, SingleLetterTag};

    #[test]
    fn board_observation_requires_an_explicit_exclusive_kind_filter() {
        let session = uuid::Uuid::new_v4().to_string();
        let board = Filter::new()
            .kind(BOARD_KIND)
            .custom_tag(SingleLetterTag::lowercase(Alphabet::H), session);
        assert!(filters_target_only_current_board(std::slice::from_ref(
            &board,
        )));
        assert!(!filters_target_only_current_board(&[]));
        assert!(!filters_target_only_current_board(&[Filter::new()]));
        assert!(!filters_target_only_current_board(&[
            board.clone(),
            Filter::new().kind(Kind::TextNote),
        ]));
        assert!(!filters_target_only_current_board(&[
            Filter::new().kinds([BOARD_KIND, Kind::TextNote]),
        ]));
    }

    #[test]
    fn raw_filter_detection_fails_closed_without_panicking() {
        let board = serde_json::json!([{
            "kinds": [buzz_core::kind::KIND_MEETING_BOARD],
            "#h": [uuid::Uuid::new_v4().to_string()],
            "limit": 10
        }]);
        let observed = MeetingV2BoardReadObservation::for_raw_filters(
            "http",
            &serde_json::to_vec(&board).expect("serialize filter"),
        );
        assert!(observed.enabled);
        let malformed = MeetingV2BoardReadObservation::for_raw_filters("http", b"not-json");
        assert!(!malformed.enabled);
    }
}
