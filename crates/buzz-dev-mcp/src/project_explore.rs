//! Bounded, read-only project exploration tools.
//!
//! These tools deliberately perform filesystem operations in-process instead
//! of delegating to a shell. Every target is canonicalized and confined to the
//! requested workdir, which is itself confined to the MCP server's initial cwd.

use crate::shell::SharedState;
use ignore::WalkBuilder;
use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::Deserialize;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

const DEFAULT_LIST_DEPTH: usize = 2;
const MAX_LIST_DEPTH: usize = 10;
const DEFAULT_LIST_ENTRIES: usize = 200;
const MAX_LIST_ENTRIES: usize = 1_000;
const DEFAULT_SEARCH_RESULTS: usize = 200;
const MAX_SEARCH_RESULTS: usize = 500;
const DEFAULT_SEARCH_DEPTH: usize = 25;
const MAX_SEARCH_DEPTH: usize = 50;
const MAX_FILES_SCANNED: usize = 20_000;
const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_QUERY_BYTES: usize = 1_024;
const MAX_LINE_BYTES: usize = 256 * 1024;
const MAX_RENDERED_LINE_BYTES: usize = 2 * 1024;
const MAX_OUTPUT_BYTES: usize = 50 * 1024;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
/// Bounded parameters for listing files under a project workdir.
pub struct ListDirectoryParams {
    /// Directory to list, relative to workdir. Defaults to `.`.
    #[serde(default)]
    pub path: Option<String>,
    /// Recursive depth. Defaults to 2 and is capped at 10.
    #[serde(default)]
    pub depth: Option<usize>,
    /// Maximum number of returned entries. Defaults to 200 and is capped at 1000.
    #[serde(default)]
    pub max_entries: Option<usize>,
    /// Exploration root, relative to the server workspace. Defaults to server cwd.
    #[serde(default)]
    pub workdir: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
/// Bounded parameters for a literal-text search under a project workdir.
pub struct SearchTextParams {
    /// Literal text to find. Regular expressions are intentionally not evaluated.
    pub query: String,
    /// File or directory to search, relative to workdir. Defaults to `.`.
    #[serde(default)]
    pub path: Option<String>,
    /// Whether matching is case-sensitive. Defaults to true.
    #[serde(default)]
    pub case_sensitive: Option<bool>,
    /// Recursive search depth. Defaults to 25 and is capped at 50.
    #[serde(default)]
    pub depth: Option<usize>,
    /// Maximum number of matching lines. Defaults to 200 and is capped at 500.
    #[serde(default)]
    pub max_results: Option<usize>,
    /// Exploration root, relative to the server workspace. Defaults to server cwd.
    #[serde(default)]
    pub workdir: Option<String>,
}

/// List files and directories without allowing traversal outside the selected workdir.
pub fn list_directory(
    state: &SharedState,
    params: ListDirectoryParams,
) -> Result<String, ErrorData> {
    let path = params.path.as_deref().unwrap_or(".");
    let depth = bounded_positive(params.depth, DEFAULT_LIST_DEPTH, MAX_LIST_DEPTH, "depth")?;
    let max_entries = bounded_positive(
        params.max_entries,
        DEFAULT_LIST_ENTRIES,
        MAX_LIST_ENTRIES,
        "max_entries",
    )?;
    let (workdir, target) =
        crate::paths::resolve_exploration_path(state, path, params.workdir.as_deref())?;
    if !target.is_dir() {
        return Err(ErrorData::invalid_params(
            format!("not a directory: {}", target.display()),
            None,
        ));
    }

    let mut builder = project_walker(&target, depth);
    builder.sort_by_file_name(|left, right| left.cmp(right));

    let mut output = Output::new();
    let target_label = relative_label(&workdir, &target);
    output.push_required(&format!("{target_label}/"))?;

    let mut returned = 0usize;
    let mut truncated = false;
    for entry in builder.build() {
        let Ok(entry) = entry else {
            continue;
        };
        if entry.depth() == 0 {
            continue;
        }
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if returned >= max_entries {
            truncated = true;
            break;
        }

        let label = relative_label(&workdir, entry.path());
        let suffix = if file_type.is_dir() { "/" } else { "" };
        if output.push(&format!("{label}{suffix}")).is_err() {
            truncated = true;
            break;
        }
        returned += 1;
    }

    if truncated {
        output.push_truncation(&format!(
            "[truncated: returned {returned} entries; narrow path or increase max_entries]"
        ));
    }
    Ok(output.finish())
}

/// Search project files for literal text without invoking a shell.
pub fn search_text(state: &SharedState, params: SearchTextParams) -> Result<String, ErrorData> {
    validate_query(&params.query)?;
    let path = params.path.as_deref().unwrap_or(".");
    let depth = bounded_positive(
        params.depth,
        DEFAULT_SEARCH_DEPTH,
        MAX_SEARCH_DEPTH,
        "depth",
    )?;
    let max_results = bounded_positive(
        params.max_results,
        DEFAULT_SEARCH_RESULTS,
        MAX_SEARCH_RESULTS,
        "max_results",
    )?;
    let case_sensitive = params.case_sensitive.unwrap_or(true);
    let needle = if case_sensitive {
        params.query.clone()
    } else {
        params.query.to_lowercase()
    };
    let (workdir, target) =
        crate::paths::resolve_exploration_path(state, path, params.workdir.as_deref())?;

    let mut output = Output::new();
    let target_label = relative_label(&workdir, &target);
    output.push_required(&format!(
        "Literal matches for {:?} under {target_label}:",
        params.query
    ))?;

    let mut matches = 0usize;
    let mut files_scanned = 0usize;
    let mut truncated = false;
    let mut builder = project_walker(&target, depth);
    builder.sort_by_file_name(|left, right| left.cmp(right));

    for entry in builder.build() {
        let Ok(entry) = entry else {
            continue;
        };
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        if files_scanned >= MAX_FILES_SCANNED {
            truncated = true;
            break;
        }
        files_scanned += 1;

        let remaining = max_results.saturating_sub(matches);
        if remaining == 0 {
            truncated = true;
            break;
        }
        let result = scan_file(
            entry.path(),
            &workdir,
            &needle,
            case_sensitive,
            remaining,
            &mut output,
        );
        match result {
            ScanOutcome::Complete(count) => matches += count,
            ScanOutcome::OutputFull(count) => {
                matches += count;
                truncated = true;
                break;
            }
        }
    }

    if matches == 0 {
        output.push_required("(no matches)")?;
    }
    if truncated || matches >= max_results {
        output.push_truncation(&format!(
            "[truncated: returned {matches} matches after scanning {files_scanned} files; narrow path or query]"
        ));
    } else {
        output.push_required(&format!(
            "[{matches} matches in {files_scanned} files scanned]"
        ))?;
    }
    Ok(output.finish())
}

fn project_walker(root: &Path, depth: usize) -> WalkBuilder {
    let mut builder = WalkBuilder::new(root);
    builder
        .follow_links(false)
        .max_depth(Some(depth))
        .standard_filters(true)
        .hidden(true)
        .filter_entry(|entry| {
            entry.depth() == 0
                || !matches!(
                    entry.file_name().to_str(),
                    Some(".git" | "target" | "node_modules" | "dist" | "build")
                )
        });
    builder
}

fn bounded_positive(
    requested: Option<usize>,
    default: usize,
    maximum: usize,
    field: &str,
) -> Result<usize, ErrorData> {
    let value = requested.unwrap_or(default);
    if value == 0 {
        return Err(ErrorData::invalid_params(
            format!("{field} must be greater than zero"),
            None,
        ));
    }
    Ok(value.min(maximum))
}

fn validate_query(query: &str) -> Result<(), ErrorData> {
    if query.is_empty() {
        return Err(ErrorData::invalid_params(
            "query must not be empty".to_string(),
            None,
        ));
    }
    if query.len() > MAX_QUERY_BYTES {
        return Err(ErrorData::invalid_params(
            format!("query exceeds {MAX_QUERY_BYTES} byte limit"),
            None,
        ));
    }
    if query.chars().any(char::is_control) {
        return Err(ErrorData::invalid_params(
            "query must not contain control characters".to_string(),
            None,
        ));
    }
    Ok(())
}

enum ScanOutcome {
    Complete(usize),
    OutputFull(usize),
}

fn scan_file(
    path: &Path,
    workdir: &Path,
    needle: &str,
    case_sensitive: bool,
    max_results: usize,
    output: &mut Output,
) -> ScanOutcome {
    match path.metadata() {
        Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_FILE_BYTES => {}
        _ => return ScanOutcome::Complete(0),
    }
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return ScanOutcome::Complete(0),
    };
    let mut reader = BufReader::new(file);
    let mut line_number = 0usize;
    let mut found = 0usize;

