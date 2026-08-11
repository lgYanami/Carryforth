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
};
use buzz_semantic::{
    extract_overview, DeterministicFakeEncoder, EncodedSemanticUnit, SemanticEncoder,
    SemanticEncoderFuture, SemanticEncoderInput, SemanticError, SemanticModelContract,
    SemanticProviderBoundary, SemanticUnitKind,
};
use reqwest::{header::RETRY_AFTER, StatusCode, Url};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::{config::SemanticWorkerConfig, AppState};

const IDLE_POLL_FLOOR: Duration = Duration::from_millis(250);
const IDLE_POLL_CEILING: Duration = Duration::from_secs(5);
const METRICS_INTERVAL: Duration = Duration::from_secs(30);

/// Continuously process capability-gated overview jobs using the writer DB.
pub async fn run(state: Arc<AppState>) {
    let provider = match VolcengineEncoder::from_config(&state.config.semantic_worker) {
        Ok(provider) => provider,
        Err(error) => {
            warn!(
                error_code = semantic_error_code(&error),
                "semantic provider unavailable"
            );
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
                process_claim(&state, provider.as_ref(), lease).await;
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
    provider: Option<&VolcengineEncoder>,
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
    provider: Option<&VolcengineEncoder>,
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
                let wait = state
                    .db
                    .reserve_semantic_provider_slot(
                        buzz_core::CommunityId::from_uuid(lease.source.community_id),
                        &lease.model_contract.provider,
                        state.config.semantic_worker.request_interval,
                    )
                    .await?;
                tokio::time::sleep(wait).await;
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
    #[error("job contract does not match the worker")]
    Contract,
}

fn semantic_error_code(error: &SemanticWorkerError) -> &'static str {
    match error {
        SemanticWorkerError::Database(_) => "database",
        SemanticWorkerError::ProviderUnavailable => "provider_unavailable",
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

struct VolcengineEncoder {
    client: reqwest::Client,
    endpoint: Url,
    api_key: String,
    request_model: String,
    contract: SemanticModelContract,
}

impl VolcengineEncoder {
    fn from_config(config: &SemanticWorkerConfig) -> Result<Option<Self>, SemanticWorkerError> {
        let Some(api_key) = config.api_key.as_deref() else {
            return Ok(None);
        };
        let Some(base_url) = config.base_url.clone() else {
            return Ok(None);
        };
        let Some(request_model) = config.request_model.clone() else {
            return Ok(None);
        };
        let endpoint = base_url
            .join("embeddings")
            .map_err(|_| SemanticWorkerError::Contract)?;
        let client = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(|_| SemanticWorkerError::ProviderUnavailable)?;
        Ok(Some(Self {
            client,
            endpoint,
            api_key: api_key.to_string(),
            request_model,
            contract: SemanticModelContract::volcengine_overview_v1(),
        }))
    }
}

#[derive(Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: Vec<&'a str>,
    dimensions: usize,
    encoding_format: &'static str,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    model: String,
    data: Vec<EmbeddingDatum>,
}

#[derive(Deserialize)]
struct EmbeddingDatum {
    index: usize,
    embedding: Vec<f32>,
}

impl SemanticEncoder for VolcengineEncoder {
    fn contract(&self) -> &SemanticModelContract {
        &self.contract
    }

    fn encode<'a>(&'a self, inputs: &'a [SemanticEncoderInput]) -> SemanticEncoderFuture<'a> {
        Box::pin(async move {
            if inputs
                .iter()
                .any(|input| input.identity().kind != SemanticUnitKind::Overview)
            {
                return Err(SemanticError::ExternalProviderBoundary);
            }
            let request = EmbeddingRequest {
                model: &self.request_model,
                input: inputs.iter().map(SemanticEncoderInput::text).collect(),
                dimensions: self.contract.dimensions,
                encoding_format: "float",
            };
            let response = self
                .client
                .post(self.endpoint.clone())
                .bearer_auth(&self.api_key)
                .json(&request)
                .send()
                .await
                .map_err(|_| SemanticError::ProviderTransport)?;
            let status = response.status();
            if status == StatusCode::TOO_MANY_REQUESTS {
                let retry_after_seconds = response
                    .headers()
                    .get(RETRY_AFTER)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.parse().ok());
                return Err(SemanticError::ProviderRateLimited {
                    retry_after_seconds,
                });
            }
            if status.is_server_error() {
                return Err(SemanticError::ProviderRetryable {
                    status: status.as_u16(),
                });
            }
            if !status.is_success() {
                return Err(SemanticError::ProviderRejected {
                    status: status.as_u16(),
                });
            }
            let body: EmbeddingResponse = response
                .json()
                .await
                .map_err(|_| SemanticError::ProviderResponse)?;
            decode_embedding_response(inputs, body, &self.contract)
        })
    }
}

fn decode_embedding_response(
    inputs: &[SemanticEncoderInput],
    mut body: EmbeddingResponse,
    contract: &SemanticModelContract,
) -> Result<Vec<EncodedSemanticUnit>, SemanticError> {
    if body.model != contract.model || body.data.len() != inputs.len() {
        return Err(SemanticError::ProviderResponse);
    }
    body.data.sort_unstable_by_key(|datum| datum.index);
    let mut encoded = Vec::with_capacity(inputs.len());
    for (index, (input, datum)) in inputs.iter().zip(body.data).enumerate() {
        if datum.index != index {
            return Err(SemanticError::ProviderResponse);
        }
        let unit = buzz_semantic::SemanticUnit {
            identity: input.identity().clone(),
            text: input.text().to_string(),
            semantic_text_digest: input.semantic_text_digest(),
            coverage: buzz_semantic::SemanticCoverage::TitleOnly,
        };
        encoded.push(EncodedSemanticUnit::new(
            &unit,
            body.model.clone(),
            datum.embedding,
            contract,
        )?);
    }
    Ok(encoded)
}

#[cfg(test)]
mod tests {
    use super::{
        decode_embedding_response, semantic_error_code, semantic_retry_after, EmbeddingDatum,
        EmbeddingResponse, SemanticWorkerError,
    };
    use buzz_semantic::{
        extract_overview, CanonicalSemanticSourceObservation, Digest32, ProjectDocumentSourceBasis,
        SemanticEligibility, SemanticEncoder, SemanticEncoderInput, SemanticError,
        SemanticFilterMetadata, SemanticLifecycleClass, SemanticModelContract, SemanticSourceBasis,
        SemanticSourceIdentity, SemanticSourceKind, SemanticUnitKind,
    };
    use std::time::Duration;
    use uuid::Uuid;

    use crate::config::SemanticWorkerConfig;

    #[test]
    fn provider_errors_are_content_free_and_bounded() {
        let error = SemanticWorkerError::Semantic(SemanticError::ProviderRateLimited {
            retry_after_seconds: Some(123),
        });
        assert_eq!(semantic_error_code(&error), "provider_rate_limited");
        assert_eq!(semantic_retry_after(&error, 1), 123);
    }

    #[test]
    fn provider_response_requires_exact_resolved_model_and_dimensions() {
        let contract = SemanticModelContract::volcengine_overview_v1();
        let observation = CanonicalSemanticSourceObservation::new(
            SemanticSourceIdentity {
                community_id: Uuid::from_u128(1),
                kind: SemanticSourceKind::ProjectDocument,
                source_id: Uuid::from_u128(2),
            },
            SemanticSourceBasis::ProjectDocument(ProjectDocumentSourceBasis {
                document_revision: 1,
                source_change_id: Digest32::from_bytes([3; 32]),
            }),
            SemanticEligibility::Eligible,
            SemanticFilterMetadata {
                lifecycle: SemanticLifecycleClass::Active,
                source_status: Some("active".to_string()),
            },
            "Provider response".to_string(),
            None,
        )
        .expect("observation");
        let unit = extract_overview(&observation).expect("overview");
        let input = SemanticEncoderInput::from_unit(&unit);
        let drift = EmbeddingResponse {
            model: "mutable-alias".to_string(),
            data: vec![EmbeddingDatum {
                index: 0,
                embedding: vec![0.0; contract.dimensions],
            }],
        };
        assert!(matches!(
            decode_embedding_response(&[input], drift, &contract),
            Err(SemanticError::ProviderResponse)
        ));
    }

    #[tokio::test]
    async fn external_provider_rejects_unapproved_content_chunks_before_transport() {
        let observation = CanonicalSemanticSourceObservation::new(
            SemanticSourceIdentity {
                community_id: Uuid::from_u128(11),
                kind: SemanticSourceKind::ProjectDocument,
                source_id: Uuid::from_u128(12),
            },
            SemanticSourceBasis::ProjectDocument(ProjectDocumentSourceBasis {
                document_revision: 1,
                source_change_id: Digest32::from_bytes([13; 32]),
            }),
            SemanticEligibility::Eligible,
            SemanticFilterMetadata {
                lifecycle: SemanticLifecycleClass::Active,
                source_status: Some("active".to_string()),
            },
            "Unapproved body".to_string(),
            None,
        )
        .expect("observation");
        let mut unit = extract_overview(&observation).expect("overview");
        unit.identity.kind = SemanticUnitKind::ContentChunk;
        unit.identity.key = "chunk:0".to_string();
        unit.identity.path = Some("body/0".to_string());
        let input = SemanticEncoderInput::from_unit(&unit);
        let encoder = super::VolcengineEncoder::from_config(&SemanticWorkerConfig {
            enabled: true,
            api_key: Some("test-only".to_string()),
            base_url: Some("https://example.invalid/api/".parse().expect("URL")),
            request_model: Some("test-alias".to_string()),
            request_timeout: Duration::from_secs(1),
            request_interval: Duration::from_secs(1),
            claim_seconds: 60,
            max_attempts: 2,
        })
        .expect("encoder config")
        .expect("configured encoder");

        assert!(matches!(
            encoder.encode(&[input]).await,
            Err(SemanticError::ExternalProviderBoundary)
        ));
    }
}
