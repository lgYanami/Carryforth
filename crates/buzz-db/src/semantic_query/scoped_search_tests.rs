use buzz_core::{CommunityId, Keys, PublicKey};
use buzz_project_context::{EdgeKey, ProjectContextCoordinate};
use buzz_project_view::ProjectViewObjectType;
use buzz_semantic::{
    Digest32, SemanticCoverage, SemanticDistanceMetric, SemanticEligibility, SemanticModelContract,
    SemanticNormalization, SemanticProviderBoundary, SemanticSourceBasis, SemanticSourceIdentity,
};
use buzz_semantic_query::{
    build_problem_query_encoder_input, OneHopCanonicalRead, OneHopOmittedCandidateCounts,
    OneHopSemanticSelection, ProjectContextCoordinateType, ProjectContextCoordinateTypeFilter,
    ProviderEncodedSemanticInput, ProviderEncodedSemanticInputBundle, QueryCompatibilityFences,
    Score, SemanticQueryInputBundle,
};
use chrono::Utc;
use pgvector::Vector;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use super::*;
use crate::semantic::{observe_semantic_source_in_connection, SemanticGenerationRecord};
use crate::semantic_query::{
    SemanticCurrentHead, SemanticEdgeTargetRankOutcome, SemanticEdgeTargetRankRequest,
    SemanticGraphQueryVectorBundle, SemanticGraphStructuralRoles, SemanticHyperedgeExpectation,
    SemanticHyperedgeReadOutcome, SemanticIncidentRelationRankOutcome,
    SemanticIncidentRelationRankRequest, SemanticTraversalQueryChannels,
};
use crate::{Db, DbConfig};

fn problem_vector(
    ticket: &super::super::SemanticGraphQueryTicket,
    request_id: Uuid,
    values: Vec<f32>,
) -> SemanticExactQueryVector {
    problem_vector_pair(ticket, request_id, values).0
}

fn problem_vector_pair(
    ticket: &super::super::SemanticGraphQueryTicket,
    request_id: Uuid,
    values: Vec<f32>,
) -> (SemanticExactQueryVector, SemanticGraphQueryVectorBundle) {
    let input =
        build_problem_query_encoder_input(request_id, "one-hop fixture").expect("problem input");
    let encoded = ProviderEncodedSemanticInput::new(
        input.semantic_input(),
        ticket.generation.model_contract.model.clone(),
        values.clone(),
        &ticket.generation.model_contract,
    )
    .expect("Provider-bound input");
    let compatibility = SemanticExactQueryVector::new(ticket, encoded.clone())
        .expect("generation-bound query vector");
    let input_bundle =
        SemanticQueryInputBundle::from_closed_inputs(vec![input.semantic_input().clone()])
            .expect("complete-path input bundle");
    let provider_bundle = ProviderEncodedSemanticInputBundle::new(
        &input_bundle,
        ticket.generation.model_contract.model.clone(),
        vec![values],
        &ticket.generation.model_contract,
    )
    .expect("complete-path Provider bundle");
    let migrated = SemanticGraphQueryVectorBundle::bind(ticket, provider_bundle)
        .expect("complete-path query bundle");
    (compatibility, migrated)
}

#[test]
fn scoped_sql_is_revision_bound_and_has_no_semantic_ranking_policy() {
    for marker in [
        "state.context_revision = $5",
        "coordinate.coordinate_subtype IS NOT DISTINCT FROM $3",
        "edge.state = 'active'",
        "binding.state = 'active'",
        "count(DISTINCT edge_key)",
    ] {
        assert!(INCIDENT_SCOPE_COUNTS_SQL.contains(marker));
    }
    for forbidden in [
        "embedding <=>",
        "RELATION_FLOOR",
        "TARGET_FLOOR",
        "TRANSITION_FLOOR",
        "coherence",
    ] {
        assert!(!INCIDENT_SCOPE_COUNTS_SQL.contains(forbidden));
    }
}

#[test]
fn omission_partition_distinguishes_zero_vector_from_other_failures() {
    fn state(
        eligibility: Option<SemanticEligibility>,
        availability: CurrentSemanticAvailabilityClass,
    ) -> CurrentSemanticSourceState {
        CurrentSemanticSourceState {
            source: SemanticSourceIdentity {
                community_id: Uuid::new_v4(),
                kind: buzz_semantic::SemanticSourceKind::ProjectDocument,
                source_id: Uuid::new_v4(),
            },
            source_invalidation_epoch: Some(1),
            eligibility,
            lifecycle: None,
            availability,
            head: None,
            semantic_text: None,
        }
    }

    let mut counts = OneHopOmittedCandidateCounts::default();
    count_omission(
        &mut counts,
        &state(
            Some(SemanticEligibility::Eligible),
            CurrentSemanticAvailabilityClass::NonQueryableZeroVector,
        ),
    )
    .expect("zero omission");
    count_omission(
        &mut counts,
        &state(
            Some(SemanticEligibility::Eligible),
            CurrentSemanticAvailabilityClass::Failed,
        ),
    )
    .expect("failed omission");
    assert_eq!(counts.non_queryable_zero_vector, 1);
    assert_eq!(counts.semantic_head_failed_or_unsupported, 1);
}

