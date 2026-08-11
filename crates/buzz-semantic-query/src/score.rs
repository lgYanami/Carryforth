use std::fmt;

use buzz_project_context::ProjectContextCoordinate;
use buzz_project_view::ProjectViewObjectType;
use buzz_semantic::Digest32;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest as _, Sha256};

use crate::{QueryContractResult, SemanticGraphQueryError};

/// Integer scale shared by every semantic graph score.
pub const SCORE_SCALE: u32 = 1_000_000;

const W_CANDIDATE_PROBLEM: Score = Score(750_000);
const W_CANDIDATE_ENVIRONMENT: Score = Score(200_000);
const W_CANDIDATE_ANCHOR: Score = Score(50_000);
const W_DOCUMENT_PROBLEM: Score = Score(700_000);
const W_DOCUMENT_ENVIRONMENT: Score = Score(200_000);
const W_DOCUMENT_COHERENCE: Score = Score(100_000);
const W_SECOND_ENVIRONMENT: Score = Score(250_000);
const W_ROOT_RELEVANCE: Score = Score(850_000);
const W_ROOT_DIVERSITY: Score = Score(150_000);
/// Per-hop integer discount, applied recursively rather than with `powf`.
pub const DISCOUNT_FACTOR: Score = Score(850_000);
/// Provisional absolute problem relevance floor for automatic roots.
pub const BASE_ENTRY_FLOOR: Score = Score(550_000);
/// Provisional relation-document score floor.
pub const RELATION_FLOOR: Score = Score(500_000);
/// Provisional continued-target score floor.
pub const TARGET_FLOOR: Score = Score(500_000);
/// Provisional complete-transition score floor.
pub const TRANSITION_FLOOR: Score = Score(500_000);
/// Frozen per-hop path penalty in fixed-point units.
pub const HOP_PENALTY: Score = Score(25_000);

/// Digest the complete current ranking formula, term-level rounding, weights,
/// provisional floors, root MMR, path discount, and canonical tie-break rules.
pub fn ranking_contract_digest() -> QueryContractResult<Digest32> {
    let canonical = postcard::to_stdvec(&(
        "semantic-graph-score",
        SCORE_SCALE,
        (
            W_CANDIDATE_PROBLEM.raw(),
            W_CANDIDATE_ENVIRONMENT.raw(),
            W_CANDIDATE_ANCHOR.raw(),
            W_DOCUMENT_PROBLEM.raw(),
            W_DOCUMENT_ENVIRONMENT.raw(),
            W_DOCUMENT_COHERENCE.raw(),
            W_SECOND_ENVIRONMENT.raw(),
            W_ROOT_RELEVANCE.raw(),
            W_ROOT_DIVERSITY.raw(),
            DISCOUNT_FACTOR.raw(),
        ),
        (
            HOP_PENALTY.raw(),
            BASE_ENTRY_FLOOR.raw(),
            RELATION_FLOOR.raw(),
            TARGET_FLOOR.raw(),
            TRANSITION_FLOOR.raw(),
        ),
        [1_000_000_u32, 900_000, 900_000, 600_000, 500_000],
        (
            "round=term-mul-half-up;sum-saturate;db-floor-half-up",
            "environment=highest-plus-quarter-second-canonical-coordinate-dedup",
            "document-missing-local=round-once-seven-problem-plus-two-environment-over-nine",
            "transition=zero-absorbing-harmonic",
            "path=renormalized-recursive-discount-minus-hop-penalty",
            "root-neutral=ceil-half-pin-highest-problem-then-mmr",
            "root-mixed=mmr-candidate-score",
            "tie=source-identity-edge-key-document-id-coordinate-provenance",
            "frontier=fair-first-wave-then-global-best-first-per-state-beam",
        ),
    ))
    .map_err(|_| SemanticGraphQueryError::Serialization)?;
    let mut hasher = Sha256::new();
    let domain = b"buzz.semantic-graph-ranking";
    hasher.update((domain.len() as u64).to_be_bytes());
    hasher.update(domain);
    hasher.update((canonical.len() as u64).to_be_bytes());
    hasher.update(canonical);
    Ok(Digest32::from_bytes(hasher.finalize().into()))
}

