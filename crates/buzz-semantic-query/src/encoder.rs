use std::future::Future;
use std::pin::Pin;

use buzz_semantic::{
    DeterministicFakeEncoder, Digest32, EmbeddingVector, SemanticEncoder, SemanticModelContract,
};
use sha2::{Digest as _, Sha256};

use crate::{
    QueryCompatibilityFences, QueryContractResult, SemanticGraphQueryError,
    SemanticModelSpaceFences, SemanticQueryEncoderInput, SemanticQueryInput,
    SemanticQueryInputBundle, SemanticQueryInputKind,
};

/// Heap-allocated async query-encoder result without an async-trait dependency.
pub type SemanticQueryEncoderFuture<'a> =
    Pin<Box<dyn Future<Output = QueryContractResult<Vec<EncodedSemanticQuery>>> + Send + 'a>>;

/// One Provider result bound to exact closed input bytes and one model space.
///
/// It deliberately carries no active generation UUID. Only a current,
/// authorized writer-DB ticket may add that identity.
#[derive(Clone, PartialEq)]
pub struct ProviderEncodedSemanticInput {
    request_id: uuid::Uuid,
    channel_id: Digest32,
    channel_kind: SemanticQueryInputKind,
    model_space: SemanticModelSpaceFences,
    encoding_contract_digest: Digest32,
    input_digest: Digest32,
    response_model: String,
    embedding: EmbeddingVector,
}

impl std::fmt::Debug for ProviderEncodedSemanticInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderEncodedSemanticInput")
            .field("channel_kind", &self.channel_kind)
            .field("response_model", &self.response_model)
            .field("dimensions", &self.embedding.as_slice().len())
            .finish_non_exhaustive()
    }
}

impl ProviderEncodedSemanticInput {
    /// Bind one raw Provider vector to the exact input and validated model space.
    pub fn new(
        input: &SemanticQueryInput,
        response_model: String,
        values: Vec<f32>,
        source_contract: &SemanticModelContract,
    ) -> QueryContractResult<Self> {
        input.validate().map_err(|error| {
            SemanticGraphQueryError::InvalidState(format!("common semantic input: {error}"))
        })?;
        if response_model != source_contract.model {
            return Err(SemanticGraphQueryError::ProviderResponse);
        }
        let model_space = SemanticModelSpaceFences::for_source_contract(source_contract)?;
        let embedding = EmbeddingVector::new(values, source_contract)
            .map_err(|_| SemanticGraphQueryError::ProviderResponse)?;
        Ok(Self {
            request_id: input.request_id(),
            channel_id: input.channel_id(),
            channel_kind: input.channel_kind().clone(),
            model_space,
            encoding_contract_digest: input.encoding_contract_digest(),
            input_digest: input.input_digest(),
            response_model,
            embedding,
        })
    }

    /// Owning request identity.
    pub const fn request_id(&self) -> uuid::Uuid {
        self.request_id
    }

    /// Stable request-local branch identity.
    pub const fn channel_id(&self) -> Digest32 {
        self.channel_id
    }

    /// Closed Coordinate, Q0, or Qi identity.
    pub const fn channel_kind(&self) -> &SemanticQueryInputKind {
        &self.channel_kind
    }

    /// Validated source model-space fences, excluding generation UUID.
    pub const fn model_space(&self) -> &SemanticModelSpaceFences {
        &self.model_space
    }

    /// Closed serializer/template digest.
    pub const fn encoding_contract_digest(&self) -> Digest32 {
        self.encoding_contract_digest
    }

    /// Digest of the exact Provider input bytes.
    pub const fn input_digest(&self) -> Digest32 {
        self.input_digest
    }

    /// Exact Provider response model.
    pub fn response_model(&self) -> &str {
        &self.response_model
    }

    /// Validated finite, dimensioned, non-zero query vector.
    pub fn embedding(&self) -> &EmbeddingVector {
        &self.embedding
    }

