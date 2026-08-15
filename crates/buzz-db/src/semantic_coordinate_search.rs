//! Current, authorized Coordinate-only semantic starting-point search.
//!
//! This module deliberately excludes Context Documents that are only Edge
//! bindings and returns no source text, preview, Edge, or path. Its candidates
//! are the distinct Coordinates of current active Project Context Edges that
//! also have exact-current overview embeddings in the active generation.

use buzz_project_context::ProjectContextCoordinate;
use buzz_project_view::ProjectViewObjectType;
use buzz_semantic::{ProjectViewSemanticType, SemanticSourceIdentity, SemanticSourceKind};
use buzz_semantic_query::{
    coordinate_search_query_contract_digest, EncodedCoordinateSearchQuery,
    ProjectContextCoordinateSearchCandidate, Score, SemanticComputationRoute,
    SemanticQueryInputKind, MAX_COORDINATE_SEARCH_LIMIT, SEMANTIC_COMPUTATION_ROUTES,
};
use pgvector::Vector;
use sqlx::Row;

use crate::semantic_query::{
    GenerationBoundQueryVector, SemanticGraphQueryTicket, SemanticGraphReadTx,
    SemanticGraphSnapshotBinding,
};
use crate::{DbError, Result};

/// One snapshot-bound Coordinate-only exact search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticCoordinateSearchBatch {
    /// Current generation and Project Context snapshot used for every row.
    pub snapshot: SemanticGraphSnapshotBinding,
    /// Public top-K candidates in deterministic score/Coordinate order.
    pub coordinates: Vec<ProjectContextCoordinateSearchCandidate>,
    /// Whether an eligible K+1 row existed in this same snapshot.
    pub truncated: bool,
}

/// One Coordinate-search vector bound to the active Foundation generation.
pub struct SemanticCoordinateSearchVector {
    inner: GenerationBoundQueryVector,
}

impl SemanticCoordinateSearchVector {
    /// Bind one validated Provider response to the exact query ticket.
    pub fn new(
        ticket: &SemanticGraphQueryTicket,
        encoded: EncodedCoordinateSearchQuery,
    ) -> Result<Self> {
        if !matches!(
            encoded.provider_encoded().channel_kind(),
            SemanticQueryInputKind::CoordinateSearch
        ) || encoded.query_contract_digest() != coordinate_search_query_contract_digest()
        {
            return Err(DbError::InvalidData(
                "Coordinate-search Provider result does not match the active generation".to_owned(),
            ));
        }
        Ok(Self {
            inner: GenerationBoundQueryVector::bind(ticket, encoded.into_provider_encoded())?,
        })
    }

    /// Independently versioned query contract digest.
    pub const fn query_contract_digest(&self) -> buzz_semantic::Digest32 {
        self.inner.encoding_contract_digest()
    }

    /// Exact canonical Provider input digest.
    pub const fn query_input_digest(&self) -> buzz_semantic::Digest32 {
        self.inner.input_digest()
    }

    pub(super) const fn generation_bound(&self) -> &GenerationBoundQueryVector {
        &self.inner
    }
}

impl SemanticGraphReadTx {
    /// Rank current active-edge Coordinates by one exact query vector.
    ///
    /// The vector must have been constructed against this transaction's
    /// generation ticket. The query has no relevance floor: an empty result
    /// means only that no current indexed eligible Coordinate was available.
    pub async fn search_coordinate_starts(
        &mut self,
        query_vector: &SemanticCoordinateSearchVector,
        limit: u8,
    ) -> Result<SemanticCoordinateSearchBatch> {
        match SEMANTIC_COMPUTATION_ROUTES.whole_graph_coordinate_discovery {
            SemanticComputationRoute::Legacy => {
                self.search_coordinate_starts_legacy(query_vector, limit)
                    .await
            }
            SemanticComputationRoute::Migrated => {
                self.search_coordinate_starts_migrated(query_vector, limit)
                    .await
            }
        }
    }