/// Validated fixed-point value in the inclusive range `0..=1_000_000`.
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Score(u32);

impl Score {
    /// Additive zero.
    pub const ZERO: Self = Self(0);
    /// Fixed-point representation of one.
    pub const ONE: Self = Self(SCORE_SCALE);

    /// Validate one raw fixed-point value.
    pub const fn new(value: u32) -> Result<Self, ScoreError> {
        if value > SCORE_SCALE {
            return Err(ScoreError::OutOfRange { value });
        }
        Ok(Self(value))
    }

    /// Return the raw fixed-point integer.
    pub const fn raw(self) -> u32 {
        self.0
    }

    /// Quantize a finite pgvector cosine distance using the frozen
    /// clamp-and-floor-half-up boundary expression.
    pub fn from_cosine_distance(distance: f64) -> Result<Self, ScoreError> {
        if !distance.is_finite() {
            return Err(ScoreError::NonFiniteDistance);
        }
        let similarity = (1.0 - distance).clamp(-1.0, 1.0);
        let normalized = ((similarity + 1.0) / 2.0).clamp(0.0, 1.0);
        let value = (normalized * f64::from(SCORE_SCALE) + 0.5).floor() as u32;
        Self::new(value)
    }

    /// Saturating fixed-point complement `1 - self`.
    pub const fn complement(self) -> Self {
        Self(SCORE_SCALE - self.0)
    }
}

impl fmt::Debug for Score {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Score").field(&self.0).finish()
    }
}

impl Serialize for Score {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u32(self.0)
    }
}

impl<'de> Deserialize<'de> for Score {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u32::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Errors from deterministic score validation or checked arithmetic.
#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq)]
pub enum ScoreError {
    /// Raw fixed-point value exceeds one.
    #[error("score {value} exceeds {SCORE_SCALE}")]
    OutOfRange {
        /// Rejected raw value.
        value: u32,
    },
    /// DB cosine distance is NaN or infinity.
    #[error("cosine distance must be finite")]
    NonFiniteDistance,
    /// A supposedly bounded integer operation overflowed.
    #[error("fixed-point score arithmetic overflow")]
    ArithmeticOverflow,
}

/// Multiply two fixed-point scores with half-up division after this term.
pub fn mul_score(left: Score, right: Score) -> Score {
    let numerator = u128::from(left.0) * u128::from(right.0);
    Score(round_div(numerator, u128::from(SCORE_SCALE)) as u32)
}

/// Sum individually rounded weighted terms and clamp the result to one.
pub fn weighted_score(terms: &[(Score, Score)]) -> Score {
    let sum = terms
        .iter()
        .map(|(weight, score)| u64::from(mul_score(*weight, *score).0))
        .sum::<u64>();
    Score(sum.min(u64::from(SCORE_SCALE)) as u32)
}

/// Harmonic mean of two non-negative fixed-point scores, with zero absorbing.
pub fn harmonic_score(left: Score, right: Score) -> Score {
    if left == Score::ZERO || right == Score::ZERO {
        return Score::ZERO;
    }
    let numerator = 2_u128 * u128::from(left.0) * u128::from(right.0);
    let denominator = u128::from(left.0) + u128::from(right.0);
    Score(round_div(numerator, denominator) as u32)
}

/// Closed anchor contribution before the candidate weight is applied.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorGain {
    /// No explicit-initial relationship.
    #[default]
    None,
    /// Candidate shares a current exact Hyperedge with an initial Coordinate.
    SameHyperedge,
    /// Candidate is itself an explicit initial Coordinate.
    ExplicitInitial,
}

impl AnchorGain {
    /// Convert the closed anchor state into its fixed score.
    pub const fn score(self) -> Score {
        match self {
            Self::None => Score::ZERO,
            Self::SameHyperedge => Score(500_000),
            Self::ExplicitInitial => Score::ONE,
        }
    }
}

/// Independently attributable conditioned evidence for one candidate source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConditionedEvidence {
    /// Context Coordinate that produced this independently encoded Qi branch.
    pub context_coordinate: ProjectContextCoordinate,
    /// Closed kind weight for that Coordinate.
    pub context_kind_weight: Score,
    /// Candidate similarity to this Qi vector.
    pub conditioned_score: Score,
    /// Positive gain over the same candidate's Q0 score.
    pub raw_gain: Score,
    /// Individually rounded kind-weighted gain.
    pub weighted_gain: Score,
}

