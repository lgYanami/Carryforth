//! Desktop-owned Runtime supervisor identities.
//!
//! One identity is scoped to one canonical Community relay URL. It is created
//! only by an explicit prepare command, stored in the OS secret store with a
//! restricted-file fallback, and passed only to the trusted ACP harness.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use nostr::{Keys, PublicKey, ToBech32 as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tauri::{AppHandle, Manager as _};
use url::{Host, Url};
use zeroize::Zeroizing;

use crate::app_state::AppState;

const SUPERVISOR_OVERRIDE_ENV: &str = "BUZZ_RUNTIME_SUPERVISOR_PRIVATE_KEY";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSupervisorIdentityAvailability {
    Missing,
    Ready,
    Locked,
    Lost,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSupervisorIdentitySource {
    Environment,
    Keyring,
    RestrictedFile,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSupervisorIdentityStatus {
    pub relay_url: String,
    pub availability: RuntimeSupervisorIdentityAvailability,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<RuntimeSupervisorIdentitySource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail_code: Option<String>,
}

struct ResolvedRuntimeSupervisorIdentity {
    pub keys: Keys,
    pub status: RuntimeSupervisorIdentityStatus,
}

pub(crate) struct RuntimeSupervisorSpawnIdentity {
    pub keys: Option<Keys>,
    pub status: RuntimeSupervisorIdentityStatus,
}

impl RuntimeSupervisorSpawnIdentity {
    pub(crate) fn unavailable(relay_url: &str, detail_code: &str) -> Self {
        Self {
            keys: None,
            status: RuntimeSupervisorIdentityStatus {
                relay_url: relay_url.to_owned(),
                availability: RuntimeSupervisorIdentityAvailability::Invalid,
                public_key: None,
                source: None,
                detail_code: Some(detail_code.to_owned()),
            },
        }
    }
}

pub(crate) fn configure_runtime_supervision_for_spawn(
    app: &AppHandle,
    command: &mut std::process::Command,
    resolved_acp_command: &Path,
    connection_relay_url: &str,
    runtime_key: &super::ManagedAgentRuntimeKey,
) -> Result<(), String> {
    // Clear inherited state first. Only Buzz's audited ACP harness may receive
    // the Desktop-owned supervisor secret, and ACP removes it before spawning
    // any model-facing child.
    command
        .env_remove("BUZZ_RUNTIME_SUPERVISOR_PRIVATE_KEY")
        .env_remove("BUZZ_RUNTIME_SUPERVISION_STATE_PATH")
        .env_remove("BUZZ_RUNTIME_FENCE_PATH")
        .env_remove("BUZZ_RUNTIME_ID")
        .env_remove("BUZZ_RUNTIME_EPOCH");
    let member_pubkey = parse_public_key(&runtime_key.pubkey, "managed Agent")?;
    let supervisor_identity =
        if is_trusted_runtime_supervisor_harness(&resolved_acp_command.to_string_lossy()) {
            match load_runtime_supervisor_identity(app, connection_relay_url, member_pubkey) {
                Ok(identity) => identity,
                Err(error) => {
                    eprintln!(
                        "buzz-desktop: Runtime supervisor identity unavailable for {}: {error}",
                        runtime_key.runtime_id()
                    );
                    RuntimeSupervisorSpawnIdentity::unavailable(
                        connection_relay_url,
                        "local_identity_unavailable",
                    )
                }
            }
        } else {
            RuntimeSupervisorSpawnIdentity::unavailable(
                connection_relay_url,
                "untrusted_acp_harness",
            )
        };
    let initial_supervision = super::ManagedAgentRuntimeSupervisionStatus::awaiting_observer(Some(
        &supervisor_identity.status,
    ));
    app.state::<AppState>()
        .managed_agent_supervision_statuses
        .lock()
        .map_err(|error| error.to_string())?
        .insert(runtime_key.clone(), initial_supervision);

    let Some(supervisor_keys) = supervisor_identity.keys else {
        return Ok(());
    };
    use nostr::ToBech32 as _;
    let supervisor_key = Zeroizing::new(
        supervisor_keys
            .secret_key()
            .to_bech32()
            .map_err(|error| format!("encode Runtime supervisor private key: {error}"))?,
    );
    let supervisor_hash = hex::encode(Sha256::digest(supervisor_keys.public_key().to_bytes()));
    let state_path = super::managed_agents_base_dir(app)?
        .join("runtime-supervision")
        .join(format!(
            "{}__{}.json",
            runtime_key.runtime_id(),
            supervisor_hash
        ));
    command
        .env(
            "BUZZ_RUNTIME_SUPERVISOR_PRIVATE_KEY",
            supervisor_key.as_str(),
        )
        .env("BUZZ_RUNTIME_SUPERVISION_STATE_PATH", state_path);
    Ok(())
}

fn is_trusted_runtime_supervisor_harness(command: &str) -> bool {
    let basename = command.rsplit(['/', '\\']).next().unwrap_or(command);
    let stem = basename
        .strip_suffix(".exe")
        .or_else(|| basename.strip_suffix(".EXE"))
        .unwrap_or(basename);
    stem.eq_ignore_ascii_case("buzz-acp") || stem.eq_ignore_ascii_case("buzz_acp")
}

#[tauri::command]
pub fn get_runtime_supervisor_identity_status(
    relay_url: String,
    app: AppHandle,
) -> Result<RuntimeSupervisorIdentityStatus, String> {
    Ok(resolve_identity(&app, &relay_url)?.status)
}

#[tauri::command]
pub async fn prepare_runtime_supervisor_identity(
    relay_url: String,
    agent_pubkey: String,
    app: AppHandle,
) -> Result<RuntimeSupervisorIdentityStatus, String> {
    let relay = fetch_relay_signer(&app, &relay_url).await?;
    let agent = parse_public_key(&agent_pubkey, "managed Agent")?;
    let existing = resolve_identity(&app, &relay_url)?;
    if let Some(resolved) = existing.resolved {
        validate_distinct_identity(&app, resolved.keys.public_key(), relay, agent)?;
        return Ok(resolved.status);
    }
    match existing.status.availability {
        RuntimeSupervisorIdentityAvailability::Missing => {}
        RuntimeSupervisorIdentityAvailability::Locked => {
            return Err(
                "Runtime supervisor keyring is unavailable; unlock it and relaunch before retrying"
                    .to_owned(),
            );
        }
        RuntimeSupervisorIdentityAvailability::Lost => {
            return Err(
                "Runtime supervisor identity is missing from a previously initialized keyring; refusing silent key rotation"
                    .to_owned(),
            );
        }
        RuntimeSupervisorIdentityAvailability::Invalid => {
            return Err(
                "Runtime supervisor identity is invalid; repair or explicitly remove it before creating another"
                    .to_owned(),
            );
        }
        RuntimeSupervisorIdentityAvailability::Ready => {
            return Err("Runtime supervisor identity resolution was inconsistent".to_owned());
        }
    }

    let keys = Keys::generate();
    validate_distinct_identity(&app, keys.public_key(), relay, agent)?;
    persist_identity(&existing.scope, &keys)
}

pub(crate) fn load_runtime_supervisor_identity(
    app: &AppHandle,
    relay_url: &str,
    agent_pubkey: PublicKey,
) -> Result<RuntimeSupervisorSpawnIdentity, String> {
    let resolution = resolve_identity(app, relay_url)?;
    if let Some(resolved) = resolution.resolved {
        let human = app
            .state::<AppState>()
            .keys
            .lock()
            .map_err(|error| error.to_string())?
            .public_key();
        if resolved.keys.public_key() == human || resolved.keys.public_key() == agent_pubkey {
            return Err(
                "Runtime supervisor identity must differ from Human and managed Agent identities"
                    .to_owned(),
            );
        }
        return Ok(RuntimeSupervisorSpawnIdentity {
            keys: Some(resolved.keys),
            status: resolved.status,
        });
    }
    Ok(RuntimeSupervisorSpawnIdentity {
        keys: None,
        status: resolution.status,
    })
}

struct IdentityResolution {
    scope: SupervisorIdentityScope,
    status: RuntimeSupervisorIdentityStatus,
    resolved: Option<ResolvedRuntimeSupervisorIdentity>,
}

struct SupervisorIdentityScope {
    relay_url: String,
    secret_name: String,
    key_path: PathBuf,
    marker_path: PathBuf,
}

fn resolve_identity(app: &AppHandle, relay_url: &str) -> Result<IdentityResolution, String> {
    let scope = identity_scope(app, relay_url)?;
    match std::env::var(SUPERVISOR_OVERRIDE_ENV) {
        Ok(secret) => {
            let secret = Zeroizing::new(secret);
            let keys = Keys::parse(secret.trim())
                .map_err(|error| format!("invalid {SUPERVISOR_OVERRIDE_ENV}: {error}"))?;
            return Ok(ready_resolution(
                scope,
                keys,
                RuntimeSupervisorIdentitySource::Environment,
            ));
        }
        Err(std::env::VarError::NotPresent) => {}
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(format!("{SUPERVISOR_OVERRIDE_ENV} contains invalid UTF-8"));
        }
    }

    let store = crate::secret_store::SecretStore::shared(crate::app_state::keyring_service());
    match store.load(&scope.secret_name) {
        Ok(Some(secret)) => match Keys::parse(secret.trim()) {
            Ok(keys) => {
                return Ok(ready_resolution(
                    scope,
                    keys,
                    RuntimeSupervisorIdentitySource::Keyring,
                ));
            }
            Err(_) => {
                return Ok(unresolved_resolution(
                    scope,
                    RuntimeSupervisorIdentityAvailability::Invalid,
                    "invalid_keyring_value",
                ));
            }
        },
        Ok(None) => {
            if let Some(keys) = load_file_identity(&scope.key_path)? {
                return Ok(ready_resolution(
                    scope,
                    keys,
                    RuntimeSupervisorIdentitySource::RestrictedFile,
                ));
            }
            if scope.marker_path.exists() {
                return Ok(unresolved_resolution(
                    scope,
                    RuntimeSupervisorIdentityAvailability::Lost,
                    "keyring_entry_missing",
                ));
            }
        }
        Err(_) => {
            if let Some(keys) = load_file_identity(&scope.key_path)? {
                return Ok(ready_resolution(
                    scope,
                    keys,
                    RuntimeSupervisorIdentitySource::RestrictedFile,
                ));
            }
            if scope.marker_path.exists() {
                return Ok(unresolved_resolution(
                    scope,
                    RuntimeSupervisorIdentityAvailability::Locked,
                    "keyring_unavailable",
                ));
            }
        }
    }
    Ok(unresolved_resolution(
        scope,
        RuntimeSupervisorIdentityAvailability::Missing,
        "not_prepared",
    ))
}

fn identity_scope(app: &AppHandle, relay_url: &str) -> Result<SupervisorIdentityScope, String> {
    let (relay_url, coordinate) = identity_coordinate(relay_url)?;
    let directory = super::managed_agents_base_dir(app)?
        .join("runtime-supervision")
        .join("identities");
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("create Runtime supervisor identity directory: {error}"))?;
    Ok(SupervisorIdentityScope {
        relay_url,
        secret_name: format!("runtime-supervisor:{coordinate}"),
        key_path: directory.join(format!("{coordinate}.key")),
        marker_path: directory.join(format!("{coordinate}.keyring")),
    })
}

