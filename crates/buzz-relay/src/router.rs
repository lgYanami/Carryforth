//! axum routers — app (WebSocket + REST), health (K8s probes), metrics (Prometheus).

use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::{
    extract::{ConnectInfo, FromRequest, State, WebSocketUpgrade},
    http::{HeaderMap, StatusCode},
    middleware,
    response::{IntoResponse, Json},
    routing::{get, post, put},
    Router,
};
use serde_json::json;
use tower::ServiceExt;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::api;
use crate::audio;
use crate::connection::handle_connection;
use crate::metrics::track_metrics;
use crate::nip11::{nip11_document, relay_info_handler};
use crate::state::AppState;

/// Build the axum [`Router`] with all relay routes, middleware, and CORS configuration.
///
/// Pure Nostr protocol: WebSocket (NIP-01), HTTP bridge (NIP-98), media (Blossom),
/// git (smart HTTP), NIP-05, and health probes.
pub fn build_router(state: Arc<AppState>) -> Router {
    let media_body_limit = state
        .config
        .media
        .max_image_bytes
        .max(state.config.media.max_video_bytes) as usize;
    let media_router = Router::new()
        .route("/upload", put(api::media::upload_blob))
        .route("/media/upload", put(api::media::upload_blob))
        .route(
            "/media/{sha256_ext}",
            get(api::media::get_blob).head(api::media::head_blob),
        )
        .layer(RequestBodyLimitLayer::new(media_body_limit))
        .with_state(state.clone());

    let git_router = api::git::git_router(state.clone());

    let git_policy_router = api::git::git_policy_router(state.clone());

    let admin_enabled = state.config.admin.is_some();
    let admin_web_dir = state
        .config
        .admin
        .as_ref()
        .and_then(|config| config.web_dir.clone());
    let admin_router = admin_enabled
        .then(|| Router::new().nest("/api/admin/v1", api::admin::router(state.clone())));

    let api_router = Router::new()
        // WebSocket + NIP-11
        .route("/", get(nip11_or_ws_handler))
        .route("/info", get(relay_info_handler))
        .route("/.well-known/nostr.json", get(api::nip05::nostr_nip05))
        // Health endpoints
        .route("/health", get(health_handler))
        .route("/_liveness", get(liveness_handler))
        .route("/_readiness", get(readiness_handler))
        // Nostr HTTP bridge (NIP-98 auth)
        .route("/events", post(api::bridge::submit_event))
        .route("/query", post(api::bridge::query_events))
        .route("/count", post(api::bridge::count_events))
        .route(
            "/api/local/owner",
            post(api::local_desktop::claim_initial_owner),
        )
        .route(
            "/operator/communities",
            get(api::operator::list_owned_communities).post(api::operator::provision_community),
        )
        .route(
            "/operator/communities/archive",
            post(api::operator::archive_community),
        )
        .route(
            "/operator/communities/unarchive",
            post(api::operator::unarchive_community),
        )
        .route(
            "/operator/communities/availability",
            get(api::operator::community_availability),
        )
        .route(
            "/operator/communities/transfer",
            post(api::operator::transfer_community),
        )
        .route(
            "/operator/project-runtime/bindings",
            post(api::operator::register_runtime_supervisor),
        )
        .route(
            "/operator/project-runtime/bindings/revoke",
            post(api::operator::revoke_runtime_supervisor),
        )
        .route(
            "/api/project-runtime/evidence",
            post(api::project_runtime::record_evidence),
        )
        .route(
            "/api/project-runtime/status",
            get(api::project_runtime::assignment_status),
        )
        .route(
            "/api/project-runtime/maintenance",
            get(api::project_runtime::maintenance_status),
        )
        .route(
            "/api/project-runtime/maintenance/ack",
            post(api::project_runtime::acknowledge_maintenance),
        )
        // Relay invites: mint (owner/admin) + claim (membership-gate exempt)
        .route("/api/invites", post(api::invites::mint_invite))
        .route("/api/join-policy", get(api::invites::join_policy))
        // Policy documents as standalone pages — desktop opens these in the
        // system browser instead of rendering the Markdown in-app.
        .route(
            "/api/join-policy/terms",
            get(api::invites::join_policy_terms),
        )
        .route(
            "/api/join-policy/privacy",
            get(api::invites::join_policy_privacy),
        )
        .route(
            "/api/invites/accept-policy",
            post(api::invites::accept_policy),
        )
        .route("/api/invites/claim", post(api::invites::claim_invite))
        // Moderation queue reads (NIP-98 auth + mod-authz gate, L6)
        .route("/moderation/reports", get(api::bridge::moderation_reports))
        .route("/moderation/audit", get(api::bridge::moderation_audit))
        .route(
            "/moderation/restricted",
            get(api::bridge::moderation_restricted),
        )
        // Webhook trigger (secret-authenticated, no NIP-98)
        .route("/hooks/{id}", post(api::bridge::workflow_webhook))
        // Mesh demo echo probe — testbed-only; 404 unless BUZZ_MESH=on and
        // BUZZ_MESH_DEMO_ECHO=on (see api::mesh_demo).
        .route("/_mesh/demo/echo", post(api::mesh_demo::demo_echo))
        // Huddle audio WebSocket route
        .route(
            "/huddle/{channel_id}/audio",
            get(audio::handler::ws_audio_handler),
        )
        // Reject request bodies larger than 1 MB to prevent resource exhaustion.
        .layer(RequestBodyLimitLayer::new(1024 * 1024))
        .with_state(state.clone());

    // Merge — each sub-router carries its own body limit.
    // Metrics → Trace → CORS applied once over the combined router.
    let mut merged = api_router
        .merge(media_router)
        .merge(git_router)
        .merge(git_policy_router);
    if let Some(admin_router) = admin_router {
        merged = merged.merge(admin_router);
    }

    // Serve both bundles from one fallback. The admin host is checked first so
    // it can never fall through to the public web bundle.
    let web_dir = state.config.web_dir.clone();
    if admin_web_dir.is_some() || web_dir.is_some() {
        let admin_index = admin_web_dir.as_ref().map(|dir| dir.join("index.html"));
        let admin_files = admin_web_dir.map(ServeDir::new);
        let web_index = web_dir.as_ref().map(|dir| dir.join("index.html"));
        let web_files = web_dir.map(ServeDir::new);
        let serve_git_web_gui = state.config.serve_git_web_gui;
        let fallback_state = state.clone();
        let spa_fallback = tower::service_fn(move |req: axum::extract::Request| {
            let admin_index = admin_index.clone();
            let admin_files = admin_files.clone();
            let web_index = web_index.clone();
            let web_files = web_files.clone();
            let state = fallback_state.clone();
            async move {
                let path = req.uri().path();
                let admin_host = api::admin::is_admin_host(&state, req.headers());
                if admin_host {
                    if let (Some(index), Some(files)) = (admin_index, admin_files) {
                        if path.starts_with("/assets/") {
                            return files.oneshot(req).await.map(IntoResponse::into_response);
                        }
                        if is_admin_spa_path(path) {
                            return Ok(read_spa_index(&index).await);
                        }
                    }
                    return Ok(StatusCode::NOT_FOUND.into_response());
                }

                if let (Some(index), Some(files)) = (web_index, web_files) {
                    if path.starts_with("/assets/") {
                        return files.oneshot(req).await.map(IntoResponse::into_response);
                    }
                    if should_serve_spa(path, serve_git_web_gui) {
                        return Ok(read_spa_index(&index).await);
                    }
                }
                Ok(StatusCode::NOT_FOUND.into_response())
            }
        });
        merged = merged.fallback_service(spa_fallback);
    }

    merged
        .layer(middleware::from_fn(track_metrics))
        .layer(TraceLayer::new_for_http())
        .layer(build_cors_layer(&state.config.cors_origins))
}

