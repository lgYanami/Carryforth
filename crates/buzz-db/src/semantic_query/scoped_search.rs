//! Structure-scoped direct semantic ranking for one-hop Agent selection.
//!
//! This module reuses the semantic-graph ticket, exact Q0 vector, current-head
//! scorer, structural loaders, and canonical source adapters. It deliberately
//! does not call root fusion, coherence, floors, traversal, or path packing.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use buzz_project_context::{EdgeKey, ProjectContextCoordinate};
use buzz_semantic::{
    IneligibilityReason, SemanticCoverage, SemanticEligibility, SemanticSourceBasis,
    SemanticSourceIdentity,
};
use buzz_semantic_query::{
    EdgeCoordinateCoverage, IncidentEdgeCoverage, OneHopCandidatePreview,
    OneHopCanonicalCandidateObservation, OneHopCanonicalRead, OneHopOmittedCandidateCounts,
    OneHopRankedCoordinate, OneHopRankedDocument, OneHopRankedEdge, OneHopSemanticSelection,
    ProjectContextCoordinateTypeFilter, MAX_ONE_HOP_DOCUMENTS_PER_EDGE,
    MAX_ONE_HOP_EDGE_COORDINATES, MAX_ONE_HOP_HYPEREDGE_IDENTITY_BYTES, MAX_ONE_HOP_INCIDENT_EDGES,
    MAX_ONE_HOP_RELATION_BINDINGS, MAX_ONE_HOP_SEMANTIC_LIMIT,
};
use sqlx::Row;
use uuid::Uuid;

use super::{
    ensure_eligible_canonical_observation, load_complete_hyperedge_in_tx,
    load_incident_relation_refs_in_tx, project_document_source, semantic_hyperedge_identity_bytes,
    semantic_source_identity_for_coordinate, semantic_source_sort_key,
    validate_canonical_against_head, validate_coordinate, validate_query_vectors,
    CurrentSemanticAvailabilityClass, CurrentSemanticSourceState, IncidentRelationRef,
    SemanticExactExplicitSourceScope, SemanticExactQueryVector, SemanticExactSourceScore,
    SemanticGraphReadTx, SemanticGraphSnapshotBinding,
};
use crate::semantic::observe_semantic_source_preview_in_connection;
use crate::{DbError, Result};

/// One snapshot-bound one-hop semantic selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticOneHopSearchBatch {
    /// Current generation and Project Context snapshot used for every field.
    pub snapshot: SemanticGraphSnapshotBinding,
    /// Exact closed variant produced by the selected structural scope.
    pub selection: OneHopSemanticSelection,
}

/// Result of selecting semantically relevant incident Edges for one Coordinate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticIncidentEdgeSearchOutcome {
    /// The Coordinate exists on at least one active Edge and ranking completed.
    Ranked(Box<SemanticOneHopSearchBatch>),
    /// The Coordinate is not a member of any current active Edge.
    NotFound,
    /// The complete incident structure exceeds a fixed server bound.
    ScopeTooLarge {
        /// Current active incident Edge count.
        active_incident_edges: u32,
        /// Current active `(Edge, Document)` binding count.
        active_relation_bindings: u32,
    },
}

/// Result of selecting semantically relevant members inside one exact Edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticEdgeCoordinateSearchOutcome {
    /// The complete current Edge was ranked.
    Ranked(Box<SemanticOneHopSearchBatch>),
    /// The Edge is absent or no longer active in the current snapshot.
    NotFound,
    /// The complete member set exceeds its fixed server bound.
    ScopeTooLarge {
        /// Current complete member count observed before ranking.
        edge_coordinate_count: u32,
    },
    /// The canonical complete Hyperedge identity exceeds 64 KiB.
    HyperedgeTooLarge {
        /// Exact serialized identity byte count.
        identity_bytes: usize,
    },
}

#[derive(Clone)]
struct ScoredRelation {
    document_id: Uuid,
    score: SemanticExactSourceScore,
}

struct RankedEdgeWork {
    edge_key: EdgeKey,
    binding_document_count: u32,
    documents: Vec<ScoredRelation>,
}

