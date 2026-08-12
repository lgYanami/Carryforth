//! Relay configuration from environment variables.

use sha2::{Digest, Sha256};
use std::{net::SocketAddr, time::Duration};
use thiserror::Error;
use tracing::warn;

use buzz_semantic_query::{SemanticGraphQueryFleetPolicy, SemanticGraphQueryRoutingTrust};

/// Default maximum inbound WebSocket frame size in bytes.
///
/// Must comfortably exceed accepted event content sizes after Nostr JSON and
/// NIP-44 encryption overhead.
pub const DEFAULT_MAX_FRAME_BYTES: usize = 512 * 1024;

/// Errors that can occur while loading relay configuration.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The `BUZZ_BIND_ADDR` environment variable could not be parsed as a socket address.
    #[error("invalid BUZZ_BIND_ADDR: {0}")]
    InvalidBindAddr(String),
    /// A configuration value failed validation.
    #[error("invalid config: {0}")]
    InvalidValue(String),
}

/// Deny-by-default read-only deployment-admin configuration.
#[derive(Debug, Clone)]
pub struct AdminConfig {
    /// Exact admin HTTP authority.
    pub host: String,
    /// Optional admin SPA bundle directory.
    pub web_dir: Option<std::path::PathBuf>,
}

/// Relay-hosted policy content presented on join surfaces.
#[derive(Debug, Clone)]
pub struct JoinPolicyConfig {
    /// Operator-provided Terms of Service document in Markdown.
    pub terms_markdown: Option<String>,
    /// Operator-provided Privacy Policy document in Markdown.
    pub privacy_markdown: Option<String>,
    /// Whether join surfaces must collect an 18+ attestation.
    pub age_attestation_required: bool,
    /// Content-derived identifier binding receipts to the exact policy revision.
    pub version: String,
}

/// Capability-gated Project Context semantic worker configuration.
#[derive(Clone)]
pub struct SemanticWorkerConfig {
    /// Process-wide worker switch. A Community gate is independently required.
    pub enabled: bool,
    /// Approved provider API key. Debug output always redacts this value.
    pub api_key: Option<String>,
    /// Provider base URL ending at the versioned API root.
    pub base_url: Option<url::Url>,
    /// Provider request model/deployment alias. The response must resolve to
    /// the exact generation model contract.
    pub request_model: Option<String>,
    /// Hard timeout for one provider request.
    pub request_timeout: Duration,
    /// Minimum interval between provider requests at the shared DB gate.
    pub request_interval: Duration,
    /// Durable claim lease duration.
    pub claim_seconds: u16,
    /// Attempts before a job becomes poison.
    pub max_attempts: u32,
}

impl std::fmt::Debug for SemanticWorkerConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SemanticWorkerConfig")
            .field("enabled", &self.enabled)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("base_url", &self.base_url)
            .field("request_model", &self.request_model)
            .field("request_timeout", &self.request_timeout)
            .field("request_interval", &self.request_interval)
            .field("claim_seconds", &self.claim_seconds)
            .field("max_attempts", &self.max_attempts)
            .finish()
    }
}

/// Explicit sizing and timeout contract for one Relay-owned Postgres pool.
#[derive(Debug, Clone, Copy)]
pub struct DatabasePoolConfig {
    /// Pool connection ceiling.
    pub max_connections: u32,
    /// Minimum idle connections maintained by SQLx.
    pub min_connections: u32,
    /// Acquire timeout in seconds.
    pub acquire_timeout_secs: u64,
    /// Maximum connection lifetime in seconds.
    pub max_lifetime_secs: u64,
    /// Idle connection timeout in seconds.
    pub idle_timeout_secs: u64,
}

/// Relay runtime configuration, loaded from environment variables.
#[derive(Debug, Clone)]
pub struct Config {
    /// Address the relay HTTP/WebSocket server binds to.
    pub bind_addr: SocketAddr,
    /// Postgres database connection URL.
    pub database_url: String,
    /// Optional read-replica connection URL (e.g. an Aurora `cluster-ro-`
    /// endpoint). Unset means all reads stay on the writer.
    pub read_database_url: Option<String>,
    /// Authoritative writer pool sizing.
    pub db_main_pool: DatabasePoolConfig,
    /// Optional read-replica pool sizing.
    pub db_read_pool: DatabasePoolConfig,
    /// Row-zero host-binding and readiness pool sizing.
    pub db_control_pool: DatabasePoolConfig,
    /// Audit pool sizing when audit logging is enabled.
    pub db_audit_pool: DatabasePoolConfig,
    /// Full-text search pool sizing.
    pub db_search_pool: DatabasePoolConfig,
    /// Writer-pool slots reserved from semantic traversal for ordinary work.
    pub db_ordinary_main_reserve: u32,
    /// PostgreSQL server slots reserved for operations and recovery.
    pub db_server_connection_reserve: u32,
    /// Redis connection URL used by the pub/sub manager.
    pub redis_url: String,
    /// Maximum connections in the shared Redis pool. Defaults to 16.
    ///
    /// deadpool's own default is `CPU_COUNT * 2`, which on a 2-vCPU relay
    /// pod is only 4 — small enough that rate-limit checks, presence, and
    /// pub/sub publishes queue behind each other under load.
    pub redis_pool_size: usize,
    /// Public WebSocket URL of this relay, advertised in NIP-11.
    pub relay_url: String,
    /// Public WebSocket URL of the dedicated device-pairing relay, when configured.
    pub pairing_relay_url: Option<String>,
    /// Maximum number of concurrent WebSocket connections.
    pub max_connections: usize,
    /// Maximum number of concurrently executing message handlers.
    pub max_concurrent_handlers: usize,
    /// Per-connection outbound message buffer size (number of messages).
    pub send_buffer_size: usize,
    /// Maximum inbound WebSocket frame size in bytes.
    pub max_frame_bytes: usize,
    /// Number of consecutive buffer-full events tolerated before cancelling a slow client.
    pub slow_client_grace_limit: u8,
    /// Authentication provider configuration.
    pub auth: buzz_auth::AuthConfig,
    /// Whether REST API requests must present a valid token. Independent of
    /// WebSocket protocol auth, which is *always* required by REQ/EVENT/COUNT.
    pub require_auth_token: bool,
    /// Comma-separated list of allowed CORS origins.
    /// If empty, permissive CORS is used (dev mode).
    /// Example: "tauri://localhost,http://localhost:3000"
    pub cors_origins: Vec<String>,
    /// Optional hex-encoded private key for the relay's signing keypair.
    /// If absent, a fresh keypair is generated at startup.
    pub relay_private_key: Option<String>,
    /// Optional Unix Domain Socket path. When set, the relay also listens on this
    /// UDS for traffic (e.g. service mesh sidecar). Health probes still use TCP.
    pub uds_path: Option<String>,
    /// TCP port for the health-only router (`/_liveness`, `/_readiness`, `/_status`).
    /// Separate from the app router so K8s probes bypass Istio and auth middleware.
    pub health_port: u16,
    /// TCP port for the Prometheus metrics exporter (`GET /metrics`).
    pub metrics_port: u16,

    /// When true, NIP-42 pubkey-only authentication (no API token) is
    /// restricted to pubkeys in the `pubkey_allowlist` table. Users with valid
    /// API tokens bypass the allowlist entirely.
    /// Applies to all NIP-42 pubkey-only connections, regardless of `require_auth_token`.
    pub pubkey_allowlist_enabled: bool,

    /// When true, every authenticated request must also pass a relay-level
    /// membership check against the `relay_members` table.
    /// When false (default), the check is a no-op and all authenticated callers
    /// are permitted regardless of auth method (API token, NIP-42).
    pub require_relay_membership: bool,

    /// Whether this deployment can serve huddle (voice) audio.
    ///
    /// Huddle audio frames are relayed peer-to-peer *within a single pod*
    /// (`AudioRoomManager` is an in-process map; only huddle lifecycle events
    /// cross pods via Redis). Under horizontal scaling (any-pod-any-connection,
    /// plan §4 fork B) two peers in the same huddle can land on different pods
    /// and never hear each other. Rather than sticky-route huddles or ship a
    /// silent split-room (plan §5b, decided by Tyler), a horizontally-scaled
    /// deployment sets this `false` and the relay surfaces a clear, client-
    /// handleable "huddle audio unavailable" signal on join.
    ///
    /// Defaults to `true` so single-pod deployments (the N=1 case) keep today's
    /// behavior unchanged. Operators running multiple relay pods MUST set
    /// `BUZZ_HUDDLE_AUDIO_AVAILABLE=false` until the out-of-relay media/SFU
    /// service lands.
    pub huddle_audio_available: bool,

    /// Whether clients may create new Meeting V1 sessions.
    ///
    /// This rollout gate defaults to `false`. It is checked only when
    /// accepting a `v=2 + moderated-baton-v1` Meeting Create command; existing
    /// V1 sessions continue to be routed and recovered when the gate is off.
    pub meeting_v1_create_enabled: bool,

    /// Whether clients may create new Meeting V2 sessions.
    ///
    /// This stage-one rollout gate defaults to `false`. It is checked only
    /// when accepting a `v=3 + moderated-board-v1` Meeting Create command.
    /// Stage one exposes creation and current-board reads while all later V2
    /// lifecycle mutations remain unavailable.
    pub meeting_v2_create_enabled: bool,

    /// Whether clients may create action-capable Meeting V2 sessions.
    ///
    /// This second rollout gate defaults to `false` and is additive to
    /// [`Self::meeting_v2_create_enabled`]. Existing
    /// `moderated-board-actions-v3` sessions continue to drain and recover
    /// when either Create gate is later disabled.
    pub meeting_v2_direct_actions_create_enabled: bool,

    /// Deployment master switch for Community-wide Meeting reads.
    ///
    /// Defaults to `false`. A Community additionally needs durable operator
    /// approval and publication in migration 0052; this process-wide switch
    /// alone never widens access. Once any Community publishes the contract,
    /// pods with this switch off fail readiness rather than serving split ACLs.
    pub meeting_community_read_enabled: bool,

    /// Inter-relay mesh configuration (`BUZZ_MESH`, `BUZZ_MESH_BIND_ADDR`).
    /// Opt-in: mesh forms only when `BUZZ_MESH=on` is explicit. The default
    /// (absent/off) is exact single-instance behavior — no bind, no Redis
    /// registry write — so an image upgrade with untouched env is a strict
    /// no-regression rollout.
    pub mesh: buzz_relay_mesh::MeshConfig,

    /// Testbed-only reliable-stream echo consumer (`BUZZ_MESH_DEMO_ECHO`).
    /// When `on`, the owner side of an inbound reliable mesh stream echoes
    /// every validated `Data` frame back to the sender — a transport/
    /// session-routing smoke for cross-pod evidence runs, NOT a product flow.
    /// Same strict opt-in as `BUZZ_MESH`; default off means inbound reliable
    /// streams are accepted, logged, and closed (no session consumer yet).
    pub mesh_demo_echo: bool,

