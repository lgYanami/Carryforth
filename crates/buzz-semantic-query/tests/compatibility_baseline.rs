use buzz_project_context::{EdgeKey, ProjectContextCoordinate};
use buzz_project_view::ProjectViewObjectType;
use buzz_semantic::Digest32;
use buzz_semantic_query::{
    budget_profile_digest, build_coordinate_search_encoder_input,
    build_one_hop_semantic_query_encoder_input, build_query_encoder_inputs, candidate_score,
    document_score, edge_coordinate_ranking_contract_digest, embedding_space_fence,
    environment_gain, harmonic_score, incident_edge_ranking_contract_digest, path_score,
    query_contract_digest, ranking_contract_digest, target_coordinate_score, AnchorGain,
    ConditionedContextOverview, ConditionedEvidence, DeterministicFakeQueryEncoder,
    EncodedCoordinateSearchQuery, LifecycleFilter, OneHopSemanticScope,
    ProjectContextCoordinateSearchQuery, ProjectContextOneHopSemanticQuery, Score,
    SemanticGraphQuery, SemanticGraphQueryBudget, SemanticQueryEncoder,
    DEFAULT_COORDINATE_SEARCH_LIMIT, DEFAULT_ONE_HOP_SEMANTIC_LIMIT,
};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

const FIXTURE_ID: &str = "carryforth.semantic-retrieval-compatibility.v1";
const SYNTHETIC_PROBLEM: &str = "locate authorization failure context";
const SYNTHETIC_CONTEXT_OVERVIEW: &str =
    "type: Work\ntitle: Client authorization\nsummary: Verify disclosure-safe failure handling";
const LEGACY_FLEET_RUNTIME_DIGEST: &str =
    "325238245fe41d6e7916fa369c539aa35ac789a0f2d9c8d7c4275fba8f360bbe";

fn uuid(value: u128) -> Uuid {
    Uuid::from_u128(0x123e_4567_e89b_42d3_a456_4266_0000_0000 | value)
}

fn project_view_coordinate(
    object_type: ProjectViewObjectType,
    value: u128,
) -> ProjectContextCoordinate {
    ProjectContextCoordinate::ProjectViewObject {
        object_type,
        object_id: uuid(value),
    }
}

fn vector_digest(values: &[f32]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"carryforth.semantic-retrieval-compatibility-vector-v1");
    hasher.update((values.len() as u64).to_be_bytes());
    for value in values {
        hasher.update(value.to_bits().to_be_bytes());
    }
    Digest32::from_bytes(hasher.finalize().into()).to_hex()
}

fn score(value: u32) -> Score {
    Score::new(value).expect("fixed score must be valid")
}