impl SemanticGraphReadTx {
    /// Rank a Coordinate's incident Edges through direct Q0 scores of their
    /// current bound relation Documents.
    ///
    /// The result never includes any Edge member Coordinate. It has no
    /// relevance floor and performs no local-coherence or traversal scoring.
    pub async fn search_incident_edges_one_hop(
        &mut self,
        coordinate: &ProjectContextCoordinate,
        query_vector: &SemanticExactQueryVector,
        limit: u8,
    ) -> Result<SemanticIncidentEdgeSearchOutcome> {
        validate_one_hop_input(self, query_vector, limit)?;
        validate_coordinate(self.ticket.community_id, coordinate)?;
        let (active_incident_edges, active_relation_bindings) =
            self.incident_scope_counts(coordinate).await?;
        if active_incident_edges == 0 {
            return Ok(SemanticIncidentEdgeSearchOutcome::NotFound);
        }
        if active_incident_edges > MAX_ONE_HOP_INCIDENT_EDGES
            || active_relation_bindings > MAX_ONE_HOP_RELATION_BINDINGS
        {
            return Ok(SemanticIncidentEdgeSearchOutcome::ScopeTooLarge {
                active_incident_edges,
                active_relation_bindings,
            });
        }

        let refs = load_incident_relation_refs_in_tx(
            &mut self.tx,
            &self.ticket,
            coordinate,
            usize::try_from(active_relation_bindings).map_err(|_| {
                DbError::InvalidData("one-hop relation count exceeds usize".to_owned())
            })?,
        )
        .await?;
        if refs.len()
            != usize::try_from(active_relation_bindings).map_err(|_| {
                DbError::InvalidData("one-hop relation count exceeds usize".to_owned())
            })?
        {
            return Err(DbError::InvalidData(
                "one-hop incident relation count changed inside its snapshot".to_owned(),
            ));
        }

        let mut unique_sources = BTreeMap::new();
        for relation in &refs {
            let source = project_document_source(self.ticket.community_id, relation.document_id);
            unique_sources
                .entry(semantic_source_sort_key(&source))
                .or_insert(source);
        }
        let sources = unique_sources.into_values().collect::<Vec<_>>();
        let scores = self
            .score_explicit_source_scope_exact(
                query_vector,
                SemanticExactExplicitSourceScope::OneHopIncidentDocuments(&sources),
            )
            .await?;
        let score_map = exact_score_map(scores, query_vector)?;
        let states = self
            .load_current_semantic_source_states(
                &sources,
                usize::try_from(MAX_ONE_HOP_RELATION_BINDINGS).map_err(|_| {
                    DbError::InvalidData("one-hop relation cap exceeds usize".to_owned())
                })?,
            )
            .await?;
        let state_map = source_state_map(&sources, states)?;

        let mut bindings_per_edge = BTreeMap::<EdgeKey, u32>::new();
        let mut scored_per_edge = BTreeMap::<EdgeKey, Vec<ScoredRelation>>::new();
        let mut omissions = OneHopOmittedCandidateCounts::default();
        let mut scorable_relation_bindings = 0_u32;
        let mut title_only_scorable_bindings = 0_u32;
        for relation in &refs {
            increment(
                bindings_per_edge.entry(relation.edge_key).or_default(),
                "one-hop Edge binding count",
            )?;
            let source = project_document_source(self.ticket.community_id, relation.document_id);
            let key = semantic_source_sort_key(&source);
            let state = state_map.get(&key).ok_or_else(|| {
                DbError::InvalidData("one-hop relation source state is missing".to_owned())
            })?;
            let Some(score) = score_map.get(&key) else {
                count_omission(&mut omissions, state)?;
                continue;
            };
            require_score_matches_state(score, state)?;
            require_relation_role(score, relation)?;
            increment(
                &mut scorable_relation_bindings,
                "one-hop scorable relation count",
            )?;
            if score.head.summary_coverage == SemanticCoverage::TitleOnly {
                increment(
                    &mut title_only_scorable_bindings,
                    "one-hop title-only relation count",
                )?;
            }
            scored_per_edge
                .entry(relation.edge_key)
                .or_default()
                .push(ScoredRelation {
                    document_id: relation.document_id,
                    score: score.clone(),
                });
        }

        let mut ranked = Vec::with_capacity(scored_per_edge.len());
        for (edge_key, mut documents) in scored_per_edge {
            documents.sort_by(compare_scored_relations);
            let binding_document_count =
                bindings_per_edge.get(&edge_key).copied().ok_or_else(|| {
                    DbError::InvalidData("one-hop Edge lost its binding count".to_owned())
                })?;
            ranked.push(RankedEdgeWork {
                edge_key,
                binding_document_count,
                documents,
            });
        }
        ranked.sort_by(compare_ranked_edges);
        let scorable_edges = u32_count(ranked.len(), "one-hop scorable Edge count")?;
        let truncated = ranked.len() > usize::from(limit);
        ranked.truncate(usize::from(limit));

        let retained_scores = unique_relation_scores(&ranked);
        let hydrated = self.hydrate_one_hop_scores(&retained_scores).await?;
        let mut edges = Vec::with_capacity(ranked.len());
        for (edge_index, edge) in ranked.into_iter().enumerate() {
            let scorable_document_count =
                u32_count(edge.documents.len(), "one-hop scorable Document count")?;
            let documents_truncated = edge.documents.len() > MAX_ONE_HOP_DOCUMENTS_PER_EDGE;
            let mut ranked_documents =
                Vec::with_capacity(edge.documents.len().min(MAX_ONE_HOP_DOCUMENTS_PER_EDGE));
            for (document_index, document) in edge
                .documents
                .into_iter()
                .take(MAX_ONE_HOP_DOCUMENTS_PER_EDGE)
                .enumerate()
            {
                let key = semantic_source_sort_key(&document.score.source);
                let observation = hydrated.get(&key).cloned().ok_or_else(|| {
                    DbError::InvalidData("one-hop Document hydration is missing".to_owned())
                })?;
                let revision = match &observation.source_basis {
                    SemanticSourceBasis::ProjectDocument(basis) => basis.document_revision,
                    _ => {
                        return Err(DbError::InvalidData(
                            "one-hop relation Document has a non-Document basis".to_owned(),
                        ));
                    }
                };
                ranked_documents.push(OneHopRankedDocument {
                    rank: rank(document_index)?,
                    document_id: document.document_id,
                    document_revision: revision,
                    score: document.score.score,
                    canonical_observation: observation,
                });
            }
            let edge_score = ranked_documents
                .first()
                .map(|document| document.score)
                .ok_or_else(|| {
                    DbError::InvalidData("one-hop ranked Edge has no Document".to_owned())
                })?;
            edges.push(OneHopRankedEdge {
                rank: rank(edge_index)?,
                edge_key: edge.edge_key,
                score: edge_score,
                ranked_documents,
                binding_document_count: edge.binding_document_count,
                scorable_document_count,
                documents_truncated,
            });
        }

        let selection = OneHopSemanticSelection::IncidentEdges {
            coordinate: coordinate.clone(),
            edges,
            coverage: IncidentEdgeCoverage {
                active_incident_edges,
                active_relation_bindings,
                scorable_relation_bindings,
                scorable_edges,
                title_only_scorable_bindings,
                omitted_relation_bindings: omissions,
            },
            truncated,
        };
        Ok(SemanticIncidentEdgeSearchOutcome::Ranked(Box::new(
            SemanticOneHopSearchBatch {
                snapshot: self.snapshot_binding(),
                selection,
            },
        )))
    }

