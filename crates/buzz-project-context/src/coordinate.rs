//! Closed coordinate union and canonical edge identity rules.

use std::cmp::Ordering;

use buzz_project_view::ProjectViewObjectType;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::validation::{validate_uuid_v4, MIN_EDGE_COORDINATES};
use crate::{ProjectContextError, ProjectContextResult};

/// One v2 endpoint of a Project Context hyperedge.
///
/// The closed union admits Project View objects, Project Documents, and
/// terminal Meetings. New coordinate families must be appended after the
/// explicit ranks allocated here so existing edge identities remain stable.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(
    tag = "coordinate_type",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ProjectContextCoordinate {
    /// A Project View object coordinate.
    ProjectViewObject {
        /// Closed Project View object kind.
        object_type: ProjectViewObjectType,
        /// Stable object UUID.
        object_id: Uuid,
    },
    /// A Project Document coordinate.
    Document {
        /// Stable Document UUID.
        document_id: Uuid,
    },
    /// A terminal Meeting coordinate.
    Meeting {
        /// Stable Meeting UUID.
        meeting_id: Uuid,
    },
}

impl ProjectContextCoordinate {
    /// Validate the identity without requiring a host-derived Project ID.
    pub fn validate(&self) -> ProjectContextResult<()> {
        match self {
            Self::ProjectViewObject { object_id, .. } => validate_uuid_v4(*object_id, "object_id"),
            Self::Document { document_id } => validate_uuid_v4(*document_id, "document_id"),
            Self::Meeting { meeting_id } => validate_uuid_v4(*meeting_id, "meeting_id"),
        }
    }

    /// Validate the coordinate against the host-derived Project identity.
    pub fn validate_for_project(&self, project_id: Uuid) -> ProjectContextResult<()> {
        self.validate()?;
        if let Self::ProjectViewObject {
            object_type: ProjectViewObjectType::ProjectProfile,
            object_id,
        } = self
        {
            if *object_id != project_id {
                return Err(ProjectContextError::InvalidCoordinate {
                    reason: "project_profile object_id must equal the host-derived project_id"
                        .to_owned(),
                });
            }
        }
        Ok(())
    }

    /// Canonical `c` tag value scoped by the host-derived Project identity.
    #[must_use]
    pub fn tag_value(&self, project_id: Uuid) -> String {
        match self {
            Self::ProjectViewObject {
                object_type,
                object_id,
            } => format!("pv:{project_id}:{}:{object_id}", object_type.as_str()),
            Self::Document { document_id } => {
                format!("document:{project_id}:{document_id}")
            }
            Self::Meeting { meeting_id } => format!("meeting:{project_id}:{meeting_id}"),
        }
    }

    pub(crate) const fn family_rank(&self) -> u8 {
        match self {
            Self::ProjectViewObject { .. } => 0,
            Self::Document { .. } => 1,
            Self::Meeting { .. } => 2,
        }
    }

    pub(crate) const fn object_type_rank(object_type: ProjectViewObjectType) -> u8 {
        match object_type {
            ProjectViewObjectType::ProjectProfile => 0,
            ProjectViewObjectType::Goal => 1,
            ProjectViewObjectType::Role => 2,
            ProjectViewObjectType::Plan => 3,
            ProjectViewObjectType::Stage => 4,
            ProjectViewObjectType::Requirement => 5,
            ProjectViewObjectType::Issue => 6,
            ProjectViewObjectType::Work => 7,
            ProjectViewObjectType::Resource => 8,
        }
    }

    pub(crate) fn append_identity_bytes(&self, output: &mut Vec<u8>) {
        output.push(self.family_rank());
        match self {
            Self::ProjectViewObject {
                object_type,
                object_id,
            } => {
                output.push(Self::object_type_rank(*object_type));
                output.extend_from_slice(object_id.as_bytes());
            }
            Self::Document { document_id } => output.extend_from_slice(document_id.as_bytes()),
            Self::Meeting { meeting_id } => output.extend_from_slice(meeting_id.as_bytes()),
        }
    }
}

impl Ord for ProjectContextCoordinate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.family_rank()
            .cmp(&other.family_rank())
            .then_with(|| match (self, other) {
                (
                    Self::ProjectViewObject {
                        object_type: left_type,
                        object_id: left_id,
                    },
                    Self::ProjectViewObject {
                        object_type: right_type,
                        object_id: right_id,
                    },
                ) => Self::object_type_rank(*left_type)
                    .cmp(&Self::object_type_rank(*right_type))
                    .then_with(|| left_id.as_bytes().cmp(right_id.as_bytes())),
                (Self::Document { document_id: left }, Self::Document { document_id: right }) => {
                    left.as_bytes().cmp(right.as_bytes())
                }
                (Self::Meeting { meeting_id: left }, Self::Meeting { meeting_id: right }) => {
                    left.as_bytes().cmp(right.as_bytes())
                }
                _ => Ordering::Equal,
            })
    }
}

impl PartialOrd for ProjectContextCoordinate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Validate, sort, and deduplicate one edge's coordinate set.
///
/// Duplicate coordinates are rejected rather than silently collapsed because
/// accepting both representations would weaken signed-command canonicality.
pub fn canonicalize_coordinates(
    mut coordinates: Vec<ProjectContextCoordinate>,
) -> ProjectContextResult<Vec<ProjectContextCoordinate>> {
    if coordinates.len() < MIN_EDGE_COORDINATES {
        return Err(ProjectContextError::TooFewCoordinates {
            minimum: MIN_EDGE_COORDINATES,
            actual: coordinates.len(),
        });
    }
    for coordinate in &coordinates {
        coordinate.validate()?;
    }
    coordinates.sort();
    if coordinates.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(ProjectContextError::DuplicateCoordinate);
    }
    Ok(coordinates)
}

pub(crate) fn validate_canonical_coordinates(
    coordinates: &[ProjectContextCoordinate],
) -> ProjectContextResult<()> {
    let canonical = canonicalize_coordinates(coordinates.to_vec())?;
    if canonical != coordinates {
        return Err(ProjectContextError::NonCanonicalCoordinates);
    }
    Ok(())
}
