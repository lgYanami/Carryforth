//! NIP-11 relay information document.

use serde::{Deserialize, Serialize};

use buzz_project_context::PROJECT_CONTEXT_CAPABILITY;
use buzz_sdk::project_view_v3::{PROJECT_VIEW_V3_BOOTSTRAP_EXTENSION, PROJECT_VIEW_V3_EXTENSION};

#[cfg(test)]
use crate::config::DEFAULT_MAX_FRAME_BYTES;

/// NIPs unconditionally supported by this relay, advertised in the NIP-11
/// document. Kept as a module-level constant so tests can verify it without
/// constructing a full `Config` (which reads env vars and races with
/// config.rs tests).
///
/// NIP-43 (relay membership) is advertised separately by [`RelayInfo::build`]
/// only when membership enforcement is actually enabled — see that function.
pub(crate) const SUPPORTED_NIPS: &[u32] = &[1, 2, 10, 11, 16, 17, 23, 25, 29, 33, 38, 42, 50, 56];

/// NIP-43 (relay membership). Advertised only when the relay actually
/// enforces membership (`BUZZ_REQUIRE_RELAY_MEMBERSHIP=true`) AND has a
/// stable signing key — both are required for kind 13534/8000/8001 events
/// to be verifiable by clients.
pub(crate) const NIP_RELAY_MEMBERSHIP: u32 = 43;
const PROJECT_CONTEXT_EXTENSION: &str = "buzz-project-context-v1";
const PROJECT_DOCUMENT_EXTENSION: &str = "buzz-project-document-v1";
pub(crate) const MEETING_V2_EXTENSION: &str = "buzz-meeting-v2";
pub(crate) const MEETING_V2_CREATE_EXTENSION: &str = "buzz-meeting-v2-create";
pub(crate) const MEETING_V2_DIRECT_ACTIONS_EXTENSION: &str = "buzz-meeting-v2-direct-actions";
pub(crate) const MEETING_V2_DIRECT_ACTIONS_CREATE_EXTENSION: &str =
    "buzz-meeting-v2-direct-actions-create";
pub(crate) const MEETING_SUMMARY_EXTENSION: &str = "buzz-meeting-summary-v1";
/// Community-wide Meeting history/read authorization contract.
pub(crate) const MEETING_COMMUNITY_READ_EXTENSION: &str = "buzz-meeting-community-read-v1";

/// Relay information document served at `GET /` with `Accept: application/nostr+json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayInfo {
    /// Human-readable relay name.
    pub name: String,
    /// Human-readable relay description.
    pub description: String,
    /// Workspace icon URL (NIP-11 `icon`), per-community, set by relay
    /// admins/owners via the kind:9033 command. Omitted when no icon is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Relay operator's public key (hex), if published.
    pub pubkey: Option<String>,
    /// Contact address for the relay operator.
    pub contact: Option<String>,
    /// NIPs supported by this relay.
    pub supported_nips: Vec<u32>,
    /// Draft/extension protocol identifiers supported by this relay.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supported_extensions: Option<Vec<String>>,
    /// Reserved NIP-PL executor descriptor field.
    ///
    /// Carryforth leaves this absent because Push is outside the supported
    /// local Relay release surface.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub push: Option<serde_json::Value>,
    /// URL of the relay software repository.
    pub software: String,
    /// Relay software version string.
    pub version: String,
    /// Protocol and resource limits advertised to clients.
    pub limitation: Option<RelayLimitation>,
    /// Public WebSocket URL of the dedicated NIP-AB device-pairing relay.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pairing_relay_url: Option<String>,
    /// Relay's own signing pubkey (NIP-11 `self` field, NIP-43).
    #[serde(rename = "self", skip_serializing_if = "Option::is_none")]
    pub relay_self: Option<String>,
}

/// Protocol and resource limits advertised in the NIP-11 document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayLimitation {
    /// Maximum WebSocket frame size in bytes.
    pub max_message_length: Option<u64>,
    /// Maximum number of concurrent subscriptions per connection.
    pub max_subscriptions: Option<u32>,
    /// Maximum number of filters per subscription.
    pub max_filters: Option<u32>,
    /// Maximum value of the `limit` field in a filter.
    pub max_limit: Option<u32>,
    /// Maximum length of a subscription ID string.
    pub max_subid_length: Option<u32>,
    /// Minimum proof-of-work difficulty required for events.
    pub min_pow_difficulty: Option<u32>,
    /// Whether NIP-42 authentication is required before subscribing or
    /// publishing events.
    pub auth_required: bool,
    /// Whether payment is required to use the relay.
    pub payment_required: bool,
    /// Whether writes are restricted to authorized pubkeys.
    pub restricted_writes: bool,
    /// NIP-ER: how the relay delivers due reminders ("push" or "lazy").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_delivery_mode: Option<String>,
    /// NIP-ER: maximum allowed `not_before` horizon in seconds from now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_not_before_delta: Option<u64>,
}

