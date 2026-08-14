//! Ignored, disposable-PostgreSQL qualification for the production
//! Coordinate-search SQL. The committed runner invokes this test explicitly;
//! ordinary unit tests never allocate the target-scale fixture.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use buzz_core::{CommunityId, Keys, PublicKey};
use buzz_semantic::{
    SemanticDistanceMetric, SemanticModelContract, SemanticNormalization, SemanticProviderBoundary,
};
use buzz_semantic_query::{
    build_coordinate_search_encoder_input, EncodedCoordinateSearchQuery,
    ProjectContextCoordinateSearchQuery, QueryCompatibilityFences,
};
use pgvector::Vector;
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

use super::{SemanticCoordinateSearchVector, COORDINATE_SEARCH_SQL};
use crate::semantic::SemanticGenerationRecord;
use crate::semantic_query::{SemanticGraphQueryTicket, SemanticGraphReadTx};
use crate::{Db, DbConfig};

const DEFAULT_TARGET_COORDINATES: u32 = 10_000;
const DEFAULT_MISSING_HEAD_COORDINATES: u32 = 1_000;
const DEFAULT_DISTRACTOR_SOURCES: u32 = 5_000;
const DEFAULT_ITERATIONS: usize = 15;
const DEFAULT_SOAK_SECONDS: u64 = 8;
const DEFAULT_CLIENTS: usize = 4;
const DIMENSIONS: usize = 2_048;

