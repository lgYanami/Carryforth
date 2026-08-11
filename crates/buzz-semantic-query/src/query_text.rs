use buzz_project_context::ProjectContextCoordinate;
use buzz_semantic::Digest32;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::{
    QueryContractResult, SemanticGraphQuery, SemanticGraphQueryError, MAX_PROBLEM_BYTES,
    MAX_PROVIDER_QUERY_INPUT_BYTES,
};

/// Fixed canonical serializer contract identifier.
pub const QUERY_SERIALIZER_CONTRACT: &str = "semantic-graph-query-json";
/// Fixed problem-only Provider input contract identifier.
pub const PROBLEM_CONTRACT: &str = "semantic-graph-query.problem";
/// Fixed one-Coordinate-conditioned Provider input contract identifier.
pub const CONDITIONED_CONTEXT_CONTRACT: &str = "semantic-graph-query.conditioned-context";

const QUERY_CONTRACT_DESCRIPTOR: &str = concat!(
    "serializer=semantic-graph-query-json\n",
    "problem=semantic-graph-query.problem\n",
    "conditioned=semantic-graph-query.conditioned-context\n",
    "field-order=contract,problem,context_overview\n",
    "escape=quote-backslash-short-c0-other-c0-lower-hex\n",
    "unicode=raw-utf8-no-normalization\n",
    "max-provider-input-bytes=65536"
);

/// Closed identity of one independently encoded query-vector branch.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SemanticQueryChannelKind {
    /// The problem-only Q0 branch.
    Problem,
    /// One problem plus one current Coordinate overview Qi branch.
    ConditionedContext {
        /// Coordinate whose current overview conditions this branch.
        context_coordinate: ProjectContextCoordinate,
    },
}

impl std::fmt::Debug for SemanticQueryChannelKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Problem => "Problem",
            Self::ConditionedContext { .. } => "ConditionedContext(<redacted-coordinate>)",
        })
    }
}

/// One current, authorized context overview supplied by the Stage A ticket.
#[derive(Clone, PartialEq, Eq)]
pub struct ConditionedContextOverview {
    /// Context Coordinate identity.
    pub coordinate: ProjectContextCoordinate,
    /// Current Foundation overview semantic text; never source body text.
    pub current_overview_semantic_text: String,
}

impl std::fmt::Debug for ConditionedContextOverview {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConditionedContextOverview")
            .field("coordinate", &"<redacted>")
            .field("current_overview_semantic_text", &"<redacted>")
            .field(
                "current_overview_semantic_text_bytes",
                &self.current_overview_semantic_text.len(),
            )
            .finish()
    }
}

/// Closed reason an otherwise current context overview did not produce Qi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionedInputOmissionReason {
    /// Canonical problem-plus-overview JSON exceeds the Provider input bound.
    ConditionedInputUnsupported,
}

/// One context branch omitted before Provider egress.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OmittedConditionedInput {
    /// Context Coordinate whose Qi branch was omitted.
    pub context_coordinate: ProjectContextCoordinate,
    /// Closed omission reason.
    pub reason: ConditionedInputOmissionReason,
}

impl std::fmt::Debug for OmittedConditionedInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OmittedConditionedInput")
            .field("context_coordinate", &"<redacted>")
            .field("reason", &self.reason)
            .finish()
    }
}

/// Deterministic Q0/Qi build result, including non-fatal conditioned-input
/// omissions that must be copied into result coverage.
#[derive(Clone, PartialEq, Eq)]
pub struct SemanticQueryInputBuildOutcome {
    /// Q0 followed by every supported Qi in canonical Coordinate order.
    pub inputs: Vec<SemanticQueryEncoderInput>,
    /// Unsupported Qi branches in canonical Coordinate order.
    pub omitted_contexts: Vec<OmittedConditionedInput>,
}

impl std::fmt::Debug for SemanticQueryInputBuildOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SemanticQueryInputBuildOutcome")
            .field("input_count", &self.inputs.len())
            .field("omitted_context_count", &self.omitted_contexts.len())
            .finish_non_exhaustive()
    }
}

/// One immutable, digest-bound Provider query input.
#[derive(Clone, PartialEq, Eq)]
pub struct SemanticQueryEncoderInput {
    request_id: Uuid,
    channel_id: Digest32,
    channel_kind: SemanticQueryChannelKind,
    query_contract_digest: Digest32,
    text_digest: Digest32,
    text: String,
}

