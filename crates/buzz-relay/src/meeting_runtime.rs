//! Restart-safe Meeting V0 deadline and delivery runtime.

use std::sync::Arc;
use std::time::Duration;

use buzz_core::tenant::TenantContext;
use buzz_db::meeting_floor::{FloorConfig, WinnerSelector};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::handlers::event::dispatch_persistent_event_now;
use crate::state::AppState;

const DEFAULT_SWEEP_INTERVAL: Duration = Duration::from_millis(100);
const DEFAULT_OUTBOX_LEASE: Duration = Duration::from_secs(30);
const DEFAULT_BATCH_LIMIT: i64 = 100;

/// Load the configured Claim window and Grant lease.
///
/// Values are milliseconds so integration tests can exercise deadlines without
/// waiting for production defaults. Invalid or out-of-range values fall back to
/// the protocol defaults enforced by `buzz-db`.
pub(crate) fn floor_config_from_env() -> FloorConfig {
    FloorConfig {
        claim_window: env_duration_ms(
            "BUZZ_MEETING_CLAIM_WINDOW_MS",
            buzz_db::meeting_floor::DEFAULT_CLAIM_WINDOW,
        ),
        grant_lease: env_duration_ms(
            "BUZZ_MEETING_GRANT_LEASE_MS",
            buzz_db::meeting_floor::DEFAULT_GRANT_LEASE,
        ),
    }
}

/// Run deadline recovery and transactional outbox delivery for Meeting V0.
pub async fn run(state: Arc<AppState>) {
    let floor_config = floor_config_from_env();
    let sweep_interval = env_duration_ms("BUZZ_MEETING_SWEEP_INTERVAL_MS", DEFAULT_SWEEP_INTERVAL);
    let outbox_lease = env_duration_ms("BUZZ_MEETING_OUTBOX_LEASE_MS", DEFAULT_OUTBOX_LEASE);
    let batch_limit = std::env::var("BUZZ_MEETING_BATCH_LIMIT")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| (1..=1000).contains(value))
        .unwrap_or(DEFAULT_BATCH_LIMIT);
    let worker_id = Uuid::new_v4();

    info!(
        claim_window_ms = floor_config.claim_window.as_millis(),
        grant_lease_ms = floor_config.grant_lease.as_millis(),
        sweep_interval_ms = sweep_interval.as_millis(),
        batch_limit,
        "Meeting V0 runtime started"
    );

    let mut ticker = tokio::time::interval(sweep_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;

        if let Err(error) = buzz_db::meeting_floor::recover_due_floors(
            &state.db,
            &state.relay_keypair,
            floor_config,
            WinnerSelector::UniformRandom,
            batch_limit,
        )
        .await
        {
            error!("Meeting V0 floor recovery failed: {error}");
        }

        if let Err(error) =
            dispatch_outbox_batch(&state, worker_id, outbox_lease, batch_limit).await
        {
            error!("Meeting V0 outbox delivery failed: {error}");
        }
    }
}

async fn dispatch_outbox_batch(
    state: &Arc<AppState>,
    worker_id: Uuid,
    lease: Duration,
    limit: i64,
) -> Result<(), buzz_db::DbError> {
    let events =
        buzz_db::meeting_floor::claim_outbox_batch(&state.db, worker_id, lease, limit).await?;
    for item in events {
        let tenant = TenantContext::resolved(item.community_id, item.host);
        let kind = item.stored_event.event.kind.as_u16() as u32;
        let actor = item.stored_event.event.pubkey.to_hex();
        dispatch_persistent_event_now(&tenant, state, &item.stored_event, kind, &actor, None).await;
        if !buzz_db::meeting_floor::mark_outbox_delivered(
            &state.db,
            item.community_id,
            item.sequence,
            worker_id,
        )
        .await?
        {
            warn!(
                meeting = %item.session_id,
                sequence = item.sequence,
                "Meeting V0 outbox claim was lost before delivery acknowledgement"
            );
        }
    }
    Ok(())
}

fn env_duration_ms(name: &str, default: Duration) -> Duration {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| (1..=300_000).contains(value))
        .map(Duration::from_millis)
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_duration_uses_default() {
        let name = format!("BUZZ_MEETING_TEST_DURATION_{}", Uuid::new_v4());
        assert_eq!(
            env_duration_ms(&name, Duration::from_millis(17)),
            Duration::from_millis(17)
        );
    }
}
