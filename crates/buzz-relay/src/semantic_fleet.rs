//! Fail-closed routing checks for semantic graph HTTP queries.

use std::future::Future;

use buzz_core::CommunityId;
use buzz_db::semantic_fleet::SemanticGraphHttpFleetFailure;
#[cfg(test)]
use buzz_semantic_query::SemanticGraphQueryFleetPolicy;
use buzz_semantic_query::SemanticGraphQueryRoutingTrust;

use crate::AppState;

#[derive(Debug, Clone, Copy)]
struct SemanticGraphHttpLocalHandlerFacts {
    deployment_master: bool,
    stable_signer: bool,
    routing_configuration_ready: bool,
    runtime_digest_ready: bool,
    provider_available: bool,
}

const fn semantic_graph_http_local_handler_ready_from_facts(
    facts: SemanticGraphHttpLocalHandlerFacts,
) -> bool {
    facts.deployment_master
        && facts.stable_signer
        && facts.routing_configuration_ready
        && facts.runtime_digest_ready
        && facts.provider_available
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SemanticGraphHttpRoutingObservation {
    Ready,
    NotReady(Option<SemanticGraphHttpFleetFailure>),
    Unavailable,
}

impl SemanticGraphHttpRoutingObservation {
    pub(crate) const fn ready(self) -> bool {
        matches!(self, Self::Ready)
    }
}

/// Apply the closed topology policy around one Fleet check.
///
/// The callback is deliberately not invoked for the trusted single-Relay
/// policy. Both tenant-scoped admission/NIP-11 checks and deployment-global
/// readiness use this function, so their bypass and fail-closed behavior
/// cannot drift apart.
async fn semantic_graph_http_routing_ready_with_fleet_check<'a, Check, CheckFuture>(
    routing_trust: SemanticGraphQueryRoutingTrust<'a>,
    fleet_check: Check,
) -> bool
where
    Check: FnOnce(SemanticGraphQueryRoutingTrust<'a>) -> CheckFuture,
    CheckFuture: Future<Output = SemanticGraphHttpRoutingObservation>,
{
    match routing_trust {
        SemanticGraphQueryRoutingTrust::TrustedSingleRelay => true,
        SemanticGraphQueryRoutingTrust::AttestedFleet { .. } => {
            fleet_check(routing_trust).await.ready()
        }
    }
}

/// Deterministic adapter used by consumer tests to exercise the same routing
/// decision as the live DB-backed paths without connecting to infrastructure.
#[cfg(test)]
pub(crate) async fn semantic_graph_http_routing_ready_for_test(
    policy: SemanticGraphQueryFleetPolicy,
    fleet_failure: Option<SemanticGraphHttpFleetFailure>,
) -> bool {
    let routing_trust = match policy {
        SemanticGraphQueryFleetPolicy::TrustedSingleRelay => {
            SemanticGraphQueryRoutingTrust::TrustedSingleRelay
        }
        SemanticGraphQueryFleetPolicy::AttestedFleet => {
            SemanticGraphQueryRoutingTrust::AttestedFleet {
                deployment_id: "test-deployment",
                instance_id: "test-relay",
            }
        }
    };
    semantic_graph_http_routing_ready_with_fleet_check(routing_trust, |_| async move {
        match fleet_failure {
            Some(failure) => SemanticGraphHttpRoutingObservation::NotReady(Some(failure)),
            None => SemanticGraphHttpRoutingObservation::Ready,
        }
    })
    .await
}

/// Whether this process can execute the compiled HTTP semantic-query handler.
///
/// This is intentionally content-free and synchronous so `/_status`, routing
/// validation, readiness, and request admission all describe the same local
/// runtime rather than merely the presence of the parser code.
pub(crate) fn semantic_graph_http_local_handler_ready(state: &AppState) -> bool {
    semantic_graph_http_local_handler_ready_from_facts(SemanticGraphHttpLocalHandlerFacts {
        deployment_master: state.config.semantic_graph_query_http_available
            || state
                .config
                .project_context_coordinate_search_http_available,
        stable_signer: state.config.relay_private_key.is_some(),
        routing_configuration_ready: state.config.semantic_graph_query_routing_trust().is_ok(),
        runtime_digest_ready: buzz_semantic_query::semantic_graph_http_runtime_digest().is_ok(),
        provider_available: matches!(state.semantic_provider(), Ok(Some(_))),
    })
}

/// Return the validated routing trust for DB Stage B and Stage D checks.
pub(crate) fn semantic_graph_query_routing_trust(
    state: &AppState,
) -> Result<SemanticGraphQueryRoutingTrust<'_>, crate::config::ConfigError> {
    if !semantic_graph_http_local_handler_ready(state) {
        return Err(crate::config::ConfigError::InvalidValue(
            "semantic graph HTTP local handler is unavailable".to_owned(),
        ));
    }
    state.config.semantic_graph_query_routing_trust()
}

