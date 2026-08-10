use std::fs;
use std::path::Path;

#[cfg(unix)]
use crate::util::create_symlink;

/// Returns the `~/.local/bin` link name for the bundled CLI.
///
/// Dev builds use a separate convenience link so a release and a local build
/// never overwrite each other's terminal entry point.
pub fn cli_link_name(is_dev: bool) -> &'static str {
    if is_dev {
        "cf-dev"
    } else {
        "cf"
    }
}

/// Ensures `~/.local/bin/cf` (prod) or `~/.local/bin/cf-dev` (dev) points to
/// the app-owned CLI sidecar.
///
/// Existing files and arbitrary symlinks are never overwritten because `cf`
/// may already belong to another tool. Legacy `buzz` links are removed only
/// when they point exactly at this app's former bundled binary.
#[cfg(unix)]
pub fn ensure_cli_symlink(exe_parent: &Path, is_dev: bool) -> Result<(), String> {
    let cf_bin = exe_parent.join("cf");
    if !cf_bin.exists() {
        return Ok(());
    }

    let local_bin = dirs::home_dir()
        .ok_or("cannot resolve home directory")?
        .join(".local")
        .join("bin");
    ensure_cli_symlink_at(exe_parent, &local_bin, is_dev)
}

#[cfg(unix)]
pub(super) fn ensure_cli_symlink_at(
    exe_parent: &Path,
    local_bin: &Path,
    is_dev: bool,
) -> Result<(), String> {
    let cf_bin = exe_parent.join("cf");
    fs::create_dir_all(local_bin).map_err(|e| format!("create {}: {e}", local_bin.display()))?;

    let legacy_target = exe_parent.join("buzz");
    for legacy_name in ["buzz", "buzz-dev"] {
        let legacy_link = local_bin.join(legacy_name);
        let Ok(metadata) = legacy_link.symlink_metadata() else {
            continue;
        };
        if metadata.file_type().is_symlink()
            && fs::read_link(&legacy_link).ok().as_deref() == Some(legacy_target.as_path())
        {
            fs::remove_file(&legacy_link)
                .map_err(|e| format!("remove legacy symlink {}: {e}", legacy_link.display()))?;
        }
    }

    let link = local_bin.join(cli_link_name(is_dev));
    match link.symlink_metadata() {
        Ok(meta) if meta.file_type().is_symlink() => {
            let existing_target = fs::read_link(&link)
                .map_err(|e| format!("read symlink {}: {e}", link.display()))?;
            if existing_target != cf_bin {
                return Err(format!(
                    "{} already exists as a symlink not owned by Carryforth",
                    link.display()
                ));
            }
        }
        Ok(_) => {
            return Err(format!(
                "{} already exists and is not owned by Carryforth",
                link.display()
            ));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            create_symlink(&cf_bin, &link)
                .map_err(|e| format!("symlink {}: {e}", link.display()))?;
        }
        Err(e) => return Err(format!("stat {}: {e}", link.display())),
    }

    Ok(())
}

/// No-op on non-Unix platforms — symlink management is macOS/Linux only.
#[cfg(not(unix))]
pub fn ensure_cli_symlink(_exe_parent: &Path, _is_dev: bool) -> Result<(), String> {
    Ok(())
}