impl ConditionedEvidence {
    /// Build an internally consistent evidence item.
    pub fn new(
        context_coordinate: ProjectContextCoordinate,
        problem_score: Score,
        conditioned_score: Score,
    ) -> Self {
        let context_kind_weight = context_kind_weight(&context_coordinate);
        let raw_gain = Score(conditioned_score.0.saturating_sub(problem_score.0));
        let weighted_gain = mul_score(context_kind_weight, raw_gain);
        Self {
            context_coordinate,
            context_kind_weight,
            conditioned_score,
            raw_gain,
            weighted_gain,
        }
    }
}

/// Exact evidence and top-two aggregation used to compute environment gain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentScoreExplanation {
    /// Canonically deduplicated evidence sorted by gain descending and then
    /// Coordinate order ascending.
    pub conditioned_evidence: Vec<ConditionedEvidence>,
    /// Strongest independent weighted gain.
    pub highest_gain: Score,
    /// Second-strongest independent weighted gain.
    pub second_highest_gain: Score,
    /// Saturated strongest plus one-quarter second-strongest gain.
    pub environment_gain: Score,
}

/// Deduplicate evidence by Coordinate, retain its strongest gain, and compute
/// the frozen top-two bounded environment contribution.
pub fn environment_gain(evidence: &[ConditionedEvidence]) -> EnvironmentScoreExplanation {
    let mut evidence = evidence.to_vec();
    evidence.sort_by(|left, right| {
        left.context_coordinate
            .cmp(&right.context_coordinate)
            .then_with(|| right.weighted_gain.cmp(&left.weighted_gain))
            .then_with(|| right.conditioned_score.cmp(&left.conditioned_score))
    });
    evidence.dedup_by(|right, left| right.context_coordinate == left.context_coordinate);
    evidence.sort_by(|left, right| {
        right
            .weighted_gain
            .cmp(&left.weighted_gain)
            .then_with(|| left.context_coordinate.cmp(&right.context_coordinate))
    });

    let highest_gain = evidence
        .first()
        .map_or(Score::ZERO, |item| item.weighted_gain);
    let second_highest_gain = evidence
        .get(1)
        .map_or(Score::ZERO, |item| item.weighted_gain);
    let environment_gain = Score(
        u64::from(highest_gain.0)
            .saturating_add(u64::from(
                mul_score(W_SECOND_ENVIRONMENT, second_highest_gain).0,
            ))
            .min(u64::from(SCORE_SCALE)) as u32,
    );
    EnvironmentScoreExplanation {
        conditioned_evidence: evidence,
        highest_gain,
        second_highest_gain,
        environment_gain,
    }
}

/// Return the closed environment weight of one context Coordinate kind.
pub const fn context_kind_weight(coordinate: &ProjectContextCoordinate) -> Score {
    match coordinate {
        ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Work,
            ..
        } => Score::ONE,
        ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Issue | ProjectViewObjectType::Requirement,
            ..
        } => Score(900_000),
        ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Role,
            ..
        } => Score(600_000),
        _ => Score(500_000),
    }
}

/// Compute the problem-dominant automatic candidate score.
pub fn candidate_score(problem: Score, environment: Score, anchor: AnchorGain) -> Score {
    weighted_score(&[
        (W_CANDIDATE_PROBLEM, problem),
        (W_CANDIDATE_ENVIRONMENT, environment),
        (W_CANDIDATE_ANCHOR, anchor.score()),
    ])
}

/// Compute a relation Document score. `None` coherence is the explicit-root
/// `round_div(7P + 2E, 9)` rational renormalization exception.
pub fn document_score(problem: Score, environment: Score, local_coherence: Option<Score>) -> Score {
    match local_coherence {
        Some(coherence) => weighted_score(&[
            (W_DOCUMENT_PROBLEM, problem),
            (W_DOCUMENT_ENVIRONMENT, environment),
            (W_DOCUMENT_COHERENCE, coherence),
        ]),
        None => {
            let numerator = 7_u128 * u128::from(problem.0) + 2_u128 * u128::from(environment.0);
            Score(round_div(numerator, 9) as u32)
        }
    }
}