impl std::fmt::Debug for SemanticQueryEncoderInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let channel_kind = match self.channel_kind {
            SemanticQueryChannelKind::Problem => "problem",
            SemanticQueryChannelKind::ConditionedContext { .. } => "conditioned_context",
        };
        formatter
            .debug_struct("SemanticQueryEncoderInput")
            .field("channel_kind", &channel_kind)
            .field("text", &"<redacted>")
            .field("text_bytes", &self.text.len())
            .finish_non_exhaustive()
    }
}

impl SemanticQueryEncoderInput {
    /// Query request that owns the ephemeral input.
    pub const fn request_id(&self) -> Uuid {
        self.request_id
    }

    /// Stable channel identity independent of input array order.
    pub const fn channel_id(&self) -> Digest32 {
        self.channel_id
    }

    /// Closed problem-only or one-Coordinate-conditioned channel kind.
    pub fn channel_kind(&self) -> &SemanticQueryChannelKind {
        &self.channel_kind
    }

    /// Digest of the canonical query serializer, templates, and input limit.
    pub const fn query_contract_digest(&self) -> Digest32 {
        self.query_contract_digest
    }

    /// Domain-separated digest of the exact canonical Provider input bytes.
    pub const fn text_digest(&self) -> Digest32 {
        self.text_digest
    }

    /// Exact canonical UTF-8 Provider input.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Revalidate immutable digests before crossing a Provider boundary.
    pub fn validate(&self) -> QueryContractResult<()> {
        if self.query_contract_digest != query_contract_digest() {
            return Err(SemanticGraphQueryError::InvalidState(
                "query contract digest mismatch".to_owned(),
            ));
        }
        if self.text_digest != query_text_digest(self.text.as_bytes()) {
            return Err(SemanticGraphQueryError::InvalidState(
                "query text digest mismatch".to_owned(),
            ));
        }
        validate_provider_size(&self.text)
    }
}

/// Return the closed query-template contract digest.
#[must_use]
pub fn query_contract_digest() -> Digest32 {
    hash_domain(
        b"buzz.semantic-graph-query-contract",
        &[QUERY_CONTRACT_DESCRIPTOR.as_bytes()],
    )
}

/// Serialize the problem-only Q0 Provider input to exact canonical UTF-8 JSON.
pub fn canonical_problem_query_text(problem: &str) -> QueryContractResult<String> {
    let problem = validated_problem(problem)?;
    let mut output = String::with_capacity(PROBLEM_CONTRACT.len() + problem.len() + 32);
    output.push_str("{\"contract\":\"");
    output.push_str(PROBLEM_CONTRACT);
    output.push_str("\",\"problem\":\"");
    push_canonical_json_string_contents(&mut output, problem);
    output.push_str("\"}");
    validate_provider_size(&output)?;
    Ok(output)
}

/// Serialize one problem plus one current Coordinate overview Qi Provider
/// input to exact canonical UTF-8 JSON.
pub fn canonical_conditioned_query_text(
    problem: &str,
    current_overview_semantic_text: &str,
) -> QueryContractResult<String> {
    let problem = validated_problem(problem)?;
    if current_overview_semantic_text.as_bytes().contains(&0) {
        return Err(SemanticGraphQueryError::NulText {
            field: "context_overview",
        });
    }
    let mut output = String::with_capacity(
        CONDITIONED_CONTEXT_CONTRACT.len()
            + problem.len()
            + current_overview_semantic_text.len()
            + 64,
    );
    output.push_str("{\"contract\":\"");
    output.push_str(CONDITIONED_CONTEXT_CONTRACT);
    output.push_str("\",\"problem\":\"");
    push_canonical_json_string_contents(&mut output, problem);
    output.push_str("\",\"context_overview\":\"");
    push_canonical_json_string_contents(&mut output, current_overview_semantic_text);
    output.push_str("\"}");
    validate_provider_size(&output)?;
    Ok(output)
}