fn positive_env_u32(name: &str, default: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn positive_env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn percentile(sorted: &[u128], percentile: usize) -> u128 {
    let index = sorted
        .len()
        .saturating_mul(percentile)
        .div_ceil(100)
        .saturating_sub(1)
        .min(sorted.len().saturating_sub(1));
    sorted[index]
}

fn plan_blocks(plan: &Value, key: &str) -> u64 {
    plan.get("Plan")
        .and_then(|root| root.get(key))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn contract() -> SemanticModelContract {
    SemanticModelContract {
        provider: "deterministic_fake".to_owned(),
        model: "deterministic-fake-v1".to_owned(),
        dimensions: DIMENSIONS,
        distance_metric: SemanticDistanceMetric::Cosine,
        normalization: SemanticNormalization::None,
        input_contract_version: "overview-v1".to_owned(),
        provider_boundary: SemanticProviderBoundary::DeterministicFake,
    }
}

fn query_embedding() -> Vec<f32> {
    let mut embedding = vec![0.0001_f32; DIMENSIONS];
    embedding[..8].copy_from_slice(&[1.0, 0.8, 0.6, 0.4, 0.2, 0.1, 0.05, 0.025]);
    embedding
}

struct TargetScaleSeed<'a> {
    db: &'a Db,
    community_id: Uuid,
    generation_id: Uuid,
    reader: &'a Keys,
    relay: &'a Keys,
    contract: &'a SemanticModelContract,
    target: u32,
    missing: u32,
    distractors: u32,
}

async fn seed_target_scale(seed: TargetScaleSeed<'_>) {
    let TargetScaleSeed {
        db,
        community_id,
        generation_id,
        reader,
        relay,
        contract,
        target,
        missing,
        distractors,
    } = seed;
    let contract_digest = contract.digest().expect("qualification model digest");
    let bytes = vec![7_u8; 32];
    let active_edges = (u64::from(target) + u64::from(missing)).div_ceil(2);
    let mut tx = db.writer().begin().await.expect("qualification seed tx");
    sqlx::query("SET LOCAL session_replication_role = 'replica'")
        .execute(&mut *tx)
        .await
        .expect("disable triggers in disposable scale fixture");
    sqlx::query(
        "CREATE FUNCTION pg_temp.coordinate_search_qualification_uuid(seed text) \
         RETURNS uuid LANGUAGE SQL IMMUTABLE PARALLEL SAFE AS $$ \
         SELECT (substr(md5(seed),1,8)||'-'||substr(md5(seed),9,4)||'-4'||\
                 substr(md5(seed),14,3)||'-8'||substr(md5(seed),18,3)||'-'||\
                 substr(md5(seed),21,12))::uuid $$",
    )
    .execute(&mut *tx)
    .await
    .expect("qualification UUIDv4 helper");
    sqlx::query(
        "INSERT INTO communities(\
         id,host,project_view_enabled,project_document_enabled,project_context_enabled,\
         project_context_edge_enabled,project_view_schema_version,\
         meeting_community_read_enabled,meeting_community_read_enabled_at,\
         legacy_meeting_visibility_watermark,legacy_meeting_visibility_audit_digest,\
         legacy_meeting_visibility_meeting_count,\
         legacy_meeting_visibility_community_source_count,\
         legacy_meeting_visibility_private_source_count,\
         legacy_meeting_visibility_missing_source_count,\
         legacy_meeting_visibility_audited_at,legacy_meeting_visibility_approved_at,\
         legacy_meeting_visibility_approved_by,semantic_index_enabled,\
         semantic_graph_query_enabled) \
         VALUES($1,$2,TRUE,TRUE,TRUE,TRUE,3,TRUE,clock_timestamp(),0,$3,0,0,0,0,\
                clock_timestamp(),clock_timestamp(),'coordinate-search-qualification',TRUE,TRUE)",
    )
    .bind(community_id)
    .bind(format!(
        "coordinate-search-qualification-{community_id}.invalid"
    ))
    .bind(&bytes)
    .execute(&mut *tx)
    .await
    .expect("qualification Community");
    sqlx::query("INSERT INTO relay_members(community_id,pubkey,role) VALUES($1,$2,'member')")
        .bind(community_id)
        .bind(reader.public_key().to_hex())
        .execute(&mut *tx)
        .await
        .expect("qualification reader");
    sqlx::query(
        "INSERT INTO project_view_maintenance(community_id,state,updated_at) \
         VALUES($1,'normal',clock_timestamp()) ON CONFLICT (community_id) DO UPDATE \
         SET state='normal',current_epoch=NULL,updated_at=EXCLUDED.updated_at",
    )
    .bind(community_id)
    .execute(&mut *tx)
    .await
    .expect("qualification maintenance");
    sqlx::query(
        "INSERT INTO project_view_state(community_id,project_revision,active_object_count,\
         initialized_at,updated_at,last_event_id,last_actor_pubkey,meta_projection_event_id,\
         projection_pubkey,projection_generation,schema_version,last_change_id,last_source_event_id) \
         VALUES($1,1,$4,clock_timestamp(),clock_timestamp(),$2,$2,$2,$3,1,3,$2,$2)",
    )
    .bind(community_id)
    .bind(&bytes)
    .bind(relay.public_key().as_bytes())
    .bind(i64::from(target) + i64::from(missing))
    .execute(&mut *tx)
    .await
    .expect("qualification Project View state");
    sqlx::query(
        "INSERT INTO project_document_state(community_id,schema_version,catalog_revision,\
         active_document_count,last_change_id,last_actor_pubkey,projection_pubkey,\
         projection_generation,meta_projection_event_id,initialized_at,updated_at) \
         VALUES($1,1,0,0,NULL,NULL,$2,1,$3,clock_timestamp(),clock_timestamp())",
    )
    .bind(community_id)
    .bind(relay.public_key().as_bytes())
    .bind(&bytes)
    .execute(&mut *tx)
    .await
    .expect("qualification Project Document state");
    sqlx::query(
        "INSERT INTO project_context_edge_state(community_id,schema_version,context_revision,\
         active_edge_count,bound_document_count,last_change_id,last_actor_pubkey,\
         projection_pubkey,projection_generation,meta_projection_event_id,initialized_at,updated_at) \
         VALUES($1,2,1,$4,$4,$2,$2,$3,1,$2,clock_timestamp(),clock_timestamp())",
    )
    .bind(community_id)
    .bind(&bytes)
    .bind(relay.public_key().as_bytes())
    .bind(i64::try_from(active_edges).expect("edge count"))
    .execute(&mut *tx)
    .await
    .expect("qualification Project Context state");
    sqlx::query(
        "INSERT INTO semantic_index_generations(community_id,generation_id,lifecycle,\
         extractor_version,input_contract_version,provider,model,dimensions,distance_metric,\
         normalization,provider_boundary,model_contract_digest,created_by,rebuild_completed_at,\
         ready_at,activated_at) \
         VALUES($1,$2,'active','overview-v1',$3,$4,$5,$6,'cosine','none',\
                'deterministic_fake',$7,'coordinate-search-qualification',\
                clock_timestamp(),clock_timestamp(),clock_timestamp())",
    )
    .bind(community_id)
    .bind(generation_id)
    .bind(contract.input_contract_version.as_str())
    .bind(contract.provider.as_str())
    .bind(contract.model.as_str())
    .bind(i32::try_from(DIMENSIONS).expect("dimensions"))
    .bind(contract_digest.as_bytes().as_slice())
    .execute(&mut *tx)
    .await
    .expect("qualification generation");
    sqlx::query("UPDATE communities SET semantic_active_generation_id=$2 WHERE id=$1")
        .bind(community_id)
        .bind(generation_id)
        .execute(&mut *tx)
        .await
        .expect("qualification active generation");

    sqlx::query(
        "INSERT INTO project_context_edges(community_id,edge_key,state,canonical_coordinates,\
         last_context_revision,current_source_change_id,updated_at,updated_by) \
         SELECT $1,decode(md5('active-edge-'||n)||md5('active-edge-b-'||n),'hex'),\
                'active',jsonb_build_array('active-'||n||'-a','active-'||n||'-b'),1,$2,\
                clock_timestamp(),$2 \
         FROM generate_series(1,$3::bigint) n",
    )
    .bind(community_id)
    .bind(&bytes)
    .bind(i64::try_from(active_edges).expect("active edges"))
    .execute(&mut *tx)
    .await
    .expect("qualification active Edges");
    sqlx::query(
        "INSERT INTO project_context_edge_coordinates(community_id,edge_key,ordinal,\
         coordinate_type,coordinate_subtype,coordinate_id,canonical_key) \
         SELECT $1,decode(md5('active-edge-'||((n+1)/2))||\
                          md5('active-edge-b-'||((n+1)/2)),'hex'),\
                ((n-1)%2)::int,'project_view_object',\
                (ARRAY['goal','role','plan','stage','requirement','issue','work','resource'])\
                    [1+((n-1)%8)::int],\
                pg_temp.coordinate_search_qualification_uuid('active-coordinate-'||n),\
                'pv:'||$1::text||':'||\
                (ARRAY['goal','role','plan','stage','requirement','issue','work','resource'])\
                    [1+((n-1)%8)::int]||':'||\
                    pg_temp.coordinate_search_qualification_uuid('active-coordinate-'||n)::text \
         FROM generate_series(1,$2::bigint) n",
    )
    .bind(community_id)
    .bind(i64::from(target) + i64::from(missing))
    .execute(&mut *tx)
    .await
    .expect("qualification active Coordinates");

    sqlx::query(
        "INSERT INTO project_context_edges(community_id,edge_key,state,canonical_coordinates,\
         last_context_revision,current_source_change_id,updated_at,updated_by) \
         SELECT $1,decode(md5('deleted-edge-'||n)||md5('deleted-edge-b-'||n),'hex'),\
                'deleted',jsonb_build_array('deleted-'||n||'-a','deleted-'||n||'-b'),1,$2,\
                clock_timestamp(),$2 FROM generate_series(1,$3::int) n",
    )
    .bind(community_id)
    .bind(&bytes)
    .bind(i32::try_from(distractors.div_ceil(2)).expect("deleted Edge count"))
    .execute(&mut *tx)
    .await
    .expect("qualification deleted Edges");
    sqlx::query(
        "INSERT INTO project_context_edge_coordinates(community_id,edge_key,ordinal,\
         coordinate_type,coordinate_subtype,coordinate_id,canonical_key) \
         SELECT $1,decode(md5('deleted-edge-'||((n+1)/2))||\
                          md5('deleted-edge-b-'||((n+1)/2)),'hex'),\
                ((n-1)%2)::int,'project_view_object','work',\
                pg_temp.coordinate_search_qualification_uuid('deleted-coordinate-'||n),\
                'pv:'||$1::text||':work:'||\
                pg_temp.coordinate_search_qualification_uuid('deleted-coordinate-'||n)::text \
         FROM generate_series(1,$2::int) n",
    )
    .bind(community_id)
    .bind(i32::try_from(distractors).expect("distractor count"))
    .execute(&mut *tx)
    .await
    .expect("qualification deleted-edge Coordinates");

    sqlx::query(
        "CREATE TEMP TABLE coordinate_search_qualification_seed ON COMMIT DROP AS \
         SELECT n::bigint AS ordinal,'project_view'::text AS source_family,\
                (ARRAY['goal','role','plan','stage','requirement','issue','work','resource'])\
                    [1+((n-1)%8)::int] AS source_subtype,\
                pg_temp.coordinate_search_qualification_uuid('active-coordinate-'||n) AS source_id,\
                pg_temp.coordinate_search_qualification_uuid('unit-active-'||n) AS unit_set_id,\
                decode(md5('snapshot-active-'||n)||md5('snapshot-active-b-'||n),'hex') AS snapshot_digest \
         FROM generate_series(1,$1::bigint) n \
         UNION ALL \
         SELECT ($1::bigint+n)::bigint,'project_view','work',\
                pg_temp.coordinate_search_qualification_uuid('deleted-coordinate-'||n),\
                pg_temp.coordinate_search_qualification_uuid('unit-deleted-'||n),\
                decode(md5('snapshot-deleted-'||n)||md5('snapshot-deleted-b-'||n),'hex') \
         FROM generate_series(1,$2::bigint) n \
         UNION ALL \
         SELECT ($1::bigint+$2::bigint+n)::bigint,'project_document','document',\
                pg_temp.coordinate_search_qualification_uuid('binding-only-document-'||n),\
                pg_temp.coordinate_search_qualification_uuid('unit-external-'||n),\
                decode(md5('snapshot-external-'||n)||md5('snapshot-external-b-'||n),'hex') \
         FROM generate_series(1,$2::bigint) n",
    )
    .bind(i64::from(target))
    .bind(i64::from(distractors))
    .execute(&mut *tx)
    .await
    .expect("qualification semantic seed");
    sqlx::query(
        "INSERT INTO semantic_sources(community_id,source_family,source_subtype,source_id,\
         eligibility,lifecycle_class,source_basis,snapshot_digest,invalidation_epoch,\
         coverage_state,observed_at) \
         SELECT $1,source_family,source_subtype,source_id,'eligible','active','{}'::jsonb,\
                snapshot_digest,1,'current',clock_timestamp() \
         FROM coordinate_search_qualification_seed",
    )
    .bind(community_id)
    .execute(&mut *tx)
    .await
    .expect("qualification semantic sources");
    sqlx::query(
        "INSERT INTO semantic_unit_sets(community_id,unit_set_id,source_family,source_subtype,\
         source_id,source_invalidation_epoch,source_basis,source_snapshot_digest,\
         extractor_version,state,complete_unit_count,activated_at) \
         SELECT $1,unit_set_id,source_family,source_subtype,source_id,1,'{}'::jsonb,\
                snapshot_digest,'overview-v1','active',1,clock_timestamp() \
         FROM coordinate_search_qualification_seed",
    )
    .bind(community_id)
    .execute(&mut *tx)
    .await
    .expect("qualification unit sets");
    sqlx::query(
        "INSERT INTO semantic_units(community_id,unit_set_id,unit_key,ordinal,unit_kind,\
         semantic_text,semantic_text_digest,summary_coverage,extraction_provenance) \
         SELECT $1,unit_set_id,'overview',0,'overview','content-free qualification fixture',\
                decode(md5('text-'||ordinal)||md5('text-b-'||ordinal),'hex'),\
                'title_only','{}'::jsonb FROM coordinate_search_qualification_seed",
    )
    .bind(community_id)
    .execute(&mut *tx)
    .await
    .expect("qualification semantic units");
    sqlx::query(
        "INSERT INTO semantic_embeddings(community_id,unit_set_id,unit_key,generation_id,\
         dimensions,model_contract_digest,response_model,embedding) \
         SELECT $1,unit_set_id,'overview',$2,$3,$4,$5,\
                (ARRAY[((ordinal%97)+1)::real/97.0::real,\
                       ((ordinal%89)+1)::real/89.0::real,\
                       ((ordinal%83)+1)::real/83.0::real,\
                       ((ordinal%79)+1)::real/79.0::real,\
                       ((ordinal%73)+1)::real/73.0::real,\
                       ((ordinal%71)+1)::real/71.0::real,\
                       ((ordinal%67)+1)::real/67.0::real,\
                       ((ordinal%61)+1)::real/61.0::real] ||\
                       array_fill(0.0001::real,ARRAY[2040]))::vector \
         FROM coordinate_search_qualification_seed",
    )
    .bind(community_id)
    .bind(generation_id)
    .bind(i32::try_from(DIMENSIONS).expect("dimensions"))
    .bind(contract_digest.as_bytes().as_slice())
    .bind(contract.model.as_str())
    .execute(&mut *tx)
    .await
    .expect("qualification embeddings");
    sqlx::query(
        "INSERT INTO semantic_source_generation_heads(community_id,generation_id,source_family,\
         source_subtype,source_id,unit_set_id,source_invalidation_epoch,\
         source_snapshot_digest,complete_unit_count,complete_embedding_count) \
         SELECT $1,$2,source_family,source_subtype,source_id,unit_set_id,1,\
                snapshot_digest,1,1 FROM coordinate_search_qualification_seed",
    )
    .bind(community_id)
    .bind(generation_id)
    .execute(&mut *tx)
    .await
    .expect("qualification current heads");
    tx.commit().await.expect("commit qualification fixture");

    for table in [
        "project_context_edges",
        "project_context_edge_coordinates",
        "semantic_sources",
        "semantic_unit_sets",
        "semantic_units",
        "semantic_embeddings",
        "semantic_source_generation_heads",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!("ANALYZE {table}")))
            .execute(db.writer())
            .await
            .expect("analyze qualification table");
    }
}

