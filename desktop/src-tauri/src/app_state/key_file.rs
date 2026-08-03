//! Restricted identity-file persistence shared by Desktop identity stores.

use std::io::Write as _;

use nostr::{Keys, ToBech32 as _};

/// Atomically write the key to disk. Uses `atomic-write-file` which:
/// 1. Writes to a temp file in the same directory
/// 2. Calls fsync on the file
/// 3. Renames temp to target (atomic on POSIX, best-effort on Windows)
/// 4. Calls fsync on the parent directory
///
/// On Unix, the file is created with mode 0600 (owner read/write only).
/// On Windows, default ACLs apply because the app data directory is already
/// per-user.
pub(crate) fn save_key_file(path: &std::path::Path, keys: &Keys) -> Result<(), String> {
    use atomic_write_file::AtomicWriteFile;

    let nsec = keys
        .secret_key()
        .to_bech32()
        .map_err(|error| format!("encode nsec: {error}"))?;
    let mut file = AtomicWriteFile::open(path)
        .map_err(|error| format!("open identity.key for atomic write: {error}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("set identity.key permissions: {error}"))?;
    }

    file.write_all(nsec.as_bytes())
        .map_err(|error| format!("write identity.key: {error}"))?;
    file.commit()
        .map_err(|error| format!("commit identity.key: {error}"))
}