#[test]
fn direct_ordering_uses_only_score_then_canonical_identity() {
    let source = |id| SemanticSourceIdentity {
        community_id: Uuid::from_u128(1),
        kind: buzz_semantic::SemanticSourceKind::ProjectDocument,
        source_id: Uuid::from_u128(id),
    };
    let score = |id, value| SemanticExactSourceScore {
        channel_id: buzz_semantic::Digest32::from_bytes([1; 32]),
        source: source(id),
        head: SemanticCurrentHead {
            invalidation_epoch: 1,
            snapshot_digest: buzz_semantic::Digest32::from_bytes([2; 32]),
            source_basis: SemanticSourceBasis::ProjectDocument(
                buzz_semantic::ProjectDocumentSourceBasis {
                    document_revision: 1,
                    source_change_id: buzz_semantic::Digest32::from_bytes([3; 32]),
                },
            ),
            unit_set_id: Uuid::new_v4(),
            unit_key: "overview".to_owned(),
            semantic_text_digest: buzz_semantic::Digest32::from_bytes([4; 32]),
            summary_coverage: SemanticCoverage::TitleOnly,
        },
        lifecycle: buzz_semantic::SemanticLifecycleClass::Active,
        source_status: None,
        roles: SemanticGraphStructuralRoles {
            coordinate: false,
            coordinate_entry_eligible: false,
            coordinate_incident_edge_keys: Vec::new(),
            context_document_bindings: Vec::new(),
        },
        score: Score::new(value).expect("score"),
        channel_rank: 1,
    };
    let mut values = [
        ScoredRelation {
            document_id: Uuid::from_u128(2),
            score: score(2, 700_000),
        },
        ScoredRelation {
            document_id: Uuid::from_u128(1),
            score: score(1, 700_000),
        },
        ScoredRelation {
            document_id: Uuid::from_u128(3),
            score: score(3, 800_000),
        },
    ];
    values.sort_by(compare_scored_relations);
    assert_eq!(
        values
            .iter()
            .map(|value| value.document_id)
            .collect::<Vec<_>>(),
        vec![Uuid::from_u128(3), Uuid::from_u128(1), Uuid::from_u128(2)]
    );
}

