//! Durable, short-lived HTTP fleet attestations for semantic graph queries.

use std::time::Duration;

use buzz_core::CommunityId;
use buzz_semantic::Digest32;
use buzz_semantic_query::{
    semantic_graph_http_runtime_digest, SemanticGraphHttpFleetInventory,
    SemanticGraphQueryEnableRequirement, SemanticGraphQueryRoutingTrust,
    SEMANTIC_GRAPH_HTTP_TRANSPORT,
};
use chrono::{DateTime, Utc};
use sqlx::{postgres::PgRow, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{Db, DbError, Result};

/// Minimum useful lifetime for an operator fleet assertion.
pub const MIN_SEMANTIC_GRAPH_FLEET_ATTESTATION_TTL: Duration = Duration::from_secs(30);
/// Maximum short lifetime enforced by both application code and migration 0058.
pub const MAX_SEMANTIC_GRAPH_FLEET_ATTESTATION_TTL: Duration = Duration::from_secs(15 * 60);

const LOCK_FINAL_HTTP_FLEET_READINESS_SQL: &str =
    "SELECT *, clock_timestamp() < expires_at AS unexpired \
     FROM semantic_graph_http_fleet_attestations \
     WHERE community_id=$1 AND transport='http' FOR SHARE";

/// Values supplied by the operator when replacing the current HTTP assertion.
pub struct WriteSemanticGraphHttpFleetAttestation<'a> {
    /// Tenant whose query capability is fenced by this assertion.
    pub community_id: CommunityId,
    /// Unique audit identity for this assertion.
    pub attestation_id: Uuid,
    /// Complete, control-plane-enumerated current routing inventory.
    pub inventory: &'a SemanticGraphHttpFleetInventory,
    /// Bounded time after which every Relay fails closed.
    pub ttl: Duration,
    /// Content-free operator identity recorded for audit.
    pub attested_by: &'a str,
}

/// Validated durable HTTP fleet assertion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticGraphHttpFleetAttestation {
    /// Tenant boundary.
    pub community_id: CommunityId,
    /// Unique assertion identity.
    pub attestation_id: Uuid,
    /// Deployment identity copied from the inventory.
    pub deployment_id: String,
    /// Common compiled runtime digest.
    pub runtime_digest: Digest32,
    /// Canonical inventory digest.
    pub inventory_digest: Digest32,
    /// Complete validated inventory.
    pub inventory: SemanticGraphHttpFleetInventory,
    /// Time at which the operator explicitly acknowledged the routing list.
    pub routing_inventory_acknowledged_at: DateTime<Utc>,
    /// Database-clock assertion time.
    pub attested_at: DateTime<Utc>,
    /// Database-clock expiry.
    pub expires_at: DateTime<Utc>,
    /// Content-free operator identity.
    pub attested_by: String,
    /// Revocation time, when explicitly revoked.
    pub revoked_at: Option<DateTime<Utc>>,
    /// Content-free revoking operator identity.
    pub revoked_by: Option<String>,
}

/// Closed reason a fleet assertion cannot authorize HTTP query runtime use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticGraphHttpFleetFailure {
    /// No assertion exists for this Community and transport.
    Missing,
    /// The assertion was explicitly revoked.
    Revoked,
    /// The assertion reached its short database-clock expiry.
    Expired,
    /// A stored value or closed JSON shape failed validation.
    Malformed,
    /// The operator assertion names another deployment.
    DeploymentMismatch,
    /// The assertion does not match this compiled HTTP runtime.
    RuntimeMismatch,
    /// This Pod is not part of the asserted current routing inventory.
    InstanceMissing,
}

impl SemanticGraphHttpFleetFailure {
    /// Stable content-free diagnostic spelling.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Revoked => "revoked",
            Self::Expired => "expired",
            Self::Malformed => "malformed",
            Self::DeploymentMismatch => "deployment_mismatch",
            Self::RuntimeMismatch => "runtime_mismatch",
            Self::InstanceMissing => "instance_missing",
        }
    }
}

/// Result of checking the durable assertion against one binary/deployment Pod.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticGraphHttpFleetReadiness {
    /// Valid assertion metadata when the stored row could be decoded.
    pub attestation: Option<SemanticGraphHttpFleetAttestation>,
    /// Absent only when every fleet fence passes.
    pub failure: Option<SemanticGraphHttpFleetFailure>,
}

impl SemanticGraphHttpFleetReadiness {
    /// Whether the current fleet assertion authorizes this expectation.
    pub const fn ready(&self) -> bool {
        self.failure.is_none()
    }
}