    /// Rank the complete current member set of one Edge by direct Q0 score.
    ///
    /// The result never includes relation Documents or the complete Edge. It
    /// has no relevance floor and performs no relation or transition scoring.
    pub async fn search_edge_coordinates_one_hop(
        &mut self,
        edge_key: EdgeKey,
        query_vector: &SemanticExactQueryVector,
        limit: u8,
    ) -> Result<SemanticEdgeCoordinateSearchOutcome> {
        self.search_edge_coordinates_one_hop_with_filter(edge_key, query_vector, limit, None)
            .await
    }

    /// Rank only members of one exact Edge matching a closed Coordinate type set.
    pub async fn search_edge_coordinates_one_hop_filtered(
        &mut self,
        edge_key: EdgeKey,
        query_vector: &SemanticExactQueryVector,
        coordinate_types: &ProjectContextCoordinateTypeFilter,
        limit: u8,
    ) -> Result<SemanticEdgeCoordinateSearchOutcome> {
        if !coordinate_types.is_canonical() {
            return Err(DbError::InvalidData(
                "one-hop Coordinate type filter is not canonical".to_owned(),
            ));
        }
        self.search_edge_coordinates_one_hop_with_filter(
            edge_key,
            query_vector,
            limit,
            Some(coordinate_types),
        )
        .await
    }