/// Build Q0 followed by canonical Coordinate-ordered Qi inputs. Missing
/// current heads are omitted by the caller before this pure boundary.
pub fn build_query_encoder_inputs(
    query: &SemanticGraphQuery,
    context_overviews: &[ConditionedContextOverview],
) -> QueryContractResult<SemanticQueryInputBuildOutcome> {
    let canonical_query = query.clone().validate_and_canonicalize()?;
    let mut overviews = context_overviews.to_vec();
    overviews.sort_by(|left, right| left.coordinate.cmp(&right.coordinate));
    if overviews
        .windows(2)
        .any(|pair| pair[0].coordinate == pair[1].coordinate)
    {
        return Err(SemanticGraphQueryError::InvalidState(
            "duplicate conditioned context overview".to_owned(),
        ));
    }
    if overviews.iter().any(|overview| {
        canonical_query
            .context_coordinates
            .binary_search(&overview.coordinate)
            .is_err()
    }) {
        return Err(SemanticGraphQueryError::InvalidState(
            "conditioned overview is not a requested context Coordinate".to_owned(),
        ));
    }

    let contract_digest = query_contract_digest();
    let problem_text = canonical_problem_query_text(&canonical_query.problem)?;
    let mut inputs = Vec::with_capacity(1 + overviews.len());
    let mut omitted_contexts = Vec::new();
    inputs.push(make_input(
        canonical_query.request_id,
        problem_channel_id(canonical_query.request_id),
        SemanticQueryChannelKind::Problem,
        contract_digest,
        problem_text,
    ));

    for overview in overviews {
        let text = match canonical_conditioned_query_text(
            &canonical_query.problem,
            &overview.current_overview_semantic_text,
        ) {
            Ok(text) => text,
            Err(SemanticGraphQueryError::ProviderInputTooLarge { .. }) => {
                omitted_contexts.push(OmittedConditionedInput {
                    context_coordinate: overview.coordinate,
                    reason: ConditionedInputOmissionReason::ConditionedInputUnsupported,
                });
                continue;
            }
            Err(error) => return Err(error),
        };
        let channel_id = conditioned_channel_id(
            canonical_query.request_id,
            canonical_query.project_id,
            &overview.coordinate,
        );
        inputs.push(make_input(
            canonical_query.request_id,
            channel_id,
            SemanticQueryChannelKind::ConditionedContext {
                context_coordinate: overview.coordinate,
            },
            contract_digest,
            text,
        ));
    }
    Ok(SemanticQueryInputBuildOutcome {
        inputs,
        omitted_contexts,
    })
}

fn make_input(
    request_id: Uuid,
    channel_id: Digest32,
    channel_kind: SemanticQueryChannelKind,
    contract_digest: Digest32,
    text: String,
) -> SemanticQueryEncoderInput {
    let text_digest = query_text_digest(text.as_bytes());
    SemanticQueryEncoderInput {
        request_id,
        channel_id,
        channel_kind,
        query_contract_digest: contract_digest,
        text_digest,
        text,
    }
}

fn problem_channel_id(request_id: Uuid) -> Digest32 {
    hash_domain(
        b"buzz.semantic-graph-query-channel",
        &[request_id.as_bytes(), b"problem"],
    )
}

fn conditioned_channel_id(
    request_id: Uuid,
    project_id: Uuid,
    coordinate: &ProjectContextCoordinate,
) -> Digest32 {
    let coordinate = coordinate.tag_value(project_id);
    hash_domain(
        b"buzz.semantic-graph-query-channel",
        &[
            request_id.as_bytes(),
            b"conditioned_context",
            coordinate.as_bytes(),
        ],
    )
}

fn query_text_digest(text: &[u8]) -> Digest32 {
    hash_domain(b"buzz.semantic-graph-query-text", &[text])
}

fn validated_problem(problem: &str) -> QueryContractResult<&str> {
    let problem = problem.trim();
    if problem.is_empty() {
        return Err(SemanticGraphQueryError::BlankProblem);
    }
    if problem.as_bytes().contains(&0) {
        return Err(SemanticGraphQueryError::NulText { field: "problem" });
    }
    if problem.len() > MAX_PROBLEM_BYTES {
        return Err(SemanticGraphQueryError::ProblemTooLarge {
            observed: problem.len(),
            maximum: MAX_PROBLEM_BYTES,
        });
    }
    Ok(problem)
}

fn validate_provider_size(text: &str) -> QueryContractResult<()> {
    if text.len() > MAX_PROVIDER_QUERY_INPUT_BYTES {
        return Err(SemanticGraphQueryError::ProviderInputTooLarge {
            observed: text.len(),
            maximum: MAX_PROVIDER_QUERY_INPUT_BYTES,
        });
    }
    Ok(())
}