/// Canonical `RelayLimitation` advertised by this relay.
///
/// `auth_required` is always `true`: the REQ, EVENT, and COUNT handlers
/// unconditionally reject connections that are not in
/// `AuthState::Authenticated`. This is independent of the REST API token
/// toggle (`config.require_auth_token`).
fn relay_limitation(max_message_length: usize) -> RelayLimitation {
    let max_not_before_delta: u64 = std::env::var("SPROUT_MAX_NOT_BEFORE_DELTA")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(31_536_000); // 1 year default

    RelayLimitation {
        max_message_length: Some(max_message_length as u64),
        max_subscriptions: Some(1024),
        max_filters: Some(10),
        max_limit: Some(10_000),
        max_subid_length: Some(256),
        min_pow_difficulty: None,
        auth_required: true,
        payment_required: false,
        restricted_writes: true,
        due_delivery_mode: Some("push".to_string()),
        max_not_before_delta: Some(max_not_before_delta),
    }
}

impl RelayInfo {
    /// Builds the relay's NIP-11 information document.
    ///
    /// `relay_self` is the relay's own signing pubkey (hex), advertised as the
    /// NIP-11 `self` field. NIP-11 defines `self` generically as the relay's
    /// identity key; other NIPs reference it. Notably NIP-29 (group metadata
    /// kinds 39000/39001/39002, which Buzz signs with `state.relay_keypair`
    /// unconditionally) requires clients to verify those events against
    /// `self`. Pass `Some` whenever the relay has a stable signing key.
    ///
    /// `icon` is the community's workspace icon (see
    /// [`workspace_icon_for_host`]) — a host-scoped scalar, pre-fetched by
    /// the caller so `build` itself stays static-input.
    ///
    /// `advertise_nip43` controls whether NIP-43 (relay membership) is added
    /// to `supported_nips`. Set `true` only when the relay actually emits and
    /// gates on NIP-43 events — i.e. has a stable key AND enforces
    /// membership. NIP-43 events are verified against `self`, so it is a
    /// programmer error to advertise NIP-43 without a `relay_self`.
    pub fn build(
        relay_self: Option<&str>,
        icon: Option<&str>,
        advertise_nip43: bool,
        advertise_project_view: bool,
        max_message_length: usize,
        pairing_relay_url: Option<&str>,
    ) -> Self {
        debug_assert!(
            !advertise_nip43 || relay_self.is_some(),
            "advertise_nip43=true requires relay_self=Some — NIP-43 events are verified against `self`"
        );

        let mut supported_nips = SUPPORTED_NIPS.to_vec();
        if advertise_nip43 {
            supported_nips.push(NIP_RELAY_MEMBERSHIP);
        }

        let mut supported_extensions = vec!["nip-er".to_string()];
        if advertise_project_view {
            supported_extensions.push(PROJECT_VIEW_V3_EXTENSION.to_owned());
        }

        Self {
            name: "Carryforth Relay".to_string(),
            description: "Local-first collaboration relay for Carryforth".to_string(),
            icon: icon.filter(|s| !s.is_empty()).map(|s| s.to_string()),
            pubkey: None,
            contact: None,
            supported_nips,
            supported_extensions: Some(supported_extensions),
            push: None,
            software: "https://github.com/lgYanami/Carryforth".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            limitation: Some(relay_limitation(max_message_length)),
            pairing_relay_url: pairing_relay_url.map(str::to_string),
            relay_self: relay_self.map(|s| s.to_string()),
        }
    }
}

/// Axum handler that returns the NIP-11 relay information document as JSON.
pub async fn relay_info_handler(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::state::AppState>>,
    headers: axum::http::HeaderMap,
) -> axum::response::Json<RelayInfo> {
    let raw_host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    axum::response::Json(nip11_document(&state, raw_host).await)
}

