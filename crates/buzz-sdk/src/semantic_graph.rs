//! Relay-only virtual Event support for semantic Project Context graph results.
//!
//! These helpers perform no network or persistence work. The Relay signs the
//! returned [`EventBuilder`] only after query postflight succeeds and returns
//! that Event solely in the current authenticated response.

use buzz_core::kind::KIND_SEMANTIC_GRAPH_QUERY_RESULT;
use buzz_core::{CommunityId, EventId, PublicKey};
use buzz_semantic::Digest32;
use buzz_semantic_query::{
    verify_http_request_binding, SemanticGraphQuery, SemanticGraphQueryResult, MAX_RESPONSE_BYTES,
};
use nostr::{Event, EventBuilder, Kind, Tag};
use serde::Serialize;

use crate::SdkError;

const RESULT_MARKER: &str = "buzz-project-context-semantic-result";

/// Canonical semantic graph HTTP request and the exact serialized body.
///
/// The exact bytes are the only bytes that may be supplied both to the
/// `POST /query` request body and to the NIP-98 payload hash.  Consumers must
/// not reserialize [`Self::request`] after signing: even an equivalent JSON
/// encoding would break the Relay's request-binding proof.
#[derive(Clone)]
pub struct SemanticGraphHttpQueryRequest {
    /// Validated, canonical request embedded in the exclusive query filter.
    pub request: SemanticGraphQuery,
    /// Byte-for-byte JSON body for the authenticated `POST /query` attempt.
    pub exact_body: Vec<u8>,
}

impl std::fmt::Debug for SemanticGraphHttpQueryRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SemanticGraphHttpQueryRequest")
            .field("request", &"<redacted>")
            .field("request_id", &self.request.request_id)
            .field(
                "exact_body",
                &format_args!("<redacted:{} bytes>", self.exact_body.len()),
            )
            .finish()
    }
}

#[derive(Serialize)]
struct SemanticGraphQueryFilter<'a> {
    kinds: [u32; 1],
    authors: [String; 1],
    #[serde(rename = "#p")]
    caller: [String; 1],
    limit: u8,
    buzz_project_context_semantic: &'a SemanticGraphQuery,
}

/// Validate and serialize the one canonical semantic graph `/query` filter.
///
/// The returned body is exactly one JSON filter whose only keys are `kinds`,
/// `authors`, `#p`, `limit`, and `buzz_project_context_semantic`. The Relay
/// author and authenticated caller are derived from the supplied public keys;
/// callers cannot inject alternate filter fields or identities.
pub fn build_semantic_graph_http_query_request(
    request: SemanticGraphQuery,
    expected_relay: &PublicKey,
    authenticated_caller: &PublicKey,
) -> Result<SemanticGraphHttpQueryRequest, SdkError> {
    let request = request
        .validate_and_canonicalize()
        .map_err(|error| SdkError::InvalidInput(format!("invalid semantic query: {error}")))?;
    let filter = SemanticGraphQueryFilter {
        kinds: [KIND_SEMANTIC_GRAPH_QUERY_RESULT],
        authors: [expected_relay.to_hex()],
        caller: [authenticated_caller.to_hex()],
        limit: 1,
        buzz_project_context_semantic: &request,
    };
    let exact_body = serde_json::to_vec(&[filter])
        .map_err(|error| SdkError::InvalidInput(format!("serialize semantic query: {error}")))?;
    Ok(SemanticGraphHttpQueryRequest {
        request,
        exact_body,
    })
}

/// The authenticated HTTP transcript and request expected by a result verifier.
///
/// `project_id` must come from host resolution. `exact_authenticated_body` is
/// the byte-for-byte body covered by the NIP-98 authentication Event, not a
/// reserialized approximation.
#[derive(Clone, Copy)]
pub struct SemanticGraphHttpRequestObservation<'a> {
    /// Host-derived Community/Project identity.
    pub project_id: CommunityId,
    /// Authenticated caller whose key must appear in the exact `p` tag.
    pub authenticated_caller: PublicKey,
    /// Canonical request sent in the semantic filter extension.
    pub request: &'a SemanticGraphQuery,
    /// NIP-98 authentication Event identity for this one HTTP attempt.
    pub nip98_auth_event_id: EventId,
    /// Exact authenticated `POST /query` body bytes.
    pub exact_authenticated_body: &'a [u8],
}

impl std::fmt::Debug for SemanticGraphHttpRequestObservation<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SemanticGraphHttpRequestObservation")
            .field("project_id", &self.project_id)
            .field("authenticated_caller", &self.authenticated_caller)
            .field("request", &"<redacted>")
            .field("request_id", &self.request.request_id)
            .field("nip98_auth_event_id", &self.nip98_auth_event_id)
            .field(
                "exact_authenticated_body",
                &format_args!("<redacted:{} bytes>", self.exact_authenticated_body.len()),
            )
            .finish()
    }
}

/// Build the unsigned response-only Event for a validated semantic query result.
///
/// The caller must sign this builder with the current Relay identity. The
/// resulting Event is virtual: it must never enter ingest, storage, search,
/// pubsub, fan-out, or ordinary query paths.
pub fn build_semantic_graph_query_result(
    result: &SemanticGraphQueryResult,
    authenticated_caller: &PublicKey,
) -> Result<EventBuilder, SdkError> {
    result
        .validate()
        .map_err(|error| SdkError::InvalidInput(format!("invalid semantic result: {error}")))?;
    require_result_uuid(result.request_id, "result.request_id", false)?;
    require_result_uuid(result.project_id, "result.project_id", false)?;
    require_result_uuid(
        result.observations.semantic_generation_id,
        "result.observations.semantic_generation_id",
        false,
    )?;

    let content = canonical_json(result, "serialize semantic graph result")?;
    let maximum = MAX_RESPONSE_BYTES as usize;
    if content.len() > maximum {
        return Err(SdkError::ContentTooLarge {
            max: maximum,
            got: content.len(),
        });
    }

    let request_id = result.request_id.to_string();
    let request_binding = result.request_binding_digest.to_hex();
    let caller = authenticated_caller.to_hex();
    Ok(EventBuilder::new(
        Kind::Custom(KIND_SEMANTIC_GRAPH_QUERY_RESULT as u16),
        content,
    )
    .tags([
        tag(["p", caller.as_str()])?,
        tag(["request_id", request_id.as_str()])?,
        tag(["request_binding", request_binding.as_str()])?,
        tag(["t", RESULT_MARKER])?,
    ]))
}

