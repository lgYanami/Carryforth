//! Closed deployment-inventory contract for semantic graph HTTP queries.

use std::{fmt, str::FromStr};

use buzz_semantic::Digest32;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{
    budget_profile_digest, query_contract_digest, ranking_contract_digest, QueryContractResult,
};

/// Transport spelling persisted in the HTTP fleet attestation.
pub const SEMANTIC_GRAPH_HTTP_TRANSPORT: &str = "http";
/// Maximum accepted bytes in an operator-provided routing inventory file.
pub const MAX_SEMANTIC_GRAPH_FLEET_INVENTORY_BYTES: usize = 64 * 1024;
/// Maximum number of concurrently routable instances in one attestation.
pub const MAX_SEMANTIC_GRAPH_FLEET_INSTANCES: usize = 256;

/// Topology policy used to authorize semantic graph HTTP query routing.
///
/// `TrustedSingleRelay` is only suitable when exactly one Relay serves the
/// deployment. `AttestedFleet` preserves the short-lived routing-inventory
/// assertion required by multi-Relay deployments.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SemanticGraphQueryFleetPolicy {
    /// Trust the one locally managed Relay and do not consult a Fleet row.
    #[default]
    TrustedSingleRelay,
    /// Require the existing durable, short-lived Fleet Attestation.
    AttestedFleet,
}

impl SemanticGraphQueryFleetPolicy {
    /// Return the exact configuration spelling for this policy.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TrustedSingleRelay => "trusted-single-relay",
            Self::AttestedFleet => "attested-fleet",
        }
    }
}

impl fmt::Display for SemanticGraphQueryFleetPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for SemanticGraphQueryFleetPolicy {
    type Err = ParseSemanticGraphQueryFleetPolicyError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "trusted-single-relay" => Ok(Self::TrustedSingleRelay),
            "attested-fleet" => Ok(Self::AttestedFleet),
            _ => Err(ParseSemanticGraphQueryFleetPolicyError),
        }
    }
}

/// Error returned when a Fleet policy is not one of the two closed values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("semantic graph query Fleet policy must be trusted-single-relay or attested-fleet")]
pub struct ParseSemanticGraphQueryFleetPolicyError;

/// Typed routing requirement for Stage B and Stage D query authorization.
///
/// This prevents an absent deployment identity from being interpreted as an
/// implicit Fleet bypass. Callers must choose the topology policy explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticGraphQueryRoutingTrust<'a> {
    /// Trust the single Relay and skip only the Fleet row lock and validation.
    TrustedSingleRelay,
    /// Require this exact deployment and serving instance in a valid Fleet row.
    AttestedFleet {
        /// Deployment identity asserted by the current routing inventory.
        deployment_id: &'a str,
        /// Exact serving instance that must remain in that inventory.
        instance_id: &'a str,
    },
}

impl SemanticGraphQueryRoutingTrust<'_> {
    /// Return the policy represented by this routing requirement.
    pub const fn policy(self) -> SemanticGraphQueryFleetPolicy {
        match self {
            Self::TrustedSingleRelay => SemanticGraphQueryFleetPolicy::TrustedSingleRelay,
            Self::AttestedFleet { .. } => SemanticGraphQueryFleetPolicy::AttestedFleet,
        }
    }
}

/// Typed topology requirement for atomically enabling a Community query gate.
///
/// Enabling in strict mode needs only the deployment-level Fleet assertion;
/// serving-instance membership is checked later at Provider egress and result
/// release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticGraphQueryEnableRequirement<'a> {
    /// Apply all database prerequisites without consulting a Fleet row.
    TrustedSingleRelay,
    /// Additionally require a valid assertion for this deployment.
    AttestedFleet {
        /// Deployment identity asserted by the current routing inventory.
        deployment_id: &'a str,
    },
}

impl SemanticGraphQueryEnableRequirement<'_> {
    /// Return the policy represented by this enable requirement.
    pub const fn policy(self) -> SemanticGraphQueryFleetPolicy {
        match self {
            Self::TrustedSingleRelay => SemanticGraphQueryFleetPolicy::TrustedSingleRelay,
            Self::AttestedFleet { .. } => SemanticGraphQueryFleetPolicy::AttestedFleet,
        }
    }
}