fn is_admin_spa_path(path: &str) -> bool {
    path == "/"
        || path == "/reports"
        || path.starts_with("/reports/")
        || path == "/feedback"
        || path.starts_with("/feedback/")
}

fn is_invite_landing_path(path: &str) -> bool {
    path.strip_prefix("/invite/")
        .is_some_and(|code| !code.is_empty() && !code.contains('/'))
}

fn should_serve_spa(path: &str, serve_git_web_gui: bool) -> bool {
    is_invite_landing_path(path) || (serve_git_web_gui && is_git_web_gui_path(path))
}

fn is_git_web_gui_path(path: &str) -> bool {
    path == "/" || path == "/repos" || path.starts_with("/repos/")
}

async fn read_spa_index(index: &std::path::Path) -> axum::response::Response {
    match tokio::fs::read(index).await {
        Ok(body) => axum::response::Html(body).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Build the health-only router for K8s probes (port 8080 in CAKE).
///
/// No metrics middleware, no auth, no CORS, no body limit.
pub fn build_health_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/_liveness", get(liveness_handler))
        .route("/_readiness", get(readiness_handler))
        .route("/_status", get(status_handler))
        .route("/_mesh", get(mesh_status_handler))
        .with_state(state)
}

/// Content-negotiated: NIP-11 JSON for plain HTTP, WebSocket upgrade otherwise.
async fn nip11_or_ws_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Request,
) -> impl IntoResponse {
    let addr = req
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0)
        .unwrap_or_else(|| std::net::SocketAddr::from(([0, 0, 0, 0], 0)));

    let accept = headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let raw_host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    // `/` is an explicit relay route, so it never reaches the SPA fallback.
    // Short-circuit the exact admin authority here and never let it serve the
    // public web bundle, NIP-11 document, or WebSocket endpoint.
    if api::admin::is_admin_host(&state, &headers) {
        if !accept.contains("text/html") {
            return StatusCode::NOT_FOUND.into_response();
        }
        let Some(index) = state
            .config
            .admin
            .as_ref()
            .and_then(|config| config.web_dir.as_ref())
            .map(|dir| dir.join("index.html"))
        else {
            return StatusCode::NOT_FOUND.into_response();
        };
        return read_spa_index(&index).await;
    }

    if accept.contains("application/nostr+json") {
        return Json(nip11_document(&state, raw_host).await).into_response();
    }

    // Row zero: bind the connection to its community from the request host
    // BEFORE the WebSocket upgrade, so no frame is ever read on an unbound
    // connection. The host is the authoritative selector; an unmapped host or a
    // lookup failure fails closed with a generic rejection — never a default
    // tenant. NIP-11 above is served before binding and stays fail-open: an
    // unmapped host still gets the document (with host-scoped fields like
    // `icon` simply absent), so the doc cannot leak which hosts are mapped.
    let tenant = match crate::tenant::bind_community(&state.db, raw_host).await {
        Ok(ctx) => ctx,
        Err(_) => {
            // Generic rejection: do not distinguish "unmapped" from "lookup
            // error", and never echo the host, so an unauthenticated caller
            // cannot probe which communities exist on this deployment.
            return (
                StatusCode::NOT_FOUND,
                "relay: no community is configured for this host",
            )
                .into_response();
        }
    };

    let max_frame_bytes = state.config.max_frame_bytes;
    match WebSocketUpgrade::from_request(req, &state).await {
        Ok(ws) => {
            // Shutting down: refuse new sockets instead of accepting a
            // connection onto a dying pod. Readiness already returns 503, but
            // that only stops K8s routing — direct and in-flight upgrades
            // still reach here during the pre-drain grace window. Clients
            // treat the refusal as a normal dial failure and retry, landing
            // on a healthy pod.
            if state.shutting_down.load(Ordering::Relaxed) {
                return (StatusCode::SERVICE_UNAVAILABLE, "relay restarting").into_response();
            }
            limit_relay_websocket(ws, max_frame_bytes)
                .on_upgrade(move |socket| handle_connection(socket, state, addr, tenant))
                .into_response()
        }
        Err(_) => {
            // Browser requesting HTML and Git web GUI is enabled → serve SPA.
            if state.config.serve_git_web_gui {
                if let Some(ref dir) = state.config.web_dir {
                    if accept.contains("text/html") {
                        let index = dir.join("index.html");
                        if let Ok(body) = tokio::fs::read(&index).await {
                            return axum::response::Html(body).into_response();
                        }
                    }
                }
            }
            // Not a WS request and not asking for nostr+json — serve NIP-11 as fallback.
            Json(nip11_document(&state, raw_host).await).into_response()
        }
    }
}