impl Db {
    /// Check that every currently query-enabled Community has a valid fleet
    /// assertion containing this exact serving instance.
    pub async fn all_enabled_semantic_graph_http_fleets_ready(
        &self,
        deployment_id: &str,
        instance_id: &str,
    ) -> Result<bool> {
        let communities: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM communities \
             WHERE semantic_graph_query_enabled AND archived_at IS NULL ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        for community_id in communities {
            if !self
                .semantic_graph_http_fleet_readiness(
                    CommunityId::from_uuid(community_id),
                    deployment_id,
                    Some(instance_id),
                )
                .await?
                .ready()
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Replace the current short-lived HTTP fleet assertion.
    ///
    /// The caller must explicitly acknowledge that `inventory` is the exact
    /// current load-balancer routing inventory. The database cannot discover
    /// or independently prove that external control-plane fact.
    pub async fn write_semantic_graph_http_fleet_attestation(
        &self,
        input: WriteSemanticGraphHttpFleetAttestation<'_>,
    ) -> Result<SemanticGraphHttpFleetAttestation> {
        input
            .inventory
            .validate_for_compiled_runtime()
            .map_err(|error| DbError::InvalidData(error.to_string()))?;
        if input.attestation_id.is_nil() {
            return Err(DbError::InvalidData(
                "semantic graph fleet attestation id must not be nil".to_owned(),
            ));
        }
        if input.ttl < MIN_SEMANTIC_GRAPH_FLEET_ATTESTATION_TTL
            || input.ttl > MAX_SEMANTIC_GRAPH_FLEET_ATTESTATION_TTL
        {
            return Err(DbError::InvalidData(
                "semantic graph fleet attestation TTL must be in 30..=900 seconds".to_owned(),
            ));
        }
        validate_actor(input.attested_by)?;
        let ttl_seconds = i32::try_from(input.ttl.as_secs()).map_err(|_| {
            DbError::InvalidData("semantic graph fleet attestation TTL is invalid".to_owned())
        })?;
        let runtime_digest = input
            .inventory
            .common_runtime_digest()
            .map_err(|error| DbError::InvalidData(error.to_string()))?;
        let inventory_digest = input
            .inventory
            .digest()
            .map_err(|error| DbError::InvalidData(error.to_string()))?;
        let inventory = serde_json::to_value(input.inventory)?;
        let row = sqlx::query(
            "WITH observed AS (SELECT clock_timestamp() AS observed_at) \
             INSERT INTO semantic_graph_http_fleet_attestations (\
                 community_id, transport, attestation_id, deployment_id, \
                 runtime_digest, inventory_digest, inventory, \
                 routing_inventory_acknowledged_at, attested_at, expires_at, attested_by) \
             SELECT $1, 'http', $2, $3, $4, $5, $6, observed_at, observed_at, \
                    observed_at + make_interval(secs => $7), $8 \
             FROM observed \
             ON CONFLICT (community_id, transport) DO UPDATE SET \
                 attestation_id=EXCLUDED.attestation_id, \
                 deployment_id=EXCLUDED.deployment_id, \
                 runtime_digest=EXCLUDED.runtime_digest, \
                 inventory_digest=EXCLUDED.inventory_digest, \
                 inventory=EXCLUDED.inventory, \
                 routing_inventory_acknowledged_at=\
                     EXCLUDED.routing_inventory_acknowledged_at, \
                 attested_at=EXCLUDED.attested_at, expires_at=EXCLUDED.expires_at, \
                 attested_by=EXCLUDED.attested_by, revoked_at=NULL, revoked_by=NULL \
             RETURNING *, clock_timestamp() < expires_at AS unexpired",
        )
        .bind(input.community_id.as_uuid())
        .bind(input.attestation_id)
        .bind(&input.inventory.deployment_id)
        .bind(runtime_digest.as_bytes().as_slice())
        .bind(inventory_digest.as_bytes().as_slice())
        .bind(inventory)
        .bind(ttl_seconds)
        .bind(input.attested_by)
        .fetch_one(&self.pool)
        .await?;
        parse_attestation_row(&row).map(|(attestation, _)| attestation)
    }

    /// Revoke the current HTTP fleet assertion. Revocation is retained for
    /// audit and fails closed immediately; it does not enable or disable the
    /// independent Community query gate.
    pub async fn revoke_semantic_graph_http_fleet_attestation(
        &self,
        community_id: CommunityId,
        revoked_by: &str,
    ) -> Result<bool> {
        validate_actor(revoked_by)?;
        let affected = sqlx::query(
            "UPDATE semantic_graph_http_fleet_attestations \
             SET revoked_at=clock_timestamp(), revoked_by=$2 \
             WHERE community_id=$1 AND transport='http' AND revoked_at IS NULL",
        )
        .bind(community_id.as_uuid())
        .bind(revoked_by)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(affected == 1)
    }

    /// Check one Community assertion against the expected deployment and,
    /// for serving Relay calls, an exact local instance identity.
    pub async fn semantic_graph_http_fleet_readiness(
        &self,
        community_id: CommunityId,
        deployment_id: &str,
        instance_id: Option<&str>,
    ) -> Result<SemanticGraphHttpFleetReadiness> {
        let row = sqlx::query(
            "SELECT *, clock_timestamp() < expires_at AS unexpired \
             FROM semantic_graph_http_fleet_attestations \
             WHERE community_id=$1 AND transport='http'",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(fleet_failure(None, SemanticGraphHttpFleetFailure::Missing));
        };
        let (attestation, unexpired) = match parse_attestation_row(&row) {
            Ok(value) => value,
            Err(DbError::InvalidData(_)) | Err(DbError::Serde(_)) => {
                return Ok(fleet_failure(
                    None,
                    SemanticGraphHttpFleetFailure::Malformed,
                ));
            }
            Err(error) => return Err(error),
        };
        Ok(validate_attestation_expectation(
            attestation,
            unexpired,
            deployment_id,
            instance_id,
        ))
    }

    /// Atomically enable the Community query gate under an explicit topology
    /// requirement while retaining all shared database prerequisites.
    ///
    /// In attested-Fleet mode, revocation or replacement cannot race between
    /// the Fleet check and the prerequisite update. Trusted single-Relay mode
    /// skips only that Fleet row lock and validation.
    pub async fn enable_semantic_graph_query(
        &self,
        community_id: CommunityId,
        requirement: SemanticGraphQueryEnableRequirement<'_>,
    ) -> Result<()> {
        if !self.semantic_graph_query_schema_ready().await? {
            return Err(DbError::InvalidData(
                "semantic graph query schema is not ready".to_owned(),
            ));
        }
        let mut tx = self.pool.begin().await?;
        match enable_semantic_graph_query_in_tx(&mut tx, community_id, requirement).await {
            Ok(()) => {
                tx.commit().await?;
                Ok(())
            }
            Err(error) => {
                tx.rollback().await?;
                Err(error)
            }
        }
    }

    /// Atomically enable the Community query gate with the strict Fleet
    /// requirement used before topology policy support was introduced.
    pub async fn enable_semantic_graph_query_with_http_fleet(
        &self,
        community_id: CommunityId,
        deployment_id: &str,
    ) -> Result<()> {
        self.enable_semantic_graph_query(
            community_id,
            SemanticGraphQueryEnableRequirement::AttestedFleet { deployment_id },
        )
        .await
    }
}

async fn enable_semantic_graph_query_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    requirement: SemanticGraphQueryEnableRequirement<'_>,
) -> Result<()> {
    // Keep the global lock order aligned with Provider egress confirmation:
    // Community before generation/source/Fleet state.
    let community: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM communities WHERE id=$1 FOR UPDATE")
            .bind(community_id.as_uuid())
            .fetch_optional(&mut **tx)
            .await?;
    if community.is_none() {
        return Err(DbError::NotFound("semantic Community".to_owned()));
    }
    if let SemanticGraphQueryEnableRequirement::AttestedFleet { deployment_id } = requirement {
        let row = sqlx::query(
            "SELECT *, clock_timestamp() < expires_at AS unexpired \
             FROM semantic_graph_http_fleet_attestations \
             WHERE community_id=$1 AND transport='http' FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&mut **tx)
        .await?;
        let row = row.ok_or_else(|| {
            DbError::InvalidData("semantic graph HTTP fleet attestation is missing".to_owned())
        })?;
        let (attestation, unexpired) = parse_attestation_row(&row)?;
        let readiness =
            validate_attestation_expectation(attestation, unexpired, deployment_id, None);
        if let Some(failure) = readiness.failure {
            return Err(DbError::InvalidData(format!(
                "semantic graph HTTP fleet attestation is not ready: {}",
                failure.code()
            )));
        }
    }
    let affected = sqlx::query(
        "UPDATE communities community \
         SET semantic_graph_query_enabled=TRUE \
         WHERE community.id=$1 \
           AND community.semantic_index_enabled \
           AND community.project_context_edge_enabled \
           AND EXISTS (SELECT 1 FROM semantic_index_generations generation \
                       WHERE generation.community_id=community.id \
                         AND generation.generation_id=community.semantic_active_generation_id \
                         AND generation.lifecycle='active') \
           AND NOT EXISTS ( \
               SELECT 1 FROM semantic_source_generation_heads head \
               JOIN semantic_index_generations generation \
                 ON generation.community_id=head.community_id \
                AND generation.generation_id=head.generation_id \
               WHERE head.community_id=community.id \
                 AND head.generation_id=community.semantic_active_generation_id \
                 AND NOT EXISTS ( \
                     SELECT 1 FROM semantic_unit_sets unit_set \
                     JOIN semantic_units unit \
                       ON unit.community_id=unit_set.community_id \
                      AND unit.unit_set_id=unit_set.unit_set_id \
                      AND unit.unit_kind='overview' \
                     JOIN semantic_embeddings embedding \
                       ON embedding.community_id=unit.community_id \
                      AND embedding.unit_set_id=unit.unit_set_id \
                      AND embedding.unit_key=unit.unit_key \
                      AND embedding.generation_id=generation.generation_id \
                     WHERE unit_set.community_id=head.community_id \
                       AND unit_set.unit_set_id=head.unit_set_id \
                       AND unit_set.state='active' \
                       AND unit_set.extractor_version=generation.extractor_version \
                       AND embedding.dimensions=generation.dimensions \
                       AND embedding.model_contract_digest=generation.model_contract_digest \
                       AND embedding.response_model=generation.model \
                       AND vector_norm(embedding.embedding)>0))",
    )
    .bind(community_id.as_uuid())
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if affected != 1 {
        return Err(DbError::InvalidData(
            "semantic graph query database prerequisites are not ready".to_owned(),
        ));
    }
    Ok(())
}

/// Validate and lock the current HTTP fleet assertion inside the caller's
/// final Provider-egress transaction.
///
/// A malformed, missing, expired, revoked, or mismatched assertion fails
/// closed. The shared row lock prevents a replacement or revocation from
/// crossing the caller's egress linearization point.
pub(crate) async fn semantic_graph_http_fleet_ready_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    deployment_id: &str,
    instance_id: &str,
) -> Result<bool> {
    let row = sqlx::query(LOCK_FINAL_HTTP_FLEET_READINESS_SQL)
        .bind(community_id.as_uuid())
        .fetch_optional(&mut **tx)
        .await?;
    let Some(row) = row else {
        return Ok(false);
    };
    let (attestation, unexpired) = match parse_attestation_row(&row) {
        Ok(value) => value,
        Err(DbError::InvalidData(_)) | Err(DbError::Serde(_)) => return Ok(false),
        Err(error) => return Err(error),
    };
    Ok(
        validate_attestation_expectation(attestation, unexpired, deployment_id, Some(instance_id))
            .ready(),
    )
}

/// Apply an explicit routing policy inside the caller's final authorization
/// transaction. Trusted single-Relay mode skips only the Fleet row access;
/// strict mode preserves the existing shared-lock validation.
pub(crate) async fn semantic_graph_query_routing_ready_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    routing_trust: SemanticGraphQueryRoutingTrust<'_>,
) -> Result<bool> {
    match routing_trust {
        SemanticGraphQueryRoutingTrust::TrustedSingleRelay => Ok(true),
        SemanticGraphQueryRoutingTrust::AttestedFleet {
            deployment_id,
            instance_id,
        } => {
            semantic_graph_http_fleet_ready_in_tx(tx, community_id, deployment_id, instance_id)
                .await
        }
    }
}

