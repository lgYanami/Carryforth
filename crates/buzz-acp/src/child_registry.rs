//! Durable registry for model-facing child process groups owned by a managed
//! ACP harness.
//!
//! Runtime maintenance cannot acknowledge an Assignment merely because the
//! current Rust task stopped. Every persistent Agent child (and the MCP/tool
//! processes in its process group) must be gone first. This registry survives
//! a harness crash so the next trusted generation can kill and verify stale
//! process groups before it polls or acknowledges maintenance.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use atomic_write_file::AtomicWriteFile;
use serde::{Deserialize, Serialize};
use sysinfo::{ProcessRefreshKind, RefreshKind, Signal, System, UpdateKind};

pub(crate) const CHILD_REGISTRY_TOKEN_ENV: &str = "BUZZ_ACP_CHILD_REGISTRY_TOKEN";
const REGISTRY_SCHEMA_VERSION: u16 = 2;
const REGISTER_IDENTITY_TIMEOUT: Duration = Duration::from_secs(2);
const REGISTER_IDENTITY_POLL_INTERVAL: Duration = Duration::from_millis(10);
const REAP_TIMEOUT: Duration = Duration::from_secs(5);
const REAP_POLL_INTERVAL: Duration = Duration::from_millis(50);

static REGISTRY: OnceLock<ChildProcessRegistry> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryDocument {
    schema_version: u16,
    process_groups: BTreeMap<u32, RegisteredProcessGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegisteredProcessGroup {
    leader_start_time: u64,
    token: String,
}

impl Default for RegistryDocument {
    fn default() -> Self {
        Self {
            schema_version: REGISTRY_SCHEMA_VERSION,
            process_groups: BTreeMap::new(),
        }
    }
}

#[derive(Debug)]
struct ChildProcessRegistry {
    path: PathBuf,
    process_groups: Mutex<BTreeMap<u32, RegisteredProcessGroup>>,
}

/// Configure the pair-scoped registry before any model-facing child is
/// spawned. Calling this more than once is accepted only for the same path.
pub(crate) fn configure(path: PathBuf) -> Result<(), String> {
    if !path.is_absolute() || path.to_str().is_none() {
        return Err("child process registry path must be absolute UTF-8".to_owned());
    }
    if let Some(existing) = REGISTRY.get() {
        return if existing.path == path {
            Ok(())
        } else {
            Err("child process registry was already configured for another Runtime".to_owned())
        };
    }
    let document = read_document(&path)?;
    for (process_group, entry) in &document.process_groups {
        if *process_group == 0
            || entry.leader_start_time == 0
            || !valid_registry_token(&entry.token)
        {
            return Err(format!(
                "child process registry {} contains an invalid identity",
                path.display()
            ));
        }
    }
    REGISTRY
        .set(ChildProcessRegistry {
            path,
            process_groups: Mutex::new(document.process_groups),
        })
        .map_err(|_| "child process registry initialization raced".to_owned())
}

/// Record a newly spawned Agent process group before it can be handed to the
/// pool. The child PID is also its PGID (`process_group(0)` at spawn).
pub(crate) async fn register(process_group: u32, token: &str) -> Result<(), String> {
    if process_group == 0 {
        return Err("child process group must be positive".to_owned());
    }
    if !valid_registry_token(token) {
        return Err("child process registry token is malformed".to_owned());
    }
    let Some(registry) = REGISTRY.get() else {
        return Ok(());
    };
    let entry = observe_spawned_identity(process_group, token).await?;
    let mut groups = registry
        .process_groups
        .lock()
        .map_err(|_| "child process registry lock was poisoned".to_owned())?;
    if groups.contains_key(&process_group) {
        return Err(format!(
            "child process group {process_group} is already registered"
        ));
    }
    groups.insert(process_group, entry);
    if let Err(error) = persist(&registry.path, &groups) {
        groups.remove(&process_group);
        return Err(error);
    }
    Ok(())
}

async fn observe_spawned_identity(
    process_group: u32,
    token: &str,
) -> Result<RegisteredProcessGroup, String> {
    let deadline = tokio::time::Instant::now() + REGISTER_IDENTITY_TIMEOUT;
    loop {
        let processes = process_snapshot();
        if let Some(leader) = processes.process(sysinfo::Pid::from_u32(process_group)) {
            if leader.start_time() != 0 && process_has_token(leader, token) {
                return Ok(RegisteredProcessGroup {
                    leader_start_time: leader.start_time(),
                    token: token.to_owned(),
                });
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "spawned Agent process {process_group} lacks its registry identity after bounded observation"
            ));
        }
        tokio::time::sleep(REGISTER_IDENTITY_POLL_INTERVAL).await;
    }
}