/// Compiled execution path for one closed semantic computation operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SemanticComputationRoute {
    /// Preserve the implementation selected by the compatibility baseline.
    Legacy,
    /// Use the shared semantic-computation implementation.
    Migrated,
}

impl SemanticComputationRoute {
    const fn profile_token(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Migrated => "migrated",
        }
    }
}

/// Closed route matrix compiled into every routable Relay instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SemanticComputationRouteMatrix {
    /// Edge-to-member-Coordinate one-hop operation.
    pub edge_member_coordinate: SemanticComputationRoute,
    /// Coordinate-to-incident-Edge one-hop operation.
    pub coordinate_incident_edge: SemanticComputationRoute,
    /// Whole-graph Coordinate discovery operation.
    pub whole_graph_coordinate_discovery: SemanticComputationRoute,
    /// Bounded complete-path operation.
    pub bounded_complete_path: SemanticComputationRoute,
}

impl SemanticComputationRouteMatrix {
    /// Return the canonical content-free descriptor bound into fleet trust.
    pub fn canonical_profile(self) -> String {
        format!(
            concat!(
                "edge-member-coordinate={}\n",
                "coordinate-incident-edge={}\n",
                "whole-graph-coordinate-discovery={}\n",
                "bounded-complete-path={}\n",
            ),
            self.edge_member_coordinate.profile_token(),
            self.coordinate_incident_edge.profile_token(),
            self.whole_graph_coordinate_discovery.profile_token(),
            self.bounded_complete_path.profile_token(),
        )
    }
}

/// Current compiled Phase 1 operation routes.
///
/// U6 moves all four closed operations to the shared computation path. Legacy
/// adapters remain compiled only for the documented profile rollback window;
/// no request can select them or fall back to them dynamically.
pub const SEMANTIC_COMPUTATION_ROUTES: SemanticComputationRouteMatrix =
    SemanticComputationRouteMatrix {
        edge_member_coordinate: SemanticComputationRoute::Migrated,
        coordinate_incident_edge: SemanticComputationRoute::Migrated,
        whole_graph_coordinate_discovery: SemanticComputationRoute::Migrated,
        bounded_complete_path: SemanticComputationRoute::Migrated,
    };

/// Explicitly bumped closed descriptor for the compiled HTTP query runtime.
///
/// This descriptor intentionally binds the strict request/result/Event and
/// transport behavior that cannot be derived from Rust type names. Any
/// incompatible change to those contracts must change the dated profile and this
/// descriptor before a mixed fleet can attest itself ready.
pub const SEMANTIC_GRAPH_HTTP_RUNTIME_CONTRACT: &str = concat!(
    "runtime-contract=semantic-query-http-runtime-20260816-coordinate-filter-v2\n",
    "transport=http-post-query-exclusive-single-filter\n",
    "request=unversioned-closed-request-id-project-id-problem-initial-context-lifecycle-budget\n",
    "result=unversioned-closed-project-request-completion-observations-input-observations-roots-paths-target-lifecycle-typed-basis-path-source-provenance-explicit-provenance-coverage\n",
    "event=kind-40912-relay-only-virtual-p-request-id-request-binding-t-tags-exact\n",
    "binding=host-project-caller-nip98-event-id-exact-authenticated-body\n",
    "authorization=host-bound-community-project-context-read-before-index-or-provider\n",
    "execution=stage-a-ticket-stage-b-final-egress-permit-one-provider-call-stage-c-repeatable-read-early-stop-snapshot-close-stage-d-result-release-confirmation\n",
    "packing=deterministic-single-signed-event-whole-summary-whole-path\n",
    "errors=closed-content-free-400-401-403-409-413-429-503-504\n",
    "ordinary-query=semantic-extension-exclusive-kind-40912-always-denied",
    "\ncomputation-route=closed-compiled-profile-bound-separately",
    "\ncoordinate-search=request-one-natural-language-input-limit-1-to-32;result-kind-40913-coordinate-rank-score-only;extensions-carryforth_project_context_coordinate_search-and-v2-filtered;v2-closed-coordinate-type-filter-before-score-and-top-k;no-floor-no-edge-no-path",
    "\none-hop-search=request-one-natural-language-q0-input-limit-1-to-32-tagged-incident-edge-or-edge-coordinate-scope;result-kind-40914-canonical-preview-and-typed-read-descriptor;extensions-carryforth_project_context_one_hop_semantic_search-and-v2-filtered-edge-coordinate;v2-filter-only-edge-coordinate-members-before-score-and-top-k;direct-cosine-no-floor-no-coherence-no-path"
);