fn limit_relay_websocket<F>(
    ws: WebSocketUpgrade<F>,
    max_frame_bytes: usize,
) -> WebSocketUpgrade<F> {
    // recv_loop keeps the application-level check as defense in depth, but
    // parser limits must be set before tungstenite assembles the message.
    ws.max_message_size(max_frame_bytes)
        .max_frame_size(max_frame_bytes)
}

async fn health_handler() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

async fn liveness_handler() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

#[derive(Debug, Clone, Copy)]
struct RelayReadinessFacts {
    postgres: bool,
    redis: bool,
    project_view: bool,
    project_document: bool,
    project_context: bool,
    meeting_community_read: bool,
    meeting_v2: bool,
    semantic: bool,
}

impl RelayReadinessFacts {
    const fn ready(self) -> bool {
        self.postgres
            && self.redis
            && self.project_view
            && self.project_document
            && self.project_context
            && self.meeting_community_read
            && self.meeting_v2
            && self.semantic
    }
}

fn relay_readiness_result(facts: RelayReadinessFacts) -> (StatusCode, Json<serde_json::Value>) {
    if facts.ready() {
        (
            StatusCode::OK,
            Json(json!({"status": "ready", "meeting_v2": true, "semantic": true})),
        )
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "status": "not_ready",
                "postgres": facts.postgres,
                "redis": facts.redis,
                "project_view": facts.project_view,
                "project_document": facts.project_document,
                "project_context": facts.project_context,
                "meeting_community_read": facts.meeting_community_read,
                "meeting_v2": facts.meeting_v2,
                "semantic": facts.semantic
            })),
        )
    }
}

