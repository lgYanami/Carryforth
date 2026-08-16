//! Relay-only virtual Event support for one-hop Project Context semantic selection.
//!
//! The closed wire family has two structurally isolated variants:
//! Coordinate-to-Edge returns ranked relation Documents but no Edge members;
//! Edge-to-Coordinate returns ranked members but no relation Documents. These
//! helpers perform no network, persistence, authorization, or ranking work.

use buzz_core::kind::KIND_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_RESULT;
use buzz_core::{CommunityId, EventId, PublicKey};
use buzz_semantic::Digest32;
use buzz_semantic_query::{
    verify_one_hop_semantic_http_request_binding, verify_one_hop_semantic_v2_http_request_binding,
    OneHopSemanticScope, OneHopSemanticSelection, ProjectContextOneHopSemanticQuery,
    ProjectContextOneHopSemanticQueryResult, MAX_ONE_HOP_SEMANTIC_EXACT_HTTP_BODY_BYTES,
    MAX_ONE_HOP_SEMANTIC_RESPONSE_BYTES,
};
use nostr::{Event, EventBuilder, Kind, Tag};
use serde::Serialize;

use crate::SdkError;

/// Exact marker carried by one-hop semantic-selection result Events.
pub const PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_RESULT_MARKER: &str =
    "carryforth-project-context-one-hop-semantic-search-result";
/// Exact marker carried by filtered one-hop semantic-selection v2 result Events.
pub const PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_V2_RESULT_MARKER: &str =
    "carryforth-project-context-one-hop-semantic-search-result-v2";

/// Canonical one-hop semantic HTTP request and exact serialized body.
///
/// `exact_body` is the only body that may be sent to `POST /query` and hashed
/// into the NIP-98 authentication Event. Re-serialization after authentication
/// would invalidate the response binding.
#[derive(Clone)]
pub struct ProjectContextOneHopSemanticHttpQueryRequest {
    /// Validated request embedded in the exclusive query filter.
    pub request: ProjectContextOneHopSemanticQuery,
    /// Byte-for-byte JSON body authenticated by NIP-98.
    pub exact_body: Vec<u8>,
}

impl std::fmt::Debug for ProjectContextOneHopSemanticHttpQueryRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProjectContextOneHopSemanticHttpQueryRequest")
            .field("request_id", &self.request.request_id)
            .field("request", &"<redacted>")
            .field(
                "exact_body",
                &format_args!("<redacted:{} bytes>", self.exact_body.len()),
            )
            .finish()
    }
}

#[derive(Serialize)]
struct ProjectContextOneHopSemanticFilter<'a> {
    kinds: [u32; 1],
    authors: [String; 1],
    #[serde(rename = "#p")]
    caller: [String; 1],
    limit: u8,
    carryforth_project_context_one_hop_semantic_search: &'a ProjectContextOneHopSemanticQuery,
}

#[derive(Serialize)]
struct ProjectContextOneHopSemanticV2Filter<'a> {
    kinds: [u32; 1],
    authors: [String; 1],
    #[serde(rename = "#p")]
    caller: [String; 1],
    limit: u8,
    carryforth_project_context_one_hop_semantic_search_v2: &'a ProjectContextOneHopSemanticQuery,
}

/// Validate and serialize one exclusive one-hop semantic `/query` filter.
///
/// The exact filter keys are `kinds`, `authors`, `#p`, `limit`, and
/// `carryforth_project_context_one_hop_semantic_search` in that order.
pub fn build_project_context_one_hop_semantic_http_query_request(
    request: ProjectContextOneHopSemanticQuery,
    expected_relay: &PublicKey,
    authenticated_caller: &PublicKey,
) -> Result<ProjectContextOneHopSemanticHttpQueryRequest, SdkError> {
    build_one_hop_semantic_http_query_request(request, expected_relay, authenticated_caller, false)
}

/// Validate and serialize one exclusive filtered one-hop semantic v2 request.
pub fn build_project_context_one_hop_semantic_v2_http_query_request(
    request: ProjectContextOneHopSemanticQuery,
    expected_relay: &PublicKey,
    authenticated_caller: &PublicKey,
) -> Result<ProjectContextOneHopSemanticHttpQueryRequest, SdkError> {
    build_one_hop_semantic_http_query_request(request, expected_relay, authenticated_caller, true)
}

