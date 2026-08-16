//! Relay-only virtual Event support for Project Context Coordinate search.
//!
//! Coordinate search is an Agent starting-point discovery surface. It returns
//! only ranked canonical Coordinates; it does not return graph Edges, paths,
//! source previews, or read commands. These helpers perform no network or
//! persistence work.

use buzz_core::kind::KIND_PROJECT_CONTEXT_COORDINATE_SEARCH_RESULT;
use buzz_core::{CommunityId, EventId, PublicKey};
use buzz_semantic::Digest32;
use buzz_semantic_query::{
    verify_coordinate_search_http_request_binding,
    verify_coordinate_search_v2_http_request_binding, ProjectContextCoordinateSearchQuery,
    ProjectContextCoordinateSearchResult, MAX_COORDINATE_SEARCH_RESPONSE_BYTES,
};
use nostr::{Event, EventBuilder, Kind, Tag};
use serde::Serialize;

use crate::SdkError;

/// Exact marker carried by Coordinate-search result Events.
pub const PROJECT_CONTEXT_COORDINATE_SEARCH_RESULT_MARKER: &str =
    "carryforth-project-context-coordinate-search-result";
/// Exact marker carried by filtered Coordinate-search v2 result Events.
pub const PROJECT_CONTEXT_COORDINATE_SEARCH_V2_RESULT_MARKER: &str =
    "carryforth-project-context-coordinate-search-result-v2";

/// Canonical Coordinate-search HTTP request and exact serialized body.
///
/// `exact_body` is the only body that may be sent to `POST /query` and hashed
/// into the NIP-98 authentication Event. Re-serializing [`Self::request`] after
/// authentication would invalidate the result binding.
#[derive(Clone)]
pub struct ProjectContextCoordinateSearchHttpQueryRequest {
    /// Validated request embedded in the exclusive query filter.
    pub request: ProjectContextCoordinateSearchQuery,
    /// Byte-for-byte JSON body authenticated by NIP-98.
    pub exact_body: Vec<u8>,
}

impl std::fmt::Debug for ProjectContextCoordinateSearchHttpQueryRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProjectContextCoordinateSearchHttpQueryRequest")
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
struct ProjectContextCoordinateSearchFilter<'a> {
    kinds: [u32; 1],
    authors: [String; 1],
    #[serde(rename = "#p")]
    caller: [String; 1],
    limit: u8,
    carryforth_project_context_coordinate_search: &'a ProjectContextCoordinateSearchQuery,
}

#[derive(Serialize)]
struct ProjectContextCoordinateSearchV2Filter<'a> {
    kinds: [u32; 1],
    authors: [String; 1],
    #[serde(rename = "#p")]
    caller: [String; 1],
    limit: u8,
    carryforth_project_context_coordinate_search_v2: &'a ProjectContextCoordinateSearchQuery,
}

/// Validate and serialize one exclusive Coordinate-search `/query` filter.
///
/// The exact filter keys are `kinds`, `authors`, `#p`, `limit`, and
/// `carryforth_project_context_coordinate_search`.
pub fn build_project_context_coordinate_search_http_query_request(
    request: ProjectContextCoordinateSearchQuery,
    expected_relay: &PublicKey,
    authenticated_caller: &PublicKey,
) -> Result<ProjectContextCoordinateSearchHttpQueryRequest, SdkError> {
    build_coordinate_search_http_query_request(request, expected_relay, authenticated_caller, false)
}

/// Validate and serialize one exclusive filtered Coordinate-search v2 request.
pub fn build_project_context_coordinate_search_v2_http_query_request(
    request: ProjectContextCoordinateSearchQuery,
    expected_relay: &PublicKey,
    authenticated_caller: &PublicKey,
) -> Result<ProjectContextCoordinateSearchHttpQueryRequest, SdkError> {
    build_coordinate_search_http_query_request(request, expected_relay, authenticated_caller, true)
}

