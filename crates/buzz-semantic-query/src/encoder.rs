use std::future::Future;
use std::pin::Pin;

use buzz_semantic::{
    DeterministicFakeEncoder, Digest32, EmbeddingVector, SemanticEncoder, SemanticModelContract,
};
use sha2::{Digest as _, Sha256};

use crate::{
    QueryCompatibilityFences, QueryContractResult, SemanticGraphQueryError,
    SemanticQueryEncoderInput,
};

/// Heap-allocated async query-encoder result without an async-trait dependency.
pub type SemanticQueryEncoderFuture<'a> =
    Pin<Box<dyn Future<Output = QueryContractResult<Vec<EncodedSemanticQuery>>> + Send + 'a>>;

/// One validated ephemeral query vector bound to its channel and all three
/// compatibility fences.
pub struct EncodedSemanticQuery {
    request_id: uuid::Uuid,
    channel_id: Digest32,
    source_generation_contract_digest: Digest32,
    embedding_space_fence: Digest32,
    query_contract_digest: Digest32,
    response_model: String,
    embedding: EmbeddingVector,
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
        let embedding = EmbeddingVector::new(values, source_contract)
            .map_err(|_| SemanticGraphQueryError::ProviderResponse)?;
        Ok(Self {
            request_id: input.request_id(),
            channel_id: input.channel_id(),
            source_generation_contract_digest: fences.source_generation_contract_digest,
            embedding_space_fence: fences.embedding_space_fence,
            query_contract_digest: fences.query_contract_digest,
            response_model,
            embedding,
        })
    }

    /// Owning request identity.
    pub const fn request_id(&self) -> uuid::Uuid {
        self.request_id
    }

    /// Query-vector branch identity.
    pub const fn channel_id(&self) -> Digest32 {
        self.channel_id
    }

    /// Complete active Foundation generation contract digest.
    pub const fn source_generation_contract_digest(&self) -> Digest32 {
        self.source_generation_contract_digest
    }

    /// Comparable model-space fence.
    pub const fn embedding_space_fence(&self) -> Digest32 {
        self.embedding_space_fence
    }

    /// Query template/serializer/input-limit digest.
    pub const fn query_contract_digest(&self) -> Digest32 {
        self.query_contract_digest
    }

    /// Exact model version returned by the Provider.
    pub fn response_model(&self) -> &str {
        &self.response_model
    }

    /// Validated finite, dimensioned, non-zero query vector.
    pub fn embedding(&self) -> &EmbeddingVector {
        &self.embedding
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

    use super::{DeterministicFakeQueryEncoder, SemanticQueryEncoder};
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
        assert_eq!(
            first[0].embedding().as_slice(),
            second[0].embedding().as_slice()
        );
    }
}