/// Builds the served NIP-11 document for a request arriving on `raw_host`.
///
/// Centralised so the content-negotiated root handler and the dedicated
/// `/info` endpoint can't drift apart. Every input to `RelayInfo::build`
/// stays a pre-derived scalar: [`nip11_facts`] (config + keypair) plus the
/// host-scoped workspace icon.
pub(crate) async fn nip11_document(state: &crate::state::AppState, raw_host: &str) -> RelayInfo {
    let (relay_self, advertise_nip43) = nip11_facts(state);
    let icon = workspace_icon_for_host(state, raw_host).await;
    let project_view_ready = project_view_ready_for_host(state, raw_host).await;
    let mut info = RelayInfo::build(
        relay_self.as_deref(),
        icon.as_deref(),
        advertise_nip43,
        project_view_ready,
        state.config.max_frame_bytes,
        state.config.pairing_relay_url.as_deref(),
    );
    append_extension(
        &mut info,
        PROJECT_CONTEXT_EXTENSION,
        project_context_ready_for_host(state, raw_host).await,
    );
    let meeting_community_read_ready = meeting_community_read_ready_for_host(state, raw_host).await;
    append_extension(
        &mut info,
        MEETING_COMMUNITY_READ_EXTENSION,
        meeting_community_read_ready,
    );
    append_extension(
        &mut info,
        PROJECT_CONTEXT_CAPABILITY,
        project_context_edge_ready_for_host(state, raw_host, meeting_community_read_ready).await,
    );
    append_project_document_extension(
        &mut info,
        project_document_ready_for_host(state, raw_host).await,
    );
    append_extension(
        &mut info,
        PROJECT_VIEW_V3_BOOTSTRAP_EXTENSION,
        !project_view_ready
            && project_view_v3_bootstrap_discoverable_for_host(state, raw_host).await,
    );
    let meeting_v2_ready = meeting_v2_runtime_ready(state).await;
    apply_meeting_v2_extensions(
        &mut info,
        meeting_v2_ready,
        state.config.meeting_v2_create_enabled,
        state.config.meeting_v2_direct_actions_create_enabled,
        meeting_summary_runtime_ready(state, meeting_v2_ready).await,
    );
    info
}

fn append_project_document_extension(info: &mut RelayInfo, ready: bool) {
    append_extension(info, PROJECT_DOCUMENT_EXTENSION, ready);
}

fn append_extension(info: &mut RelayInfo, extension: &str, ready: bool) {
    if ready {
        let extensions = info.supported_extensions.get_or_insert_default();
        if !extensions.iter().any(|candidate| candidate == extension) {
            extensions.push(extension.to_owned());
        }
    }
}

async fn project_context_ready_for_host(state: &crate::state::AppState, raw_host: &str) -> bool {
    if state.config.relay_private_key.is_none() {
        return false;
    }
    let Ok(tenant) = crate::tenant::bind_community(&state.db, raw_host).await else {
        return false;
    };
    match state
        .db
        .project_context_v1_advertised_ready(tenant.community(), &state.relay_keypair.public_key())
        .await
    {
        Ok(ready) => ready,
        Err(error) => {
            tracing::warn!(
                community_id = %tenant.community(),
                "Project Context NIP-11 readiness failed closed: {error}"
            );
            false
        }
    }
}

async fn project_context_edge_ready_for_host(
    state: &crate::state::AppState,
    raw_host: &str,
    meeting_community_read_ready: bool,
) -> bool {
    if state.config.relay_private_key.is_none() || !meeting_community_read_ready {
        return false;
    }
    let Ok(tenant) = crate::tenant::bind_community(&state.db, raw_host).await else {
        return false;
    };
    match state
        .db
        .project_context_advertised_ready(tenant.community(), &state.relay_keypair.public_key())
        .await
    {
        Ok(ready) => ready,
        Err(error) => {
            tracing::warn!(
                community_id = %tenant.community(),
                "Project Context Edge NIP-11 readiness failed closed: {error}"
            );
            false
        }
    }
}

async fn meeting_community_read_ready_for_host(
    state: &crate::state::AppState,
    raw_host: &str,
) -> bool {
    let Ok(tenant) = crate::tenant::bind_community(&state.db, raw_host).await else {
        return false;
    };
    match crate::handlers::req::meeting_community_read_active(state, tenant.community()).await {
        Ok(ready) => ready,
        Err(error) => {
            tracing::warn!(
                community_id = %tenant.community(),
                "Meeting Community-read NIP-11 readiness failed closed: {error}"
            );
            false
        }
    }
}