fn build_coordinate_search_http_query_request(
    request: ProjectContextCoordinateSearchQuery,
    expected_relay: &PublicKey,
    authenticated_caller: &PublicKey,
    filtered: bool,
) -> Result<ProjectContextCoordinateSearchHttpQueryRequest, SdkError> {
    let request = request.validate_and_canonicalize().map_err(|error| {
        SdkError::InvalidInput(format!(
            "invalid Project Context Coordinate search: {error}"
        ))
    })?;
    if filtered != request.coordinate_types.is_some() {
        return Err(SdkError::InvalidInput(
            "Coordinate-search surface does not match the type-filter contract".to_owned(),
        ));
    }
    let kinds = [KIND_PROJECT_CONTEXT_COORDINATE_SEARCH_RESULT];
    let authors = [expected_relay.to_hex()];
    let caller = [authenticated_caller.to_hex()];
    let exact_body = if filtered {
        serde_json::to_vec(&[ProjectContextCoordinateSearchV2Filter {
            kinds,
            authors,
            caller,
            limit: 1,
            carryforth_project_context_coordinate_search_v2: &request,
        }])
    } else {
        serde_json::to_vec(&[ProjectContextCoordinateSearchFilter {
            kinds,
            authors,
            caller,
            limit: 1,
            carryforth_project_context_coordinate_search: &request,
        }])
    };
    let exact_body = exact_body.map_err(|error| {
        SdkError::InvalidInput(format!(
            "serialize Project Context Coordinate search: {error}"
        ))
    })?;
    Ok(ProjectContextCoordinateSearchHttpQueryRequest {
        request,
        exact_body,
    })
}

/// Authenticated HTTP transcript expected by a Coordinate-search verifier.
///
/// `project_id` must come from the verified request host. The exact body must
/// be the bytes covered by the NIP-98 authentication Event.
#[derive(Clone, Copy)]
pub struct ProjectContextCoordinateSearchHttpRequestObservation<'a> {
    /// Host-derived Community/Project identity.
    pub project_id: CommunityId,
    /// Authenticated caller required in the exact `p` tag.
    pub authenticated_caller: PublicKey,
    /// Canonical request sent in the filter extension.
    pub request: &'a ProjectContextCoordinateSearchQuery,
    /// NIP-98 Event identity for this one HTTP request.
    pub nip98_auth_event_id: EventId,
    /// Exact authenticated HTTP body.
    pub exact_authenticated_body: &'a [u8],
}

impl std::fmt::Debug for ProjectContextCoordinateSearchHttpRequestObservation<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProjectContextCoordinateSearchHttpRequestObservation")
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

/// Build the unsigned response-only Event for a validated result.
///
/// The Relay must sign this builder only after current authorization and
/// release checks pass. The Event must never enter ordinary ingest, storage,
/// search, fan-out, REQ, or COUNT paths.
pub fn build_project_context_coordinate_search_result(
    result: &ProjectContextCoordinateSearchResult,
    authenticated_caller: &PublicKey,
) -> Result<EventBuilder, SdkError> {
    build_coordinate_search_result(result, authenticated_caller, false)
}

/// Build the unsigned response-only Event for a filtered Coordinate-search v2 result.
pub fn build_project_context_coordinate_search_v2_result(
    result: &ProjectContextCoordinateSearchResult,
    authenticated_caller: &PublicKey,
) -> Result<EventBuilder, SdkError> {
    build_coordinate_search_result(result, authenticated_caller, true)
}

fn build_coordinate_search_result(
    result: &ProjectContextCoordinateSearchResult,
    authenticated_caller: &PublicKey,
    filtered: bool,
) -> Result<EventBuilder, SdkError> {
    result.validate().map_err(|error| {
        SdkError::InvalidInput(format!("invalid Coordinate-search result: {error}"))
    })?;
    if filtered != result.observations.coordinate_types.is_some() {
        return Err(SdkError::InvalidInput(
            "Coordinate-search result surface does not match the type-filter contract".to_owned(),
        ));
    }
    let content = canonical_json(result, "serialize Coordinate-search result")?;
    if content.len() > MAX_COORDINATE_SEARCH_RESPONSE_BYTES {
        return Err(SdkError::ContentTooLarge {
            max: MAX_COORDINATE_SEARCH_RESPONSE_BYTES,
            got: content.len(),
        });
    }

    let caller = authenticated_caller.to_hex();
    let request_id = result.request_id.to_string();
    let request_binding = result.request_binding_digest.to_hex();
    let marker = if filtered {
        PROJECT_CONTEXT_COORDINATE_SEARCH_V2_RESULT_MARKER
    } else {
        PROJECT_CONTEXT_COORDINATE_SEARCH_RESULT_MARKER
    };
    Ok(EventBuilder::new(
        Kind::Custom(KIND_PROJECT_CONTEXT_COORDINATE_SEARCH_RESULT as u16),
        content,
    )
    .tags([
        tag(["p", caller.as_str()])?,
        tag(["request_id", request_id.as_str()])?,
        tag(["request_binding", request_binding.as_str()])?,
        tag(["t", marker])?,
    ]))
}