async fn manifest() -> Value {
    let request_id = uuid(1);
    let project_id = uuid(2);
    let role = project_view_coordinate(ProjectViewObjectType::Role, 3);
    let work = project_view_coordinate(ProjectViewObjectType::Work, 4);
    let issue = project_view_coordinate(ProjectViewObjectType::Issue, 5);
    let mut edge_coordinates = vec![role.clone(), work.clone(), issue.clone()];
    edge_coordinates.sort();
    let edge_key =
        EdgeKey::derive(project_id, &edge_coordinates).expect("synthetic Edge must be valid");

    let coordinate_request = ProjectContextCoordinateSearchQuery {
        request_id,
        project_id,
        query: format!("  {SYNTHETIC_PROBLEM}  "),
        limit: DEFAULT_COORDINATE_SEARCH_LIMIT,
    }
    .validate_and_canonicalize()
    .expect("Coordinate request");
    let coordinate_input =
        build_coordinate_search_encoder_input(&coordinate_request).expect("Coordinate input");

    let incident_request = ProjectContextOneHopSemanticQuery {
        request_id,
        project_id,
        query: SYNTHETIC_PROBLEM.to_owned(),
        limit: DEFAULT_ONE_HOP_SEMANTIC_LIMIT,
        scope: OneHopSemanticScope::IncidentEdges {
            coordinate: work.clone(),
        },
    }
    .validate_and_canonicalize()
    .expect("incident request");
    let incident_input =
        build_one_hop_semantic_query_encoder_input(&incident_request).expect("incident Q0");

    let edge_coordinate_request = ProjectContextOneHopSemanticQuery {
        request_id,
        project_id,
        query: SYNTHETIC_PROBLEM.to_owned(),
        limit: DEFAULT_ONE_HOP_SEMANTIC_LIMIT,
        scope: OneHopSemanticScope::EdgeCoordinates { edge_key },
    }
    .validate_and_canonicalize()
    .expect("Edge Coordinate request");
    let edge_coordinate_input =
        build_one_hop_semantic_query_encoder_input(&edge_coordinate_request).expect("Edge Q0");

    assert_eq!(incident_input.text(), edge_coordinate_input.text());
    assert_eq!(
        incident_input.text_digest(),
        edge_coordinate_input.text_digest()
    );

    let graph_request = SemanticGraphQuery {
        request_id,
        project_id,
        problem: SYNTHETIC_PROBLEM.to_owned(),
        initial_coordinates: vec![role.clone()],
        context_coordinates: vec![work.clone()],
        lifecycle_filter: LifecycleFilter::AllCurrent,
        budget: SemanticGraphQueryBudget::default(),
    }
    .validate_and_canonicalize()
    .expect("graph request");
    let graph_inputs = build_query_encoder_inputs(
        &graph_request,
        &[ConditionedContextOverview {
            coordinate: work.clone(),
            current_overview_semantic_text: SYNTHETIC_CONTEXT_OVERVIEW.to_owned(),
        }],
    )
    .expect("Q0 and Qi")
    .inputs;
    assert_eq!(graph_inputs.len(), 2);
    assert_eq!(incident_input.text(), graph_inputs[0].text());
    assert_eq!(incident_input.text_digest(), graph_inputs[0].text_digest());

    let encoder = DeterministicFakeQueryEncoder::new(16).expect("fake encoder");
    let encoded_graph = encoder
        .encode_queries(&graph_inputs)
        .await
        .expect("fake Q0 and Qi");
    assert_eq!(encoded_graph.len(), graph_inputs.len());

    let mut coordinate_values = vec![0.0_f32; encoder.source_contract().dimensions];
    coordinate_values[0] = 1.0;
    for (index, value) in coordinate_values.iter_mut().enumerate().skip(1) {
        *value = (index as f32) / 64.0;
    }
    let encoded_coordinate = EncodedCoordinateSearchQuery::new(
        &coordinate_input,
        encoder.source_contract().model.clone(),
        coordinate_values,
        encoder.source_contract(),
    )
    .expect("fixed Coordinate query vector");

    let problem_score = score(650_000);
    let conditioned_score = score(850_000);
    let environment = environment_gain(&[ConditionedEvidence::new(
        work.clone(),
        problem_score,
        conditioned_score,
    )]);
    let candidate = candidate_score(
        problem_score,
        environment.environment_gain,
        AnchorGain::None,
    );
    let relation = document_score(
        problem_score,
        environment.environment_gain,
        Some(score(800_000)),
    );
    let target =
        target_coordinate_score(problem_score, environment.environment_gain, score(800_000));
    let transition = harmonic_score(relation, target);
    let path =
        path_score(Some(candidate), &[transition, score(700_000)]).expect("path score fixture");

    let source_contract = encoder.source_contract();
    let coordinate_vector_digest = vector_digest(encoded_coordinate.embedding().as_slice());
    let graph_vector_digests = encoded_graph
        .iter()
        .map(|encoded| vector_digest(encoded.embedding().as_slice()))
        .collect::<Vec<_>>();

    json!({
        "schema_version": 1,
        "fixture_id": FIXTURE_ID,
        "synthetic_graph": {
            "project_id": project_id,
            "coordinates": [
                role.tag_value(project_id),
                work.tag_value(project_id),
                issue.tag_value(project_id)
            ],
            "edge_key": edge_key.to_hex(),
            "topology": "one complete undirected three-coordinate Hyperedge"
        },
        "model_space": {
            "provider": source_contract.provider,
            "model": source_contract.model,
            "dimensions": source_contract.dimensions,
            "generation_contract_digest": source_contract.digest()
                .expect("model digest")
                .to_hex(),
            "embedding_space_fence": embedding_space_fence(source_contract)
                .expect("embedding fence")
                .to_hex()
        },
        "contract_digests": {
            "coordinate_query": coordinate_input.query_contract_digest().to_hex(),
            "graph_query_q0_qi": query_contract_digest().to_hex(),
            "one_hop_incident_edge_ranking":
                incident_edge_ranking_contract_digest().to_hex(),
            "one_hop_edge_coordinate_ranking":
                edge_coordinate_ranking_contract_digest().to_hex(),
            "graph_ranking": ranking_contract_digest().expect("ranking digest").to_hex(),
            "graph_budget": budget_profile_digest().expect("budget digest").to_hex(),
            "fleet_runtime": LEGACY_FLEET_RUNTIME_DIGEST
        },
        "operations": [
            {
                "logical_operation": "whole_graph_coordinate_discovery",
                "surface": "coordinate_search",
                "result_kind": 40913,
                "input_count": 1,
                "canonical_inputs": [coordinate_input.text()],
                "input_digests": [coordinate_input.text_digest().to_hex()],
                "query_vector_digests": [coordinate_vector_digest],
                "result_shape": "rank_coordinate_score_only",
                "execution_lifecycle": "one_shot"
            },
            {
                "logical_operation": "coordinate_incident_edge",
                "surface": "one_hop_tagged_family",
                "result_kind": 40914,
                "input_count": 1,
                "canonical_inputs": [incident_input.text()],
                "input_digests": [incident_input.text_digest().to_hex()],
                "query_vector_digests": [graph_vector_digests[0]],
                "result_shape": "edge_with_ranked_relation_document_observations_no_coordinates",
                "execution_lifecycle": "one_shot"
            },
            {
                "logical_operation": "edge_member_coordinate",
                "surface": "one_hop_tagged_family",
                "result_kind": 40914,
                "input_count": 1,
                "canonical_inputs": [edge_coordinate_input.text()],
                "input_digests": [edge_coordinate_input.text_digest().to_hex()],
                "query_vector_digests": [graph_vector_digests[0]],
                "result_shape": "coordinate_observations_no_relation_documents_or_paths",
                "execution_lifecycle": "one_shot"
            },
            {
                "logical_operation": "bounded_complete_path",
                "surface": "semantic_graph_query",
                "result_kind": 40912,
                "input_count": graph_inputs.len(),
                "canonical_inputs": graph_inputs
                    .iter()
                    .map(|input| input.text())
                    .collect::<Vec<_>>(),
                "input_digests": graph_inputs
                    .iter()
                    .map(|input| input.text_digest().to_hex())
                    .collect::<Vec<_>>(),
                "query_vector_digests": graph_vector_digests,
                "result_shape": "roots_paths_provenance_coverage",
                "execution_lifecycle": "multi_stage_traversal"
            }
        ],
        "fixed_score_goldens": {
            "cosine_distance_0_2": Score::from_cosine_distance(0.2)
                .expect("distance score")
                .raw(),
            "problem_score": problem_score.raw(),
            "conditioned_score": conditioned_score.raw(),
            "environment_gain": environment.environment_gain.raw(),
            "candidate_score": candidate.raw(),
            "relation_document_score": relation.raw(),
            "target_coordinate_score": target.raw(),
            "transition_score": transition.raw(),
            "path_final_score": path.final_score.map(Score::raw)
        },
        "protected_result_boundaries": {
            "coordinate_search": {
                "includes": ["rank", "coordinate", "score"],
                "excludes": ["edge", "path", "preview"]
            },
            "coordinate_incident_edge": {
                "includes": ["edge", "ranked_relation_document_observations"],
                "excludes": ["member_coordinates", "path"]
            },
            "edge_member_coordinate": {
                "includes": ["ranked_coordinate_observations"],
                "excludes": ["relation_documents", "other_edges", "path"]
            },
            "bounded_complete_path": {
                "includes": ["roots", "paths", "provenance", "coverage"],
                "excludes": ["caller_defined_query_plan"]
            }
        },
        "current_runtime_profile": {
            "one_shot": {
                "operations": [
                    "whole_graph_coordinate_discovery",
                    "coordinate_incident_edge",
                    "edge_member_coordinate"
                ],
                "internal_retry": "none",
                "release_snapshot": "exact_expected_snapshot"
            },
            "complete_path": {
                "operations": ["bounded_complete_path"],
                "internal_retry": "one_generation_or_context_churn_root_attempt",
                "release_snapshot": "existing_operation_specific_contract"
            }
        }
    })
}

#[tokio::test]
async fn compatibility_manifest_matches_golden() {
    let actual = manifest().await;
    println!(
        "semantic_retrieval_compatibility_manifest={}",
        serde_json::to_string(&actual).expect("compact manifest")
    );
    let expected: Value = serde_json::from_str(include_str!(
        "fixtures/semantic_retrieval_compatibility_manifest.json"
    ))
    .expect("tracked compatibility manifest");
    assert_eq!(actual, expected);
}