    /// Shared scorer selected by the compiled U6 production profile.
    ///
    /// The explicit entry remains available to same-snapshot differential
    /// qualification while the legacy implementation is retained for the
    /// documented profile rollback window.
    pub(crate) async fn search_coordinate_starts_migrated(
        &mut self,
        query_vector: &SemanticCoordinateSearchVector,
        limit: u8,
    ) -> Result<SemanticCoordinateSearchBatch> {
        self.validate_coordinate_search(query_vector, limit)?;
        let observed_limit = u32::from(limit) + 1;
        let scores = self
            .score_global_graph_coordinates_exact(query_vector.generation_bound(), observed_limit)
            .await?;
        let truncated = scores.len() > usize::from(limit);
        let coordinates = scores
            .into_iter()
            .take(usize::from(limit))
            .enumerate()
            .map(|(index, scored)| {
                let expected_rank = u32::try_from(index + 1).map_err(|_| {
                    DbError::InvalidData("Coordinate-search rank exceeds uint32".to_owned())
                })?;
                if scored.channel_id != query_vector.inner.channel_id()
                    || scored.channel_rank != expected_rank
                {
                    return Err(DbError::InvalidData(
                        "global Coordinate scope returned a non-canonical channel rank".to_owned(),
                    ));
                }
                let rank = u8::try_from(index + 1).map_err(|_| {
                    DbError::InvalidData("Coordinate-search rank exceeds uint8".to_owned())
                })?;
                Ok(ProjectContextCoordinateSearchCandidate {
                    rank,
                    coordinate: coordinate_from_source(&scored.source)?,
                    score: scored.score,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(SemanticCoordinateSearchBatch {
            snapshot: self.coordinate_search_snapshot(),
            coordinates,
            truncated,
        })
    }

    fn validate_coordinate_search(
        &self,
        query_vector: &SemanticCoordinateSearchVector,
        limit: u8,
    ) -> Result<()> {
        if !(1..=MAX_COORDINATE_SEARCH_LIMIT).contains(&limit) {
            return Err(DbError::InvalidData(format!(
                "Coordinate-search limit must be between 1 and {MAX_COORDINATE_SEARCH_LIMIT}"
            )));
        }
        if !matches!(
            query_vector.inner.channel_kind(),
            SemanticQueryInputKind::CoordinateSearch
        ) || query_vector.query_contract_digest() != coordinate_search_query_contract_digest()
            || query_vector.inner.generation_fences() != &self.ticket.generation_fences()?
        {
            return Err(DbError::InvalidData(
                "Coordinate-search vector does not match the query contract".to_owned(),
            ));
        }
        Ok(())
    }

    fn coordinate_search_snapshot(&self) -> SemanticGraphSnapshotBinding {
        SemanticGraphSnapshotBinding {
            community_id: self.ticket.community_id,
            generation_id: self.ticket.generation.generation_id,
            query_fences: self.ticket.query_fences,
            extractor_version: self.ticket.generation.extractor_version.clone(),
            project_context_revision: self.ticket.project_context_revision,
            observed_at: self.ticket.observed_at,
        }
    }

    /// Retained during the profile rollback window and used by same-snapshot
    /// differential qualification. It is never selected dynamically inside a
    /// request.
    async fn search_coordinate_starts_legacy(
        &mut self,
        query_vector: &SemanticCoordinateSearchVector,
        limit: u8,
    ) -> Result<SemanticCoordinateSearchBatch> {
        self.validate_coordinate_search(query_vector, limit)?;
        let dimensions = i32::try_from(self.ticket.generation.model_contract.dimensions)
            .map_err(|_| DbError::InvalidData("semantic dimensions exceed int4".to_owned()))?;
        let observed_limit = i64::from(limit) + 1;
        let rows = sqlx::query(COORDINATE_SEARCH_SQL)
            .bind(self.ticket.community_id.as_uuid())
            .bind(self.reader_pubkey.as_slice())
            .bind(self.expected_projection_pubkey.as_bytes())
            .bind(self.ticket.generation.generation_id)
            .bind(
                self.ticket
                    .generation
                    .model_contract_digest
                    .as_bytes()
                    .as_slice(),
            )
            .bind(self.ticket.generation.extractor_version.as_str())
            .bind(self.ticket.generation.model_contract.model.as_str())
            .bind(dimensions)
            .bind(Vector::from(
                query_vector.inner.embedding().as_slice().to_vec(),
            ))
            .bind(observed_limit)
            .fetch_all(&mut *self.tx)
            .await?;

        let truncated = rows.len() > usize::from(limit);
        let public_rows = rows.into_iter().take(usize::from(limit));
        let mut coordinates = Vec::with_capacity(usize::from(limit));
        for (index, row) in public_rows.enumerate() {
            let coordinate = coordinate_from_row(&row, *self.ticket.community_id.as_uuid())?;
            let score_value: i64 = row.try_get("semantic_score")?;
            let score_value = u32::try_from(score_value).map_err(|_| {
                DbError::InvalidData("Coordinate-search score is outside uint32".to_owned())
            })?;
            let score = Score::new(score_value).map_err(|error| {
                DbError::InvalidData(format!("invalid Coordinate-search score: {error}"))
            })?;
            let rank = u8::try_from(index + 1).map_err(|_| {
                DbError::InvalidData("Coordinate-search rank exceeds uint8".to_owned())
            })?;
            coordinates.push(ProjectContextCoordinateSearchCandidate {
                rank,
                coordinate,
                score,
            });
        }

        Ok(SemanticCoordinateSearchBatch {
            snapshot: self.coordinate_search_snapshot(),
            coordinates,
            truncated,
        })
    }
}

fn coordinate_from_source(source: &SemanticSourceIdentity) -> Result<ProjectContextCoordinate> {
    let coordinate = match source.kind {
        SemanticSourceKind::ProjectView(object_type) => {
            ProjectContextCoordinate::ProjectViewObject {
                object_type: project_view_object_type_from_semantic(object_type),
                object_id: source.source_id,
            }
        }
        SemanticSourceKind::ProjectDocument => ProjectContextCoordinate::Document {
            document_id: source.source_id,
        },
        SemanticSourceKind::Meeting => ProjectContextCoordinate::Meeting {
            meeting_id: source.source_id,
        },
    };
    coordinate
        .validate_for_project(source.community_id)
        .map_err(|error| {
            DbError::InvalidData(format!(
                "invalid Coordinate-search source identity: {error}"
            ))
        })?;
    Ok(coordinate)
}

const fn project_view_object_type_from_semantic(
    value: ProjectViewSemanticType,
) -> ProjectViewObjectType {
    match value {
        ProjectViewSemanticType::ProjectProfile => ProjectViewObjectType::ProjectProfile,
        ProjectViewSemanticType::Goal => ProjectViewObjectType::Goal,
        ProjectViewSemanticType::Role => ProjectViewObjectType::Role,
        ProjectViewSemanticType::Plan => ProjectViewObjectType::Plan,
        ProjectViewSemanticType::Stage => ProjectViewObjectType::Stage,
        ProjectViewSemanticType::Requirement => ProjectViewObjectType::Requirement,
        ProjectViewSemanticType::Issue => ProjectViewObjectType::Issue,
        ProjectViewSemanticType::Work => ProjectViewObjectType::Work,
        ProjectViewSemanticType::Resource => ProjectViewObjectType::Resource,
    }
}

fn coordinate_from_row(
    row: &sqlx::postgres::PgRow,
    project_id: uuid::Uuid,
) -> Result<ProjectContextCoordinate> {
    let coordinate_type: String = row.try_get("coordinate_type")?;
    let coordinate_subtype: Option<String> = row.try_get("coordinate_subtype")?;
    let coordinate_id: uuid::Uuid = row.try_get("coordinate_id")?;
    let coordinate = match (coordinate_type.as_str(), coordinate_subtype.as_deref()) {
        ("project_view_object", Some(subtype)) => ProjectContextCoordinate::ProjectViewObject {
            object_type: project_view_object_type(subtype)?,
            object_id: coordinate_id,
        },
        ("document", None) => ProjectContextCoordinate::Document {
            document_id: coordinate_id,
        },
        ("meeting", None) => ProjectContextCoordinate::Meeting {
            meeting_id: coordinate_id,
        },
        _ => {
            return Err(DbError::InvalidData(
                "Coordinate-search row has an unsupported Coordinate shape".to_owned(),
            ));
        }
    };
    coordinate
        .validate_for_project(project_id)
        .map_err(|error| {
            DbError::InvalidData(format!("invalid Coordinate-search row identity: {error}"))
        })?;
    Ok(coordinate)
}

fn project_view_object_type(value: &str) -> Result<ProjectViewObjectType> {
    match value {
        "project_profile" => Ok(ProjectViewObjectType::ProjectProfile),
        "goal" => Ok(ProjectViewObjectType::Goal),
        "role" => Ok(ProjectViewObjectType::Role),
        "plan" => Ok(ProjectViewObjectType::Plan),
        "stage" => Ok(ProjectViewObjectType::Stage),
        "requirement" => Ok(ProjectViewObjectType::Requirement),
        "issue" => Ok(ProjectViewObjectType::Issue),
        "work" => Ok(ProjectViewObjectType::Work),
        "resource" => Ok(ProjectViewObjectType::Resource),
        _ => Err(DbError::InvalidData(
            "Coordinate-search row has an unknown Project View subtype".to_owned(),
        )),
    }
}

const COORDINATE_SEARCH_SQL: &str = r#"
WITH requested_reader(pubkey) AS (
    VALUES ($2::bytea)
),
authorized_reader AS MATERIALIZED (
    SELECT requested_reader.pubkey
    FROM requested_reader
    LEFT JOIN users actor
      ON actor.community_id = $1 AND actor.pubkey = requested_reader.pubkey
    WHERE (
        (actor.agent_owner_pubkey IS NULL AND EXISTS (
            SELECT 1 FROM relay_members member
            WHERE member.community_id = $1
              AND member.pubkey = encode(requested_reader.pubkey, 'hex')
        ))
        OR (
            actor.agent_owner_pubkey IS NOT NULL
            AND EXISTS (
                SELECT 1 FROM relay_members owner_member
                WHERE owner_member.community_id = $1
                  AND owner_member.pubkey = encode(actor.agent_owner_pubkey, 'hex')
            )
            AND NOT EXISTS (
                SELECT 1 FROM community_bans owner_ban
                WHERE owner_ban.community_id = $1
                  AND owner_ban.pubkey = actor.agent_owner_pubkey
                  AND owner_ban.banned
                  AND (owner_ban.ban_expires_at IS NULL
                       OR owner_ban.ban_expires_at > clock_timestamp())
            )
            AND NOT EXISTS (
                SELECT 1 FROM users owner_actor
                WHERE owner_actor.community_id = $1
                  AND owner_actor.pubkey = actor.agent_owner_pubkey
                  AND owner_actor.agent_owner_pubkey IS NOT NULL
            )
        )
    )
    AND NOT EXISTS (
        SELECT 1 FROM community_bans actor_ban
        WHERE actor_ban.community_id = $1
          AND actor_ban.pubkey = requested_reader.pubkey
          AND actor_ban.banned
          AND (actor_ban.ban_expires_at IS NULL
               OR actor_ban.ban_expires_at > clock_timestamp())
    )
),
authorized_project AS MATERIALIZED (
    SELECT community.id AS community_id,
           community.semantic_active_generation_id AS generation_id
    FROM communities community
    CROSS JOIN authorized_reader
    JOIN project_view_maintenance maintenance
      ON maintenance.community_id = community.id
    JOIN project_view_state view_state ON view_state.community_id = community.id
    JOIN project_document_state document_state ON document_state.community_id = community.id
    JOIN project_context_edge_state context_state ON context_state.community_id = community.id
    WHERE community.id = $1
      AND community.archived_at IS NULL
      AND community.project_view_schema_version = 3
      AND community.project_view_enabled
      AND community.project_document_enabled
      AND community.meeting_community_read_enabled
      AND community.project_context_edge_enabled
      AND community.semantic_index_enabled
      AND community.semantic_graph_query_enabled
      AND maintenance.state = 'normal'
      AND view_state.schema_version = 3
      AND document_state.schema_version = 1
      AND context_state.schema_version = 2
      AND view_state.projection_pubkey = $3
      AND document_state.projection_pubkey = $3
      AND context_state.projection_pubkey = $3
      AND community.semantic_active_generation_id = $4
),
active_generation AS MATERIALIZED (
    SELECT generation.*
    FROM authorized_project project
    JOIN semantic_index_generations generation
      ON generation.community_id = project.community_id
     AND generation.generation_id = project.generation_id
    WHERE generation.lifecycle = 'active'
      AND generation.model_contract_digest = $5
      AND generation.extractor_version = $6
      AND generation.model = $7
      AND generation.dimensions = $8
      AND generation.distance_metric = 'cosine'
),
graph_coordinates AS MATERIALIZED (
    SELECT DISTINCT coordinate.community_id,
           coordinate.coordinate_type,
           coordinate.coordinate_subtype,
           coordinate.coordinate_id,
           CASE coordinate.coordinate_type
             WHEN 'project_view_object' THEN 0
             WHEN 'document' THEN 1
             WHEN 'meeting' THEN 2
           END AS coordinate_family_rank,
           CASE coordinate.coordinate_subtype
             WHEN 'project_profile' THEN 0
             WHEN 'goal' THEN 1
             WHEN 'role' THEN 2
             WHEN 'plan' THEN 3
             WHEN 'stage' THEN 4
             WHEN 'requirement' THEN 5
             WHEN 'issue' THEN 6
             WHEN 'work' THEN 7
             WHEN 'resource' THEN 8
             ELSE 0
           END AS coordinate_subtype_rank,
           CASE coordinate.coordinate_type
             WHEN 'project_view_object' THEN 'project_view'
             WHEN 'document' THEN 'project_document'
             WHEN 'meeting' THEN 'meeting'
           END AS source_family,
           CASE coordinate.coordinate_type
             WHEN 'project_view_object' THEN coordinate.coordinate_subtype
             WHEN 'document' THEN 'document'
             WHEN 'meeting' THEN 'meeting'
           END AS source_subtype
    FROM project_context_edge_coordinates coordinate
    JOIN project_context_edges edge
      ON edge.community_id = coordinate.community_id
     AND edge.edge_key = coordinate.edge_key
     AND edge.state = 'active'
    WHERE coordinate.community_id = $1
),
eligible AS MATERIALIZED (
    SELECT coordinate.coordinate_type, coordinate.coordinate_subtype,
           coordinate.coordinate_id, coordinate.coordinate_family_rank,
           coordinate.coordinate_subtype_rank,
           embedding.embedding
    FROM active_generation generation
    JOIN graph_coordinates coordinate
      ON coordinate.community_id = generation.community_id
    JOIN semantic_source_generation_heads head
      ON head.community_id = generation.community_id
     AND head.generation_id = generation.generation_id
     AND head.source_family = coordinate.source_family
     AND head.source_subtype = coordinate.source_subtype
     AND head.source_id = coordinate.coordinate_id
    JOIN semantic_sources source
      ON source.community_id = head.community_id
     AND source.source_family = head.source_family
     AND source.source_subtype = head.source_subtype
     AND source.source_id = head.source_id
     AND source.invalidation_epoch = head.source_invalidation_epoch
     AND source.snapshot_digest = head.source_snapshot_digest
     AND source.eligibility = 'eligible'
    JOIN semantic_unit_sets unit_set
      ON unit_set.community_id = head.community_id
     AND unit_set.unit_set_id = head.unit_set_id
     AND unit_set.source_family = head.source_family
     AND unit_set.source_subtype = head.source_subtype
     AND unit_set.source_id = head.source_id
     AND unit_set.source_invalidation_epoch = head.source_invalidation_epoch
     AND unit_set.source_snapshot_digest = head.source_snapshot_digest
     AND unit_set.state = 'active'
     AND unit_set.extractor_version = generation.extractor_version
    JOIN semantic_units unit
      ON unit.community_id = unit_set.community_id
     AND unit.unit_set_id = unit_set.unit_set_id
     AND unit.unit_kind = 'overview'
     AND unit.unit_key = 'overview'
    JOIN semantic_embeddings embedding
      ON embedding.community_id = unit.community_id
     AND embedding.unit_set_id = unit.unit_set_id
     AND embedding.unit_key = unit.unit_key
     AND embedding.generation_id = generation.generation_id
     AND embedding.model_contract_digest = generation.model_contract_digest
     AND embedding.dimensions = generation.dimensions
     AND embedding.response_model = generation.model
     AND vector_dims(embedding.embedding) = generation.dimensions
     AND vector_norm(embedding.embedding) > 0
),
distances AS MATERIALIZED (
    SELECT eligible.*,
           eligible.embedding <=> $9::vector AS distance
    FROM eligible
),
ranked AS (
    SELECT distances.*,
           floor(((greatest(-1.0, least(1.0, 1.0 - distance)) + 1.0)
                  / 2.0) * 1000000.0 + 0.5)::bigint AS semantic_score
    FROM distances
    WHERE distance > '-Infinity'::double precision
      AND distance < 'Infinity'::double precision
)
SELECT coordinate_type, coordinate_subtype, coordinate_id,
       semantic_score
FROM ranked
ORDER BY semantic_score DESC, coordinate_family_rank ASC,
         coordinate_subtype_rank ASC, coordinate_id ASC
LIMIT $10
"#;

#[cfg(test)]
#[path = "semantic_coordinate_search_qualification_tests.rs"]
mod qualification_tests;

#[cfg(test)]
mod tests {
    use buzz_core::{CommunityId, Keys};
    use buzz_project_context::ProjectContextCoordinate;
    use buzz_project_view::ProjectViewObjectType;
    use buzz_semantic::{
        Digest32, MeetingSourceBasis, ProjectDocumentSourceBasis, ProjectViewSourceBasis,
        SemanticDistanceMetric, SemanticModelContract, SemanticNormalization,
        SemanticProviderBoundary, SemanticSourceBasis,
    };
    use buzz_semantic_query::{
        build_coordinate_search_encoder_input, EncodedCoordinateSearchQuery,
        ProjectContextCoordinateSearchQuery, QueryCompatibilityFences,
    };
    use chrono::Utc;
    use pgvector::Vector;
    use sqlx::{Postgres, Transaction};
    use uuid::Uuid;

    use super::{SemanticCoordinateSearchVector, COORDINATE_SEARCH_SQL};
    use crate::semantic::SemanticGenerationRecord;
    use crate::semantic_query::{SemanticGraphQueryTicket, SemanticGraphReadTx};
    use crate::{Db, DbConfig};

    fn uuid(seed: u64) -> Uuid {
        Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("UUIDv4 fixture")
    }

    fn project_view_coordinate(
        object_type: ProjectViewObjectType,
        object_id: Uuid,
    ) -> ProjectContextCoordinate {
        ProjectContextCoordinate::ProjectViewObject {
            object_type,
            object_id,
        }
    }

    struct CurrentHeadSeed<'a> {
        community_id: Uuid,
        generation_id: Uuid,
        contract: &'a SemanticModelContract,
        contract_digest: Digest32,
        extractor_version: &'a str,
        family: &'a str,
        subtype: &'a str,
        source_id: Uuid,
        lifecycle: &'a str,
        vector: Vec<f32>,
        seed: u8,
    }

    async fn seed_current_head(tx: &mut Transaction<'_, Postgres>, head: CurrentHeadSeed<'_>) {
        let CurrentHeadSeed {
            community_id,
            generation_id,
            contract,
            contract_digest,
            extractor_version,
            family,
            subtype,
            source_id,
            lifecycle,
            vector,
            seed,
        } = head;
        let unit_set_id = Uuid::new_v4();
        let snapshot_digest = vec![seed; 32];
        let text_digest = vec![seed.wrapping_add(1); 32];
        let source_change_id = Digest32::from_bytes([seed.wrapping_add(2); 32]);
        let source_basis = match family {
            "project_view" => SemanticSourceBasis::ProjectView(ProjectViewSourceBasis {
                schema_version: 3,
                object_revision: 1,
                source_change_id,
            }),
            "project_document" => {
                SemanticSourceBasis::ProjectDocument(ProjectDocumentSourceBasis {
                    document_revision: 1,
                    source_change_id,
                })
            }
            "meeting" => SemanticSourceBasis::Meeting(MeetingSourceBasis {
                create_event_id: source_change_id,
                end_event_id: None,
            }),
            _ => panic!("unsupported semantic source family fixture"),
        };
        let source_basis = serde_json::to_value(source_basis).expect("serialize source basis");
        sqlx::query(
            "INSERT INTO semantic_sources(community_id,source_family,source_subtype,source_id,\
             eligibility,lifecycle_class,source_basis,snapshot_digest,invalidation_epoch,\
             coverage_state,observed_at) \
             VALUES($1,$2,$3,$4,'eligible',$5,$6,$7,1,'current',clock_timestamp()) \
             ON CONFLICT (community_id,source_family,source_subtype,source_id) DO UPDATE SET \
             eligibility='eligible',ineligibility_reason=NULL,lifecycle_class=EXCLUDED.lifecycle_class,\
             source_basis=EXCLUDED.source_basis,snapshot_digest=EXCLUDED.snapshot_digest,\
             invalidation_epoch=1,coverage_state='current',observed_at=EXCLUDED.observed_at",
        )
        .bind(community_id)
        .bind(family)
        .bind(subtype)
        .bind(source_id)
        .bind(lifecycle)
        .bind(source_basis)
        .bind(&snapshot_digest)
        .execute(&mut **tx)
        .await
        .expect("insert semantic source");
        sqlx::query(
            "INSERT INTO semantic_unit_sets(community_id,unit_set_id,source_family,source_subtype,\
             source_id,source_invalidation_epoch,source_basis,source_snapshot_digest,\
             extractor_version,state,complete_unit_count,activated_at) \
             VALUES($1,$2,$3,$4,$5,1,'{}'::jsonb,$6,$7,'active',1,clock_timestamp())",
        )
        .bind(community_id)
        .bind(unit_set_id)
        .bind(family)
        .bind(subtype)
        .bind(source_id)
        .bind(&snapshot_digest)
        .bind(extractor_version)
        .execute(&mut **tx)
        .await
        .expect("insert semantic unit set");
        sqlx::query(
            "INSERT INTO semantic_units(community_id,unit_set_id,unit_key,ordinal,unit_kind,\
             semantic_text,semantic_text_digest,summary_coverage,extraction_provenance) \
             VALUES($1,$2,'overview',0,'overview','content-free fixture',$3,\
                    'title_only','{}'::jsonb)",
        )
        .bind(community_id)
        .bind(unit_set_id)
        .bind(&text_digest)
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
        .bind(i32::try_from(contract.dimensions).expect("fixture dimensions"))
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
             VALUES($1,$2,$3,$4,$5,$6,1,$7,1,1)",
        )
        .bind(community_id)
        .bind(generation_id)
        .bind(family)
        .bind(subtype)
        .bind(source_id)
        .bind(unit_set_id)
        .bind(&snapshot_digest)
        .execute(&mut **tx)
        .await
        .expect("insert semantic head");
    }

    async fn seed_coordinate(
        tx: &mut Transaction<'_, Postgres>,
        community_id: Uuid,
        edge_key: &[u8],
        ordinal: i32,
        coordinate_type: &str,
        coordinate_subtype: Option<&str>,
        coordinate_id: Uuid,
    ) {
        let canonical_key = match (coordinate_type, coordinate_subtype) {
            ("project_view_object", Some(subtype)) => {
                format!("pv:{community_id}:{subtype}:{coordinate_id}")
            }
            ("document", None) => format!("document:{community_id}:{coordinate_id}"),
            ("meeting", None) => format!("meeting:{community_id}:{coordinate_id}"),
            _ => panic!("unsupported Coordinate fixture"),
        };
        sqlx::query(
            "INSERT INTO project_context_edge_coordinates(community_id,edge_key,ordinal,\
             coordinate_type,coordinate_subtype,coordinate_id,canonical_key) \
             VALUES($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(community_id)
        .bind(edge_key)
        .bind(ordinal)
        .bind(coordinate_type)
        .bind(coordinate_subtype)
        .bind(coordinate_id)
        .bind(canonical_key)
        .execute(&mut **tx)
        .await
        .expect("insert Edge Coordinate");
    }

    #[test]
    fn exact_query_is_coordinate_only_current_and_authorized() {
        assert!(COORDINATE_SEARCH_SQL.contains("SELECT DISTINCT coordinate.community_id"));
        assert!(COORDINATE_SEARCH_SQL.contains("edge.state = 'active'"));
        assert!(COORDINATE_SEARCH_SQL.contains("source.eligibility = 'eligible'"));
        assert!(COORDINATE_SEARCH_SQL.contains("unit.unit_kind = 'overview'"));
        assert!(COORDINATE_SEARCH_SQL.contains("semantic_graph_query_enabled"));
        assert!(COORDINATE_SEARCH_SQL.contains("eligible.embedding <=> $9::vector"));
        assert!(!COORDINATE_SEARCH_SQL.contains("project_context_document_bindings"));
        assert!(!COORDINATE_SEARCH_SQL.contains("semantic_text"));
    }

    #[test]
    fn exact_query_has_stable_k_plus_one_ordering() {
        assert!(COORDINATE_SEARCH_SQL.contains(
            "ORDER BY semantic_score DESC, coordinate_family_rank ASC,\n         coordinate_subtype_rank ASC, coordinate_id ASC\nLIMIT $10"
        ));
        for rank in [
            "WHEN 'project_profile' THEN 0",
            "WHEN 'goal' THEN 1",
            "WHEN 'role' THEN 2",
            "WHEN 'resource' THEN 8",
        ] {
            assert!(COORDINATE_SEARCH_SQL.contains(rank));
        }
    }

    #[tokio::test]
    async fn coordinate_search_real_pgvector_is_coordinate_only_deduplicated_and_stable() {
        let Ok(database_url) = std::env::var("BUZZ_TEST_SEMANTIC_DATABASE_URL") else {
            return;
        };
        assert_eq!(
            std::env::var("BUZZ_TEST_SEMANTIC_DISPOSABLE").as_deref(),
            Ok("fleet-policy-v1"),
            "refusing Coordinate-search test without the disposable marker"
        );
        assert!(database_url.contains("@127.0.0.1:"));
        assert!(database_url.contains("/buzz_semantic_disposable"));

        let db = Db::new(&DbConfig {
            database_url,
            ..DbConfig::default()
        })
        .await
        .expect("Coordinate-search test database");
        db.migrate().await.expect("Coordinate-search migrations");
        let mut tx = db.writer().begin().await.expect("fixture transaction");

        let community_id = Uuid::new_v4();
        let generation_id = Uuid::new_v4();
        let reader = Keys::generate();
        let relay = Keys::generate();
        let bytes = vec![7_u8; 32];
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
        let extractor_version = "coordinate-search-real-pg-v1";

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
                    clock_timestamp(),clock_timestamp(),'coordinate-search-test',TRUE,TRUE)",
        )
        .bind(community_id)
        .bind(format!("coordinate-search-{}.invalid", community_id))
        .bind(&bytes)
        .execute(&mut *tx)
        .await
        .expect("insert Community");
        sqlx::query("INSERT INTO relay_members(community_id,pubkey,role) VALUES($1,$2,'member')")
            .bind(community_id)
            .bind(reader.public_key().to_hex())
            .execute(&mut *tx)
            .await
            .expect("insert reader membership");
        sqlx::query(
            "INSERT INTO project_view_maintenance(community_id,state,updated_at) \
             VALUES($1,'normal',clock_timestamp()) \
             ON CONFLICT (community_id) DO UPDATE \
             SET state='normal',current_epoch=NULL,updated_at=EXCLUDED.updated_at",
        )
        .bind(community_id)
        .execute(&mut *tx)
        .await
        .expect("insert Project View maintenance");
        sqlx::query(
            "INSERT INTO project_view_state(community_id,project_revision,active_object_count,\
             initialized_at,updated_at,last_event_id,last_actor_pubkey,\
             meta_projection_event_id,projection_pubkey,projection_generation,schema_version,\
             last_change_id,last_source_event_id) \
             VALUES($1,1,5,clock_timestamp(),clock_timestamp(),$2,$2,$2,$3,1,3,$2,$2)",
        )
        .bind(community_id)
        .bind(&bytes)
        .bind(relay.public_key().as_bytes())
        .execute(&mut *tx)
        .await
        .expect("insert Project View state");
        sqlx::query(
            "INSERT INTO project_document_state(community_id,schema_version,catalog_revision,\
             active_document_count,last_change_id,last_actor_pubkey,projection_pubkey,\
             projection_generation,meta_projection_event_id,initialized_at,updated_at) \
             VALUES($1,1,3,3,$2,$2,$3,1,$2,clock_timestamp(),clock_timestamp())",
        )
        .bind(community_id)
        .bind(&bytes)
        .bind(relay.public_key().as_bytes())
        .execute(&mut *tx)
        .await
        .expect("insert Project Document state");
        sqlx::query(
            "INSERT INTO project_context_edge_state(community_id,schema_version,context_revision,\
             active_edge_count,bound_document_count,last_change_id,last_actor_pubkey,\
             projection_pubkey,projection_generation,meta_projection_event_id,\
             initialized_at,updated_at) \
             VALUES($1,2,9,2,2,$2,$2,$3,1,$2,clock_timestamp(),clock_timestamp())",
        )
        .bind(community_id)
        .bind(&bytes)
        .bind(relay.public_key().as_bytes())
        .execute(&mut *tx)
        .await
        .expect("insert Project Context state");
        sqlx::query(
            "INSERT INTO semantic_index_generations(community_id,generation_id,lifecycle,\
             extractor_version,input_contract_version,provider,model,dimensions,distance_metric,\
             normalization,provider_boundary,model_contract_digest,created_by,rebuild_completed_at,\
             ready_at,activated_at) \
             VALUES($1,$2,'active',$3,$4,$5,$6,$7,'cosine','none','deterministic_fake',$8,\
                    'coordinate-search-test',clock_timestamp(),clock_timestamp(),clock_timestamp())",
        )
        .bind(community_id)
        .bind(generation_id)
        .bind(extractor_version)
        .bind(contract.input_contract_version.as_str())
        .bind(contract.provider.as_str())
        .bind(contract.model.as_str())
        .bind(i32::try_from(contract.dimensions).expect("dimensions"))
        .bind(contract_digest.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .expect("insert semantic generation");
        sqlx::query("UPDATE communities SET semantic_active_generation_id=$2 WHERE id=$1")
            .bind(community_id)
            .bind(generation_id)
            .execute(&mut *tx)
            .await
            .expect("activate semantic generation");

        let relation_document = uuid(100);
        let second_relation_document = uuid(101);
        let coordinate_document = uuid(102);
        for document_id in [
            relation_document,
            second_relation_document,
            coordinate_document,
        ] {
            sqlx::query(
                "INSERT INTO project_documents(community_id,document_id,current_revision,state,\
                 created_at,created_by,updated_at,updated_by,current_source_change_id,\
                 current_head_event_id,current_revision_event_id) \
                 VALUES($1,$2,1,'active',clock_timestamp(),$3,clock_timestamp(),$3,$3,$3,$3)",
            )
            .bind(community_id)
            .bind(document_id)
            .bind(&bytes)
            .execute(&mut *tx)
            .await
            .expect("insert Project Document");
        }

        let edge_one = vec![11_u8; 32];
        let edge_two = vec![12_u8; 32];
        let deleted_edge = vec![13_u8; 32];
        for (edge_key, state, shape) in [
            (
                &edge_one,
                "active",
                serde_json::json!(["edge-one", "members"]),
            ),
            (
                &edge_two,
                "active",
                serde_json::json!(["edge-two", "members"]),
            ),
            (
                &deleted_edge,
                "deleted",
                serde_json::json!(["deleted-edge", "members"]),
            ),
        ] {
            sqlx::query(
                "INSERT INTO project_context_edges(community_id,edge_key,state,\
                 canonical_coordinates,last_context_revision,current_source_change_id,\
                 updated_at,updated_by) \
                 VALUES($1,$2,$3,$4,9,$5,clock_timestamp(),$5)",
            )
            .bind(community_id)
            .bind(edge_key)
            .bind(state)
            .bind(shape)
            .bind(&bytes)
            .execute(&mut *tx)
            .await
            .expect("insert Project Context Edge");
        }
        for (document_id, edge_key) in [
            (relation_document, &edge_one),
            (second_relation_document, &edge_two),
        ] {
            sqlx::query(
                "INSERT INTO project_context_document_bindings(community_id,context_document_id,\
                 edge_key,state,binding_context_revision,current_source_change_id,\
                 current_projection_event_id,updated_at,updated_by) \
                 VALUES($1,$2,$3,'active',9,$4,$4,clock_timestamp(),$4)",
            )
            .bind(community_id)
            .bind(document_id)
            .bind(edge_key)
            .bind(&bytes)
            .execute(&mut *tx)
            .await
            .expect("insert Context Document binding");
        }

        let role = uuid(1);
        let work_low = uuid(2);
        let work_high = uuid(3);
        let terminal_issue = uuid(4);
        let missing_meeting = uuid(5);
        let inactive_resource = uuid(6);
        seed_coordinate(
            &mut tx,
            community_id,
            &edge_one,
            0,
            "project_view_object",
            Some("role"),
            role,
        )
        .await;
        seed_coordinate(
            &mut tx,
            community_id,
            &edge_one,
            1,
            "project_view_object",
            Some("work"),
            work_high,
        )
        .await;
        seed_coordinate(
            &mut tx,
            community_id,
            &edge_one,
            2,
            "project_view_object",
            Some("issue"),
            terminal_issue,
        )
        .await;
        seed_coordinate(
            &mut tx,
            community_id,
            &edge_one,
            3,
            "document",
            None,
            coordinate_document,
        )
        .await;
        seed_coordinate(
            &mut tx,
            community_id,
            &edge_one,
            4,
            "meeting",
            None,
            missing_meeting,
        )
        .await;
        seed_coordinate(
            &mut tx,
            community_id,
            &edge_two,
            0,
            "project_view_object",
            Some("role"),
            role,
        )
        .await;
        seed_coordinate(
            &mut tx,
            community_id,
            &edge_two,
            1,
            "project_view_object",
            Some("work"),
            work_low,
        )
        .await;
        seed_coordinate(
            &mut tx,
            community_id,
            &deleted_edge,
            0,
            "project_view_object",
            Some("resource"),
            inactive_resource,
        )
        .await;
        seed_coordinate(
            &mut tx,
            community_id,
            &deleted_edge,
            1,
            "project_view_object",
            Some("work"),
            work_high,
        )
        .await;

        for (family, subtype, id, lifecycle, vector, seed) in [
            (
                "project_view",
                "role",
                role,
                "active",
                vec![1.0, 0.0, 0.0],
                1,
            ),
            (
                "project_view",
                "work",
                work_low,
                "active",
                vec![1.0, 0.0008, 0.0],
                2,
            ),
            (
                "project_view",
                "work",
                work_high,
                "active",
                vec![1.0, 0.0001, 0.0],
                3,
            ),
            (
                "project_view",
                "issue",
                terminal_issue,
                "terminal",
                vec![0.0, 1.0, 0.0],
                4,
            ),
            (
                "project_document",
                "document",
                coordinate_document,
                "active",
                vec![-1.0, 0.0, 0.0],
                5,
            ),
            (
                "project_document",
                "document",
                relation_document,
                "active",
                vec![1.0, 0.0, 0.0],
                6,
            ),
            (
                "project_view",
                "resource",
                inactive_resource,
                "active",
                vec![1.0, 0.0, 0.0],
                7,
            ),
        ] {
            seed_current_head(
                &mut tx,
                CurrentHeadSeed {
                    community_id,
                    generation_id,
                    contract: &contract,
                    contract_digest,
                    extractor_version,
                    family,
                    subtype,
                    source_id: id,
                    lifecycle,
                    vector,
                    seed,
                },
            )
            .await;
        }

        let observed_at = Utc::now();
        let ticket = SemanticGraphQueryTicket {
            community_id: CommunityId::from_uuid(community_id),
            generation: SemanticGenerationRecord {
                community_id: CommunityId::from_uuid(community_id),
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
        let request = ProjectContextCoordinateSearchQuery {
            request_id: Uuid::new_v4(),
            project_id: community_id,
            query: "find the relevant starting coordinate".to_owned(),
            limit: 5,
        };
        let input = build_coordinate_search_encoder_input(&request).expect("encoder input");
        let encoded = EncodedCoordinateSearchQuery::new(
            &input,
            contract.model.clone(),
            vec![1.0, 0.0, 0.0],
            &contract,
        )
        .expect("encoded query");
        let query_vector =
            SemanticCoordinateSearchVector::new(&ticket, encoded).expect("query vector");
        let mut read = SemanticGraphReadTx {
            tx,
            ticket,
            reader_pubkey: reader.public_key().to_bytes().to_vec(),
            expected_projection_pubkey: relay.public_key(),
        };

        let legacy_limited = read
            .search_coordinate_starts_legacy(&query_vector, 4)
            .await
            .expect("legacy K+1 Coordinate search");
        let compiled_limited = read
            .search_coordinate_starts(&query_vector, 4)
            .await
            .expect("compiled-route K+1 Coordinate search");
        assert_eq!(compiled_limited, legacy_limited);
        let limited = read
            .search_coordinate_starts_migrated(&query_vector, 4)
            .await
            .expect("K+1 Coordinate search");
        assert_eq!(limited, legacy_limited);
        assert!(limited.truncated);
        assert_eq!(limited.coordinates.len(), 4);
        let legacy_complete = read
            .search_coordinate_starts_legacy(&query_vector, 5)
            .await
            .expect("legacy complete Coordinate search");
        let complete = read
            .search_coordinate_starts_migrated(&query_vector, 5)
            .await
            .expect("complete Coordinate search");
        assert_eq!(complete, legacy_complete);
        assert!(!complete.truncated);
        assert_eq!(
            complete
                .coordinates
                .iter()
                .map(|candidate| &candidate.coordinate)
                .collect::<Vec<_>>(),
            vec![
                &project_view_coordinate(ProjectViewObjectType::Role, role),
                &project_view_coordinate(ProjectViewObjectType::Work, work_low),
                &project_view_coordinate(ProjectViewObjectType::Work, work_high),
                &project_view_coordinate(ProjectViewObjectType::Issue, terminal_issue),
                &ProjectContextCoordinate::Document {
                    document_id: coordinate_document,
                },
            ]
        );
        assert_eq!(complete.coordinates[0].score.raw(), 1_000_000);
        assert_eq!(complete.coordinates[3].score.raw(), 500_000);
        assert_eq!(complete.coordinates[4].score.raw(), 0);
        assert_eq!(
            complete
                .coordinates
                .iter()
                .filter(|candidate| candidate.coordinate
                    == project_view_coordinate(ProjectViewObjectType::Role, role))
                .count(),
            1,
            "a Coordinate shared by active Edges must be returned once"
        );
        assert!(complete.coordinates.iter().all(|candidate| {
            candidate.coordinate
                != ProjectContextCoordinate::Document {
                    document_id: relation_document,
                }
                && candidate.coordinate
                    != project_view_coordinate(ProjectViewObjectType::Resource, inactive_resource)
                && candidate.coordinate
                    != ProjectContextCoordinate::Meeting {
                        meeting_id: missing_meeting,
                    }
        }));
        read.rollback().await.expect("roll back fixture");
    }
}