fn build_one_hop_semantic_http_query_request(
    request: ProjectContextOneHopSemanticQuery,
    expected_relay: &PublicKey,
    authenticated_caller: &PublicKey,
    filtered: bool,
) -> Result<ProjectContextOneHopSemanticHttpQueryRequest, SdkError> {
    let request = request.validate_and_canonicalize().map_err(|error| {
        SdkError::InvalidInput(format!(
            "invalid Project Context one-hop semantic request: {error}"
        ))
    })?;
    if filtered != request_is_filtered(&request) {
        return Err(SdkError::InvalidInput(
            "one-hop semantic surface does not match the type-filter contract".to_owned(),
        ));
    }
    let kinds = [KIND_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_RESULT];
    let authors = [expected_relay.to_hex()];
    let caller = [authenticated_caller.to_hex()];
    let exact_body = if filtered {
        serde_json::to_vec(&[ProjectContextOneHopSemanticV2Filter {
            kinds,
            authors,
            caller,
            limit: 1,
            carryforth_project_context_one_hop_semantic_search_v2: &request,
        }])
    } else {
        serde_json::to_vec(&[ProjectContextOneHopSemanticFilter {
            kinds,
            authors,
            caller,
            limit: 1,
            carryforth_project_context_one_hop_semantic_search: &request,
        }])
    };
    let exact_body = exact_body.map_err(|error| {
        SdkError::InvalidInput(format!(
            "serialize Project Context one-hop semantic request: {error}"
        ))
    })?;
    if exact_body.len() > MAX_ONE_HOP_SEMANTIC_EXACT_HTTP_BODY_BYTES {
        return Err(SdkError::ContentTooLarge {
            max: MAX_ONE_HOP_SEMANTIC_EXACT_HTTP_BODY_BYTES,
            got: exact_body.len(),
        });
    }
    Ok(ProjectContextOneHopSemanticHttpQueryRequest {
        request,
        exact_body,
    })
}

/// Authenticated HTTP transcript expected by a one-hop result verifier.
///
/// `project_id` must come from the verified request host. The exact body must
/// be the bytes covered by the NIP-98 authentication Event.
#[derive(Clone, Copy)]
pub struct ProjectContextOneHopSemanticHttpRequestObservation<'a> {
    /// Host-derived Community/Project identity.
    pub project_id: CommunityId,
    /// Authenticated caller required in the exact `p` tag.
    pub authenticated_caller: PublicKey,
    /// Canonical request sent in the filter extension.
    pub request: &'a ProjectContextOneHopSemanticQuery,
    /// NIP-98 Event identity for this exact HTTP request.
    pub nip98_auth_event_id: EventId,
    /// Exact authenticated HTTP body.
    pub exact_authenticated_body: &'a [u8],
}

impl std::fmt::Debug for ProjectContextOneHopSemanticHttpRequestObservation<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProjectContextOneHopSemanticHttpRequestObservation")
            .field("project_id", &"<redacted>")
            .field("authenticated_caller", &"<redacted>")
            .field("request_id", &self.request.request_id)
            .field("request", &"<redacted>")
            .field("nip98_auth_event_id", &self.nip98_auth_event_id)
            .field(
                "exact_authenticated_body",
                &format_args!("<redacted:{} bytes>", self.exact_authenticated_body.len()),
            )
            .finish()
    }
}

/// Build the unsigned response-only Event for a validated one-hop result.
///
/// The Relay must sign this builder only after current authorization and
/// release checks pass. The Event must never enter ordinary ingest, storage,
/// search, fan-out, REQ, or COUNT paths.
pub fn build_project_context_one_hop_semantic_search_result(
    result: &ProjectContextOneHopSemanticQueryResult,
    authenticated_caller: &PublicKey,
) -> Result<EventBuilder, SdkError> {
    build_one_hop_semantic_search_result(result, authenticated_caller, false)
}

/// Build the unsigned response-only Event for a filtered one-hop v2 result.
pub fn build_project_context_one_hop_semantic_search_v2_result(
    result: &ProjectContextOneHopSemanticQueryResult,
    authenticated_caller: &PublicKey,
) -> Result<EventBuilder, SdkError> {
    build_one_hop_semantic_search_result(result, authenticated_caller, true)
}