    async fn search_edge_coordinates_one_hop_with_filter(
        &mut self,
        edge_key: EdgeKey,
        query_vector: &SemanticExactQueryVector,
        limit: u8,
        coordinate_types: Option<&ProjectContextCoordinateTypeFilter>,
    ) -> Result<SemanticEdgeCoordinateSearchOutcome> {
        validate_one_hop_input(self, query_vector, limit)?;
        let Some(edge) =
            load_complete_hyperedge_in_tx(&mut self.tx, &self.ticket, edge_key).await?
        else {
            return Ok(SemanticEdgeCoordinateSearchOutcome::NotFound);
        };
        let identity_bytes = semantic_hyperedge_identity_bytes(&edge)?;
        if identity_bytes > MAX_ONE_HOP_HYPEREDGE_IDENTITY_BYTES {
            return Ok(SemanticEdgeCoordinateSearchOutcome::HyperedgeTooLarge { identity_bytes });
        }
        let edge_coordinate_count = u32_count(
            edge.complete_coordinates.len(),
            "one-hop complete Edge member count",
        )?;
        if edge_coordinate_count > MAX_ONE_HOP_EDGE_COORDINATES {
            return Ok(SemanticEdgeCoordinateSearchOutcome::ScopeTooLarge {
                edge_coordinate_count,
            });
        }

        let matched_coordinates = edge
            .complete_coordinates
            .iter()
            .filter(|coordinate| coordinate_types.is_none_or(|filter| filter.matches(coordinate)))
            .cloned()
            .collect::<Vec<_>>();
        let type_matched_coordinate_count = u32_count(
            matched_coordinates.len(),
            "one-hop type-matched Coordinate count",
        )?;
        let type_filtered_out_coordinates = edge_coordinate_count
            .checked_sub(type_matched_coordinate_count)
            .ok_or_else(|| {
                DbError::InvalidData("one-hop filtered Coordinate count underflow".to_owned())
            })?;

        let sources = matched_coordinates
            .iter()
            .map(|coordinate| {
                semantic_source_identity_for_coordinate(self.ticket.community_id, coordinate)
            })
            .collect::<Result<Vec<_>>>()?;
        let scores = self
            .score_explicit_source_scope_exact(
                query_vector,
                SemanticExactExplicitSourceScope::OneHopEdgeCoordinates(&sources),
            )
            .await?;
        let score_map = exact_score_map(scores, query_vector)?;
        let states = self
            .load_current_semantic_source_states(
                &sources,
                usize::try_from(MAX_ONE_HOP_EDGE_COORDINATES).map_err(|_| {
                    DbError::InvalidData("one-hop Coordinate cap exceeds usize".to_owned())
                })?,
            )
            .await?;
        let state_map = source_state_map(&sources, states)?;

        let mut omissions = OneHopOmittedCandidateCounts::default();
        let mut title_only_scorable_coordinates = 0_u32;
        let mut scored = Vec::with_capacity(score_map.len());
        for (coordinate, source) in matched_coordinates.into_iter().zip(&sources) {
            let key = semantic_source_sort_key(source);
            let state = state_map.get(&key).ok_or_else(|| {
                DbError::InvalidData("one-hop Coordinate source state is missing".to_owned())
            })?;
            let Some(score) = score_map.get(&key) else {
                count_omission(&mut omissions, state)?;
                continue;
            };
            require_score_matches_state(score, state)?;
            if !score.roles.coordinate
                || !score
                    .roles
                    .coordinate_incident_edge_keys
                    .contains(&edge_key)
            {
                return Err(DbError::InvalidData(
                    "one-hop Coordinate score lost its exact Edge membership".to_owned(),
                ));
            }
            if score.head.summary_coverage == SemanticCoverage::TitleOnly {
                increment(
                    &mut title_only_scorable_coordinates,
                    "one-hop title-only Coordinate count",
                )?;
            }
            scored.push((coordinate, score.clone()));
        }
        scored.sort_by(|left, right| {
            right
                .1
                .score
                .cmp(&left.1.score)
                .then_with(|| left.0.cmp(&right.0))
        });
        let scorable_coordinates = u32_count(scored.len(), "one-hop scorable Coordinate count")?;
        let truncated = scored.len() > usize::from(limit);
        scored.truncate(usize::from(limit));
        let retained_scores = scored
            .iter()
            .map(|(_, score)| score.clone())
            .collect::<Vec<_>>();
        let hydrated = self.hydrate_one_hop_scores(&retained_scores).await?;
        let mut ranked_coordinates = Vec::with_capacity(scored.len());
        for (index, (coordinate, score)) in scored.into_iter().enumerate() {
            let key = semantic_source_sort_key(&score.source);
            let observation = hydrated.get(&key).cloned().ok_or_else(|| {
                DbError::InvalidData("one-hop Coordinate hydration is missing".to_owned())
            })?;
            ranked_coordinates.push(OneHopRankedCoordinate {
                rank: rank(index)?,
                coordinate,
                score: score.score,
                canonical_observation: observation,
            });
        }
        let selection = OneHopSemanticSelection::EdgeCoordinates {
            edge_key,
            coordinate_types: coordinate_types.cloned(),
            ranked_coordinates,
            coverage: EdgeCoordinateCoverage {
                edge_coordinate_count,
                type_matched_coordinate_count: coordinate_types
                    .map(|_| type_matched_coordinate_count),
                type_filtered_out_coordinates: coordinate_types
                    .map(|_| type_filtered_out_coordinates),
                scorable_coordinates,
                title_only_scorable_coordinates,
                omitted_coordinates: omissions,
            },
            truncated,
        };
        Ok(SemanticEdgeCoordinateSearchOutcome::Ranked(Box::new(
            SemanticOneHopSearchBatch {
                snapshot: self.snapshot_binding(),
                selection,
            },
        )))
    }