#[tokio::test]
async fn one_hop_scoped_search_real_pgvector_is_direct_complete_and_hydrated() {
    let Ok(database_url) = std::env::var("BUZZ_TEST_SEMANTIC_DATABASE_URL") else {
        return;
    };
    assert_eq!(
        std::env::var("BUZZ_TEST_SEMANTIC_DISPOSABLE").as_deref(),
        Ok("fleet-policy-v1"),
        "refusing one-hop fixture without the disposable database marker"
    );
    assert!(database_url.contains("@127.0.0.1:"));
    assert!(database_url.contains("/buzz_semantic_disposable"));

    let db = Db::new(&DbConfig {
        database_url,
        ..DbConfig::default()
    })
    .await
    .expect("one-hop test database");
    db.migrate().await.expect("one-hop test migrations");
    let mut tx = db
        .writer()
        .begin()
        .await
        .expect("one-hop fixture transaction");
    let community_uuid = Uuid::new_v4();
    let community_id = CommunityId::from_uuid(community_uuid);
    let generation_id = Uuid::new_v4();
    let reader = Keys::generate();
    let relay = Keys::generate();
    let projection = relay.public_key();
    let opaque = vec![0x21_u8; 32];
    let contract = SemanticModelContract {
        provider: "deterministic_fake".to_owned(),
        model: "deterministic-fake-v1".to_owned(),
        dimensions: 3,
        distance_metric: SemanticDistanceMetric::Cosine,
        normalization: SemanticNormalization::None,
        input_contract_version: "overview-v1".to_owned(),
        provider_boundary: SemanticProviderBoundary::DeterministicFake,
    };
    let contract_digest = contract.digest().expect("model contract digest");
    let query_fences =
        QueryCompatibilityFences::for_source_contract(&contract).expect("query fences");
    let extractor_version = "one-hop-scoped-real-pg-v1";

    seed_one_hop_foundation(
        &mut tx,
        community_uuid,
        generation_id,
        &contract,
        contract_digest,
        extractor_version,
        &reader,
        &relay,
        &opaque,
    )
    .await;

    let entered_document = fixture_uuid(101);
    let weaker_document = fixture_uuid(102);
    let missing_document = fixture_uuid(103);
    let relation_document = fixture_uuid(104);
    for (ordinal, document_id, title, summary) in [
        (
            1_i64,
            entered_document,
            "Frontend authorization work",
            "Frontend evidence used to select this context.",
        ),
        (
            2,
            weaker_document,
            "Shared authorization issue",
            "A lower-scoring but current member.",
        ),
        (
            3,
            missing_document,
            "Unindexed active source",
            "Canonical but intentionally missing a semantic head.",
        ),
        (
            4,
            relation_document,
            "Frontend relation evidence",
            "Proves why the frontend members belong to this relation.",
        ),
    ] {
        seed_canonical_document(
            &mut tx,
            community_uuid,
            document_id,
            ordinal,
            title,
            summary,
            reader.public_key().as_bytes(),
        )
        .await;
    }

    let mut coordinates = vec![
        ProjectContextCoordinate::Document {
            document_id: entered_document,
        },
        ProjectContextCoordinate::Document {
            document_id: weaker_document,
        },
        ProjectContextCoordinate::Document {
            document_id: missing_document,
        },
    ];
    coordinates.sort();
    let edge_key = EdgeKey::derive(community_uuid, &coordinates).expect("fixture EdgeKey");
    sqlx::query(
        "INSERT INTO project_context_edges(community_id,edge_key,state,canonical_coordinates,\
         last_context_revision,current_source_change_id,updated_at,updated_by) \
         VALUES($1,$2,'active',$3,9,$4,clock_timestamp(),$5)",
    )
    .bind(community_uuid)
    .bind(edge_key.as_bytes().as_slice())
    .bind(serde_json::to_value(&coordinates).expect("canonical Coordinates"))
    .bind(vec![0x31_u8; 32])
    .bind(reader.public_key().as_bytes().as_slice())
    .execute(&mut *tx)
    .await
    .expect("insert one-hop Edge");
    for (ordinal, coordinate) in coordinates.iter().enumerate() {
        let ProjectContextCoordinate::Document { document_id } = coordinate else {
            panic!("Document fixture Coordinate")
        };
        sqlx::query(
            "INSERT INTO project_context_edge_coordinates(community_id,edge_key,ordinal,\
             coordinate_type,coordinate_subtype,coordinate_id,canonical_key) \
             VALUES($1,$2,$3,'document',NULL,$4,$5)",
        )
        .bind(community_uuid)
        .bind(edge_key.as_bytes().as_slice())
        .bind(i32::try_from(ordinal).expect("Coordinate ordinal"))
        .bind(document_id)
        .bind(format!("document:{community_uuid}:{document_id}"))
        .execute(&mut *tx)
        .await
        .expect("insert one-hop Coordinate");
    }
    sqlx::query(
        "INSERT INTO project_context_document_bindings(community_id,context_document_id,\
         edge_key,state,binding_context_revision,current_source_change_id,\
         current_projection_event_id,updated_at,updated_by) \
         VALUES($1,$2,$3,'active',9,$4,$5,clock_timestamp(),$6)",
    )
    .bind(community_uuid)
    .bind(relation_document)
    .bind(edge_key.as_bytes().as_slice())
    .bind(vec![0x32_u8; 32])
    .bind(vec![0x33_u8; 32])
    .bind(reader.public_key().as_bytes().as_slice())
    .execute(&mut *tx)
    .await
    .expect("insert one-hop Binding");

    for (document_id, vector, seed) in [
        (entered_document, vec![1.0, 0.0, 0.0], 0x41_u8),
        (weaker_document, vec![0.0, 1.0, 0.0], 0x42),
        (relation_document, vec![0.8, 0.6, 0.0], 0x43),
    ] {
        seed_document_semantic_head(
            &mut tx,
            community_uuid,
            generation_id,
            &contract,
            contract_digest,
            extractor_version,
            document_id,
            vector,
            seed,
        )
        .await;
    }
    sqlx::query(
        "DELETE FROM semantic_index_jobs WHERE community_id=$1 \
         AND source_family='project_document' AND source_subtype='document' AND source_id=$2",
    )
    .bind(community_uuid)
    .bind(missing_document)
    .execute(&mut *tx)
    .await
    .expect("remove missing-head fixture job");
    sqlx::query(
        "UPDATE semantic_sources SET eligibility='eligible',coverage_state='missing' \
         WHERE community_id=$1 \
         AND source_family='project_document' AND source_subtype='document' AND source_id=$2",
    )
    .bind(community_uuid)
    .bind(missing_document)
    .execute(&mut *tx)
    .await
    .expect("mark missing-head fixture source");

    let observed_at = Utc::now();
    let ticket = super::super::SemanticGraphQueryTicket {
        community_id,
        generation: SemanticGenerationRecord {
            community_id,
            generation_id,
            lifecycle: "active".to_owned(),
            extractor_version: extractor_version.to_owned(),
            model_contract: contract.clone(),
            model_contract_digest: contract_digest,
            rebuild_completed_at: Some(observed_at),
            created_at: observed_at,
        },
        query_fences,
        projection_generation: 1,
        project_context_revision: 9,
        observed_at,
    };
    let (vector, graph_vectors) =
        problem_vector_pair(&ticket, Uuid::from_u128(0x51), vec![1.0, 0.0, 0.0]);
    let problem_channel_id = vector.channel_id();
    let mut read = super::super::SemanticGraphReadTx {
        tx,
        ticket,
        reader_pubkey: reader.public_key().to_bytes().to_vec(),
        expected_projection_pubkey: projection,
    };

    let compatibility_scores = read
        .query_exact_source_scores(
            buzz_semantic_query::LifecycleFilter::AllCurrent,
            &[],
            std::slice::from_ref(&vector),
            None,
            Some(8),
        )
        .await
        .expect("compatibility complete-path root scores");
    let migrated_scores = read
        .query_graph_exact_source_scores(
            buzz_semantic_query::LifecycleFilter::AllCurrent,
            &[],
            &graph_vectors,
            None,
            Some(8),
        )
        .await
        .expect("migrated complete-path root scores");
    assert_eq!(migrated_scores, compatibility_scores);

    let traversal_channels = SemanticTraversalQueryChannels {
        query_vectors: &graph_vectors,
        problem_channel_id,
        conditioned: &[],
    };
    let relation_rank = read
        .rank_incident_relation_options_exact(SemanticIncidentRelationRankRequest {
            entered_from: &coordinates[0],
            channels: traversal_channels,
            after: None,
            limit: 8,
        })
        .await
        .expect("migrated complete-path relation rank");
    let SemanticIncidentRelationRankOutcome::Ranked(relation_rank) = relation_rank else {
        panic!("ranked complete-path relation")
    };
    let relation = relation_rank.options.first().expect("relation option");
    assert_eq!(relation.edge_key, edge_key);
    assert_eq!(relation.document_id, relation_document);
    let edge = read
        .load_complete_hyperedge(&SemanticHyperedgeExpectation {
            edge_key,
            edge_provenance: relation.edge_provenance.clone(),
            required_binding: Some(buzz_semantic_query::ContextDocumentBindingObservation {
                document_id: relation.document_id,
                provenance: relation.binding_provenance.clone(),
            }),
        })
        .await
        .expect("complete-path Hyperedge");
    let SemanticHyperedgeReadOutcome::Current(edge) = edge else {
        panic!("current complete-path Hyperedge")
    };
    let relation_head = &relation
        .channel_scores
        .first()
        .expect("relation Q0 score")
        .head;
    let target_rank = read
        .rank_edge_target_options_exact(SemanticEdgeTargetRankRequest {
            hyperedge: &edge,
            relation_document_id: relation.document_id,
            relation_document_head: relation_head,
            document_score: relation.document_score,
            lifecycle_filter: buzz_semantic_query::LifecycleFilter::AllCurrent,
            channels: traversal_channels,
            after: None,
            limit: 8,
        })
        .await
        .expect("migrated complete-path target rank");
    let SemanticEdgeTargetRankOutcome::Ranked(target_rank) = target_rank else {
        panic!("ranked complete-path targets")
    };
    assert_eq!(target_rank.edge.edge_key, edge_key);
    assert_eq!(target_rank.options.len(), 1);
    assert!(target_rank
        .options
        .iter()
        .all(|option| option.transition_score > Score::ZERO));
    assert!(target_rank
        .options
        .windows(2)
        .all(|pair| pair[0].transition_score >= pair[1].transition_score));

    let incident = read
        .search_incident_edges_one_hop(&coordinates[0], &vector, 8)
        .await
        .expect("incident one-hop query");
    let SemanticIncidentEdgeSearchOutcome::Ranked(incident) = incident else {
        panic!("ranked incident Edge")
    };
    let OneHopSemanticSelection::IncidentEdges {
        edges,
        coverage,
        truncated,
        ..
    } = &incident.selection
    else {
        panic!("incident selection")
    };
    assert!(!truncated);
    assert_eq!(coverage.active_incident_edges, 1);
    assert_eq!(coverage.active_relation_bindings, 1);
    assert_eq!(coverage.scorable_relation_bindings, 1);
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].edge_key, edge_key);
    assert_eq!(edges[0].ranked_documents.len(), 1);
    let relation = &edges[0].ranked_documents[0];
    assert_eq!(relation.document_id, relation_document);
    assert_eq!(relation.score.raw(), 900_000);
    assert_eq!(
        relation.canonical_observation.preview.title,
        "Frontend relation evidence"
    );
    assert_eq!(
        relation.canonical_observation.preview.summary.as_deref(),
        Some("Proves why the frontend members belong to this relation.")
    );
    assert!(relation.canonical_observation.preview.description.is_none());
    assert!(matches!(
        &relation.canonical_observation.canonical_read,
        OneHopCanonicalRead::Document {
            expected_document_revision: 1,
            ..
        }
    ));

    let limited = read
        .search_edge_coordinates_one_hop(edge_key, &vector, 1)
        .await
        .expect("limited Edge Coordinate query");
    let SemanticEdgeCoordinateSearchOutcome::Ranked(limited) = limited else {
        panic!("ranked Edge Coordinates")
    };
    let OneHopSemanticSelection::EdgeCoordinates {
        ranked_coordinates,
        coverage,
        truncated,
        ..
    } = &limited.selection
    else {
        panic!("Edge Coordinate selection")
    };
    assert!(*truncated);
    assert_eq!(coverage.edge_coordinate_count, 3);
    assert_eq!(coverage.scorable_coordinates, 2);
    assert_eq!(coverage.omitted_coordinates.semantic_head_missing, 1);
    assert_eq!(ranked_coordinates.len(), 1);
    assert_eq!(ranked_coordinates[0].coordinate, coordinates[0]);
    assert_eq!(ranked_coordinates[0].score.raw(), 1_000_000);
    assert_eq!(
        ranked_coordinates[0].canonical_observation.preview.title,
        "Frontend authorization work"
    );
    assert_eq!(
        ranked_coordinates[0]
            .canonical_observation
            .preview
            .summary
            .as_deref(),
        Some("Frontend evidence used to select this context.")
    );

    let complete = read
        .search_edge_coordinates_one_hop(edge_key, &vector, 8)
        .await
        .expect("complete Edge Coordinate query");
    let SemanticEdgeCoordinateSearchOutcome::Ranked(complete) = complete else {
        panic!("complete ranked Edge Coordinates")
    };
    let OneHopSemanticSelection::EdgeCoordinates {
        ranked_coordinates,
        truncated,
        ..
    } = &complete.selection
    else {
        panic!("complete Edge Coordinate selection")
    };
    assert!(!truncated);
    assert_eq!(ranked_coordinates.len(), 2);
    assert_eq!(ranked_coordinates[0].score.raw(), 1_000_000);
    assert_eq!(ranked_coordinates[1].score.raw(), 500_000);
    let complete_coordinates = ranked_coordinates.clone();
    let complete_truncated = *truncated;

    let work_only =
        ProjectContextCoordinateTypeFilter::new(vec![ProjectContextCoordinateType::Work])
            .expect("Work filter");
    let filtered = read
        .search_edge_coordinates_one_hop_filtered(edge_key, &vector, &work_only, 1)
        .await
        .expect("filtered Edge Coordinate query");
    let SemanticEdgeCoordinateSearchOutcome::Ranked(filtered) = filtered else {
        panic!("filtered ranked Edge Coordinates")
    };
    let OneHopSemanticSelection::EdgeCoordinates {
        coordinate_types,
        ranked_coordinates,
        coverage,
        truncated,
        ..
    } = &filtered.selection
    else {
        panic!("filtered Edge Coordinate selection")
    };
    assert_eq!(coordinate_types.as_ref(), Some(&work_only));
    assert!(ranked_coordinates.is_empty());
    assert!(!truncated);
    assert_eq!(coverage.edge_coordinate_count, 3);
    assert_eq!(coverage.type_matched_coordinate_count, Some(0));
    assert_eq!(coverage.type_filtered_out_coordinates, Some(3));
    assert_eq!(coverage.scorable_coordinates, 0);
    assert_eq!(
        coverage.omitted_coordinates,
        OneHopOmittedCandidateCounts::default()
    );

    let document_only =
        ProjectContextCoordinateTypeFilter::new(vec![ProjectContextCoordinateType::Document])
            .expect("Document filter");
    let filtered_complete = read
        .search_edge_coordinates_one_hop_filtered(edge_key, &vector, &document_only, 8)
        .await
        .expect("all-member-type Edge Coordinate query");
    let SemanticEdgeCoordinateSearchOutcome::Ranked(filtered_complete) = filtered_complete else {
        panic!("filtered complete ranked Edge Coordinates")
    };
    let OneHopSemanticSelection::EdgeCoordinates {
        ranked_coordinates: filtered_coordinates,
        coverage: filtered_coverage,
        truncated: filtered_truncated,
        ..
    } = &filtered_complete.selection
    else {
        panic!("filtered complete Edge Coordinate selection")
    };
    assert_eq!(filtered_coordinates, &complete_coordinates);
    assert_eq!(*filtered_truncated, complete_truncated);
    assert_eq!(filtered_coverage.edge_coordinate_count, 3);
    assert_eq!(filtered_coverage.type_matched_coordinate_count, Some(3));
    assert_eq!(filtered_coverage.type_filtered_out_coordinates, Some(0));
    assert_eq!(filtered_coverage.scorable_coordinates, 2);
    assert_eq!(
        filtered_coverage.omitted_coordinates.semantic_head_missing,
        1
    );
    read.rollback().await.expect("one-hop fixture rollback");
}