/// Verify the deployment master, local handler, and policy-specific Community
/// routing requirement before request admission.
pub(crate) async fn semantic_graph_http_routing_ready(
    state: &AppState,
    community_id: CommunityId,
) -> bool {
    semantic_graph_http_routing_observation(state, community_id)
        .await
        .ready()
}

/// Observe tenant-scoped routing readiness while preserving dependency
/// unavailability for NIP-11 capability observation.
pub(crate) async fn semantic_graph_http_routing_observation(
    state: &AppState,
    community_id: CommunityId,
) -> SemanticGraphHttpRoutingObservation {
    let Ok(routing_trust) = semantic_graph_query_routing_trust(state) else {
        return SemanticGraphHttpRoutingObservation::NotReady(None);
    };
    match routing_trust {
        SemanticGraphQueryRoutingTrust::TrustedSingleRelay => {
            SemanticGraphHttpRoutingObservation::Ready
        }
        SemanticGraphQueryRoutingTrust::AttestedFleet {
            deployment_id,
            instance_id,
        } => match state
            .db
            .semantic_graph_http_fleet_readiness(community_id, deployment_id, Some(instance_id))
            .await
        {
            Ok(readiness) => match readiness.failure {
                Some(failure) => SemanticGraphHttpRoutingObservation::NotReady(Some(failure)),
                None => SemanticGraphHttpRoutingObservation::Ready,
            },
            Err(error) => {
                tracing::warn!(
                    community_id = %community_id,
                    "Semantic graph HTTP fleet readiness failed closed: {error}"
                );
                SemanticGraphHttpRoutingObservation::Unavailable
            }
        },
    }
}

