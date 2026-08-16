use std::collections::BTreeSet;

use buzz_project_context::ProjectContextCoordinate;
use buzz_semantic::Digest32;
use uuid::Uuid;

use crate::coordinate_search::coordinate_search_query_text_digest;
use crate::query_text::query_text_digest;
use crate::{
    coordinate_search_query_contract_digest, query_contract_digest,
    MAX_COORDINATE_SEARCH_PROVIDER_INPUT_BYTES, MAX_PROVIDER_QUERY_INPUT_BYTES, MAX_QUERY_CHANNELS,
};

/// Result alias for the common closed semantic-input contract.
pub type SemanticInputResult<T> = Result<T, SemanticInputError>;

/// Failures produced while validating an internal semantic input or bundle.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SemanticInputError {
    /// An input carries a serializer/template digest not approved for its kind.
    #[error("semantic input encoding contract mismatch")]
    EncodingContractMismatch,
    /// The exact Provider bytes do not match their domain-separated digest.
    #[error("semantic input digest mismatch")]
    InputDigestMismatch,
    /// One input exceeds its existing Provider boundary.
    #[error("semantic input exceeds its Provider byte boundary")]
    InputTooLarge,
    /// An ordered bundle violates its closed shape.
    #[error("invalid semantic input bundle: {0}")]
    InvalidBundle(&'static str),
}

/// Closed identity of one semantic Provider input.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum SemanticQueryInputKind {
    /// Whole-graph starting-Coordinate discovery input.
    CoordinateSearch,
    /// Problem-only Q0 input shared by one-hop and complete-path operations.
    Problem,
    /// One problem plus one current Coordinate overview Qi input.
    ConditionedContext {
        /// Coordinate whose current overview conditions the branch.
        context_coordinate: ProjectContextCoordinate,
    },
}

impl std::fmt::Debug for SemanticQueryInputKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::CoordinateSearch => "CoordinateSearch",
            Self::Problem => "Problem",
            Self::ConditionedContext { .. } => "ConditionedContext(<redacted-coordinate>)",
        })
    }
}

/// One immutable, exact-byte semantic Provider input produced by a closed builder.
#[derive(Clone, PartialEq, Eq)]
pub struct SemanticQueryInput {
    request_id: Uuid,
    channel_id: Digest32,
    channel_kind: SemanticQueryInputKind,
    encoding_contract_digest: Digest32,
    input_digest: Digest32,
    exact_utf8_text: String,
}

impl std::fmt::Debug for SemanticQueryInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SemanticQueryInput")
            .field("channel_kind", &self.channel_kind)
            .field("exact_utf8_text", &"<redacted>")
            .field("exact_utf8_text_bytes", &self.exact_utf8_text.len())
            .finish_non_exhaustive()
    }
}

impl SemanticQueryInput {
    pub(crate) fn new_closed(
        request_id: Uuid,
        channel_id: Digest32,
        channel_kind: SemanticQueryInputKind,
        encoding_contract_digest: Digest32,
        input_digest: Digest32,
        exact_utf8_text: String,
    ) -> SemanticInputResult<Self> {
        let input = Self {
            request_id,
            channel_id,
            channel_kind,
            encoding_contract_digest,
            input_digest,
            exact_utf8_text,
        };
        input.validate()?;
        Ok(input)
    }

    /// Request identity that owns this ephemeral input.
    pub const fn request_id(&self) -> Uuid {
        self.request_id
    }

    /// Stable request-local input branch identity.
    pub const fn channel_id(&self) -> Digest32 {
        self.channel_id
    }

    /// Closed Coordinate, Q0, or Qi input identity.
    pub const fn channel_kind(&self) -> &SemanticQueryInputKind {
        &self.channel_kind
    }

    /// Digest of the exact serializer/template contract used by its builder.
    pub const fn encoding_contract_digest(&self) -> Digest32 {
        self.encoding_contract_digest
    }

    /// Domain-separated digest of the exact Provider bytes.
    pub const fn input_digest(&self) -> Digest32 {
        self.input_digest
    }

    /// Exact UTF-8 bytes sent to the Provider.
    pub fn exact_utf8_text(&self) -> &str {
        &self.exact_utf8_text
    }