#[allow(clippy::too_many_arguments)]
async fn seed_one_hop_foundation(
    tx: &mut Transaction<'_, Postgres>,
    community_id: Uuid,
    generation_id: Uuid,
    contract: &SemanticModelContract,
    contract_digest: Digest32,
    extractor_version: &str,
    reader: &Keys,
    relay: &Keys,
    opaque: &[u8],
) {
    sqlx::query(
        "INSERT INTO communities(\
         id,host,project_view_enabled,project_document_enabled,project_context_enabled,\
         project_context_edge_enabled,project_view_schema_version,\
         meeting_community_read_enabled,meeting_community_read_enabled_at,\
         legacy_meeting_visibility_watermark,legacy_meeting_visibility_audit_digest,\
         legacy_meeting_visibility_meeting_count,legacy_meeting_visibility_community_source_count,\
         legacy_meeting_visibility_private_source_count,legacy_meeting_visibility_missing_source_count,\
         legacy_meeting_visibility_audited_at,legacy_meeting_visibility_approved_at,\
         legacy_meeting_visibility_approved_by,semantic_index_enabled,semantic_graph_query_enabled) \
         VALUES($1,$2,TRUE,TRUE,TRUE,TRUE,3,TRUE,clock_timestamp(),0,$3,0,0,0,0,\
                clock_timestamp(),clock_timestamp(),'one-hop-test',TRUE,TRUE)",
    )
    .bind(community_id)
    .bind(format!("one-hop-{community_id}.invalid"))
    .bind(opaque)
    .execute(&mut **tx)
    .await
    .expect("insert one-hop Community");
    sqlx::query("INSERT INTO relay_members(community_id,pubkey,role) VALUES($1,$2,'member')")
        .bind(community_id)
        .bind(reader.public_key().to_hex())
        .execute(&mut **tx)
        .await
        .expect("insert one-hop member");
    sqlx::query(
        "INSERT INTO project_view_maintenance(community_id,state,updated_at) \
         VALUES($1,'normal',clock_timestamp()) \
         ON CONFLICT (community_id) DO UPDATE \
         SET state='normal',current_epoch=NULL,updated_at=EXCLUDED.updated_at",
    )
    .bind(community_id)
    .execute(&mut **tx)
    .await
    .expect("insert Project View maintenance");
    sqlx::query(
        "INSERT INTO project_view_state(community_id,project_revision,active_object_count,\
         initialized_at,updated_at,last_event_id,last_actor_pubkey,meta_projection_event_id,\
         projection_pubkey,projection_generation,schema_version,last_change_id,last_source_event_id) \
         VALUES($1,1,0,clock_timestamp(),clock_timestamp(),$2,$2,$2,$3,1,3,$2,$2)",
    )
    .bind(community_id)
    .bind(opaque)
    .bind(relay.public_key().as_bytes().as_slice())
    .execute(&mut **tx)
    .await
    .expect("insert Project View state");
    sqlx::query(
        "INSERT INTO project_document_state(community_id,schema_version,catalog_revision,\
         active_document_count,last_change_id,last_actor_pubkey,projection_pubkey,\
         projection_generation,meta_projection_event_id,initialized_at,updated_at) \
         VALUES($1,1,4,4,$2,$3,$4,1,$2,clock_timestamp(),clock_timestamp())",
    )
    .bind(community_id)
    .bind(opaque)
    .bind(reader.public_key().as_bytes().as_slice())
    .bind(relay.public_key().as_bytes().as_slice())
    .execute(&mut **tx)
    .await
    .expect("insert Project Document state");
    sqlx::query(
        "INSERT INTO project_context_edge_state(community_id,schema_version,context_revision,\
         active_edge_count,bound_document_count,last_change_id,last_actor_pubkey,projection_pubkey,\
         projection_generation,meta_projection_event_id,initialized_at,updated_at) \
         VALUES($1,2,9,1,1,$2,$3,$4,1,$2,clock_timestamp(),clock_timestamp())",
    )
    .bind(community_id)
    .bind(opaque)
    .bind(reader.public_key().as_bytes().as_slice())
    .bind(relay.public_key().as_bytes().as_slice())
    .execute(&mut **tx)
    .await
    .expect("insert Project Context state");
    sqlx::query(
        "INSERT INTO semantic_index_generations(community_id,generation_id,lifecycle,\
         extractor_version,input_contract_version,provider,model,dimensions,distance_metric,\
         normalization,provider_boundary,model_contract_digest,created_by,rebuild_completed_at,\
         ready_at,activated_at) \
         VALUES($1,$2,'active',$3,$4,$5,$6,$7,'cosine','none','deterministic_fake',$8,\
                'one-hop-test',clock_timestamp(),clock_timestamp(),clock_timestamp())",
    )
    .bind(community_id)
    .bind(generation_id)
    .bind(extractor_version)
    .bind(contract.input_contract_version.as_str())
    .bind(contract.provider.as_str())
    .bind(contract.model.as_str())
    .bind(i32::try_from(contract.dimensions).expect("fixture dimensions"))
    .bind(contract_digest.as_bytes().as_slice())
    .execute(&mut **tx)
    .await
    .expect("insert one-hop semantic generation");
    sqlx::query("UPDATE communities SET semantic_active_generation_id=$2 WHERE id=$1")
        .bind(community_id)
        .bind(generation_id)
        .execute(&mut **tx)
        .await
        .expect("activate one-hop generation");
}