async fn semantic_deployment_readiness_with<Check, CheckFuture, Error>(
    graph_query_runtime_ready: bool,
    check: Check,
) -> bool
where
    Check: FnOnce(bool) -> CheckFuture,
    CheckFuture: Future<Output = Result<bool, Error>>,
{
    check(graph_query_runtime_ready).await.unwrap_or(false)
}

/// Readiness probe — checks shutdown flag, Postgres, and Redis connectivity.
async fn readiness_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    use std::time::Duration;

    if state.shutting_down.load(Ordering::Relaxed) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status": "shutting_down"})),
        )
            .into_response();
    }

    let check = async {
        let (
            pg_ok,
            redis_ok,
            project_view_ok,
            project_document_ok,
            project_context_ok,
            meeting_community_read_ok,
            meeting_v2_ok,
            semantic_ok,
        ) = tokio::join!(
            state.db.ping(),
            async { state.redis_pool.get().await.is_ok() },
            async {
                state
                    .db
                    .project_view_deployment_ready(state.config.relay_private_key.is_some())
                    .await
                    .unwrap_or(false)
            },
            async {
                state
                    .db
                    .project_document_deployment_ready(state.config.relay_private_key.is_some())
                    .await
                    .unwrap_or(false)
            },
            async {
                state
                    .db
                    .project_context_deployment_ready(state.config.relay_private_key.is_some())
                    .await
                    .unwrap_or(false)
            },
            async {
                state
                    .db
                    .meeting_community_read_deployment_ready(
                        state.config.meeting_community_read_enabled,
                    )
                    .await
                    .unwrap_or(false)
            },
            async {
                state
                    .db
                    .meeting_v2_deployment_ready(
                        state.config.relay_private_key.is_some(),
                        state.config.meeting_v2_create_enabled
                            || state.config.meeting_v2_direct_actions_create_enabled,
                    )
                    .await
                    .unwrap_or(false)
            },
            async {
                let graph_query_runtime_ready =
                    crate::semantic_fleet::all_enabled_semantic_graph_http_routes_ready(&state)
                        .await;
                semantic_deployment_readiness_with(graph_query_runtime_ready, |routing_ready| {
                    state.db.semantic_deployment_ready(
                        state.config.semantic_worker.enabled,
                        routing_ready,
                    )
                })
                .await
            },
        );
        (
            pg_ok,
            redis_ok,
            project_view_ok,
            project_document_ok,
            project_context_ok,
            meeting_community_read_ok,
            meeting_v2_ok,
            semantic_ok,
        )
    };

    let (
        pg_ok,
        redis_ok,
        project_view_ok,
        project_document_ok,
        project_context_ok,
        meeting_community_read_ok,
        meeting_v2_ok,
        semantic_ok,
    ) = tokio::time::timeout(Duration::from_secs(2), check)
        .await
        .unwrap_or((false, false, false, false, false, false, false, false));

    relay_readiness_result(RelayReadinessFacts {
        postgres: pg_ok,
        redis: redis_ok,
        project_view: project_view_ok,
        project_document: project_document_ok,
        project_context: project_context_ok,
        meeting_community_read: meeting_community_read_ok,
        meeting_v2: meeting_v2_ok,
        semantic: semantic_ok,
    })
    .into_response()
}

