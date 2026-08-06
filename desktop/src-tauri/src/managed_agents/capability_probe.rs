//! Bounded probes for capabilities exposed by the exact configured ACP harness.
//!
//! A managed Agent record may point at a custom ACP command. Desktop must not
//! infer Meeting compatibility from the record type or from another binary on
//! `PATH`; it probes the executable that the runtime would actually spawn.

use std::io::{Read as _, Seek as _, SeekFrom};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const CAPABILITY_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PROBE_OUTPUT_BYTES: u64 = 1024 * 1024;

/// Result of probing an ACP harness for the Meeting V2 direct-action contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MeetingCapabilityProbe {
    /// The harness explicitly listed the required capability.
    Supported,
    /// The harness returned a valid capability snapshot without the required
    /// capability. This is authoritative enough to withdraw a stale claim.
    Unsupported,
    /// The harness could not provide a trustworthy snapshot. Existing Relay
    /// state must be left untouched until a later probe succeeds.
    Unknown(String),
}

fn classify_capability_document(document: &str) -> MeetingCapabilityProbe {
    let value: serde_json::Value = match serde_json::from_str(document) {
        Ok(value) => value,
        Err(error) => {
            return MeetingCapabilityProbe::Unknown(format!(
                "ACP capability probe returned invalid JSON: {error}"
            ));
        }
    };
    let Some(capabilities) = value
        .get("meeting")
        .and_then(|meeting| meeting.get("capabilities"))
        .and_then(serde_json::Value::as_array)
    else {
        return MeetingCapabilityProbe::Unknown(
            "ACP capability probe omitted meeting.capabilities".to_string(),
        );
    };
    if capabilities
        .iter()
        .any(|capability| capability.as_str() == Some(buzz_sdk_pkg::MEETING_V2_ACTIONS_CAPABILITY))
    {
        MeetingCapabilityProbe::Supported
    } else if capabilities.iter().all(serde_json::Value::is_string) {
        MeetingCapabilityProbe::Unsupported
    } else {
        MeetingCapabilityProbe::Unknown(
            "ACP capability probe returned a non-string capability".to_string(),
        )
    }
}

/// Probe the exact ACP command configured on a local managed Agent.
///
/// The child is bounded and writes to regular temporary files so a forked
/// descendant cannot keep a stdout/stderr pipe open after the probe exits.
pub(crate) fn probe_meeting_capability(acp_command: &str) -> MeetingCapabilityProbe {
    let Some(binary_path) = super::resolve_command(acp_command) else {
        return MeetingCapabilityProbe::Unknown(format!(
            "configured ACP harness {acp_command:?} could not be resolved"
        ));
    };
    let mut stdout = match tempfile::tempfile() {
        Ok(file) => file,
        Err(error) => {
            return MeetingCapabilityProbe::Unknown(format!(
                "could not allocate ACP capability probe output: {error}"
            ));
        }
    };
    let mut stderr = match tempfile::tempfile() {
        Ok(file) => file,
        Err(error) => {
            return MeetingCapabilityProbe::Unknown(format!(
                "could not allocate ACP capability probe error output: {error}"
            ));
        }
    };

    let mut command = Command::new(&binary_path);
    command.args(["capabilities", "--json"]);
    command.stdin(Stdio::null());
    let stdout_clone = match stdout.try_clone() {
        Ok(file) => file,
        Err(error) => {
            return MeetingCapabilityProbe::Unknown(format!(
                "could not prepare ACP capability probe output: {error}"
            ));
        }
    };
    let stderr_clone = match stderr.try_clone() {
        Ok(file) => file,
        Err(error) => {
            return MeetingCapabilityProbe::Unknown(format!(
                "could not prepare ACP capability probe error output: {error}"
            ));
        }
    };
    command.stdout(Stdio::from(stdout_clone));
    command.stderr(Stdio::from(stderr_clone));
    if let Some(path) = super::readiness::cli_probe::augmented_path() {
        command.env("PATH", path);
    }
    crate::util::configure_no_window(&mut command);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return MeetingCapabilityProbe::Unknown(format!(
                "could not run ACP capability probe at {}: {error}",
                binary_path.display()
            ));
        }
    };
    let deadline = Instant::now() + CAPABILITY_PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return MeetingCapabilityProbe::Unknown(format!(
                    "ACP capability probe timed out after {} seconds",
                    CAPABILITY_PROBE_TIMEOUT.as_secs()
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return MeetingCapabilityProbe::Unknown(format!(
                    "could not wait for ACP capability probe: {error}"
                ));
            }
        }
    };

    let read_bounded = |file: &mut std::fs::File| -> Result<String, String> {
        file.seek(SeekFrom::Start(0))
            .map_err(|error| format!("seek probe output: {error}"))?;
        let mut bytes = Vec::new();
        file.take(MAX_PROBE_OUTPUT_BYTES)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("read probe output: {error}"))?;
        String::from_utf8(bytes).map_err(|error| format!("probe output was not UTF-8: {error}"))
    };

    if !status.success() {
        let detail = read_bounded(&mut stderr)
            .unwrap_or_default()
            .trim()
            .lines()
            .next()
            .unwrap_or("")
            .to_string();
        return MeetingCapabilityProbe::Unknown(if detail.is_empty() {
            format!("ACP capability probe exited with {status}")
        } else {
            format!("ACP capability probe exited with {status}: {detail}")
        });
    }

    match read_bounded(&mut stdout) {
        Ok(document) => classify_capability_document(&document),
        Err(error) => MeetingCapabilityProbe::Unknown(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_document_distinguishes_supported_unsupported_and_unknown() {
        assert_eq!(
            classify_capability_document(
                &serde_json::json!({
                    "meeting": {
                        "capabilities": [buzz_sdk_pkg::MEETING_V2_ACTIONS_CAPABILITY]
                    }
                })
                .to_string()
            ),
            MeetingCapabilityProbe::Supported,
        );
        assert_eq!(
            classify_capability_document(r#"{"meeting":{"capabilities":[]}}"#),
            MeetingCapabilityProbe::Unsupported,
        );
        assert!(matches!(
            classify_capability_document(r#"{"meeting":{}}"#),
            MeetingCapabilityProbe::Unknown(_)
        ));
        assert!(matches!(
            classify_capability_document("not-json"),
            MeetingCapabilityProbe::Unknown(_)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn probe_executes_the_exact_configured_harness() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().expect("temp dir");
        let harness = temp.path().join("custom-acp");
        std::fs::write(
            &harness,
            format!(
                "#!/bin/sh\nprintf '%s\\n' '{{\"meeting\":{{\"capabilities\":[\"{}\"]}}}}'\n",
                buzz_sdk_pkg::MEETING_V2_ACTIONS_CAPABILITY
            ),
        )
        .expect("write harness");
        let mut permissions = std::fs::metadata(&harness)
            .expect("harness metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&harness, permissions).expect("make harness executable");

        assert_eq!(
            probe_meeting_capability(harness.to_str().expect("UTF-8 harness path")),
            MeetingCapabilityProbe::Supported,
        );
    }
}