#[allow(clippy::too_many_arguments)]
async fn seed_canonical_document(
    tx: &mut Transaction<'_, Postgres>,
    community_id: Uuid,
    document_id: Uuid,
    catalog_revision: i64,
    title: &str,
    summary: &str,
    actor: &[u8; 32],
) {
    let source_change = vec![u8::try_from(catalog_revision).expect("fixture seed"); 32];
    let canonical_at = Utc::now();
    sqlx::query(
        "INSERT INTO project_documents(community_id,document_id,current_revision,state,\
         created_at,created_by,updated_at,updated_by,current_source_change_id,\
         current_head_event_id,current_revision_event_id) \
         VALUES($1,$2,1,'active',$5,$3,$5,$3,$4,$4,$4)",
    )
    .bind(community_id)
    .bind(document_id)
    .bind(actor.as_slice())
    .bind(&source_change)
    .bind(canonical_at)
    .execute(&mut **tx)
    .await
    .expect("insert canonical Document");
    sqlx::query(
        "INSERT INTO project_document_revisions(community_id,document_id,document_revision,\
         catalog_revision,state,title,summary,content_markdown,actor_pubkey,canonical_at,\
         source_change_id,source_event_id,projection_generation,projection_event_id) \
         VALUES($1,$2,1,$3,'active',$4,$5,'private full body must not enter preview',$6,\
                $8,$7,$7,1,$7)",
    )
    .bind(community_id)
    .bind(document_id)
    .bind(catalog_revision)
    .bind(title)
    .bind(summary)
    .bind(actor.as_slice())
    .bind(&source_change)
    .bind(canonical_at)
    .execute(&mut **tx)
    .await
    .expect("insert canonical Document revision");
}