fn identity_coordinate(relay_url: &str) -> Result<(String, String), String> {
    let mut url = Url::parse(relay_url.trim())
        .map_err(|error| format!("invalid Runtime supervisor relay URL: {error}"))?;
    if !matches!(url.scheme(), "ws" | "wss") {
        return Err("Runtime supervisor relay URL scheme must be ws or wss".to_owned());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Runtime supervisor relay URL must not contain credentials".to_owned());
    }
    if url.fragment().is_some() {
        return Err("Runtime supervisor relay URL must not contain a fragment".to_owned());
    }
    let host = url
        .host()
        .ok_or_else(|| "Runtime supervisor relay URL must contain a host".to_owned())?;
    if let Host::Domain(domain) = host {
        url.set_host(Some(&domain.to_ascii_lowercase()))
            .map_err(|_| "Runtime supervisor relay URL host is invalid".to_owned())?;
    }
    let default_port = match url.scheme() {
        "ws" => Some(80),
        "wss" => Some(443),
        _ => None,
    };
    if url.port() == default_port {
        url.set_port(None)
            .map_err(|_| "Runtime supervisor relay URL port is invalid".to_owned())?;
    }
    if url.path() == "/" {
        url.set_path("");
    }
    let relay_url = url.to_string().trim_end_matches('/').to_owned();
    let coordinate = hex::encode(Sha256::digest(relay_url.as_bytes()));
    Ok((relay_url, coordinate))
}