fn push_canonical_json_string_contents(output: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{0008}' => output.push_str("\\b"),
            '\u{000c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{0000}'..='\u{001f}' => {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                let value = character as u8;
                output.push_str("\\u00");
                output.push(HEX[(value >> 4) as usize] as char);
                output.push(HEX[(value & 0x0f) as usize] as char);
            }
            other => output.push(other),
        }
    }
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
    use buzz_project_context::ProjectContextCoordinate;
    use buzz_project_view::ProjectViewObjectType;
    use uuid::Uuid;

    use super::{
        build_query_encoder_inputs, canonical_conditioned_query_text, canonical_problem_query_text,
        ConditionedContextOverview, SemanticQueryChannelKind,
    };
    use crate::{LifecycleFilter, SemanticGraphQuery, SemanticGraphQueryBudget};

    fn uuid(value: u128) -> Uuid {
        Uuid::from_u128(0x123e_4567_e89b_42d3_a456_4266_0000_0000 | value)
    }

    fn work(value: u128) -> ProjectContextCoordinate {
        ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Work,
            object_id: uuid(value),
        }
    }

    fn query() -> SemanticGraphQuery {
        SemanticGraphQuery {
            request_id: uuid(1),
            project_id: uuid(2),
            problem: " why? ".to_owned(),
            initial_coordinates: Vec::new(),
            context_coordinates: vec![work(8), work(4)],
            lifecycle_filter: LifecycleFilter::AllCurrent,
            budget: SemanticGraphQueryBudget::default(),
        }
    }

    #[test]
    fn canonical_json_has_exact_frozen_utf8_and_escaping() {
        let problem = "  中文\u{0001}\u{0008}\u{000c}\n\r\t\"\\/\u{2028}尾  ";
        assert_eq!(
            canonical_problem_query_text(problem).expect("canonical problem"),
            "{\"contract\":\"semantic-graph-query.problem\",\"problem\":\"中文\\u0001\\b\\f\\n\\r\\t\\\"\\\\/\u{2028}尾\"}"
        );
        assert_eq!(
            canonical_conditioned_query_text(" P ", "type: Work\nsummary: 中文/\u{2029}")
                .expect("canonical conditioned"),
            "{\"contract\":\"semantic-graph-query.conditioned-context\",\"problem\":\"P\",\"context_overview\":\"type: Work\\nsummary: 中文/\u{2029}\"}"
        );
    }

    #[test]
    fn each_context_is_one_order_independent_conditioned_branch() {
        let first = build_query_encoder_inputs(
            &query(),
            &[
                ConditionedContextOverview {
                    coordinate: work(8),
                    current_overview_semantic_text: "eight".to_owned(),
                },
                ConditionedContextOverview {
                    coordinate: work(4),
                    current_overview_semantic_text: "four".to_owned(),
                },
            ],
        )
        .expect("query inputs");
        let second = build_query_encoder_inputs(
            &query(),
            &[
                ConditionedContextOverview {
                    coordinate: work(4),
                    current_overview_semantic_text: "four".to_owned(),
                },
                ConditionedContextOverview {
                    coordinate: work(8),
                    current_overview_semantic_text: "eight".to_owned(),
                },
            ],
        )
        .expect("query inputs");
        assert_eq!(first, second);
        assert_eq!(first.inputs.len(), 3);
        assert!(matches!(
            first.inputs[0].channel_kind(),
            SemanticQueryChannelKind::Problem
        ));
        assert!(first.inputs.iter().all(|input| input.validate().is_ok()));
        assert!(!first.inputs[0].text().contains("four"));
        assert!(!first.inputs[0].text().contains("eight"));
        assert!(first.omitted_contexts.is_empty());
    }

    #[test]
    fn oversized_conditioned_input_is_omitted_without_losing_q0() {
        let outcome = build_query_encoder_inputs(
            &query(),
            &[ConditionedContextOverview {
                coordinate: work(4),
                current_overview_semantic_text: "x".repeat(crate::MAX_PROVIDER_QUERY_INPUT_BYTES),
            }],
        )
        .expect("Q0 remains executable");
        assert_eq!(outcome.inputs.len(), 1);
        assert_eq!(outcome.omitted_contexts.len(), 1);
    }

    #[test]
    fn provider_input_debug_redacts_problem_overview_and_coordinate_identity() {
        let problem = "CONFIDENTIAL-PROBLEM-中文";
        let overview = "SECRET-TITLE\nSECRET-SUMMARY";
        let coordinate = work(4);
        let request = SemanticGraphQuery {
            problem: problem.to_owned(),
            context_coordinates: vec![coordinate.clone()],
            ..query()
        };
        let context = ConditionedContextOverview {
            coordinate,
            current_overview_semantic_text: overview.to_owned(),
        };
        let context_debug = format!("{context:?}");
        let outcome = build_query_encoder_inputs(&request, &[context]).expect("query inputs");
        let input_debug = format!("{:?}", outcome.inputs[1]);
        let outcome_debug = format!("{outcome:?}");

        for debug in [&context_debug, &input_debug, &outcome_debug] {
            assert!(!debug.contains(problem));
            assert!(!debug.contains(overview));
            assert!(!debug.contains(&uuid(4).to_string()));
        }
        assert!(context_debug.contains("<redacted>"));
        assert!(input_debug.contains("<redacted>"));
        assert!(outcome_debug.contains("input_count: 2"));
    }
}
