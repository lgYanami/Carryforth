use std::cmp::Ordering;
use std::collections::BTreeSet;

use buzz_project_context::{EdgeKey, ProjectContextCoordinate};
use buzz_semantic::Digest32;
use uuid::Uuid;

use crate::{
    path_score, BranchStopReason, QueryContractResult, RootStructuralEntrypoint, Score,
    SemanticEdgeObservation, SemanticGraphQueryBudget, SemanticGraphQueryError,
    SemanticHyperedgeHop, SemanticRelationDocument, MAX_BEAM_WIDTH,
};

/// Internal Stage C keyset position for relation ranking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationRankCursor {
    /// Last emitted Document score in descending rank.
    pub document_score: Score,
    /// Canonical Edge tie-break.
    pub edge_key: EdgeKey,
    /// Canonical Document tie-break.
    pub document_id: Uuid,
}

/// Internal Stage C keyset position for target ranking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetRankCursor {
    /// Last emitted transition score in descending rank.
    pub transition_score: Score,
    /// Canonical Coordinate tie-break.
    pub target_coordinate: ProjectContextCoordinate,
}

/// One immutable logical traversal prefix. Successors are admitted separately
/// so a partial hop can never enter the frontier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontierPathState {
    /// Owning root.
    pub root_id: Digest32,
    /// Exact structural root entrypoint.
    pub structural_entrypoint: RootStructuralEntrypoint,
    /// Complete ordered hop prefix.
    pub hops: Vec<SemanticHyperedgeHop>,
    /// Coordinate whose incident relations should next be expanded.
    pub current_coordinate: Option<ProjectContextCoordinate>,
    /// Path-local visited Coordinates.
    pub visited_coordinates: BTreeSet<ProjectContextCoordinate>,
    /// Path-local visited Edges.
    pub visited_edges: BTreeSet<EdgeKey>,
    /// Optional root score for an embedding-less explicit initial.
    pub root_score: Option<Score>,
    /// Current path score once at least one scored component exists.
    pub path_score: Option<Score>,
}

impl FrontierPathState {
    /// Construct a zero-hop seed with one visited Coordinate when applicable.
    pub fn seed(
        root_id: Digest32,
        structural_entrypoint: RootStructuralEntrypoint,
        root_score: Option<Score>,
    ) -> Self {
        let current_coordinate = match &structural_entrypoint {
            RootStructuralEntrypoint::Coordinate { coordinate } => Some(coordinate.clone()),
            RootStructuralEntrypoint::ContextDocument { .. } => None,
        };
        let visited_coordinates = current_coordinate.iter().cloned().collect();
        Self {
            root_id,
            structural_entrypoint,
            hops: Vec::new(),
            current_coordinate,
            visited_coordinates,
            visited_edges: BTreeSet::new(),
            root_score,
            path_score: root_score,
        }
    }

