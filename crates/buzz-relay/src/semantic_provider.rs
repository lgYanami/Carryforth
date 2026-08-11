//! Shared, closed adapters for the approved Volcengine embedding Provider.
//!
//! The transport is deliberately private. The only public encoding surfaces
//! accept either Foundation overview inputs or semantic-graph query inputs, so
//! callers cannot use this module as a general-purpose text egress client.

use std::collections::BTreeSet;

use buzz_semantic::{
    EmbeddingVector, EncodedSemanticUnit, SemanticEncoder, SemanticEncoderFuture,
    SemanticEncoderInput, SemanticError, SemanticModelContract, SemanticUnit, SemanticUnitKind,
};
use buzz_semantic_query::{
    EncodedSemanticQuery, QueryCompatibilityFences, QueryContractResult, SemanticGraphQueryError,
    SemanticQueryChannelKind, SemanticQueryEncoder, SemanticQueryEncoderFuture,
    SemanticQueryEncoderInput, MAX_QUERY_CHANNELS,
};
use reqwest::{header::RETRY_AFTER, StatusCode, Url};
use serde::{Deserialize, Serialize};

use crate::config::SemanticWorkerConfig;

/// Maximum successful Provider JSON body accepted by either current adapter.
///
/// The query adapter sends at most nine 2048-dimensional vectors in one batch;
/// the current worker sends one. One MiB leaves more than 48 wire bytes per
/// query-vector element plus bounded JSON/model metadata without permitting an
/// external Provider or proxy to make the Relay buffer an arbitrary body.
const SEMANTIC_PROVIDER_SUCCESS_RESPONSE_MAX_BYTES: usize = 1024 * 1024;
/// Error bodies are never inspected or logged and are read only within this
/// smaller bound, so status classification cannot hide an unbounded response.
const SEMANTIC_PROVIDER_ERROR_RESPONSE_MAX_BYTES: usize = 16 * 1024;

const _: () = assert!(
    SEMANTIC_PROVIDER_SUCCESS_RESPONSE_MAX_BYTES
        > MAX_QUERY_CHANNELS * 2_048 * 48 + MAX_QUERY_CHANNELS * 256 + 4_096
);

/// Failure to construct the configured semantic Provider client.
#[derive(Debug, thiserror::Error)]
pub enum SemanticProviderConfigError {
    /// The configured versioned base URL could not form the embeddings route.
    #[error("semantic provider embeddings endpoint is invalid")]
    InvalidEndpoint,
    /// The HTTP client could not be constructed.
    #[error("semantic provider HTTP client is unavailable")]
    ClientUnavailable,
}

/// Shared approved Volcengine transport with closed worker/query adapters.
///
/// This type intentionally has no arbitrary-text encoding method. Its raw
/// transport remains private to this module.
#[derive(Clone)]
pub struct VolcengineSemanticProvider {
    client: reqwest::Client,
    endpoint: Url,
    api_key: String,
    request_model: String,
    source_contract: SemanticModelContract,
}

impl VolcengineSemanticProvider {
    /// Construct the Provider when all optional credentials are configured.
    ///
    /// Configuration loading already enforces that an enabled worker or query
    /// deployment master supplies all three values. Returning `None` keeps
    /// capability-off development and tests free of Provider configuration.
    pub fn from_config(
        config: &SemanticWorkerConfig,
    ) -> Result<Option<Self>, SemanticProviderConfigError> {
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
            .map_err(|_| SemanticProviderConfigError::InvalidEndpoint)?;
        let client = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(|_| SemanticProviderConfigError::ClientUnavailable)?;
        Ok(Some(Self {
            client,
            endpoint,
            api_key: api_key.to_owned(),
            request_model,
            source_contract: SemanticModelContract::volcengine_overview_v1(),
        }))
    }

    /// Frozen source-generation contract emitted by this Provider.
    pub fn source_contract(&self) -> &SemanticModelContract {
        &self.source_contract
    }