#[allow(clippy::too_many_arguments)]
async fn seed_document_semantic_head(
    tx: &mut Transaction<'_, Postgres>,
    community_id: Uuid,
    generation_id: Uuid,
    contract: &SemanticModelContract,
    contract_digest: Digest32,
    extractor_version: &str,
    document_id: Uuid,
    vector: Vec<f32>,
    seed: u8,
) {
    let source = SemanticSourceIdentity {
        community_id,
        kind: buzz_semantic::SemanticSourceKind::ProjectDocument,
        source_id: document_id,
    };
    let observation = observe_semantic_source_in_connection(tx, &source)
        .await
        .expect("observe canonical Document");
    let unit_set_id = Uuid::new_v4();
    let semantic_text_digest = vec![seed.wrapping_add(1); 32];
    sqlx::query(
        "UPDATE semantic_sources SET eligibility='eligible',ineligibility_reason=NULL,\
         lifecycle_class='active',source_status='active',source_basis=$3,snapshot_digest=$4,\
         invalidation_epoch=1,coverage_state='current',observed_at=clock_timestamp() \
         WHERE community_id=$1 AND source_family='project_document' \
           AND source_subtype='document' AND source_id=$2",
    )
    .bind(community_id)
    .bind(document_id)
    .bind(serde_json::to_value(&observation.basis).expect("source basis"))
    .bind(observation.snapshot_digest.as_bytes().as_slice())
    .execute(&mut **tx)
    .await
    .expect("update semantic source");
    sqlx::query(
        "INSERT INTO semantic_unit_sets(community_id,unit_set_id,source_family,source_subtype,\
         source_id,source_invalidation_epoch,source_basis,source_snapshot_digest,\
         extractor_version,state,complete_unit_count,activated_at) \
         VALUES($1,$2,'project_document','document',$3,1,$4,$5,$6,'active',1,clock_timestamp())",
    )
    .bind(community_id)
    .bind(unit_set_id)
    .bind(document_id)
    .bind(serde_json::to_value(&observation.basis).expect("unit source basis"))
    .bind(observation.snapshot_digest.as_bytes().as_slice())
    .bind(extractor_version)
    .execute(&mut **tx)
    .await
    .expect("insert semantic unit set");
    sqlx::query(
        "INSERT INTO semantic_units(community_id,unit_set_id,unit_key,ordinal,unit_kind,\
         semantic_text,semantic_text_digest,summary_coverage,extraction_provenance) \
         VALUES($1,$2,'overview',0,'overview','different embedding-only overview',$3,\
                'title_and_summary','{}'::jsonb)",
    )
    .bind(community_id)
    .bind(unit_set_id)
    .bind(&semantic_text_digest)
    .execute(&mut **tx)
    .await
    .expect("insert semantic unit");
    sqlx::query(
        "INSERT INTO semantic_embeddings(community_id,unit_set_id,unit_key,generation_id,\
         dimensions,model_contract_digest,response_model,embedding) \
         VALUES($1,$2,'overview',$3,$4,$5,$6,$7)",
    )
    .bind(community_id)
    .bind(unit_set_id)
    .bind(generation_id)
    .bind(i32::try_from(contract.dimensions).expect("semantic dimensions"))
    .bind(contract_digest.as_bytes().as_slice())
    .bind(contract.model.as_str())
    .bind(Vector::from(vector))
    .execute(&mut **tx)
    .await
    .expect("insert semantic embedding");
    sqlx::query(
        "INSERT INTO semantic_source_generation_heads(community_id,generation_id,\
         source_family,source_subtype,source_id,unit_set_id,source_invalidation_epoch,\
         source_snapshot_digest,complete_unit_count,complete_embedding_count) \
         VALUES($1,$2,'project_document','document',$3,$4,1,$5,1,1)",
    )
    .bind(community_id)
    .bind(generation_id)
    .bind(document_id)
    .bind(unit_set_id)
    .bind(observation.snapshot_digest.as_bytes().as_slice())
    .execute(&mut **tx)
    .await
    .expect("insert semantic head");
}