    /// Revalidate the immutable closed-builder bindings.
    pub fn validate(&self) -> SemanticInputResult<()> {
        let (expected_contract, expected_input, maximum_bytes) = match self.channel_kind {
            SemanticQueryInputKind::CoordinateSearch => (
                coordinate_search_query_contract_digest(),
                coordinate_search_query_text_digest(self.exact_utf8_text.as_bytes()),
                MAX_COORDINATE_SEARCH_PROVIDER_INPUT_BYTES,
            ),
            SemanticQueryInputKind::Problem | SemanticQueryInputKind::ConditionedContext { .. } => {
                (
                    query_contract_digest(),
                    query_text_digest(self.exact_utf8_text.as_bytes()),
                    MAX_PROVIDER_QUERY_INPUT_BYTES,
                )
            }
        };
        if self.encoding_contract_digest != expected_contract {
            return Err(SemanticInputError::EncodingContractMismatch);
        }
        if self.input_digest != expected_input {
            return Err(SemanticInputError::InputDigestMismatch);
        }
        if self.exact_utf8_text.len() > maximum_bytes {
            return Err(SemanticInputError::InputTooLarge);
        }
        Ok(())
    }
}

/// One bounded ordered input batch belonging to a single logical request.
#[derive(Clone, PartialEq, Eq)]
pub struct SemanticQueryInputBundle {
    inputs: Vec<SemanticQueryInput>,
}

impl std::fmt::Debug for SemanticQueryInputBundle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SemanticQueryInputBundle")
            .field("input_count", &self.inputs.len())
            .finish_non_exhaustive()
    }
}

impl SemanticQueryInputBundle {
    /// Validate one closed ordered input set without combining requests.
    pub fn from_closed_inputs(inputs: Vec<SemanticQueryInput>) -> SemanticInputResult<Self> {
        let bundle = Self { inputs };
        bundle.validate()?;
        Ok(bundle)
    }

    /// Ordered Coordinate/Q0/Qi inputs.
    pub fn inputs(&self) -> &[SemanticQueryInput] {
        &self.inputs
    }

    /// Number of inputs in this one Provider batch.
    pub fn len(&self) -> usize {
        self.inputs.len()
    }

    /// Whether this bundle contains no input.
    pub fn is_empty(&self) -> bool {
        self.inputs.is_empty()
    }