/// Compute a target Coordinate score using relation-document coherence.
pub fn target_coordinate_score(
    problem: Score,
    environment: Score,
    relation_document_coherence: Score,
) -> Score {
    weighted_score(&[
        (W_DOCUMENT_PROBLEM, problem),
        (W_DOCUMENT_ENVIRONMENT, environment),
        (W_DOCUMENT_COHERENCE, relation_document_coherence),
    ])
}

/// Compute the frozen root-MMR priority from relevance and maximum redundancy.
pub fn root_diversity_priority(relevance: Score, redundancy: Score) -> Score {
    weighted_score(&[
        (W_ROOT_RELEVANCE, relevance),
        (W_ROOT_DIVERSITY, redundancy.complement()),
    ])
}

/// Exact fixed-point path score components.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathScoreExplanation {
    /// Root relevance, absent only for an embedding-less explicit initial.
    pub root_score: Option<Score>,
    /// Complete transition scores in hop order.
    pub transition_scores: Vec<Score>,
    /// Recursively rounded discount weights corresponding to transitions.
    pub discount_weights: Vec<Score>,
    /// Renormalized weighted quality, absent when no scored component exists.
    pub weighted_path_quality: Option<Score>,
    /// Total capped hop penalty.
    pub hop_penalty: Score,
    /// Final path score, absent when no scored component exists.
    pub final_score: Option<Score>,
}

/// Compute a length-neutral, recursively discounted path score and exact
/// explanation. A missing root is omitted from both numerator and denominator.
pub fn path_score(
    root_score: Option<Score>,
    transition_scores: &[Score],
) -> Result<PathScoreExplanation, ScoreError> {
    let mut discount_weights = Vec::with_capacity(transition_scores.len());
    let mut weight = Score::ONE;
    for _ in transition_scores {
        discount_weights.push(weight);
        weight = mul_score(weight, DISCOUNT_FACTOR);
    }

    let mut numerator = 0_u128;
    let mut denominator = 0_u128;
    if let Some(root) = root_score {
        numerator = numerator
            .checked_add(u128::from(root.0) * u128::from(SCORE_SCALE))
            .ok_or(ScoreError::ArithmeticOverflow)?;
        denominator = denominator
            .checked_add(u128::from(SCORE_SCALE))
            .ok_or(ScoreError::ArithmeticOverflow)?;
    }
    for (transition, discount) in transition_scores.iter().zip(&discount_weights) {
        numerator = numerator
            .checked_add(u128::from(transition.0) * u128::from(discount.0))
            .ok_or(ScoreError::ArithmeticOverflow)?;
        denominator = denominator
            .checked_add(u128::from(discount.0))
            .ok_or(ScoreError::ArithmeticOverflow)?;
    }

    let weighted_path_quality = if denominator == 0 {
        None
    } else {
        Some(Score(round_div(numerator, denominator) as u32))
    };
    let hop_count =
        u128::try_from(transition_scores.len()).map_err(|_| ScoreError::ArithmeticOverflow)?;
    let penalty = hop_count
        .checked_mul(u128::from(HOP_PENALTY.0))
        .ok_or(ScoreError::ArithmeticOverflow)?
        .min(u128::from(SCORE_SCALE));
    let hop_penalty = Score(penalty as u32);
    let final_score =
        weighted_path_quality.map(|quality| Score(quality.0.saturating_sub(hop_penalty.0)));
    Ok(PathScoreExplanation {
        root_score,
        transition_scores: transition_scores.to_vec(),
        discount_weights,
        weighted_path_quality,
        hop_penalty,
        final_score,
    })
}

fn round_div(numerator: u128, denominator: u128) -> u128 {
    (numerator + denominator / 2) / denominator
}

#[cfg(test)]
mod tests {
    use buzz_project_context::ProjectContextCoordinate;
    use buzz_project_view::ProjectViewObjectType;
    use proptest::prelude::*;
    use uuid::Uuid;

    use super::{
        candidate_score, document_score, environment_gain, harmonic_score, path_score, AnchorGain,
        ConditionedEvidence, Score, BASE_ENTRY_FLOOR, SCORE_SCALE,
    };