    /// Optional hex-encoded pubkey of the relay owner.
    /// When set, this pubkey is automatically bootstrapped into `relay_members`
    /// with the `owner` role on first startup.
    pub relay_owner_pubkey: Option<String>,

    /// Canonical HTTP origin of the deployment-global operator API.
    ///
    /// Every operator NIP-98 `u` tag is verified against this origin, independent
    /// of the inbound HTTP `Host` header and tenant registry. Required when
    /// `RELAY_OPERATOR_PUBKEYS` is non-empty. Set via `RELAY_OPERATOR_API_ORIGIN`
    /// as an `http://` or `https://` origin with no path, query, or fragment.
    pub relay_operator_api_origin: Option<String>,

    /// Deployment-level relay operator pubkeys allowed to use the
    /// `/operator/communities` management endpoints.
    ///
    /// Unlike `relay_owner_pubkey` (a role *within* the deployment community),
    /// operators span tenants: they may create new communities and bootstrap
    /// initial owners, but hold no implicit tenant membership row.
    /// Empty (the default) disables community provisioning entirely — fail closed.
    ///
    /// Set via `RELAY_OPERATOR_PUBKEYS` as a comma-separated list of 64-char
    /// hex pubkeys. Invalid entries are rejected at startup (config error), not
    /// skipped — a typo must not silently disable an operator.
    pub relay_operator_pubkeys: Vec<String>,

    /// Allow NIP-OA owner attestation for relay membership.
    ///
    /// When `true` and `require_relay_membership` is also `true`, agents
    /// bearing a valid NIP-OA `auth` tag can authenticate by proving their
    /// owner is a relay member. The agent gets session-scoped access.
    ///
    /// On open relays (`require_relay_membership = false`), NIP-OA owner
    /// extraction for agent→owner backfill happens unconditionally (the
    /// signature is cryptographically self-proving). This flag only controls
    /// whether NIP-OA can grant membership access on closed relays.
    ///
    /// Default: `false`. Set via `BUZZ_ALLOW_NIP_OA_AUTH=true`.
    pub allow_nip_oa_auth: bool,

    /// Media storage configuration (S3/MinIO).
    pub media: buzz_media::MediaConfig,
    /// Maximum concurrent media uploads handled by one relay process.
    pub media_max_concurrent_uploads: usize,
    /// Maximum concurrent media uploads accepted from one pubkey.
    pub media_max_concurrent_uploads_per_pubkey: u32,
    /// Maximum media upload starts accepted from one pubkey per minute.
    pub media_uploads_per_minute: u32,

    /// Require Blossom kind:24242 `t=get` auth plus relay membership before
    /// serving media GET/HEAD. Default off for staged client rollout.
    pub require_media_get_auth: bool,

    /// Whether tamper-evident event/media audit logging is enabled. Defaults to true.
    /// This does not control the separate `moderation_actions` audit trail.
    /// Set `BUZZ_AUDIT_ENABLED=false` for deployments that do not require it.
    pub audit_enabled: bool,

    /// Deployment kill switch for automatic managed-runtime
    /// `ended(unrecoverable)` transitions. Defaults to false.
    pub runtime_unrecoverable_enabled: bool,
    /// Runtime supervision scheduler polling interval.
    pub runtime_supervision_interval_secs: u64,
    /// Maximum Assignment claims processed by one scheduler tick.
    pub runtime_supervision_batch_limit: u16,
    /// Duration of one multi-pod scheduler claim.
    pub runtime_supervision_claim_secs: u64,

    /// Derived Project Context semantic worker/provider configuration.
    pub semantic_worker: SemanticWorkerConfig,

    /// Deployment master for the semantic graph query HTTP runtime.
    ///
    /// This does not enable any Community and defaults to false. Community DB
    /// readiness and the configured routing policy remain separate gates.
    pub semantic_graph_query_http_available: bool,
    /// Maximum concurrent semantic graph requests admitted by this process.
    pub semantic_graph_query_max_in_flight: usize,
    /// Maximum concurrent Stage C database snapshot/traversal sessions.
    ///
    /// This is distinct from Provider concurrency and preserves ordinary
    /// database capacity under graph-query load.
    pub semantic_graph_traversal_max_in_flight: usize,
    /// Topology trust applied to semantic graph HTTP query routing.
    ///
    /// Local source builds default to trusting the one Relay process. The
    /// attested policy preserves the short-lived fleet inventory gate for a
    /// future multi-instance deployment.
    pub semantic_graph_query_fleet_policy: SemanticGraphQueryFleetPolicy,
    /// Deployment identity that must match the operator Fleet assertion in
    /// `attested-fleet` mode.
    pub semantic_graph_query_deployment_id: Option<String>,
    /// Exact control-plane instance identity for this Relay in
    /// `attested-fleet` mode.
    pub semantic_graph_query_instance_id: Option<String>,

    /// Optional override for ephemeral channel TTL (in seconds).
    /// When set, any channel created with a TTL tag will use this value instead
    /// of the client-provided one. Useful for testing ephemeral expiry quickly.
    /// Example: `BUZZ_EPHEMERAL_TTL_OVERRIDE=60` → all ephemeral channels expire
    /// 60 seconds after the last message.
    pub ephemeral_ttl_override: Option<i32>,

    /// Root directory for the relay's local git scratch. No authoritative
    /// repository state lives here — runtime reads/writes hydrate ephemeral
    /// repos from object storage per request. Temporary workspaces, buffered
    /// subprocess output, and the disposable immutable pack cache live below
    /// this path.
    /// Repo-name uniqueness lives in Postgres (`git_repo_names`), not on disk,
    /// so this directory need not be persistent or shared across replicas.
    pub git_repo_path: std::path::PathBuf,
    /// Parent directory for process-isolated immutable pack cache sessions.
    pub git_pack_cache_path: std::path::PathBuf,
    /// Maximum pack file size for git push (bytes). Default: 500 MB.
    pub git_max_pack_bytes: u64,
    /// Maximum total bytes materialized for one git repo request. Default: 1 GB.
    ///
    /// This bounds clone/fetch hydration work across a repo's historical pack
    /// set rather than only bounding one incoming push body.
    pub git_max_repo_bytes: u64,
    /// Maximum bytes retained in the process-local immutable pack/index cache.
    /// Zero disables retention while preserving request-local hydration.
    pub git_pack_cache_max_bytes: u64,
    /// Maximum pack digests populated concurrently in one relay process.
    pub git_pack_cache_max_concurrent_populations: usize,
    /// Maximum number of repos per pubkey. Default: 100.
    pub git_max_repos_per_pubkey: u32,
    /// Maximum concurrent git subprocess operations. Default: 20.
    pub git_max_concurrent_ops: usize,
    /// HMAC secret for git pre-receive hook callbacks.
    /// Used to authenticate internal policy endpoint requests.
    pub git_hook_hmac_secret: String,

    /// Optional relay-hosted policy shown on join surfaces. Disabled when no
    /// documents or age attestation are configured.
    pub join_policy: Option<JoinPolicyConfig>,

    /// Deployment-admin API and SPA configuration. Absent means the surface is disabled.
    pub admin: Option<AdminConfig>,

    /// Optional path to the web UI `dist/` directory.
    /// When set, the relay serves the invite landing page and its static assets.
    /// When unset, no static file serving happens (relay behaves as before).
    pub web_dir: Option<std::path::PathBuf>,
    /// Whether the configured web bundle serves Git browser routes in addition
    /// to the public invite landing page. Defaults to false.
    pub serve_git_web_gui: bool,
}

fn parse_bind_addr(raw: &str) -> Result<SocketAddr, ConfigError> {
    raw.parse::<SocketAddr>()
        .map_err(|e| ConfigError::InvalidBindAddr(e.to_string()))
}

fn positive_u64_from_env(name: &str, default: u64) -> Result<u64, ConfigError> {
    match std::env::var(name) {
        Ok(raw) => raw
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| ConfigError::InvalidValue(format!("{name} must be a positive integer"))),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(std::env::VarError::NotUnicode(_)) => Err(ConfigError::InvalidValue(format!(
            "{name} must be valid Unicode"
        ))),
    }
}

fn database_pool_config_from_env(
    prefix: &str,
    defaults: DatabasePoolConfig,
) -> Result<DatabasePoolConfig, ConfigError> {
    let max_name = format!("{prefix}_MAX_CONNECTIONS");
    let min_name = format!("{prefix}_MIN_CONNECTIONS");
    let acquire_name = format!("{prefix}_ACQUIRE_TIMEOUT_SECS");
    let lifetime_name = format!("{prefix}_MAX_LIFETIME_SECS");
    let idle_name = format!("{prefix}_IDLE_TIMEOUT_SECS");
    let max_connections = u32::try_from(positive_u64_from_env(
        &max_name,
        u64::from(defaults.max_connections),
    )?)
    .map_err(|_| ConfigError::InvalidValue(format!("{max_name} exceeds u32")))?;
    let min_connections = u32::try_from(positive_u64_from_env(
        &min_name,
        u64::from(defaults.min_connections),
    )?)
    .map_err(|_| ConfigError::InvalidValue(format!("{min_name} exceeds u32")))?;
    if min_connections > max_connections {
        return Err(ConfigError::InvalidValue(format!(
            "{min_name} must not exceed {max_name}"
        )));
    }
    Ok(DatabasePoolConfig {
        max_connections,
        min_connections,
        acquire_timeout_secs: positive_u64_from_env(&acquire_name, defaults.acquire_timeout_secs)?,
        max_lifetime_secs: positive_u64_from_env(&lifetime_name, defaults.max_lifetime_secs)?,
        idle_timeout_secs: positive_u64_from_env(&idle_name, defaults.idle_timeout_secs)?,
    })
}

fn validate_semantic_traversal_capacity(
    process_limit: usize,
    traversal_limit: usize,
    main_pool_max: u32,
    ordinary_main_reserve: u32,
) -> Result<(), ConfigError> {
    if traversal_limit == 0 || traversal_limit > process_limit {
        return Err(ConfigError::InvalidValue(
            "BUZZ_SEMANTIC_GRAPH_TRAVERSAL_MAX_IN_FLIGHT must be in 1..=BUZZ_SEMANTIC_GRAPH_QUERY_MAX_IN_FLIGHT"
                .to_owned(),
        ));
    }
    if ordinary_main_reserve >= main_pool_max {
        return Err(ConfigError::InvalidValue(
            "BUZZ_DB_ORDINARY_MAIN_RESERVE must be smaller than BUZZ_DB_MAIN_MAX_CONNECTIONS"
                .to_owned(),
        ));
    }
    if traversal_limit > (main_pool_max - ordinary_main_reserve) as usize {
        return Err(ConfigError::InvalidValue(
            "BUZZ_SEMANTIC_GRAPH_TRAVERSAL_MAX_IN_FLIGHT exceeds the semantic share of the writer pool"
                .to_owned(),
        ));
    }
    Ok(())
}