async fn project_document_ready_for_host(state: &crate::state::AppState, raw_host: &str) -> bool {
    if state.config.relay_private_key.is_none() {
        return false;
    }
    let Ok(tenant) = crate::tenant::bind_community(&state.db, raw_host).await else {
        return false;
    };
    match state
        .db
        .project_document_capability_ready(tenant.community(), &state.relay_keypair.public_key())
        .await
    {
        Ok(ready) => ready,
        Err(error) => {
            tracing::warn!(
                community_id = %tenant.community(),
                "Project Document NIP-11 readiness failed closed: {error}"
            );
            false
        }
    }
}

fn apply_meeting_v2_extensions(
    info: &mut RelayInfo,
    runtime_ready: bool,
    create_enabled: bool,
    direct_actions_create_enabled: bool,
    meeting_summary_ready: bool,
) {
    if !runtime_ready {
        return;
    }
    let extensions = info.supported_extensions.get_or_insert_default();
    extensions.push(MEETING_V2_EXTENSION.to_owned());
    extensions.push(MEETING_V2_DIRECT_ACTIONS_EXTENSION.to_owned());
    if create_enabled {
        extensions.push(MEETING_V2_CREATE_EXTENSION.to_owned());
    }
    if create_enabled && direct_actions_create_enabled {
        extensions.push(MEETING_V2_DIRECT_ACTIONS_CREATE_EXTENSION.to_owned());
    }
    if meeting_summary_ready {
        extensions.push(MEETING_SUMMARY_EXTENSION.to_owned());
    }
}

async fn meeting_summary_runtime_ready(
    state: &crate::state::AppState,
    meeting_v2_ready: bool,
) -> bool {
    if !meeting_v2_ready || state.config.relay_private_key.is_none() {
        return false;
    }
    match state.db.meeting_summary_schema_ready().await {
        Ok(ready) => ready,
        Err(error) => {
            tracing::warn!("Meeting summary NIP-11 readiness failed closed: {error}");
            false
        }
    }
}

async fn meeting_v2_runtime_ready(state: &crate::state::AppState) -> bool {
    if state.config.relay_private_key.is_none() {
        return false;
    }
    match state.db.meeting_v2_schema_ready().await {
        Ok(ready) => ready,
        Err(error) => {
            tracing::warn!("Meeting V2 NIP-11 readiness failed closed: {error}");
            false
        }
    }
}

async fn project_view_ready_for_host(state: &crate::state::AppState, raw_host: &str) -> bool {
    if state.config.relay_private_key.is_none() {
        return false;
    }
    let Ok(tenant) = crate::tenant::bind_community(&state.db, raw_host).await else {
        return false;
    };
    let schema_version = match state
        .db
        .project_view_schema_version(tenant.community())
        .await
    {
        Ok(version) => version,
        Err(error) => {
            tracing::warn!(
                community_id = %tenant.community(),
                "Project View NIP-11 schema lookup failed closed: {error}"
            );
            return false;
        }
    };
    if !project_view_schema_is_advertisable(schema_version) {
        return false;
    }
    match state
        .db
        .project_view_v3_advertised_write_ready(
            tenant.community(),
            &state.relay_keypair.public_key(),
        )
        .await
    {
        Ok(ready) => ready,
        Err(error) => {
            tracing::warn!(
                community_id = %tenant.community(),
                "Project View NIP-11 readiness failed closed: {error}"
            );
            false
        }
    }
}

/// Return whether Desktop may show the closed v3 owner-bootstrap guide.
///
/// This marker is intentionally distinct from `buzz-project-view-v3`: it is
/// present only for a schema-v3 Community with no canonical state and cannot
/// authorize ordinary reads or writes. An initialized-but-disabled Community
/// is a maintenance state and therefore does not masquerade as greenfield.
async fn project_view_v3_bootstrap_discoverable_for_host(
    state: &crate::state::AppState,
    raw_host: &str,
) -> bool {
    if state.config.relay_private_key.is_none() {
        return false;
    }
    let Ok(tenant) = crate::tenant::bind_community(&state.db, raw_host).await else {
        return false;
    };
    match state
        .db
        .project_view_v3_bootstrap_discoverable(tenant.community())
        .await
    {
        Ok(discoverable) => discoverable,
        Err(error) => {
            tracing::warn!(
                community_id = %tenant.community(),
                "Project View bootstrap discovery lookup failed closed: {error}"
            );
            false
        }
    }
}

const fn project_view_schema_is_advertisable(schema_version: i16) -> bool {
    schema_version == 3
}