    /// Revalidate request identity, channel uniqueness, and closed ordering.
    pub fn validate(&self) -> SemanticInputResult<()> {
        if self.inputs.is_empty() || self.inputs.len() > MAX_QUERY_CHANNELS {
            return Err(SemanticInputError::InvalidBundle(
                "input count is outside the closed bound",
            ));
        }
        let request_id = self.inputs[0].request_id;
        let mut channel_ids = BTreeSet::new();
        for input in &self.inputs {
            input.validate()?;
            if input.request_id != request_id {
                return Err(SemanticInputError::InvalidBundle(
                    "inputs cross request identities",
                ));
            }
            if !channel_ids.insert(input.channel_id) {
                return Err(SemanticInputError::InvalidBundle(
                    "inputs repeat a channel identity",
                ));
            }
        }

        match &self.inputs[0].channel_kind {
            SemanticQueryInputKind::CoordinateSearch => {
                if self.inputs.len() != 1 {
                    return Err(SemanticInputError::InvalidBundle(
                        "Coordinate search requires exactly one input",
                    ));
                }
            }
            SemanticQueryInputKind::Problem => {
                let mut previous_context: Option<&ProjectContextCoordinate> = None;
                for input in self.inputs.iter().skip(1) {
                    let SemanticQueryInputKind::ConditionedContext { context_coordinate } =
                        &input.channel_kind
                    else {
                        return Err(SemanticInputError::InvalidBundle(
                            "Q0 may only be followed by conditioned Qi inputs",
                        ));
                    };
                    if previous_context.is_some_and(|previous| previous >= context_coordinate) {
                        return Err(SemanticInputError::InvalidBundle(
                            "conditioned Coordinates are not canonical and unique",
                        ));
                    }
                    previous_context = Some(context_coordinate);
                }
            }
            SemanticQueryInputKind::ConditionedContext { .. } => {
                return Err(SemanticInputError::InvalidBundle(
                    "a graph input bundle must begin with Q0",
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use buzz_project_context::ProjectContextCoordinate;
    use buzz_project_view::ProjectViewObjectType;
    use uuid::Uuid;

    use super::{
        SemanticInputError, SemanticQueryInput, SemanticQueryInputBundle, SemanticQueryInputKind,
    };
    use crate::{
        build_coordinate_search_encoder_input, build_problem_query_encoder_input,
        build_query_encoder_inputs, ConditionedContextOverview, LifecycleFilter,
        ProjectContextCoordinateSearchQuery, SemanticGraphQuery, SemanticGraphQueryBudget,
        DEFAULT_COORDINATE_SEARCH_LIMIT,
    };

    fn uuid(value: u128) -> Uuid {
        Uuid::from_u128(0x123e_4567_e89b_42d3_a456_4266_1000_0000 | value)
    }

    fn work(value: u128) -> ProjectContextCoordinate {
        ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Work,
            object_id: uuid(value),
        }
    }

    fn graph_bundle(request_id: Uuid) -> SemanticQueryInputBundle {
        let query = SemanticGraphQuery {
            request_id,
            project_id: uuid(2),
            problem: "authorization context".to_owned(),
            initial_coordinates: Vec::new(),
            context_coordinates: vec![work(3), work(4)],
            lifecycle_filter: LifecycleFilter::AllCurrent,
            budget: SemanticGraphQueryBudget::default(),
        };
        build_query_encoder_inputs(
            &query,
            &[
                ConditionedContextOverview {
                    coordinate: work(4),
                    current_overview_semantic_text: "four".to_owned(),
                },
                ConditionedContextOverview {
                    coordinate: work(3),
                    current_overview_semantic_text: "three".to_owned(),
                },
            ],
        )
        .expect("graph inputs")
        .semantic_input_bundle()
        .expect("graph bundle")
    }

    #[test]
    fn closed_adapters_build_single_coordinate_and_ordered_q0_qi_bundles() {
        let coordinate =
            build_coordinate_search_encoder_input(&ProjectContextCoordinateSearchQuery {
                request_id: uuid(1),
                project_id: uuid(2),
                query: "authorization context".to_owned(),
                limit: DEFAULT_COORDINATE_SEARCH_LIMIT,
            })
            .expect("Coordinate input")
            .semantic_input_bundle()
            .expect("Coordinate bundle");
        assert_eq!(coordinate.len(), 1);
        assert!(matches!(
            coordinate.inputs()[0].channel_kind(),
            SemanticQueryInputKind::CoordinateSearch
        ));

        let graph = graph_bundle(uuid(5));
        assert_eq!(graph.len(), 3);
        assert!(matches!(
            graph.inputs()[0].channel_kind(),
            SemanticQueryInputKind::Problem
        ));
        assert!(matches!(
            graph.inputs()[1].channel_kind(),
            SemanticQueryInputKind::ConditionedContext { context_coordinate }
                if context_coordinate == &work(3)
        ));
    }

    #[test]
    fn bundle_rejects_cross_request_duplicate_and_noncanonical_channels() {
        let first = build_problem_query_encoder_input(uuid(6), "first").expect("first");
        let second = build_problem_query_encoder_input(uuid(7), "second").expect("second");
        assert!(matches!(
            SemanticQueryInputBundle::from_closed_inputs(vec![
                first.semantic_input().clone(),
                second.semantic_input().clone()
            ]),
            Err(SemanticInputError::InvalidBundle(_))
        ));

        let graph = graph_bundle(uuid(8));
        let mut duplicate = graph.inputs().to_vec();
        duplicate.push(duplicate[2].clone());
        assert!(SemanticQueryInputBundle::from_closed_inputs(duplicate).is_err());

        let mut reversed = graph.inputs().to_vec();
        reversed.swap(1, 2);
        assert!(SemanticQueryInputBundle::from_closed_inputs(reversed).is_err());
    }

    #[test]
    fn input_tampering_and_debug_are_content_free() {
        let mut input = graph_bundle(uuid(9)).inputs()[0].clone();
        input.input_digest = buzz_semantic::Digest32::from_bytes([0xFF; 32]);
        assert_eq!(
            input.validate(),
            Err(SemanticInputError::InputDigestMismatch)
        );

        let debug = format!("{:?}", graph_bundle(uuid(10)));
        assert!(!debug.contains("authorization"));
        assert!(!debug.contains(&uuid(10).to_string()));
    }

    #[test]
    fn common_input_fields_cannot_be_reinterpreted_as_another_contract() {
        let graph =
            build_problem_query_encoder_input(uuid(11), "same language").expect("problem input");
        let mut reinterpreted: SemanticQueryInput = graph.semantic_input().clone();
        reinterpreted.channel_kind = SemanticQueryInputKind::CoordinateSearch;
        assert_eq!(
            reinterpreted.validate(),
            Err(SemanticInputError::EncodingContractMismatch)
        );
    }
}