fn parse_attestation_row(row: &PgRow) -> Result<(SemanticGraphHttpFleetAttestation, bool)> {
    let runtime_digest = digest_from_db(row.try_get("runtime_digest")?, "runtime_digest")?;
    let inventory_digest = digest_from_db(row.try_get("inventory_digest")?, "inventory_digest")?;
    let inventory: SemanticGraphHttpFleetInventory =
        serde_json::from_value(row.try_get("inventory")?)?;
    inventory
        .validate()
        .map_err(|error| DbError::InvalidData(error.to_string()))?;
    if inventory
        .common_runtime_digest()
        .map_err(|error| DbError::InvalidData(error.to_string()))?
        != runtime_digest
        || inventory
            .digest()
            .map_err(|error| DbError::InvalidData(error.to_string()))?
            != inventory_digest
    {
        return Err(DbError::InvalidData(
            "semantic graph fleet attestation digest mismatch".to_owned(),
        ));
    }
    let community_id = CommunityId::from_uuid(row.try_get("community_id")?);
    let transport: String = row.try_get("transport")?;
    if transport != SEMANTIC_GRAPH_HTTP_TRANSPORT {
        return Err(DbError::InvalidData(
            "semantic graph fleet attestation transport is invalid".to_owned(),
        ));
    }
    let deployment_id: String = row.try_get("deployment_id")?;
    if inventory.deployment_id != deployment_id {
        return Err(DbError::InvalidData(
            "semantic graph fleet attestation deployment mismatch".to_owned(),
        ));
    }
    let attestation = SemanticGraphHttpFleetAttestation {
        community_id,
        attestation_id: row.try_get("attestation_id")?,
        deployment_id,
        runtime_digest,
        inventory_digest,
        inventory,
        routing_inventory_acknowledged_at: row.try_get("routing_inventory_acknowledged_at")?,
        attested_at: row.try_get("attested_at")?,
        expires_at: row.try_get("expires_at")?,
        attested_by: row.try_get("attested_by")?,
        revoked_at: row.try_get("revoked_at")?,
        revoked_by: row.try_get("revoked_by")?,
    };
    if attestation.attestation_id.is_nil()
        || validate_actor(&attestation.attested_by).is_err()
        || attestation.routing_inventory_acknowledged_at < attestation.attested_at
        || attestation.routing_inventory_acknowledged_at > attestation.expires_at
        || attestation.expires_at <= attestation.attested_at
        || attestation.expires_at - attestation.attested_at > chrono::Duration::minutes(15)
        || match (
            attestation.revoked_at.as_ref(),
            attestation.revoked_by.as_deref(),
        ) {
            (None, None) => false,
            (Some(revoked_at), Some(revoked_by)) => {
                revoked_at < &attestation.attested_at || validate_actor(revoked_by).is_err()
            }
            _ => true,
        }
    {
        return Err(DbError::InvalidData(
            "semantic graph fleet attestation audit shape is invalid".to_owned(),
        ));
    }
    Ok((attestation, row.try_get("unexpired")?))
}