/// Status endpoint — service name, version, uptime.
async fn status_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let uptime_secs = state.started_at.elapsed().as_secs();
    let semantic_query_runtime_digest = buzz_semantic_query::semantic_graph_http_runtime_digest()
        .ok()
        .map(buzz_semantic::Digest32::to_hex);
    let semantic_query_handler_ready =
        crate::semantic_fleet::semantic_graph_http_local_handler_ready(&state);
    let (fleet_attestation_required, fleet_attestation_status) =
        semantic_graph_fleet_status(state.config.semantic_graph_query_fleet_policy);
    Json(json!({
        "service": "buzz-relay",
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_seconds": uptime_secs,
        "semantic_graph_query_http": {
            "runtime_digest": semantic_query_runtime_digest,
            "parser_ready": true,
            "handler_ready": semantic_query_handler_ready,
            "deployment_master": state.config.semantic_graph_query_http_available,
            "fleet_policy": state.config.semantic_graph_query_fleet_policy.as_str(),
            "fleet_attestation_required": fleet_attestation_required,
            "fleet_attestation_status": fleet_attestation_status,
            "deployment_id": state.config.semantic_graph_query_deployment_id,
            "instance_id": state.config.semantic_graph_query_instance_id,
        },
    }))
}

const fn semantic_graph_fleet_status(
    policy: buzz_semantic_query::SemanticGraphQueryFleetPolicy,
) -> (bool, &'static str) {
    match policy {
        buzz_semantic_query::SemanticGraphQueryFleetPolicy::TrustedSingleRelay => {
            (false, "not_required")
        }
        buzz_semantic_query::SemanticGraphQueryFleetPolicy::AttestedFleet => {
            (true, "community_scoped_not_evaluated")
        }
    }
}

/// `/_mesh` — live mesh status: peer table, connection/phi state, per-peer
/// counters, fence-rejection totals. Mesh-off reports `{"enabled": false}` so
/// operators can distinguish "off" from "on with zero peers".
async fn mesh_status_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.mesh() {
        Some(handle) => Json(serde_json::to_value(handle.status()).unwrap_or_else(
            |e| json!({"enabled": true, "error": format!("status serialize: {e}")}),
        )),
        None => Json(json!({"enabled": false})),
    }
}

/// Build a CORS layer from the configured origins list.
fn build_cors_layer(cors_origins: &[String]) -> CorsLayer {
    if cors_origins.is_empty() {
        return CorsLayer::permissive();
    }

    let origins: Vec<axum::http::HeaderValue> = cors_origins
        .iter()
        .filter_map(|o| o.parse::<axum::http::HeaderValue>().ok())
        .collect();

    if origins.is_empty() {
        tracing::error!(
            "BUZZ_CORS_ORIGINS set but no valid origins could be parsed — \
             refusing to fall back to permissive CORS. Fix the origins or unset \
             the variable for development mode."
        );
        return CorsLayer::new();
    }

    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any)
}

#[cfg(test)]
mod tests {
    use axum::{routing::get, Router};
    use buzz_db::semantic_fleet::SemanticGraphHttpFleetFailure;
    use buzz_semantic_query::SemanticGraphQueryFleetPolicy;
    use futures_util::SinkExt;
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    use super::*;

    #[test]
    fn invite_landing_path_requires_exactly_one_nonempty_code_segment() {
        assert!(is_invite_landing_path("/invite/payload.mac"));
        assert!(!is_invite_landing_path("/invite/"));
        assert!(!is_invite_landing_path("/invite/code/extra"));
        assert!(!is_invite_landing_path("/repos"));
        assert!(!is_invite_landing_path("/"));
    }

    #[test]
    fn git_web_gui_paths_are_explicit() {
        assert!(is_git_web_gui_path("/"));
        assert!(is_git_web_gui_path("/repos"));
        assert!(is_git_web_gui_path("/repos/example"));
        assert!(!is_git_web_gui_path("/repository"));
        assert!(!is_git_web_gui_path("/arbitrary"));
        assert!(!is_git_web_gui_path("/api/invites"));
    }