    /// Consume the binding and return its validated embedding.
    pub fn into_embedding(self) -> EmbeddingVector {
        self.embedding
    }
}

/// Ordered Provider outputs corresponding one-for-one with one input bundle.
#[derive(Clone, PartialEq)]
pub struct ProviderEncodedSemanticInputBundle {
    inputs: Vec<ProviderEncodedSemanticInput>,
}

impl std::fmt::Debug for ProviderEncodedSemanticInputBundle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderEncodedSemanticInputBundle")
            .field("input_count", &self.inputs.len())
            .finish_non_exhaustive()
    }
}

impl ProviderEncodedSemanticInputBundle {
    /// Validate and bind an entire Provider response in input order.
    pub fn new(
        input_bundle: &SemanticQueryInputBundle,
        response_model: String,
        values: Vec<Vec<f32>>,
        source_contract: &SemanticModelContract,
    ) -> QueryContractResult<Self> {
        input_bundle.validate().map_err(|error| {
            SemanticGraphQueryError::InvalidState(format!("common semantic input bundle: {error}"))
        })?;
        if values.len() != input_bundle.len() {
            return Err(SemanticGraphQueryError::ProviderResponse);
        }
        let inputs = input_bundle
            .inputs()
            .iter()
            .zip(values)
            .map(|(input, values)| {
                ProviderEncodedSemanticInput::new(
                    input,
                    response_model.clone(),
                    values,
                    source_contract,
                )
            })
            .collect::<QueryContractResult<Vec<_>>>()?;
        Ok(Self { inputs })
    }

    /// Ordered Provider-bound inputs.
    pub fn inputs(&self) -> &[ProviderEncodedSemanticInput] {
        &self.inputs
    }

    /// Consume the bundle without changing input order.
    pub fn into_inputs(self) -> Vec<ProviderEncodedSemanticInput> {
        self.inputs
    }
}

/// Compatibility wrapper for one graph Q0/Qi Provider result.
pub struct EncodedSemanticQuery {
    inner: ProviderEncodedSemanticInput,
}

impl EncodedSemanticQuery {
    /// Validate and bind one Provider vector to its exact input and active
    /// Foundation model contract.
    pub fn new(
        input: &SemanticQueryEncoderInput,
        response_model: String,
        values: Vec<f32>,
        source_contract: &SemanticModelContract,
    ) -> QueryContractResult<Self> {
        input.validate()?;
        if response_model != source_contract.model {
            return Err(SemanticGraphQueryError::ProviderResponse);
        }
        let fences = QueryCompatibilityFences::for_source_contract(source_contract)?;
        if input.query_contract_digest() != fences.query_contract_digest {
            return Err(SemanticGraphQueryError::InvalidState(
                "query encoder input contract fence mismatch".to_owned(),
            ));
        }
        let inner = ProviderEncodedSemanticInput::new(
            input.semantic_input(),
            response_model,
            values,
            source_contract,
        )?;
        if inner.model_space.source_generation_contract_digest
            != fences.source_generation_contract_digest
            || inner.model_space.embedding_space_fence != fences.embedding_space_fence
            || inner.encoding_contract_digest != fences.query_contract_digest
        {
            return Err(SemanticGraphQueryError::InvalidState(
                "graph query Provider binding mismatch".to_owned(),
            ));
        }
        Ok(Self { inner })
    }

    /// Owning request identity.
    pub const fn request_id(&self) -> uuid::Uuid {
        self.inner.request_id()
    }

    /// Query-vector branch identity.
    pub const fn channel_id(&self) -> Digest32 {
        self.inner.channel_id()
    }

    /// Complete active Foundation generation contract digest.
    pub const fn source_generation_contract_digest(&self) -> Digest32 {
        self.inner.model_space().source_generation_contract_digest
    }

    /// Comparable model-space fence.
    pub const fn embedding_space_fence(&self) -> Digest32 {
        self.inner.model_space().embedding_space_fence
    }