/// Remove a process group only after its direct child has been waited and the
/// OS proves that no group member or token-bearing descendant remains. An
/// uncertain survivor stays registered for the next cleanup proof.
pub(crate) fn unregister_reaped(process_group: u32) -> Result<(), String> {
    let Some(registry) = REGISTRY.get() else {
        return Ok(());
    };
    let mut groups = registry
        .process_groups
        .lock()
        .map_err(|_| "child process registry lock was poisoned".to_owned())?;
    let Some(entry) = groups.get(&process_group) else {
        return Ok(());
    };
    if registered_group_survives(process_group, entry) {
        return Err(format!(
            "owned Agent process group {process_group} still has a live process"
        ));
    }
    let entry = groups
        .remove(&process_group)
        .ok_or_else(|| "child process registry entry disappeared".to_owned())?;
    if let Err(error) = persist(&registry.path, &groups) {
        groups.insert(process_group, entry);
        return Err(error);
    }
    Ok(())
}

pub(crate) fn is_configured() -> bool {
    REGISTRY.get().is_some()
}

/// Signal one exact registered group after validating its durable leader/token
/// identity. Callers must not fall back to a raw PGID signal when this rejects
/// an uncertain or leaderless group.
pub(crate) fn signal_current_process_group(process_group: u32) -> Result<(), String> {
    let registry = REGISTRY
        .get()
        .ok_or_else(|| "child process registry is not configured".to_owned())?;
    let entry = registry
        .process_groups
        .lock()
        .map_err(|_| "child process registry lock was poisoned".to_owned())?
        .get(&process_group)
        .cloned()
        .ok_or_else(|| format!("Agent process group {process_group} is not registered"))?;
    signal_registered_group(process_group, &entry)
}

/// Kill and prove absence of one exact registered Agent process group.
///
/// This is the panic-path counterpart to [`crate::acp::AcpClient::shutdown`]:
/// task unwinding has already dropped the owned `Child`, so the main harness
/// can only recover through the durable identity recorded here. The wait is
/// bounded; callers must fail closed (normally by exiting the harness) when
/// confirmation cannot be obtained.
pub(crate) async fn reap_registered_process_group(process_group: u32) -> Result<(), String> {
    let registry = REGISTRY
        .get()
        .ok_or_else(|| "child process registry is not configured".to_owned())?;
    let entry = registry
        .process_groups
        .lock()
        .map_err(|_| "child process registry lock was poisoned".to_owned())?
        .get(&process_group)
        .cloned();

    let Some(entry) = entry else {
        // Absence from a configured durable registry is itself the proof that
        // this harness no longer owns the coordinate. Do not consult the raw
        // PGID here: an unrelated process group may have reused it after the
        // registered generation exited.
        return Ok(());
    };

    signal_registered_group(process_group, &entry)?;
    let deadline = tokio::time::Instant::now() + REAP_TIMEOUT;
    loop {
        terminate_marked_processes(&entry)?;
        if !registered_group_survives(process_group, &entry) {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "owned Agent process group {process_group} survived forced cleanup"
            ));
        }
        tokio::time::sleep(REAP_POLL_INTERVAL).await;
    }

    let mut registered = registry
        .process_groups
        .lock()
        .map_err(|_| "child process registry lock was poisoned".to_owned())?;
    registered.remove(&process_group);
    persist(&registry.path, &registered)
}