    async fn incident_scope_counts(
        &mut self,
        coordinate: &ProjectContextCoordinate,
    ) -> Result<(u32, u32)> {
        let requested = super::coordinate_key_arrays(std::slice::from_ref(coordinate));
        let context_revision =
            i64::try_from(self.ticket.project_context_revision).map_err(|_| {
                DbError::InvalidData("one-hop Project Context revision exceeds int8".to_owned())
            })?;
        let row = sqlx::query(INCIDENT_SCOPE_COUNTS_SQL)
            .bind(self.ticket.community_id.as_uuid())
            .bind(requested.types[0])
            .bind(requested.subtypes[0])
            .bind(requested.ids[0])
            .bind(context_revision)
            .fetch_one(&mut *self.tx)
            .await?;
        Ok((
            u32_from_i64(
                row.try_get("active_incident_edges")?,
                "active incident Edges",
            )?,
            u32_from_i64(
                row.try_get("active_relation_bindings")?,
                "active relation bindings",
            )?,
        ))
    }

    async fn hydrate_one_hop_scores(
        &mut self,
        scores: &[SemanticExactSourceScore],
    ) -> Result<BTreeMap<(&'static str, &'static str, Uuid), OneHopCanonicalCandidateObservation>>
    {
        let mut unique = BTreeMap::new();
        for score in scores {
            let key = semantic_source_sort_key(&score.source);
            match unique.get(&key) {
                Some(previous) if *previous != score => {
                    return Err(DbError::InvalidData(
                        "one-hop retained source has conflicting exact scores".to_owned(),
                    ));
                }
                Some(_) => {}
                None => {
                    unique.insert(key, score);
                }
            }
        }
        let sources = unique
            .values()
            .map(|score| score.source.clone())
            .collect::<Vec<_>>();
        let hydration_cap =
            usize::from(MAX_ONE_HOP_SEMANTIC_LIMIT).saturating_mul(MAX_ONE_HOP_DOCUMENTS_PER_EDGE);
        let states = self
            .load_current_semantic_source_states(&sources, hydration_cap)
            .await?;
        let mut hydrated = BTreeMap::new();
        for ((key, score), state) in unique.into_iter().zip(states) {
            if state.source != score.source
                || state.availability != CurrentSemanticAvailabilityClass::Current
                || state.head.as_ref() != Some(&score.head)
            {
                return Err(DbError::InvalidData(
                    "one-hop retained source no longer matches its exact current head".to_owned(),
                ));
            }
            let preview =
                observe_semantic_source_preview_in_connection(&mut self.tx, &score.source).await?;
            ensure_eligible_canonical_observation(&preview.observation)?;
            validate_canonical_against_head(&preview.observation, &score.head)?;
            if preview.observation.filter.lifecycle != score.lifecycle
                || preview.observation.filter.source_status != score.source_status
            {
                return Err(DbError::InvalidData(
                    "one-hop canonical preview metadata disagrees with its exact score".to_owned(),
                ));
            }
            let source_invalidation_epoch = state.source_invalidation_epoch.ok_or_else(|| {
                DbError::InvalidData(
                    "one-hop current source lacks an invalidation epoch".to_owned(),
                )
            })?;
            let canonical_read =
                canonical_read_for_source(&score.source, &preview.observation.basis)?;
            hydrated.insert(
                key,
                OneHopCanonicalCandidateObservation {
                    source_basis: preview.observation.basis,
                    source_invalidation_epoch,
                    source_snapshot_digest: preview.observation.snapshot_digest,
                    lifecycle: preview.observation.filter.lifecycle,
                    source_status: preview.observation.filter.source_status,
                    preview: OneHopCandidatePreview {
                        title: preview.observation.title,
                        description: preview.description,
                        summary: preview.observation.summary,
                    },
                    canonical_read,
                },
            );
        }
        Ok(hydrated)
    }
}

