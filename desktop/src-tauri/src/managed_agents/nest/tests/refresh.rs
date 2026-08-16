use super::*;

#[test]
fn refresh_agents_md_writes_version_file() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join(".buzz");
    ensure_nest_at(&root).unwrap();
    let version = fs::read_to_string(root.join(".nest-agents-version")).unwrap();
    assert_eq!(version.trim(), NEST_AGENTS_VERSION.to_string());
}

#[test]
fn refresh_managed_skills_write_version_files() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join(".buzz");
    ensure_nest_at(&root).unwrap();
    for (path, version) in [
        (
            ".agents/skills/carryforth-cli/.skill-version",
            NEST_SKILL_VERSION,
        ),
        (
            ".agents/skills/search-project-context/.skill-version",
            SEARCH_PROJECT_CONTEXT_SKILL_VERSION,
        ),
        (
            ".agents/skills/carryforth-meeting/.skill-version",
            CARRYFORTH_MEETING_SKILL_VERSION,
        ),
    ] {
        let actual = fs::read_to_string(root.join(path)).unwrap();
        assert_eq!(actual.trim(), version.to_string());
    }
}

#[test]
fn refresh_agents_md_preserves_managed_section() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join(".buzz");
    ensure_nest_at(&root).unwrap();

    // Simulate a managed section update.
    let agents_md = root.join("AGENTS.md");
    upsert_managed_section(
        &agents_md,
        "## Active Agents\n\n| Name | Role |\n|------|------|\n| Kit | Builder |",
    )
    .unwrap();

    // Remove version file to simulate an upgrade.
    fs::remove_file(root.join(".nest-agents-version")).unwrap();

    // Re-run ensure_nest_at (triggers refresh).
    ensure_nest_at(&root).unwrap();

    let content = fs::read_to_string(&agents_md).unwrap();
    // Static content should be refreshed (from template).
    assert!(
        content.starts_with("# Carryforth Nest"),
        "template header must be present"
    );
    // Managed section should be preserved.
    assert!(
        content.contains("Kit"),
        "managed section agent table must survive refresh"
    );
    assert!(content.contains(BEGIN_MARKER), "BEGIN marker must survive");
    assert!(content.contains(END_MARKER), "END marker must survive");
}

#[test]
fn refresh_skips_when_version_current() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join(".buzz");
    ensure_nest_at(&root).unwrap();

    // Manually change AGENTS.md content after version file is written.
    let agents_md = root.join("AGENTS.md");
    fs::write(&agents_md, "user modified content").unwrap();

    // Re-run ensure_nest_at — version file is current, so no refresh.
    ensure_nest_at(&root).unwrap();

    let content = fs::read_to_string(&agents_md).unwrap();
    assert_eq!(
        content, "user modified content",
        "should not overwrite when version is current"
    );
}

#[test]
fn refresh_managed_skills_overwrite_on_version_bump() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join(".buzz");
    ensure_nest_at(&root).unwrap();

    let carryforth = root.join(".agents/skills/carryforth-cli/SKILL.md");
    let search = root.join(".agents/skills/search-project-context/SKILL.md");
    let meeting = root.join(".agents/skills/carryforth-meeting/SKILL.md");
    let participant =
        root.join(".agents/skills/carryforth-meeting/references/participant-turns.md");
    fs::write(&carryforth, "stale CLI skill content").unwrap();
    fs::write(&search, "stale search skill content").unwrap();
    fs::write(&meeting, "stale Meeting skill content").unwrap();
    fs::write(&participant, "stale participant reference").unwrap();

    for version in [
        ".agents/skills/carryforth-cli/.skill-version",
        ".agents/skills/search-project-context/.skill-version",
    ] {
        let _ = fs::remove_file(root.join(version));
    }
    fs::write(
        root.join(".agents/skills/carryforth-meeting/.skill-version"),
        "1\n",
    )
    .unwrap();

    ensure_nest_at(&root).unwrap();

    assert_eq!(
        fs::read_to_string(&carryforth).unwrap(),
        CARRYFORTH_CLI_SKILL_MD
    );
    assert_eq!(
        fs::read_to_string(&search).unwrap(),
        SEARCH_PROJECT_CONTEXT_SKILL_MD
    );
    assert_eq!(
        fs::read_to_string(&meeting).unwrap(),
        CARRYFORTH_MEETING_SKILL_MD
    );
    assert_eq!(
        fs::read_to_string(&participant).unwrap(),
        CARRYFORTH_MEETING_PARTICIPANT_TURNS_MD
    );
}
