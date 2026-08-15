//! Generation-bound semantic query vectors shared by closed exact scorers.

use std::collections::BTreeSet;

use buzz_core::CommunityId;
use buzz_semantic::{Digest32, EmbeddingVector, SemanticSourceIdentity};
use buzz_semantic_query::{
    coordinate_search_query_contract_digest, LifecycleFilter, ProviderEncodedSemanticInput,
    ProviderEncodedSemanticInputBundle, Score, SemanticModelSpaceFences, SemanticQueryInputKind,
    MAX_ONE_HOP_EDGE_COORDINATES, MAX_ONE_HOP_RELATION_BINDINGS, MAX_QUERY_CHANNELS,
};
use sqlx::Row;
use uuid::Uuid;

use super::{
    vector_norm_squared, SemanticExactSourceScore, SemanticGraphQueryTicket, SemanticGraphReadTx,
};
use crate::{DbError, Result};

/// Exact tenant-scoped active semantic generation identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SemanticGenerationKey {
    /// Host-derived Community identity.
    pub community_id: CommunityId,
    /// Active generation UUID within that Community.
    pub generation_id: Uuid,
}

/// Ticket-owned generation and model-space fences required by exact scoring.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SemanticGenerationFences {
    /// Composite tenant/generation key.
    pub generation_key: SemanticGenerationKey,
    /// Digest of the active Foundation generation model contract.
    pub source_generation_contract_digest: Digest32,
    /// Comparable embedding-space fence.
    pub embedding_space_fence: Digest32,
    /// Exact model identity.
    pub model: String,
    /// Exact vector dimensions.
    pub dimensions: usize,
}

impl SemanticGenerationFences {
    pub(crate) fn from_ticket(ticket: &SemanticGraphQueryTicket) -> Result<Self> {
        let model_space =
            SemanticModelSpaceFences::for_source_contract(&ticket.generation.model_contract)
                .map_err(|error| {
                    DbError::InvalidData(format!("invalid semantic ticket model space: {error}"))
                })?;
        if model_space.source_generation_contract_digest != ticket.generation.model_contract_digest
            || model_space.source_generation_contract_digest
                != ticket.query_fences.source_generation_contract_digest
            || model_space.embedding_space_fence != ticket.query_fences.embedding_space_fence
        {
            return Err(DbError::InvalidData(
                "semantic ticket generation/model-space fence mismatch".to_owned(),
            ));
        }
        Ok(Self {
            generation_key: SemanticGenerationKey {
                community_id: ticket.community_id,
                generation_id: ticket.generation.generation_id,
            },
            source_generation_contract_digest: model_space.source_generation_contract_digest,
            embedding_space_fence: model_space.embedding_space_fence,
            model: model_space.model,
            dimensions: model_space.dimensions,
        })
    }
}

/// One Provider vector bound by the writer DB to an exact active generation.
#[derive(Clone, PartialEq)]
pub struct GenerationBoundQueryVector {
    request_id: Uuid,
    channel_id: Digest32,
    channel_kind: SemanticQueryInputKind,
    generation_fences: SemanticGenerationFences,
    encoding_contract_digest: Digest32,
    input_digest: Digest32,
    response_model: String,
    embedding: EmbeddingVector,
}

impl std::fmt::Debug for GenerationBoundQueryVector {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GenerationBoundQueryVector")
            .field("channel_kind", &self.channel_kind)
            .field("dimensions", &self.embedding.as_slice().len())
            .finish_non_exhaustive()
    }
}

impl GenerationBoundQueryVector {
    /// Add the exact authorized generation identity to one Provider result.
    pub fn bind(
        ticket: &SemanticGraphQueryTicket,
        encoded: ProviderEncodedSemanticInput,
    ) -> Result<Self> {
        let generation_fences = ticket.generation_fences()?;
        let expected_model_space = SemanticModelSpaceFences {
            source_generation_contract_digest: generation_fences.source_generation_contract_digest,
            embedding_space_fence: generation_fences.embedding_space_fence,
            model: generation_fences.model.clone(),
            dimensions: generation_fences.dimensions,
        };
        if encoded.model_space() != &expected_model_space
            || encoded.response_model() != generation_fences.model
            || encoded.embedding().as_slice().len() != generation_fences.dimensions
            || encoded
                .embedding()
                .as_slice()
                .iter()
                .any(|value| !value.is_finite())
            || vector_norm_squared(encoded.embedding().as_slice()) <= 0.0
        {
            return Err(DbError::InvalidData(
                "semantic Provider result does not match the exact active generation".to_owned(),
            ));
        }
        let request_id = encoded.request_id();
        let channel_id = encoded.channel_id();
        let channel_kind = encoded.channel_kind().clone();
        let encoding_contract_digest = encoded.encoding_contract_digest();
        let input_digest = encoded.input_digest();
        let response_model = encoded.response_model().to_owned();
        let embedding = encoded.into_embedding();
        Ok(Self {
            request_id,
            channel_id,
            channel_kind,
            generation_fences,
            encoding_contract_digest,
            input_digest,
            response_model,
            embedding,
        })
    }

