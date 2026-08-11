use std::cmp::Ordering;

use buzz_semantic::{ProjectViewSemanticType, SemanticSourceIdentity, SemanticSourceKind};
use serde::{Deserialize, Serialize};

use crate::{
    root_diversity_priority, QueryContractResult, Score, SemanticGraphQueryError, BASE_ENTRY_FLOOR,
};

/// Pairwise source similarity used only by deterministic root MMR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootPairRedundancy {
    /// Other automatic source candidate.
    pub other_source: SemanticSourceIdentity,
    /// Normalized full-vector similarity to the other source.
    pub similarity: Score,
}

/// One source candidate admitted to pure root selection after role-specific
/// structural eligibility has been computed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootSelectionCandidate {
    /// Canonical source identity.
    pub source: SemanticSourceIdentity,
    /// Pure Q0 score used by the neutral lane and absolute floor.
    pub problem_score: Score,
    /// Problem-dominant Q0/Qi/anchor candidate score used by the mixed lane.
    pub candidate_score: Score,
    /// Whether this source appeared in Q0 top-K.
    pub discovered_problem_neutral: bool,
    /// Pairwise similarity to other candidates; missing pairs mean zero.
    pub redundancy_to: Vec<RootPairRedundancy>,
}

/// Closed quota lane that selected an automatic source root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomaticRootLane {
    /// Reserved problem-only half of the automatic root budget.
    ProblemNeutral,
    /// Remaining mixed Q0/Qi/relation competition.
    Mixed,
}

/// Deterministic root-selection output in selection order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedAutomaticRoot {
    /// Selected source identity.
    pub source: SemanticSourceIdentity,
    /// Lane that consumed the root slot.
    pub lane: AutomaticRootLane,
    /// Relevance used by this lane (`ProblemScore` or `CandidateScore`).
    pub relevance_score: Score,
    /// Greedy MMR priority at the time of selection.
    pub selection_priority: Score,
}

/// Select automatic roots with a reserved neutral quota, pinned strongest Q0
/// root, then deterministic per-step MMR. Input order cannot affect output.
pub fn select_automatic_roots(
    candidates: &[RootSelectionCandidate],
    max_semantic_roots: u16,
) -> QueryContractResult<Vec<SelectedAutomaticRoot>> {
    let mut candidates = candidates
        .iter()
        .filter(|candidate| candidate.problem_score >= BASE_ENTRY_FLOOR)
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| compare_sources(&left.source, &right.source));
    if candidates
        .windows(2)
        .any(|pair| pair[0].source == pair[1].source)
    {
        return Err(SemanticGraphQueryError::InvalidState(
            "automatic root candidates must be unique by source".to_owned(),
        ));
    }
    if max_semantic_roots == 0 {
        return Ok(Vec::new());
    }
    let limit = usize::from(max_semantic_roots);
    let neutral_reserved = limit.div_ceil(2);
    let mut selected = Vec::with_capacity(limit.min(candidates.len()));

    let pinned = candidates
        .iter()
        .filter(|candidate| candidate.discovered_problem_neutral)
        .max_by(|left, right| {
            left.problem_score
                .cmp(&right.problem_score)
                .then_with(|| compare_sources(&right.source, &left.source))
        });
    if let Some(pinned) = pinned {
        selected.push(SelectedAutomaticRoot {
            source: pinned.source.clone(),
            lane: AutomaticRootLane::ProblemNeutral,
            relevance_score: pinned.problem_score,
            selection_priority: pinned.problem_score,
        });
    }

    while selected.len() < neutral_reserved && selected.len() < limit {
        let next = best_candidate(&candidates, &selected, AutomaticRootLane::ProblemNeutral);
        let Some(next) = next else {
            break;
        };
        selected.push(next);
    }

    while selected.len() < limit {
        let next = best_candidate(&candidates, &selected, AutomaticRootLane::Mixed);
        let Some(next) = next else {
            break;
        };
        selected.push(next);
    }
    Ok(selected)
}