/// Verify and parse one Relay-signed Coordinate-search result Event.
///
/// Verification covers the Schnorr signature, Relay/caller/Project identity,
/// exact kind and tag sequence, canonical closed result content, the NIP-98
/// body binding, ranking invariants, and the fixed response byte cap.
pub fn parse_project_context_coordinate_search_result(
    event: &Event,
    expected_relay: &PublicKey,
    expected: ProjectContextCoordinateSearchHttpRequestObservation<'_>,
) -> Result<ProjectContextCoordinateSearchResult, SdkError> {
    parse_coordinate_search_result(event, expected_relay, expected, false)
}

/// Verify and parse one Relay-signed filtered Coordinate-search v2 result Event.
pub fn parse_project_context_coordinate_search_v2_result(
    event: &Event,
    expected_relay: &PublicKey,
    expected: ProjectContextCoordinateSearchHttpRequestObservation<'_>,
) -> Result<ProjectContextCoordinateSearchResult, SdkError> {
    parse_coordinate_search_result(event, expected_relay, expected, true)
}

fn parse_coordinate_search_result(
    event: &Event,
    expected_relay: &PublicKey,
    expected: ProjectContextCoordinateSearchHttpRequestObservation<'_>,
    filtered: bool,
) -> Result<ProjectContextCoordinateSearchResult, SdkError> {
    let request = expected
        .request
        .clone()
        .validate_and_canonicalize()
        .map_err(|error| {
            SdkError::InvalidInput(format!("invalid expected Coordinate search: {error}"))
        })?;
    if filtered != request.coordinate_types.is_some() {
        return Err(SdkError::InvalidInput(
            "Coordinate-search surface does not match the type-filter contract".to_owned(),
        ));
    }
    if request.project_id != *expected.project_id.as_uuid() {
        return Err(SdkError::InvalidInput(
            "expected Coordinate-search request disagrees with the host-derived Project".to_owned(),
        ));
    }

    let response_size = serialized_event_array_size(event)?;
    if response_size > MAX_COORDINATE_SEARCH_RESPONSE_BYTES {
        return Err(SdkError::ContentTooLarge {
            max: MAX_COORDINATE_SEARCH_RESPONSE_BYTES,
            got: response_size,
        });
    }

    event
        .verify()
        .map_err(|error| invalid_projection(format!("invalid event signature: {error}")))?;
    if event.pubkey != *expected_relay {
        return Err(invalid_projection(
            "Coordinate-search result signer does not match the expected Relay identity",
        ));
    }
    if u32::from(event.kind.as_u16()) != KIND_PROJECT_CONTEXT_COORDINATE_SEARCH_RESULT {
        return Err(invalid_projection(format!(
            "Coordinate-search result kind must be {KIND_PROJECT_CONTEXT_COORDINATE_SEARCH_RESULT}"
        )));
    }

    let result = parse_closed_result(event)?;
    require_canonical_content(&event.content, &result)?;
    if filtered != result.observations.coordinate_types.is_some() {
        return Err(invalid_projection(
            "Coordinate-search result surface does not match the type-filter contract",
        ));
    }
    result.validate_for_request(&request).map_err(|error| {
        invalid_projection(format!("invalid Coordinate-search result: {error}"))
    })?;
    if result.request_id != request.request_id {
        return Err(invalid_projection(
            "Coordinate-search result belongs to a different request",
        ));
    }
    if result.project_id != request.project_id
        || result.project_id != *expected.project_id.as_uuid()
    {
        return Err(invalid_projection(
            "Coordinate-search result belongs to a different Project/Community",
        ));
    }

    let caller = expected.authenticated_caller.to_hex();
    let request_id = result.request_id.to_string();
    let request_binding = result.request_binding_digest.to_hex();
    let marker = if filtered {
        PROJECT_CONTEXT_COORDINATE_SEARCH_V2_RESULT_MARKER
    } else {
        PROJECT_CONTEXT_COORDINATE_SEARCH_RESULT_MARKER
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
        verify_coordinate_search_v2_http_request_binding(
            result.request_binding_digest,
            *expected.project_id.as_uuid(),
            &expected.authenticated_caller.to_bytes(),
            Digest32::from_bytes(expected.nip98_auth_event_id.to_bytes()),
            expected.exact_authenticated_body,
        )
    } else {
        verify_coordinate_search_http_request_binding(
            result.request_binding_digest,
            *expected.project_id.as_uuid(),
            &expected.authenticated_caller.to_bytes(),
            Digest32::from_bytes(expected.nip98_auth_event_id.to_bytes()),
            expected.exact_authenticated_body,
        )
    };
    binding_result
        .map_err(|_| invalid_projection("Coordinate-search request binding does not match"))?;

    Ok(result)
}