/// One instance asserted to be in the current HTTP load-balancer inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticGraphHttpFleetInstance {
    /// Stable deployment-control-plane instance identity.
    pub instance_id: String,
    /// Compiled semantic graph HTTP runtime digest reported by this instance.
    pub runtime_digest: Digest32,
    /// Whether this instance has the fail-closed parser and HTTP handler ready.
    pub http_ready: bool,
}

/// Closed operator assertion of every instance currently reachable by HTTP.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticGraphHttpFleetInventory {
    /// Exact transport identity; only `http` is accepted in this phase.
    pub transport: String,
    /// Deployment identity shared by the operator and serving Relay Pods.
    pub deployment_id: String,
    /// Strictly instance-id-sorted, duplicate-free current routing inventory.
    pub instances: Vec<SemanticGraphHttpFleetInstance>,
}

/// Validation failures for an operator-provided routing inventory.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SemanticGraphFleetInventoryError {
    /// Raw JSON exceeded the bounded operator input.
    #[error("semantic graph fleet inventory exceeds {max_bytes} bytes")]
    TooLarge {
        /// Maximum accepted UTF-8 bytes.
        max_bytes: usize,
    },
    /// JSON did not match the strict closed inventory schema.
    #[error("semantic graph fleet inventory is not valid closed JSON")]
    InvalidJson,
    /// The transport was not the currently supported HTTP transport.
    #[error("semantic graph fleet inventory transport must be http")]
    InvalidTransport,
    /// A deployment or instance identity violated the closed identity grammar.
    #[error("semantic graph fleet inventory {field} is invalid")]
    InvalidIdentity {
        /// Closed field label.
        field: &'static str,
    },
    /// Inventory must contain at least one and at most the fleet hard cap.
    #[error("semantic graph fleet inventory instance count is invalid")]
    InvalidInstanceCount,
    /// Instances were not strictly sorted or contained a duplicate identity.
    #[error("semantic graph fleet instances must be strictly sorted and unique")]
    NonCanonicalInstanceOrder,
    /// A routable instance did not report an HTTP-ready handler.
    #[error("semantic graph fleet contains an HTTP-unready instance")]
    HttpNotReady,
    /// The inventory contains more than one compiled runtime contract.
    #[error("semantic graph fleet runtime digests are not homogeneous")]
    MixedRuntime,
    /// The homogeneous inventory does not match this binary's runtime contract.
    #[error("semantic graph fleet runtime digest does not match this binary")]
    RuntimeMismatch,
    /// This binary could not derive its complete runtime contract.
    #[error("semantic graph HTTP runtime contract could not be derived")]
    RuntimeContract,
}

impl SemanticGraphHttpFleetInventory {
    /// Parse and validate one strict bounded JSON routing inventory.
    pub fn parse_json(bytes: &[u8]) -> Result<Self, SemanticGraphFleetInventoryError> {
        if bytes.len() > MAX_SEMANTIC_GRAPH_FLEET_INVENTORY_BYTES {
            return Err(SemanticGraphFleetInventoryError::TooLarge {
                max_bytes: MAX_SEMANTIC_GRAPH_FLEET_INVENTORY_BYTES,
            });
        }
        let inventory: Self = serde_json::from_slice(bytes)
            .map_err(|_| SemanticGraphFleetInventoryError::InvalidJson)?;
        inventory.validate()?;
        Ok(inventory)
    }