    while let Some(line) = read_bounded_line(&mut reader) {
        let Ok(line) = line else {
            break;
        };
        line_number += 1;
        if line.contains('\0') {
            break;
        }
        let is_match = if case_sensitive {
            line.contains(needle)
        } else {
            line.to_lowercase().contains(needle)
        };
        if !is_match {
            continue;
        }

        let rendered = sanitize_and_truncate(&line);
        let label = relative_label(workdir, path);
        if output
            .push(&format!("{label}:{line_number}:{rendered}"))
            .is_err()
        {
            return ScanOutcome::OutputFull(found);
        }
        found += 1;
        if found >= max_results {
            break;
        }
    }

    ScanOutcome::Complete(found)
}

fn read_bounded_line(reader: &mut impl BufRead) -> Option<Result<String, ()>> {
    let mut buffer = Vec::new();
    loop {
        let available = match reader.fill_buf() {
            Ok([]) => {
                if buffer.is_empty() {
                    return None;
                }
                return Some(String::from_utf8(buffer).map_err(|_| ()));
            }
            Ok(available) => available,
            Err(_) => return None,
        };
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if buffer.len().saturating_add(take) > MAX_LINE_BYTES {
            return Some(Err(()));
        }
        buffer.extend_from_slice(&available[..take]);
        reader.consume(take);
        if buffer.ends_with(b"\n") {
            buffer.pop();
            if buffer.ends_with(b"\r") {
                buffer.pop();
            }
            return Some(String::from_utf8(buffer).map_err(|_| ()));
        }
    }
}