    /// Query template/serializer/input-limit digest.
    pub const fn query_contract_digest(&self) -> Digest32 {
        self.inner.encoding_contract_digest()
    }

    /// Digest of the exact Provider input bytes.
    pub const fn query_input_digest(&self) -> Digest32 {
        self.inner.input_digest()
    }

    /// Exact model version returned by the Provider.
    pub fn response_model(&self) -> &str {
        self.inner.response_model()
    }

    /// Validated finite, dimensioned, non-zero query vector.
    pub fn embedding(&self) -> &EmbeddingVector {
        self.inner.embedding()
    }

    /// Common Provider-bound representation used by the DB ticket binder.
    pub const fn provider_encoded(&self) -> &ProviderEncodedSemanticInput {
        &self.inner
    }

    /// Consume this compatibility wrapper.
    pub fn into_provider_encoded(self) -> ProviderEncodedSemanticInput {
        self.inner
    }
}

/// Async Provider boundary for one bounded Q0/Qi batch.
pub trait SemanticQueryEncoder: Send + Sync {
    /// Exact Foundation model space produced by this encoder.
    fn source_contract(&self) -> &SemanticModelContract;

    /// Encode one bounded batch in input order or fail the complete batch.
    fn encode_queries<'a>(
        &'a self,
        inputs: &'a [SemanticQueryEncoderInput],
    ) -> SemanticQueryEncoderFuture<'a>;
}

/// Offline deterministic fake query encoder for unit and DB integration tests.
pub struct DeterministicFakeQueryEncoder {
    source_contract: SemanticModelContract,
}

impl DeterministicFakeQueryEncoder {
    /// Build a fake encoder in the same deterministic Foundation test space.
    pub fn new(dimensions: usize) -> QueryContractResult<Self> {
        let foundation = DeterministicFakeEncoder::new(dimensions)
            .map_err(|error| SemanticGraphQueryError::InvalidState(error.to_string()))?;
        Ok(Self {
            source_contract: foundation.contract().clone(),
        })
    }
}

impl SemanticQueryEncoder for DeterministicFakeQueryEncoder {
    fn source_contract(&self) -> &SemanticModelContract {
        &self.source_contract
    }

    fn encode_queries<'a>(
        &'a self,
        inputs: &'a [SemanticQueryEncoderInput],
    ) -> SemanticQueryEncoderFuture<'a> {
        Box::pin(async move {
            let fences = QueryCompatibilityFences::for_source_contract(&self.source_contract)?;
            let mut outputs = Vec::with_capacity(inputs.len());
            for input in inputs {
                input.validate()?;
                let mut values = Vec::with_capacity(self.source_contract.dimensions);
                let mut counter = 0_u64;
                while values.len() < self.source_contract.dimensions {
                    let mut hasher = Sha256::new();
                    hasher.update(b"buzz.semantic-graph-query-deterministic-fake");
                    hasher.update(fences.embedding_space_fence.as_bytes());
                    hasher.update(input.channel_id().as_bytes());
                    hasher.update(input.text_digest().as_bytes());
                    hasher.update(counter.to_be_bytes());
                    let block: [u8; 32] = hasher.finalize().into();
                    for bytes in block.chunks_exact(4) {
                        if values.len() == self.source_contract.dimensions {
                            break;
                        }
                        let raw = u32::from_be_bytes(
                            bytes
                                .try_into()
                                .map_err(|_| SemanticGraphQueryError::Serialization)?,
                        );
                        values.push(((f64::from(raw) / f64::from(u32::MAX)) * 2.0 - 1.0) as f32);
                    }
                    counter = counter
                        .checked_add(1)
                        .ok_or(SemanticGraphQueryError::Serialization)?;
                }
                outputs.push(EncodedSemanticQuery::new(
                    input,
                    self.source_contract.model.clone(),
                    values,
                    &self.source_contract,
                )?);
            }
            Ok(outputs)
        })
    }
}