    /// Validate closed transport, identity, ordering, readiness, and runtime
    /// homogeneity without consulting a database or local process config.
    pub fn validate(&self) -> Result<(), SemanticGraphFleetInventoryError> {
        if self.transport != SEMANTIC_GRAPH_HTTP_TRANSPORT {
            return Err(SemanticGraphFleetInventoryError::InvalidTransport);
        }
        validate_identity("deployment_id", &self.deployment_id)?;
        if self.instances.is_empty() || self.instances.len() > MAX_SEMANTIC_GRAPH_FLEET_INSTANCES {
            return Err(SemanticGraphFleetInventoryError::InvalidInstanceCount);
        }
        let mut previous: Option<&str> = None;
        let mut common_runtime: Option<Digest32> = None;
        for instance in &self.instances {
            validate_identity("instance_id", &instance.instance_id)?;
            if previous.is_some_and(|value| value >= instance.instance_id.as_str()) {
                return Err(SemanticGraphFleetInventoryError::NonCanonicalInstanceOrder);
            }
            if !instance.http_ready {
                return Err(SemanticGraphFleetInventoryError::HttpNotReady);
            }
            if common_runtime.is_some_and(|digest| digest != instance.runtime_digest) {
                return Err(SemanticGraphFleetInventoryError::MixedRuntime);
            }
            previous = Some(&instance.instance_id);
            common_runtime = Some(instance.runtime_digest);
        }
        Ok(())
    }

    /// Return the common runtime digest after validating the inventory.
    pub fn common_runtime_digest(&self) -> Result<Digest32, SemanticGraphFleetInventoryError> {
        self.validate()?;
        self.instances
            .first()
            .map(|instance| instance.runtime_digest)
            .ok_or(SemanticGraphFleetInventoryError::InvalidInstanceCount)
    }

    /// Validate that every routable instance runs this binary's compiled
    /// semantic graph HTTP contract.
    pub fn validate_for_compiled_runtime(&self) -> Result<(), SemanticGraphFleetInventoryError> {
        let compiled = semantic_graph_http_runtime_digest()
            .map_err(|_| SemanticGraphFleetInventoryError::RuntimeContract)?;
        if self.common_runtime_digest()? != compiled {
            return Err(SemanticGraphFleetInventoryError::RuntimeMismatch);
        }
        Ok(())
    }

    /// Return whether one exact instance identity appears in the inventory.
    pub fn contains_instance(&self, instance_id: &str) -> bool {
        self.instances
            .binary_search_by_key(&instance_id, |instance| instance.instance_id.as_str())
            .is_ok()
    }

    /// Derive a domain-separated digest of the canonical routing inventory.
    pub fn digest(&self) -> Result<Digest32, SemanticGraphFleetInventoryError> {
        self.validate()?;
        let canonical = postcard::to_stdvec(&(
            self.transport.as_str(),
            self.deployment_id.as_str(),
            self.instances
                .iter()
                .map(|instance| {
                    (
                        instance.instance_id.as_str(),
                        instance.runtime_digest,
                        instance.http_ready,
                    )
                })
                .collect::<Vec<_>>(),
        ))
        .map_err(|_| SemanticGraphFleetInventoryError::InvalidJson)?;
        Ok(hash_domain(
            b"buzz.semantic-graph-http-fleet-inventory",
            &[canonical.as_slice()],
        ))
    }
}

/// Return the compiled semantic graph HTTP runtime digest.
///
/// The value combines the explicitly bumped wire/runtime descriptor with the
/// independently frozen query-input, ranking, and budget contracts.
pub fn semantic_graph_http_runtime_digest() -> QueryContractResult<Digest32> {
    let query = query_contract_digest();
    let ranking = ranking_contract_digest()?;
    let budget = budget_profile_digest()?;
    let computation_routes = SEMANTIC_COMPUTATION_ROUTES.canonical_profile();
    Ok(hash_domain(
        b"buzz.semantic-graph-http-runtime",
        &[
            SEMANTIC_GRAPH_HTTP_RUNTIME_CONTRACT.as_bytes(),
            computation_routes.as_bytes(),
            query.as_bytes(),
            ranking.as_bytes(),
            budget.as_bytes(),
        ],
    ))
}