fn best_candidate(
    candidates: &[RootSelectionCandidate],
    selected: &[SelectedAutomaticRoot],
    lane: AutomaticRootLane,
) -> Option<SelectedAutomaticRoot> {
    candidates
        .iter()
        .filter(|candidate| {
            !selected.iter().any(|item| item.source == candidate.source)
                && (lane == AutomaticRootLane::Mixed || candidate.discovered_problem_neutral)
        })
        .map(|candidate| {
            let relevance = match lane {
                AutomaticRootLane::ProblemNeutral => candidate.problem_score,
                AutomaticRootLane::Mixed => candidate.candidate_score,
            };
            let redundancy = selected
                .iter()
                .filter_map(|item| {
                    candidate
                        .redundancy_to
                        .iter()
                        .find(|pair| pair.other_source == item.source)
                        .map(|pair| pair.similarity)
                })
                .max()
                .map_or(Score::ZERO, |score| score);
            SelectedAutomaticRoot {
                source: candidate.source.clone(),
                lane,
                relevance_score: relevance,
                selection_priority: root_diversity_priority(relevance, redundancy),
            }
        })
        .max_by(|left, right| {
            left.selection_priority
                .cmp(&right.selection_priority)
                .then_with(|| left.relevance_score.cmp(&right.relevance_score))
                .then_with(|| compare_sources(&right.source, &left.source))
        })
}

fn compare_sources(left: &SemanticSourceIdentity, right: &SemanticSourceIdentity) -> Ordering {
    left.community_id
        .as_bytes()
        .cmp(right.community_id.as_bytes())
        .then_with(|| source_kind_rank(left.kind).cmp(&source_kind_rank(right.kind)))
        .then_with(|| left.source_id.as_bytes().cmp(right.source_id.as_bytes()))
}

const fn source_kind_rank(kind: SemanticSourceKind) -> (u8, u8) {
    match kind {
        SemanticSourceKind::ProjectView(subtype) => (0, project_view_kind_rank(subtype)),
        SemanticSourceKind::ProjectDocument => (1, 0),
        SemanticSourceKind::Meeting => (2, 0),
    }
}

const fn project_view_kind_rank(kind: ProjectViewSemanticType) -> u8 {
    match kind {
        ProjectViewSemanticType::ProjectProfile => 0,
        ProjectViewSemanticType::Goal => 1,
        ProjectViewSemanticType::Role => 2,
        ProjectViewSemanticType::Plan => 3,
        ProjectViewSemanticType::Stage => 4,
        ProjectViewSemanticType::Requirement => 5,
        ProjectViewSemanticType::Issue => 6,
        ProjectViewSemanticType::Work => 7,
        ProjectViewSemanticType::Resource => 8,
    }
}

#[cfg(test)]
mod tests {
    use buzz_semantic::{ProjectViewSemanticType, SemanticSourceIdentity, SemanticSourceKind};
    use uuid::Uuid;

    use super::{select_automatic_roots, RootPairRedundancy, RootSelectionCandidate};
    use crate::Score;

    fn score(value: u32) -> Score {
        Score::new(value).expect("score")
    }

    fn source(value: u128) -> SemanticSourceIdentity {
        SemanticSourceIdentity {
            community_id: Uuid::from_u128(1),
            kind: SemanticSourceKind::ProjectView(ProjectViewSemanticType::Work),
            source_id: Uuid::from_u128(value),
        }
    }

    fn candidate(value: u128, problem: u32, mixed: u32, neutral: bool) -> RootSelectionCandidate {
        RootSelectionCandidate {
            source: source(value),
            problem_score: score(problem),
            candidate_score: score(mixed),
            discovered_problem_neutral: neutral,
            redundancy_to: Vec::<RootPairRedundancy>::new(),
        }
    }

    #[test]
    fn selection_is_permutation_invariant_and_pins_strongest_q0() {
        let first = vec![
            candidate(3, 600_000, 900_000, false),
            candidate(2, 800_000, 800_000, true),
            candidate(1, 900_000, 700_000, true),
        ];
        let mut second = first.clone();
        second.reverse();
        let selected_first = select_automatic_roots(&first, 3).expect("roots");
        let selected_second = select_automatic_roots(&second, 3).expect("roots");
        assert_eq!(selected_first, selected_second);
        assert_eq!(selected_first[0].source, source(1));
        assert_eq!(selected_first[1].source, source(2));
        assert_eq!(selected_first[2].source, source(3));
    }
}