#[cfg(test)]
mod tests {
    use buzz_project_context::ProjectContextCoordinate;
    use buzz_project_view::ProjectViewObjectType;
    use uuid::Uuid;

    use super::{
        DeterministicFakeQueryEncoder, ProviderEncodedSemanticInputBundle, SemanticQueryEncoder,
    };
    use crate::{
        build_query_encoder_inputs, ConditionedContextOverview, LifecycleFilter,
        SemanticGraphQuery, SemanticGraphQueryBudget,
    };

    fn uuid(value: u128) -> Uuid {
        Uuid::from_u128(0x123e_4567_e89b_42d3_a456_4266_0000_0000 | value)
    }

    #[tokio::test]
    async fn fake_query_encoder_is_deterministic_and_preserves_channel_order() {
        let coordinate = ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Work,
            object_id: uuid(3),
        };
        let query = SemanticGraphQuery {
            request_id: uuid(1),
            project_id: uuid(2),
            problem: "why?".to_owned(),
            initial_coordinates: Vec::new(),
            context_coordinates: vec![coordinate.clone()],
            lifecycle_filter: LifecycleFilter::AllCurrent,
            budget: SemanticGraphQueryBudget::default(),
        };
        let inputs = build_query_encoder_inputs(
            &query,
            &[ConditionedContextOverview {
                coordinate,
                current_overview_semantic_text: "type: Work\ntitle: Retry".to_owned(),
            }],
        )
        .expect("inputs")
        .inputs;
        let encoder = DeterministicFakeQueryEncoder::new(16).expect("encoder");
        let first = encoder.encode_queries(&inputs).await.expect("first");
        let second = encoder.encode_queries(&inputs).await.expect("second");
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].channel_id(), inputs[0].channel_id());
        assert_eq!(first[0].query_input_digest(), inputs[0].text_digest());
        assert_eq!(
            first[0].embedding().as_slice(),
            second[0].embedding().as_slice()
        );
    }

    #[test]
    fn common_provider_bundle_preserves_order_input_digest_and_redaction() {
        let coordinate = ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Work,
            object_id: uuid(4),
        };
        let query = SemanticGraphQuery {
            request_id: uuid(1),
            project_id: uuid(2),
            problem: "CONFIDENTIAL-PROBLEM".to_owned(),
            initial_coordinates: Vec::new(),
            context_coordinates: vec![coordinate.clone()],
            lifecycle_filter: LifecycleFilter::AllCurrent,
            budget: SemanticGraphQueryBudget::default(),
        };
        let outcome = build_query_encoder_inputs(
            &query,
            &[ConditionedContextOverview {
                coordinate,
                current_overview_semantic_text: "CONFIDENTIAL-OVERVIEW".to_owned(),
            }],
        )
        .expect("inputs");
        let bundle = outcome.semantic_input_bundle().expect("common bundle");
        let encoder = DeterministicFakeQueryEncoder::new(3).expect("encoder");
        let encoded = ProviderEncodedSemanticInputBundle::new(
            &bundle,
            encoder.source_contract().model.clone(),
            vec![vec![1.0, 0.0, 0.0], vec![0.0, 1.0, 0.0]],
            encoder.source_contract(),
        )
        .expect("Provider result bundle");

        assert_eq!(encoded.inputs().len(), 2);
        assert_eq!(
            encoded.inputs()[1].input_digest(),
            bundle.inputs()[1].input_digest()
        );
        let debug = format!("{encoded:?}");
        assert!(!debug.contains("CONFIDENTIAL"));
        assert!(!debug.contains(&query.request_id.to_string()));

        assert!(ProviderEncodedSemanticInputBundle::new(
            &bundle,
            "wrong-model".to_owned(),
            vec![vec![1.0, 0.0, 0.0], vec![0.0, 1.0, 0.0]],
            encoder.source_contract(),
        )
        .is_err());
    }
}