fn sanitize_and_truncate(line: &str) -> String {
    let mut rendered = String::new();
    let mut truncated = false;
    for character in line.chars() {
        let character = if character.is_control() {
            if character == '\t' {
                ' '
            } else {
                '\u{fffd}'
            }
        } else {
            character
        };
        if rendered.len().saturating_add(character.len_utf8()) > MAX_RENDERED_LINE_BYTES {
            truncated = true;
            break;
        }
        rendered.push(character);
    }
    if truncated {
        rendered.push('…');
    }
    rendered
}

fn relative_label(workdir: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(workdir).unwrap_or(path);
    if relative.as_os_str().is_empty() {
        ".".to_string()
    } else {
        relative.to_string_lossy().replace('\\', "/")
    }
}

struct Output {
    text: String,
}

impl Output {
    fn new() -> Self {
        Self {
            text: String::new(),
        }
    }

    fn push(&mut self, line: &str) -> Result<(), ()> {
        if self.text.len().saturating_add(line.len()).saturating_add(1) > MAX_OUTPUT_BYTES {
            return Err(());
        }
        self.text.push_str(line);
        self.text.push('\n');
        Ok(())
    }

    fn push_required(&mut self, line: &str) -> Result<(), ErrorData> {
        self.push(line).map_err(|()| {
            ErrorData::internal_error(
                "project exploration output header exceeded its safety limit".to_string(),
                None,
            )
        })
    }

    fn push_truncation(&mut self, line: &str) {
        if self.push(line).is_ok() {
            return;
        }
        const MARKER: &str = "\n[truncated]\n";
        if self.text.len().saturating_add(MARKER.len()) > MAX_OUTPUT_BYTES {
            let keep = MAX_OUTPUT_BYTES.saturating_sub(MARKER.len());
            while self.text.len() > keep {
                self.text.pop();
            }
        }
        self.text.push_str(MARKER);
    }