    /// Owning request identity.
    pub const fn request_id(&self) -> Uuid {
        self.request_id
    }

    /// Stable request-local branch identity.
    pub const fn channel_id(&self) -> Digest32 {
        self.channel_id
    }

    /// Closed input identity.
    pub const fn channel_kind(&self) -> &SemanticQueryInputKind {
        &self.channel_kind
    }

    /// Exact generation and model-space binding.
    pub const fn generation_fences(&self) -> &SemanticGenerationFences {
        &self.generation_fences
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

    pub(crate) fn embedding(&self) -> &EmbeddingVector {
        &self.embedding
    }
}

/// Ordered generation-bound outputs from one Provider batch.
#[derive(Clone, PartialEq)]
pub struct GenerationBoundQueryVectorBundle {
    vectors: Vec<GenerationBoundQueryVector>,
}

impl GenerationBoundQueryVectorBundle {
    /// Bind one complete ordered Provider result bundle to one DB ticket.
    pub fn bind(
        ticket: &SemanticGraphQueryTicket,
        encoded: ProviderEncodedSemanticInputBundle,
    ) -> Result<Self> {
        let vectors = encoded
            .into_inputs()
            .into_iter()
            .map(|input| GenerationBoundQueryVector::bind(ticket, input))
            .collect::<Result<Vec<_>>>()?;
        validate_generation_bound_vectors(ticket, vectors.iter(), vectors.len())?;
        Ok(Self { vectors })
    }

    /// Ordered generation-bound vectors.
    pub fn vectors(&self) -> &[GenerationBoundQueryVector] {
        &self.vectors
    }

    /// Consume the bundle without changing channel order.
    pub fn into_vectors(self) -> Vec<GenerationBoundQueryVector> {
        self.vectors
    }
}

/// Compatibility wrapper for graph Q0/Qi exact scoring.
#[derive(Clone, PartialEq)]
pub struct SemanticExactQueryVector {
    inner: GenerationBoundQueryVector,
}

/// DB-internal closed explicit source scopes accepted by the shared scorer.
///
/// The Relay cannot construct this type. Structural operation methods resolve
/// the source identities first, then choose the matching fixed bound.
pub(super) enum SemanticExactExplicitSourceScope<'a> {
    /// Current relation Documents on one Coordinate's incident Edges.
    OneHopIncidentDocuments(&'a [SemanticSourceIdentity]),
    /// Complete current member Coordinates of one exact Edge.
    OneHopEdgeCoordinates(&'a [SemanticSourceIdentity]),
}

/// DB-owned candidate and tie policy selected before exact scoring begins.
///
/// This is deliberately closed and cannot be constructed from Relay input.
/// Public operations retain their own ranking, floor, budget, and projection
/// policies above this mathematical/currentness kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SemanticExactScoreScope {
    /// Existing graph recall and explicit-source scoring semantics.
    GraphSources,
    /// Every current eligible Coordinate on at least one active Edge.
    GlobalGraphCoordinates,
}

/// Lightweight exact score returned by the global Coordinate scope.
///
/// Coordinate search intentionally does not hydrate graph-role or current-head
/// previews into its public result.
pub(crate) struct SemanticGlobalCoordinateScore {
    pub(crate) channel_id: Digest32,
    pub(crate) source: SemanticSourceIdentity,
    pub(crate) score: Score,
    pub(crate) channel_rank: u32,
}

impl SemanticExactScoreScope {
    pub(super) const fn coordinate_only(self) -> bool {
        matches!(self, Self::GlobalGraphCoordinates)
    }
}

impl SemanticExactExplicitSourceScope<'_> {
    fn sources(&self) -> &[SemanticSourceIdentity] {
        match self {
            Self::OneHopIncidentDocuments(sources) | Self::OneHopEdgeCoordinates(sources) => {
                sources
            }
        }
    }

    fn maximum(&self) -> usize {
        match self {
            Self::OneHopIncidentDocuments(_) => MAX_ONE_HOP_RELATION_BINDINGS as usize,
            Self::OneHopEdgeCoordinates(_) => MAX_ONE_HOP_EDGE_COORDINATES as usize,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::OneHopIncidentDocuments(_) => "one-hop incident relation Document",
            Self::OneHopEdgeCoordinates(_) => "one-hop Edge Coordinate",
        }
    }

    fn validate(&self) -> Result<()> {
        if self.sources().len() > self.maximum() {
            return Err(DbError::InvalidData(format!(
                "semantic {} source count exceeds the closed bound",
                self.label()
            )));
        }
        Ok(())
    }
}

impl SemanticGraphReadTx {
    /// Score one DB-resolved explicit source scope with direct all-current Q0.
    ///
    /// This facade intentionally owns no Edge grouping, tie policy, coverage,
    /// hydration, floor, coherence, or public result projection.
    pub(super) async fn score_explicit_source_scope_exact(
        &mut self,
        query_vector: &SemanticExactQueryVector,
        scope: SemanticExactExplicitSourceScope<'_>,
    ) -> Result<Vec<SemanticExactSourceScore>> {
        scope.validate()?;
        self.query_exact_source_scores(
            LifecycleFilter::AllCurrent,
            &[],
            std::slice::from_ref(query_vector),
            Some(scope.sources()),
            None,
        )
        .await
    }