fn validate_one_hop_input(
    read: &SemanticGraphReadTx,
    query_vector: &SemanticExactQueryVector,
    limit: u8,
) -> Result<()> {
    if !(1..=MAX_ONE_HOP_SEMANTIC_LIMIT).contains(&limit) {
        return Err(DbError::InvalidData(format!(
            "one-hop semantic limit must be between 1 and {MAX_ONE_HOP_SEMANTIC_LIMIT}"
        )));
    }
    validate_query_vectors(&read.ticket, std::slice::from_ref(query_vector))
}

fn exact_score_map(
    scores: Vec<SemanticExactSourceScore>,
    query_vector: &SemanticExactQueryVector,
) -> Result<BTreeMap<(&'static str, &'static str, Uuid), SemanticExactSourceScore>> {
    let mut result = BTreeMap::new();
    for score in scores {
        if score.channel_id != query_vector.channel_id() {
            return Err(DbError::InvalidData(
                "one-hop exact score has an unexpected channel".to_owned(),
            ));
        }
        let key = semantic_source_sort_key(&score.source);
        if result.insert(key, score).is_some() {
            return Err(DbError::InvalidData(
                "one-hop exact score duplicated a source".to_owned(),
            ));
        }
    }
    Ok(result)
}

fn source_state_map(
    sources: &[SemanticSourceIdentity],
    states: Vec<CurrentSemanticSourceState>,
) -> Result<BTreeMap<(&'static str, &'static str, Uuid), CurrentSemanticSourceState>> {
    if sources.len() != states.len() {
        return Err(DbError::InvalidData(
            "one-hop current source-state result is incomplete".to_owned(),
        ));
    }
    let mut result = BTreeMap::new();
    for (source, state) in sources.iter().zip(states) {
        if source != &state.source {
            return Err(DbError::InvalidData(
                "one-hop current source-state order is inconsistent".to_owned(),
            ));
        }
        result.insert(semantic_source_sort_key(source), state);
    }
    Ok(result)
}