fn rate_limit_config_from_env() -> Result<buzz_auth::RateLimitConfig, ConfigError> {
    let defaults = buzz_auth::RateLimitConfig::default();
    Ok(buzz_auth::RateLimitConfig {
        human_messages_per_min: positive_u64_from_env(
            "BUZZ_RATE_LIMIT_HUMAN_MESSAGES_PER_MIN",
            defaults.human_messages_per_min,
        )?,
        human_api_calls_per_min: positive_u64_from_env(
            "BUZZ_RATE_LIMIT_HUMAN_API_CALLS_PER_MIN",
            defaults.human_api_calls_per_min,
        )?,
        human_ws_events_per_sec: positive_u64_from_env(
            "BUZZ_RATE_LIMIT_HUMAN_WS_EVENTS_PER_SEC",
            defaults.human_ws_events_per_sec,
        )?,
        agent_standard_messages_per_min: positive_u64_from_env(
            "BUZZ_RATE_LIMIT_AGENT_STANDARD_MESSAGES_PER_MIN",
            defaults.agent_standard_messages_per_min,
        )?,
        agent_standard_api_calls_per_min: positive_u64_from_env(
            "BUZZ_RATE_LIMIT_AGENT_STANDARD_API_CALLS_PER_MIN",
            defaults.agent_standard_api_calls_per_min,
        )?,
        agent_elevated_messages_per_min: positive_u64_from_env(
            "BUZZ_RATE_LIMIT_AGENT_ELEVATED_MESSAGES_PER_MIN",
            defaults.agent_elevated_messages_per_min,
        )?,
        agent_platform_messages_per_min: positive_u64_from_env(
            "BUZZ_RATE_LIMIT_AGENT_PLATFORM_MESSAGES_PER_MIN",
            defaults.agent_platform_messages_per_min,
        )?,
    })
}

fn parse_operator_api_origin(raw: &str) -> Result<String, ConfigError> {
    let raw = raw.trim();
    let url = url::Url::parse(raw).map_err(|e| {
        ConfigError::InvalidValue(format!("RELAY_OPERATOR_API_ORIGIN is not a valid URL: {e}"))
    })?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ConfigError::InvalidValue(
            "RELAY_OPERATOR_API_ORIGIN must be an http(s) origin with no credentials, path, query, or fragment"
                .to_string(),
        ));
    }
    Ok(raw.trim_end_matches('/').to_string())
}

fn parse_bool(name: &str, default: bool) -> Result<bool, ConfigError> {
    match std::env::var(name) {
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(ConfigError::InvalidValue(format!(
            "{name} must be valid UTF-8: {error}"
        ))),
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "on" => Ok(true),
            "false" | "0" | "off" | "" => Ok(false),
            _ => Err(ConfigError::InvalidValue(format!(
                "{name} must be true or false"
            ))),
        },
    }
}

fn parse_optional_bool(name: &str) -> Result<bool, ConfigError> {
    parse_bool(name, false)
}

fn parse_semantic_query_identity(name: &'static str) -> Result<Option<String>, ConfigError> {
    let raw = match std::env::var(name) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(error) => {
            return Err(ConfigError::InvalidValue(format!(
                "{name} must be valid UTF-8: {error}"
            )));
        }
    };
    validate_semantic_query_identity(name, raw)
}

fn parse_semantic_query_fleet_policy() -> Result<SemanticGraphQueryFleetPolicy, ConfigError> {
    let raw = match std::env::var("BUZZ_SEMANTIC_GRAPH_QUERY_FLEET_POLICY") {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => {
            return Ok(SemanticGraphQueryFleetPolicy::TrustedSingleRelay);
        }
        Err(error) => {
            return Err(ConfigError::InvalidValue(format!(
                "BUZZ_SEMANTIC_GRAPH_QUERY_FLEET_POLICY must be valid UTF-8: {error}"
            )));
        }
    };
    raw.trim().parse().map_err(|error| {
        ConfigError::InvalidValue(format!(
            "BUZZ_SEMANTIC_GRAPH_QUERY_FLEET_POLICY is invalid: {error}"
        ))
    })
}

fn validate_semantic_query_identity(
    name: &'static str,
    raw: Option<String>,
) -> Result<Option<String>, ConfigError> {
    let Some(value) = raw else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > 128
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'/' | b'-')
        })
    {
        return Err(ConfigError::InvalidValue(format!(
            "{name} must use the 1..=128 byte deployment identity grammar"
        )));
    }
    Ok(Some(value.to_owned()))
}

fn require_semantic_query_fleet_identities(
    deployment_master: bool,
    fleet_policy: SemanticGraphQueryFleetPolicy,
    deployment_id: Option<&str>,
    instance_id: Option<&str>,
) -> Result<(), ConfigError> {
    if deployment_master
        && fleet_policy == SemanticGraphQueryFleetPolicy::AttestedFleet
        && (deployment_id.is_none() || instance_id.is_none())
    {
        return Err(ConfigError::InvalidValue(
            "BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE with \
             BUZZ_SEMANTIC_GRAPH_QUERY_FLEET_POLICY=attested-fleet requires non-empty \
             BUZZ_SEMANTIC_GRAPH_QUERY_DEPLOYMENT_ID and \
             BUZZ_SEMANTIC_GRAPH_QUERY_INSTANCE_ID"
                .to_owned(),
        ));
    }
    Ok(())
}

fn ensure_git_repo_path(
    raw: impl Into<std::path::PathBuf>,
) -> Result<std::path::PathBuf, ConfigError> {
    ensure_git_path("BUZZ_GIT_REPO_PATH", raw)
}

fn ensure_git_path(
    setting: &str,
    raw: impl Into<std::path::PathBuf>,
) -> Result<std::path::PathBuf, ConfigError> {
    let git_repo_path = raw.into();
    if let Err(e) = std::fs::create_dir_all(&git_repo_path) {
        return Err(ConfigError::InvalidValue(format!(
            "{setting}={} could not be created: {e}",
            git_repo_path.display()
        )));
    }
    Ok(git_repo_path)
}

impl Config {
    /// Loads configuration from environment variables, falling back to development defaults.
    pub fn from_env() -> Result<Self, ConfigError> {
        let bind_addr_raw =
            std::env::var("BUZZ_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".to_string());
        let bind_addr = parse_bind_addr(&bind_addr_raw)?;

        let database_url = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgres://buzz:buzz_dev@localhost:5432/buzz".to_string()); // sadscan:disable np.postgres.1

        let read_database_url = std::env::var("READ_DATABASE_URL")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());

        let db_main_pool = database_pool_config_from_env(
            "BUZZ_DB_MAIN",
            DatabasePoolConfig {
                max_connections: 12,
                min_connections: 2,
                acquire_timeout_secs: 3,
                max_lifetime_secs: 1_800,
                idle_timeout_secs: 600,
            },
        )?;
        let db_read_pool = database_pool_config_from_env(
            "BUZZ_DB_READ",
            DatabasePoolConfig {
                max_connections: 8,
                min_connections: 1,
                acquire_timeout_secs: 3,
                max_lifetime_secs: 1_800,
                idle_timeout_secs: 600,
            },
        )?;
        let db_control_pool = database_pool_config_from_env(
            "BUZZ_DB_CONTROL",
            DatabasePoolConfig {
                max_connections: 2,
                min_connections: 1,
                acquire_timeout_secs: 1,
                max_lifetime_secs: 1_800,
                idle_timeout_secs: 600,
            },
        )?;
        let db_audit_pool = database_pool_config_from_env(
            "BUZZ_DB_AUDIT",
            DatabasePoolConfig {
                max_connections: 2,
                min_connections: 1,
                acquire_timeout_secs: 3,
                max_lifetime_secs: 1_800,
                idle_timeout_secs: 600,
            },
        )?;
        let db_search_pool = database_pool_config_from_env(
            "BUZZ_DB_SEARCH",
            DatabasePoolConfig {
                max_connections: 2,
                min_connections: 1,
                acquire_timeout_secs: 3,
                max_lifetime_secs: 1_800,
                idle_timeout_secs: 600,
            },
        )?;
        let db_ordinary_main_reserve = u32::try_from(positive_u64_from_env(
            "BUZZ_DB_ORDINARY_MAIN_RESERVE",
            4,
        )?)
        .map_err(|_| {
            ConfigError::InvalidValue("BUZZ_DB_ORDINARY_MAIN_RESERVE exceeds u32".to_owned())
        })?;
        let db_server_connection_reserve = u32::try_from(positive_u64_from_env(
            "BUZZ_DB_SERVER_CONNECTION_RESERVE",
            4,
        )?)
        .map_err(|_| {
            ConfigError::InvalidValue("BUZZ_DB_SERVER_CONNECTION_RESERVE exceeds u32".to_owned())
        })?;

        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".to_string());