/// Fetches the workspace icon for the community bound to `raw_host`, as the
/// host-scoped scalar consumed by [`RelayInfo::build`].
///
/// The icon is per-community state (`communities.icon`, set by relay
/// admins/owners via the kind:9033 command) served in the standard NIP-11
/// `icon` field. The lookup is scoped through
/// [`crate::tenant::bind_community`] — never an unscoped query. Fails open to
/// `None` (no `icon` field): NIP-11 is intentionally served to unmapped hosts
/// too, and an icon lookup failure must not break that.
async fn workspace_icon_for_host(state: &crate::state::AppState, raw_host: &str) -> Option<String> {
    let tenant = crate::tenant::bind_community(&state.db, raw_host)
        .await
        .ok()?;
    state
        .db
        .get_community_icon(tenant.community())
        .await
        .ok()
        .flatten()
}

/// Derives the two NIP-11 facts that depend on runtime config:
///
/// - `relay_self`: the NIP-11 `self` pubkey, set whenever the relay has a
///   stable signing key. Consumed by NIP-29 (group metadata verification)
///   and NIP-43, among others. Ephemeral keys are excluded because they
///   change on restart, leaving previously-signed events unverifiable.
/// - `advertise_nip43`: whether to list NIP-43 in `supported_nips`. True
///   only when membership is actually enforced AND we have a stable key
///   (NIP-43 events must be verifiable against `self`).
///
/// Centralised so the content-negotiated root handler and the dedicated
/// `/info` endpoint can't drift apart.
pub(crate) fn nip11_facts(state: &crate::state::AppState) -> (Option<String>, bool) {
    let has_stable_key = state.config.relay_private_key.is_some();
    let relay_self = has_stable_key.then(|| state.relay_keypair.public_key().to_hex());
    let advertise_nip43 = has_stable_key && state.config.require_relay_membership;
    (relay_self, advertise_nip43)
}