fn validate_identity(
    field: &'static str,
    value: &str,
) -> Result<(), SemanticGraphFleetInventoryError> {
    if value.is_empty()
        || value.len() > 128
        || value.trim() != value
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'/' | b'-')
        })
    {
        return Err(SemanticGraphFleetInventoryError::InvalidIdentity { field });
    }
    Ok(())
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
    use super::{
        semantic_graph_http_runtime_digest, ParseSemanticGraphQueryFleetPolicyError,
        SemanticComputationRoute, SemanticGraphFleetInventoryError, SemanticGraphHttpFleetInstance,
        SemanticGraphHttpFleetInventory, SemanticGraphQueryEnableRequirement,
        SemanticGraphQueryFleetPolicy, SemanticGraphQueryRoutingTrust, SEMANTIC_COMPUTATION_ROUTES,
        SEMANTIC_GRAPH_HTTP_TRANSPORT,
    };

    fn inventory() -> SemanticGraphHttpFleetInventory {
        let runtime_digest = semantic_graph_http_runtime_digest().expect("runtime digest");
        SemanticGraphHttpFleetInventory {
            transport: SEMANTIC_GRAPH_HTTP_TRANSPORT.to_owned(),
            deployment_id: "buzz-prod-cn-1".to_owned(),
            instances: vec![
                SemanticGraphHttpFleetInstance {
                    instance_id: "relay-0".to_owned(),
                    runtime_digest,
                    http_ready: true,
                },
                SemanticGraphHttpFleetInstance {
                    instance_id: "relay-1".to_owned(),
                    runtime_digest,
                    http_ready: true,
                },
            ],
        }
    }

    #[test]
    fn runtime_digest_is_stable_and_nonzero() {
        let digest = semantic_graph_http_runtime_digest().expect("runtime digest");
        assert_ne!(digest.as_bytes(), &[0; 32]);
        assert_eq!(
            digest.to_hex(),
            "d9878ff28260cc8161795ce8cd479ba879387f3f34ae35b389734fd6ea753bef",
            "incompatible HTTP runtime changes require an explicit contract bump"
        );
        assert_eq!(
            digest,
            semantic_graph_http_runtime_digest().expect("runtime digest")
        );
    }

    #[test]
    fn compiled_computation_routes_are_closed_and_digest_visible() {
        assert_eq!(
            SEMANTIC_COMPUTATION_ROUTES.edge_member_coordinate,
            SemanticComputationRoute::Migrated
        );
        assert_eq!(
            SEMANTIC_COMPUTATION_ROUTES.coordinate_incident_edge,
            SemanticComputationRoute::Migrated
        );
        assert_eq!(
            SEMANTIC_COMPUTATION_ROUTES.whole_graph_coordinate_discovery,
            SemanticComputationRoute::Migrated
        );
        assert_eq!(
            SEMANTIC_COMPUTATION_ROUTES.bounded_complete_path,
            SemanticComputationRoute::Migrated
        );
        assert_eq!(
            SEMANTIC_COMPUTATION_ROUTES.canonical_profile(),
            concat!(
                "edge-member-coordinate=migrated\n",
                "coordinate-incident-edge=migrated\n",
                "whole-graph-coordinate-discovery=migrated\n",
                "bounded-complete-path=migrated\n",
            )
        );
    }

    #[test]
    fn fleet_policy_is_strict_stable_and_defaults_to_single_relay() {
        assert_eq!(
            SemanticGraphQueryFleetPolicy::default(),
            SemanticGraphQueryFleetPolicy::TrustedSingleRelay
        );
        for (wire, policy) in [
            (
                "trusted-single-relay",
                SemanticGraphQueryFleetPolicy::TrustedSingleRelay,
            ),
            (
                "attested-fleet",
                SemanticGraphQueryFleetPolicy::AttestedFleet,
            ),
        ] {
            assert_eq!(wire.parse(), Ok(policy));
            assert_eq!(policy.to_string(), wire);
            assert_eq!(
                serde_json::to_string(&policy).expect("serialize"),
                format!("\"{wire}\"")
            );
            assert_eq!(
                serde_json::from_str::<SemanticGraphQueryFleetPolicy>(&format!("\"{wire}\""))
                    .expect("deserialize"),
                policy
            );
        }
        for invalid in [
            "",
            "trusted_single_relay",
            " trusted-single-relay",
            "ATTested-fleet",
        ] {
            assert_eq!(
                invalid.parse::<SemanticGraphQueryFleetPolicy>(),
                Err(ParseSemanticGraphQueryFleetPolicyError)
            );
            assert!(
                serde_json::from_str::<SemanticGraphQueryFleetPolicy>(&format!("\"{invalid}\""))
                    .is_err()
            );
        }
    }

    #[test]
    fn typed_requirements_report_their_explicit_policy() {
        assert_eq!(
            SemanticGraphQueryRoutingTrust::TrustedSingleRelay.policy(),
            SemanticGraphQueryFleetPolicy::TrustedSingleRelay
        );
        assert_eq!(
            SemanticGraphQueryRoutingTrust::AttestedFleet {
                deployment_id: "deployment-a",
                instance_id: "relay-0",
            }
            .policy(),
            SemanticGraphQueryFleetPolicy::AttestedFleet
        );
        assert_eq!(
            SemanticGraphQueryEnableRequirement::TrustedSingleRelay.policy(),
            SemanticGraphQueryFleetPolicy::TrustedSingleRelay
        );
        assert_eq!(
            SemanticGraphQueryEnableRequirement::AttestedFleet {
                deployment_id: "deployment-a",
            }
            .policy(),
            SemanticGraphQueryFleetPolicy::AttestedFleet
        );
    }

    #[test]
    fn strict_inventory_round_trips_and_has_stable_digest() {
        let inventory = inventory();
        inventory.validate_for_compiled_runtime().expect("valid");
        let json = serde_json::to_vec(&inventory).expect("JSON");
        let parsed = SemanticGraphHttpFleetInventory::parse_json(&json).expect("parse");
        assert_eq!(parsed, inventory);
        assert_eq!(
            parsed.digest().expect("digest"),
            inventory.digest().expect("digest")
        );
        assert!(parsed.contains_instance("relay-0"));
        assert!(!parsed.contains_instance("relay-2"));
    }

    #[test]
    fn previous_route_profile_requires_a_new_fleet_attestation() {
        let previous_runtime = buzz_semantic::Digest32::from_hex(
            "9601b1014e85e16d0eaa8db6146e168653353e489646478380234fc4f56565c8",
        )
        .expect("U4 runtime digest");
        let mut previous_inventory = inventory();
        for instance in &mut previous_inventory.instances {
            instance.runtime_digest = previous_runtime;
        }

        previous_inventory
            .validate()
            .expect("previous profile remains internally homogeneous");
        assert_eq!(
            previous_inventory.validate_for_compiled_runtime(),
            Err(SemanticGraphFleetInventoryError::RuntimeMismatch)
        );
    }

    #[test]
    fn inventory_rejects_unknown_unsorted_unready_and_mixed_runtime() {
        let mut value = serde_json::to_value(inventory()).expect("JSON");
        value
            .as_object_mut()
            .expect("object")
            .insert("unknown".to_owned(), serde_json::json!(true));
        assert_eq!(
            SemanticGraphHttpFleetInventory::parse_json(&serde_json::to_vec(&value).expect("JSON")),
            Err(SemanticGraphFleetInventoryError::InvalidJson)
        );

        let mut invalid = inventory();
        invalid.instances.swap(0, 1);
        assert_eq!(
            invalid.validate(),
            Err(SemanticGraphFleetInventoryError::NonCanonicalInstanceOrder)
        );

        let mut invalid = inventory();
        invalid.instances[0].http_ready = false;
        assert_eq!(
            invalid.validate(),
            Err(SemanticGraphFleetInventoryError::HttpNotReady)
        );

        let mut invalid = inventory();
        invalid.instances[1].runtime_digest = buzz_semantic::Digest32::from_bytes([9; 32]);
        assert_eq!(
            invalid.validate(),
            Err(SemanticGraphFleetInventoryError::MixedRuntime)
        );
    }
}