fn ready_resolution(
    scope: SupervisorIdentityScope,
    keys: Keys,
    source: RuntimeSupervisorIdentitySource,
) -> IdentityResolution {
    let status = RuntimeSupervisorIdentityStatus {
        relay_url: scope.relay_url.clone(),
        availability: RuntimeSupervisorIdentityAvailability::Ready,
        public_key: Some(keys.public_key().to_hex()),
        source: Some(source),
        detail_code: None,
    };
    IdentityResolution {
        scope,
        status: status.clone(),
        resolved: Some(ResolvedRuntimeSupervisorIdentity { keys, status }),
    }
}

fn unresolved_resolution(
    scope: SupervisorIdentityScope,
    availability: RuntimeSupervisorIdentityAvailability,
    detail_code: &str,
) -> IdentityResolution {
    IdentityResolution {
        status: RuntimeSupervisorIdentityStatus {
            relay_url: scope.relay_url.clone(),
            availability,
            public_key: None,
            source: None,
            detail_code: Some(detail_code.to_owned()),
        },
        scope,
        resolved: None,
    }
}

fn persist_identity(
    scope: &SupervisorIdentityScope,
    keys: &Keys,
) -> Result<RuntimeSupervisorIdentityStatus, String> {
    let secret = Zeroizing::new(
        keys.secret_key()
            .to_bech32()
            .map_err(|error| format!("encode Runtime supervisor key: {error}"))?,
    );
    let store = crate::secret_store::SecretStore::shared(crate::app_state::keyring_service());
    let stored_in_keyring = store.store(&scope.secret_name, secret.as_str()).is_ok();
    if stored_in_keyring
        && store
            .verify_stored_raw(&scope.secret_name, secret.as_str())
            .unwrap_or(false)
    {
        write_marker(&scope.marker_path)?;
        if let Err(error) = std::fs::remove_file(&scope.key_path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "buzz-desktop: Runtime supervisor keyring write succeeded but stale file cleanup failed: {error}"
                );
            }
        }
        return Ok(RuntimeSupervisorIdentityStatus {
            relay_url: scope.relay_url.clone(),
            availability: RuntimeSupervisorIdentityAvailability::Ready,
            public_key: Some(keys.public_key().to_hex()),
            source: Some(RuntimeSupervisorIdentitySource::Keyring),
            detail_code: None,
        });
    }

    // A write that cannot be read back is not a usable keyring identity. Clear
    // it before falling back to the restricted file so the next launch cannot
    // prefer an unverified entry over the verified fallback.
    if stored_in_keyring {
        store.delete(&scope.secret_name).map_err(|error| {
            format!(
                "clear unverified Runtime supervisor keyring entry before file fallback: {error}"
            )
        })?;
    }

    crate::app_state::save_key_file(&scope.key_path, keys)?;
    let loaded = load_file_identity(&scope.key_path)?
        .ok_or_else(|| "Runtime supervisor file read-back returned no key".to_owned())?;
    if loaded.public_key() != keys.public_key() {
        return Err("Runtime supervisor file read-back verification failed".to_owned());
    }
    Ok(RuntimeSupervisorIdentityStatus {
        relay_url: scope.relay_url.clone(),
        availability: RuntimeSupervisorIdentityAvailability::Ready,
        public_key: Some(keys.public_key().to_hex()),
        source: Some(RuntimeSupervisorIdentitySource::RestrictedFile),
        detail_code: None,
    })
}

