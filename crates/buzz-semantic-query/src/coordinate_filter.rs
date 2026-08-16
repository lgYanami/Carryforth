//! Closed Coordinate type filters shared by semantic Coordinate operations.

use buzz_project_context::ProjectContextCoordinate;
use buzz_project_view::ProjectViewObjectType;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Every canonical Coordinate type accepted by semantic Coordinate filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectContextCoordinateType {
    /// The Project's unique profile object.
    ProjectProfile,
    /// A Project goal object.
    Goal,
    /// A Project role object.
    Role,
    /// A Project plan object.
    Plan,
    /// A Project stage object.
    Stage,
    /// A Project requirement object.
    Requirement,
    /// A Project issue object.
    Issue,
    /// A Project work object.
    Work,
    /// A Project resource object.
    Resource,
    /// A Project Document used as a graph Coordinate.
    Document,
    /// An attachable Meeting used as a graph Coordinate.
    Meeting,
}

impl ProjectContextCoordinateType {
    /// Stable database and CLI token for this closed Coordinate type.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProjectProfile => "project_profile",
            Self::Goal => "goal",
            Self::Role => "role",
            Self::Plan => "plan",
            Self::Stage => "stage",
            Self::Requirement => "requirement",
            Self::Issue => "issue",
            Self::Work => "work",
            Self::Resource => "resource",
            Self::Document => "document",
            Self::Meeting => "meeting",
        }
    }

    /// Whether one canonical Coordinate belongs to this exact type.
    #[must_use]
    pub const fn matches(self, coordinate: &ProjectContextCoordinate) -> bool {
        matches!(
            (self, coordinate),
            (
                Self::ProjectProfile,
                ProjectContextCoordinate::ProjectViewObject {
                    object_type: ProjectViewObjectType::ProjectProfile,
                    ..
                },
            ) | (
                Self::Goal,
                ProjectContextCoordinate::ProjectViewObject {
                    object_type: ProjectViewObjectType::Goal,
                    ..
                },
            ) | (
                Self::Role,
                ProjectContextCoordinate::ProjectViewObject {
                    object_type: ProjectViewObjectType::Role,
                    ..
                },
            ) | (
                Self::Plan,
                ProjectContextCoordinate::ProjectViewObject {
                    object_type: ProjectViewObjectType::Plan,
                    ..
                },
            ) | (
                Self::Stage,
                ProjectContextCoordinate::ProjectViewObject {
                    object_type: ProjectViewObjectType::Stage,
                    ..
                },
            ) | (
                Self::Requirement,
                ProjectContextCoordinate::ProjectViewObject {
                    object_type: ProjectViewObjectType::Requirement,
                    ..
                },
            ) | (
                Self::Issue,
                ProjectContextCoordinate::ProjectViewObject {
                    object_type: ProjectViewObjectType::Issue,
                    ..
                },
            ) | (
                Self::Work,
                ProjectContextCoordinate::ProjectViewObject {
                    object_type: ProjectViewObjectType::Work,
                    ..
                },
            ) | (
                Self::Resource,
                ProjectContextCoordinate::ProjectViewObject {
                    object_type: ProjectViewObjectType::Resource,
                    ..
                },
            ) | (Self::Document, ProjectContextCoordinate::Document { .. })
                | (Self::Meeting, ProjectContextCoordinate::Meeting { .. })
        )
    }
}

/// Invalid closed Coordinate type-filter state.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProjectContextCoordinateTypeFilterError {
    /// A present filter must contain at least one type.
    #[error("Coordinate type filter must not be empty")]
    Empty,
}

/// Non-empty canonical OR-set of Coordinate types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectContextCoordinateTypeFilter {
    types: Vec<ProjectContextCoordinateType>,
}

impl ProjectContextCoordinateTypeFilter {
    /// Sort, deduplicate, and validate one filter.
    pub fn new(
        mut types: Vec<ProjectContextCoordinateType>,
    ) -> Result<Self, ProjectContextCoordinateTypeFilterError> {
        types.sort_unstable();
        types.dedup();
        if types.is_empty() {
            return Err(ProjectContextCoordinateTypeFilterError::Empty);
        }
        Ok(Self { types })
    }

    /// Return this filter's canonical sorted unique types.
    #[must_use]
    pub fn types(&self) -> &[ProjectContextCoordinateType] {
        &self.types
    }

    /// Whether one Coordinate matches any requested type.
    #[must_use]
    pub fn matches(&self, coordinate: &ProjectContextCoordinate) -> bool {
        self.types
            .iter()
            .any(|coordinate_type| coordinate_type.matches(coordinate))
    }

    /// Rebuild an untrusted deserialized filter into canonical form.
    pub fn canonicalized(&self) -> Result<Self, ProjectContextCoordinateTypeFilterError> {
        Self::new(self.types.clone())
    }

    /// Whether the stored representation is already sorted, unique, and non-empty.
    #[must_use]
    pub fn is_canonical(&self) -> bool {
        !self.types.is_empty() && self.types.windows(2).all(|pair| pair[0] < pair[1])
    }
}

#[cfg(test)]
mod tests {
    use buzz_project_context::ProjectContextCoordinate;
    use buzz_project_view::ProjectViewObjectType;
    use uuid::Uuid;

    use super::*;

    #[test]
    fn filter_canonicalizes_and_matches_exact_coordinate_types() {
        let filter = ProjectContextCoordinateTypeFilter::new(vec![
            ProjectContextCoordinateType::Document,
            ProjectContextCoordinateType::Work,
            ProjectContextCoordinateType::Work,
        ])
        .expect("filter");
        assert_eq!(
            filter.types(),
            &[
                ProjectContextCoordinateType::Work,
                ProjectContextCoordinateType::Document,
            ]
        );
        assert!(
            filter.matches(&ProjectContextCoordinate::ProjectViewObject {
                object_type: ProjectViewObjectType::Work,
                object_id: Uuid::new_v4(),
            })
        );
        assert!(filter.matches(&ProjectContextCoordinate::Document {
            document_id: Uuid::new_v4(),
        }));
        assert!(!filter.matches(&ProjectContextCoordinate::Meeting {
            meeting_id: Uuid::new_v4(),
        }));
    }

    #[test]
    fn present_filter_rejects_empty_types() {
        assert_eq!(
            ProjectContextCoordinateTypeFilter::new(Vec::new()),
            Err(ProjectContextCoordinateTypeFilterError::Empty)
        );
    }

    #[test]
    fn closed_type_tokens_are_exact_and_complete() {
        let types = [
            ProjectContextCoordinateType::ProjectProfile,
            ProjectContextCoordinateType::Goal,
            ProjectContextCoordinateType::Role,
            ProjectContextCoordinateType::Plan,
            ProjectContextCoordinateType::Stage,
            ProjectContextCoordinateType::Requirement,
            ProjectContextCoordinateType::Issue,
            ProjectContextCoordinateType::Work,
            ProjectContextCoordinateType::Resource,
            ProjectContextCoordinateType::Document,
            ProjectContextCoordinateType::Meeting,
        ];
        let tokens = types.map(ProjectContextCoordinateType::as_str);
        assert_eq!(
            tokens,
            [
                "project_profile",
                "goal",
                "role",
                "plan",
                "stage",
                "requirement",
                "issue",
                "work",
                "resource",
                "document",
                "meeting",
            ]
        );
        for (coordinate_type, token) in types.into_iter().zip(tokens) {
            assert_eq!(
                serde_json::to_string(&coordinate_type).expect("serialize Coordinate type"),
                format!("\"{token}\"")
            );
        }
    }
}
