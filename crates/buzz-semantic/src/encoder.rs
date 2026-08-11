use std::future::Future;
use std::pin::Pin;

use sha2::{Digest as _, Sha256};

use crate::{
    Digest32, EmbeddingVector, SemanticDistanceMetric, SemanticError, SemanticModelContract,
    SemanticNormalization, SemanticProviderBoundary, SemanticUnit, SemanticUnitIdentity,
};

/// Heap-allocated async encoder result used without an async-trait dependency.
pub type SemanticEncoderFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<EncodedSemanticUnit>, SemanticError>> + Send + 'a>>;

/// One complete, digest-bound encoder input.
pub struct SemanticEncoderInput {
    identity: SemanticUnitIdentity,
    semantic_text_digest: Digest32,
    text: String,
}

impl SemanticEncoderInput {
    /// Copy a pure extracted unit into an encoder request.
    pub fn from_unit(unit: &SemanticUnit) -> Self {
        Self {
            identity: unit.identity.clone(),
            semantic_text_digest: unit.semantic_text_digest,
            text: unit.text.clone(),
        }
    }

    /// Unit identity and source/extractor provenance.
    pub fn identity(&self) -> &SemanticUnitIdentity {
        &self.identity
    }

    /// Declared domain-separated semantic-text digest.
    pub const fn semantic_text_digest(&self) -> Digest32 {
        self.semantic_text_digest
    }

    /// Untrusted visible text that may be sent only to an approved provider.
    pub fn text(&self) -> &str {
        &self.text
    }

    fn validate(&self) -> Result<(), SemanticError> {
        let observed =
            Digest32::hash_domain(b"buzz.semantic.overview-text.v1", &[self.text.as_bytes()]);
        if observed != self.semantic_text_digest {
            return Err(SemanticError::EncoderInputDigestMismatch);
        }
        Ok(())
    }
}

/// Validated model output bound to one semantic unit.
pub struct EncodedSemanticUnit {
    identity: SemanticUnitIdentity,
    semantic_text_digest: Digest32,
    model_contract_digest: Digest32,
    response_model: String,
    embedding: EmbeddingVector,
}

impl EncodedSemanticUnit {
    /// Bind validated provider output to one extracted unit and exact model
    /// contract.
    pub fn new(
        unit: &SemanticUnit,
        response_model: String,
        values: Vec<f32>,
        contract: &SemanticModelContract,
    ) -> Result<Self, SemanticError> {
        if response_model != contract.model {
            return Err(SemanticError::InvalidModelContract {
                reason: "provider response model does not match generation contract",
            });
        }
        Ok(Self {
            identity: unit.identity.clone(),
            semantic_text_digest: unit.semantic_text_digest,
            model_contract_digest: contract.digest()?,
            response_model,
            embedding: EmbeddingVector::new(values, contract)?,
        })
    }

    /// Unit identity encoded by the provider.
    pub fn identity(&self) -> &SemanticUnitIdentity {
        &self.identity
    }

    /// Digest of the exact visible text encoded by the provider.
    pub const fn semantic_text_digest(&self) -> Digest32 {
        self.semantic_text_digest
    }

    /// Digest of the exact generation model contract.
    pub const fn model_contract_digest(&self) -> Digest32 {
        self.model_contract_digest
    }

    /// Exact model version reported by the encoder.
    pub fn response_model(&self) -> &str {
        &self.response_model
    }

    /// Validated finite, non-zero embedding values.
    pub fn embedding(&self) -> &EmbeddingVector {
        &self.embedding
    }
}

/// Async provider contract implemented by deterministic tests and approved
/// production encoders.
pub trait SemanticEncoder: Send + Sync {
    /// Exact model contract produced by this encoder.
    fn contract(&self) -> &SemanticModelContract;

    /// Encode a bounded batch. Implementations must preserve input order and
    /// return exactly one output per input or fail the whole batch.
    fn encode<'a>(&'a self, inputs: &'a [SemanticEncoderInput]) -> SemanticEncoderFuture<'a>;
}

/// Deterministic, offline fake encoder for unit and database integration tests.
pub struct DeterministicFakeEncoder {
    contract: SemanticModelContract,
}

impl DeterministicFakeEncoder {
    /// Build an offline encoder with a caller-selected finite vector width.
    pub fn new(dimensions: usize) -> Result<Self, SemanticError> {
        let contract = SemanticModelContract {
            provider: "deterministic_fake".to_string(),
            model: "deterministic-fake-v1".to_string(),
            dimensions,
            distance_metric: SemanticDistanceMetric::Cosine,
            normalization: SemanticNormalization::None,
            input_contract_version: "overview-v1".to_string(),
            provider_boundary: SemanticProviderBoundary::DeterministicFake,
        };
        contract.validate()?;
        Ok(Self { contract })
    }
}

impl SemanticEncoder for DeterministicFakeEncoder {
    fn contract(&self) -> &SemanticModelContract {
        &self.contract
    }