    #[test]
    fn invite_is_always_served_but_git_gui_requires_opt_in() {
        assert!(should_serve_spa("/invite/payload.mac", false));
        assert!(should_serve_spa("/invite/payload.mac", true));
        assert!(!should_serve_spa("/", false));
        assert!(!should_serve_spa("/repos/example", false));
        assert!(should_serve_spa("/", true));
        assert!(should_serve_spa("/repos/example", true));
        assert!(!should_serve_spa("/arbitrary", true));
    }

    #[test]
    fn semantic_graph_status_distinguishes_local_and_community_scoped_fleet_policies() {
        assert_eq!(
            semantic_graph_fleet_status(
                buzz_semantic_query::SemanticGraphQueryFleetPolicy::TrustedSingleRelay
            ),
            (false, "not_required")
        );
        assert_eq!(
            semantic_graph_fleet_status(
                buzz_semantic_query::SemanticGraphQueryFleetPolicy::AttestedFleet
            ),
            (true, "community_scoped_not_evaluated")
        );
    }

    #[tokio::test]
    async fn readiness_uses_shared_fleet_policy_decision_when_other_gates_are_ready() {
        let otherwise_ready = RelayReadinessFacts {
            postgres: true,
            redis: true,
            project_view: true,
            project_document: true,
            project_context: true,
            meeting_community_read: true,
            meeting_v2: true,
            semantic: true,
        };
        for failure in [
            SemanticGraphHttpFleetFailure::Missing,
            SemanticGraphHttpFleetFailure::Expired,
            SemanticGraphHttpFleetFailure::Revoked,
        ] {
            for (policy, expected_status) in [
                (
                    SemanticGraphQueryFleetPolicy::TrustedSingleRelay,
                    StatusCode::OK,
                ),
                (
                    SemanticGraphQueryFleetPolicy::AttestedFleet,
                    StatusCode::SERVICE_UNAVAILABLE,
                ),
            ] {
                let routing_ready =
                    crate::semantic_fleet::semantic_graph_http_routing_ready_for_test(
                        policy,
                        Some(failure),
                    )
                    .await;
                let semantic_ready = semantic_deployment_readiness_with(
                    routing_ready,
                    |observed_routing_ready| async move {
                        assert_eq!(observed_routing_ready, routing_ready);
                        Ok::<bool, ()>(observed_routing_ready)
                    },
                )
                .await;
                let (status, Json(body)) = relay_readiness_result(RelayReadinessFacts {
                    semantic: semantic_ready,
                    ..otherwise_ready
                });

                assert_eq!(
                    status, expected_status,
                    "readiness policy={policy} fleet_failure={failure:?}"
                );
                assert_eq!(body["semantic"], semantic_ready);
            }
        }
    }

    async fn handler_receives_message_with_limit(limit: usize, size: usize) -> bool {
        let (received_tx, mut received_rx) = mpsc::unbounded_channel();
        let app = Router::new().route(
            "/",
            get(move |ws: WebSocketUpgrade| {
                let received_tx = received_tx.clone();
                async move {
                    limit_relay_websocket(ws, limit).on_upgrade(move |mut socket| async move {
                        let _ = received_tx.send(matches!(socket.recv().await, Some(Ok(_))));
                    })
                }
            }),
        );

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test WebSocket listener");
        let addr = listener.local_addr().expect("test listener address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test WebSocket server");
        });

        let (mut client, _) = connect_async(format!("ws://{addr}/"))
            .await
            .expect("connect test WebSocket client");
        client
            .send(Message::Text("x".repeat(size).into()))
            .await
            .expect("send test WebSocket message");

        let received = tokio::time::timeout(std::time::Duration::from_secs(2), received_rx.recv())
            .await
            .expect("server should process the test message")
            .expect("server should report whether it received the message");

        server.abort();
        let _ = server.await;

        received
    }

    #[tokio::test]
    async fn relay_websocket_parser_rejects_oversized_messages_before_handler_reads_them() {
        let limit = 64;

        assert!(
            handler_receives_message_with_limit(limit, limit).await,
            "messages at the relay limit should still reach the handler"
        );
        assert!(
            !handler_receives_message_with_limit(limit, limit + 1).await,
            "oversized messages must be rejected by the WebSocket parser before the handler sees them"
        );
    }
}