fn require_relation_role(
    score: &SemanticExactSourceScore,
    relation: &IncidentRelationRef,
) -> Result<()> {
    if !score.roles.context_document_bindings.iter().any(|binding| {
        binding.edge_key == relation.edge_key
            && binding.edge_last_context_revision == relation.edge_provenance.last_context_revision
            && binding.edge_source_change_id == relation.edge_provenance.source_change_id
            && binding.binding_context_revision
                == relation.binding_provenance.binding_context_revision
            && binding.binding_source_change_id == relation.binding_provenance.source_change_id
            && binding.binding_projection_event_id
                == relation.binding_provenance.projection_event_id
    }) {
        return Err(DbError::InvalidData(
            "one-hop relation score lost its exact active Binding role".to_owned(),
        ));
    }
    Ok(())
}

fn require_score_matches_state(
    score: &SemanticExactSourceScore,
    state: &CurrentSemanticSourceState,
) -> Result<()> {
    if state.source != score.source
        || state.availability != CurrentSemanticAvailabilityClass::Current
        || state.head.as_ref() != Some(&score.head)
    {
        return Err(DbError::InvalidData(
            "one-hop exact score disagrees with its current source state".to_owned(),
        ));
    }
    Ok(())
}

fn compare_scored_relations(left: &ScoredRelation, right: &ScoredRelation) -> Ordering {
    right
        .score
        .score
        .cmp(&left.score.score)
        .then_with(|| left.document_id.cmp(&right.document_id))
}

fn compare_ranked_edges(left: &RankedEdgeWork, right: &RankedEdgeWork) -> Ordering {
    right.documents[0]
        .score
        .score
        .cmp(&left.documents[0].score.score)
        .then_with(|| left.edge_key.cmp(&right.edge_key))
}

fn unique_relation_scores(edges: &[RankedEdgeWork]) -> Vec<SemanticExactSourceScore> {
    let mut result = BTreeMap::new();
    for edge in edges {
        for document in edge.documents.iter().take(MAX_ONE_HOP_DOCUMENTS_PER_EDGE) {
            result
                .entry(semantic_source_sort_key(&document.score.source))
                .or_insert_with(|| document.score.clone());
        }
    }
    result.into_values().collect()
}

fn count_omission(
    counts: &mut OneHopOmittedCandidateCounts,
    state: &CurrentSemanticSourceState,
) -> Result<()> {
    match state.eligibility {
        None => increment(
            &mut counts.source_not_found,
            "one-hop source-not-found count",
        ),
        Some(SemanticEligibility::Ineligible(
            IneligibilityReason::Tombstone | IneligibilityReason::Deleted,
        )) => increment(
            &mut counts.source_tombstoned_or_deleted,
            "one-hop tombstone/deleted count",
        ),
        Some(SemanticEligibility::Ineligible(_)) => increment(
            &mut counts.source_ineligible_or_unreadable,
            "one-hop ineligible count",
        ),
        Some(SemanticEligibility::Eligible) => match state.availability {
            CurrentSemanticAvailabilityClass::Missing => increment(
                &mut counts.semantic_head_missing,
                "one-hop missing-head count",
            ),
            CurrentSemanticAvailabilityClass::Building => increment(
                &mut counts.semantic_head_building,
                "one-hop building-head count",
            ),
            CurrentSemanticAvailabilityClass::Failed
            | CurrentSemanticAvailabilityClass::Unsupported => increment(
                &mut counts.semantic_head_failed_or_unsupported,
                "one-hop failed/unsupported-head count",
            ),
            CurrentSemanticAvailabilityClass::NonQueryableZeroVector => increment(
                &mut counts.non_queryable_zero_vector,
                "one-hop zero-vector count",
            ),
            CurrentSemanticAvailabilityClass::Current => increment(
                &mut counts.source_ineligible_or_unreadable,
                "one-hop unreadable-current count",
            ),
        },
    }
}