async fn begin_qualification_read(
    db: &Db,
    ticket: &SemanticGraphQueryTicket,
    reader_pubkey: &[u8],
    expected_projection_pubkey: PublicKey,
) -> SemanticGraphReadTx {
    let mut tx = db.writer().begin().await.expect("qualification read tx");
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await
        .expect("qualification repeatable-read snapshot");
    sqlx::query("SET LOCAL statement_timeout = '30s'")
        .execute(&mut *tx)
        .await
        .expect("qualification statement timeout");
    SemanticGraphReadTx {
        tx,
        ticket: ticket.clone(),
        reader_pubkey: reader_pubkey.to_vec(),
        expected_projection_pubkey,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "requires an isolated disposable PostgreSQL/pgvector qualification database"]
async fn coordinate_search_target_scale_exact_sql_qualification() {
    let database_url =
        std::env::var("BUZZ_TEST_SEMANTIC_DATABASE_URL").expect("BUZZ_TEST_SEMANTIC_DATABASE_URL");
    assert_eq!(
        std::env::var("BUZZ_TEST_SEMANTIC_DISPOSABLE").as_deref(),
        Ok("coordinate-search-qualification-v1"),
        "refusing Coordinate-search qualification without the disposable marker"
    );
    assert!(database_url.contains("@127.0.0.1:"));
    assert!(database_url.contains("/buzz_coordinate_search_qualification"));

    let target = positive_env_u32(
        "COORDINATE_SEARCH_QUALIFICATION_TARGET_COORDINATES",
        DEFAULT_TARGET_COORDINATES,
    );
    let missing = positive_env_u32(
        "COORDINATE_SEARCH_QUALIFICATION_MISSING_HEAD_COORDINATES",
        DEFAULT_MISSING_HEAD_COORDINATES,
    );
    let distractors = positive_env_u32(
        "COORDINATE_SEARCH_QUALIFICATION_DISTRACTOR_SOURCES",
        DEFAULT_DISTRACTOR_SOURCES,
    );
    let iterations = positive_env_usize(
        "COORDINATE_SEARCH_QUALIFICATION_ITERATIONS",
        DEFAULT_ITERATIONS,
    );
    let clients = positive_env_usize("COORDINATE_SEARCH_QUALIFICATION_CLIENTS", DEFAULT_CLIENTS);
    let soak_seconds = u64::from(positive_env_u32(
        "COORDINATE_SEARCH_QUALIFICATION_SOAK_SECONDS",
        u32::try_from(DEFAULT_SOAK_SECONDS).expect("default soak"),
    ));
    assert!(target >= 32);
    assert!(clients <= 16);

    let db = Db::new(&DbConfig {
        database_url,
        max_connections: u32::try_from(clients + 4).expect("pool size"),
        ..DbConfig::default()
    })
    .await
    .expect("qualification database");
    db.migrate().await.expect("qualification migrations");
    let postgres_version: String = sqlx::query_scalar("SHOW server_version_num")
        .fetch_one(db.writer())
        .await
        .expect("PostgreSQL version");
    let pgvector_version: String =
        sqlx::query_scalar("SELECT extversion FROM pg_extension WHERE extname='vector'")
            .fetch_one(db.writer())
            .await
            .expect("pgvector version");
    assert!(postgres_version.starts_with("17"));
    assert_eq!(pgvector_version, "0.8.5");
    let community_id = Uuid::new_v4();
    let generation_id = Uuid::new_v4();
    let reader = Keys::generate();
    let relay = Keys::generate();
    let model_contract = contract();
    seed_target_scale(TargetScaleSeed {
        db: &db,
        community_id,
        generation_id,
        reader: &reader,
        relay: &relay,
        contract: &model_contract,
        target,
        missing,
        distractors,
    })
    .await;

    let observed_at = chrono::Utc::now();
    let ticket = SemanticGraphQueryTicket {
        community_id: CommunityId::from_uuid(community_id),
        generation: SemanticGenerationRecord {
            community_id: CommunityId::from_uuid(community_id),
            generation_id,
            lifecycle: "active".to_owned(),
            extractor_version: "overview-v1".to_owned(),
            model_contract: model_contract.clone(),
            model_contract_digest: model_contract.digest().expect("qualification model digest"),
            rebuild_completed_at: Some(observed_at),
            created_at: observed_at,
        },
        query_fences: QueryCompatibilityFences::for_source_contract(&model_contract)
            .expect("qualification query fences"),
        projection_generation: 1,
        project_context_revision: 1,
        observed_at,
    };
    let request = ProjectContextCoordinateSearchQuery {
        request_id: Uuid::new_v4(),
        project_id: community_id,
        query: "content-free target-scale starting point qualification".to_owned(),
        limit: 32,
    };
    let input = build_coordinate_search_encoder_input(&request).expect("qualification input");
    let encoded = EncodedCoordinateSearchQuery::new(
        &input,
        model_contract.model.clone(),
        query_embedding(),
        &model_contract,
    )
    .expect("qualification vector");
    let query_vector = Arc::new(
        SemanticCoordinateSearchVector::new(&ticket, encoded)
            .expect("snapshot-bound qualification vector"),
    );
    for _ in 0..3 {
        let mut read = begin_qualification_read(
            &db,
            &ticket,
            reader.public_key().as_bytes(),
            relay.public_key(),
        )
        .await;
        let batch = read
            .search_coordinate_starts(&query_vector, 32)
            .await
            .expect("warmup query");
        assert_eq!(batch.coordinates.len(), 32);
        assert!(batch.truncated);
        read.rollback().await.expect("warmup rollback");
    }

    let mut sequential_ms = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        let mut read = begin_qualification_read(
            &db,
            &ticket,
            reader.public_key().as_bytes(),
            relay.public_key(),
        )
        .await;
        let batch = read
            .search_coordinate_starts(&query_vector, 32)
            .await
            .expect("timed query");
        assert_eq!(batch.coordinates.len(), 32);
        assert!(batch.truncated);
        read.rollback().await.expect("timed rollback");
        sequential_ms.push(started.elapsed().as_millis());
    }
    sequential_ms.sort_unstable();

    let explain_sql =
        format!("EXPLAIN (ANALYZE, BUFFERS, WAL, SETTINGS, FORMAT JSON) {COORDINATE_SEARCH_SQL}");
    let explain_row = sqlx::query(sqlx::AssertSqlSafe(explain_sql))
        .bind(community_id)
        .bind(reader.public_key().as_bytes())
        .bind(relay.public_key().as_bytes())
        .bind(generation_id)
        .bind(
            ticket
                .generation
                .model_contract_digest
                .as_bytes()
                .as_slice(),
        )
        .bind(ticket.generation.extractor_version.as_str())
        .bind(ticket.generation.model_contract.model.as_str())
        .bind(i32::try_from(DIMENSIONS).expect("dimensions"))
        .bind(Vector::from(query_vector.embedding.as_slice().to_vec()))
        .bind(33_i64)
        .fetch_one(db.writer())
        .await
        .expect("qualification EXPLAIN");
    let plan: Value = explain_row.try_get(0).expect("EXPLAIN JSON");
    let plan_root = &plan[0];

    let latencies = Arc::new(Mutex::new(Vec::<u128>::new()));
    let errors = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let deadline = Instant::now() + Duration::from_secs(soak_seconds);
    let mut tasks = Vec::with_capacity(clients);
    for _ in 0..clients {
        let db = db.clone();
        let ticket = ticket.clone();
        let query_vector = Arc::clone(&query_vector);
        let latencies = Arc::clone(&latencies);
        let errors = Arc::clone(&errors);
        let reader_pubkey = reader.public_key().to_bytes();
        let relay_pubkey = relay.public_key();
        tasks.push(tokio::spawn(async move {
            while Instant::now() < deadline {
                let started = Instant::now();
                let result = async {
                    let mut read =
                        begin_qualification_read(&db, &ticket, &reader_pubkey, relay_pubkey).await;
                    let batch = read.search_coordinate_starts(&query_vector, 32).await?;
                    if batch.coordinates.len() != 32 || !batch.truncated {
                        return Err(crate::DbError::InvalidData(
                            "qualification query returned an unexpected shape".to_owned(),
                        ));
                    }
                    read.rollback().await
                }
                .await;
                match result {
                    Ok(()) => latencies
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push(started.elapsed().as_millis()),
                    Err(_) => {
                        errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }
        }));
    }
    for task in tasks {
        task.await.expect("qualification client task");
    }
    let errors = errors.load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(errors, 0, "qualification soak must have no query failures");
    let mut concurrent_ms = latencies
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    assert!(!concurrent_ms.is_empty());
    concurrent_ms.sort_unstable();

    let summary = json!({
        "status": "measurement_complete_slo_not_frozen",
        "postgres_version_num": postgres_version,
        "pgvector": pgvector_version,
        "dimensions": DIMENSIONS,
        "target_indexed_coordinates": target,
        "active_coordinates_missing_head": missing,
        "deleted_edge_indexed_distractors": distractors,
        "graph_external_indexed_distractors": distractors,
        "limit": 32,
        "sequential": {
            "iterations": sequential_ms.len(),
            "p50_ms": percentile(&sequential_ms, 50),
            "p95_ms": percentile(&sequential_ms, 95),
            "p99_ms": percentile(&sequential_ms, 99),
        },
        "concurrent_soak": {
            "seconds": soak_seconds,
            "clients": clients,
            "completed": concurrent_ms.len(),
            "errors": errors,
            "p50_ms": percentile(&concurrent_ms, 50),
            "p95_ms": percentile(&concurrent_ms, 95),
            "p99_ms": percentile(&concurrent_ms, 99),
        },
        "explain": {
            "planning_ms": plan_root.get("Planning Time").and_then(Value::as_f64),
            "execution_ms": plan_root.get("Execution Time").and_then(Value::as_f64),
            "shared_hit_blocks": plan_blocks(plan_root, "Shared Hit Blocks"),
            "shared_read_blocks": plan_blocks(plan_root, "Shared Read Blocks"),
            "shared_dirtied_blocks": plan_blocks(plan_root, "Shared Dirtied Blocks"),
            "shared_written_blocks": plan_blocks(plan_root, "Shared Written Blocks"),
            "temp_read_blocks": plan_blocks(plan_root, "Temp Read Blocks"),
            "temp_written_blocks": plan_blocks(plan_root, "Temp Written Blocks"),
        },
    });
    println!("coordinate_search_qualification={summary}");
}