fn fixture_uuid(seed: u64) -> Uuid {
    Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("UUIDv4 fixture")
}

#[tokio::test]
async fn one_hop_scoped_search_read_only_current_database_canary() {
    let Ok(database_url) = std::env::var("BUZZ_TEST_ONE_HOP_DATABASE_URL") else {
        return;
    };
    assert_eq!(
        std::env::var("BUZZ_TEST_ONE_HOP_READ_ONLY").as_deref(),
        Ok("scoped-search-v1"),
        "refusing one-hop canary without the explicit read-only marker"
    );
    assert!(
        database_url.contains("@127.0.0.1:") || database_url.contains("@localhost:"),
        "one-hop canary is restricted to a loopback PostgreSQL endpoint"
    );

    let db = Db::new(&DbConfig {
        database_url,
        ..DbConfig::default()
    })
    .await
    .expect("one-hop canary database");
    let row = sqlx::query(
        "SELECT community.id AS community_id, state.projection_pubkey, member.pubkey, \
                encode(edge.edge_key, 'hex') AS edge_key, \
                coordinate.coordinate_type, coordinate.coordinate_subtype, \
                coordinate.coordinate_id \
         FROM communities community \
         JOIN project_context_edge_state state ON state.community_id=community.id \
         JOIN project_context_edges edge \
           ON edge.community_id=community.id AND edge.state='active' \
         JOIN project_context_document_bindings binding \
           ON binding.community_id=edge.community_id AND binding.edge_key=edge.edge_key \
          AND binding.state='active' \
         JOIN project_context_edge_coordinates coordinate \
           ON coordinate.community_id=edge.community_id AND coordinate.edge_key=edge.edge_key \
         JOIN relay_members member ON member.community_id=community.id \
         WHERE community.archived_at IS NULL \
           AND community.project_view_enabled \
           AND community.project_document_enabled \
           AND community.meeting_community_read_enabled \
           AND community.project_context_edge_enabled \
           AND community.semantic_index_enabled \
           AND community.semantic_graph_query_enabled \
           AND community.semantic_active_generation_id IS NOT NULL \
           AND NOT EXISTS ( \
             SELECT 1 FROM community_bans ban \
             WHERE ban.community_id=member.community_id \
               AND ban.pubkey=decode(member.pubkey, 'hex') AND ban.banned \
               AND (ban.ban_expires_at IS NULL OR ban.ban_expires_at>clock_timestamp()) \
           ) \
         ORDER BY community.id, edge.edge_key, coordinate.ordinal, member.pubkey \
         LIMIT 1",
    )
    .fetch_one(db.writer())
    .await
    .expect("one-hop current graph fixture");

    let community_id: Uuid = row.try_get("community_id").expect("Community ID");
    let projection_bytes: Vec<u8> = row.try_get("projection_pubkey").expect("projection key");
    let projection = PublicKey::from_slice(&projection_bytes).expect("projection PublicKey");
    let reader_hex: String = row.try_get("pubkey").expect("reader key");
    let reader = PublicKey::from_hex(&reader_hex).expect("reader PublicKey");
    let edge_key = EdgeKey::from_hex(
        row.try_get::<String, _>("edge_key")
            .expect("EdgeKey")
            .as_str(),
    )
    .expect("canonical EdgeKey");
    let coordinate = coordinate_from_canary_row(&row);
    let community_id = CommunityId::from_uuid(community_id);
    let ticket = db
        .semantic_graph_query_ticket(community_id, reader.as_bytes(), &projection)
        .await
        .expect("one-hop ticket");
    let mut values = vec![0.0_f32; ticket.generation.model_contract.dimensions];
    values[0] = 1.0;
    let query = problem_vector(&ticket, Uuid::from_u128(0xA5), values);
    let mut read = db
        .begin_semantic_graph_read(
            &ticket,
            reader.as_bytes(),
            projection,
            super::super::SemanticGraphReadTimeouts::default(),
        )
        .await
        .expect("one-hop read snapshot");

    let incident = read
        .search_incident_edges_one_hop(&coordinate, &query, 8)
        .await
        .expect("one-hop incident search");
    let SemanticIncidentEdgeSearchOutcome::Ranked(incident) = incident else {
        panic!("current Coordinate must remain incident")
    };
    let OneHopSemanticSelection::IncidentEdges { edges, .. } = &incident.selection else {
        panic!("incident result variant")
    };
    assert!(!edges.is_empty());
    assert!(edges.iter().all(|edge| {
        !edge.ranked_documents.is_empty()
            && edge.ranked_documents.iter().all(|document| {
                !document
                    .canonical_observation
                    .preview
                    .title
                    .trim()
                    .is_empty()
            })
    }));

    let members = read
        .search_edge_coordinates_one_hop(edge_key, &query, 8)
        .await
        .expect("one-hop Edge member search");
    let SemanticEdgeCoordinateSearchOutcome::Ranked(members) = members else {
        panic!("current Edge must remain active")
    };
    let OneHopSemanticSelection::EdgeCoordinates {
        ranked_coordinates, ..
    } = &members.selection
    else {
        panic!("Edge Coordinate result variant")
    };
    assert!(!ranked_coordinates.is_empty());
    assert!(ranked_coordinates.iter().all(|candidate| {
        !candidate
            .canonical_observation
            .preview
            .title
            .trim()
            .is_empty()
    }));
    read.rollback().await.expect("one-hop canary rollback");
}