fn load_file_identity(path: &Path) -> Result<Option<Keys>, String> {
    match std::fs::read_to_string(path) {
        Ok(secret) => Keys::parse(secret.trim())
            .map(Some)
            .map_err(|error| format!("parse Runtime supervisor identity file: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("read Runtime supervisor identity file: {error}")),
    }
}

fn write_marker(path: &Path) -> Result<(), String> {
    let mut file = atomic_write_file::AtomicWriteFile::open(path)
        .map_err(|error| format!("open Runtime supervisor keyring marker: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("set Runtime supervisor marker permissions: {error}"))?;
    }
    file.write_all(b"keyring-v1\n")
        .map_err(|error| format!("write Runtime supervisor keyring marker: {error}"))?;
    file.commit()
        .map_err(|error| format!("commit Runtime supervisor keyring marker: {error}"))
}

fn validate_distinct_identity(
    app: &AppHandle,
    supervisor: PublicKey,
    relay: PublicKey,
    agent: PublicKey,
) -> Result<(), String> {
    let human = app
        .state::<AppState>()
        .keys
        .lock()
        .map_err(|error| error.to_string())?
        .public_key();
    if supervisor == human || supervisor == relay || supervisor == agent {
        return Err(
            "Runtime supervisor identity must differ from Human, Relay, and managed Agent identities"
                .to_owned(),
        );
    }
    Ok(())
}

fn parse_public_key(value: &str, label: &str) -> Result<PublicKey, String> {
    PublicKey::parse(value.trim()).map_err(|error| format!("invalid {label} public key: {error}"))
}

async fn fetch_relay_signer(app: &AppHandle, relay_url: &str) -> Result<PublicKey, String> {
    let http_url = crate::relay::relay_http_base_url(relay_url);
    let response = app
        .state::<AppState>()
        .http_client
        .get(http_url)
        .header(reqwest::header::ACCEPT, "application/nostr+json")
        .send()
        .await
        .map_err(|error| format!("fetch Relay identity: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "fetch Relay identity returned HTTP {}",
            response.status()
        ));
    }
    let document: serde_json::Value = response
        .json()
        .await
        .map_err(|error| format!("decode Relay identity: {error}"))?;
    let relay_pubkey = document
        .get("self")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "Relay identity document does not advertise `self`".to_owned())?;
    parse_public_key(relay_pubkey, "Relay signer")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_coordinate_preserves_host_scoped_community_authority() {
        let localhost = identity_coordinate("ws://localhost:3000")
            .expect("normalize localhost identity coordinate");
        let loopback = identity_coordinate("ws://127.0.0.1:3000/")
            .expect("normalize loopback identity coordinate");
        assert_eq!(localhost.0, "ws://localhost:3000");
        assert_eq!(loopback.0, "ws://127.0.0.1:3000");
        assert_ne!(localhost.1, loopback.1);
        assert_eq!(
            identity_coordinate("WSS://Relay.Example:443/")
                .expect("normalize remote relay identity")
                .0,
            "wss://relay.example"
        );
    }

    #[test]
    fn public_identity_status_never_serializes_secret_material() {
        let status = RuntimeSupervisorIdentityStatus {
            relay_url: "ws://127.0.0.1:3000".to_owned(),
            availability: RuntimeSupervisorIdentityAvailability::Ready,
            public_key: Some("11".repeat(32)),
            source: Some(RuntimeSupervisorIdentitySource::RestrictedFile),
            detail_code: None,
        };
        let encoded = serde_json::to_value(status).expect("serialize public identity status");
        assert_eq!(
            encoded.get("publicKey").and_then(serde_json::Value::as_str),
            Some("1111111111111111111111111111111111111111111111111111111111111111")
        );
        assert!(encoded.get("privateKey").is_none());
        assert!(encoded.get("secretKey").is_none());
    }

    #[test]
    fn supervisor_secret_is_limited_to_the_buzz_acp_boundary() {
        assert!(is_trusted_runtime_supervisor_harness("buzz-acp"));
        assert!(is_trusted_runtime_supervisor_harness("/opt/buzz/buzz_acp"));
        assert!(is_trusted_runtime_supervisor_harness(
            "C:\\Buzz\\buzz-acp.exe"
        ));
        assert!(!is_trusted_runtime_supervisor_harness("codex-acp"));
        assert!(!is_trusted_runtime_supervisor_harness(
            "/tmp/custom-harness"
        ));
    }
}
