//! Verified one-hop semantic selection for progressive Project Context traversal.

use buzz_core::CommunityId;
use buzz_project_context::EdgeKey;
use buzz_sdk::semantic_one_hop_search::{
    parse_project_context_one_hop_semantic_search_result,
    ProjectContextOneHopSemanticHttpRequestObservation,
};
use buzz_semantic_query::{
    OneHopSemanticScope, ProjectContextOneHopSemanticQuery, ProjectContextOneHopSemanticQueryResult,
};
use uuid::Uuid;

use super::{
    parse_coordinate_token, parse_single_semantic_result_event, read_identity,
    read_verified_v3_snapshot, ProjectViewIdentity, ProjectViewSchema,
    ONE_HOP_SEMANTIC_SEARCH_HTTP_EXTENSION,
};
use crate::client::CarryforthClient;
use crate::error::CliError;
use crate::OutputFormat;

/// Rank current incident Edges through their canonical relation Documents.
pub(super) async fn run_coordinate_edge_search(
    client: &CarryforthClient,
    coordinate: &str,
    query: String,
    limit: u8,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let coordinate = parse_coordinate_token(coordinate)?;
    run_one_hop_search(
        client,
        query,
        limit,
        OneHopSemanticScope::IncidentEdges { coordinate },
        format,
    )
    .await
}

/// Rank the current complete Coordinate members of one active Edge.
pub(super) async fn run_edge_coordinate_search(
    client: &CarryforthClient,
    edge_key: &str,
    query: String,
    limit: u8,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let edge_key = EdgeKey::from_hex(edge_key)
        .map_err(|error| CliError::Usage(format!("invalid Edge key: {error}")))?;
    run_one_hop_search(
        client,
        query,
        limit,
        OneHopSemanticScope::EdgeCoordinates { edge_key },
        format,
    )
    .await
}

async fn run_one_hop_search(
    client: &CarryforthClient,
    query: String,
    limit: u8,
    scope: OneHopSemanticScope,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let identity = require_one_hop_identity(client).await?;
    let project = read_verified_v3_snapshot(client, identity)
        .await
        .map_err(|error| {
            integrity_error(format!(
                "cannot resolve current Project identity for one-hop semantic search: {error}"
            ))
        })?;
    let project_id = *project.meta().project_id.as_uuid();
    let request = ProjectContextOneHopSemanticQuery {
        request_id: Uuid::new_v4(),
        project_id,
        query,
        limit,
        scope,
    }
    .validate_and_canonicalize()
    .map_err(|error| CliError::Usage(format!("invalid one-hop semantic search: {error}")))?;
    let response = client
        .one_hop_semantic_search_once(&identity.relay_pubkey, request)
        .await?;
    let event = parse_single_semantic_result_event(&response.response_body)?;
    let result = parse_project_context_one_hop_semantic_search_result(
        &event,
        &identity.relay_pubkey,
        ProjectContextOneHopSemanticHttpRequestObservation {
            project_id: CommunityId::from_uuid(project_id),
            authenticated_caller: client.public_key(),
            request: &response.request,
            nip98_auth_event_id: response.nip98_auth_event_id,
            exact_authenticated_body: &response.exact_body,
        },
    )
    .map_err(|error| integrity_error(format!("invalid one-hop semantic result: {error}")))?;
    print_result(&result, format)
}

async fn require_one_hop_identity(
    client: &CarryforthClient,
) -> Result<ProjectViewIdentity, CliError> {
    let identity = read_identity(client).await?.ok_or_else(|| {
        CliError::Unavailable("Project View v3 is not ready for one-hop semantic search".to_owned())
    })?;
    if identity.schema != ProjectViewSchema::V3 {
        return Err(CliError::Usage(
            "one-hop semantic search requires Project View v3".to_owned(),
        ));
    }
    if !identity.one_hop_semantic_search_http_enabled {
        if identity.extensions_temporarily_unavailable {
            return Err(CliError::Unavailable(
                "Relay one-hop semantic capability observation could not be completed".to_owned(),
            ));
        }
        return Err(CliError::Usage(format!(
            "Relay does not advertise {ONE_HOP_SEMANTIC_SEARCH_HTTP_EXTENSION}"
        )));
    }
    Ok(identity)
}

fn print_result(
    result: &ProjectContextOneHopSemanticQueryResult,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let serialized = match format {
        OutputFormat::Json => serde_json::to_string_pretty(result),
        OutputFormat::Compact => serde_json::to_string(result),
    }
    .map_err(|error| CliError::Other(format!("failed to serialize output: {error}")))?;
    println!("{serialized}");
    Ok(())
}

fn integrity_error(message: impl Into<String>) -> CliError {
    CliError::Other(format!(
        "verification_failed:one_hop_semantic:{}",
        message.into()
    ))
}

#[cfg(test)]
mod tests {
    use buzz_project_context::ProjectContextCoordinate;
    use buzz_project_view::ProjectViewObjectType;

    use super::*;

    #[test]
    fn command_scopes_remain_structurally_isolated() {
        let project_id = Uuid::new_v4();
        let coordinate = ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Role,
            object_id: Uuid::new_v4(),
        };
        let incident = ProjectContextOneHopSemanticQuery {
            request_id: Uuid::new_v4(),
            project_id,
            query: "release authorization evidence".to_owned(),
            limit: 8,
            scope: OneHopSemanticScope::IncidentEdges { coordinate },
        };
        let edge = ProjectContextOneHopSemanticQuery {
            request_id: Uuid::new_v4(),
            project_id,
            query: "frontend work responsible for the failure".to_owned(),
            limit: 8,
            scope: OneHopSemanticScope::EdgeCoordinates {
                edge_key: EdgeKey::from_hex(&"07".repeat(32)).expect("canonical Edge key"),
            },
        };
        let incident = serde_json::to_value(incident).expect("serialize incident scope");
        let edge = serde_json::to_value(edge).expect("serialize Edge scope");
        assert_eq!(incident["scope"]["scope_type"], "incident_edges");
        assert!(incident["scope"].get("edge_key").is_none());
        assert_eq!(edge["scope"]["scope_type"], "edge_coordinates");
        assert!(edge["scope"].get("coordinate").is_none());
    }
}