    fn score(value: u32) -> Score {
        Score::new(value).expect("score fixture")
    }

    fn work(value: u128) -> ProjectContextCoordinate {
        ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Work,
            object_id: Uuid::from_u128(value),
        }
    }

    #[test]
    fn distance_quantization_clamps_and_rounds_half_up() {
        assert_eq!(Score::from_cosine_distance(-1.0), Ok(Score::ONE));
        assert_eq!(Score::from_cosine_distance(0.0), Ok(Score::ONE));
        assert_eq!(Score::from_cosine_distance(1.0), Ok(score(500_000)));
        assert_eq!(Score::from_cosine_distance(2.0), Ok(Score::ZERO));
        assert_eq!(Score::from_cosine_distance(-2.0), Ok(Score::ONE));
        assert!(Score::from_cosine_distance(f64::NAN).is_err());
    }

    #[test]
    fn top_two_environment_gain_is_bounded_and_deduplicated() {
        let problem = score(400_000);
        let evidence = vec![
            ConditionedEvidence::new(work(1), problem, score(700_000)),
            ConditionedEvidence::new(work(2), problem, score(580_000)),
            ConditionedEvidence::new(work(3), problem, score(450_000)),
            ConditionedEvidence::new(work(1), problem, score(650_000)),
        ];
        let explanation = environment_gain(&evidence);
        assert_eq!(explanation.highest_gain, score(300_000));
        assert_eq!(explanation.second_highest_gain, score(180_000));
        assert_eq!(explanation.environment_gain, score(345_000));
        assert_eq!(explanation.conditioned_evidence.len(), 3);
    }

    #[test]
    fn harmonic_mean_and_rational_missing_local_match_goldens() {
        assert_eq!(
            harmonic_score(score(800_000), score(800_000)),
            score(800_000)
        );
        assert_eq!(
            harmonic_score(score(900_000), score(200_000)),
            score(327_273)
        );
        assert_eq!(harmonic_score(score(900_000), Score::ZERO), Score::ZERO);
        assert_eq!(
            document_score(score(700_001), score(300_002), None),
            score(611_112)
        );
    }

    #[test]
    fn missing_root_is_renormalized_not_treated_as_zero() {
        let explanation = path_score(None, &[score(800_000), score(600_000)]).expect("path score");
        assert_eq!(
            explanation.discount_weights,
            vec![Score::ONE, score(850_000)]
        );
        assert_eq!(explanation.weighted_path_quality, Some(score(708_108)));
        assert_eq!(explanation.hop_penalty, score(50_000));
        assert_eq!(explanation.final_score, Some(score(658_108)));
    }

    #[test]
    fn provisional_floor_is_a_valid_score() {
        assert!(BASE_ENTRY_FLOOR.raw() <= SCORE_SCALE);
    }

    proptest! {
        #[test]
        fn problem_weight_dominates_all_environment_and_anchor_contributions(
            low in 0_u32..=SCORE_SCALE,
            high in 0_u32..=SCORE_SCALE,
            environment in 0_u32..=SCORE_SCALE,
        ) {
            let (low, high) = if low <= high { (low, high) } else { (high, low) };
            let low_candidate = candidate_score(
                score(low),
                score(environment),
                AnchorGain::ExplicitInitial,
            );
            let high_candidate = candidate_score(
                score(high),
                Score::ZERO,
                AnchorGain::None,
            );
            if high.saturating_sub(low) >= 334_000 {
                prop_assert!(high_candidate > low_candidate);
            }
        }

        #[test]
        fn harmonic_is_symmetric_zero_absorbing_and_not_above_arithmetic_mean(
            left in 0_u32..=SCORE_SCALE,
            right in 0_u32..=SCORE_SCALE,
        ) {
            let left = score(left);
            let right = score(right);
            let harmonic = harmonic_score(left, right);
            prop_assert_eq!(harmonic, harmonic_score(right, left));
            if left == Score::ZERO || right == Score::ZERO {
                prop_assert_eq!(harmonic, Score::ZERO);
            }
            let arithmetic =
                (u64::from(left.raw()) + u64::from(right.raw())).div_ceil(2);
            prop_assert!(u64::from(harmonic.raw()) <= arithmetic);
        }
    }
}
