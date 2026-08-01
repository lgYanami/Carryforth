//! Multi-pod scheduler for trusted managed-runtime recovery exhaustion.
//!
//! The scheduler is a deployment-level kill-switched consumer of database
//! claims. The database transaction remains the correctness boundary: every
//! candidate is revalidated under lock before any Project change is committed.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::MissedTickBehavior;
use tracing::{error, info, warn};

use crate::state::AppState;

/// Run the automatic unrecoverable scheduler until the Relay shuts down.
pub async fn run(state: Arc<AppState>) {
    if !state.config.runtime_unrecoverable_enabled {
        return;
    }
    let interval_secs = state.config.runtime_supervision_interval_secs;
    let batch_limit = state.config.runtime_supervision_batch_limit;
    let claim_duration = Duration::from_secs(state.config.runtime_supervision_claim_secs);
    let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    interval.tick().await;
    info!(
        interval_secs,
        batch_limit,
        claim_secs = claim_duration.as_secs(),
        "trusted runtime supervision scheduler started"
    );

    loop {
        interval.tick().await;
        let claims = match state
            .db
            .claim_unrecoverable_runtime_assignments(batch_limit, claim_duration)
            .await
        {
            Ok(claims) => claims,
            Err(error) => {
                metrics::counter!("buzz_project_runtime_scheduler_errors_total").increment(1);
                error!(%error, "runtime supervision claim sweep failed");
                continue;
            }
        };
        if claims.is_empty() {
            continue;
        }
        metrics::counter!("buzz_project_runtime_scheduler_claims_total")
            .increment(claims.len() as u64);

        for claim in claims {
            match state
                .db
                .end_unrecoverable_assignment(&claim, &state.relay_keypair)
                .await
            {
                Ok(outcome) => {
                    if !outcome.replayed {
                        let tenant = buzz_core::TenantContext::resolved(
                            claim.community_id,
                            claim.community_host.clone(),
                        );
                        crate::handlers::project_view::dispatch_v2_committed_events(
                            &tenant,
                            &state,
                            &outcome.events,
                        )
                        .await;
                        metrics::counter!(
                            "buzz_project_runtime_assignment_ended_total",
                            "community" => claim.community_host.clone()
                        )
                        .increment(1);
                        metrics::counter!(
                            "buzz_role_runtime_recovery_total",
                            "result" => "assignment_ended"
                        )
                        .increment(1);
                        info!(
                            community = %claim.community_id,
                            assignment = %claim.assignment_id,
                            binding = %claim.binding_id,
                            project_revision = outcome.project_revision,
                            "trusted runtime recovery exhausted; Assignment ended"
                        );
                    }
                }
                Err(error) => {
                    metrics::counter!("buzz_project_runtime_scheduler_errors_total").increment(1);
                    metrics::counter!(
                        "buzz_role_runtime_recovery_total",
                        "result" => "terminal_rejected"
                    )
                    .increment(1);
                    warn!(
                        community = %claim.community_id,
                        assignment = %claim.assignment_id,
                        binding = %claim.binding_id,
                        %error,
                        "runtime terminal transaction rejected; releasing claim"
                    );
                    match state.db.release_unrecoverable_runtime_claim(&claim).await {
                        Ok(true) => {}
                        Ok(false) => warn!(
                            community = %claim.community_id,
                            binding = %claim.binding_id,
                            "runtime scheduler claim was already replaced or finalized"
                        ),
                        Err(release_error) => error!(
                            community = %claim.community_id,
                            binding = %claim.binding_id,
                            error = %release_error,
                            "failed to release runtime scheduler claim"
                        ),
                    }
                }
            }
        }
    }
}