/// Verify and parse one Relay-signed response-only semantic result Event.
///
/// Verification covers the Schnorr signature, expected Relay and caller,
/// exact kind/tag sequence, closed canonical content, request and Project
/// identity, the NIP-98/body request binding, result invariants, and the
/// caller-requested serialized Event-array byte budget.
pub fn parse_semantic_graph_query_result(
    event: &Event,
    expected_relay: &PublicKey,
    expected: SemanticGraphHttpRequestObservation<'_>,
) -> Result<SemanticGraphQueryResult, SdkError> {
    let canonical_request = expected
        .request
        .clone()
        .validate_and_canonicalize()
        .map_err(|error| {
            SdkError::InvalidInput(format!("invalid expected semantic request: {error}"))
        })?;
    if canonical_request.project_id != *expected.project_id.as_uuid() {
        return Err(SdkError::InvalidInput(
            "expected semantic request disagrees with the host-derived Project".to_owned(),
        ));
    }

    let response_size = serialized_event_array_size(event)?;
    let maximum = canonical_request.budget.max_response_bytes as usize;
    if response_size > maximum {
        return Err(SdkError::ContentTooLarge {
            max: maximum,
            got: response_size,
        });
    }

    event
        .verify()
        .map_err(|error| invalid_projection(format!("invalid event signature: {error}")))?;
    if event.pubkey != *expected_relay {
        return Err(invalid_projection(
            "semantic result signer does not match the expected Relay identity",
        ));
    }
    if u32::from(event.kind.as_u16()) != KIND_SEMANTIC_GRAPH_QUERY_RESULT {
        return Err(invalid_projection(format!(
            "semantic result kind must be {KIND_SEMANTIC_GRAPH_QUERY_RESULT}"
        )));
    }

    let result = parse_closed_result(event)?;
    require_canonical_content(&event.content, &result)?;
    result
        .validate_for_request(&canonical_request)
        .map_err(|error| invalid_projection(format!("invalid semantic result: {error}")))?;
    if result.request_id != canonical_request.request_id {
        return Err(invalid_projection(
            "semantic result belongs to a different request",
        ));
    }
    if result.project_id != canonical_request.project_id
        || result.project_id != *expected.project_id.as_uuid()
    {
        return Err(invalid_projection(
            "semantic result belongs to a different Project/Community",
        ));
    }

    require_result_uuid(result.request_id, "result.request_id", true)?;
    require_result_uuid(result.project_id, "result.project_id", true)?;
    require_result_uuid(
        result.observations.semantic_generation_id,
        "result.observations.semantic_generation_id",
        true,
    )?;

    let caller = expected.authenticated_caller.to_hex();
    let request_id = result.request_id.to_string();
    let request_binding = result.request_binding_digest.to_hex();
    require_exact_tags(
        event,
        &[
            vec!["p".to_owned(), caller],
            vec!["request_id".to_owned(), request_id],
            vec!["request_binding".to_owned(), request_binding],
            vec!["t".to_owned(), RESULT_MARKER.to_owned()],
        ],
    )?;

    let auth_event_id = Digest32::from_bytes(expected.nip98_auth_event_id.to_bytes());
    verify_http_request_binding(
        result.request_binding_digest,
        *expected.project_id.as_uuid(),
        &expected.authenticated_caller.to_bytes(),
        auth_event_id,
        expected.exact_authenticated_body,
    )
    .map_err(|_| invalid_projection("semantic result request binding does not match"))?;

    Ok(result)
}

fn parse_closed_result(event: &Event) -> Result<SemanticGraphQueryResult, SdkError> {
    serde_json::from_str(&event.content)
        .map_err(|error| invalid_projection(format!("invalid semantic result content: {error}")))
}

fn require_canonical_content(
    content: &str,
    result: &SemanticGraphQueryResult,
) -> Result<(), SdkError> {
    let canonical = serde_json::to_string(result)
        .map_err(|error| invalid_projection(format!("serialize semantic result: {error}")))?;
    if content != canonical {
        return Err(invalid_projection(
            "semantic result content is not the exact canonical JSON encoding",
        ));
    }
    Ok(())
}

fn require_exact_tags(event: &Event, expected: &[Vec<String>]) -> Result<(), SdkError> {
    let actual = event.tags.iter().map(Tag::as_slice).collect::<Vec<_>>();
    if actual.len() != expected.len()
        || actual
            .iter()
            .zip(expected)
            .any(|(actual, expected)| *actual != expected.as_slice())
    {
        return Err(invalid_projection(
            "semantic result tags are not the exact canonical tag sequence",
        ));
    }
    Ok(())
}

fn serialized_event_array_size(event: &Event) -> Result<usize, SdkError> {
    serde_json::to_vec(std::slice::from_ref(event))
        .map(|bytes| bytes.len())
        .map_err(|error| invalid_projection(format!("serialize semantic result Event: {error}")))
}

fn require_result_uuid(
    value: uuid::Uuid,
    field: &'static str,
    projection_error: bool,
) -> Result<(), SdkError> {
    if value.is_nil() || value.get_version_num() != 4 {
        let message = format!("{field} must be a UUIDv4");
        return if projection_error {
            Err(invalid_projection(message))
        } else {
            Err(SdkError::InvalidInput(message))
        };
    }
    Ok(())
}

fn canonical_json<T: Serialize>(value: &T, context: &str) -> Result<String, SdkError> {
    serde_json::to_string(value)
        .map_err(|error| SdkError::InvalidInput(format!("{context}: {error}")))
}

fn tag<const N: usize>(parts: [&str; N]) -> Result<Tag, SdkError> {
    Tag::parse(parts).map_err(|error| SdkError::InvalidTag(error.to_string()))
}