/// Verify all currently query-enabled Communities for deployment-global
/// readiness. The local policy has no Community Fleet rows; strict mode keeps
/// the existing per-Community assertion aggregation.
pub(crate) async fn all_enabled_semantic_graph_http_routes_ready(state: &AppState) -> bool {
    let Ok(routing_trust) = semantic_graph_query_routing_trust(state) else {
        return false;
    };
    semantic_graph_http_routing_ready_with_fleet_check(routing_trust, |routing_trust| async move {
        let SemanticGraphQueryRoutingTrust::AttestedFleet {
            deployment_id,
            instance_id,
        } = routing_trust
        else {
            return SemanticGraphHttpRoutingObservation::Unavailable;
        };
        match state
            .db
            .all_enabled_semantic_graph_http_fleets_ready(deployment_id, instance_id)
            .await
        {
            Ok(true) => SemanticGraphHttpRoutingObservation::Ready,
            Ok(false) => SemanticGraphHttpRoutingObservation::NotReady(None),
            Err(error) => {
                tracing::warn!(
                    "Semantic graph HTTP deployment fleet readiness failed closed: {error}"
                );
                SemanticGraphHttpRoutingObservation::Unavailable
            }
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use buzz_db::semantic_fleet::SemanticGraphHttpFleetFailure;
    use buzz_semantic_query::{SemanticGraphQueryFleetPolicy, SemanticGraphQueryRoutingTrust};

    use super::{
        semantic_graph_http_local_handler_ready_from_facts,
        semantic_graph_http_routing_ready_with_fleet_check, SemanticGraphHttpLocalHandlerFacts,
        SemanticGraphHttpRoutingObservation,
    };

    #[test]
    fn compiled_http_runtime_digest_is_available_to_every_relay_binary() {
        assert_ne!(
            buzz_semantic_query::semantic_graph_http_runtime_digest()
                .expect("runtime digest")
                .as_bytes(),
            &[0; 32]
        );
    }

    #[test]
    fn fleet_policy_exposes_closed_status_contract() {
        assert_eq!(
            SemanticGraphQueryFleetPolicy::TrustedSingleRelay.as_str(),
            "trusted-single-relay"
        );
        assert_eq!(
            SemanticGraphQueryFleetPolicy::AttestedFleet.as_str(),
            "attested-fleet"
        );
    }

    #[test]
    fn local_handler_requires_every_non_fleet_runtime_gate() {
        let ready = SemanticGraphHttpLocalHandlerFacts {
            deployment_master: true,
            stable_signer: true,
            routing_configuration_ready: true,
            runtime_digest_ready: true,
            provider_available: true,
        };
        assert!(semantic_graph_http_local_handler_ready_from_facts(ready));

        for blocked in [
            SemanticGraphHttpLocalHandlerFacts {
                deployment_master: false,
                ..ready
            },
            SemanticGraphHttpLocalHandlerFacts {
                stable_signer: false,
                ..ready
            },
            SemanticGraphHttpLocalHandlerFacts {
                routing_configuration_ready: false,
                ..ready
            },
            SemanticGraphHttpLocalHandlerFacts {
                runtime_digest_ready: false,
                ..ready
            },
            SemanticGraphHttpLocalHandlerFacts {
                provider_available: false,
                ..ready
            },
        ] {
            assert!(!semantic_graph_http_local_handler_ready_from_facts(blocked));
        }
    }

    #[tokio::test]
    async fn trusted_single_relay_does_not_consult_dormant_fleet_state() {
        for dormant_failure in [
            SemanticGraphHttpFleetFailure::Missing,
            SemanticGraphHttpFleetFailure::Expired,
            SemanticGraphHttpFleetFailure::Revoked,
        ] {
            let fleet_checks = Cell::new(0);
            let ready = semantic_graph_http_routing_ready_with_fleet_check(
                SemanticGraphQueryRoutingTrust::TrustedSingleRelay,
                |_| async {
                    fleet_checks.set(fleet_checks.get() + 1);
                    SemanticGraphHttpRoutingObservation::NotReady(Some(dormant_failure))
                },
            )
            .await;

            assert!(ready, "dormant Fleet state must not gate the local policy");
            assert_eq!(
                fleet_checks.get(),
                0,
                "the local policy must not read the dormant Fleet row"
            );
        }
    }

    #[tokio::test]
    async fn attested_fleet_fails_closed_for_missing_expired_and_revoked_rows() {
        for failure in [
            SemanticGraphHttpFleetFailure::Missing,
            SemanticGraphHttpFleetFailure::Expired,
            SemanticGraphHttpFleetFailure::Revoked,
        ] {
            let fleet_checks = Cell::new(0);
            let fleet_checks_ref = &fleet_checks;
            let ready = semantic_graph_http_routing_ready_with_fleet_check(
                SemanticGraphQueryRoutingTrust::AttestedFleet {
                    deployment_id: "deployment-a",
                    instance_id: "relay-0",
                },
                |routing_trust| async move {
                    fleet_checks_ref.set(fleet_checks_ref.get() + 1);
                    assert_eq!(
                        routing_trust,
                        SemanticGraphQueryRoutingTrust::AttestedFleet {
                            deployment_id: "deployment-a",
                            instance_id: "relay-0",
                        }
                    );
                    SemanticGraphHttpRoutingObservation::NotReady(Some(failure))
                },
            )
            .await;

            assert!(!ready, "strict policy must reject {failure:?}");
            assert_eq!(fleet_checks.get(), 1, "strict policy must check Fleet");
        }
    }

    #[tokio::test]
    async fn attested_fleet_accepts_only_ready_evidence() {
        let trust = SemanticGraphQueryRoutingTrust::AttestedFleet {
            deployment_id: "deployment-a",
            instance_id: "relay-0",
        };
        assert!(
            semantic_graph_http_routing_ready_with_fleet_check(trust, |_| async {
                SemanticGraphHttpRoutingObservation::Ready
            })
            .await
        );
        assert!(
            !semantic_graph_http_routing_ready_with_fleet_check(trust, |_| async {
                SemanticGraphHttpRoutingObservation::Unavailable
            })
            .await
        );
    }
}
