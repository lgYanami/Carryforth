//! Fail-closed local-Pod checks for semantic graph HTTP fleet assertions.

use buzz_core::CommunityId;

use crate::AppState;

/// Whether this process can execute the compiled HTTP semantic-query handler.
///
/// This is intentionally content-free and synchronous so `/_status`, fleet
/// validation, readiness, and request admission all describe the same local
/// runtime rather than merely the presence of the parser code.
pub(crate) fn semantic_graph_http_local_handler_ready(state: &AppState) -> bool {
    state.config.semantic_graph_query_http_available
        && state.config.relay_private_key.is_some()
        && state.config.semantic_graph_query_deployment_id.is_some()
        && state.config.semantic_graph_query_instance_id.is_some()
        && buzz_semantic_query::semantic_graph_http_runtime_digest().is_ok()
        && matches!(state.semantic_provider(), Ok(Some(_)))
}

/// Verify the deployment master, local identity, and current Community fleet
/// assertion before Provider egress and again before final result signing.
pub(crate) async fn semantic_graph_http_fleet_ready(
    state: &AppState,
    community_id: CommunityId,
) -> bool {
    if !semantic_graph_http_local_handler_ready(state) {
        return false;
    }
    let (Some(deployment_id), Some(instance_id)) = (
        state.config.semantic_graph_query_deployment_id.as_deref(),
        state.config.semantic_graph_query_instance_id.as_deref(),
    ) else {
        return false;
    };
    match state
        .db
        .semantic_graph_http_fleet_readiness(community_id, deployment_id, Some(instance_id))
        .await
    {
        Ok(readiness) => readiness.ready(),
        Err(error) => {
            tracing::warn!(
                community_id = %community_id,
                "Semantic graph HTTP fleet readiness failed closed: {error}"
            );
            false
        }
    }
}

/// Verify all currently query-enabled Communities for deployment-global
/// Kubernetes readiness. Disabled Communities do not need an assertion.
pub(crate) async fn all_enabled_semantic_graph_http_fleets_ready(state: &AppState) -> bool {
    if !semantic_graph_http_local_handler_ready(state) {
        return false;
    }
    let (Some(deployment_id), Some(instance_id)) = (
        state.config.semantic_graph_query_deployment_id.as_deref(),
        state.config.semantic_graph_query_instance_id.as_deref(),
    ) else {
        return false;
    };
    state
        .db
        .all_enabled_semantic_graph_http_fleets_ready(deployment_id, instance_id)
        .await
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    #[test]
    fn compiled_http_runtime_digest_is_available_to_every_relay_binary() {
        assert_ne!(
            buzz_semantic_query::semantic_graph_http_runtime_digest()
                .expect("runtime digest")
                .as_bytes(),
            &[0; 32]
        );
    }
}