/// Kill and prove absence of every process group left by a previous harness
/// generation. This must run before maintenance polling or Runtime admission.
pub(crate) async fn reap_previous_generation() -> Result<(), String> {
    let Some(registry) = REGISTRY.get() else {
        return Ok(());
    };
    let groups = registry
        .process_groups
        .lock()
        .map_err(|_| "child process registry lock was poisoned".to_owned())?
        .clone();
    if groups.is_empty() {
        return Ok(());
    }

    for (process_group, entry) in &groups {
        signal_registered_group(*process_group, entry)?;
    }
    let deadline = tokio::time::Instant::now() + REAP_TIMEOUT;
    loop {
        let mut surviving = Vec::new();
        for (process_group, entry) in &groups {
            terminate_marked_processes(entry)?;
            if registered_group_survives(*process_group, entry) {
                surviving.push(*process_group);
            }
        }
        if surviving.is_empty() {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "owned Agent process groups survived forced cleanup: {surviving:?}"
            ));
        }
        tokio::time::sleep(REAP_POLL_INTERVAL).await;
    }

    let mut registered = registry
        .process_groups
        .lock()
        .map_err(|_| "child process registry lock was poisoned".to_owned())?;
    for process_group in groups.keys() {
        registered.remove(process_group);
    }
    persist(&registry.path, &registered)
}

/// True only when no current or crash-retained child coordinate remains.
pub(crate) fn is_empty() -> Result<bool, String> {
    let Some(registry) = REGISTRY.get() else {
        return Ok(true);
    };
    registry
        .process_groups
        .lock()
        .map(|groups| groups.is_empty())
        .map_err(|_| "child process registry lock was poisoned".to_owned())
}

fn read_document(path: &Path) -> Result<RegistryDocument, String> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let document: RegistryDocument = serde_json::from_slice(&bytes).map_err(|error| {
                format!("invalid child process registry {}: {error}", path.display())
            })?;
            if document.schema_version != REGISTRY_SCHEMA_VERSION {
                return Err(format!(
                    "child process registry {} has unsupported schema {}",
                    path.display(),
                    document.schema_version
                ));
            }
            Ok(document)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(RegistryDocument::default())
        }
        Err(error) => Err(format!(
            "read child process registry {}: {error}",
            path.display()
        )),
    }
}

fn persist(
    path: &Path,
    process_groups: &BTreeMap<u32, RegisteredProcessGroup>,
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "child process registry path has no parent".to_owned())?;
    std::fs::create_dir_all(parent).map_err(|error| {
        format!(
            "create child process registry directory {}: {error}",
            parent.display()
        )
    })?;
    let payload = serde_json::to_vec(&RegistryDocument {
        schema_version: REGISTRY_SCHEMA_VERSION,
        process_groups: process_groups.clone(),
    })
    .map_err(|error| format!("serialize child process registry: {error}"))?;
    let mut file = AtomicWriteFile::open(path)
        .map_err(|error| format!("open child process registry {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|error| {
                format!(
                    "set child process registry permissions {}: {error}",
                    path.display()
                )
            })?;
    }
    file.write_all(&payload)
        .map_err(|error| format!("write child process registry {}: {error}", path.display()))?;
    file.commit()
        .map_err(|error| format!("commit child process registry {}: {error}", path.display()))
}

fn process_snapshot() -> System {
    System::new_with_specifics(
        RefreshKind::nothing().with_processes(
            ProcessRefreshKind::nothing()
                .with_environ(UpdateKind::Always)
                .without_tasks(),
        ),
    )
}

fn process_has_token(process: &sysinfo::Process, token: &str) -> bool {
    let expected = format!("{CHILD_REGISTRY_TOKEN_ENV}={token}");
    process
        .environ()
        .iter()
        .any(|value| value.as_os_str() == OsStr::new(&expected))
}