/// Multi-tenant conformance static-input fence (surface row "NIP-11 relay info
/// and relay `self`").
///
/// The conformance obligation: `RelayInfo::build` "must not grow unscoped
/// DB/search/audit inputs", so an unauthenticated NIP-11 read can never become
/// an enumeration oracle for *other* communities. `build` takes only static
/// and scalar inputs — the per-deployment facts arrive pre-derived through
/// [`nip11_facts`] (config + relay keypair), and the one host-scoped fact
/// (the workspace `icon`) arrives as a scalar from
/// [`workspace_icon_for_host`], whose DB lookup is scoped through
/// [`crate::tenant::bind_community`] and can therefore only ever surface the
/// requesting host's own community state.
///
/// This const binds `RelayInfo::build` to its **exact** allowed signature. The
/// moment someone adds a `&Db`, `&AppState`, a search handle, an audit handle,
/// or any other unscoped input, the function pointer's type stops matching and
/// **this file fails to compile** — turning a silent cross-tenant leak into a
/// hard build break, the same way a deny-lint would. If you must change this
/// signature, you are changing the conformance contract: update the conformance
/// doc and prove the new input is host-scoped, not unscoped, first.
#[allow(clippy::type_complexity)]
const _RELAY_INFO_BUILD_STATIC_INPUT_FENCE: fn(
    Option<&str>,
    Option<&str>,
    bool,
    bool,
    usize,
    Option<&str>,
) -> RelayInfo = RelayInfo::build;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carryforth_relay_info_does_not_advertise_push() {
        let info = RelayInfo::build(None, None, false, false, 1024, None);
        assert!(info.push.is_none());
        assert!(
            info.supported_extensions
                .as_ref()
                .is_none_or(|extensions| !extensions.iter().any(|item| item == "nip-pl")),
            "Carryforth must not advertise the disabled NIP-PL surface"
        );
    }

    #[test]
    fn supported_nips_includes_nip23_and_nip33() {
        // Tests the production SUPPORTED_NIPS constant directly — no Config::from_env()
        // needed, avoiding the env-var race with config.rs tests.
        assert!(
            SUPPORTED_NIPS.contains(&23),
            "NIP-23 (long-form content) must be advertised"
        );
        assert!(
            SUPPORTED_NIPS.contains(&33),
            "NIP-33 (parameterized replaceable) must be advertised"
        );
    }

    #[test]
    fn supported_nips_includes_nip38() {
        assert!(
            SUPPORTED_NIPS.contains(&38),
            "NIP-38 (user statuses) must be advertised"
        );
    }

    #[test]
    fn supported_nips_includes_nip56() {
        assert!(
            SUPPORTED_NIPS.contains(&56),
            "NIP-56 (reporting) must be advertised — kind:1984 ingest is live"
        );
    }

    #[test]
    fn build_advertises_carryforth_product_identity() {
        let info = RelayInfo::build(None, None, false, false, DEFAULT_MAX_FRAME_BYTES, None);
        assert_eq!(info.name, "Carryforth Relay");
        assert_eq!(
            info.description,
            "Local-first collaboration relay for Carryforth"
        );
        assert_eq!(info.software, "https://github.com/lgYanami/Carryforth");
    }

    #[test]
    fn configured_pairing_relay_is_advertised_and_unset_value_is_omitted() {
        let info = RelayInfo::build(
            None,
            None,
            false,
            false,
            DEFAULT_MAX_FRAME_BYTES,
            Some("wss://pairing.buzz.xyz"),
        );
        let json = serde_json::to_value(&info).expect("serialize");
        assert_eq!(
            json.get("pairing_relay_url")
                .and_then(|value| value.as_str()),
            Some("wss://pairing.buzz.xyz")
        );

        let info = RelayInfo::build(None, None, false, false, DEFAULT_MAX_FRAME_BYTES, None);
        let json = serde_json::to_value(&info).expect("serialize");
        assert!(json.get("pairing_relay_url").is_none());
    }

    /// NIP-WP → NIP-11 mirror: a set workspace icon is served in the standard
    /// `icon` field; no icon (or a cleared, empty icon) omits the field
    /// entirely so the JSON matches pre-icon documents byte-for-byte.
    #[test]
    fn icon_is_mirrored_and_empty_or_absent_is_omitted() {
        let info = RelayInfo::build(
            None,
            Some("data:image/webp;base64,UklGRg=="),
            false,
            false,
            DEFAULT_MAX_FRAME_BYTES,
            None,
        );
        assert_eq!(
            info.icon.as_deref(),
            Some("data:image/webp;base64,UklGRg==")
        );
        let json = serde_json::to_value(&info).expect("serialize");
        assert_eq!(
            json.get("icon").and_then(|v| v.as_str()),
            Some("data:image/webp;base64,UklGRg==")
        );

        for icon in [None, Some("")] {
            let info = RelayInfo::build(None, icon, false, false, DEFAULT_MAX_FRAME_BYTES, None);
            assert!(info.icon.is_none());
            let json = serde_json::to_value(&info).expect("serialize");
            assert!(
                json.get("icon").is_none(),
                "unset/cleared icon must omit the `icon` field, not serialize null/empty"
            );
        }
    }

    #[test]
    fn auth_required_is_advertised_true() {
        // REQ, EVENT, and COUNT all unconditionally require
        // `AuthState::Authenticated` (see `crates/buzz-relay/src/handlers/`),
        // so the NIP-11 doc must advertise it.
        assert!(relay_limitation(DEFAULT_MAX_FRAME_BYTES).auth_required);
    }

    #[test]
    fn max_message_length_uses_configured_frame_limit() {
        let info = RelayInfo::build(None, None, false, false, 262_144, None);
        let limitation = info.limitation.expect("limitation");
        assert_eq!(limitation.max_message_length, Some(262_144));
    }

    #[test]
    fn supported_nips_are_sorted() {
        let mut sorted = SUPPORTED_NIPS.to_vec();
        sorted.sort();
        assert_eq!(
            SUPPORTED_NIPS,
            &sorted[..],
            "supported_nips should be sorted"
        );
    }

    #[test]
    fn nip43_not_in_static_supported_nips() {
        // NIP-43 advertisement is conditional on runtime config (stable signing
        // key + membership enforcement) and must NOT live in the static list.
        // The desktop pairing probe keys off this NIP — advertising it on
        // open relays misroutes pairing peers to a non-existent /pair sidecar.
        assert!(
            !SUPPORTED_NIPS.contains(&NIP_RELAY_MEMBERSHIP),
            "NIP-43 must be advertised only when advertise_nip43=true is passed to RelayInfo::build"
        );
    }

    /// Open relay, ephemeral key — both `self` and NIP-43 are absent.
    #[test]
    fn build_open_relay_ephemeral_key_omits_self_and_nip43() {
        let info = RelayInfo::build(None, None, false, false, DEFAULT_MAX_FRAME_BYTES, None);
        assert!(info.relay_self.is_none());
        assert!(!info.supported_nips.contains(&NIP_RELAY_MEMBERSHIP));
    }

    #[test]
    fn only_v3_project_view_extension_is_controlled_by_host_scoped_ready_scalar() {
        let disabled = RelayInfo::build(None, None, false, false, DEFAULT_MAX_FRAME_BYTES, None);
        assert!(!disabled
            .supported_extensions
            .as_ref()
            .is_some_and(|extensions| extensions
                .iter()
                .any(|value| value == PROJECT_VIEW_V3_EXTENSION)));

        let enabled = RelayInfo::build(None, None, false, true, DEFAULT_MAX_FRAME_BYTES, None);
        assert!(enabled
            .supported_extensions
            .as_ref()
            .is_some_and(|extensions| extensions
                .iter()
                .any(|value| value == PROJECT_VIEW_V3_EXTENSION)));
        assert!(!enabled
            .supported_extensions
            .as_ref()
            .is_some_and(|extensions| {
                extensions.iter().any(|value| {
                    matches!(
                        value.as_str(),
                        "buzz-project-view-v1" | "buzz-project-view-v2"
                    )
                })
            }));
    }

    #[test]
    fn bootstrap_discovery_is_distinct_from_runtime_readiness() {
        let mut info = RelayInfo::build(None, None, false, false, DEFAULT_MAX_FRAME_BYTES, None);
        append_extension(&mut info, PROJECT_VIEW_V3_BOOTSTRAP_EXTENSION, true);
        let extensions = info.supported_extensions.expect("extensions");
        assert!(extensions
            .iter()
            .any(|value| value == PROJECT_VIEW_V3_BOOTSTRAP_EXTENSION));
        assert!(!extensions
            .iter()
            .any(|value| value == PROJECT_VIEW_V3_EXTENSION));
    }

    #[test]
    fn project_view_advertisement_rejects_legacy_schema_majors() {
        assert!(!project_view_schema_is_advertisable(1));
        assert!(!project_view_schema_is_advertisable(2));
        assert!(project_view_schema_is_advertisable(3));
        assert!(!project_view_schema_is_advertisable(4));
    }

    #[test]
    fn project_document_extension_is_appended_only_for_ready_host_state() {
        let mut info = RelayInfo::build(None, None, false, false, DEFAULT_MAX_FRAME_BYTES, None);
        append_project_document_extension(&mut info, false);
        assert!(!info
            .supported_extensions
            .as_ref()
            .is_some_and(|extensions| extensions
                .iter()
                .any(|value| value == PROJECT_DOCUMENT_EXTENSION)));

        append_project_document_extension(&mut info, true);
        append_project_document_extension(&mut info, true);
        let extensions = info.supported_extensions.expect("extensions");
        assert_eq!(
            extensions
                .iter()
                .filter(|value| value.as_str() == PROJECT_DOCUMENT_EXTENSION)
                .count(),
            1
        );
    }

    #[test]
    fn meeting_community_read_extension_is_appended_only_after_publication() {
        let mut info = RelayInfo::build(None, None, false, false, DEFAULT_MAX_FRAME_BYTES, None);
        append_extension(&mut info, MEETING_COMMUNITY_READ_EXTENSION, false);
        assert!(!info
            .supported_extensions
            .as_ref()
            .is_some_and(|extensions| extensions
                .iter()
                .any(|value| value == MEETING_COMMUNITY_READ_EXTENSION)));

        append_extension(&mut info, MEETING_COMMUNITY_READ_EXTENSION, true);
        append_extension(&mut info, MEETING_COMMUNITY_READ_EXTENSION, true);
        let extensions = info.supported_extensions.expect("extensions");
        assert_eq!(
            extensions
                .iter()
                .filter(|value| value.as_str() == MEETING_COMMUNITY_READ_EXTENSION)
                .count(),
            1
        );
    }

    #[test]
    fn project_context_edge_extension_is_distinct_and_idempotent() {
        let mut info = RelayInfo::build(None, None, false, false, DEFAULT_MAX_FRAME_BYTES, None);
        append_extension(&mut info, PROJECT_CONTEXT_CAPABILITY, false);
        append_extension(&mut info, PROJECT_CONTEXT_CAPABILITY, true);
        append_extension(&mut info, PROJECT_CONTEXT_CAPABILITY, true);
        let extensions = info.supported_extensions.expect("extensions");
        assert_ne!(PROJECT_CONTEXT_EXTENSION, PROJECT_CONTEXT_CAPABILITY);
        assert_eq!(
            extensions
                .iter()
                .filter(|value| value.as_str() == PROJECT_CONTEXT_CAPABILITY)
                .count(),
            1
        );
    }

    #[test]
    fn meeting_v2_runtime_and_create_capabilities_are_independent() {
        let mut unavailable =
            RelayInfo::build(None, None, false, false, DEFAULT_MAX_FRAME_BYTES, None);
        apply_meeting_v2_extensions(&mut unavailable, false, true, true, false);
        let unavailable_extensions = unavailable.supported_extensions.unwrap_or_default();
        assert!(!unavailable_extensions.contains(&MEETING_V2_EXTENSION.to_owned()));
        assert!(!unavailable_extensions.contains(&MEETING_V2_CREATE_EXTENSION.to_owned()));
        assert!(!unavailable_extensions.contains(&MEETING_V2_DIRECT_ACTIONS_EXTENSION.to_owned()));
        assert!(!unavailable_extensions
            .contains(&MEETING_V2_DIRECT_ACTIONS_CREATE_EXTENSION.to_owned()));

        let mut drain_only =
            RelayInfo::build(None, None, false, false, DEFAULT_MAX_FRAME_BYTES, None);
        apply_meeting_v2_extensions(&mut drain_only, true, false, false, true);
        let drain_extensions = drain_only.supported_extensions.unwrap_or_default();
        assert!(drain_extensions.contains(&MEETING_V2_EXTENSION.to_owned()));
        assert!(drain_extensions.contains(&MEETING_V2_DIRECT_ACTIONS_EXTENSION.to_owned()));
        assert!(drain_extensions.contains(&MEETING_SUMMARY_EXTENSION.to_owned()));
        assert!(!drain_extensions.contains(&MEETING_V2_CREATE_EXTENSION.to_owned()));
        assert!(!drain_extensions.contains(&MEETING_V2_DIRECT_ACTIONS_CREATE_EXTENSION.to_owned()));

        let mut creating =
            RelayInfo::build(None, None, false, false, DEFAULT_MAX_FRAME_BYTES, None);
        apply_meeting_v2_extensions(&mut creating, true, true, false, true);
        let creating_extensions = creating.supported_extensions.unwrap_or_default();
        assert!(creating_extensions.contains(&MEETING_V2_EXTENSION.to_owned()));
        assert!(creating_extensions.contains(&MEETING_V2_DIRECT_ACTIONS_EXTENSION.to_owned()));
        assert!(creating_extensions.contains(&MEETING_V2_CREATE_EXTENSION.to_owned()));
        assert!(
            !creating_extensions.contains(&MEETING_V2_DIRECT_ACTIONS_CREATE_EXTENSION.to_owned())
        );

        let mut action_creating =
            RelayInfo::build(None, None, false, false, DEFAULT_MAX_FRAME_BYTES, None);
        apply_meeting_v2_extensions(&mut action_creating, true, true, true, true);
        let action_extensions = action_creating.supported_extensions.unwrap_or_default();
        assert!(action_extensions.contains(&MEETING_V2_DIRECT_ACTIONS_CREATE_EXTENSION.to_owned()));
    }

    /// Open relay with a stable signing key (e.g. for NIP-29 group metadata
    /// signing): `self` MUST be advertised so clients can verify those
    /// events; NIP-43 must NOT be, because the relay isn't enforcing
    /// membership. This is the staging-default shape — the bug we're
    /// fixing — and the regression we must not reintroduce.
    #[test]
    fn build_open_relay_stable_key_advertises_self_but_not_nip43() {
        let pk = "0000000000000000000000000000000000000000000000000000000000000001";
        let info = RelayInfo::build(Some(pk), None, false, false, DEFAULT_MAX_FRAME_BYTES, None);
        assert_eq!(info.relay_self.as_deref(), Some(pk));
        assert!(!info.supported_nips.contains(&NIP_RELAY_MEMBERSHIP));
    }

    /// Membership-enforcing relay: both `self` and NIP-43 advertised.
    #[test]
    fn build_membership_relay_advertises_self_and_nip43() {
        let pk = "0000000000000000000000000000000000000000000000000000000000000001";
        let info = RelayInfo::build(Some(pk), None, true, false, DEFAULT_MAX_FRAME_BYTES, None);
        assert_eq!(info.relay_self.as_deref(), Some(pk));
        assert!(info.supported_nips.contains(&NIP_RELAY_MEMBERSHIP));
    }

    /// NIP-43 events are verified against `self`; advertising NIP-43 without
    /// `self` would give clients no way to verify membership events. The
    /// debug_assert in `build` catches this in tests/debug builds.
    #[test]
    #[should_panic(expected = "advertise_nip43=true requires relay_self=Some")]
    fn build_nip43_without_self_panics_in_debug() {
        let _ = RelayInfo::build(None, None, true, false, DEFAULT_MAX_FRAME_BYTES, None);
    }
}