    /// Score the complete current active-edge Coordinate scope with one
    /// Coordinate-search vector and canonical Coordinate tie ordering.
    ///
    /// The returned rows include the caller's K+1 observation. This facade
    /// owns no public result projection and applies no relevance floor.
    pub(crate) async fn score_global_graph_coordinates_exact(
        &mut self,
        query_vector: &GenerationBoundQueryVector,
        observed_limit: u32,
    ) -> Result<Vec<SemanticGlobalCoordinateScore>> {
        if observed_limit == 0
            || !matches!(
                query_vector.channel_kind(),
                SemanticQueryInputKind::CoordinateSearch
            )
            || query_vector.encoding_contract_digest() != coordinate_search_query_contract_digest()
            || query_vector.generation_fences() != &self.ticket.generation_fences()?
        {
            return Err(DbError::InvalidData(
                "Coordinate-search vector does not match the closed global Coordinate scope"
                    .to_owned(),
            ));
        }
        let rows = self
            .query_generation_bound_source_score_rows(
                LifecycleFilter::AllCurrent,
                &[],
                &[query_vector],
                None,
                Some(observed_limit),
                SemanticExactScoreScope::GlobalGraphCoordinates,
            )
            .await?;
        rows.iter()
            .map(|row| {
                Ok(SemanticGlobalCoordinateScore {
                    channel_id: super::digest_from_bytes(row.try_get("channel_id")?, "channel_id")?,
                    source: super::source_identity_from_row(
                        row,
                        "source_family",
                        "source_subtype",
                        "source_id",
                    )?,
                    score: super::score_from_i64(row.try_get("semantic_score")?)?,
                    channel_rank: super::positive_u32(
                        row.try_get("channel_rank")?,
                        "channel_rank",
                    )?,
                })
            })
            .collect()
    }
}

impl std::fmt::Debug for SemanticExactQueryVector {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_tuple("SemanticExactQueryVector").finish()
    }
}

impl SemanticExactQueryVector {
    /// Bind one graph Q0/Qi Provider result to the exact ticket.
    pub fn new(
        ticket: &SemanticGraphQueryTicket,
        encoded: ProviderEncodedSemanticInput,
    ) -> Result<Self> {
        if matches!(
            encoded.channel_kind(),
            SemanticQueryInputKind::CoordinateSearch
        ) || encoded.encoding_contract_digest() != ticket.query_fences.query_contract_digest
        {
            return Err(DbError::InvalidData(
                "graph query vector uses the wrong closed input contract".to_owned(),
            ));
        }
        Ok(Self {
            inner: GenerationBoundQueryVector::bind(ticket, encoded)?,
        })
    }

    /// Stable request-local branch identity.
    pub const fn channel_id(&self) -> Digest32 {
        self.inner.channel_id()
    }

    /// Digest of the exact Provider input bytes.
    pub const fn input_digest(&self) -> Digest32 {
        self.inner.input_digest()
    }

    pub(super) const fn generation_bound(&self) -> &GenerationBoundQueryVector {
        &self.inner
    }
}

pub(super) fn validate_generation_bound_vectors<'a>(
    ticket: &SemanticGraphQueryTicket,
    vectors: impl IntoIterator<Item = &'a GenerationBoundQueryVector>,
    count: usize,
) -> Result<()> {
    if count == 0 || count > MAX_QUERY_CHANNELS {
        return Err(DbError::InvalidData(
            "semantic generation-bound vector count is outside the server bound".to_owned(),
        ));
    }
    let expected = ticket.generation_fences()?;
    let mut request_id = None;
    let mut channel_ids = BTreeSet::new();
    let mut observed = 0_usize;
    for (index, vector) in vectors.into_iter().enumerate() {
        observed = observed
            .checked_add(1)
            .ok_or_else(|| DbError::InvalidData("semantic vector count overflow".to_owned()))?;
        if vector.generation_fences() != &expected
            || vector.response_model() != expected.model
            || vector.embedding().as_slice().len() != expected.dimensions
            || vector
                .embedding()
                .as_slice()
                .iter()
                .any(|value| !value.is_finite())
            || vector_norm_squared(vector.embedding().as_slice()) <= 0.0
        {
            return Err(DbError::InvalidData(format!(
                "semantic query vector {index} does not match the exact generation"
            )));
        }
        if request_id.is_some_and(|request_id| request_id != vector.request_id()) {
            return Err(DbError::InvalidData(
                "semantic query vectors cross request identities".to_owned(),
            ));
        }
        request_id = Some(vector.request_id());
        if !channel_ids.insert(vector.channel_id()) {
            return Err(DbError::InvalidData(
                "semantic query channel ids must be unique".to_owned(),
            ));
        }
    }
    if observed != count {
        return Err(DbError::InvalidData(
            "semantic query vector iterator count mismatch".to_owned(),
        ));
    }
    Ok(())
}