fn coordinate_from_canary_row(row: &sqlx::postgres::PgRow) -> ProjectContextCoordinate {
    let coordinate_type: String = row.try_get("coordinate_type").expect("Coordinate type");
    let coordinate_subtype: Option<String> = row
        .try_get("coordinate_subtype")
        .expect("Coordinate subtype");
    let coordinate_id: Uuid = row.try_get("coordinate_id").expect("Coordinate ID");
    match (coordinate_type.as_str(), coordinate_subtype.as_deref()) {
        ("project_view_object", Some(subtype)) => ProjectContextCoordinate::ProjectViewObject {
            object_type: project_view_type(subtype),
            object_id: coordinate_id,
        },
        ("document", None) => ProjectContextCoordinate::Document {
            document_id: coordinate_id,
        },
        ("meeting", None) => ProjectContextCoordinate::Meeting {
            meeting_id: coordinate_id,
        },
        _ => panic!("unsupported canary Coordinate"),
    }
}

fn project_view_type(value: &str) -> ProjectViewObjectType {
    match value {
        "project_profile" => ProjectViewObjectType::ProjectProfile,
        "goal" => ProjectViewObjectType::Goal,
        "role" => ProjectViewObjectType::Role,
        "plan" => ProjectViewObjectType::Plan,
        "stage" => ProjectViewObjectType::Stage,
        "requirement" => ProjectViewObjectType::Requirement,
        "issue" => ProjectViewObjectType::Issue,
        "work" => ProjectViewObjectType::Work,
        "resource" => ProjectViewObjectType::Resource,
        _ => panic!("unsupported canary Project View type"),
    }
}
