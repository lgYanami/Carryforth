//! App-managed Skill templates and file installation helpers.

use super::read_version_file;
use std::fs;
use std::path::Path;

/// Default SKILL.md content for the carryforth-cli skill.
pub(super) const CARRYFORTH_CLI_SKILL_MD: &str = include_str!("../nest_skill.md");

/// Default SKILL.md content for progressive Project Context retrieval.
pub(super) const SEARCH_PROJECT_CONTEXT_SKILL_MD: &str =
    include_str!("../search_project_context_skill.md");

/// Complete managed Meeting skill.
pub(super) const CARRYFORTH_MEETING_SKILL_MD: &str =
    include_str!("../carryforth_meeting_skill/SKILL.md");
const CARRYFORTH_MEETING_CREATE_MD: &str =
    include_str!("../carryforth_meeting_skill/references/create.md");
pub(super) const CARRYFORTH_MEETING_PARTICIPANT_TURNS_MD: &str =
    include_str!("../carryforth_meeting_skill/references/participant-turns.md");
const CARRYFORTH_MEETING_MODERATOR_TURNS_MD: &str =
    include_str!("../carryforth_meeting_skill/references/moderator-turns.md");
const CARRYFORTH_MEETING_ACTION_FINALIZATION_MD: &str =
    include_str!("../carryforth_meeting_skill/references/action-finalization.md");
const CARRYFORTH_MEETING_OPENAI_YAML: &str =
    include_str!("../carryforth_meeting_skill/agents/openai.yaml");

/// Template content version for the carryforth-cli SKILL.md.
pub(super) const NEST_SKILL_VERSION: u32 = 5;

/// Template content version for the search-project-context SKILL.md.
pub(super) const SEARCH_PROJECT_CONTEXT_SKILL_VERSION: u32 = 1;

/// Template content version for the carryforth-meeting skill directory.
pub(super) const CARRYFORTH_MEETING_SKILL_VERSION: u32 = 2;

/// Canonical skill directories relative to the nest root.
pub(super) const CANONICAL_SKILL_DIR: &str = ".agents/skills/carryforth-cli";
pub(super) const SEARCH_PROJECT_CONTEXT_SKILL_DIR: &str = ".agents/skills/search-project-context";
pub(super) const CARRYFORTH_MEETING_SKILL_DIR: &str = ".agents/skills/carryforth-meeting";
pub(super) const LEGACY_CANONICAL_SKILL_DIR: &str = ".agents/skills/buzz-cli";

pub(super) struct ManagedSkillFile {
    pub(super) relative_path: &'static str,
    pub(super) body: &'static str,
}

pub(super) struct ManagedSkillTemplate {
    pub(super) name: &'static str,
    pub(super) canonical_dir: &'static str,
    pub(super) body: &'static str,
    pub(super) supporting_files: &'static [ManagedSkillFile],
    pub(super) version: u32,
}

pub(super) const CARRYFORTH_MEETING_SUPPORTING_FILES: &[ManagedSkillFile] = &[
    ManagedSkillFile {
        relative_path: "references/create.md",
        body: CARRYFORTH_MEETING_CREATE_MD,
    },
    ManagedSkillFile {
        relative_path: "references/participant-turns.md",
        body: CARRYFORTH_MEETING_PARTICIPANT_TURNS_MD,
    },
    ManagedSkillFile {
        relative_path: "references/moderator-turns.md",
        body: CARRYFORTH_MEETING_MODERATOR_TURNS_MD,
    },
    ManagedSkillFile {
        relative_path: "references/action-finalization.md",
        body: CARRYFORTH_MEETING_ACTION_FINALIZATION_MD,
    },
    ManagedSkillFile {
        relative_path: "agents/openai.yaml",
        body: CARRYFORTH_MEETING_OPENAI_YAML,
    },
];

pub(super) const MANAGED_SKILL_TEMPLATES: &[ManagedSkillTemplate] = &[
    ManagedSkillTemplate {
        name: "carryforth-cli",
        canonical_dir: CANONICAL_SKILL_DIR,
        body: CARRYFORTH_CLI_SKILL_MD,
        supporting_files: &[],
        version: NEST_SKILL_VERSION,
    },
    ManagedSkillTemplate {
        name: "search-project-context",
        canonical_dir: SEARCH_PROJECT_CONTEXT_SKILL_DIR,
        body: SEARCH_PROJECT_CONTEXT_SKILL_MD,
        supporting_files: &[],
        version: SEARCH_PROJECT_CONTEXT_SKILL_VERSION,
    },
    ManagedSkillTemplate {
        name: "carryforth-meeting",
        canonical_dir: CARRYFORTH_MEETING_SKILL_DIR,
        body: CARRYFORTH_MEETING_SKILL_MD,
        supporting_files: CARRYFORTH_MEETING_SUPPORTING_FILES,
        version: CARRYFORTH_MEETING_SKILL_VERSION,
    },
];

pub(super) fn create_managed_skill_file_if_missing(
    skill_dir: &Path,
    relative_path: &str,
    body: &str,
) -> Result<(), String> {
    let target = skill_dir.join(relative_path);
    let parent = target
        .parent()
        .ok_or_else(|| format!("managed skill file has no parent: {}", target.display()))?;
    fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
    {
        Ok(mut file) => {
            use std::io::Write;
            file.write_all(body.as_bytes())
                .map_err(|e| format!("write {}: {e}", target.display()))?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("create {}: {e}", target.display())),
    }
    Ok(())
}

/// Refresh every managed Skill when its template version changes.
pub(super) fn refresh_skill_md_if_stale(root: &Path) -> Result<(), String> {
    for skill in MANAGED_SKILL_TEMPLATES {
        refresh_managed_skill_if_stale(root, skill)?;
    }
    Ok(())
}

fn refresh_managed_skill_if_stale(root: &Path, skill: &ManagedSkillTemplate) -> Result<(), String> {
    let agents_skill_dir = root.join(skill.canonical_dir);
    let version_path = agents_skill_dir.join(".skill-version");
    if read_version_file(&version_path) >= skill.version {
        return Ok(());
    }

    fs::create_dir_all(&agents_skill_dir)
        .map_err(|e| format!("create {}: {e}", agents_skill_dir.display()))?;

    write_managed_skill_file(&agents_skill_dir, "SKILL.md", skill.body)?;
    for file in skill.supporting_files {
        write_managed_skill_file(&agents_skill_dir, file.relative_path, file.body)?;
    }

    fs::write(&version_path, format!("{}\n", skill.version))
        .map_err(|e| format!("write {}: {e}", version_path.display()))?;

    Ok(())
}

fn write_managed_skill_file(
    skill_dir: &Path,
    relative_path: &str,
    body: &str,
) -> Result<(), String> {
    let target = skill_dir.join(relative_path);
    let parent = target
        .parent()
        .ok_or_else(|| format!("managed skill file has no parent: {}", target.display()))?;
    fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| format!("tempfile in {}: {e}", parent.display()))?;
    {
        use std::io::Write;
        tmp.write_all(body.as_bytes())
            .map_err(|e| format!("write tempfile: {e}"))?;
    }
    tmp.persist(&target)
        .map_err(|e| format!("persist {}: {e}", target.display()))?;
    Ok(())
}
