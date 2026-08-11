//! Capability-gated Project Context overview embedding worker.
//!
//! This runtime never serves semantic query results. It consumes only durable
//! jobs for explicitly enabled Communities and publishes complete derived
//! source-generation heads after source and claim CAS validation.

use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

use buzz_db::semantic::{
    SemanticActivationOutcome, SemanticClaimObservationOutcome, SemanticJobLease,
    SemanticProviderReservation, SemanticProviderWorkload, SemanticWorkerEgressConfirmation,
};
use buzz_semantic::{
    extract_overview, DeterministicFakeEncoder, EncodedSemanticUnit, SemanticEncoder,
    SemanticEncoderInput, SemanticError, SemanticProviderBoundary,
};
use tracing::{info, warn};

use crate::{semantic_provider::VolcengineSemanticProvider, AppState};

const IDLE_POLL_FLOOR: Duration = Duration::from_millis(250);
const IDLE_POLL_CEILING: Duration = Duration::from_secs(5);
const METRICS_INTERVAL: Duration = Duration::from_secs(30);

/// Continuously process capability-gated overview jobs using the writer DB.
pub async fn run(state: Arc<AppState>) {
    let provider = match state.semantic_provider() {
        Ok(provider) => provider,
        Err(error) => {
            warn!(error = %error, "semantic provider unavailable");
            None
        }
    };
    let mut idle_delay = IDLE_POLL_FLOOR;
    let mut next_metrics_at = tokio::time::Instant::now();
    info!("Project Context semantic worker started");
    while !state.shutting_down.load(Ordering::Relaxed) {
        if tokio::time::Instant::now() >= next_metrics_at {
            record_runtime_metrics(&state).await;
            next_metrics_at = tokio::time::Instant::now() + METRICS_INTERVAL;
        }
        match state
            .db
            .claim_due_semantic_job(state.config.semantic_worker.claim_seconds)
            .await
        {
            Ok(Some(lease)) => {
                idle_delay = IDLE_POLL_FLOOR;
                metrics::counter!("buzz_semantic_jobs_claimed_total").increment(1);
                process_claim(&state, provider, lease).await;
            }
            Ok(None) => {
                tokio::time::sleep(idle_delay).await;
                idle_delay = (idle_delay * 2).min(IDLE_POLL_CEILING);
            }
            Err(error) => {
                warn!(error = %error, "semantic job claim failed");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
    info!("Project Context semantic worker stopped");
}

async fn record_runtime_metrics(state: &AppState) {
    match state.db.semantic_runtime_metrics().await {
        Ok(snapshot) => {
            metrics::gauge!("buzz_semantic_sources_eligible").set(snapshot.eligible_sources as f64);
            metrics::gauge!("buzz_semantic_sources_current").set(snapshot.current_sources as f64);
            metrics::gauge!("buzz_semantic_jobs_queued").set(snapshot.queued_jobs as f64);
            metrics::gauge!("buzz_semantic_jobs_claimed").set(snapshot.claimed_jobs as f64);
            metrics::gauge!("buzz_semantic_jobs_poison").set(snapshot.poison_jobs as f64);
            metrics::gauge!("buzz_semantic_oldest_due_seconds").set(snapshot.oldest_due_seconds);
        }
        Err(error) => warn!(error = %error, "semantic metrics snapshot failed"),
    }
}

async fn process_claim(
    state: &AppState,
    provider: Option<&VolcengineSemanticProvider>,
    lease: SemanticJobLease,
) {
    let result = process_claim_inner(state, provider, &lease).await;
    if let Err(error) = result {
        let code = semantic_error_code(&error);
        let retry_after = semantic_retry_after(&error, lease.attempts);
        if let Err(retry_error) = state
            .db
            .retry_semantic_claim(
                &lease,
                retry_after,
                state.config.semantic_worker.max_attempts,
                code,
                None,
            )
            .await
        {
            warn!(error = %retry_error, error_code = code, "semantic claim retry failed");
        } else {
            metrics::counter!("buzz_semantic_jobs_deferred_total", "reason" => code).increment(1);
            warn!(error_code = code, "semantic claim deferred");
        }
    }
}

async fn process_claim_inner(
    state: &AppState,
    provider: Option<&VolcengineSemanticProvider>,
    lease: &SemanticJobLease,
) -> Result<(), SemanticWorkerError> {
    let observation = state.db.observe_semantic_source(&lease.source).await?;
    match state
        .db
        .prepare_semantic_claim_observation(lease, &observation)
        .await?
    {
        SemanticClaimObservationOutcome::Ready => {}
        SemanticClaimObservationOutcome::Ineligible
        | SemanticClaimObservationOutcome::Superseded => return Ok(()),
    }
    let unit = extract_overview(&observation)?;
    if unit.identity.extractor_version != lease.extractor_version {
        return Err(SemanticWorkerError::Contract);
    }
    let encoded = if let Some(reused) = state
        .db
        .reusable_semantic_embedding(
            buzz_core::CommunityId::from_uuid(lease.source.community_id),
            lease.generation_id,
            &unit,
            lease.model_contract_digest,
        )
        .await?
    {
        metrics::counter!("buzz_semantic_embedding_reuse_total").increment(1);
        EncodedSemanticUnit::new(
            &unit,
            reused.response_model,
            reused.values,
            &lease.model_contract,
        )?
    } else {
        let input = SemanticEncoderInput::from_unit(&unit);
        let mut output = match &lease.model_contract.provider_boundary {
            SemanticProviderBoundary::DeterministicFake => {
                let encoder = DeterministicFakeEncoder::new(lease.model_contract.dimensions)?;
                if encoder.contract() != &lease.model_contract {
                    return Err(SemanticWorkerError::Contract);
                }
                encoder.encode(&[input]).await?
            }
            SemanticProviderBoundary::External(_) => {
                let provider = provider.ok_or(SemanticWorkerError::ProviderUnavailable)?;
                if provider.contract() != &lease.model_contract {
                    return Err(SemanticWorkerError::Contract);
                }
                let provider_budget =
                    chrono::Duration::from_std(state.config.semantic_worker.request_timeout)
                        .map_err(|_| SemanticWorkerError::Contract)?;
                let latest_start_at = lease.lease_until - provider_budget;
                let reservation = state
                    .db
                    .try_reserve_semantic_provider_slot_until(
                        buzz_core::CommunityId::from_uuid(lease.source.community_id),
                        &lease.model_contract.provider,
                        SemanticProviderWorkload::BackgroundIndex,
                        state.config.semantic_worker.request_interval,
                        latest_start_at,
                    )
                    .await?;
                let SemanticProviderReservation::Reserved { wait } = reservation else {
                    return Err(SemanticWorkerError::ProviderBusy);
                };
                tokio::time::sleep(wait).await;
                let _egress_permit = match state
                    .db
                    .confirm_semantic_worker_egress(lease, &observation)
                    .await?
                {
                    SemanticWorkerEgressConfirmation::Permitted(permit) => permit,
                    SemanticWorkerEgressConfirmation::Unavailable => {
                        metrics::counter!(
                            "buzz_semantic_provider_egress_rejected_total",
                            "workload" => "background_index"
                        )
                        .increment(1);
                        return Ok(());
                    }
                };
                metrics::histogram!("buzz_semantic_provider_input_bytes")
                    .record(input.text().len() as f64);
                let started = std::time::Instant::now();
                let encoded = provider.encode(&[input]).await;
                metrics::histogram!("buzz_semantic_provider_request_seconds")
                    .record(started.elapsed().as_secs_f64());
                match encoded {
                    Ok(encoded) => {
                        metrics::counter!(
                            "buzz_semantic_provider_requests_total",
                            "result" => "success"
                        )
                        .increment(1);
                        encoded
                    }
                    Err(error) => {
                        metrics::counter!(
                            "buzz_semantic_provider_requests_total",
                            "result" => "error"
                        )
                        .increment(1);
                        return Err(error.into());
                    }
                }
            }
        };
        if output.len() != 1 {
            return Err(SemanticWorkerError::Contract);
        }
        output.remove(0)
    };
    match state
        .db
        .activate_semantic_claim(lease, &observation, &[unit], &[encoded])
        .await?
    {
        SemanticActivationOutcome::Activated => {
            metrics::counter!("buzz_semantic_heads_activated_total").increment(1);
            Ok(())
        }
        SemanticActivationOutcome::Superseded => {
            metrics::counter!("buzz_semantic_cas_superseded_total").increment(1);
            Ok(())
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum SemanticWorkerError {
    #[error("database operation failed")]
    Database(#[from] buzz_db::DbError),
    #[error("semantic contract failed")]
    Semantic(#[from] SemanticError),
    #[error("configured provider is unavailable")]
    ProviderUnavailable,
    #[error("provider admission is busy")]
    ProviderBusy,
    #[error("job contract does not match the worker")]
    Contract,
}

fn semantic_error_code(error: &SemanticWorkerError) -> &'static str {
    match error {
        SemanticWorkerError::Database(_) => "database",
        SemanticWorkerError::ProviderUnavailable => "provider_unavailable",
        SemanticWorkerError::ProviderBusy => "provider_busy",
        SemanticWorkerError::Contract => "contract_mismatch",
        SemanticWorkerError::Semantic(SemanticError::ProviderRateLimited { .. }) => {
            "provider_rate_limited"
        }
        SemanticWorkerError::Semantic(SemanticError::ProviderTransport) => "provider_transport",
        SemanticWorkerError::Semantic(SemanticError::ProviderRetryable { .. }) => {
            "provider_retryable"
        }
        SemanticWorkerError::Semantic(SemanticError::ProviderRejected { .. }) => {
            "provider_rejected"
        }
        SemanticWorkerError::Semantic(SemanticError::ProviderResponse) => "provider_response",
        SemanticWorkerError::Semantic(_) => "semantic_validation",
    }
}

fn semantic_retry_after(error: &SemanticWorkerError, attempts: u32) -> u32 {
    if let SemanticWorkerError::Semantic(SemanticError::ProviderRateLimited {
        retry_after_seconds: Some(seconds),
    }) = error
    {
        return u32::try_from(*seconds).unwrap_or(u32::MAX).clamp(1, 3_600);
    }
    2_u32.saturating_pow(attempts.min(10)).min(900)
}

#[cfg(test)]
mod tests {
    use super::{semantic_error_code, semantic_retry_after, SemanticWorkerError};
    use buzz_semantic::SemanticError;

    #[test]
    fn provider_errors_are_content_free_and_bounded() {
        let error = SemanticWorkerError::Semantic(SemanticError::ProviderRateLimited {
            retry_after_seconds: Some(123),
        });
        assert_eq!(semantic_error_code(&error), "provider_rate_limited");
        assert_eq!(semantic_retry_after(&error, 1), 123);
    }
}
