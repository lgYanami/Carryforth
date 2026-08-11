//! Durable, short-lived HTTP fleet attestations for semantic graph queries.

use std::time::Duration;

use buzz_core::CommunityId;
use buzz_semantic::Digest32;
use buzz_semantic_query::{
    semantic_graph_http_runtime_digest, SemanticGraphHttpFleetInventory,
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

    /// Atomically enable the Community query gate while holding and validating
    /// the current HTTP fleet assertion. Revocation/replacement cannot race
    /// between the fleet check and the database prerequisite update.
    pub async fn enable_semantic_graph_query_with_http_fleet(
        &self,
        community_id: CommunityId,
        deployment_id: &str,
    ) -> Result<()> {
        if !self.semantic_graph_query_schema_ready().await? {
            return Err(DbError::InvalidData(
                "semantic graph query schema is not ready".to_owned(),
            ));
        }
        let mut tx = self.pool.begin().await?;
        // Keep the global lock order aligned with Provider egress
        // confirmation: Community before generation/source/fleet state.
        let community: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM communities WHERE id=$1 FOR UPDATE")
                .bind(community_id.as_uuid())
                .fetch_optional(&mut *tx)
                .await?;
        if community.is_none() {
            tx.rollback().await?;
            return Err(DbError::NotFound("semantic Community".to_owned()));
        }
        let row = sqlx::query(
            "SELECT *, clock_timestamp() < expires_at AS unexpired \
             FROM semantic_graph_http_fleet_attestations \
             WHERE community_id=$1 AND transport='http' FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
        let row = row.ok_or_else(|| {
            DbError::InvalidData("semantic graph HTTP fleet attestation is missing".to_owned())
        })?;
        let (attestation, unexpired) = parse_attestation_row(&row)?;
        let readiness =
            validate_attestation_expectation(attestation, unexpired, deployment_id, None);
        if let Some(failure) = readiness.failure {
            tx.rollback().await?;
            return Err(DbError::InvalidData(format!(
                "semantic graph HTTP fleet attestation is not ready: {}",
                failure.code()
            )));
        }
        let affected = sqlx::query(
            "UPDATE communities community \
             SET semantic_graph_query_enabled=TRUE \
             WHERE community.id=$1 \
               AND community.semantic_index_enabled \
               AND community.project_context_edge_enabled \
               AND EXISTS (SELECT 1 FROM semantic_index_generations generation \
                           WHERE generation.community_id=community.id \
                             AND generation.generation_id=\
                                 community.semantic_active_generation_id \
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
                           AND embedding.model_contract_digest=\
                               generation.model_contract_digest \
                           AND embedding.response_model=generation.model \
                           AND vector_norm(embedding.embedding)>0))",
        )
        .bind(community_id.as_uuid())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if affected != 1 {
            tx.rollback().await?;
            return Err(DbError::InvalidData(
                "semantic graph query database prerequisites are not ready".to_owned(),
            ));
        }
        tx.commit().await?;
        Ok(())
    }
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
    use buzz_semantic_query::{
        semantic_graph_http_runtime_digest, SemanticGraphHttpFleetInstance,
        SemanticGraphHttpFleetInventory,
    };

    use super::{
        validate_attestation_expectation, SemanticGraphHttpFleetAttestation,
        SemanticGraphHttpFleetFailure, LOCK_FINAL_HTTP_FLEET_READINESS_SQL,
    };

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
}