fn validate_attestation_expectation(
    attestation: SemanticGraphHttpFleetAttestation,
    unexpired: bool,
    deployment_id: &str,
    instance_id: Option<&str>,
) -> SemanticGraphHttpFleetReadiness {
    if attestation.revoked_at.is_some() {
        return fleet_failure(Some(attestation), SemanticGraphHttpFleetFailure::Revoked);
    }
    if !unexpired {
        return fleet_failure(Some(attestation), SemanticGraphHttpFleetFailure::Expired);
    }
    if attestation.deployment_id != deployment_id {
        return fleet_failure(
            Some(attestation),
            SemanticGraphHttpFleetFailure::DeploymentMismatch,
        );
    }
    let Ok(compiled_runtime) = semantic_graph_http_runtime_digest() else {
        return fleet_failure(
            Some(attestation),
            SemanticGraphHttpFleetFailure::RuntimeMismatch,
        );
    };
    if attestation.runtime_digest != compiled_runtime {
        return fleet_failure(
            Some(attestation),
            SemanticGraphHttpFleetFailure::RuntimeMismatch,
        );
    }
    if instance_id.is_some_and(|identity| !attestation.inventory.contains_instance(identity)) {
        return fleet_failure(
            Some(attestation),
            SemanticGraphHttpFleetFailure::InstanceMissing,
        );
    }
    SemanticGraphHttpFleetReadiness {
        attestation: Some(attestation),
        failure: None,
    }
}

fn fleet_failure(
    attestation: Option<SemanticGraphHttpFleetAttestation>,
    failure: SemanticGraphHttpFleetFailure,
) -> SemanticGraphHttpFleetReadiness {
    SemanticGraphHttpFleetReadiness {
        attestation,
        failure: Some(failure),
    }
}

fn digest_from_db(value: Vec<u8>, field: &'static str) -> Result<Digest32> {
    let bytes: [u8; 32] = value
        .try_into()
        .map_err(|_| DbError::InvalidData(format!("semantic graph fleet {field} is invalid")))?;
    Ok(Digest32::from_bytes(bytes))
}