    /// Atomically append one complete hop, rejecting path-local Coordinate or
    /// Edge cycles and recomputing the exact fixed-point path score.
    pub fn append_hop(&self, mut hop: SemanticHyperedgeHop) -> QueryContractResult<Self> {
        let target = hop.continued_to_coordinate.coordinate.clone();
        if hop.entered_from_coordinate != self.current_coordinate {
            return Err(SemanticGraphQueryError::InvalidState(
                "frontier successor is not Coordinate-contiguous".to_owned(),
            ));
        }
        if self.hops.is_empty() {
            let starts_at_entrypoint = match &self.structural_entrypoint {
                RootStructuralEntrypoint::Coordinate { coordinate } => {
                    hop.entered_from_coordinate.as_ref() == Some(coordinate)
                }
                RootStructuralEntrypoint::ContextDocument {
                    edge_key,
                    document_id,
                    edge_provenance,
                    binding_provenance,
                } => {
                    hop.entered_from_coordinate.is_none()
                        && hop.edge.edge_key == *edge_key
                        && hop.edge.provenance == *edge_provenance
                        && hop.selected_relation_document.document_id == *document_id
                        && hop.selected_relation_document.binding_provenance == *binding_provenance
                }
            };
            if !starts_at_entrypoint {
                return Err(SemanticGraphQueryError::InvalidState(
                    "frontier successor does not start at its structural entrypoint".to_owned(),
                ));
            }
        }
        if hop
            .entered_from_coordinate
            .as_ref()
            .is_some_and(|entered| !hop.edge.complete_coordinates.contains(entered))
        {
            return Err(SemanticGraphQueryError::InvalidState(
                "frontier entered Coordinate is not a complete Hyperedge member".to_owned(),
            ));
        }
        if !hop.edge.complete_coordinates.contains(&target)
            || hop.entered_from_coordinate.as_ref() == Some(&target)
        {
            return Err(SemanticGraphQueryError::InvalidState(
                "frontier target Coordinate is not a distinct complete Hyperedge member".to_owned(),
            ));
        }
        if self.visited_edges.contains(&hop.edge.edge_key)
            || self.visited_coordinates.contains(&target)
        {
            return Err(SemanticGraphQueryError::InvalidState(
                "frontier successor repeats a path Coordinate or Edge".to_owned(),
            ));
        }
        let ordinal = self
            .hops
            .len()
            .checked_add(1)
            .and_then(|value| u16::try_from(value).ok())
            .ok_or_else(|| {
                SemanticGraphQueryError::InvalidState("hop ordinal overflow".to_owned())
            })?;
        hop.ordinal = ordinal;
        let mut successor = self.clone();
        successor.visited_edges.insert(hop.edge.edge_key);
        successor.visited_coordinates.insert(target.clone());
        successor.current_coordinate = Some(target);
        successor.hops.push(hop);
        let transitions = successor
            .hops
            .iter()
            .map(|item| item.transition_score)
            .collect::<Vec<_>>();
        successor.path_score = path_score(successor.root_score, &transitions)
            .map_err(|error| SemanticGraphQueryError::InvalidScore(error.to_string()))?
            .final_score;
        Ok(successor)
    }

    /// Scheduling score; unscored explicit seeds use zero without changing the
    /// public semantic score.
    pub fn scheduling_priority(&self) -> Score {
        self.path_score.map_or(Score::ZERO, |score| score)
    }

    fn provenance_key(&self) -> Vec<u8> {
        let mut key = structural_entrypoint_key(&self.structural_entrypoint);
        for hop in &self.hops {
            key.extend_from_slice(hop.edge.edge_key.as_bytes());
            key.extend_from_slice(hop.selected_relation_document.document_id.as_bytes());
            append_coordinate_key(&mut key, &hop.continued_to_coordinate.coordinate);
        }
        key
    }
}

/// Coordinate-incident expansion continuation retained after a fair quantum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncidentExpansionContinuation {
    /// Prefix being expanded.
    pub path_state: Box<FrontierPathState>,
    /// Last relation rank emitted inside the Stage C snapshot.
    pub after_relation_rank: Option<RelationRankCursor>,
}

/// Edge-target expansion continuation retained after a fair quantum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetExpansionContinuation {
    /// Prefix being expanded.
    pub path_state: Box<FrontierPathState>,
    /// Entered Coordinate, absent for a relation-Document seed.
    pub entered_from_coordinate: Option<ProjectContextCoordinate>,
    /// Complete current Hyperedge identity.
    pub edge: SemanticEdgeObservation,
    /// Selected relation Document.
    pub document: SemanticRelationDocument,
    /// Last target rank emitted inside the Stage C snapshot.
    pub after_target_rank: Option<TargetRankCursor>,
}

/// Closed internal continuation union; it is never a public pagination cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpansionContinuation {
    /// Resume incident relation ranking for one Coordinate prefix.
    CoordinateIncident(IncidentExpansionContinuation),
    /// Resume target ranking for one selected Edge/Document option.
    EdgeTargets(Box<TargetExpansionContinuation>),
}