    fn encode<'a>(&'a self, inputs: &'a [SemanticEncoderInput]) -> SemanticEncoderFuture<'a> {
        Box::pin(async move {
            let model_contract_digest = self.contract.digest()?;
            let mut encoded = Vec::with_capacity(inputs.len());
            for input in inputs {
                input.validate()?;
                let mut values = Vec::with_capacity(self.contract.dimensions);
                let mut counter = 0_u64;
                while values.len() < self.contract.dimensions {
                    let mut hasher = Sha256::new();
                    hasher.update(b"buzz.semantic.deterministic-fake.v1");
                    hasher.update(model_contract_digest.as_bytes());
                    hasher.update(input.semantic_text_digest.as_bytes());
                    hasher.update(counter.to_be_bytes());
                    let block: [u8; 32] = hasher.finalize().into();
                    for bytes in block.chunks_exact(4) {
                        if values.len() == self.contract.dimensions {
                            break;
                        }
                        let raw = u32::from_be_bytes(
                            bytes.try_into().map_err(|_| SemanticError::Serialization)?,
                        );
                        let value = ((raw as f64 / u32::MAX as f64) * 2.0 - 1.0) as f32;
                        values.push(value);
                    }
                    counter = counter.checked_add(1).ok_or(SemanticError::Serialization)?;
                }
                let unit = SemanticUnit {
                    identity: input.identity.clone(),
                    text: input.text.clone(),
                    semantic_text_digest: input.semantic_text_digest,
                    coverage: crate::SemanticCoverage::TitleOnly,
                };
                let mut result = EncodedSemanticUnit::new(
                    &unit,
                    self.contract.model.clone(),
                    values,
                    &self.contract,
                )?;
                result.model_contract_digest = model_contract_digest;
                encoded.push(result);
            }
            Ok(encoded)
        })
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{DeterministicFakeEncoder, SemanticEncoder, SemanticEncoderInput};
    use crate::{
        extract_overview, CanonicalSemanticSourceObservation, Digest32, ProjectDocumentSourceBasis,
        SemanticEligibility, SemanticFilterMetadata, SemanticLifecycleClass, SemanticSourceBasis,
        SemanticSourceIdentity, SemanticSourceKind,
    };

    fn document_unit() -> crate::SemanticUnit {
        let observation = CanonicalSemanticSourceObservation::new(
            SemanticSourceIdentity {
                community_id: Uuid::from_u128(9),
                kind: SemanticSourceKind::ProjectDocument,
                source_id: Uuid::from_u128(10),
            },
            SemanticSourceBasis::ProjectDocument(ProjectDocumentSourceBasis {
                document_revision: 1,
                source_change_id: Digest32::from_bytes([11; 32]),
            }),
            SemanticEligibility::Eligible,
            SemanticFilterMetadata {
                lifecycle: SemanticLifecycleClass::Active,
                source_status: None,
            },
            "Semantic contract".to_string(),
            Some("Foundation overview".to_string()),
        )
        .expect("valid observation");
        extract_overview(&observation).expect("overview")
    }

    #[tokio::test]
    async fn fake_encoder_is_finite_dimensioned_and_deterministic() {
        let encoder = DeterministicFakeEncoder::new(32).expect("fake encoder");
        let input = SemanticEncoderInput::from_unit(&document_unit());
        let first = encoder
            .encode(std::slice::from_ref(&input))
            .await
            .expect("first encode");
        let second = encoder.encode(&[input]).await.expect("second encode");
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].embedding().as_slice().len(), 32);
        assert_eq!(
            first[0].embedding().as_slice(),
            second[0].embedding().as_slice()
        );
        assert_eq!(first[0].response_model(), "deterministic-fake-v1");
    }

    #[test]
    fn encoded_output_rejects_wrong_dimension_non_finite_zero_and_model_drift() {
        let encoder = DeterministicFakeEncoder::new(2).expect("fake encoder");
        let unit = document_unit();
        assert!(matches!(
            super::EncodedSemanticUnit::new(
                &unit,
                encoder.contract().model.clone(),
                vec![0.0, -0.0],
                encoder.contract(),
            ),
            Err(crate::SemanticError::ZeroNormEmbedding)
        ));
        assert!(matches!(
            super::EncodedSemanticUnit::new(
                &unit,
                encoder.contract().model.clone(),
                vec![0.5],
                encoder.contract(),
            ),
            Err(crate::SemanticError::EmbeddingDimensionMismatch { .. })
        ));
        assert!(matches!(
            super::EncodedSemanticUnit::new(
                &unit,
                encoder.contract().model.clone(),
                vec![f32::NAN, 0.5],
                encoder.contract(),
            ),
            Err(crate::SemanticError::NonFiniteEmbedding { .. })
        ));
        assert!(matches!(
            super::EncodedSemanticUnit::new(
                &unit,
                "mutable-alias".to_string(),
                vec![0.25, 0.5],
                encoder.contract(),
            ),
            Err(crate::SemanticError::InvalidModelContract { .. })
        ));
    }
}
