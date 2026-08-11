//! Environment-variable names shared by Desktop, ACP, and the `cf` CLI.
//!
//! The values intentionally have separate responsibilities: Desktop ownership
//! identifies which app instance may reap a process, while managed Runtime mode
//! enables stricter Role and Project View behavior in the CLI.

/// Desktop instance ownership marker propagated unchanged through an Agent tree.
pub const MANAGED_AGENT_OWNER_ENV: &str = "BUZZ_MANAGED_AGENT";

/// Boolean (`"1"`) marker enabling managed Runtime behavior in Agent-facing CLIs.
pub const MANAGED_RUNTIME_MODE_ENV: &str = "BUZZ_MANAGED_RUNTIME";

/// Per-harness generation nonce used for managed Runtime lifecycle receipts.
pub const MANAGED_AGENT_START_NONCE_ENV: &str = "BUZZ_MANAGED_AGENT_START_NONCE";