fn canonical_read_for_source(
    source: &SemanticSourceIdentity,
    basis: &SemanticSourceBasis,
) -> Result<OneHopCanonicalRead> {
    match (source.kind, basis) {
        (
            buzz_semantic::SemanticSourceKind::ProjectView(subtype),
            SemanticSourceBasis::ProjectView(basis),
        ) => Ok(OneHopCanonicalRead::ProjectView {
            command: format!(
                "cf project-view get-object {} {}",
                project_view_object_type(subtype).as_str(),
                source.source_id
            ),
            expected_object_revision: basis.object_revision,
        }),
        (
            buzz_semantic::SemanticSourceKind::ProjectDocument,
            SemanticSourceBasis::ProjectDocument(basis),
        ) => Ok(OneHopCanonicalRead::Document {
            fetch_command: format!(
                "cf documents get {} --revision {} --content-only",
                source.source_id, basis.document_revision
            ),
            expected_document_revision: basis.document_revision,
        }),
        (buzz_semantic::SemanticSourceKind::Meeting, SemanticSourceBasis::Meeting(basis)) => {
            Ok(OneHopCanonicalRead::Meeting {
                metadata: format!("cf meetings show --meeting {}", source.source_id),
                board: format!("cf meetings board get --meeting {}", source.source_id),
                speech: format!(
                    "cf --format compact meetings history --meeting {} --limit 200",
                    source.source_id
                ),
                expected_create_event_id: basis.create_event_id,
                expected_end_event_id: basis.end_event_id,
            })
        }
        _ => Err(DbError::InvalidData(
            "one-hop canonical source basis does not match its identity".to_owned(),
        )),
    }
}

fn project_view_object_type(
    subtype: buzz_semantic::ProjectViewSemanticType,
) -> buzz_project_view::ProjectViewObjectType {
    use buzz_project_view::ProjectViewObjectType;
    use buzz_semantic::ProjectViewSemanticType;

    match subtype {
        ProjectViewSemanticType::ProjectProfile => ProjectViewObjectType::ProjectProfile,
        ProjectViewSemanticType::Goal => ProjectViewObjectType::Goal,
        ProjectViewSemanticType::Role => ProjectViewObjectType::Role,
        ProjectViewSemanticType::Plan => ProjectViewObjectType::Plan,
        ProjectViewSemanticType::Stage => ProjectViewObjectType::Stage,
        ProjectViewSemanticType::Requirement => ProjectViewObjectType::Requirement,
        ProjectViewSemanticType::Issue => ProjectViewObjectType::Issue,
        ProjectViewSemanticType::Work => ProjectViewObjectType::Work,
        ProjectViewSemanticType::Resource => ProjectViewObjectType::Resource,
    }
}

fn increment(value: &mut u32, field: &str) -> Result<()> {
    *value = value
        .checked_add(1)
        .ok_or_else(|| DbError::InvalidData(format!("{field} overflow")))?;
    Ok(())
}

fn rank(index: usize) -> Result<u8> {
    u8::try_from(index + 1)
        .map_err(|_| DbError::InvalidData("one-hop rank exceeds uint8".to_owned()))
}

fn u32_count(value: usize, field: &str) -> Result<u32> {
    u32::try_from(value).map_err(|_| DbError::InvalidData(format!("{field} exceeds uint32")))
}

fn u32_from_i64(value: i64, field: &str) -> Result<u32> {
    u32::try_from(value).map_err(|_| DbError::InvalidData(format!("{field} is outside uint32")))
}

const INCIDENT_SCOPE_COUNTS_SQL: &str = r#"
WITH authorized_snapshot AS MATERIALIZED (
    SELECT state.community_id
    FROM project_context_edge_state state
    WHERE state.community_id = $1
      AND state.schema_version = 2
      AND state.context_revision = $5
),
incident AS MATERIALIZED (
    SELECT edge.edge_key, binding.context_document_id
    FROM authorized_snapshot snapshot
    JOIN project_context_edge_coordinates coordinate
      ON coordinate.community_id = snapshot.community_id
     AND coordinate.coordinate_type = $2
     AND coordinate.coordinate_subtype IS NOT DISTINCT FROM $3
     AND coordinate.coordinate_id = $4
    JOIN project_context_edges edge
      ON edge.community_id = coordinate.community_id
     AND edge.edge_key = coordinate.edge_key
     AND edge.state = 'active'
    JOIN project_context_document_bindings binding
      ON binding.community_id = edge.community_id
     AND binding.edge_key = edge.edge_key
     AND binding.state = 'active'
)
SELECT count(DISTINCT edge_key)::bigint AS active_incident_edges,
       count(*)::bigint AS active_relation_bindings
FROM incident
"#;

#[cfg(test)]
#[path = "scoped_search_tests.rs"]
mod tests;