/// Result of admitting one unique logical materialization key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CounterAdmission {
    /// Key was already materialized and consumes no additional budget.
    Reused,
    /// New key was atomically admitted and counted.
    Admitted,
    /// New key was suppressed because the corresponding cap was reached.
    Exhausted,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ExpandedCoordinateKey {
    provenance_key: Vec<u8>,
    coordinate: ProjectContextCoordinate,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RelationOptionKey {
    entered_from: Option<ProjectContextCoordinate>,
    edge_key: EdgeKey,
    document_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct TargetOptionKey {
    entered_from: Option<ProjectContextCoordinate>,
    edge_key: EdgeKey,
    document_id: Uuid,
    target: ProjectContextCoordinate,
}

/// Exact unique-key materialization counters used by traversal. Failed later
/// admission never creates a partial hop, while already completed inspection
/// remains honestly counted.
#[derive(Debug, Default, Clone)]
pub struct TraversalMaterializationCounters {
    expanded_coordinates: BTreeSet<ExpandedCoordinateKey>,
    incident_edges: BTreeSet<EdgeKey>,
    relation_options: BTreeSet<RelationOptionKey>,
    target_options: BTreeSet<TargetOptionKey>,
}

impl TraversalMaterializationCounters {
    /// Current unique expanded-Coordinate count.
    pub fn expanded_coordinates(&self) -> usize {
        self.expanded_coordinates.len()
    }

    /// Current unique complete-Edge materialization count.
    pub fn incident_edges_materialized(&self) -> usize {
        self.incident_edges.len()
    }

    /// Current unique relation-option count.
    pub fn relation_options_materialized(&self) -> usize {
        self.relation_options.len()
    }

    /// Current unique target-option count.
    pub fn target_options_materialized(&self) -> usize {
        self.target_options.len()
    }

    /// Begin a provenance-distinct Coordinate incident expansion.
    pub fn admit_expanded_coordinate(
        &mut self,
        state: &FrontierPathState,
        coordinate: ProjectContextCoordinate,
        budget: &SemanticGraphQueryBudget,
    ) -> QueryContractResult<CounterAdmission> {
        let key = ExpandedCoordinateKey {
            provenance_key: state.provenance_key(),
            coordinate,
        };
        Ok(admit_unique(
            &mut self.expanded_coordinates,
            key,
            usize::from(budget.max_expanded_coordinates),
        ))
    }

    /// Admit one globally unique complete Edge identity.
    pub fn admit_incident_edge(
        &mut self,
        edge_key: EdgeKey,
        budget: &SemanticGraphQueryBudget,
    ) -> CounterAdmission {
        admit_unique(
            &mut self.incident_edges,
            edge_key,
            usize::from(budget.max_incident_edges_materialized),
        )
    }

    /// Admit one unique `(U?, E, D)` relation option.
    pub fn admit_relation_option(
        &mut self,
        entered_from: Option<ProjectContextCoordinate>,
        edge_key: EdgeKey,
        document_id: Uuid,
        budget: &SemanticGraphQueryBudget,
    ) -> CounterAdmission {
        admit_unique(
            &mut self.relation_options,
            RelationOptionKey {
                entered_from,
                edge_key,
                document_id,
            },
            usize::from(budget.max_relation_options_materialized),
        )
    }

    /// Admit one unique `(U?, E, D, V)` target option.
    pub fn admit_target_option(
        &mut self,
        entered_from: Option<ProjectContextCoordinate>,
        edge_key: EdgeKey,
        document_id: Uuid,
        target: ProjectContextCoordinate,
        budget: &SemanticGraphQueryBudget,
    ) -> CounterAdmission {
        admit_unique(
            &mut self.target_options,
            TargetOptionKey {
                entered_from,
                edge_key,
                document_id,
                target,
            },
            usize::from(budget.max_target_options_materialized),
        )
    }
}

fn admit_unique<T: Ord>(set: &mut BTreeSet<T>, key: T, cap: usize) -> CounterAdmission {
    if set.contains(&key) {
        CounterAdmission::Reused
    } else if set.len() >= cap {
        CounterAdmission::Exhausted
    } else {
        set.insert(key);
        CounterAdmission::Admitted
    }
}

/// Per-logical-state deterministic top-B successor accumulator.
#[derive(Debug, Clone)]
pub struct BoundedSuccessorAccumulator {
    beam_width: usize,
    successors: Vec<FrontierPathState>,
    observed_suppressed_successor: bool,
}

impl BoundedSuccessorAccumulator {
    /// Construct an accumulator under the frozen beam hard cap.
    pub fn new(beam_width: u16) -> QueryContractResult<Self> {
        if beam_width == 0 || beam_width > MAX_BEAM_WIDTH {
            return Err(SemanticGraphQueryError::InvalidState(
                "successor accumulator beam width is outside the closed cap".to_owned(),
            ));
        }
        Ok(Self {
            beam_width: usize::from(beam_width),
            successors: Vec::with_capacity(usize::from(beam_width)),
            observed_suppressed_successor: false,
        })
    }

    /// Admit one complete successor and retain the deterministic best B.
    /// Returns whether the supplied successor remains retained.
    pub fn admit(&mut self, successor: FrontierPathState) -> QueryContractResult<bool> {
        let supplied_key = successor.provenance_key();
        self.successors.push(successor);
        self.successors.sort_by(compare_path_states);
        if self.successors.len() > self.beam_width {
            self.observed_suppressed_successor = true;
            self.successors.pop();
        }
        Ok(self
            .successors
            .iter()
            .any(|state| state.provenance_key() == supplied_key))
    }

    /// Whether a qualifying B+1 successor was observed.
    pub const fn observed_suppressed_successor(&self) -> bool {
        self.observed_suppressed_successor
    }

    /// Publish retained successors only after the logical state is sealed.
    pub fn into_successors(self) -> Vec<FrontierPathState> {
        self.successors
    }
}

fn compare_path_states(left: &FrontierPathState, right: &FrontierPathState) -> Ordering {
    right
        .scheduling_priority()
        .cmp(&left.scheduling_priority())
        .then_with(|| left.provenance_key().cmp(&right.provenance_key()))
}

fn structural_entrypoint_key(entrypoint: &RootStructuralEntrypoint) -> Vec<u8> {
    match entrypoint {
        RootStructuralEntrypoint::Coordinate { coordinate } => {
            let mut key = vec![0];
            append_coordinate_key(&mut key, coordinate);
            key
        }
        RootStructuralEntrypoint::ContextDocument {
            edge_key,
            document_id,
            ..
        } => {
            let mut key = vec![1];
            key.extend_from_slice(edge_key.as_bytes());
            key.extend_from_slice(document_id.as_bytes());
            key
        }
    }
}

fn append_coordinate_key(output: &mut Vec<u8>, coordinate: &ProjectContextCoordinate) {
    match coordinate {
        ProjectContextCoordinate::ProjectViewObject {
            object_type,
            object_id,
        } => {
            output.push(0);
            output.extend_from_slice(object_type.as_str().as_bytes());
            output.push(0);
            output.extend_from_slice(object_id.as_bytes());
        }
        ProjectContextCoordinate::Document { document_id } => {
            output.push(1);
            output.extend_from_slice(document_id.as_bytes());
        }
        ProjectContextCoordinate::Meeting { meeting_id } => {
            output.push(2);
            output.extend_from_slice(meeting_id.as_bytes());
        }
    }
}

/// Compute the deterministic first-wave per-seed quantum for one remaining
/// materialization dimension.
pub fn first_wave_slice(remaining_budget: usize, remaining_seeds: usize) -> usize {
    if remaining_seeds == 0 {
        0
    } else {
        remaining_budget.div_ceil(remaining_seeds)
    }
}

/// Select the highest-precedence branch stop reason.
pub fn highest_precedence_stop(reasons: &[BranchStopReason]) -> Option<BranchStopReason> {
    reasons
        .iter()
        .copied()
        .min_by_key(|reason| reason.precedence())
}

#[cfg(test)]
mod tests {
    use super::{first_wave_slice, highest_precedence_stop};
    use crate::BranchStopReason;

    #[test]
    fn first_wave_uses_ceiling_and_stop_reason_uses_closed_precedence() {
        assert_eq!(first_wave_slice(10, 3), 4);
        assert_eq!(first_wave_slice(0, 3), 0);
        assert_eq!(first_wave_slice(10, 0), 0);
        assert_eq!(
            highest_precedence_stop(&[
                BranchStopReason::FrontierExhausted,
                BranchStopReason::MaxHopsReached,
                BranchStopReason::WallTimeExhausted,
            ]),
            Some(BranchStopReason::WallTimeExhausted)
        );
    }
}