        let redis_pool_size = std::env::var("BUZZ_REDIS_POOL_SIZE")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&v| v > 0)
            .unwrap_or(16);

        let relay_url =
            std::env::var("RELAY_URL").unwrap_or_else(|_| "ws://localhost:3000".to_string());

        let pairing_relay_url = std::env::var("BUZZ_PAIRING_RELAY_URL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(|value| {
                let parsed = url::Url::parse(&value).map_err(|e| {
                    ConfigError::InvalidValue(format!(
                        "BUZZ_PAIRING_RELAY_URL must be a valid ws:// or wss:// URL: {e}"
                    ))
                })?;
                if !matches!(parsed.scheme(), "ws" | "wss") || parsed.host_str().is_none() {
                    return Err(ConfigError::InvalidValue(
                        "BUZZ_PAIRING_RELAY_URL must be a valid ws:// or wss:// URL".to_string(),
                    ));
                }
                Ok(value)
            })
            .transpose()?;

        let max_connections = std::env::var("BUZZ_MAX_CONNECTIONS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10_000);

        let max_concurrent_handlers = std::env::var("BUZZ_MAX_CONCURRENT_HANDLERS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1024);

        let send_buffer_size = std::env::var("BUZZ_SEND_BUFFER")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1_000);

        let max_frame_bytes = std::env::var("BUZZ_MAX_FRAME_BYTES")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&v| v > 0)
            .unwrap_or(DEFAULT_MAX_FRAME_BYTES);

        let slow_client_grace_limit = std::env::var("BUZZ_SLOW_CLIENT_GRACE_LIMIT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(15);

        let require_auth_token = std::env::var("BUZZ_REQUIRE_AUTH_TOKEN")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        let pubkey_allowlist_enabled = std::env::var("BUZZ_PUBKEY_ALLOWLIST")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        let require_relay_membership = std::env::var("BUZZ_REQUIRE_RELAY_MEMBERSHIP")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        // Defaults true → single-pod (N=1) keeps today's huddle behavior. A
        // horizontally-scaled deployment sets this false; see the field doc.
        let huddle_audio_available = std::env::var("BUZZ_HUDDLE_AUDIO_AVAILABLE")
            .map(|v| !(v == "false" || v == "0"))
            .unwrap_or(true);

        let meeting_v1_create_enabled = parse_optional_bool("BUZZ_MEETING_V1_CREATE_ENABLED")?;
        let meeting_v2_create_enabled = parse_optional_bool("BUZZ_MEETING_V2_CREATE_ENABLED")?;
        let meeting_v2_direct_actions_create_enabled =
            parse_optional_bool("BUZZ_MEETING_V2_DIRECT_ACTIONS_CREATE_ENABLED")?;
        let meeting_community_read_enabled =
            parse_optional_bool("BUZZ_MEETING_COMMUNITY_READ_ENABLED")?;

        // Mesh opt-in: default OFF. Strict rollout no-regression — an image
        // upgrade with untouched env must not bind a new UDP port or write a
        // new Redis key. Horizontally-scaled deployments explicitly set
        // `BUZZ_MESH=on`; anything else (absent, `off`, other values) keeps
        // exact single-instance behavior.
        let mesh_enabled = std::env::var("BUZZ_MESH")
            .map(|v| v.eq_ignore_ascii_case("on") || v == "true" || v == "1")
            .unwrap_or(false);
        let mesh_bind_addr = std::env::var("BUZZ_MESH_BIND_ADDR")
            .map(|raw| {
                raw.parse::<SocketAddr>().map_err(|e| {
                    ConfigError::InvalidValue(format!("invalid BUZZ_MESH_BIND_ADDR: {e}"))
                })
            })
            .unwrap_or_else(|_| Ok("0.0.0.0:3478".parse().expect("static default parses")))?;
        let mesh = buzz_relay_mesh::MeshConfig {
            enabled: mesh_enabled,
            bind_addr: mesh_bind_addr,
            registry_refresh: std::time::Duration::from_secs(15),
        };

        // Demo echo opt-in: same strict pattern as BUZZ_MESH — explicit
        // `on`/`true`/`1` only, anything else (absent, `off`, typos) is off.
        let mesh_demo_echo = std::env::var("BUZZ_MESH_DEMO_ECHO")
            .map(|v| v.eq_ignore_ascii_case("on") || v == "true" || v == "1")
            .unwrap_or(false);

        let allow_nip_oa_auth = std::env::var("BUZZ_ALLOW_NIP_OA_AUTH")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        // Note: intentionally not prefixed with BUZZ_ — this is a relay-identity
        // config that may be shared across multiple services (e.g., ACP agent).
        let relay_owner_pubkey = std::env::var("RELAY_OWNER_PUBKEY")
            .ok()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .and_then(|s| {
                // Must be exactly 64 lowercase hex characters (32-byte pubkey).
                let valid = s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit());
                if valid {
                    Some(s)
                } else {
                    warn!(
                        "RELAY_OWNER_PUBKEY is not a valid 64-char hex pubkey — ignoring. \
                         Got: {s:?}"
                    );
                    None
                }
            });

        // Note: intentionally not prefixed with BUZZ_ — same relay-identity
        // config family as RELAY_OWNER_PUBKEY. Comma-separated 64-char hex
        // pubkeys. Unlike RELAY_OWNER_PUBKEY (warn-and-ignore), an invalid
        // entry here is a hard config error: silently dropping an operator
        // pubkey would silently disable provisioning for that operator.
        let relay_operator_api_origin = std::env::var("RELAY_OPERATOR_API_ORIGIN")
            .ok()
            .filter(|raw| !raw.trim().is_empty())
            .map(|raw| parse_operator_api_origin(&raw))
            .transpose()?;

        let relay_operator_pubkeys = match std::env::var("RELAY_OPERATOR_PUBKEYS") {
            Ok(raw) => {
                let mut pubkeys = Vec::new();
                for entry in raw.split(',') {
                    let entry = entry.trim().to_lowercase();
                    if entry.is_empty() {
                        continue;
                    }
                    let valid = entry.len() == 64 && entry.chars().all(|c| c.is_ascii_hexdigit());
                    if !valid {
                        return Err(ConfigError::InvalidValue(format!(
                            "RELAY_OPERATOR_PUBKEYS entry is not a valid 64-char hex pubkey: {entry:?}"
                        )));
                    }
                    if !pubkeys.contains(&entry) {
                        pubkeys.push(entry);
                    }
                }
                pubkeys
            }
            Err(_) => Vec::new(),
        };
        if !relay_operator_pubkeys.is_empty() && relay_operator_api_origin.is_none() {
            return Err(ConfigError::InvalidValue(
                "RELAY_OPERATOR_API_ORIGIN is required when RELAY_OPERATOR_PUBKEYS is configured"
                    .to_string(),
            ));
        }

        let auth = buzz_auth::AuthConfig {
            rate_limits: rate_limit_config_from_env()?,
        };

        if !require_auth_token {
            warn!(
                "BUZZ_REQUIRE_AUTH_TOKEN is false — REST API requests bypass token auth. \
                 WebSocket protocol auth is unaffected. Set to true for production."
            );
        }

        let cors_origins = std::env::var("BUZZ_CORS_ORIGINS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let relay_private_key = std::env::var("BUZZ_RELAY_PRIVATE_KEY").ok();

        let uds_path = std::env::var("BUZZ_UDS_PATH")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let health_port = std::env::var("BUZZ_HEALTH_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8080);

        let metrics_port = std::env::var("BUZZ_METRICS_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(9102);

        let media = buzz_media::MediaConfig {
            s3_endpoint: std::env::var("BUZZ_S3_ENDPOINT")
                .unwrap_or_else(|_| "http://localhost:9000".to_string()),
            s3_access_key: std::env::var("BUZZ_S3_ACCESS_KEY")
                .unwrap_or_else(|_| "buzz_dev".to_string()),
            s3_secret_key: std::env::var("BUZZ_S3_SECRET_KEY")
                .unwrap_or_else(|_| "buzz_dev_secret".to_string()),
            s3_bucket: std::env::var("BUZZ_S3_BUCKET").unwrap_or_else(|_| "buzz-media".to_string()),
            s3_region: std::env::var("BUZZ_S3_REGION")
                .or_else(|_| std::env::var("AWS_REGION"))
                .unwrap_or_else(|_| "us-east-1".to_string()),
            max_image_bytes: std::env::var("BUZZ_MAX_IMAGE_BYTES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(50 * 1024 * 1024),
            max_gif_bytes: std::env::var("BUZZ_MAX_GIF_BYTES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(10 * 1024 * 1024),
            max_video_bytes: std::env::var("BUZZ_MAX_VIDEO_BYTES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(500 * 1024 * 1024),
            max_file_bytes: std::env::var("BUZZ_MAX_FILE_BYTES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(100 * 1024 * 1024),
            public_base_url: std::env::var("BUZZ_MEDIA_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:3000/media".to_string()),
            // Per-upload-event records (`_uploads/` moderation side channel).
            // Off by default; coherence between the three knobs is enforced in
            // MediaConfig::validate at startup.
            upload_records_enabled: std::env::var("BUZZ_MEDIA_UPLOAD_RECORDS")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
            upload_ip_header: std::env::var("BUZZ_MEDIA_UPLOAD_IP_HEADER")
                .ok()
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty()),
            upload_port_header: std::env::var("BUZZ_MEDIA_UPLOAD_PORT_HEADER")
                .ok()
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty()),
        };
        let media_max_concurrent_uploads: usize =
            std::env::var("BUZZ_MEDIA_MAX_CONCURRENT_UPLOADS")
                .ok()
                .and_then(|v| v.parse().ok())
                .filter(|&v| v > 0)
                .unwrap_or(8);
        let media_max_concurrent_uploads_per_pubkey: u32 =
            std::env::var("BUZZ_MEDIA_MAX_CONCURRENT_UPLOADS_PER_PUBKEY")
                .ok()
                .and_then(|v| v.parse().ok())
                .filter(|&v| v > 0)
                .unwrap_or(2)
                .min(u32::try_from(media_max_concurrent_uploads).unwrap_or(u32::MAX));
        let media_uploads_per_minute: u32 = std::env::var("BUZZ_MEDIA_UPLOADS_PER_MINUTE")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&v| v > 0)
            .unwrap_or(30);

        let require_media_get_auth = std::env::var("BUZZ_REQUIRE_MEDIA_GET_AUTH")
            .map(|v| {
                v == "true"
                    || v == "1"
                    || v.eq_ignore_ascii_case("yes")
                    || v.eq_ignore_ascii_case("on")
            })
            .unwrap_or(false);

        let ephemeral_ttl_override = std::env::var("BUZZ_EPHEMERAL_TTL_OVERRIDE")
            .ok()
            .and_then(|v| v.parse::<i32>().ok())
            .filter(|&v| v > 0);

        if let Some(ttl) = ephemeral_ttl_override {
            warn!(
                "BUZZ_EPHEMERAL_TTL_OVERRIDE={ttl}s — all ephemeral channels will use \
                 this TTL instead of the client-provided value."
            );
        }

        // Git server config
        let git_repo_path = ensure_git_repo_path(
            std::env::var("BUZZ_GIT_REPO_PATH").unwrap_or_else(|_| "./repos".to_string()),
        )?;
        let git_pack_cache_path = ensure_git_path(
            "BUZZ_GIT_PACK_CACHE_PATH",
            std::env::var("BUZZ_GIT_PACK_CACHE_PATH")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| git_repo_path.join(".pack-cache")),
        )?;
        let git_max_pack_bytes: u64 = std::env::var("BUZZ_GIT_MAX_PACK_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(500 * 1024 * 1024); // 500 MB
        let git_max_repo_bytes: u64 = std::env::var("BUZZ_GIT_MAX_REPO_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| git_max_pack_bytes.saturating_mul(2)); // 1 GB at defaults
        let git_pack_cache_max_bytes: u64 = std::env::var("BUZZ_GIT_PACK_CACHE_MAX_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| git_max_repo_bytes.saturating_mul(5)); // 5 GB at defaults
        let git_pack_cache_max_concurrent_populations: usize =
            std::env::var("BUZZ_GIT_PACK_CACHE_MAX_CONCURRENT_POPULATIONS")
                .ok()
                .and_then(|v| v.parse().ok())
                .filter(|value| *value > 0)
                .unwrap_or(2);
        let git_max_repos_per_pubkey: u32 = std::env::var("BUZZ_GIT_MAX_REPOS_PER_PUBKEY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100);
        let git_max_concurrent_ops: usize = std::env::var("BUZZ_GIT_MAX_CONCURRENT_OPS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20);
        let git_hook_hmac_secret: String = std::env::var("BUZZ_GIT_HOOK_HMAC_SECRET")
            .unwrap_or_else(|_| {
                // Generate a random secret if not configured (dev mode).
                let secret: [u8; 32] = rand::random();
                hex::encode(secret)
            });
        const MAX_POLICY_MARKDOWN_BYTES: usize = 256 * 1024;
        let read_policy_markdown = |name: &str| -> Result<Option<String>, ConfigError> {
            let value = std::env::var(name)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            if value
                .as_ref()
                .is_some_and(|value| value.len() > MAX_POLICY_MARKDOWN_BYTES)
            {
                return Err(ConfigError::InvalidValue(format!(
                    "{name} must contain at most {MAX_POLICY_MARKDOWN_BYTES} bytes"
                )));
            }
            Ok(value)
        };
        let terms_markdown = read_policy_markdown("BUZZ_TERMS_OF_SERVICE_MARKDOWN")?;
        let privacy_markdown = read_policy_markdown("BUZZ_PRIVACY_POLICY_MARKDOWN")?;
        let age_attestation_required = parse_optional_bool("BUZZ_AGE_ATTESTATION_REQUIRED")?;
        let audit_enabled = parse_bool("BUZZ_AUDIT_ENABLED", true)?;
        let runtime_unrecoverable_enabled = parse_bool("BUZZ_RUNTIME_UNRECOVERABLE", false)?;
        let runtime_supervision_interval_secs =
            positive_u64_from_env("BUZZ_RUNTIME_SUPERVISION_INTERVAL_SECS", 5)?;
        if runtime_supervision_interval_secs > 60 {
            return Err(ConfigError::InvalidValue(
                "BUZZ_RUNTIME_SUPERVISION_INTERVAL_SECS must be in 1..=60".to_owned(),
            ));
        }
        let runtime_supervision_batch_limit =
            positive_u64_from_env("BUZZ_RUNTIME_SUPERVISION_BATCH_LIMIT", 25)?;
        let runtime_supervision_batch_limit = u16::try_from(runtime_supervision_batch_limit)
            .ok()
            .filter(|value| *value <= 1_000)
            .ok_or_else(|| {
                ConfigError::InvalidValue(
                    "BUZZ_RUNTIME_SUPERVISION_BATCH_LIMIT must be in 1..=1000".to_owned(),
                )
            })?;
        let runtime_supervision_claim_secs =
            positive_u64_from_env("BUZZ_RUNTIME_SUPERVISION_CLAIM_SECS", 60)?;
        if !(10..=300).contains(&runtime_supervision_claim_secs)
            || runtime_supervision_claim_secs < runtime_supervision_interval_secs.saturating_mul(2)
        {
            return Err(ConfigError::InvalidValue(
                "BUZZ_RUNTIME_SUPERVISION_CLAIM_SECS must be in 10..=300 and at least twice the scheduler interval"
                    .to_owned(),
            ));
        }
        if runtime_unrecoverable_enabled && !audit_enabled {
            return Err(ConfigError::InvalidValue(
                "BUZZ_RUNTIME_UNRECOVERABLE requires BUZZ_AUDIT_ENABLED=true".to_owned(),
            ));
        }
        if runtime_unrecoverable_enabled && relay_private_key.is_none() {
            return Err(ConfigError::InvalidValue(
                "BUZZ_RUNTIME_UNRECOVERABLE requires a stable BUZZ_RELAY_PRIVATE_KEY".to_owned(),
            ));
        }
        let semantic_worker_enabled = parse_bool("BUZZ_SEMANTIC_WORKER_ENABLED", false)?;
        let semantic_graph_query_http_available =
            parse_bool("BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE", false)?;
        let semantic_graph_query_max_in_flight = usize::try_from(positive_u64_from_env(
            "BUZZ_SEMANTIC_GRAPH_QUERY_MAX_IN_FLIGHT",
            8,
        )?)
        .ok()
        .filter(|value| *value <= 64)
        .ok_or_else(|| {
            ConfigError::InvalidValue(
                "BUZZ_SEMANTIC_GRAPH_QUERY_MAX_IN_FLIGHT must be in 1..=64".to_owned(),
            )
        })?;
        let semantic_graph_traversal_max_in_flight = usize::try_from(positive_u64_from_env(
            "BUZZ_SEMANTIC_GRAPH_TRAVERSAL_MAX_IN_FLIGHT",
            2,
        )?)
        .map_err(|_| {
            ConfigError::InvalidValue(
                "BUZZ_SEMANTIC_GRAPH_TRAVERSAL_MAX_IN_FLIGHT exceeds usize".to_owned(),
            )
        })?;
        validate_semantic_traversal_capacity(
            semantic_graph_query_max_in_flight,
            semantic_graph_traversal_max_in_flight,
            db_main_pool.max_connections,
            db_ordinary_main_reserve,
        )?;
        let semantic_graph_query_fleet_policy = parse_semantic_query_fleet_policy()?;
        let semantic_graph_query_deployment_id =
            parse_semantic_query_identity("BUZZ_SEMANTIC_GRAPH_QUERY_DEPLOYMENT_ID")?;
        let semantic_graph_query_instance_id =
            parse_semantic_query_identity("BUZZ_SEMANTIC_GRAPH_QUERY_INSTANCE_ID")?;
        require_semantic_query_fleet_identities(
            semantic_graph_query_http_available,
            semantic_graph_query_fleet_policy,
            semantic_graph_query_deployment_id.as_deref(),
            semantic_graph_query_instance_id.as_deref(),
        )?;
        let semantic_api_key = std::env::var("BUZZ_SEMANTIC_API_KEY")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let semantic_base_url = std::env::var("BUZZ_SEMANTIC_BASE_URL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(|value| {
                let mut parsed = url::Url::parse(&value).map_err(|_| {
                    ConfigError::InvalidValue(
                        "BUZZ_SEMANTIC_BASE_URL must be an absolute HTTPS URL".to_string(),
                    )
                })?;
                if parsed.scheme() != "https"
                    || parsed.host_str().is_none()
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                    || parsed.query().is_some()
                    || parsed.fragment().is_some()
                {
                    return Err(ConfigError::InvalidValue(
                        "BUZZ_SEMANTIC_BASE_URL must be an HTTPS origin/path without credentials, query, or fragment"
                            .to_string(),
                    ));
                }
                if !parsed.path().ends_with('/') {
                    let path = format!("{}/", parsed.path());
                    parsed.set_path(&path);
                }
                Ok(parsed)
            })
            .transpose()?;
        let semantic_request_model = std::env::var("BUZZ_SEMANTIC_REQUEST_MODEL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let semantic_request_timeout_secs =
            positive_u64_from_env("BUZZ_SEMANTIC_REQUEST_TIMEOUT_SECS", 30)?;
        if !(1..=120).contains(&semantic_request_timeout_secs) {
            return Err(ConfigError::InvalidValue(
                "BUZZ_SEMANTIC_REQUEST_TIMEOUT_SECS must be in 1..=120".to_string(),
            ));
        }
        let semantic_request_interval_millis =
            positive_u64_from_env("BUZZ_SEMANTIC_REQUEST_INTERVAL_MS", 1_000)?;
        if !(100..=60_000).contains(&semantic_request_interval_millis) {
            return Err(ConfigError::InvalidValue(
                "BUZZ_SEMANTIC_REQUEST_INTERVAL_MS must be in 100..=60000".to_string(),
            ));
        }
        let semantic_claim_seconds = positive_u64_from_env("BUZZ_SEMANTIC_CLAIM_SECS", 60)?;
        let semantic_claim_seconds = u16::try_from(semantic_claim_seconds)
            .ok()
            .filter(|value| (10..=300).contains(value))
            .ok_or_else(|| {
                ConfigError::InvalidValue(
                    "BUZZ_SEMANTIC_CLAIM_SECS must be in 10..=300".to_string(),
                )
            })?;
        let semantic_max_attempts = positive_u64_from_env("BUZZ_SEMANTIC_MAX_ATTEMPTS", 8)?;
        let semantic_max_attempts = u32::try_from(semantic_max_attempts)
            .ok()
            .filter(|value| (1..=100).contains(value))
            .ok_or_else(|| {
                ConfigError::InvalidValue(
                    "BUZZ_SEMANTIC_MAX_ATTEMPTS must be in 1..=100".to_string(),
                )
            })?;
        if semantic_worker_enabled
            && u64::from(semantic_claim_seconds) <= semantic_request_timeout_secs
        {
            return Err(ConfigError::InvalidValue(
                "BUZZ_SEMANTIC_CLAIM_SECS must exceed BUZZ_SEMANTIC_REQUEST_TIMEOUT_SECS"
                    .to_string(),
            ));
        }
        if (semantic_worker_enabled || semantic_graph_query_http_available)
            && (semantic_api_key.is_none()
                || semantic_base_url.is_none()
                || semantic_request_model.is_none())
        {
            return Err(ConfigError::InvalidValue(
                "semantic worker/query runtime requires provider key, base URL, and request model"
                    .to_string(),
            ));
        }
        let semantic_worker = SemanticWorkerConfig {
            enabled: semantic_worker_enabled,
            api_key: semantic_api_key,
            base_url: semantic_base_url,
            request_model: semantic_request_model,
            request_timeout: Duration::from_secs(semantic_request_timeout_secs),
            request_interval: Duration::from_millis(semantic_request_interval_millis),
            claim_seconds: semantic_claim_seconds,
            max_attempts: semantic_max_attempts,
        };
        let join_policy = if terms_markdown.is_none()
            && privacy_markdown.is_none()
            && !age_attestation_required
        {
            None
        } else {
            let mut hasher = Sha256::new();
            hasher.update(terms_markdown.as_deref().unwrap_or_default().as_bytes());
            hasher.update([0]);
            hasher.update(privacy_markdown.as_deref().unwrap_or_default().as_bytes());
            hasher.update([0, u8::from(age_attestation_required)]);
            Some(JoinPolicyConfig {
                terms_markdown,
                privacy_markdown,
                age_attestation_required,
                version: hex::encode(hasher.finalize()),
            })
        };

        // Read-only deployment-admin surface. The route is absent when the host is unset.
        let admin = match std::env::var("BUZZ_ADMIN_HOST")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
        {
            None => None,
            Some(host) => {
                if host.contains(['/', '\\', '@']) {
                    return Err(ConfigError::InvalidValue(
                        "BUZZ_ADMIN_HOST must be an exact authority".to_string(),
                    ));
                }
                let web_dir = std::env::var("BUZZ_ADMIN_WEB_DIR")
                    .ok()
                    .map(|value| std::path::PathBuf::from(value.trim()))
                    .filter(|value| !value.as_os_str().is_empty());
                if let Some(ref dir) = web_dir {
                    if !dir.join("index.html").is_file() {
                        return Err(ConfigError::InvalidValue(format!(
                            "BUZZ_ADMIN_WEB_DIR={} does not contain index.html",
                            dir.display()
                        )));
                    }
                }
                Some(AdminConfig { host, web_dir })
            }
        };

        // Web UI static file serving
        let web_dir = std::env::var("BUZZ_WEB_DIR")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(std::path::PathBuf::from);
        let serve_git_web_gui = std::env::var("BUZZ_SERVE_GIT_WEB_GUI")
            .map(|value| value == "true" || value == "1")
            .unwrap_or(false);

        if let Some(ref dir) = web_dir {
            if !dir.join("index.html").is_file() {
                return Err(ConfigError::InvalidValue(format!(
                    "BUZZ_WEB_DIR={} does not contain index.html",
                    dir.display()
                )));
            }
            tracing::info!("BUZZ_WEB_DIR={} — serving web UI from relay", dir.display());
        }

        // Reject explicitly-configured secrets that are too short.
        // The auto-generated fallback is always 64 hex chars (32 bytes), so this
        // only fires when someone sets BUZZ_GIT_HOOK_HMAC_SECRET to a weak value.
        if std::env::var("BUZZ_GIT_HOOK_HMAC_SECRET").is_ok() && git_hook_hmac_secret.len() < 32 {
            return Err(ConfigError::InvalidValue(
                "BUZZ_GIT_HOOK_HMAC_SECRET must be at least 32 characters (16 bytes hex)"
                    .to_string(),
            ));
        }

        Ok(Self {
            bind_addr,
            database_url,
            read_database_url,
            db_main_pool,
            db_read_pool,
            db_control_pool,
            db_audit_pool,
            db_search_pool,
            db_ordinary_main_reserve,
            db_server_connection_reserve,
            redis_url,
            redis_pool_size,
            relay_url,
            pairing_relay_url,
            max_connections,
            max_concurrent_handlers,
            send_buffer_size,
            max_frame_bytes,
            slow_client_grace_limit,
            auth,
            require_auth_token,
            cors_origins,
            relay_private_key,
            uds_path,
            health_port,
            metrics_port,
            pubkey_allowlist_enabled,
            require_relay_membership,
            huddle_audio_available,
            meeting_v1_create_enabled,
            meeting_v2_create_enabled,
            meeting_v2_direct_actions_create_enabled,
            meeting_community_read_enabled,
            mesh,
            mesh_demo_echo,
            relay_owner_pubkey,
            relay_operator_api_origin,
            relay_operator_pubkeys,
            allow_nip_oa_auth,
            media,
            media_max_concurrent_uploads,
            media_max_concurrent_uploads_per_pubkey,
            media_uploads_per_minute,
            require_media_get_auth,
            audit_enabled,
            runtime_unrecoverable_enabled,
            runtime_supervision_interval_secs,
            runtime_supervision_batch_limit,
            runtime_supervision_claim_secs,
            semantic_worker,
            semantic_graph_query_http_available,
            semantic_graph_query_max_in_flight,
            semantic_graph_traversal_max_in_flight,
            semantic_graph_query_fleet_policy,
            semantic_graph_query_deployment_id,
            semantic_graph_query_instance_id,
            ephemeral_ttl_override,
            git_repo_path,
            git_pack_cache_path,
            git_max_pack_bytes,
            git_max_repo_bytes,
            git_pack_cache_max_bytes,
            git_pack_cache_max_concurrent_populations,
            git_max_repos_per_pubkey,
            git_max_concurrent_ops,
            git_hook_hmac_secret,
            join_policy,
            admin,
            web_dir,
            serve_git_web_gui,
        })
    }

    /// Return the validated routing trust used by semantic query DB fences.
    ///
    /// An enabled strict deployment is rejected by [`Config::from_env`] when
    /// either identity is absent. Returning an error here keeps manually
    /// constructed disabled configs fail-closed instead of treating a missing
    /// identity as an implicit single-Relay bypass.
    pub(crate) fn semantic_graph_query_routing_trust(
        &self,
    ) -> Result<SemanticGraphQueryRoutingTrust<'_>, ConfigError> {
        match self.semantic_graph_query_fleet_policy {
            SemanticGraphQueryFleetPolicy::TrustedSingleRelay => {
                Ok(SemanticGraphQueryRoutingTrust::TrustedSingleRelay)
            }
            SemanticGraphQueryFleetPolicy::AttestedFleet => {
                let invalid = || {
                    ConfigError::InvalidValue(
                        "attested semantic graph query routing requires deployment and instance identities"
                            .to_owned(),
                    )
                };
                let deployment_id = self
                    .semantic_graph_query_deployment_id
                    .as_deref()
                    .ok_or_else(invalid)?;
                let instance_id = self
                    .semantic_graph_query_instance_id
                    .as_deref()
                    .ok_or_else(invalid)?;
                Ok(SemanticGraphQueryRoutingTrust::AttestedFleet {
                    deployment_id,
                    instance_id,
                })
            }
        }
    }

    /// Conservative connection ceiling charged to the writer endpoint.
    ///
    /// Until a deployment provides verified endpoint grouping, an optional
    /// read pool is included here so host aliases cannot undercount capacity.
    pub fn conservative_postgres_connection_budget(&self) -> u32 {
        self.db_main_pool
            .max_connections
            .saturating_add(self.db_control_pool.max_connections)
            .saturating_add(
                self.read_database_url
                    .as_ref()
                    .map_or(0, |_| self.db_read_pool.max_connections),
            )
            .saturating_add(if self.audit_enabled {
                self.db_audit_pool.max_connections
            } else {
                0
            })
            .saturating_add(self.db_search_pool.max_connections)
            .saturating_add(self.db_server_connection_reserve)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mutex to serialize tests that mutate environment variables.
    // Parallel env-var mutation causes `defaults_are_valid` to see the invalid
    // value set by `invalid_bind_addr_returns_error`, causing a flaky failure.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn semantic_query_fleet_policy_is_closed_and_defaults_to_single_relay() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let previous = std::env::var_os("BUZZ_SEMANTIC_GRAPH_QUERY_FLEET_POLICY");

        std::env::remove_var("BUZZ_SEMANTIC_GRAPH_QUERY_FLEET_POLICY");
        assert_eq!(
            parse_semantic_query_fleet_policy().expect("default policy"),
            SemanticGraphQueryFleetPolicy::TrustedSingleRelay
        );
        std::env::set_var("BUZZ_SEMANTIC_GRAPH_QUERY_FLEET_POLICY", "attested-fleet");
        assert_eq!(
            parse_semantic_query_fleet_policy().expect("strict policy"),
            SemanticGraphQueryFleetPolicy::AttestedFleet
        );
        std::env::set_var("BUZZ_SEMANTIC_GRAPH_QUERY_FLEET_POLICY", "automatic");
        assert!(parse_semantic_query_fleet_policy().is_err());

        if let Some(value) = previous {
            std::env::set_var("BUZZ_SEMANTIC_GRAPH_QUERY_FLEET_POLICY", value);
        } else {
            std::env::remove_var("BUZZ_SEMANTIC_GRAPH_QUERY_FLEET_POLICY");
        }
    }

    #[test]
    fn semantic_query_fleet_identities_use_closed_grammar() {
        assert_eq!(
            validate_semantic_query_identity("TEST", Some(" prod/relay-0 ".to_owned()))
                .expect("identity"),
            Some("prod/relay-0".to_owned())
        );
        assert!(validate_semantic_query_identity("TEST", Some("relay 0".to_owned())).is_err());
        assert!(validate_semantic_query_identity("TEST", Some("中".to_owned())).is_err());
        assert_eq!(
            validate_semantic_query_identity("TEST", Some("  ".to_owned())).expect("blank"),
            None
        );
        assert!(require_semantic_query_fleet_identities(
            true,
            SemanticGraphQueryFleetPolicy::AttestedFleet,
            None,
            Some("relay-0")
        )
        .is_err());
        assert!(require_semantic_query_fleet_identities(
            true,
            SemanticGraphQueryFleetPolicy::AttestedFleet,
            Some("deployment-a"),
            None
        )
        .is_err());
        assert!(require_semantic_query_fleet_identities(
            true,
            SemanticGraphQueryFleetPolicy::AttestedFleet,
            Some("deployment-a"),
            Some("relay-0")
        )
        .is_ok());
        assert!(require_semantic_query_fleet_identities(
            true,
            SemanticGraphQueryFleetPolicy::TrustedSingleRelay,
            None,
            None
        )
        .is_ok());
        assert!(require_semantic_query_fleet_identities(
            false,
            SemanticGraphQueryFleetPolicy::AttestedFleet,
            None,
            None
        )
        .is_ok());
    }

    #[test]
    fn routing_trust_never_treats_missing_strict_identity_as_local() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let mut config = Config::from_env().expect("default config");
        config.semantic_graph_query_fleet_policy =
            SemanticGraphQueryFleetPolicy::TrustedSingleRelay;
        config.semantic_graph_query_deployment_id = None;
        config.semantic_graph_query_instance_id = None;
        assert_eq!(
            config
                .semantic_graph_query_routing_trust()
                .expect("single Relay trust"),
            SemanticGraphQueryRoutingTrust::TrustedSingleRelay
        );

        config.semantic_graph_query_fleet_policy = SemanticGraphQueryFleetPolicy::AttestedFleet;
        assert!(config.semantic_graph_query_routing_trust().is_err());
        config.semantic_graph_query_deployment_id = Some("deployment-a".to_owned());
        config.semantic_graph_query_instance_id = Some("relay-0".to_owned());
        assert_eq!(
            config
                .semantic_graph_query_routing_trust()
                .expect("attested Fleet trust"),
            SemanticGraphQueryRoutingTrust::AttestedFleet {
                deployment_id: "deployment-a",
                instance_id: "relay-0",
            }
        );
    }

    #[test]
    fn database_pool_and_traversal_capacity_contracts_are_closed() {
        let defaults = DatabasePoolConfig {
            max_connections: 12,
            min_connections: 2,
            acquire_timeout_secs: 3,
            max_lifetime_secs: 1_800,
            idle_timeout_secs: 600,
        };
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::set_var("TEST_DB_POOL_MAX_CONNECTIONS", "2");
        std::env::set_var("TEST_DB_POOL_MIN_CONNECTIONS", "3");
        let invalid = database_pool_config_from_env("TEST_DB_POOL", defaults);
        std::env::remove_var("TEST_DB_POOL_MAX_CONNECTIONS");
        std::env::remove_var("TEST_DB_POOL_MIN_CONNECTIONS");
        assert!(invalid.is_err());

        assert!(validate_semantic_traversal_capacity(8, 2, 12, 4).is_ok());
        assert!(validate_semantic_traversal_capacity(8, 0, 12, 4).is_err());
        assert!(validate_semantic_traversal_capacity(8, 9, 12, 4).is_err());
        assert!(validate_semantic_traversal_capacity(8, 2, 4, 4).is_err());
        assert!(validate_semantic_traversal_capacity(8, 5, 8, 4).is_err());
    }

    #[test]
    fn conservative_pool_budget_never_undercounts_an_unverified_read_endpoint() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let mut config = Config::from_env().expect("default config");
        config.read_database_url = None;
        let without_read = config.conservative_postgres_connection_budget();
        config.read_database_url = Some("postgres://reader.invalid/carryforth".to_owned());
        assert_eq!(
            config.conservative_postgres_connection_budget(),
            without_read + config.db_read_pool.max_connections
        );
    }

    #[test]
    fn defaults_are_valid() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let config = Config::from_env().expect("default config");
        assert!(config.bind_addr.port() > 0);
        assert!(!config.database_url.is_empty());
        assert!(!config.redis_url.is_empty());
        assert_eq!(config.redis_pool_size, 16);
        assert!(config.max_connections > 0);
        assert!(config.send_buffer_size > 0);
        assert_eq!(config.max_frame_bytes, DEFAULT_MAX_FRAME_BYTES);
        assert!(config.slow_client_grace_limit > 0);
        assert!(
            !config.pubkey_allowlist_enabled,
            "pubkey_allowlist_enabled should default to false"
        );
        assert!(
            !config.require_relay_membership,
            "require_relay_membership should default to false"
        );
        assert!(
            config.relay_owner_pubkey.is_none(),
            "relay_owner_pubkey should default to None"
        );
        assert!(
            config.relay_operator_pubkeys.is_empty(),
            "relay_operator_pubkeys should default empty (provisioning disabled)"
        );
        assert!(
            !config.allow_nip_oa_auth,
            "allow_nip_oa_auth should default to false"
        );
        assert!(
            !config.serve_git_web_gui,
            "serve_git_web_gui should default to false"
        );
        assert!(
            !config.require_media_get_auth,
            "require_media_get_auth should default to false for staged client rollout"
        );
        assert!(
            config.join_policy.is_none(),
            "join_policy should default to None so policy prompts and acceptance receipts are opt-in"
        );
        assert!(
            config.huddle_audio_available,
            "huddle_audio_available should default to true so single-pod (N=1) keeps today's huddle behavior"
        );
        assert!(
            !config.meeting_v1_create_enabled,
            "Meeting V1 creation must remain opt-in during the backend rollout"
        );
        assert!(
            !config.meeting_v2_create_enabled,
            "Meeting V2 creation must remain opt-in during the stage-one rollout"
        );
        assert!(
            !config.meeting_v2_direct_actions_create_enabled,
            "action-capable Meeting V2 creation must remain separately opt-in"
        );
        assert!(
            !config.meeting_community_read_enabled,
            "Community-wide Meeting reads must remain dark until migration approval"
        );
        assert!(
            !config.runtime_unrecoverable_enabled,
            "automatic runtime unrecoverable must default fail-closed"
        );
        assert_eq!(config.runtime_supervision_interval_secs, 5);
        assert_eq!(config.runtime_supervision_batch_limit, 25);
        assert_eq!(config.runtime_supervision_claim_secs, 60);
    }

    #[test]
    fn meeting_v1_create_gate_is_strict_and_defaults_off() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let previous = std::env::var_os("BUZZ_MEETING_V1_CREATE_ENABLED");

        std::env::remove_var("BUZZ_MEETING_V1_CREATE_ENABLED");
        assert!(
            !Config::from_env()
                .expect("unset gate is valid")
                .meeting_v1_create_enabled
        );

        std::env::set_var("BUZZ_MEETING_V1_CREATE_ENABLED", "true");
        assert!(
            Config::from_env()
                .expect("enabled gate is valid")
                .meeting_v1_create_enabled
        );

        std::env::set_var("BUZZ_MEETING_V1_CREATE_ENABLED", "sometimes");
        let invalid = Config::from_env();

        if let Some(value) = previous {
            std::env::set_var("BUZZ_MEETING_V1_CREATE_ENABLED", value);
        } else {
            std::env::remove_var("BUZZ_MEETING_V1_CREATE_ENABLED");
        }

        assert!(matches!(
            invalid,
            Err(ConfigError::InvalidValue(ref message))
                if message.contains("BUZZ_MEETING_V1_CREATE_ENABLED")
        ));
    }

    #[test]
    fn meeting_v2_create_gate_is_strict_and_defaults_off() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let previous = std::env::var_os("BUZZ_MEETING_V2_CREATE_ENABLED");

        std::env::remove_var("BUZZ_MEETING_V2_CREATE_ENABLED");
        assert!(
            !Config::from_env()
                .expect("unset gate is valid")
                .meeting_v2_create_enabled
        );

        std::env::set_var("BUZZ_MEETING_V2_CREATE_ENABLED", "true");
        assert!(
            Config::from_env()
                .expect("enabled gate is valid")
                .meeting_v2_create_enabled
        );

        std::env::set_var("BUZZ_MEETING_V2_CREATE_ENABLED", "sometimes");
        let invalid = Config::from_env();

        if let Some(value) = previous {
            std::env::set_var("BUZZ_MEETING_V2_CREATE_ENABLED", value);
        } else {
            std::env::remove_var("BUZZ_MEETING_V2_CREATE_ENABLED");
        }

        assert!(matches!(
            invalid,
            Err(ConfigError::InvalidValue(ref message))
                if message.contains("BUZZ_MEETING_V2_CREATE_ENABLED")
        ));
    }

    #[test]
    fn meeting_v2_direct_actions_create_gate_is_strict_and_defaults_off() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let previous = std::env::var_os("BUZZ_MEETING_V2_DIRECT_ACTIONS_CREATE_ENABLED");
        let legacy_previous = std::env::var_os("BUZZ_MEETING_V2_ACTIONS_CREATE_ENABLED");

        std::env::remove_var("BUZZ_MEETING_V2_DIRECT_ACTIONS_CREATE_ENABLED");
        std::env::set_var("BUZZ_MEETING_V2_ACTIONS_CREATE_ENABLED", "true");
        assert!(
            !Config::from_env()
                .expect("unset direct action gate is valid")
                .meeting_v2_direct_actions_create_enabled
        );

        std::env::set_var("BUZZ_MEETING_V2_DIRECT_ACTIONS_CREATE_ENABLED", "true");
        assert!(
            Config::from_env()
                .expect("enabled action gate is valid")
                .meeting_v2_direct_actions_create_enabled
        );

        std::env::set_var("BUZZ_MEETING_V2_DIRECT_ACTIONS_CREATE_ENABLED", "sometimes");
        let invalid = Config::from_env();

        if let Some(value) = previous {
            std::env::set_var("BUZZ_MEETING_V2_DIRECT_ACTIONS_CREATE_ENABLED", value);
        } else {
            std::env::remove_var("BUZZ_MEETING_V2_DIRECT_ACTIONS_CREATE_ENABLED");
        }
        if let Some(value) = legacy_previous {
            std::env::set_var("BUZZ_MEETING_V2_ACTIONS_CREATE_ENABLED", value);
        } else {
            std::env::remove_var("BUZZ_MEETING_V2_ACTIONS_CREATE_ENABLED");
        }

        assert!(matches!(
            invalid,
            Err(ConfigError::InvalidValue(ref message))
                if message.contains("BUZZ_MEETING_V2_DIRECT_ACTIONS_CREATE_ENABLED")
        ));
    }

    #[test]
    fn meeting_community_read_gate_is_strict_and_defaults_off() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let previous = std::env::var_os("BUZZ_MEETING_COMMUNITY_READ_ENABLED");

        std::env::remove_var("BUZZ_MEETING_COMMUNITY_READ_ENABLED");
        assert!(
            !Config::from_env()
                .expect("unset Meeting read gate is valid")
                .meeting_community_read_enabled
        );

        std::env::set_var("BUZZ_MEETING_COMMUNITY_READ_ENABLED", "true");
        assert!(
            Config::from_env()
                .expect("enabled Meeting read gate is valid")
                .meeting_community_read_enabled
        );

        std::env::set_var("BUZZ_MEETING_COMMUNITY_READ_ENABLED", "sometimes");
        let invalid = Config::from_env();

        if let Some(value) = previous {
            std::env::set_var("BUZZ_MEETING_COMMUNITY_READ_ENABLED", value);
        } else {
            std::env::remove_var("BUZZ_MEETING_COMMUNITY_READ_ENABLED");
        }

        assert!(matches!(
            invalid,
            Err(ConfigError::InvalidValue(ref message))
                if message.contains("BUZZ_MEETING_COMMUNITY_READ_ENABLED")
        ));
    }

    #[test]
    fn runtime_automation_requires_audit_and_a_stable_relay_key() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let names = [
            "BUZZ_RUNTIME_UNRECOVERABLE",
            "BUZZ_AUDIT_ENABLED",
            "BUZZ_RELAY_PRIVATE_KEY",
        ];
        let previous = names.map(std::env::var_os);

        std::env::set_var("BUZZ_RUNTIME_UNRECOVERABLE", "true");
        std::env::set_var("BUZZ_AUDIT_ENABLED", "false");
        std::env::set_var("BUZZ_RELAY_PRIVATE_KEY", "11".repeat(32));
        let without_audit = Config::from_env();

        std::env::set_var("BUZZ_AUDIT_ENABLED", "true");
        std::env::remove_var("BUZZ_RELAY_PRIVATE_KEY");
        let without_key = Config::from_env();

        std::env::set_var("BUZZ_RELAY_PRIVATE_KEY", "11".repeat(32));
        let enabled = Config::from_env().expect("fully fenced runtime automation config");

        for (name, value) in names.into_iter().zip(previous) {
            if let Some(value) = value {
                std::env::set_var(name, value);
            } else {
                std::env::remove_var(name);
            }
        }

        assert!(matches!(
            without_audit,
            Err(ConfigError::InvalidValue(ref message))
                if message.contains("BUZZ_AUDIT_ENABLED")
        ));
        assert!(matches!(
            without_key,
            Err(ConfigError::InvalidValue(ref message))
                if message.contains("BUZZ_RELAY_PRIVATE_KEY")
        ));
        assert!(enabled.runtime_unrecoverable_enabled);
    }

    #[test]
    fn redis_pool_size_env_override_and_invalid_fallback() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let previous = std::env::var_os("BUZZ_REDIS_POOL_SIZE");

        std::env::set_var("BUZZ_REDIS_POOL_SIZE", "32");
        let overridden = Config::from_env().expect("config").redis_pool_size;

        std::env::set_var("BUZZ_REDIS_POOL_SIZE", "0");
        let zero = Config::from_env().expect("config").redis_pool_size;

        std::env::set_var("BUZZ_REDIS_POOL_SIZE", "not-a-number");
        let junk = Config::from_env().expect("config").redis_pool_size;

        if let Some(value) = previous {
            std::env::set_var("BUZZ_REDIS_POOL_SIZE", value);
        } else {
            std::env::remove_var("BUZZ_REDIS_POOL_SIZE");
        }

        assert_eq!(overridden, 32);
        assert_eq!(zero, 16, "zero must fall back to the default");
        assert_eq!(junk, 16, "unparsable value must fall back to the default");
    }

    #[test]
    fn read_database_url_unset_or_blank_is_none() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let previous = std::env::var_os("READ_DATABASE_URL");

        std::env::remove_var("READ_DATABASE_URL");
        let unset = Config::from_env().expect("config").read_database_url;

        std::env::set_var("READ_DATABASE_URL", "   ");
        let blank = Config::from_env().expect("config").read_database_url;

        std::env::set_var("READ_DATABASE_URL", "postgres://buzz:pw@replica:5432/buzz"); // sadscan:disable np.postgres.1
        let set = Config::from_env().expect("config").read_database_url;

        if let Some(value) = previous {
            std::env::set_var("READ_DATABASE_URL", value);
        } else {
            std::env::remove_var("READ_DATABASE_URL");
        }

        assert_eq!(unset, None, "unset READ_DATABASE_URL must disable routing");
        assert_eq!(blank, None, "blank READ_DATABASE_URL must disable routing");
        assert_eq!(
            set.as_deref(),
            Some("postgres://buzz:pw@replica:5432/buzz") // sadscan:disable np.postgres.1
        );
    }

    #[test]
    fn audit_logging_defaults_on_and_accepts_explicit_off() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let previous = std::env::var_os("BUZZ_AUDIT_ENABLED");
        std::env::remove_var("BUZZ_AUDIT_ENABLED");
        assert!(parse_bool("BUZZ_AUDIT_ENABLED", true).unwrap());
        std::env::set_var("BUZZ_AUDIT_ENABLED", "false");
        assert!(!parse_bool("BUZZ_AUDIT_ENABLED", true).unwrap());
        if let Some(value) = previous {
            std::env::set_var("BUZZ_AUDIT_ENABLED", value);
        } else {
            std::env::remove_var("BUZZ_AUDIT_ENABLED");
        }
    }

    #[test]
    fn audit_logging_rejects_invalid_boolean() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let previous = std::env::var_os("BUZZ_AUDIT_ENABLED");
        std::env::set_var("BUZZ_AUDIT_ENABLED", "sometimes");
        let result = parse_bool("BUZZ_AUDIT_ENABLED", true);
        if let Some(value) = previous {
            std::env::set_var("BUZZ_AUDIT_ENABLED", value);
        } else {
            std::env::remove_var("BUZZ_AUDIT_ENABLED");
        }
        assert!(matches!(
            result,
            Err(ConfigError::InvalidValue(ref message))
                if message.contains("BUZZ_AUDIT_ENABLED")
        ));
    }

    #[test]
    fn join_policy_age_attestation_rejects_invalid_boolean() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let previous = std::env::var_os("BUZZ_AGE_ATTESTATION_REQUIRED");
        std::env::set_var("BUZZ_AGE_ATTESTATION_REQUIRED", "sometimes");
        let result = parse_optional_bool("BUZZ_AGE_ATTESTATION_REQUIRED");
        if let Some(value) = previous {
            std::env::set_var("BUZZ_AGE_ATTESTATION_REQUIRED", value);
        } else {
            std::env::remove_var("BUZZ_AGE_ATTESTATION_REQUIRED");
        }
        assert!(matches!(
            result,
            Err(ConfigError::InvalidValue(ref message))
                if message.contains("BUZZ_AGE_ATTESTATION_REQUIRED")
        ));
    }

    #[test]
    fn rate_limits_can_be_overridden() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::set_var("BUZZ_RATE_LIMIT_HUMAN_MESSAGES_PER_MIN", "1001");
        std::env::set_var("BUZZ_RATE_LIMIT_HUMAN_API_CALLS_PER_MIN", "1002");
        std::env::set_var("BUZZ_RATE_LIMIT_HUMAN_WS_EVENTS_PER_SEC", "1003");

        let config = Config::from_env().expect("config");

        std::env::remove_var("BUZZ_RATE_LIMIT_HUMAN_MESSAGES_PER_MIN");
        std::env::remove_var("BUZZ_RATE_LIMIT_HUMAN_API_CALLS_PER_MIN");
        std::env::remove_var("BUZZ_RATE_LIMIT_HUMAN_WS_EVENTS_PER_SEC");
        assert_eq!(config.auth.rate_limits.human_messages_per_min, 1001);
        assert_eq!(config.auth.rate_limits.human_api_calls_per_min, 1002);
        assert_eq!(config.auth.rate_limits.human_ws_events_per_sec, 1003);
    }

    #[test]
    fn rate_limit_overrides_reject_zero() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::set_var("BUZZ_RATE_LIMIT_HUMAN_WS_EVENTS_PER_SEC", "0");
        let result = Config::from_env();
        std::env::remove_var("BUZZ_RATE_LIMIT_HUMAN_WS_EVENTS_PER_SEC");

        assert!(matches!(
            result,
            Err(ConfigError::InvalidValue(ref message))
                if message.contains("BUZZ_RATE_LIMIT_HUMAN_WS_EVENTS_PER_SEC")
        ));
    }

    #[test]
    fn relay_operator_pubkeys_parse_dedupe_and_normalize() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::set_var(
            "RELAY_OPERATOR_PUBKEYS",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA,bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb,aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        std::env::set_var(
            "RELAY_OPERATOR_API_ORIGIN",
            "http://buzz.mesh.bb-production.com",
        );
        let config = Config::from_env().expect("config");
        std::env::remove_var("RELAY_OPERATOR_PUBKEYS");
        std::env::remove_var("RELAY_OPERATOR_API_ORIGIN");

        assert_eq!(
            config.relay_operator_pubkeys,
            vec![
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
            ]
        );
    }

    #[test]
    fn relay_operator_pubkeys_invalid_entry_is_error() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::set_var("RELAY_OPERATOR_PUBKEYS", "not-a-pubkey");
        let result = Config::from_env();
        std::env::remove_var("RELAY_OPERATOR_PUBKEYS");

        assert!(matches!(
            result,
            Err(ConfigError::InvalidValue(ref msg)) if msg.contains("RELAY_OPERATOR_PUBKEYS")
        ));
    }

    #[test]
    fn relay_operator_pubkeys_require_api_origin() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::set_var(
            "RELAY_OPERATOR_PUBKEYS",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        std::env::remove_var("RELAY_OPERATOR_API_ORIGIN");
        let result = Config::from_env();
        std::env::remove_var("RELAY_OPERATOR_PUBKEYS");

        assert!(matches!(
            result,
            Err(ConfigError::InvalidValue(ref msg)) if msg.contains("RELAY_OPERATOR_API_ORIGIN is required")
        ));
    }

    #[test]
    fn relay_operator_api_origin_rejects_paths() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::set_var("RELAY_OPERATOR_API_ORIGIN", "https://buzz.example/operator");
        let result = Config::from_env();
        std::env::remove_var("RELAY_OPERATOR_API_ORIGIN");

        assert!(matches!(
            result,
            Err(ConfigError::InvalidValue(ref msg)) if msg.contains("must be an http(s) origin")
        ));
    }

    #[test]
    fn legacy_push_environment_is_ignored() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let names = [
            "BUZZ_PUSH_GATEWAY_DELIVERY_URL",
            "BUZZ_PUSH_GATEWAY_TIMEOUT_MS",
            "BUZZ_PUSH_EXECUTOR_KEY_ID",
        ];
        let previous = names.map(std::env::var_os);
        std::env::set_var(
            "BUZZ_PUSH_GATEWAY_DELIVERY_URL",
            "https://legacy-push.invalid/v1/deliveries/apns",
        );
        std::env::set_var("BUZZ_PUSH_GATEWAY_TIMEOUT_MS", "2000");
        std::env::set_var("BUZZ_PUSH_EXECUTOR_KEY_ID", "relay-v1");

        // Carryforth deliberately has no Relay push configuration. These
        // legacy variables must be inert rather than becoming a hidden path
        // back to the hosted Buzz push service.
        Config::from_env().expect("legacy push variables are ignored");

        for (name, value) in names.into_iter().zip(previous) {
            if let Some(value) = value {
                std::env::set_var(name, value);
            } else {
                std::env::remove_var(name);
            }
        }
    }

    #[test]
    fn huddle_audio_available_can_be_disabled_for_horizontal_scaling() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::set_var("BUZZ_HUDDLE_AUDIO_AVAILABLE", "false");
        let config = Config::from_env().expect("config");
        std::env::remove_var("BUZZ_HUDDLE_AUDIO_AVAILABLE");
        assert!(
            !config.huddle_audio_available,
            "BUZZ_HUDDLE_AUDIO_AVAILABLE=false must disable huddle audio (multi-pod deployments)"
        );
    }

    #[test]
    fn invalid_bind_addr_returns_error() {
        assert!(matches!(
            parse_bind_addr("not-an-addr"),
            Err(ConfigError::InvalidBindAddr(_))
        ));
    }

    #[test]
    fn pairing_relay_url_accepts_websocket_urls_and_rejects_http() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::set_var("BUZZ_PAIRING_RELAY_URL", "wss://pairing.buzz.xyz");
        let config = Config::from_env().expect("config");
        assert_eq!(
            config.pairing_relay_url.as_deref(),
            Some("wss://pairing.buzz.xyz")
        );

        std::env::set_var("BUZZ_PAIRING_RELAY_URL", "https://pairing.buzz.xyz");
        let result = Config::from_env();
        std::env::remove_var("BUZZ_PAIRING_RELAY_URL");
        assert!(matches!(
            result,
            Err(ConfigError::InvalidValue(ref msg)) if msg.contains("BUZZ_PAIRING_RELAY_URL")
        ));
    }

    #[test]
    fn max_frame_bytes_can_be_configured() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::set_var("BUZZ_MAX_FRAME_BYTES", "262144");
        let config = Config::from_env().expect("config");
        std::env::remove_var("BUZZ_MAX_FRAME_BYTES");
        assert_eq!(config.max_frame_bytes, 262_144);
    }

    #[test]
    fn git_repo_path_is_created_if_missing() {
        let _guard = ENV_MUTEX.lock().unwrap();
        // Pick a path under temp_dir that definitely doesn't exist yet.
        let base = std::env::temp_dir().join(format!(
            "buzz-test-git-repo-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let nested = base.join("nested").join("repos");
        assert!(!nested.exists(), "test precondition: path must not exist");

        std::env::set_var("BUZZ_GIT_REPO_PATH", &nested);
        let result = Config::from_env();
        std::env::remove_var("BUZZ_GIT_REPO_PATH");

        let config = result.expect("config should self-bootstrap missing git_repo_path");
        assert_eq!(config.git_repo_path, nested);
        assert!(
            nested.is_dir(),
            "git_repo_path should exist after config load"
        );

        // Cleanup.
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    #[cfg(unix)]
    fn git_repo_path_unwritable_returns_error() {
        // Try to create a path under a regular file — must fail.
        // Using /dev/null as the parent guarantees create_dir_all fails on unix.
        let bogus = std::path::PathBuf::from("/dev/null/cannot-create-here");
        let result = ensure_git_repo_path(&bogus);
        assert!(
            matches!(result, Err(ConfigError::InvalidValue(ref msg)) if msg.contains("BUZZ_GIT_REPO_PATH")),
            "expected InvalidValue mentioning BUZZ_GIT_REPO_PATH, got {result:?}"
        );
    }
}