fn parse_closed_result(event: &Event) -> Result<ProjectContextCoordinateSearchResult, SdkError> {
    serde_json::from_str(&event.content).map_err(|error| {
        invalid_projection(format!("invalid Coordinate-search result content: {error}"))
    })
}

fn require_canonical_content(
    content: &str,
    result: &ProjectContextCoordinateSearchResult,
) -> Result<(), SdkError> {
    let canonical = serde_json::to_string(result).map_err(|error| {
        invalid_projection(format!("serialize Coordinate-search result: {error}"))
    })?;
    if content != canonical {
        return Err(invalid_projection(
            "Coordinate-search result content is not the canonical JSON encoding",
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
            "Coordinate-search result tags are not the canonical tag sequence",
        ));
    }
    Ok(())
}

fn serialized_event_array_size(event: &Event) -> Result<usize, SdkError> {
    serde_json::to_vec(std::slice::from_ref(event))
        .map(|bytes| bytes.len())
        .map_err(|error| {
            invalid_projection(format!("serialize Coordinate-search result Event: {error}"))
        })
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
    use buzz_core::kind::{KIND_HTTP_AUTH, KIND_PROJECT_CONTEXT_COORDINATE_SEARCH_RESULT};
    use buzz_core::{CommunityId, Keys};
    use buzz_project_context::ProjectContextCoordinate;
    use buzz_project_view::ProjectViewObjectType;
    use buzz_semantic::Digest32;
    use buzz_semantic_query::{
        coordinate_search_query_contract_digest, derive_coordinate_search_http_request_binding,
        derive_coordinate_search_v2_http_request_binding, ProjectContextCoordinateSearchCandidate,
        ProjectContextCoordinateSearchObservations, ProjectContextCoordinateSearchQuery,
        ProjectContextCoordinateSearchResult, ProjectContextCoordinateType,
        ProjectContextCoordinateTypeFilter, Score,
    };
    use chrono::{TimeZone, Utc};
    use nostr::{Event, EventBuilder, Kind, Tag};
    use uuid::Uuid;

    use super::{
        build_project_context_coordinate_search_http_query_request,
        build_project_context_coordinate_search_result,
        build_project_context_coordinate_search_v2_http_query_request,
        build_project_context_coordinate_search_v2_result,
        parse_project_context_coordinate_search_result,
        parse_project_context_coordinate_search_v2_result,
        ProjectContextCoordinateSearchHttpRequestObservation,
    };

    fn uuid(seed: u64) -> Uuid {
        Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("UUIDv4 fixture")
    }

    fn coordinate(seed: u64) -> ProjectContextCoordinate {
        ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Work,
            object_id: uuid(seed),
        }
    }

    struct Fixture {
        relay: Keys,
        caller: Keys,
        other: Keys,
        auth: Event,
        request: ProjectContextCoordinateSearchQuery,
        exact_body: Vec<u8>,
        result: ProjectContextCoordinateSearchResult,
        event: Event,
    }

    impl Fixture {
        fn new() -> Self {
            let relay = Keys::generate();
            let caller = Keys::generate();
            let other = Keys::generate();
            let request = ProjectContextCoordinateSearchQuery {
                request_id: Uuid::new_v4(),
                project_id: Uuid::new_v4(),
                query: "Which work is closest to the release task?".to_owned(),
                coordinate_types: None,
                limit: 2,
            };
            let built = build_project_context_coordinate_search_http_query_request(
                request,
                &relay.public_key(),
                &caller.public_key(),
            )
            .expect("request builds");
            let auth =
                EventBuilder::new(Kind::Custom(KIND_HTTP_AUTH as u16), "POST /query fixture")
                    .sign_with_keys(&caller)
                    .expect("auth signs");
            let request = built.request;
            let exact_body = built.exact_body;
            let request_binding_digest = derive_coordinate_search_http_request_binding(
                request.project_id,
                &caller.public_key().to_bytes(),
                Digest32::from_bytes(auth.id.to_bytes()),
                &exact_body,
            )
            .expect("binding derives");
            let result = ProjectContextCoordinateSearchResult {
                request_id: request.request_id,
                project_id: request.project_id,
                request_binding_digest,
                observations: ProjectContextCoordinateSearchObservations {
                    semantic_generation_id: uuid(9),
                    embedding_space_fence: Digest32::from_bytes([1; 32]),
                    query_contract_digest: coordinate_search_query_contract_digest(),
                    coordinate_types: None,
                    projection_generation: 5,
                    project_context_revision: 7,
                    snapshot_observed_at: Utc
                        .timestamp_opt(1_700_000_000, 0)
                        .single()
                        .expect("timestamp"),
                },
                coordinates: vec![
                    ProjectContextCoordinateSearchCandidate {
                        rank: 1,
                        coordinate: coordinate(1),
                        score: Score::new(900_000).expect("score"),
                    },
                    ProjectContextCoordinateSearchCandidate {
                        rank: 2,
                        coordinate: coordinate(2),
                        score: Score::new(800_000).expect("score"),
                    },
                ],
                truncated: true,
            };
            let event =
                build_project_context_coordinate_search_result(&result, &caller.public_key())
                    .expect("result builds")
                    .sign_with_keys(&relay)
                    .expect("result signs");
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

        fn observation(&self) -> ProjectContextCoordinateSearchHttpRequestObservation<'_> {
            ProjectContextCoordinateSearchHttpRequestObservation {
                project_id: CommunityId::from_uuid(self.request.project_id),
                authenticated_caller: self.caller.public_key(),
                request: &self.request,
                nip98_auth_event_id: self.auth.id,
                exact_authenticated_body: &self.exact_body,
            }
        }

        fn sign_content(&self, content: String, append_extra_tag: bool) -> Event {
            let caller = self.caller.public_key().to_hex();
            let request_id = self.result.request_id.to_string();
            let request_binding = self.result.request_binding_digest.to_hex();
            let mut tags = vec![
                Tag::parse(["p", caller.as_str()]).expect("caller tag"),
                Tag::parse(["request_id", request_id.as_str()]).expect("request tag"),
                Tag::parse(["request_binding", request_binding.as_str()]).expect("binding tag"),
                Tag::parse(["t", super::PROJECT_CONTEXT_COORDINATE_SEARCH_RESULT_MARKER])
                    .expect("marker tag"),
            ];
            if append_extra_tag {
                tags.push(Tag::parse(["x", "unexpected"]).expect("extra tag"));
            }
            EventBuilder::new(
                Kind::Custom(KIND_PROJECT_CONTEXT_COORDINATE_SEARCH_RESULT as u16),
                content,
            )
            .tags(tags)
            .sign_with_keys(&self.relay)
            .expect("mutated result signs")
        }
    }

    #[test]
    fn request_filter_is_exact_canonical_and_debug_redacts_query() {
        let fixture = Fixture::new();
        let value: serde_json::Value =
            serde_json::from_slice(&fixture.exact_body).expect("body parses");
        assert_eq!(
            value[0]["kinds"][0],
            KIND_PROJECT_CONTEXT_COORDINATE_SEARCH_RESULT
        );
        assert_eq!(value[0]["limit"], 1);
        assert!(value[0]
            .get("carryforth_project_context_coordinate_search")
            .is_some());
        let debug = format!(
            "{:?}",
            build_project_context_coordinate_search_http_query_request(
                fixture.request.clone(),
                &fixture.relay.public_key(),
                &fixture.caller.public_key(),
            )
            .expect("request builds")
        );
        assert!(!debug.contains(&fixture.request.query));
    }

    #[test]
    fn verifier_accepts_exact_transcript_and_rejects_identity_or_body_changes() {
        let fixture = Fixture::new();
        let parsed = parse_project_context_coordinate_search_result(
            &fixture.event,
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .expect("valid transcript verifies");
        assert_eq!(parsed, fixture.result);

        assert!(parse_project_context_coordinate_search_result(
            &fixture.event,
            &fixture.other.public_key(),
            fixture.observation(),
        )
        .is_err());
        let mut body = fixture.exact_body.clone();
        body.push(b' ');
        let mut observation = fixture.observation();
        observation.exact_authenticated_body = &body;
        assert!(parse_project_context_coordinate_search_result(
            &fixture.event,
            &fixture.relay.public_key(),
            observation,
        )
        .is_err());

        let mut observation = fixture.observation();
        observation.authenticated_caller = fixture.other.public_key();
        assert!(parse_project_context_coordinate_search_result(
            &fixture.event,
            &fixture.relay.public_key(),
            observation,
        )
        .is_err());
    }

    #[test]
    fn verifier_rejects_noncanonical_closed_or_malformed_signed_results() {
        let fixture = Fixture::new();

        let pretty = serde_json::to_string_pretty(&fixture.result).expect("pretty result");
        assert!(parse_project_context_coordinate_search_result(
            &fixture.sign_content(pretty, false),
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .is_err());

        let mut unknown = serde_json::to_value(&fixture.result).expect("result value");
        unknown
            .as_object_mut()
            .expect("result object")
            .insert("preview".to_owned(), serde_json::json!({"title": "leak"}));
        assert!(parse_project_context_coordinate_search_result(
            &fixture.sign_content(unknown.to_string(), false),
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .is_err());

        let mut invalid_rank = fixture.result.clone();
        invalid_rank.coordinates[1].rank = 1;
        let invalid_rank = serde_json::to_string(&invalid_rank).expect("invalid ranked result");
        assert!(parse_project_context_coordinate_search_result(
            &fixture.sign_content(invalid_rank, false),
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .is_err());

        let canonical = serde_json::to_string(&fixture.result).expect("canonical result");
        assert!(parse_project_context_coordinate_search_result(
            &fixture.sign_content(canonical, true),
            &fixture.relay.public_key(),
            fixture.observation(),
        )
        .is_err());
    }

    #[test]
    fn filtered_v2_is_a_separate_bound_surface_and_v1_remains_closed() {
        let relay = Keys::generate();
        let caller = Keys::generate();
        let filter =
            ProjectContextCoordinateTypeFilter::new(vec![ProjectContextCoordinateType::Work])
                .expect("filter");
        let request = ProjectContextCoordinateSearchQuery {
            request_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            query: "Find the frontend work".to_owned(),
            coordinate_types: Some(filter.clone()),
            limit: 1,
        };
        assert!(build_project_context_coordinate_search_http_query_request(
            request.clone(),
            &relay.public_key(),
            &caller.public_key(),
        )
        .is_err());
        let built = build_project_context_coordinate_search_v2_http_query_request(
            request,
            &relay.public_key(),
            &caller.public_key(),
        )
        .expect("v2 request");
        let body: serde_json::Value = serde_json::from_slice(&built.exact_body).expect("body");
        assert!(body[0]
            .get("carryforth_project_context_coordinate_search_v2")
            .is_some());
        assert!(body[0]
            .get("carryforth_project_context_coordinate_search")
            .is_none());
        let auth = EventBuilder::new(Kind::Custom(KIND_HTTP_AUTH as u16), "POST /query")
            .sign_with_keys(&caller)
            .expect("auth");
        let binding = derive_coordinate_search_v2_http_request_binding(
            built.request.project_id,
            &caller.public_key().to_bytes(),
            Digest32::from_bytes(auth.id.to_bytes()),
            &built.exact_body,
        )
        .expect("binding");
        let result = ProjectContextCoordinateSearchResult {
            request_id: built.request.request_id,
            project_id: built.request.project_id,
            request_binding_digest: binding,
            observations: ProjectContextCoordinateSearchObservations {
                semantic_generation_id: uuid(9),
                embedding_space_fence: Digest32::from_bytes([1; 32]),
                query_contract_digest: coordinate_search_query_contract_digest(),
                coordinate_types: Some(filter),
                projection_generation: 5,
                project_context_revision: 7,
                snapshot_observed_at: Utc
                    .timestamp_opt(1_700_000_000, 0)
                    .single()
                    .expect("timestamp"),
            },
            coordinates: vec![ProjectContextCoordinateSearchCandidate {
                rank: 1,
                coordinate: coordinate(1),
                score: Score::new(900_000).expect("score"),
            }],
            truncated: false,
        };
        assert!(
            build_project_context_coordinate_search_result(&result, &caller.public_key()).is_err()
        );
        let event =
            build_project_context_coordinate_search_v2_result(&result, &caller.public_key())
                .expect("result")
                .sign_with_keys(&relay)
                .expect("sign");
        let observation = ProjectContextCoordinateSearchHttpRequestObservation {
            project_id: CommunityId::from_uuid(built.request.project_id),
            authenticated_caller: caller.public_key(),
            request: &built.request,
            nip98_auth_event_id: auth.id,
            exact_authenticated_body: &built.exact_body,
        };
        assert_eq!(
            parse_project_context_coordinate_search_v2_result(
                &event,
                &relay.public_key(),
                observation,
            )
            .expect("v2 verifies"),
            result
        );
        assert!(parse_project_context_coordinate_search_result(
            &event,
            &relay.public_key(),
            observation,
        )
        .is_err());
    }
}