fn valid_registry_token(token: &str) -> bool {
    token.len() == 32
        && token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn matching_processes(entry: &RegisteredProcessGroup) -> Vec<sysinfo::Pid> {
    process_snapshot()
        .processes()
        .iter()
        .filter_map(|(pid, process)| process_has_token(process, &entry.token).then_some(*pid))
        .collect()
}

fn terminate_marked_processes(entry: &RegisteredProcessGroup) -> Result<(), String> {
    let processes = process_snapshot();
    for process in processes
        .processes()
        .values()
        .filter(|process| process_has_token(process, &entry.token))
    {
        match process.kill_with(Signal::Kill) {
            Some(true) | Some(false) => {}
            None => {
                return Err(format!(
                    "kill owned Agent process {} from durable registry",
                    process.pid()
                ));
            }
        }
    }
    Ok(())
}

fn signal_registered_group(
    process_group: u32,
    entry: &RegisteredProcessGroup,
) -> Result<(), String> {
    let processes = process_snapshot();
    if let Some(leader) = processes.process(sysinfo::Pid::from_u32(process_group)) {
        if leader.start_time() != entry.leader_start_time {
            return terminate_marked_processes(entry);
        }
        if !process_has_token(leader, &entry.token) {
            return Err(format!(
                "refusing to kill process group {process_group}: its durable identity cannot be proven"
            ));
        }
        #[cfg(unix)]
        return terminate_process_group(process_group);
        #[cfg(not(unix))]
        return terminate_marked_processes(entry);
    }
    if matching_processes(entry).is_empty() {
        #[cfg(unix)]
        if process_group_exists(process_group) {
            return Err(format!(
                "refusing to kill leaderless process group {process_group}: its durable identity cannot be proven"
            ));
        }
        return Ok(());
    }
    terminate_marked_processes(entry)
}

fn registered_group_survives(process_group: u32, entry: &RegisteredProcessGroup) -> bool {
    let processes = process_snapshot();
    if let Some(leader) = processes.process(sysinfo::Pid::from_u32(process_group)) {
        if leader.start_time() != entry.leader_start_time {
            return processes
                .processes()
                .values()
                .any(|process| process_has_token(process, &entry.token));
        }
        return true;
    }
    if !matching_processes(entry).is_empty() {
        return true;
    }
    #[cfg(unix)]
    return process_group_exists(process_group);
    #[cfg(not(unix))]
    false
}

#[cfg(unix)]
fn terminate_process_group(process_group: u32) -> Result<(), String> {
    use nix::errno::Errno;
    use nix::sys::signal::{killpg, Signal};
    use nix::unistd::Pid;

    match killpg(Pid::from_raw(process_group as i32), Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(error) => Err(format!(
            "kill owned Agent process group {process_group}: {error}"
        )),
    }
}

#[cfg(unix)]
fn process_group_exists(process_group: u32) -> bool {
    use nix::errno::Errno;
    use nix::sys::signal::killpg;
    use nix::unistd::Pid;

    match killpg(Pid::from_raw(process_group as i32), None) {
        Ok(()) | Err(Errno::EPERM) => true,
        Err(Errno::ESRCH) => false,
        Err(_) => true,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spawned_group_identity_is_observable_and_reapable() {
        let token = uuid::Uuid::new_v4().simple().to_string();
        let mut command = tokio::process::Command::new("sh");
        command
            .arg("-c")
            .arg("sleep 30")
            .env(CHILD_REGISTRY_TOKEN_ENV, &token)
            .process_group(0);
        let mut child = command.spawn().expect("spawn registry fixture");
        let process_group = child.id().expect("fixture process ID");
        let entry = observe_spawned_identity(process_group, &token)
            .await
            .expect("observe fixture identity");
        signal_registered_group(process_group, &entry).expect("signal owned process group");
        tokio::time::timeout(REAP_TIMEOUT, child.wait())
            .await
            .expect("bounded child wait")
            .expect("reap registry fixture");
        assert!(!registered_group_survives(process_group, &entry));
    }

    #[tokio::test]
    async fn leaderless_token_descendant_is_terminated_without_raw_group_identity() {
        let token = uuid::Uuid::new_v4().simple().to_string();
        let mut command = tokio::process::Command::new("sh");
        command
            .arg("-c")
            .arg("sleep 30 & sleep 0.5")
            .env(CHILD_REGISTRY_TOKEN_ENV, &token)
            .process_group(0);
        let mut child = command.spawn().expect("spawn leaderless registry fixture");
        let process_group = child.id().expect("fixture process ID");
        let entry = observe_spawned_identity(process_group, &token)
            .await
            .expect("observe fixture leader identity");

        tokio::time::timeout(Duration::from_secs(2), child.wait())
            .await
            .expect("fixture leader exits")
            .expect("reap fixture leader");
        assert!(
            !matching_processes(&entry).is_empty(),
            "token-bearing descendant must outlive the process-group leader"
        );

        // With no leader, this must kill only token-matching descendants. It
        // must not issue raw killpg against a potentially reused PGID.
        signal_registered_group(process_group, &entry)
            .expect("signal exact token-bearing descendants");
        let deadline = tokio::time::Instant::now() + REAP_TIMEOUT;
        while registered_group_survives(process_group, &entry) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "leaderless token descendant survived exact cleanup"
            );
            terminate_marked_processes(&entry).expect("repeat exact descendant kill");
            tokio::time::sleep(REAP_POLL_INTERVAL).await;
        }
    }
}
