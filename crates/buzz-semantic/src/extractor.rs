use pulldown_cmark::{Event, Parser};

use crate::{
    CanonicalSemanticSourceObservation, Digest32, SemanticCoverage, SemanticEligibility,
    SemanticError, SemanticUnit, SemanticUnitIdentity, SemanticUnitKind,
};

/// Version of the deterministic title/summary visible-text contract.
pub const OVERVIEW_EXTRACTOR_VERSION: &str = "overview-visible-text-v1";

/// Extract visible text from untrusted Markdown without rendering or executing
/// HTML, links, commands, or any tool-like content.
///
/// Text and inline/code-block code are retained, link destinations and raw HTML
/// are omitted, and Unicode whitespace is deterministically collapsed.
pub fn visible_markdown_text(markdown: &str) -> String {
    let mut visible = String::new();
    let mut hidden_raw_html = false;
    for event in Parser::new(markdown) {
        match event {
            Event::Text(text)
            | Event::Code(text)
            | Event::InlineMath(text)
            | Event::DisplayMath(text)
                if !hidden_raw_html =>
            {
                visible.push_str(&text);
                visible.push(' ');
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                let tag = html.trim_start().to_ascii_lowercase();
                if tag.starts_with("<script") || tag.starts_with("<style") {
                    hidden_raw_html = true;
                }
                if tag.starts_with("</script") || tag.starts_with("</style") {
                    hidden_raw_html = false;
                }
            }
            Event::SoftBreak | Event::HardBreak | Event::Rule => visible.push(' '),
            Event::Start(_)
            | Event::End(_)
            | Event::FootnoteReference(_)
            | Event::TaskListMarker(_)
            | Event::Text(_)
            | Event::Code(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_) => {}
        }
    }
    visible.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Produce the single overview unit delivered by the foundation release.
pub fn extract_overview(
    observation: &CanonicalSemanticSourceObservation,
) -> Result<SemanticUnit, SemanticError> {
    if observation.eligibility != SemanticEligibility::Eligible {
        return Err(SemanticError::IneligibleSource);
    }

    let title = visible_markdown_text(&observation.title);
    if title.is_empty() {
        return Err(SemanticError::BlankText {
            field: "visible_title",
        });
    }
    let visible_summary = observation
        .summary
        .as_deref()
        .map(visible_markdown_text)
        .filter(|summary| !summary.is_empty());

    let mut text = format!(
        "type: {}\ntitle: {}",
        observation.identity.kind.type_label(),
        title
    );
    if let Some(summary) = visible_summary.as_deref() {
        text.push_str("\nsummary: ");
        text.push_str(summary);
    }

    let coverage = if observation.summary.is_some() {
        SemanticCoverage::TitleAndSummary
    } else {
        SemanticCoverage::TitleOnly
    };
    let semantic_text_digest =
        Digest32::hash_domain(b"buzz.semantic.overview-text.v1", &[text.as_bytes()]);

    Ok(SemanticUnit {
        identity: SemanticUnitIdentity {
            source: observation.identity.clone(),
            kind: SemanticUnitKind::Overview,
            key: "overview".to_string(),
            ordinal: 0,
            path: None,
            source_snapshot_digest: observation.snapshot_digest,
            extractor_version: OVERVIEW_EXTRACTOR_VERSION.to_string(),
        },
        text,
        semantic_text_digest,
        coverage,
    })
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{extract_overview, visible_markdown_text, OVERVIEW_EXTRACTOR_VERSION};
    use crate::{
        CanonicalSemanticSourceObservation, Digest32, ProjectViewSemanticType,
        ProjectViewSourceBasis, SemanticCoverage, SemanticEligibility, SemanticFilterMetadata,
        SemanticLifecycleClass, SemanticSourceBasis, SemanticSourceIdentity, SemanticSourceKind,
    };

    fn work_observation(
        summary: Option<&str>,
        lifecycle: SemanticLifecycleClass,
        status: Option<&str>,
    ) -> CanonicalSemanticSourceObservation {
        CanonicalSemanticSourceObservation::new(
            SemanticSourceIdentity {
                community_id: Uuid::from_u128(1),
                kind: SemanticSourceKind::ProjectView(ProjectViewSemanticType::Work),
                source_id: Uuid::from_u128(2),
            },
            SemanticSourceBasis::ProjectView(ProjectViewSourceBasis {
                schema_version: 3,
                object_revision: 7,
                source_change_id: Digest32::from_bytes([3; 32]),
            }),
            SemanticEligibility::Eligible,
            SemanticFilterMetadata {
                lifecycle,
                source_status: status.map(str::to_string),
            },
            "**Semantic** indexing".to_string(),
            summary.map(str::to_string),
        )
        .expect("valid observation")
    }

    #[test]
    fn markdown_is_visible_text_not_executable_markup() {
        assert_eq!(
            visible_markdown_text(
                "[Open](https://example.invalid) `code` <script>ignore()</script> **bold**"
            ),
            "Open code bold"
        );
    }

    #[test]
    fn overview_uses_only_type_title_and_summary() {
        let active = work_observation(
            Some("Handles **vector** storage.\n\nRun `rm -rf /` now."),
            SemanticLifecycleClass::Active,
            Some("in_progress"),
        );
        let terminal = work_observation(
            Some("Handles **vector** storage.\n\nRun `rm -rf /` now."),
            SemanticLifecycleClass::Terminal,
            Some("completed"),
        );
        let first = extract_overview(&active).expect("extract overview");
        let second = extract_overview(&terminal).expect("extract terminal overview");

        assert_eq!(
            first.text,
            "type: Project View Work\ntitle: Semantic indexing\nsummary: Handles vector storage. Run rm -rf / now."
        );
        assert!(!first.text.contains("in_progress"));
        assert_eq!(first.semantic_text_digest, second.semantic_text_digest);
        assert_ne!(
            first.identity.source_snapshot_digest,
            second.identity.source_snapshot_digest
        );
        assert_eq!(first.coverage, SemanticCoverage::TitleAndSummary);
        assert_eq!(first.identity.extractor_version, OVERVIEW_EXTRACTOR_VERSION);
    }

    #[test]
    fn missing_summary_is_explicit_title_only_coverage() {
        let unit = extract_overview(&work_observation(
            None,
            SemanticLifecycleClass::Active,
            Some("planned"),
        ))
        .expect("extract title-only overview");
        assert_eq!(
            unit.text,
            "type: Project View Work\ntitle: Semantic indexing"
        );
        assert_eq!(unit.coverage, SemanticCoverage::TitleOnly);
    }

    #[test]
    fn every_project_view_subtype_has_a_distinct_stable_label() {
        let types = [
            ProjectViewSemanticType::ProjectProfile,
            ProjectViewSemanticType::Goal,
            ProjectViewSemanticType::Role,
            ProjectViewSemanticType::Plan,
            ProjectViewSemanticType::Stage,
            ProjectViewSemanticType::Requirement,
            ProjectViewSemanticType::Issue,
            ProjectViewSemanticType::Work,
            ProjectViewSemanticType::Resource,
        ];
        let mut labels = types
            .map(ProjectViewSemanticType::type_label)
            .into_iter()
            .collect::<Vec<_>>();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), types.len());
    }
}