fn build_one_hop_semantic_search_result(
    result: &ProjectContextOneHopSemanticQueryResult,
    authenticated_caller: &PublicKey,
    filtered: bool,
) -> Result<EventBuilder, SdkError> {
    result.validate().map_err(|error| {
        SdkError::InvalidInput(format!("invalid one-hop semantic result: {error}"))
    })?;
    if filtered != result_is_filtered(result) {
        return Err(SdkError::InvalidInput(
            "one-hop semantic result surface does not match the type-filter contract".to_owned(),
        ));
    }
    let content = canonical_json(result, "serialize one-hop semantic result")?;
    if content.len() > MAX_ONE_HOP_SEMANTIC_RESPONSE_BYTES {
        return Err(SdkError::ContentTooLarge {
            max: MAX_ONE_HOP_SEMANTIC_RESPONSE_BYTES,
            got: content.len(),
        });
    }

    let caller = authenticated_caller.to_hex();
    let request_id = result.request_id.to_string();
    let request_binding = result.request_binding_digest.to_hex();
    let marker = if filtered {
        PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_V2_RESULT_MARKER
    } else {
        PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_RESULT_MARKER
    };
    Ok(EventBuilder::new(
        Kind::Custom(KIND_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_RESULT as u16),
        content,
    )
    .tags([
        tag(["p", caller.as_str()])?,
        tag(["request_id", request_id.as_str()])?,
        tag(["request_binding", request_binding.as_str()])?,
        tag(["t", marker])?,
    ]))
}

/// Verify and parse one Relay-signed one-hop semantic result Event.
///
/// Verification covers the Schnorr signature; Relay, caller, and Project
/// identity; exact kind and tag sequence; canonical closed content; exact HTTP
/// transcript binding; variant isolation; ranking; previews and typed reads;
/// and the final signed Event-array byte cap.
pub fn parse_project_context_one_hop_semantic_search_result(
    event: &Event,
    expected_relay: &PublicKey,
    expected: ProjectContextOneHopSemanticHttpRequestObservation<'_>,
) -> Result<ProjectContextOneHopSemanticQueryResult, SdkError> {
    parse_one_hop_semantic_search_result(event, expected_relay, expected, false)
}

/// Verify and parse one Relay-signed filtered one-hop semantic v2 result Event.
pub fn parse_project_context_one_hop_semantic_search_v2_result(
    event: &Event,
    expected_relay: &PublicKey,
    expected: ProjectContextOneHopSemanticHttpRequestObservation<'_>,
) -> Result<ProjectContextOneHopSemanticQueryResult, SdkError> {
    parse_one_hop_semantic_search_result(event, expected_relay, expected, true)
}

fn parse_one_hop_semantic_search_result(
    event: &Event,
    expected_relay: &PublicKey,
    expected: ProjectContextOneHopSemanticHttpRequestObservation<'_>,
    filtered: bool,
) -> Result<ProjectContextOneHopSemanticQueryResult, SdkError> {
    let request = expected
        .request
        .clone()
        .validate_and_canonicalize()
        .map_err(|error| {
            SdkError::InvalidInput(format!(
                "invalid expected Project Context one-hop semantic request: {error}"
            ))
        })?;
    if filtered != request_is_filtered(&request) {
        return Err(SdkError::InvalidInput(
            "one-hop semantic surface does not match the type-filter contract".to_owned(),
        ));
    }
    if request.project_id != *expected.project_id.as_uuid() {
        return Err(SdkError::InvalidInput(
            "expected one-hop request disagrees with the host-derived Project".to_owned(),
        ));
    }
    if expected.exact_authenticated_body.len() > MAX_ONE_HOP_SEMANTIC_EXACT_HTTP_BODY_BYTES {
        return Err(SdkError::ContentTooLarge {
            max: MAX_ONE_HOP_SEMANTIC_EXACT_HTTP_BODY_BYTES,
            got: expected.exact_authenticated_body.len(),
        });
    }

    let response_size = serialized_event_array_size(event)?;
    if response_size > MAX_ONE_HOP_SEMANTIC_RESPONSE_BYTES {
        return Err(SdkError::ContentTooLarge {
            max: MAX_ONE_HOP_SEMANTIC_RESPONSE_BYTES,
            got: response_size,
        });
    }

    event
        .verify()
        .map_err(|error| invalid_projection(format!("invalid event signature: {error}")))?;
    if event.pubkey != *expected_relay {
        return Err(invalid_projection(
            "one-hop semantic result signer does not match the expected Relay identity",
        ));
    }
    if u32::from(event.kind.as_u16()) != KIND_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_RESULT {
        return Err(invalid_projection(format!(
            "one-hop semantic result kind must be {KIND_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_RESULT}"
        )));
    }

    let result = parse_closed_result(event)?;
    require_canonical_content(&event.content, &result)?;
    if filtered != result_is_filtered(&result) {
        return Err(invalid_projection(
            "one-hop semantic result surface does not match the type-filter contract",
        ));
    }
    result
        .validate_for_request(&request)
        .map_err(|error| invalid_projection(format!("invalid one-hop semantic result: {error}")))?;
    if result.project_id != request.project_id
        || result.project_id != *expected.project_id.as_uuid()
    {
        return Err(invalid_projection(
            "one-hop semantic result belongs to a different Project/Community",
        ));
    }

    let caller = expected.authenticated_caller.to_hex();
    let request_id = result.request_id.to_string();
    let request_binding = result.request_binding_digest.to_hex();
    let marker = if filtered {
        PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_V2_RESULT_MARKER
    } else {
        PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_RESULT_MARKER
    };
    require_exact_tags(
        event,
        &[
            vec!["p".to_owned(), caller],
            vec!["request_id".to_owned(), request_id],
            vec!["request_binding".to_owned(), request_binding],
            vec!["t".to_owned(), marker.to_owned()],
        ],
    )?;

    let binding_result = if filtered {
        verify_one_hop_semantic_v2_http_request_binding(
            result.request_binding_digest,
            *expected.project_id.as_uuid(),
            &expected.authenticated_caller.to_bytes(),
            &expected_relay.to_bytes(),
            Digest32::from_bytes(expected.nip98_auth_event_id.to_bytes()),
            expected.exact_authenticated_body,
        )
    } else {
        verify_one_hop_semantic_http_request_binding(
            result.request_binding_digest,
            *expected.project_id.as_uuid(),
            &expected.authenticated_caller.to_bytes(),
            &expected_relay.to_bytes(),
            Digest32::from_bytes(expected.nip98_auth_event_id.to_bytes()),
            expected.exact_authenticated_body,
        )
    };
    binding_result
        .map_err(|_| invalid_projection("one-hop semantic request binding does not match"))?;

    Ok(result)
}