fn invalid_projection(message: impl Into<String>) -> SdkError {
    SdkError::InvalidProjection(message.into())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use buzz_core::kind::KIND_HTTP_AUTH;
    use buzz_core::{CommunityId, Keys};
    use buzz_project_context::{canonicalize_coordinates, EdgeKey, ProjectContextCoordinate};
    use buzz_project_view::ProjectViewObjectType;
    use buzz_semantic::{
        Digest32, ProjectDocumentSourceBasis, ProjectViewSemanticType, ProjectViewSourceBasis,
        SemanticCoverage, SemanticLifecycleClass, SemanticSourceBasis, SemanticSourceIdentity,
        SemanticSourceKind,
    };
    use buzz_semantic_query::{
        budget_profile_digest, candidate_score, derive_http_request_binding, derive_path_id,
        derive_root_id, document_score, harmonic_score, path_score, query_contract_digest,
        ranking_contract_digest, target_coordinate_score, AcceptedContextCoordinateObservation,
        AcceptedInitialCoordinateObservation, AnchorGain, BranchStopReason,
        CanonicalSourceProvenance, CompletionReason, ContextDocumentBindingObservation,
        CurrentGraphMembershipObservation, DegradedModeCounts, EmbeddingCoverageCounts,
        LifecycleFilter, OmittedContextChannelCounts, OmittedContextCoordinateObservation,
        OmittedContextCoordinateReason, OmittedForResponseBudgetCounts,
        ProjectContextBindingProvenance, ProjectContextEdgeProvenance, RootDiscoveryChannel,
        RootStructuralEntrypoint, Score, ScoreExplanation, SeedOutcome,
        SemanticContinuedCoordinate, SemanticEdgeObservation, SemanticGraphQuery,
        SemanticGraphQueryBudget, SemanticGraphQueryCoverage, SemanticGraphQueryInputObservations,
        SemanticGraphQueryObservations, SemanticGraphQueryResult, SemanticHeadProvenance,
        SemanticHeadState, SemanticHyperedgeHop, SemanticPath, SemanticProvenance,
        SemanticRelationDocument, SemanticRoot, SemanticScoreRole, SemanticSourcePreview,
        TruncationCountsByDimension,
    };
    use chrono::{TimeZone, Utc};
    use nostr::{Event, EventBuilder, Kind, Tag};
    use serde_json::Value;
    use uuid::Uuid;

    use super::{
        build_semantic_graph_http_query_request, build_semantic_graph_query_result,
        parse_semantic_graph_query_result, SemanticGraphHttpRequestObservation, RESULT_MARKER,
    };

    struct Fixture {
        relay: Keys,
        caller: Keys,
        other: Keys,
        auth_event: Event,
        body: Vec<u8>,
        request: SemanticGraphQuery,
        result: SemanticGraphQueryResult,
        event: Event,
    }

    fn uuid(seed: u64) -> Uuid {
        Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("UUIDv4 fixture")
    }

    fn digest(seed: u8) -> Digest32 {
        Digest32::from_bytes([seed; 32])
    }

    fn coordinate(object_type: ProjectViewObjectType, seed: u64) -> ProjectContextCoordinate {
        ProjectContextCoordinate::ProjectViewObject {
            object_type,
            object_id: uuid(seed),
        }
    }

    impl Fixture {
        fn new() -> Self {
            Self::with_lifecycle_filter(LifecycleFilter::AllCurrent)
        }

        fn with_lifecycle_filter(lifecycle_filter: LifecycleFilter) -> Self {
            let relay = Keys::generate();
            let caller = Keys::generate();
            let other = Keys::generate();
            let initial = coordinate(ProjectViewObjectType::Requirement, 10);
            let request = SemanticGraphQuery {
                request_id: Uuid::new_v4(),
                project_id: Uuid::new_v4(),
                problem: "Why does this release failure recur?".to_owned(),
                initial_coordinates: vec![initial],
                context_coordinates: Vec::new(),
                lifecycle_filter,
                budget: SemanticGraphQueryBudget::default(),
            };
            let body = serde_json::to_vec(&request).expect("request fixture serializes");
            let auth_event = EventBuilder::new(
                Kind::Custom(KIND_HTTP_AUTH as u16),
                "authenticated POST /query fixture",
            )
            .sign_with_keys(&caller)
            .expect("auth fixture signs");
            let binding = derive_http_request_binding(
                request.project_id,
                &caller.public_key().to_bytes(),
                Digest32::from_bytes(auth_event.id.to_bytes()),
                &body,
            )
            .expect("request binding");
            let result = nonempty_result(&request, binding);
            let event = build_semantic_graph_query_result(&result, &caller.public_key())
                .expect("valid result builds")
                .sign_with_keys(&relay)
                .expect("result fixture signs");
            Self {
                relay,
                caller,
                other,
                auth_event,
                body,
                request,
                result,
                event,
            }
        }

        fn observation(&self) -> SemanticGraphHttpRequestObservation<'_> {
            SemanticGraphHttpRequestObservation {
                project_id: CommunityId::from_uuid(self.request.project_id),
                authenticated_caller: self.caller.public_key(),
                request: &self.request,
                nip98_auth_event_id: self.auth_event.id,
                exact_authenticated_body: &self.body,
            }
        }

        fn resign(&self, kind: Kind, content: String, tags: Vec<Tag>) -> Event {
            EventBuilder::new(kind, content)
                .tags(tags)
                .custom_created_at(self.event.created_at)
                .sign_with_keys(&self.relay)
                .expect("modified fixture signs")
        }
    }

    fn project_view_basis(seed: u8) -> SemanticSourceBasis {
        SemanticSourceBasis::ProjectView(ProjectViewSourceBasis {
            schema_version: 3,
            object_revision: u64::from(seed) + 1,
            source_change_id: digest(seed),
        })
    }

    fn canonical_provenance(
        source_basis: SemanticSourceBasis,
        seed: u8,
    ) -> CanonicalSourceProvenance {
        CanonicalSourceProvenance {
            source_basis,
            source_invalidation_epoch: u64::from(seed) + 1,
            source_snapshot_digest: digest(seed),
            summary_coverage: SemanticCoverage::TitleOnly,
        }
    }

    fn semantic_provenance(generation_id: Uuid, seed: u8) -> SemanticProvenance {
        SemanticProvenance {
            generation_id,
            unit_key: "overview".to_owned(),
            source_snapshot_digest: digest(seed),
            source_generation_contract_digest: digest(1),
            embedding_space_fence: digest(2),
        }
    }

    fn preview(title: &str) -> SemanticSourcePreview {
        SemanticSourcePreview {
            title: title.to_owned(),
            summary: None,
            summary_omitted_reason: None,
        }
    }

    fn candidate_explanation(problem_score: Score, anchor_gain: AnchorGain) -> ScoreExplanation {
        let final_score = candidate_score(problem_score, Score::ZERO, anchor_gain);
        ScoreExplanation {
            score_role: SemanticScoreRole::Candidate,
            problem_score,
            conditioned_evidence: Vec::new(),
            highest_gain: Score::ZERO,
            second_highest_gain: Score::ZERO,
            environment_gain: Score::ZERO,
            anchor_gain,
            local_coherence: None,
            document_score: None,
            target_coordinate_score: None,
            transition_score: None,
            penalties: Vec::new(),
            final_score,
        }
    }

    fn relation_explanation(problem_score: Score, coherence: Score) -> ScoreExplanation {
        let final_score = document_score(problem_score, Score::ZERO, Some(coherence));
        ScoreExplanation {
            score_role: SemanticScoreRole::RelationDocument,
            problem_score,
            conditioned_evidence: Vec::new(),
            highest_gain: Score::ZERO,
            second_highest_gain: Score::ZERO,
            environment_gain: Score::ZERO,
            anchor_gain: AnchorGain::None,
            local_coherence: Some(coherence),
            document_score: None,
            target_coordinate_score: None,
            transition_score: None,
            penalties: Vec::new(),
            final_score,
        }
    }

    fn target_explanation(problem_score: Score, coherence: Score) -> ScoreExplanation {
        let final_score = target_coordinate_score(problem_score, Score::ZERO, coherence);
        ScoreExplanation {
            score_role: SemanticScoreRole::TargetCoordinate,
            problem_score,
            conditioned_evidence: Vec::new(),
            highest_gain: Score::ZERO,
            second_highest_gain: Score::ZERO,
            environment_gain: Score::ZERO,
            anchor_gain: AnchorGain::None,
            local_coherence: Some(coherence),
            document_score: None,
            target_coordinate_score: None,
            transition_score: None,
            penalties: Vec::new(),
            final_score,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn hop(
        project_id: Uuid,
        generation_id: Uuid,
        ordinal: u16,
        entered: ProjectContextCoordinate,
        target: ProjectContextCoordinate,
        alternate_member: ProjectContextCoordinate,
        document_seed: u8,
    ) -> SemanticHyperedgeHop {
        let coordinates =
            canonicalize_coordinates(vec![entered.clone(), target.clone(), alternate_member])
                .expect("canonical edge");
        let edge_key = EdgeKey::derive(project_id, &coordinates).expect("edge key");
        let document_id = uuid(u64::from(document_seed));
        let binding_provenance = ProjectContextBindingProvenance {
            binding_context_revision: u64::from(document_seed) + 1,
            source_change_id: digest(document_seed.wrapping_add(1)),
            projection_event_id: digest(document_seed.wrapping_add(2)),
        };
        let problem_score = Score::new(820_000).expect("problem score");
        let coherence = Score::new(760_000).expect("coherence score");
        let relation_explanation = relation_explanation(problem_score, coherence);
        let target_explanation = target_explanation(problem_score, coherence);
        let transition_score = harmonic_score(
            relation_explanation.final_score,
            target_explanation.final_score,
        );
        SemanticHyperedgeHop {
            ordinal,
            entered_from_coordinate: Some(entered),
            edge: SemanticEdgeObservation {
                edge_key,
                complete_coordinates: coordinates,
                provenance: ProjectContextEdgeProvenance {
                    last_context_revision: u64::from(document_seed),
                    source_change_id: digest(document_seed),
                },
                current_context_document_bindings: vec![ContextDocumentBindingObservation {
                    document_id,
                    provenance: binding_provenance.clone(),
                }],
            },
            selected_relation_document: SemanticRelationDocument {
                document_id,
                binding_provenance,
                preview: preview(&format!("Context document {document_seed}")),
                canonical_provenance: canonical_provenance(
                    SemanticSourceBasis::ProjectDocument(ProjectDocumentSourceBasis {
                        document_revision: u64::from(document_seed),
                        source_change_id: digest(document_seed.wrapping_add(3)),
                    }),
                    document_seed.wrapping_add(4),
                ),
                semantic_provenance: semantic_provenance(
                    generation_id,
                    document_seed.wrapping_add(4),
                ),
                document_score: relation_explanation.final_score,
                score_explanation: relation_explanation,
            },
            continued_to_coordinate: SemanticContinuedCoordinate {
                coordinate: target,
                preview: preview(&format!("Target coordinate {document_seed}")),
                lifecycle: SemanticLifecycleClass::Active,
                canonical_provenance: canonical_provenance(
                    project_view_basis(document_seed.wrapping_add(5)),
                    document_seed.wrapping_add(5),
                ),
                semantic_provenance: semantic_provenance(
                    generation_id,
                    document_seed.wrapping_add(5),
                ),
                target_score: target_explanation.final_score,
                score_explanation: target_explanation,
            },
            transition_score,
        }
    }

    fn nonempty_result(
        request: &SemanticGraphQuery,
        binding: Digest32,
    ) -> SemanticGraphQueryResult {
        let generation_id = Uuid::new_v4();
        let root_coordinate = request.initial_coordinates[0].clone();
        let middle_coordinate = coordinate(ProjectViewObjectType::Work, 11);
        let terminal_coordinate = coordinate(ProjectViewObjectType::Issue, 12);
        let alternate_coordinate = coordinate(ProjectViewObjectType::Role, 13);
        let root_source = SemanticSourceIdentity {
            community_id: request.project_id,
            kind: SemanticSourceKind::ProjectView(ProjectViewSemanticType::Requirement),
            source_id: uuid(10),
        };
        let entrypoint = RootStructuralEntrypoint::Coordinate {
            coordinate: root_coordinate.clone(),
        };
        let root_score_explanation = candidate_explanation(
            Score::new(840_000).expect("root score"),
            AnchorGain::ExplicitInitial,
        );
        let root = SemanticRoot {
            root_id: derive_root_id(
                request.project_id,
                &root_source,
                std::slice::from_ref(&entrypoint),
            )
            .expect("root id"),
            discovery_channels: vec![RootDiscoveryChannel::ExplicitInitial],
            structural_entrypoints: vec![entrypoint.clone()],
            source: root_source,
            preview: preview("Release recurrence requirement"),
            lifecycle: SemanticLifecycleClass::Active,
            source_status: Some("active".to_owned()),
            canonical_provenance: canonical_provenance(project_view_basis(10), 10),
            semantic_provenance: Some(semantic_provenance(generation_id, 10)),
            semantic_score: Some(root_score_explanation.final_score),
            score_explanation: Some(root_score_explanation),
            seed_outcomes: vec![SeedOutcome {
                structural_entrypoint: entrypoint,
                produced_path_count: 1,
                zero_hop_stop_reason: None,
            }],
        };
        let first_hop = hop(
            request.project_id,
            generation_id,
            1,
            root_coordinate.clone(),
            middle_coordinate.clone(),
            alternate_coordinate.clone(),
            20,
        );
        let second_hop = hop(
            request.project_id,
            generation_id,
            2,
            middle_coordinate.clone(),
            terminal_coordinate.clone(),
            alternate_coordinate,
            30,
        );
        let hops = vec![first_hop, second_hop];
        let path_score_explanation = path_score(
            root.semantic_score,
            &hops
                .iter()
                .map(|hop| hop.transition_score)
                .collect::<Vec<_>>(),
        )
        .expect("path score");
        let path = SemanticPath {
            path_id: derive_path_id(root.root_id, &hops).expect("path id"),
            root_id: root.root_id,
            hops,
            terminal_coordinate,
            path_score: path_score_explanation.final_score.expect("scored path"),
            path_score_explanation,
            branch_stop_reason: BranchStopReason::FrontierExhausted,
        };

        SemanticGraphQueryResult {
            request_id: request.request_id,
            project_id: request.project_id,
            request_binding_digest: binding,
            observations: SemanticGraphQueryObservations {
                semantic_generation_id: generation_id,
                source_generation_contract_digest: Digest32::from_bytes([1; 32]),
                embedding_space_fence: Digest32::from_bytes([2; 32]),
                query_contract_digest: query_contract_digest(),
                ranking_contract_digest: ranking_contract_digest().expect("ranking digest"),
                budget_profile_digest: budget_profile_digest().expect("budget digest"),
                extractor_version: "project-overview-v1".to_owned(),
                project_context_revision: 7,
                snapshot_observed_at: Utc
                    .timestamp_opt(1_700_000_000, 0)
                    .single()
                    .expect("timestamp fixture"),
            },
            input_observations: SemanticGraphQueryInputObservations {
                accepted_initial_coordinates: vec![AcceptedInitialCoordinateObservation {
                    coordinate: root_coordinate,
                    graph_membership: CurrentGraphMembershipObservation {
                        context_revision: 7,
                        incident_edge_keys: vec![path.hops[0].edge.edge_key],
                    },
                    source_basis: project_view_basis(10),
                    semantic_state: SemanticHeadState::Current(SemanticHeadProvenance {
                        generation_id,
                        unit_key: "overview".to_owned(),
                        snapshot_digest: digest(10),
                    }),
                }],
                initial_not_in_graph: Vec::new(),
                omitted_initial_coordinates: Vec::new(),
                accepted_context_coordinates: Vec::new(),
                omitted_context_coordinates: Vec::new(),
            },
            roots: vec![root],
            paths: vec![path],
            coverage: SemanticGraphQueryCoverage {
                authorized_graph_sources: 5,
                current_indexed_graph_sources: 5,
                title_only_sources: 0,
                embedding_coverage: EmbeddingCoverageCounts {
                    current: 5,
                    ..EmbeddingCoverageCounts::default()
                },
                query_channels_requested: 1,
                query_channels_executed: 1,
                omitted_context_channel_counts_by_reason: OmittedContextChannelCounts::default(),
                neutral_candidates_considered: 5,
                conditioned_candidates_considered: 0,
                roots_selected: 1,
                roots_returned: 1,
                expanded_coordinates: 2,
                incident_edges_materialized: 2,
                relation_options_materialized: 2,
                target_options_materialized: 2,
                paths_generated: 1,
                paths_retained: 1,
                paths_returned: 1,
                omitted_for_response_budget: OmittedForResponseBudgetCounts::default(),
                truncation_counts_by_dimension: TruncationCountsByDimension::default(),
                truncation_samples: Vec::new(),
                degraded_mode_counts: DegradedModeCounts::default(),
            },
            completion_reason: CompletionReason::FrontierExhausted,
            exhausted_dimensions: Vec::new(),
        }
    }

    #[test]
    fn signed_virtual_result_round_trips_with_the_exact_four_tags() {
        let fixture = Fixture::new();
        let parsed = parse_semantic_graph_query_result(
            &fixture.event,
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .expect("valid virtual result verifies");
        assert_eq!(parsed, fixture.result);

        let expected = vec![
            vec!["p".to_owned(), fixture.caller.public_key().to_hex()],
            vec![
                "request_id".to_owned(),
                fixture.request.request_id.to_string(),
            ],
            vec![
                "request_binding".to_owned(),
                fixture.result.request_binding_digest.to_hex(),
            ],
            vec!["t".to_owned(), RESULT_MARKER.to_owned()],
        ];
        let actual = fixture
            .event
            .tags
            .iter()
            .map(|tag| tag.as_slice().to_vec())
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
        assert!(actual
            .iter()
            .all(|tag| !matches!(tag[0].as_str(), "h" | "q" | "x")));
    }

    #[test]
    fn http_request_observation_debug_redacts_problem_and_exact_body() {
        let fixture = Fixture::new();
        let rendered = format!("{:?}", fixture.observation());
        assert!(rendered.contains("<redacted>"));
        assert!(rendered.contains("bytes>"));
        assert!(!rendered.contains(&fixture.request.problem));
        assert!(!rendered
            .contains(std::str::from_utf8(&fixture.body).expect("request body fixture is UTF-8")));
    }

    #[test]
    fn http_query_request_is_canonical_closed_and_redacted() {
        let relay = Keys::generate();
        let caller = Keys::generate();
        let mut request = SemanticGraphQuery {
            request_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            problem: "  secret release problem  ".to_owned(),
            initial_coordinates: vec![coordinate(ProjectViewObjectType::Issue, 22)],
            context_coordinates: vec![coordinate(ProjectViewObjectType::Role, 21)],
            lifecycle_filter: LifecycleFilter::AllCurrent,
            budget: SemanticGraphQueryBudget::default(),
        };
        request
            .initial_coordinates
            .push(request.initial_coordinates[0].clone());

        let prepared = build_semantic_graph_http_query_request(
            request,
            &relay.public_key(),
            &caller.public_key(),
        )
        .expect("valid semantic query request");
        assert_eq!(prepared.request.problem, "secret release problem");
        assert_eq!(prepared.request.initial_coordinates.len(), 1);

        let [filter]: [Value; 1] = serde_json::from_slice::<Vec<Value>>(&prepared.exact_body)
            .expect("canonical HTTP body")
            .try_into()
            .expect("one filter");
        assert_eq!(
            filter
                .as_object()
                .expect("filter object")
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "#p",
                "authors",
                "buzz_project_context_semantic",
                "kinds",
                "limit",
            ])
        );
        assert_eq!(
            filter["authors"],
            serde_json::json!([relay.public_key().to_hex()])
        );
        assert_eq!(
            filter["#p"],
            serde_json::json!([caller.public_key().to_hex()])
        );
        assert!(filter["buzz_project_context_semantic"]
            .get("schema_version")
            .is_none());

        let rendered = format!("{prepared:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("secret release problem"));
        assert!(!rendered
            .contains(std::str::from_utf8(&prepared.exact_body).expect("body must be UTF-8")));
    }

    #[test]
    fn verifier_rejects_a_signed_path_with_disconnected_hops() {
        let fixture = Fixture::new();
        let mut malicious = fixture.result.clone();
        let alternate = coordinate(ProjectViewObjectType::Role, 13);
        malicious.paths[0].hops[1].entered_from_coordinate = Some(alternate);
        malicious.paths[0].path_id =
            derive_path_id(malicious.paths[0].root_id, &malicious.paths[0].hops)
                .expect("mutated path id");
        let event = fixture.resign(
            fixture.event.kind,
            serde_json::to_string(&malicious).expect("malicious result serializes"),
            fixture.event.tags.iter().cloned().collect(),
        );

        let error = parse_semantic_graph_query_result(
            &event,
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .expect_err("disconnected path must be rejected");
        assert!(error.to_string().contains("not Coordinate-contiguous"));
    }

    #[test]
    fn verifier_rejects_a_signed_path_target_outside_the_hyperedge() {
        let fixture = Fixture::new();
        let mut malicious = fixture.result.clone();
        let nonmember = coordinate(ProjectViewObjectType::Resource, 99);
        malicious.paths[0].hops[1]
            .continued_to_coordinate
            .coordinate = nonmember.clone();
        malicious.paths[0].terminal_coordinate = nonmember;
        malicious.paths[0].path_id =
            derive_path_id(malicious.paths[0].root_id, &malicious.paths[0].hops)
                .expect("mutated path id");
        let event = fixture.resign(
            fixture.event.kind,
            serde_json::to_string(&malicious).expect("malicious result serializes"),
            fixture.event.tags.iter().cloned().collect(),
        );

        let error = parse_semantic_graph_query_result(
            &event,
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .expect_err("nonmember target must be rejected");
        assert!(error
            .to_string()
            .contains("not a distinct unvisited Hyperedge member"));
    }

    #[test]
    fn verifier_rejects_a_signed_path_with_the_wrong_root_entrypoint() {
        let fixture = Fixture::new();
        let mut malicious = fixture.result.clone();
        let alternate = coordinate(ProjectViewObjectType::Role, 13);
        malicious.paths[0].hops[0].entered_from_coordinate = Some(alternate);
        malicious.paths[0].path_id =
            derive_path_id(malicious.paths[0].root_id, &malicious.paths[0].hops)
                .expect("mutated path id");
        let event = fixture.resign(
            fixture.event.kind,
            serde_json::to_string(&malicious).expect("malicious result serializes"),
            fixture.event.tags.iter().cloned().collect(),
        );

        let error = parse_semantic_graph_query_result(
            &event,
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .expect_err("wrong root entrypoint must be rejected");
        assert!(error
            .to_string()
            .contains("does not start at one of its root structural entrypoints"));
    }

    #[test]
    fn verifier_rejects_re_signed_results_with_foreign_compiled_contract_digests() {
        let fixture = Fixture::new();
        let mut cases = Vec::new();

        let mut query_contract = fixture.result.clone();
        query_contract.observations.query_contract_digest = digest(91);
        cases.push(query_contract);

        let mut ranking_contract = fixture.result.clone();
        ranking_contract.observations.ranking_contract_digest = digest(92);
        cases.push(ranking_contract);

        let mut budget_contract = fixture.result.clone();
        budget_contract.observations.budget_profile_digest = digest(93);
        cases.push(budget_contract);

        for malicious in cases {
            let event = fixture.resign(
                fixture.event.kind,
                serde_json::to_string(&malicious).expect("malicious result serializes"),
                fixture.event.tags.iter().cloned().collect(),
            );
            let error = parse_semantic_graph_query_result(
                &event,
                &fixture.relay.public_key(),
                fixture.observation(),
            )
            .expect_err("foreign compiled contract digest must be rejected");
            assert!(error.to_string().contains("compiled contract digests"));
        }
    }

    #[test]
    fn verifier_rejects_a_re_signed_input_partition_not_owned_by_the_request() {
        let fixture = Fixture::new();
        let mut malicious = fixture.result.clone();
        malicious.input_observations.accepted_initial_coordinates[0].coordinate =
            coordinate(ProjectViewObjectType::Role, 99);
        let event = fixture.resign(
            fixture.event.kind,
            serde_json::to_string(&malicious).expect("malicious result serializes"),
            fixture.event.tags.iter().cloned().collect(),
        );

        let error = parse_semantic_graph_query_result(
            &event,
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .expect_err("foreign initial observation must be rejected");
        assert!(error.to_string().contains("exactly partition the request"));
    }

    #[test]
    fn verifier_rejects_re_signed_noncanonical_or_overlapping_input_observations() {
        let fixture = Fixture::new();
        let mut overlapping = fixture.result.clone();
        overlapping
            .input_observations
            .accepted_initial_coordinates
            .push(overlapping.input_observations.accepted_initial_coordinates[0].clone());

        let mut noncanonical = fixture.result.clone();
        noncanonical.input_observations.initial_not_in_graph = vec![
            coordinate(ProjectViewObjectType::Work, 102),
            coordinate(ProjectViewObjectType::Work, 101),
        ];

        for malicious in [overlapping, noncanonical] {
            let event = fixture.resign(
                fixture.event.kind,
                serde_json::to_string(&malicious).expect("malicious result serializes"),
                fixture.event.tags.iter().cloned().collect(),
            );
            assert!(parse_semantic_graph_query_result(
                &event,
                &fixture.relay.public_key(),
                fixture.observation(),
            )
            .is_err());
        }
    }

    #[test]
    fn verifier_rejects_re_signed_discovery_not_backed_by_request_inputs() {
        let fixture = Fixture::new();

        let mut missing_explicit = fixture.result.clone();
        missing_explicit.roots[0].discovery_channels = vec![RootDiscoveryChannel::ProblemNeutral];

        let mut foreign_conditioned = fixture.result.clone();
        foreign_conditioned.roots[0].discovery_channels.push(
            RootDiscoveryChannel::ContextConditioned {
                context_coordinate: coordinate(ProjectViewObjectType::Work, 103),
            },
        );

        for malicious in [missing_explicit, foreign_conditioned] {
            let event = fixture.resign(
                fixture.event.kind,
                serde_json::to_string(&malicious).expect("malicious result serializes"),
                fixture.event.tags.iter().cloned().collect(),
            );
            assert!(parse_semantic_graph_query_result(
                &event,
                &fixture.relay.public_key(),
                fixture.observation(),
            )
            .is_err());
        }
    }

    #[test]
    fn verifier_rejects_re_signed_ineligible_root_lifecycle() {
        let fixture = Fixture::new();
        let mut malicious = fixture.result.clone();
        malicious.roots[0].lifecycle = SemanticLifecycleClass::Tombstone;
        let event = fixture.resign(
            fixture.event.kind,
            serde_json::to_string(&malicious).expect("malicious result serializes"),
            fixture.event.tags.iter().cloned().collect(),
        );

        let error = parse_semantic_graph_query_result(
            &event,
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .expect_err("tombstoned explicit root must be rejected");
        assert!(error.to_string().contains("ineligible lifecycle"));
    }

    #[test]
    fn verifier_rejects_re_signed_target_outside_the_requested_lifecycle() {
        let fixture = Fixture::with_lifecycle_filter(LifecycleFilter::NonTerminal);
        let mut malicious = fixture.result.clone();
        malicious.paths[0].hops[0].continued_to_coordinate.lifecycle =
            SemanticLifecycleClass::Terminal;
        malicious.paths[0].path_id =
            derive_path_id(malicious.paths[0].root_id, &malicious.paths[0].hops)
                .expect("mutated path id");
        let event = fixture.resign(
            fixture.event.kind,
            serde_json::to_string(&malicious).expect("malicious result serializes"),
            fixture.event.tags.iter().cloned().collect(),
        );

        let error = parse_semantic_graph_query_result(
            &event,
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .expect_err("terminal target must not satisfy a non-terminal request");
        assert!(error.to_string().contains("requested lifecycle filter"));
    }

    #[test]
    fn verifier_rejects_re_signed_explicit_root_basis_or_current_head_mismatch() {
        let fixture = Fixture::new();
        let mut wrong_basis = fixture.result.clone();
        wrong_basis.input_observations.accepted_initial_coordinates[0].source_basis =
            project_view_basis(99);

        let mut wrong_head = fixture.result.clone();
        let SemanticHeadState::Current(head) =
            &mut wrong_head.input_observations.accepted_initial_coordinates[0].semantic_state
        else {
            panic!("fixture initial head must be current");
        };
        head.snapshot_digest = digest(99);

        for (malicious, expected_error) in [
            (wrong_basis, "canonical basis"),
            (wrong_head, "current initial head"),
        ] {
            let event = fixture.resign(
                fixture.event.kind,
                serde_json::to_string(&malicious).expect("malicious result serializes"),
                fixture.event.tags.iter().cloned().collect(),
            );
            let error = parse_semantic_graph_query_result(
                &event,
                &fixture.relay.public_key(),
                fixture.observation(),
            )
            .expect_err("explicit root provenance mismatch must be rejected");
            assert!(error.to_string().contains(expected_error));
        }
    }

    #[test]
    fn verifier_rejects_re_signed_semantic_provenance_for_a_missing_explicit_head() {
        let fixture = Fixture::new();
        let mut malicious = fixture.result.clone();
        malicious.input_observations.accepted_initial_coordinates[0].semantic_state =
            SemanticHeadState::Missing;
        let event = fixture.resign(
            fixture.event.kind,
            serde_json::to_string(&malicious).expect("malicious result serializes"),
            fixture.event.tags.iter().cloned().collect(),
        );

        let error = parse_semantic_graph_query_result(
            &event,
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .expect_err("embedding-less initial must not retain semantic root provenance");
        assert!(error
            .to_string()
            .contains("embedding-less explicit initial"));
    }

    #[test]
    fn verifier_rejects_re_signed_semantic_discovery_without_semantic_evidence() {
        let fixture = Fixture::new();
        let mut malicious = fixture.result.clone();
        malicious.roots[0]
            .discovery_channels
            .push(RootDiscoveryChannel::ProblemNeutral);
        malicious.roots[0].semantic_provenance = None;
        malicious.roots[0].semantic_score = None;
        malicious.roots[0].score_explanation = None;
        let transition_scores = malicious.paths[0]
            .hops
            .iter()
            .map(|hop| hop.transition_score)
            .collect::<Vec<_>>();
        malicious.paths[0].path_score_explanation =
            path_score(None, &transition_scores).expect("embedding-less path score");
        malicious.paths[0].path_score = malicious.paths[0]
            .path_score_explanation
            .final_score
            .expect("non-empty path remains scored");
        let event = fixture.resign(
            fixture.event.kind,
            serde_json::to_string(&malicious).expect("malicious result serializes"),
            fixture.event.tags.iter().cloned().collect(),
        );

        let error = parse_semantic_graph_query_result(
            &event,
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .expect_err("semantic discovery without evidence must be rejected");
        assert!(error.to_string().contains("semantic root discovery"));
    }

    #[test]
    fn request_aware_verifier_rejects_ineligible_accepted_context_lifecycle() {
        let fixture = Fixture::new();
        let context_coordinate = coordinate(ProjectViewObjectType::Work, 105);
        let mut request = fixture.request.clone();
        request.context_coordinates = vec![context_coordinate.clone()];
        let request = request
            .validate_and_canonicalize()
            .expect("context request is canonical");
        let mut malicious = fixture.result.clone();
        malicious
            .input_observations
            .accepted_context_coordinates
            .push(AcceptedContextCoordinateObservation {
                coordinate: context_coordinate,
                source_basis: project_view_basis(105),
                lifecycle: SemanticLifecycleClass::Deleted,
                semantic_head: SemanticHeadProvenance {
                    generation_id: malicious.observations.semantic_generation_id,
                    unit_key: "overview".to_owned(),
                    snapshot_digest: digest(105),
                },
            });
        malicious.coverage.query_channels_requested = 2;
        malicious.coverage.query_channels_executed = 2;

        let error = malicious
            .validate_for_request(&request)
            .expect_err("deleted context lens must be rejected");
        assert!(error.to_string().contains("ineligible lifecycle"));
    }

    #[test]
    fn verifier_rejects_re_signed_foreign_context_observation() {
        let fixture = Fixture::new();
        let mut malicious = fixture.result.clone();
        malicious
            .input_observations
            .omitted_context_coordinates
            .push(OmittedContextCoordinateObservation {
                coordinate: coordinate(ProjectViewObjectType::Work, 104),
                reason: OmittedContextCoordinateReason::SourceNotFound,
            });
        let event = fixture.resign(
            fixture.event.kind,
            serde_json::to_string(&malicious).expect("malicious result serializes"),
            fixture.event.tags.iter().cloned().collect(),
        );

        assert!(parse_semantic_graph_query_result(
            &event,
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .is_err());
    }

    #[test]
    fn verifier_rejects_re_signed_query_channel_counts() {
        let fixture = Fixture::new();
        let mut malicious = fixture.result.clone();
        malicious.coverage.query_channels_requested = 2;
        let event = fixture.resign(
            fixture.event.kind,
            serde_json::to_string(&malicious).expect("malicious result serializes"),
            fixture.event.tags.iter().cloned().collect(),
        );

        let error = parse_semantic_graph_query_result(
            &event,
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .expect_err("invented query channel must be rejected");
        assert!(error.to_string().contains("channel counts"));
    }

    #[test]
    fn verifier_rejects_re_signed_work_beyond_the_caller_budget() {
        let fixture = Fixture::new();
        let mut malicious = fixture.result.clone();
        malicious.coverage.expanded_coordinates =
            u64::from(fixture.request.budget.max_expanded_coordinates) + 1;
        let event = fixture.resign(
            fixture.event.kind,
            serde_json::to_string(&malicious).expect("malicious result serializes"),
            fixture.event.tags.iter().cloned().collect(),
        );

        let error = parse_semantic_graph_query_result(
            &event,
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .expect_err("work beyond the caller budget must be rejected");
        assert!(
            error
                .to_string()
                .contains("exceed the caller-requested budget"),
            "unexpected verifier error: {error}"
        );
    }

    #[test]
    fn verifier_binds_relay_caller_project_request_auth_event_and_exact_body() {
        let fixture = Fixture::new();
        assert!(parse_semantic_graph_query_result(
            &fixture.event,
            &fixture.other.public_key(),
            fixture.observation(),
        )
        .is_err());

        let wrong_caller = fixture.other.public_key();
        let mut observation = fixture.observation();
        observation.authenticated_caller = wrong_caller;
        assert!(parse_semantic_graph_query_result(
            &fixture.event,
            &fixture.relay.public_key(),
            observation,
        )
        .is_err());

        let mut wrong_request = fixture.request.clone();
        wrong_request.request_id = Uuid::new_v4();
        let mut observation = fixture.observation();
        observation.request = &wrong_request;
        assert!(parse_semantic_graph_query_result(
            &fixture.event,
            &fixture.relay.public_key(),
            observation,
        )
        .is_err());

        let mut observation = fixture.observation();
        observation.project_id = CommunityId::from_uuid(Uuid::new_v4());
        assert!(parse_semantic_graph_query_result(
            &fixture.event,
            &fixture.relay.public_key(),
            observation,
        )
        .is_err());

        let other_auth = fixture.resign(
            Kind::Custom(KIND_HTTP_AUTH as u16),
            "other auth".to_owned(),
            Vec::new(),
        );
        let mut observation = fixture.observation();
        observation.nip98_auth_event_id = other_auth.id;
        assert!(parse_semantic_graph_query_result(
            &fixture.event,
            &fixture.relay.public_key(),
            observation,
        )
        .is_err());

        let changed_body = [fixture.body.as_slice(), b" "].concat();
        let mut observation = fixture.observation();
        observation.exact_authenticated_body = &changed_body;
        assert!(parse_semantic_graph_query_result(
            &fixture.event,
            &fixture.relay.public_key(),
            observation,
        )
        .is_err());
    }

    #[test]
    fn verifier_rejects_extra_duplicate_reordered_or_noncanonical_tags() {
        let fixture = Fixture::new();
        let canonical = fixture.event.tags.iter().cloned().collect::<Vec<_>>();
        let mut cases = Vec::new();

        let mut extra = canonical.clone();
        extra.push(Tag::parse(["x", "00"]).expect("extra tag"));
        cases.push(extra);

        let mut duplicate = canonical.clone();
        duplicate.insert(1, canonical[0].clone());
        cases.push(duplicate);

        let mut reordered = canonical.clone();
        reordered.swap(0, 1);
        cases.push(reordered);

        let mut uppercase_binding = canonical;
        uppercase_binding[2] = Tag::parse([
            "request_binding",
            fixture
                .result
                .request_binding_digest
                .to_hex()
                .to_uppercase()
                .as_str(),
        ])
        .expect("uppercase digest tag");
        cases.push(uppercase_binding);

        for tags in cases {
            let event = fixture.resign(fixture.event.kind, fixture.event.content.clone(), tags);
            assert!(parse_semantic_graph_query_result(
                &event,
                &fixture.relay.public_key(),
                fixture.observation(),
            )
            .is_err());
        }
    }

    #[test]
    fn verifier_rejects_unknown_or_noncanonical_closed_content() {
        let fixture = Fixture::new();
        let mut unknown: Value =
            serde_json::from_str(&fixture.event.content).expect("result content fixture");
        unknown
            .as_object_mut()
            .expect("result object")
            .insert("schema_version".to_owned(), Value::from(1));
        let event = fixture.resign(
            fixture.event.kind,
            serde_json::to_string(&unknown).expect("unknown content serializes"),
            fixture.event.tags.iter().cloned().collect(),
        );
        assert!(parse_semantic_graph_query_result(
            &event,
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .is_err());

        let uppercase = fixture
            .result
            .request_binding_digest
            .to_hex()
            .to_uppercase();
        let mut noncanonical: Value =
            serde_json::from_str(&fixture.event.content).expect("result content fixture");
        noncanonical
            .as_object_mut()
            .expect("result object")
            .insert("request_binding_digest".to_owned(), Value::from(uppercase));
        let event = fixture.resign(
            fixture.event.kind,
            serde_json::to_string(&noncanonical).expect("noncanonical content serializes"),
            fixture.event.tags.iter().cloned().collect(),
        );
        assert!(parse_semantic_graph_query_result(
            &event,
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .is_err());

        let whitespace = fixture.resign(
            fixture.event.kind,
            format!(" {}", fixture.event.content),
            fixture.event.tags.iter().cloned().collect(),
        );
        assert!(parse_semantic_graph_query_result(
            &whitespace,
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .is_err());
    }

    #[test]
    fn verifier_rejects_wrong_kind_and_invalid_signature() {
        let fixture = Fixture::new();
        let wrong_kind = fixture.resign(
            Kind::Custom((buzz_core::kind::KIND_SEMANTIC_GRAPH_QUERY_RESULT - 1) as u16),
            fixture.event.content.clone(),
            fixture.event.tags.iter().cloned().collect(),
        );
        assert!(parse_semantic_graph_query_result(
            &wrong_kind,
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .is_err());

        let mut tampered = fixture.event.clone();
        tampered.content.push(' ');
        assert!(parse_semantic_graph_query_result(
            &tampered,
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .is_err());
    }

    #[test]
    fn verifier_enforces_the_requested_full_event_array_byte_budget() {
        let fixture = Fixture::new();
        let mut request = fixture.request.clone();
        request.budget.max_response_bytes = 1;
        let mut observation = fixture.observation();
        observation.request = &request;
        assert!(parse_semantic_graph_query_result(
            &fixture.event,
            &fixture.relay.public_key(),
            observation,
        )
        .is_err());
    }

    #[test]
    fn builder_rejects_non_v4_result_identity() {
        let fixture = Fixture::new();
        let mut invalid = fixture.result;
        invalid.request_id = Uuid::nil();
        assert!(build_semantic_graph_query_result(&invalid, &fixture.caller.public_key()).is_err());
    }
}