fn validate_actor(actor: &str) -> Result<()> {
    if actor.trim().is_empty() || actor.len() > 255 || actor.as_bytes().contains(&0) {
        return Err(DbError::InvalidData(
            "semantic graph fleet operator identity is invalid".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use buzz_core::kind::KIND_NIP43_MEMBERSHIP_LIST;
    use buzz_core::{CommunityId, PublicKey};
    use buzz_project_context::{ProjectContextCatalog, ProjectContextProjectionPlan};
    use buzz_project_document::{DocumentCatalog, DocumentProjectionPlan};
    use buzz_sdk::project_context::build_project_context_meta_projection;
    use buzz_sdk::project_document::build_document_meta_projection;
    use buzz_sdk::project_view_v3::{
        build_meta_projection, V3EntityCounts, V3ProjectionContext, V3ProjectionSource,
    };
    use buzz_semantic::{DeterministicFakeEncoder, SemanticEncoder, OVERVIEW_EXTRACTOR_VERSION};
    use buzz_semantic_query::{
        semantic_graph_http_runtime_digest, SemanticGraphHttpFleetInstance,
        SemanticGraphHttpFleetInventory, SemanticGraphQueryEnableRequirement,
        SemanticGraphQueryRoutingTrust,
    };
    use chrono::{DateTime, Utc};
    use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};
    use uuid::Uuid;

    use super::{
        semantic_graph_query_routing_ready_in_tx, validate_attestation_expectation,
        SemanticGraphHttpFleetAttestation, SemanticGraphHttpFleetFailure,
        WriteSemanticGraphHttpFleetAttestation, LOCK_FINAL_HTTP_FLEET_READINESS_SQL,
    };
    use crate::project_context::PreparedProjectContextBootstrap;
    use crate::project_document::PreparedProjectDocumentBootstrap;
    use crate::semantic::CreateSemanticGeneration;
    use crate::semantic_query::{
        SemanticGraphQueryEgressConfirmation, SemanticGraphQueryEgressConfirmationRequest,
        SemanticGraphQueryReleaseConfirmation, SemanticGraphQueryReleaseRequest,
        SemanticGraphQueryTicket,
    };
    use crate::{Db, DbConfig};

    const TEST_DEPLOYMENT_ID: &str = "semantic-policy-test";
    const TEST_INSTANCE_ID: &str = "relay-0";

    async fn semantic_test_db() -> Db {
        assert_eq!(
            std::env::var("BUZZ_TEST_SEMANTIC_DISPOSABLE").as_deref(),
            Ok("fleet-policy-v1"),
            "refusing semantic Fleet tests without the disposable-test marker",
        );
        let database_url = std::env::var("BUZZ_TEST_SEMANTIC_DATABASE_URL")
            .expect("semantic Fleet tests require an explicit disposable database URL");
        let database_name = database_url
            .rsplit('/')
            .next()
            .and_then(|tail| tail.split(['?', '#']).next())
            .expect("semantic Fleet test URL must include a database name");
        assert_eq!(
            database_name, "buzz_semantic_disposable",
            "refusing to target a non-disposable semantic Fleet database",
        );
        assert!(
            database_url.contains("@127.0.0.1:"),
            "semantic Fleet tests only accept a loopback disposable database",
        );
        let db = Db::new(&DbConfig {
            database_url,
            ..DbConfig::default()
        })
        .await
        .expect("semantic Fleet policy test database");
        let (actual_database, disposable_marker): (String, Option<String>) = sqlx::query_as(
            "SELECT current_database(), current_setting('buzz.disposable_test', TRUE)",
        )
        .fetch_one(db.writer())
        .await
        .expect("read semantic Fleet disposable database identity");
        assert_eq!(actual_database, "buzz_semantic_disposable");
        assert_eq!(disposable_marker.as_deref(), Some("fleet-policy-v1"));
        db.migrate()
            .await
            .expect("semantic Fleet policy migrations");
        db
    }

    fn fleet_inventory() -> SemanticGraphHttpFleetInventory {
        SemanticGraphHttpFleetInventory {
            transport: "http".to_owned(),
            deployment_id: TEST_DEPLOYMENT_ID.to_owned(),
            instances: vec![SemanticGraphHttpFleetInstance {
                instance_id: TEST_INSTANCE_ID.to_owned(),
                runtime_digest: semantic_graph_http_runtime_digest().expect("runtime digest"),
                http_ready: true,
            }],
        }
    }

    struct AuthorizedQueryFixture {
        community_id: CommunityId,
        reader_pubkey: [u8; 32],
        projection_pubkey: PublicKey,
        ticket: SemanticGraphQueryTicket,
    }

    fn whole_second_now() -> DateTime<Utc> {
        DateTime::from_timestamp(Utc::now().timestamp(), 0).expect("current whole second")
    }

    async fn seed_authorized_query_fixture(db: &Db) -> AuthorizedQueryFixture {
        let community_id = CommunityId::from_uuid(Uuid::new_v4());
        let reader = Keys::generate();
        let relay = Keys::generate();
        let canonical_time = whole_second_now();
        let membership = EventBuilder::new(Kind::Custom(KIND_NIP43_MEMBERSHIP_LIST as u16), "")
            .tags([
                Tag::parse(["-"]).expect("membership protection tag"),
                Tag::parse(["member", reader.public_key().to_hex().as_str(), "owner"])
                    .expect("membership owner tag"),
            ])
            .custom_created_at(Timestamp::from(
                u64::try_from(canonical_time.timestamp()).expect("positive fixture time"),
            ))
            .sign_with_keys(&relay)
            .expect("sign membership projection");
        let view_context = V3ProjectionContext {
            project_id: community_id,
            projection_generation: 1,
            project_revision: 1,
            source: V3ProjectionSource::System {
                change_id: membership.id,
                audit_seq: 1,
            },
            updated_at: canonical_time,
        };
        let view_meta = build_meta_projection(
            &view_context,
            V3EntityCounts {
                active_objects: 0,
                open_proposals: 0,
                active_assignments: 0,
                active_commitments: 0,
                checkpoints: 0,
                handoffs: 0,
            },
            membership.id,
            true,
            &[],
        )
        .expect("build empty Project View metadata")
        .sign_with_keys(&relay)
        .expect("sign empty Project View metadata");

        sqlx::query("INSERT INTO communities(id,host) VALUES ($1,$2)")
            .bind(community_id.as_uuid())
            .bind(format!(
                "semantic-authorized-{}.invalid",
                community_id.as_uuid()
            ))
            .execute(db.writer())
            .await
            .expect("authorized semantic Community");

        // The production v3 initializer necessarily creates a Profile and
        // governance objects. This test needs an otherwise empty, signed read
        // model so Stage B/D can isolate the routing-policy branch. Skip only
        // fixture-write triggers inside the database guarded above; every
        // production read/parity/signature gate is restored and exercised
        // before a query ticket can be issued.
        let mut view_tx = db.writer().begin().await.expect("Project View fixture");
        sqlx::query("SET LOCAL session_replication_role = replica")
            .execute(&mut *view_tx)
            .await
            .expect("suspend Project View fixture-write triggers");
        sqlx::query("INSERT INTO relay_members(community_id,pubkey,role) VALUES ($1,$2,'owner')")
            .bind(community_id.as_uuid())
            .bind(reader.public_key().to_hex())
            .execute(&mut *view_tx)
            .await
            .expect("authorized semantic reader");
        for event in [&membership, &view_meta] {
            let (_, inserted) =
                crate::event::insert_event_in_tx(&mut view_tx, community_id, event, None)
                    .await
                    .expect("insert Project View fixture projection");
            assert!(inserted);
        }
        sqlx::query(
            "INSERT INTO project_view_state (\
                 community_id,project_revision,active_object_count,initialized_at,updated_at,\
                 last_event_id,last_actor_pubkey,meta_projection_event_id,projection_pubkey,\
                 projection_generation,schema_version,last_change_id,last_source_event_id,\
                 open_proposal_count,active_assignment_count,active_commitment_count,\
                 checkpoint_count,handoff_count,membership_snapshot_event_id) \
             VALUES ($1,1,0,$2,$2,$3,$4,$5,$6,1,3,$3,NULL,0,0,0,0,0,$7)",
        )
        .bind(community_id.as_uuid())
        .bind(canonical_time)
        .bind(membership.id.as_bytes().as_slice())
        .bind(reader.public_key().as_bytes())
        .bind(view_meta.id.as_bytes().as_slice())
        .bind(relay.public_key().as_bytes())
        .bind(membership.id.as_bytes().as_slice())
        .execute(&mut *view_tx)
        .await
        .expect("empty Project View state");
        sqlx::query("SET LOCAL session_replication_role = origin")
            .execute(&mut *view_tx)
            .await
            .expect("restore Project View fixture-write triggers");
        view_tx.commit().await.expect("commit Project View fixture");
        assert!(db
            .set_project_view_enabled_checked(community_id, true, Some(&relay.public_key()),)
            .await
            .expect("enable Project View fixture"));

        db.set_meeting_community_read_create_paused(community_id, true)
            .await
            .expect("pause empty Meeting corpus");
        let meeting_audit = db
            .audit_legacy_meeting_visibility(community_id)
            .await
            .expect("audit empty Meeting corpus");
        db.approve_legacy_meeting_visibility(
            community_id,
            meeting_audit.watermark,
            &meeting_audit.digest,
            "semantic-fleet-policy-test",
        )
        .await
        .expect("approve empty Meeting corpus");
        db.enable_meeting_community_read(community_id)
            .await
            .expect("publish Meeting read contract");

        let document_catalog = DocumentCatalog::empty(community_id, 1, whole_second_now())
            .expect("empty Document catalog");
        let document_plan =
            DocumentProjectionPlan::for_bootstrap(&document_catalog).expect("Document plan");
        let document_meta = build_document_meta_projection(&document_plan, &[])
            .expect("build empty Document metadata")
            .sign_with_keys(&relay)
            .expect("sign empty Document metadata");
        db.bootstrap_empty_project_document_catalog(PreparedProjectDocumentBootstrap {
            catalog: document_catalog,
            meta_projection: document_meta,
        })
        .await
        .expect("bootstrap empty Document catalog");
        assert!(db
            .set_project_document_enabled_checked(community_id, true, Some(&relay.public_key()),)
            .await
            .expect("enable Document fixture"));

        let context_catalog = ProjectContextCatalog::empty(community_id, 1, whole_second_now())
            .expect("empty Context catalog");
        let context_plan =
            ProjectContextProjectionPlan::for_reset(&context_catalog).expect("Context plan");
        let context_meta = build_project_context_meta_projection(&context_plan, &[])
            .expect("build empty Context metadata")
            .sign_with_keys(&relay)
            .expect("sign empty Context metadata");
        db.bootstrap_empty_project_context_catalog(PreparedProjectContextBootstrap {
            catalog: context_catalog,
            meta_projection: context_meta,
        })
        .await
        .expect("bootstrap empty Context catalog");
        assert!(
            db.set_project_context_edge_enabled_checked(
                community_id,
                true,
                Some(&relay.public_key()),
            )
            .await
            .expect("enable Context fixture")
        );

        let encoder = DeterministicFakeEncoder::new(8).expect("fake encoder");
        let generation_id = Uuid::new_v4();
        db.create_semantic_generation(CreateSemanticGeneration {
            community_id,
            generation_id,
            extractor_version: OVERVIEW_EXTRACTOR_VERSION,
            model_contract: encoder.contract(),
            created_by: "semantic-fleet-policy-test",
        })
        .await
        .expect("authorized semantic generation");
        sqlx::query(
            "UPDATE semantic_index_generations \
             SET lifecycle='active',ready_at=clock_timestamp(),\
                 activated_at=clock_timestamp() \
             WHERE community_id=$1 AND generation_id=$2",
        )
        .bind(community_id.as_uuid())
        .bind(generation_id)
        .execute(db.writer())
        .await
        .expect("activate authorized semantic generation");
        sqlx::query(
            "UPDATE communities \
             SET semantic_index_enabled=TRUE,semantic_active_generation_id=$2 \
             WHERE id=$1",
        )
        .bind(community_id.as_uuid())
        .bind(generation_id)
        .execute(db.writer())
        .await
        .expect("publish authorized semantic generation");
        db.enable_semantic_graph_query(
            community_id,
            SemanticGraphQueryEnableRequirement::TrustedSingleRelay,
        )
        .await
        .expect("enable authorized semantic query gate");

        let reader_pubkey = reader.public_key().to_bytes();
        let projection_pubkey = relay.public_key();
        let ticket = db
            .semantic_graph_query_ticket(community_id, &reader_pubkey, &projection_pubkey)
            .await
            .expect("authorized semantic query ticket");
        AuthorizedQueryFixture {
            community_id,
            reader_pubkey,
            projection_pubkey,
            ticket,
        }
    }

    async fn write_fleet_attestation(db: &Db, community_id: CommunityId) {
        let inventory = fleet_inventory();
        db.write_semantic_graph_http_fleet_attestation(WriteSemanticGraphHttpFleetAttestation {
            community_id,
            attestation_id: Uuid::new_v4(),
            inventory: &inventory,
            ttl: Duration::from_secs(60),
            attested_by: "semantic-fleet-policy-test",
        })
        .await
        .expect("write semantic Fleet policy attestation");
    }

    async fn routing_ready(
        db: &Db,
        community_id: CommunityId,
        routing_trust: SemanticGraphQueryRoutingTrust<'_>,
    ) -> bool {
        let mut tx = db.writer().begin().await.expect("routing transaction");
        let ready = semantic_graph_query_routing_ready_in_tx(&mut tx, community_id, routing_trust)
            .await
            .expect("routing readiness");
        tx.rollback().await.expect("rollback routing transaction");
        ready
    }

    async fn query_enabled(db: &Db, community_id: CommunityId) -> bool {
        sqlx::query_scalar("SELECT semantic_graph_query_enabled FROM communities WHERE id=$1")
            .bind(community_id.as_uuid())
            .fetch_one(db.writer())
            .await
            .expect("query gate state")
    }

    async fn expire_fleet_attestation(db: &Db, community_id: CommunityId) {
        sqlx::query(
            "WITH observed AS (SELECT clock_timestamp() AS observed_at) \
             UPDATE semantic_graph_http_fleet_attestations attestation \
             SET routing_inventory_acknowledged_at=observed.observed_at-INTERVAL '2 minutes',\
                 attested_at=observed.observed_at-INTERVAL '2 minutes',\
                 expires_at=observed.observed_at-INTERVAL '1 minute' \
             FROM observed \
             WHERE attestation.community_id=$1 AND attestation.transport='http'",
        )
        .bind(community_id.as_uuid())
        .execute(db.writer())
        .await
        .expect("expire semantic Fleet policy attestation");
    }

    async fn assert_stage_b_and_stage_d_routing_matrix(db: &Db, fixture: &AuthorizedQueryFixture) {
        assert!(matches!(
            db.confirm_semantic_graph_query_egress(SemanticGraphQueryEgressConfirmationRequest {
                expected_ticket: &fixture.ticket,
                reader_pubkey: &fixture.reader_pubkey,
                expected_projection_pubkey: &fixture.projection_pubkey,
                expected_contexts: &[],
                routing_trust: SemanticGraphQueryRoutingTrust::TrustedSingleRelay,
            })
            .await
            .expect("trusted Stage B confirmation"),
            SemanticGraphQueryEgressConfirmation::Permitted(_)
        ));
        assert!(matches!(
            db.confirm_semantic_graph_query_egress(SemanticGraphQueryEgressConfirmationRequest {
                expected_ticket: &fixture.ticket,
                reader_pubkey: &fixture.reader_pubkey,
                expected_projection_pubkey: &fixture.projection_pubkey,
                expected_contexts: &[],
                routing_trust: SemanticGraphQueryRoutingTrust::AttestedFleet {
                    deployment_id: TEST_DEPLOYMENT_ID,
                    instance_id: TEST_INSTANCE_ID,
                },
            })
            .await
            .expect("strict Stage B confirmation"),
            SemanticGraphQueryEgressConfirmation::FleetUnavailable
        ));
        assert!(matches!(
            db.confirm_semantic_graph_query_release(SemanticGraphQueryReleaseRequest {
                community_id: fixture.community_id,
                reader_pubkey: &fixture.reader_pubkey,
                expected_projection_pubkey: &fixture.projection_pubkey,
                routing_trust: SemanticGraphQueryRoutingTrust::TrustedSingleRelay,
            })
            .await
            .expect("trusted Stage D confirmation"),
            SemanticGraphQueryReleaseConfirmation::Permitted(_)
        ));
        assert!(matches!(
            db.confirm_semantic_graph_query_release(SemanticGraphQueryReleaseRequest {
                community_id: fixture.community_id,
                reader_pubkey: &fixture.reader_pubkey,
                expected_projection_pubkey: &fixture.projection_pubkey,
                routing_trust: SemanticGraphQueryRoutingTrust::AttestedFleet {
                    deployment_id: TEST_DEPLOYMENT_ID,
                    instance_id: TEST_INSTANCE_ID,
                },
            })
            .await
            .expect("strict Stage D confirmation"),
            SemanticGraphQueryReleaseConfirmation::FleetUnavailable
        ));
    }

    async fn assert_policy_matrix_for_unready_fleet(db: &Db, community_id: CommunityId) {
        assert!(
            routing_ready(
                db,
                community_id,
                SemanticGraphQueryRoutingTrust::TrustedSingleRelay,
            )
            .await,
            "trusted single-Relay routing must not consult the Fleet row",
        );
        assert!(
            !routing_ready(
                db,
                community_id,
                SemanticGraphQueryRoutingTrust::AttestedFleet {
                    deployment_id: TEST_DEPLOYMENT_ID,
                    instance_id: TEST_INSTANCE_ID,
                },
            )
            .await,
            "attested-Fleet routing must fail closed",
        );

        sqlx::query("UPDATE communities SET semantic_graph_query_enabled=FALSE WHERE id=$1")
            .bind(community_id.as_uuid())
            .execute(db.writer())
            .await
            .expect("disable query gate before enable-policy assertion");
        let strict_error = db
            .enable_semantic_graph_query(
                community_id,
                SemanticGraphQueryEnableRequirement::AttestedFleet {
                    deployment_id: TEST_DEPLOYMENT_ID,
                },
            )
            .await
            .expect_err("attested-Fleet enable must fail closed");
        assert!(
            strict_error
                .to_string()
                .contains("fleet attestation is not ready")
                || strict_error
                    .to_string()
                    .contains("fleet attestation is missing"),
            "unexpected strict Fleet error: {strict_error}",
        );
        assert!(!query_enabled(db, community_id).await);

        db.enable_semantic_graph_query(
            community_id,
            SemanticGraphQueryEnableRequirement::TrustedSingleRelay,
        )
        .await
        .expect("trusted single-Relay enable");
        assert!(query_enabled(db, community_id).await);
    }

    fn attestation() -> SemanticGraphHttpFleetAttestation {
        let now = chrono::Utc::now();
        let runtime_digest = semantic_graph_http_runtime_digest().expect("runtime digest");
        let inventory = SemanticGraphHttpFleetInventory {
            transport: "http".to_owned(),
            deployment_id: "deployment-a".to_owned(),
            instances: vec![SemanticGraphHttpFleetInstance {
                instance_id: "relay-0".to_owned(),
                runtime_digest,
                http_ready: true,
            }],
        };
        SemanticGraphHttpFleetAttestation {
            community_id: buzz_core::CommunityId::from_uuid(uuid::Uuid::new_v4()),
            attestation_id: uuid::Uuid::new_v4(),
            deployment_id: inventory.deployment_id.clone(),
            runtime_digest,
            inventory_digest: inventory.digest().expect("inventory digest"),
            inventory,
            routing_inventory_acknowledged_at: now,
            attested_at: now,
            expires_at: now + chrono::Duration::minutes(5),
            attested_by: "test".to_owned(),
            revoked_at: None,
            revoked_by: None,
        }
    }

    #[test]
    fn local_expectation_requires_deployment_runtime_instance_and_liveness() {
        let ready =
            validate_attestation_expectation(attestation(), true, "deployment-a", Some("relay-0"));
        assert!(ready.ready());

        let missing =
            validate_attestation_expectation(attestation(), true, "deployment-a", Some("relay-1"));
        assert_eq!(
            missing.failure,
            Some(SemanticGraphHttpFleetFailure::InstanceMissing)
        );

        let expired =
            validate_attestation_expectation(attestation(), false, "deployment-a", Some("relay-0"));
        assert_eq!(
            expired.failure,
            Some(SemanticGraphHttpFleetFailure::Expired)
        );
    }

    #[test]
    fn final_fleet_readiness_uses_database_clock_and_holds_a_shared_row_lock() {
        assert!(LOCK_FINAL_HTTP_FLEET_READINESS_SQL.contains("clock_timestamp() < expires_at"));
        assert!(LOCK_FINAL_HTTP_FLEET_READINESS_SQL.contains("transport='http'"));
        assert!(LOCK_FINAL_HTTP_FLEET_READINESS_SQL.contains("FOR SHARE"));
    }

    #[tokio::test]
    #[ignore = "requires scripts/test-semantic-migrations.sh disposable pgvector database"]
    async fn fleet_policy_database_matrix_is_closed_for_missing_expired_and_revoked_rows() {
        let db = semantic_test_db().await;
        let fixture = seed_authorized_query_fixture(&db).await;
        let community_id = fixture.community_id;

        assert_policy_matrix_for_unready_fleet(&db, community_id).await;

        write_fleet_attestation(&db, community_id).await;
        expire_fleet_attestation(&db, community_id).await;
        assert_eq!(
            db.semantic_graph_http_fleet_readiness(
                community_id,
                TEST_DEPLOYMENT_ID,
                Some(TEST_INSTANCE_ID),
            )
            .await
            .expect("expired Fleet readiness")
            .failure,
            Some(SemanticGraphHttpFleetFailure::Expired),
        );
        assert_policy_matrix_for_unready_fleet(&db, community_id).await;

        write_fleet_attestation(&db, community_id).await;
        assert!(db
            .revoke_semantic_graph_http_fleet_attestation(
                community_id,
                "semantic-fleet-policy-test",
            )
            .await
            .expect("revoke semantic Fleet policy attestation"));
        assert_eq!(
            db.semantic_graph_http_fleet_readiness(
                community_id,
                TEST_DEPLOYMENT_ID,
                Some(TEST_INSTANCE_ID),
            )
            .await
            .expect("revoked Fleet readiness")
            .failure,
            Some(SemanticGraphHttpFleetFailure::Revoked),
        );
        assert_policy_matrix_for_unready_fleet(&db, community_id).await;
    }

    #[tokio::test]
    #[ignore = "requires scripts/test-semantic-migrations.sh disposable pgvector database"]
    async fn stage_b_and_stage_d_apply_the_selected_routing_policy_at_final_authorization() {
        let db = semantic_test_db().await;

        let disabled = CommunityId::from_uuid(Uuid::new_v4());
        sqlx::query("INSERT INTO communities(id,host) VALUES ($1,$2)")
            .bind(disabled.as_uuid())
            .bind(format!(
                "semantic-fleet-policy-disabled-{}.invalid",
                disabled.as_uuid()
            ))
            .execute(db.writer())
            .await
            .expect("disabled semantic Fleet policy Community");
        db.enable_semantic_graph_query(
            disabled,
            SemanticGraphQueryEnableRequirement::TrustedSingleRelay,
        )
        .await
        .expect_err("trusted topology must retain database enable prerequisites");
        assert!(!query_enabled(&db, disabled).await);

        let fixture = seed_authorized_query_fixture(&db).await;

        // Missing Fleet row.
        assert_stage_b_and_stage_d_routing_matrix(&db, &fixture).await;

        // Expired Fleet row, evaluated with the database clock.
        write_fleet_attestation(&db, fixture.community_id).await;
        expire_fleet_attestation(&db, fixture.community_id).await;
        assert_stage_b_and_stage_d_routing_matrix(&db, &fixture).await;

        // Explicitly revoked Fleet row. Re-attestation replaces the expired
        // assertion first, so this observes the independent revocation path.
        write_fleet_attestation(&db, fixture.community_id).await;
        assert!(db
            .revoke_semantic_graph_http_fleet_attestation(
                fixture.community_id,
                "semantic-fleet-policy-test",
            )
            .await
            .expect("revoke authorized fixture Fleet attestation"));
        assert_stage_b_and_stage_d_routing_matrix(&db, &fixture).await;
    }
}
