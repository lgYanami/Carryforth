use buzz_semantic::{
    Digest32, SemanticDistanceMetric, SemanticModelContract, SemanticNormalization,
    SemanticProviderBoundary,
};
use sha2::{Digest as _, Sha256};

use crate::{query_contract_digest, QueryContractResult, SemanticGraphQueryError};

/// Three independent fences required before encoding or exact recall.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct QueryCompatibilityFences {
    /// Digest of the complete active Foundation generation contract.
    pub source_generation_contract_digest: Digest32,
    /// Digest of only the vector-space comparability fields.
    pub embedding_space_fence: Digest32,
    /// Digest of query templates, serializer, and Provider input limits.
    pub query_contract_digest: Digest32,
}

impl QueryCompatibilityFences {
    /// Build fences only for a closed, code-approved source/query contract
    /// combination. Production permits the frozen Volcengine overview
    /// generation; deterministic fake contracts remain available to tests.
    pub fn for_source_contract(contract: &SemanticModelContract) -> QueryContractResult<Self> {
        contract
            .validate()
            .map_err(|error| SemanticGraphQueryError::InvalidState(error.to_string()))?;
        if !is_approved_source_contract(contract) {
            return Err(SemanticGraphQueryError::InvalidState(
                "source generation is not in the closed query compatibility allowlist".to_owned(),
            ));
        }
        let source_generation_contract_digest = contract
            .digest()
            .map_err(|error| SemanticGraphQueryError::InvalidState(error.to_string()))?;
        Ok(Self {
            source_generation_contract_digest,
            embedding_space_fence: embedding_space_fence(contract)?,
            query_contract_digest: query_contract_digest(),
        })
    }

    /// Fail closed unless all observed ticket/DB fences exactly match the
    /// approved active source model and current query contract.
    pub fn validate_observed(
        contract: &SemanticModelContract,
        observed_source_generation_contract_digest: Digest32,
        observed_embedding_space_fence: Digest32,
        observed_query_contract_digest: Digest32,
    ) -> QueryContractResult<Self> {
        let expected = Self::for_source_contract(contract)?;
        if expected.source_generation_contract_digest != observed_source_generation_contract_digest
            || expected.embedding_space_fence != observed_embedding_space_fence
            || expected.query_contract_digest != observed_query_contract_digest
        {
            return Err(SemanticGraphQueryError::InvalidState(
                "semantic query compatibility fence mismatch".to_owned(),
            ));
        }
        Ok(expected)
    }
}

/// Derive the vector-space fence without claiming source and query input
/// contracts are identical.
pub fn embedding_space_fence(contract: &SemanticModelContract) -> QueryContractResult<Digest32> {
    contract
        .validate()
        .map_err(|error| SemanticGraphQueryError::InvalidState(error.to_string()))?;
    let canonical = postcard::to_stdvec(&(
        contract.provider.as_str(),
        contract.model.as_str(),
        contract.dimensions,
        contract.distance_metric,
        contract.normalization,
        &contract.provider_boundary,
    ))
    .map_err(|_| SemanticGraphQueryError::Serialization)?;
    Ok(hash_domain(
        b"buzz.semantic-embedding-space",
        &[canonical.as_slice()],
    ))
}

fn is_approved_source_contract(contract: &SemanticModelContract) -> bool {
    if contract == &SemanticModelContract::volcengine_overview_v1() {
        return true;
    }
    contract.provider == "deterministic_fake"
        && contract.model == "deterministic-fake-v1"
        && contract.distance_metric == SemanticDistanceMetric::Cosine
        && contract.normalization == SemanticNormalization::None
        && contract.input_contract_version == "overview-v1"
        && matches!(
            contract.provider_boundary,
            SemanticProviderBoundary::DeterministicFake
        )
}

fn hash_domain(domain: &[u8], parts: &[&[u8]]) -> Digest32 {
    let mut hasher = Sha256::new();
    hasher.update((domain.len() as u64).to_be_bytes());
    hasher.update(domain);
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    Digest32::from_bytes(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use buzz_semantic::SemanticModelContract;

    use super::{embedding_space_fence, QueryCompatibilityFences};
    use crate::query_contract_digest;

    #[test]
    fn source_query_and_space_fences_remain_distinct_and_exact() {
        let contract = SemanticModelContract::volcengine_overview_v1();
        let fences =
            QueryCompatibilityFences::for_source_contract(&contract).expect("approved contract");
        assert_eq!(
            fences.embedding_space_fence,
            embedding_space_fence(&contract).expect("space fence")
        );
        assert_eq!(fences.query_contract_digest, query_contract_digest());
        assert_ne!(
            fences.source_generation_contract_digest,
            fences.query_contract_digest
        );
        assert!(QueryCompatibilityFences::validate_observed(
            &contract,
            fences.source_generation_contract_digest,
            fences.embedding_space_fence,
            fences.query_contract_digest,
        )
        .is_ok());
    }
}