    async fn encode_text_batch(
        &self,
        texts: &[&str],
        expected_contract: &SemanticModelContract,
    ) -> Result<RawEmbeddingBatch, SemanticError> {
        if texts.is_empty() || expected_contract != &self.source_contract {
            return Err(SemanticError::ExternalProviderBoundary);
        }
        let request = EmbeddingRequest {
            model: &self.request_model,
            input: texts.to_vec(),
            dimensions: expected_contract.dimensions,
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
        let retry_after_seconds = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok());
        let maximum_body_bytes = if status.is_success() {
            SEMANTIC_PROVIDER_SUCCESS_RESPONSE_MAX_BYTES
        } else {
            SEMANTIC_PROVIDER_ERROR_RESPONSE_MAX_BYTES
        };
        let response_body = read_bounded_provider_response(response, maximum_body_bytes).await?;
        if status == StatusCode::TOO_MANY_REQUESTS {
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
        let body: EmbeddingResponse =
            serde_json::from_slice(&response_body).map_err(|_| SemanticError::ProviderResponse)?;
        decode_embedding_response(texts.len(), body, expected_contract)
    }
}

async fn read_bounded_provider_response(
    mut response: reqwest::Response,
    maximum_body_bytes: usize,
) -> Result<Vec<u8>, SemanticError> {
    if response.content_length().is_some_and(|content_length| {
        usize::try_from(content_length).map_or(true, |length| length > maximum_body_bytes)
    }) {
        return Err(SemanticError::ProviderResponse);
    }

    let initial_capacity = response
        .content_length()
        .and_then(|content_length| usize::try_from(content_length).ok())
        .unwrap_or(0)
        .min(maximum_body_bytes);
    let mut body = Vec::with_capacity(initial_capacity);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| SemanticError::ProviderResponse)?
    {
        if chunk.len() > maximum_body_bytes.saturating_sub(body.len()) {
            return Err(SemanticError::ProviderResponse);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
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

struct RawEmbeddingBatch {
    response_model: String,
    embeddings: Vec<EmbeddingVector>,
}

fn decode_embedding_response(
    input_count: usize,
    mut body: EmbeddingResponse,
    contract: &SemanticModelContract,
) -> Result<RawEmbeddingBatch, SemanticError> {
    if input_count == 0 || body.model != contract.model || body.data.len() != input_count {
        return Err(SemanticError::ProviderResponse);
    }
    body.data.sort_unstable_by_key(|datum| datum.index);
    let mut embeddings = Vec::with_capacity(input_count);
    for (expected_index, datum) in body.data.into_iter().enumerate() {
        if datum.index != expected_index {
            return Err(SemanticError::ProviderResponse);
        }
        embeddings.push(EmbeddingVector::new(datum.embedding, contract)?);
    }
    Ok(RawEmbeddingBatch {
        response_model: body.model,
        embeddings,
    })
}

impl SemanticEncoder for VolcengineSemanticProvider {
    fn contract(&self) -> &SemanticModelContract {
        &self.source_contract
    }

    fn encode<'a>(&'a self, inputs: &'a [SemanticEncoderInput]) -> SemanticEncoderFuture<'a> {
        Box::pin(async move {
            if inputs.is_empty()
                || inputs
                    .iter()
                    .any(|input| input.identity().kind != SemanticUnitKind::Overview)
            {
                return Err(SemanticError::ExternalProviderBoundary);
            }
            let texts = inputs
                .iter()
                .map(SemanticEncoderInput::text)
                .collect::<Vec<_>>();
            let batch = self
                .encode_text_batch(&texts, &self.source_contract)
                .await?;
            let mut encoded = Vec::with_capacity(inputs.len());
            for (input, embedding) in inputs.iter().zip(batch.embeddings) {
                let unit = SemanticUnit {
                    identity: input.identity().clone(),
                    text: input.text().to_owned(),
                    semantic_text_digest: input.semantic_text_digest(),
                    coverage: buzz_semantic::SemanticCoverage::TitleOnly,
                };
                encoded.push(EncodedSemanticUnit::new(
                    &unit,
                    batch.response_model.clone(),
                    embedding.into_values(),
                    &self.source_contract,
                )?);
            }
            Ok(encoded)
        })
    }
}

impl SemanticQueryEncoder for VolcengineSemanticProvider {
    fn source_contract(&self) -> &SemanticModelContract {
        &self.source_contract
    }

    fn encode_queries<'a>(
        &'a self,
        inputs: &'a [SemanticQueryEncoderInput],
    ) -> SemanticQueryEncoderFuture<'a> {
        Box::pin(async move {
            validate_query_batch(inputs)?;
            QueryCompatibilityFences::for_source_contract(&self.source_contract)?;
            let texts = inputs
                .iter()
                .map(SemanticQueryEncoderInput::text)
                .collect::<Vec<_>>();
            let batch = self
                .encode_text_batch(&texts, &self.source_contract)
                .await
                .map_err(query_provider_error)?;
            bind_query_response(inputs, batch, &self.source_contract)
        })
    }
}

fn bind_query_response(
    inputs: &[SemanticQueryEncoderInput],
    batch: RawEmbeddingBatch,
    source_contract: &SemanticModelContract,
) -> QueryContractResult<Vec<EncodedSemanticQuery>> {
    if batch.embeddings.len() != inputs.len() {
        return Err(SemanticGraphQueryError::ProviderResponse);
    }
    let mut encoded = Vec::with_capacity(inputs.len());
    for (input, embedding) in inputs.iter().zip(batch.embeddings) {
        encoded.push(EncodedSemanticQuery::new(
            input,
            batch.response_model.clone(),
            embedding.into_values(),
            source_contract,
        )?);
    }
    Ok(encoded)
}

fn validate_query_batch(inputs: &[SemanticQueryEncoderInput]) -> QueryContractResult<()> {
    if inputs.is_empty() || inputs.len() > MAX_QUERY_CHANNELS {
        return Err(SemanticGraphQueryError::InvalidState(
            "query Provider batch must contain the bounded Q0/Qi set".to_owned(),
        ));
    }
    let request_id = inputs[0].request_id();
    let mut channel_ids = BTreeSet::new();
    let mut context_coordinates = BTreeSet::new();
    for (index, input) in inputs.iter().enumerate() {
        input.validate()?;
        if input.request_id() != request_id {
            return Err(SemanticGraphQueryError::InvalidState(
                "query Provider batch crosses request identities".to_owned(),
            ));
        }
        if !channel_ids.insert(input.channel_id()) {
            return Err(SemanticGraphQueryError::InvalidState(
                "query Provider batch repeats a channel identity".to_owned(),
            ));
        }
        match (index, input.channel_kind()) {
            (0, SemanticQueryChannelKind::Problem) => {}
            (0, SemanticQueryChannelKind::ConditionedContext { .. }) => {
                return Err(SemanticGraphQueryError::InvalidState(
                    "query Provider batch must begin with exactly one problem channel".to_owned(),
                ));
            }
            (_, SemanticQueryChannelKind::Problem) => {
                return Err(SemanticGraphQueryError::InvalidState(
                    "query Provider batch contains more than one problem channel".to_owned(),
                ));
            }
            (_, SemanticQueryChannelKind::ConditionedContext { context_coordinate }) => {
                if !context_coordinates.insert(context_coordinate) {
                    return Err(SemanticGraphQueryError::InvalidState(
                        "query Provider batch repeats a conditioned Coordinate".to_owned(),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn query_provider_error(error: SemanticError) -> SemanticGraphQueryError {
    match error {
        SemanticError::ProviderTransport => SemanticGraphQueryError::ProviderTransport,
        SemanticError::ProviderRateLimited {
            retry_after_seconds,
        } => SemanticGraphQueryError::ProviderRateLimited {
            retry_after_seconds,
        },
        SemanticError::ProviderRetryable { status } => {
            SemanticGraphQueryError::ProviderRetryable { status }
        }
        SemanticError::ProviderRejected { status } => {
            SemanticGraphQueryError::ProviderRejected { status }
        }
        SemanticError::ProviderResponse
        | SemanticError::EmbeddingDimensionMismatch { .. }
        | SemanticError::NonFiniteEmbedding { .. }
        | SemanticError::ZeroNormEmbedding => SemanticGraphQueryError::ProviderResponse,
        other => SemanticGraphQueryError::InvalidState(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::{convert::Infallible, time::Duration};

    use axum::{
        body::Body,
        http::{header::CONTENT_LENGTH, Response, StatusCode},
        routing::post,
        Json, Router,
    };
    use buzz_semantic::{
        extract_overview, CanonicalSemanticSourceObservation, Digest32, ProjectDocumentSourceBasis,
        SemanticEligibility, SemanticEncoder, SemanticEncoderInput, SemanticError,
        SemanticFilterMetadata, SemanticLifecycleClass, SemanticModelContract, SemanticSourceBasis,
        SemanticSourceIdentity, SemanticSourceKind, SemanticUnitKind,
    };
    use bytes::Bytes;
    use futures_util::stream;
    use reqwest::Url;
    use tokio::task::JoinHandle;
    use uuid::Uuid;

    use super::{
        bind_query_response, decode_embedding_response, query_provider_error, validate_query_batch,
        EmbeddingDatum, EmbeddingResponse, VolcengineSemanticProvider,
        SEMANTIC_PROVIDER_ERROR_RESPONSE_MAX_BYTES, SEMANTIC_PROVIDER_SUCCESS_RESPONSE_MAX_BYTES,
    };
    use crate::config::SemanticWorkerConfig;
    use buzz_project_context::ProjectContextCoordinate;
    use buzz_project_view::ProjectViewObjectType;
    use buzz_semantic_query::{
        build_query_encoder_inputs, ConditionedContextOverview, LifecycleFilter,
        QueryCompatibilityFences, SemanticGraphQuery, SemanticGraphQueryBudget,
        SemanticGraphQueryError, SemanticQueryEncoder, MAX_QUERY_CHANNELS,
    };

    fn provider_config() -> SemanticWorkerConfig {
        SemanticWorkerConfig {
            enabled: true,
            api_key: Some("test-only".to_owned()),
            base_url: Some(
                "https://example.invalid/api/"
                    .parse()
                    .expect("valid test URL"),
            ),
            request_model: Some("test-alias".to_owned()),
            request_timeout: Duration::from_secs(1),
            request_interval: Duration::from_secs(1),
            claim_seconds: 60,
            max_attempts: 2,
        }
    }

    async fn spawn_provider(app: Router) -> (Url, JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test Provider");
        let address = listener.local_addr().expect("test Provider address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve test Provider");
        });
        let base_url = format!("http://{address}/api/")
            .parse()
            .expect("test Provider URL");
        (base_url, task)
    }

    fn provider_for_base_url(base_url: Url) -> VolcengineSemanticProvider {
        let mut config = provider_config();
        config.base_url = Some(base_url);
        config.request_timeout = Duration::from_secs(10);
        VolcengineSemanticProvider::from_config(&config)
            .expect("provider config")
            .expect("configured provider")
    }

    fn document_input() -> SemanticEncoderInput {
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
                source_status: Some("active".to_owned()),
            },
            "Provider boundary".to_owned(),
            None,
        )
        .expect("valid observation");
        let unit = extract_overview(&observation).expect("overview");
        SemanticEncoderInput::from_unit(&unit)
    }

    #[test]
    fn response_requires_exact_model_count_indices_dimensions_and_nonzero_vectors() {
        let contract = SemanticModelContract::volcengine_overview_v1();
        let valid = || EmbeddingResponse {
            model: contract.model.clone(),
            data: vec![EmbeddingDatum {
                index: 0,
                embedding: vec![0.25; contract.dimensions],
            }],
        };
        assert!(decode_embedding_response(1, valid(), &contract).is_ok());

        let mut drift = valid();
        drift.model = "mutable-alias".to_owned();
        assert!(matches!(
            decode_embedding_response(1, drift, &contract),
            Err(SemanticError::ProviderResponse)
        ));

        let mut wrong_dimensions = valid();
        wrong_dimensions.data[0].embedding.pop();
        assert!(matches!(
            decode_embedding_response(1, wrong_dimensions, &contract),
            Err(SemanticError::EmbeddingDimensionMismatch { .. })
        ));

        let mut zero = valid();
        zero.data[0].embedding.fill(0.0);
        assert!(matches!(
            decode_embedding_response(1, zero, &contract),
            Err(SemanticError::ZeroNormEmbedding)
        ));

        let mut wrong_index = valid();
        wrong_index.data[0].index = 1;
        assert!(matches!(
            decode_embedding_response(1, wrong_index, &contract),
            Err(SemanticError::ProviderResponse)
        ));
    }

    #[tokio::test]
    async fn provider_accepts_a_bounded_normal_embedding_response() {
        let contract = SemanticModelContract::volcengine_overview_v1();
        let payload = serde_json::json!({
            "model": contract.model,
            "data": [{
                "index": 0,
                "embedding": vec![0.25_f32; contract.dimensions],
            }],
        });
        let app = Router::new().route(
            "/api/embeddings",
            post(move || {
                let payload = payload.clone();
                async move { Json(payload) }
            }),
        );
        let (base_url, server) = spawn_provider(app).await;
        let provider = provider_for_base_url(base_url);

        let encoded = provider
            .encode(&[document_input()])
            .await
            .expect("bounded Provider response");
        server.abort();

        assert_eq!(encoded.len(), 1);
        assert_eq!(encoded[0].embedding().as_slice().len(), 2_048);
    }

    #[tokio::test]
    async fn provider_rejects_oversized_content_length_before_buffering() {
        let app = Router::new().route(
            "/api/embeddings",
            post(|| async {
                let delayed_body = stream::once(async {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    Ok::<_, Infallible>(Bytes::from_static(b"x"))
                });
                Response::builder()
                    .status(StatusCode::OK)
                    .header(
                        CONTENT_LENGTH,
                        (SEMANTIC_PROVIDER_SUCCESS_RESPONSE_MAX_BYTES + 1).to_string(),
                    )
                    .body(Body::from_stream(delayed_body))
                    .expect("oversized response")
            }),
        );
        let (base_url, server) = spawn_provider(app).await;
        let provider = provider_for_base_url(base_url);

        let result =
            tokio::time::timeout(Duration::from_secs(2), provider.encode(&[document_input()]))
                .await
                .expect("Content-Length precheck must not poll the delayed body");
        server.abort();

        assert!(matches!(result, Err(SemanticError::ProviderResponse)));
    }

    #[tokio::test]
    async fn provider_rejects_oversized_chunked_response_while_streaming() {
        let app = Router::new().route(
            "/api/embeddings",
            post(|| async {
                let chunks = [
                    Ok::<_, Infallible>(Bytes::from(vec![
                        b'x';
                        SEMANTIC_PROVIDER_SUCCESS_RESPONSE_MAX_BYTES
                    ])),
                    Ok::<_, Infallible>(Bytes::from_static(b"x")),
                ];
                Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from_stream(stream::iter(chunks)))
                    .expect("chunked response")
            }),
        );
        let (base_url, server) = spawn_provider(app).await;
        let provider = provider_for_base_url(base_url);

        let result = provider.encode(&[document_input()]).await;
        server.abort();

        assert!(matches!(result, Err(SemanticError::ProviderResponse)));
    }

    #[tokio::test]
    async fn provider_applies_the_smaller_cap_to_non_success_bodies() {
        let app = Router::new().route(
            "/api/embeddings",
            post(|| async {
                let delayed_body = stream::once(async {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    Ok::<_, Infallible>(Bytes::from_static(b"sensitive error body"))
                });
                Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header(
                        CONTENT_LENGTH,
                        (SEMANTIC_PROVIDER_ERROR_RESPONSE_MAX_BYTES + 1).to_string(),
                    )
                    .body(Body::from_stream(delayed_body))
                    .expect("oversized error response")
            }),
        );
        let (base_url, server) = spawn_provider(app).await;
        let provider = provider_for_base_url(base_url);

        let result =
            tokio::time::timeout(Duration::from_secs(2), provider.encode(&[document_input()]))
                .await
                .expect("error Content-Length precheck must not poll the delayed body");
        server.abort();

        assert!(matches!(result, Err(SemanticError::ProviderResponse)));
    }

    #[tokio::test]
    async fn worker_adapter_rejects_content_chunks_before_transport() {
        let mut input = document_input();
        let mut unit = buzz_semantic::SemanticUnit {
            identity: input.identity().clone(),
            text: input.text().to_owned(),
            semantic_text_digest: input.semantic_text_digest(),
            coverage: buzz_semantic::SemanticCoverage::TitleOnly,
        };
        unit.identity.kind = SemanticUnitKind::ContentChunk;
        unit.identity.key = "chunk:0".to_owned();
        unit.identity.path = Some("body/0".to_owned());
        input = SemanticEncoderInput::from_unit(&unit);
        let provider = VolcengineSemanticProvider::from_config(&provider_config())
            .expect("provider config")
            .expect("configured provider");

        assert!(matches!(
            provider.encode(&[input]).await,
            Err(SemanticError::ExternalProviderBoundary)
        ));
    }

    fn query_inputs() -> Vec<buzz_semantic_query::SemanticQueryEncoderInput> {
        let coordinate = ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Work,
            object_id: Uuid::from_u128(0x123e_4567_e89b_42d3_a456_4266_0000_0003),
        };
        let query = SemanticGraphQuery {
            request_id: Uuid::from_u128(0x123e_4567_e89b_42d3_a456_4266_0000_0001),
            project_id: Uuid::from_u128(0x123e_4567_e89b_42d3_a456_4266_0000_0002),
            problem: "why does the release regress?".to_owned(),
            initial_coordinates: Vec::new(),
            context_coordinates: vec![coordinate.clone()],
            lifecycle_filter: LifecycleFilter::AllCurrent,
            budget: SemanticGraphQueryBudget::default(),
        };
        build_query_encoder_inputs(
            &query,
            &[ConditionedContextOverview {
                coordinate,
                current_overview_semantic_text: "Project View Work: stabilize release".to_owned(),
            }],
        )
        .expect("query inputs")
        .inputs
    }

    #[test]
    fn query_adapter_accepts_only_one_bounded_request_batch() {
        let inputs = query_inputs();
        assert!(validate_query_batch(&inputs).is_ok());
        assert!(validate_query_batch(&[]).is_err());

        let mut oversized = Vec::with_capacity(MAX_QUERY_CHANNELS + 1);
        for _ in 0..=MAX_QUERY_CHANNELS {
            oversized.push(inputs[0].clone());
        }
        assert!(validate_query_batch(&oversized).is_err());

        let duplicate_q0 = vec![inputs[0].clone(), inputs[0].clone()];
        assert!(validate_query_batch(&duplicate_q0).is_err());

        let duplicate_qi = vec![inputs[0].clone(), inputs[1].clone(), inputs[1].clone()];
        assert!(validate_query_batch(&duplicate_qi).is_err());

        let mut mixed = inputs;
        let other = SemanticGraphQuery {
            request_id: Uuid::from_u128(0x123e_4567_e89b_42d3_a456_4266_0000_0004),
            project_id: Uuid::from_u128(0x123e_4567_e89b_42d3_a456_4266_0000_0002),
            problem: "why?".to_owned(),
            initial_coordinates: Vec::new(),
            context_coordinates: Vec::new(),
            lifecycle_filter: LifecycleFilter::AllCurrent,
            budget: SemanticGraphQueryBudget::default(),
        };
        mixed.push(
            build_query_encoder_inputs(&other, &[])
                .expect("other query input")
                .inputs
                .remove(0),
        );
        assert!(validate_query_batch(&mixed).is_err());
    }

    #[tokio::test]
    async fn query_adapter_uses_one_batch_and_fails_before_transport_on_invalid_shape() {
        let provider = VolcengineSemanticProvider::from_config(&provider_config())
            .expect("provider config")
            .expect("configured provider");
        assert!(provider.encode_queries(&[]).await.is_err());
    }

    #[test]
    fn query_response_preserves_order_and_binds_all_three_fences() {
        let inputs = query_inputs();
        let contract = SemanticModelContract::volcengine_overview_v1();
        let raw = decode_embedding_response(
            inputs.len(),
            EmbeddingResponse {
                model: contract.model.clone(),
                data: vec![
                    EmbeddingDatum {
                        index: 1,
                        embedding: vec![0.25; contract.dimensions],
                    },
                    EmbeddingDatum {
                        index: 0,
                        embedding: vec![0.5; contract.dimensions],
                    },
                ],
            },
            &contract,
        )
        .expect("valid reordered Provider response");
        let encoded = bind_query_response(&inputs, raw, &contract).expect("bound queries");
        let fences = QueryCompatibilityFences::for_source_contract(&contract)
            .expect("approved query compatibility");

        assert_eq!(encoded.len(), inputs.len());
        for (output, input) in encoded.iter().zip(&inputs) {
            assert_eq!(output.request_id(), input.request_id());
            assert_eq!(output.channel_id(), input.channel_id());
            assert_eq!(
                output.source_generation_contract_digest(),
                fences.source_generation_contract_digest
            );
            assert_eq!(output.embedding_space_fence(), fences.embedding_space_fence);
            assert_eq!(output.query_contract_digest(), fences.query_contract_digest);
            assert_eq!(output.response_model(), contract.model);
            assert_eq!(output.embedding().as_slice().len(), 2_048);
        }
    }

    #[test]
    fn malformed_query_vectors_are_closed_provider_response_errors() {
        for error in [
            SemanticError::EmbeddingDimensionMismatch {
                expected: 2_048,
                observed: 2_047,
            },
            SemanticError::NonFiniteEmbedding { index: 7 },
            SemanticError::ZeroNormEmbedding,
            SemanticError::ProviderResponse,
        ] {
            assert_eq!(
                query_provider_error(error),
                SemanticGraphQueryError::ProviderResponse
            );
        }
    }
}