fn request_is_filtered(request: &ProjectContextOneHopSemanticQuery) -> bool {
    matches!(
        request.scope,
        OneHopSemanticScope::EdgeCoordinates {
            coordinate_types: Some(_),
            ..
        }
    )
}

fn result_is_filtered(result: &ProjectContextOneHopSemanticQueryResult) -> bool {
    matches!(
        result.selection,
        OneHopSemanticSelection::EdgeCoordinates {
            coordinate_types: Some(_),
            ..
        }
    )
}

fn parse_closed_result(event: &Event) -> Result<ProjectContextOneHopSemanticQueryResult, SdkError> {
    serde_json::from_str(&event.content)
        .map_err(|error| invalid_projection(format!("invalid one-hop result content: {error}")))
}

fn require_canonical_content(
    content: &str,
    result: &ProjectContextOneHopSemanticQueryResult,
) -> Result<(), SdkError> {
    let canonical = serde_json::to_string(result)
        .map_err(|error| invalid_projection(format!("serialize one-hop result: {error}")))?;
    if content != canonical {
        return Err(invalid_projection(
            "one-hop semantic result content is not the canonical JSON encoding",
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
            "one-hop semantic result tags are not the canonical tag sequence",
        ));
    }
    Ok(())
}

fn serialized_event_array_size(event: &Event) -> Result<usize, SdkError> {
    serde_json::to_vec(std::slice::from_ref(event))
        .map(|bytes| bytes.len())
        .map_err(|error| invalid_projection(format!("serialize one-hop result Event: {error}")))
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
    use buzz_core::kind::{KIND_HTTP_AUTH, KIND_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_RESULT};
    use buzz_core::{CommunityId, Keys};
    use buzz_project_context::{EdgeKey, ProjectContextCoordinate};
    use buzz_project_view::ProjectViewObjectType;
    use buzz_semantic::{
        Digest32, ProjectDocumentSourceBasis, ProjectViewSourceBasis, SemanticLifecycleClass,
        SemanticSourceBasis,
    };
    use buzz_semantic_query::{
        derive_one_hop_semantic_http_request_binding,
        derive_one_hop_semantic_v2_http_request_binding, query_contract_digest,
        EdgeCoordinateCoverage, IncidentEdgeCoverage, OneHopCandidatePreview,
        OneHopCanonicalCandidateObservation, OneHopCanonicalRead, OneHopOmittedCandidateCounts,
        OneHopRankedCoordinate, OneHopRankedDocument, OneHopRankedEdge, OneHopSemanticObservations,
        OneHopSemanticScope, OneHopSemanticSelection, ProjectContextCoordinateType,
        ProjectContextCoordinateTypeFilter, ProjectContextOneHopSemanticQuery,
        ProjectContextOneHopSemanticQueryResult, Score, MAX_ONE_HOP_SEMANTIC_RESPONSE_BYTES,
    };
    use chrono::{TimeZone, Utc};
    use nostr::{Event, EventBuilder, Kind, Tag};
    use uuid::Uuid;

    use super::{
        build_project_context_one_hop_semantic_http_query_request,
        build_project_context_one_hop_semantic_search_result,
        build_project_context_one_hop_semantic_search_v2_result,
        build_project_context_one_hop_semantic_v2_http_query_request,
        parse_project_context_one_hop_semantic_search_result,
        parse_project_context_one_hop_semantic_search_v2_result, request_is_filtered,
        ProjectContextOneHopSemanticHttpRequestObservation,
        PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_RESULT_MARKER,
    };

    fn uuid(seed: u64) -> Uuid {
        Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("UUIDv4 fixture")
    }

    fn digest(seed: u8) -> Digest32 {
        Digest32::from_bytes([seed; 32])
    }

    fn work(seed: u64) -> ProjectContextCoordinate {
        ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Work,
            object_id: uuid(seed),
        }
    }

    fn edge(project_id: Uuid, left: u64, right: u64) -> EdgeKey {
        let mut coordinates = vec![work(left), work(right)];
        coordinates.sort();
        EdgeKey::derive(project_id, &coordinates).expect("Edge fixture")
    }

    fn observations(ranking_contract_digest: Digest32) -> OneHopSemanticObservations {
        OneHopSemanticObservations {
            semantic_generation_id: uuid(9),
            source_generation_contract_digest: digest(10),
            embedding_space_fence: digest(11),
            query_contract_digest: query_contract_digest(),
            ranking_contract_digest,
            projection_generation: 5,
            project_context_revision: 7,
            snapshot_observed_at: Utc
                .timestamp_opt(1_700_000_000, 0)
                .single()
                .expect("timestamp"),
        }
    }

    fn document_observation(
        document_id: Uuid,
        revision: u64,
    ) -> OneHopCanonicalCandidateObservation {
        OneHopCanonicalCandidateObservation {
            source_basis: SemanticSourceBasis::ProjectDocument(ProjectDocumentSourceBasis {
                document_revision: revision,
                source_change_id: digest(20),
            }),
            source_invalidation_epoch: 2,
            source_snapshot_digest: digest(21),
            lifecycle: SemanticLifecycleClass::Active,
            source_status: Some("active".to_owned()),
            preview: OneHopCandidatePreview {
                title: "Authorization relation".to_owned(),
                description: None,
                summary: Some("Client-side authorization evidence".to_owned()),
            },
            canonical_read: OneHopCanonicalRead::Document {
                fetch_command: format!(
                    "cf documents get {document_id} --revision {revision} --content-only"
                ),
                expected_document_revision: revision,
            },
        }
    }

    fn work_observation(
        coordinate: &ProjectContextCoordinate,
    ) -> OneHopCanonicalCandidateObservation {
        let ProjectContextCoordinate::ProjectViewObject {
            object_type,
            object_id,
        } = coordinate
        else {
            panic!("Work fixture")
        };
        OneHopCanonicalCandidateObservation {
            source_basis: SemanticSourceBasis::ProjectView(ProjectViewSourceBasis {
                schema_version: 3,
                object_revision: 7,
                source_change_id: digest(30),
            }),
            source_invalidation_epoch: 3,
            source_snapshot_digest: digest(31),
            lifecycle: SemanticLifecycleClass::Active,
            source_status: Some("active".to_owned()),
            preview: OneHopCandidatePreview {
                title: "Authorization UI".to_owned(),
                description: Some("Client implementation".to_owned()),
                summary: Some("Disclosure-safe errors".to_owned()),
            },
            canonical_read: OneHopCanonicalRead::ProjectView {
                command: format!(
                    "cf project-view get-object {} {object_id}",
                    object_type.as_str()
                ),
                expected_object_revision: 7,
            },
        }
    }

    struct Fixture {
        relay: Keys,
        caller: Keys,
        other: Keys,
        auth: Event,
        request: ProjectContextOneHopSemanticQuery,
        exact_body: Vec<u8>,
        result: ProjectContextOneHopSemanticQueryResult,
        event: Event,
    }

    impl Fixture {
        fn new(scope: OneHopSemanticScope) -> Self {
            let relay = Keys::generate();
            let caller = Keys::generate();
            let other = Keys::generate();
            let request = ProjectContextOneHopSemanticQuery {
                request_id: uuid(1),
                project_id: uuid(2),
                query: "Which authorization relationship is relevant?".to_owned(),
                limit: 2,
                scope,
            };
            let filtered = request_is_filtered(&request);
            let built = if filtered {
                build_project_context_one_hop_semantic_v2_http_query_request(
                    request,
                    &relay.public_key(),
                    &caller.public_key(),
                )
            } else {
                build_project_context_one_hop_semantic_http_query_request(
                    request,
                    &relay.public_key(),
                    &caller.public_key(),
                )
            }
            .expect("request builds");
            let auth = EventBuilder::new(Kind::Custom(KIND_HTTP_AUTH as u16), "POST /query")
                .sign_with_keys(&caller)
                .expect("auth signs");
            let request = built.request;
            let exact_body = built.exact_body;
            let request_binding_digest = if filtered {
                derive_one_hop_semantic_v2_http_request_binding(
                    request.project_id,
                    &caller.public_key().to_bytes(),
                    &relay.public_key().to_bytes(),
                    Digest32::from_bytes(auth.id.to_bytes()),
                    &exact_body,
                )
            } else {
                derive_one_hop_semantic_http_request_binding(
                    request.project_id,
                    &caller.public_key().to_bytes(),
                    &relay.public_key().to_bytes(),
                    Digest32::from_bytes(auth.id.to_bytes()),
                    &exact_body,
                )
            }
            .expect("binding derives");
            let selection = match &request.scope {
                OneHopSemanticScope::IncidentEdges { coordinate } => {
                    let document_id = uuid(40);
                    let document = OneHopRankedDocument {
                        rank: 1,
                        document_id,
                        document_revision: 7,
                        score: Score::new(863_300).expect("score"),
                        canonical_observation: document_observation(document_id, 7),
                    };
                    OneHopSemanticSelection::IncidentEdges {
                        coordinate: coordinate.clone(),
                        edges: vec![OneHopRankedEdge {
                            rank: 1,
                            edge_key: edge(request.project_id, 3, 4),
                            score: document.score,
                            ranked_documents: vec![document],
                            binding_document_count: 1,
                            scorable_document_count: 1,
                            documents_truncated: false,
                        }],
                        coverage: IncidentEdgeCoverage {
                            active_incident_edges: 1,
                            active_relation_bindings: 1,
                            scorable_relation_bindings: 1,
                            scorable_edges: 1,
                            title_only_scorable_bindings: 0,
                            omitted_relation_bindings: OneHopOmittedCandidateCounts::default(),
                        },
                        truncated: false,
                    }
                }
                OneHopSemanticScope::EdgeCoordinates {
                    edge_key,
                    coordinate_types,
                } => {
                    let coordinate = work(3);
                    OneHopSemanticSelection::EdgeCoordinates {
                        edge_key: *edge_key,
                        coordinate_types: coordinate_types.clone(),
                        ranked_coordinates: vec![OneHopRankedCoordinate {
                            rank: 1,
                            coordinate: coordinate.clone(),
                            score: Score::new(841_230).expect("score"),
                            canonical_observation: work_observation(&coordinate),
                        }],
                        coverage: EdgeCoordinateCoverage {
                            edge_coordinate_count: 1,
                            type_matched_coordinate_count: filtered.then_some(1),
                            type_filtered_out_coordinates: filtered.then_some(0),
                            scorable_coordinates: 1,
                            title_only_scorable_coordinates: 0,
                            omitted_coordinates: OneHopOmittedCandidateCounts::default(),
                        },
                        truncated: false,
                    }
                }
            };
            let ranking_contract_digest = request.scope.ranking_contract_digest();
            let result = ProjectContextOneHopSemanticQueryResult {
                request_id: request.request_id,
                project_id: request.project_id,
                request_binding_digest,
                observations: observations(ranking_contract_digest),
                selection,
            };
            let builder = if filtered {
                build_project_context_one_hop_semantic_search_v2_result(
                    &result,
                    &caller.public_key(),
                )
            } else {
                build_project_context_one_hop_semantic_search_result(&result, &caller.public_key())
            }
            .expect("result builds");
            let event = builder.sign_with_keys(&relay).expect("result signs");
            Self {
                relay,
                caller,
                other,
                auth,
                request,
                exact_body,
                result,
                event,
            }
        }

        fn observation(&self) -> ProjectContextOneHopSemanticHttpRequestObservation<'_> {
            ProjectContextOneHopSemanticHttpRequestObservation {
                project_id: CommunityId::from_uuid(self.request.project_id),
                authenticated_caller: self.caller.public_key(),
                request: &self.request,
                nip98_auth_event_id: self.auth.id,
                exact_authenticated_body: &self.exact_body,
            }
        }

        fn sign_content(&self, content: String, tags: Vec<Tag>, kind: u32) -> Event {
            EventBuilder::new(Kind::Custom(kind as u16), content)
                .tags(tags)
                .sign_with_keys(&self.relay)
                .expect("mutated result signs")
        }

        fn canonical_tags(&self) -> Vec<Tag> {
            let caller = self.caller.public_key().to_hex();
            let request_id = self.result.request_id.to_string();
            let request_binding = self.result.request_binding_digest.to_hex();
            vec![
                Tag::parse(["p", caller.as_str()]).expect("caller tag"),
                Tag::parse(["request_id", request_id.as_str()]).expect("request tag"),
                Tag::parse(["request_binding", request_binding.as_str()]).expect("binding tag"),
                Tag::parse(["t", PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_RESULT_MARKER])
                    .expect("marker tag"),
            ]
        }
    }

    #[test]
    fn request_filter_is_exact_closed_and_debug_redacts_query_and_scope() {
        let fixture = Fixture::new(OneHopSemanticScope::IncidentEdges {
            coordinate: work(3),
        });
        let text = String::from_utf8(fixture.exact_body.clone()).expect("UTF-8 body");
        let keys = [
            "\"kinds\"",
            "\"authors\"",
            "\"#p\"",
            "\"limit\"",
            "\"carryforth_project_context_one_hop_semantic_search\"",
        ];
        let mut previous = 0;
        for key in keys {
            let position = text.find(key).expect("canonical filter key");
            assert!(position >= previous);
            previous = position;
        }
        let body: serde_json::Value = serde_json::from_slice(&fixture.exact_body).expect("body");
        assert_eq!(
            body[0]["kinds"][0],
            KIND_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_RESULT
        );
        assert_eq!(body[0]["limit"], 1);
        assert_eq!(
            body[0]["carryforth_project_context_one_hop_semantic_search"]["scope"]["scope_type"],
            "incident_edges"
        );
        let debug = format!(
            "{:?}",
            build_project_context_one_hop_semantic_http_query_request(
                fixture.request.clone(),
                &fixture.relay.public_key(),
                &fixture.caller.public_key(),
            )
            .expect("request")
        );
        assert!(!debug.contains(&fixture.request.query));
        assert!(!debug.contains(&uuid(3).to_string()));
        let observation_debug = format!("{:?}", fixture.observation());
        assert!(!observation_debug.contains(&fixture.request.query));
        assert!(!observation_debug.contains(&fixture.request.project_id.to_string()));
        assert!(!observation_debug.contains(&fixture.caller.public_key().to_hex()));
    }

    #[test]
    fn verifier_accepts_both_closed_variants_with_previews() {
        for fixture in [
            Fixture::new(OneHopSemanticScope::IncidentEdges {
                coordinate: work(3),
            }),
            Fixture::new(OneHopSemanticScope::EdgeCoordinates {
                edge_key: edge(uuid(2), 3, 4),
                coordinate_types: None,
            }),
        ] {
            let parsed = parse_project_context_one_hop_semantic_search_result(
                &fixture.event,
                &fixture.relay.public_key(),
                fixture.observation(),
            )
            .expect("exact transcript verifies");
            assert_eq!(parsed, fixture.result);
            let json = serde_json::to_value(parsed).expect("result JSON");
            assert!(json.to_string().contains("canonical_observation"));
            assert!(json.to_string().contains("preview"));
            match &fixture.request.scope {
                OneHopSemanticScope::IncidentEdges { .. } => {
                    assert!(json["selection"].get("ranked_coordinates").is_none());
                    assert!(json["selection"]["edges"][0].get("coordinates").is_none());
                }
                OneHopSemanticScope::EdgeCoordinates { .. } => {
                    assert!(json["selection"].get("edges").is_none());
                    assert!(json["selection"].get("ranked_documents").is_none());
                }
            }
        }
    }

    #[test]
    fn verifier_rejects_changed_identity_auth_or_exact_body() {
        let fixture = Fixture::new(OneHopSemanticScope::IncidentEdges {
            coordinate: work(3),
        });
        assert!(parse_project_context_one_hop_semantic_search_result(
            &fixture.event,
            &fixture.other.public_key(),
            fixture.observation(),
        )
        .is_err());

        let mut body = fixture.exact_body.clone();
        body.push(b' ');
        let mut observation = fixture.observation();
        observation.exact_authenticated_body = &body;
        assert!(parse_project_context_one_hop_semantic_search_result(
            &fixture.event,
            &fixture.relay.public_key(),
            observation,
        )
        .is_err());

        let mut observation = fixture.observation();
        observation.authenticated_caller = fixture.other.public_key();
        assert!(parse_project_context_one_hop_semantic_search_result(
            &fixture.event,
            &fixture.relay.public_key(),
            observation,
        )
        .is_err());

        let other_auth = EventBuilder::new(Kind::Custom(KIND_HTTP_AUTH as u16), "other auth")
            .sign_with_keys(&fixture.other)
            .expect("other auth signs");
        let mut observation = fixture.observation();
        observation.nip98_auth_event_id = other_auth.id;
        assert!(parse_project_context_one_hop_semantic_search_result(
            &fixture.event,
            &fixture.relay.public_key(),
            observation,
        )
        .is_err());
    }

    #[test]
    fn verifier_rejects_noncanonical_unknown_duplicate_wrong_kind_and_tags() {
        let fixture = Fixture::new(OneHopSemanticScope::EdgeCoordinates {
            edge_key: edge(uuid(2), 3, 4),
            coordinate_types: None,
        });
        let canonical = serde_json::to_string(&fixture.result).expect("canonical result");

        let pretty = serde_json::to_string_pretty(&fixture.result).expect("pretty result");
        assert!(parse_project_context_one_hop_semantic_search_result(
            &fixture.sign_content(
                pretty,
                fixture.canonical_tags(),
                KIND_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_RESULT,
            ),
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .is_err());

        let mut unknown = serde_json::to_value(&fixture.result).expect("result value");
        unknown
            .as_object_mut()
            .expect("result object")
            .insert("raw_event".to_owned(), serde_json::json!({"leak": true}));
        assert!(parse_project_context_one_hop_semantic_search_result(
            &fixture.sign_content(
                unknown.to_string(),
                fixture.canonical_tags(),
                KIND_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_RESULT,
            ),
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .is_err());

        let duplicate = canonical.replacen(
            '{',
            &format!("{{\"request_id\":\"{}\",", fixture.result.request_id),
            1,
        );
        assert!(parse_project_context_one_hop_semantic_search_result(
            &fixture.sign_content(
                duplicate,
                fixture.canonical_tags(),
                KIND_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_RESULT,
            ),
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .is_err());

        assert!(parse_project_context_one_hop_semantic_search_result(
            &fixture.sign_content(canonical.clone(), fixture.canonical_tags(), 40913),
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .is_err());

        let mut tags = fixture.canonical_tags();
        tags.swap(0, 1);
        assert!(parse_project_context_one_hop_semantic_search_result(
            &fixture.sign_content(
                canonical,
                tags,
                KIND_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_RESULT,
            ),
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .is_err());
    }

    #[test]
    fn result_builder_rejects_response_over_cap_without_logging_preview() {
        let fixture = Fixture::new(OneHopSemanticScope::IncidentEdges {
            coordinate: work(3),
        });
        let mut result = fixture.result;
        let OneHopSemanticSelection::IncidentEdges { edges, .. } = &mut result.selection else {
            panic!("incident result")
        };
        edges[0].ranked_documents[0]
            .canonical_observation
            .preview
            .summary = Some("s".repeat(MAX_ONE_HOP_SEMANTIC_RESPONSE_BYTES));
        assert!(matches!(
            build_project_context_one_hop_semantic_search_result(
                &result,
                &fixture.caller.public_key(),
            ),
            Err(crate::SdkError::ContentTooLarge { .. })
        ));
        let debug = format!("{result:?}");
        assert!(!debug.contains(&"s".repeat(128)));
    }

    #[test]
    fn filtered_edge_coordinates_use_the_separate_v2_surface() {
        let filter =
            ProjectContextCoordinateTypeFilter::new(vec![ProjectContextCoordinateType::Work])
                .expect("filter");
        let fixture = Fixture::new(OneHopSemanticScope::EdgeCoordinates {
            edge_key: edge(uuid(2), 3, 4),
            coordinate_types: Some(filter),
        });
        let body: serde_json::Value = serde_json::from_slice(&fixture.exact_body).expect("body");
        assert!(body[0]
            .get("carryforth_project_context_one_hop_semantic_search_v2")
            .is_some());
        assert!(body[0]
            .get("carryforth_project_context_one_hop_semantic_search")
            .is_none());
        assert!(build_project_context_one_hop_semantic_http_query_request(
            fixture.request.clone(),
            &fixture.relay.public_key(),
            &fixture.caller.public_key(),
        )
        .is_err());
        let parsed = parse_project_context_one_hop_semantic_search_v2_result(
            &fixture.event,
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .expect("v2 verifies");
        assert_eq!(parsed, fixture.result);
        assert!(parse_project_context_one_hop_semantic_search_result(
            &fixture.event,
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .is_err());
        let OneHopSemanticSelection::EdgeCoordinates { coverage, .. } = parsed.selection else {
            panic!("Edge Coordinate result")
        };
        assert_eq!(coverage.type_matched_coordinate_count, Some(1));
        assert_eq!(coverage.type_filtered_out_coordinates, Some(0));
    }
}