    fn finish(self) -> String {
        self.text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn make_state(cwd: &Path) -> SharedState {
        let shim = crate::shim::Shim::install().expect("shim install");
        SharedState::new(cwd.to_path_buf(), shim).expect("state new")
    }

    #[test]
    fn list_directory_is_bounded_and_relative() {
        let workspace = tempdir().expect("workspace");
        fs::create_dir_all(workspace.path().join("src/nested")).expect("mkdir");
        fs::write(workspace.path().join("src/a.rs"), "a").expect("write");
        fs::write(workspace.path().join("src/b.rs"), "b").expect("write");
        fs::write(workspace.path().join("src/nested/c.rs"), "c").expect("write");
        let state = make_state(workspace.path());

        let result = list_directory(
            &state,
            ListDirectoryParams {
                path: Some("src".to_string()),
                depth: Some(3),
                max_entries: Some(2),
                workdir: None,
            },
        )
        .expect("list");

        assert!(result.starts_with("src/\n"), "{result}");
        assert!(result.contains("[truncated:"), "{result}");
        assert!(!result.contains(&workspace.path().display().to_string()));
        assert!(result.len() <= MAX_OUTPUT_BYTES);
    }

    #[test]
    fn search_text_returns_literal_relative_matches() {
        let workspace = tempdir().expect("workspace");
        fs::create_dir(workspace.path().join("src")).expect("mkdir");
        fs::write(
            workspace.path().join("src/lib.rs"),
            "first\nMeeting Baton\nlast\n",
        )
        .expect("write");
        let state = make_state(workspace.path());

        let result = search_text(
            &state,
            SearchTextParams {
                query: "meeting baton".to_string(),
                path: Some("src".to_string()),
                case_sensitive: Some(false),
                depth: None,
                max_results: None,
                workdir: None,
            },
        )
        .expect("search");

        assert!(result.contains("src/lib.rs:2:Meeting Baton"), "{result}");
        assert!(
            result.contains("[1 matches in 1 files scanned]"),
            "{result}"
        );
        assert!(!result.contains(&workspace.path().display().to_string()));
    }

    #[test]
    fn search_text_caps_results_and_output() {
        let workspace = tempdir().expect("workspace");
        let content = "needle ".repeat(400) + "\nneedle\nneedle\n";
        fs::write(workspace.path().join("many.txt"), content).expect("write");
        let state = make_state(workspace.path());

        let result = search_text(
            &state,
            SearchTextParams {
                query: "needle".to_string(),
                path: None,
                case_sensitive: None,
                depth: None,
                max_results: Some(2),
                workdir: None,
            },
        )
        .expect("search");

        assert!(result.contains("[truncated:"), "{result}");
        assert!(result.contains('…'), "{result}");
        assert!(result.len() <= MAX_OUTPUT_BYTES);
    }

    #[test]
    fn search_rejects_path_escape() {
        let workspace = tempdir().expect("workspace");
        let outside = tempdir().expect("outside");
        fs::write(outside.path().join("secret.txt"), "needle").expect("write");
        let state = make_state(workspace.path());

        let error = search_text(
            &state,
            SearchTextParams {
                query: "needle".to_string(),
                path: Some(outside.path().display().to_string()),
                case_sensitive: None,
                depth: None,
                max_results: None,
                workdir: None,
            },
        )
        .expect_err("escape must fail");

        assert!(error.message.contains("escapes workdir"), "{error:?}");
    }

    #[cfg(unix)]
    #[test]
    fn listing_does_not_follow_symlinks() {
        let workspace = tempdir().expect("workspace");
        let outside = tempdir().expect("outside");
        fs::write(outside.path().join("secret.txt"), "secret").expect("write");
        std::os::unix::fs::symlink(outside.path(), workspace.path().join("outside"))
            .expect("symlink");
        let state = make_state(workspace.path());

        let result = list_directory(
            &state,
            ListDirectoryParams {
                path: None,
                depth: Some(3),
                max_entries: None,
                workdir: None,
            },
        )
        .expect("list");

        assert!(!result.contains("secret.txt"), "{result}");
        assert!(!result.contains("outside"), "{result}");
    }
}
