use super::*;

#[tokio::test]
#[ignore = "requires the isolated Project Context Stage 7 Relay fixture"]
async fn real_relay_stage7_matches_cli_and_desktop_trusted_read() {
    let relay_url = std::env::var("PROJECT_CONTEXT_DESKTOP_E2E_RELAY_URL")
        .expect("PROJECT_CONTEXT_DESKTOP_E2E_RELAY_URL");
    let private_key = std::env::var("PROJECT_CONTEXT_DESKTOP_E2E_PRIVATE_KEY")
        .expect("PROJECT_CONTEXT_DESKTOP_E2E_PRIVATE_KEY");
    let mode = std::env::var("PROJECT_CONTEXT_DESKTOP_E2E_MODE")
        .expect("PROJECT_CONTEXT_DESKTOP_E2E_MODE");
    let expected_revision = std::env::var("PROJECT_CONTEXT_DESKTOP_E2E_EXPECTED_REVISION")
        .expect("PROJECT_CONTEXT_DESKTOP_E2E_EXPECTED_REVISION")
        .parse::<u64>()
        .expect("numeric expected Context revision");

    let state = build_app_state();
    *state.keys.lock().expect("lock Stage 7 signer") =
        Keys::parse(&private_key).expect("parse Stage 7 member private key");
    *state
        .relay_url_override
        .lock()
        .expect("lock Stage 7 Relay override") = Some(relay_url);

    let all = stage7_live_query(
        &state,
        ProjectContextQueryDto::ContainsAll {
            coordinates: Vec::new(),
        },
    )
    .await;
    assert_eq!(all.context.context_revision, expected_revision);
    assert_eq!(all.context.projection_generation, 2);
    assert_eq!(all.project_id, uuid(STAGE7_PROJECT_ID));

    let encoded = serde_json::to_string(&all).expect("serialize real Desktop result");
    for forbidden in [
        "contentMarkdown",
        "content_markdown",
        "fetchCommand",
        "rawEvent",
        "STAGE7_CONTEXT_BODY",
    ] {
        assert!(
            !encoded.contains(forbidden),
            "trusted graph result leaked {forbidden}"
        );
    }

    match mode.as_str() {
        "split" => {
            assert_eq!(all.edges.len(), 2);
            assert_eq!(stage7_connected_component_count(&all), 2);

            let profile = stage7_project_view_coordinate(
                ProjectViewObjectType::ProjectProfile,
                STAGE7_PROJECT_ID,
            );
            let goal = stage7_project_view_coordinate(ProjectViewObjectType::Goal, STAGE7_GOAL_ID);
            let role = stage7_project_view_coordinate(ProjectViewObjectType::Role, STAGE7_ROLE_ID);
            let exact = stage7_live_query(
                &state,
                ProjectContextQueryDto::Exact {
                    coordinates: vec![profile, goal.clone()],
                },
            )
            .await;
            let incident = stage7_live_query(
                &state,
                ProjectContextQueryDto::Incident { coordinate: goal },
            )
            .await;
            let contains = stage7_live_query(
                &state,
                ProjectContextQueryDto::ContainsAll {
                    coordinates: vec![role],
                },
            )
            .await;
            assert_eq!(exact.context.context_revision, expected_revision);
            assert_eq!(incident.context.context_revision, expected_revision);
            assert_eq!(contains.context.context_revision, expected_revision);
            assert_eq!(exact.edges.len(), 1);
            assert_eq!(incident.edges.len(), 1);
            assert_eq!(contains.edges.len(), 1);

            let all_membership = stage7_membership(&all);
            for result in [&exact, &incident, &contains] {
                let membership = stage7_membership(result);
                assert!(
                    membership.iter().all(|edge| all_membership.contains(edge)),
                    "focused query returned membership absent from All"
                );
            }
            assert_eq!(stage7_membership(&exact), stage7_membership(&incident));
        }
        "merged" => {
            assert_eq!(all.edges.len(), 3);
            assert_eq!(stage7_connected_component_count(&all), 1);
        }
        "updated" => {
            assert_eq!(all.edges.len(), 3);
            let document = all
                .document_details
                .iter()
                .find(|detail| detail.document_id == uuid(STAGE7_CONTEXT_DOCUMENT_A_ID))
                .expect("updated Context Document detail");
            assert_eq!(document.state, ProjectContextDetailState::Active);
            assert_eq!(document.document_revision, Some(2));
            assert_eq!(
                document.title.as_deref(),
                Some("Stage 7 Context A corrected")
            );
        }
        "tombstoned" => {
            assert_eq!(all.edges.len(), 3);
            let coordinate_key = format!("document:{STAGE7_COORDINATE_DOCUMENT_ID}");
            assert!(all
                .edges
                .iter()
                .any(|edge| edge.coordinate_keys.contains(&coordinate_key)));
            let coordinate = all
                .coordinate_details
                .iter()
                .find(|detail| detail.coordinate_key == coordinate_key)
                .expect("retained tombstoned Document coordinate");
            assert_eq!(coordinate.state, ProjectContextDetailState::Tombstoned);
            assert_eq!(coordinate.document_revision, Some(2));
        }
        "capability_off" => {
            assert_eq!(all.edges.len(), 3);
            assert!(!all.context.capability_enabled);
        }
        other => panic!("unknown Stage 7 live probe mode: {other}"),
    }

    let mut expected_document_ids = BTreeSet::from([
        uuid(STAGE7_CONTEXT_DOCUMENT_A_ID),
        uuid(STAGE7_CONTEXT_DOCUMENT_B_ID),
    ]);
    if mode != "split" {
        expected_document_ids.insert(uuid(STAGE7_CONTEXT_DOCUMENT_BRIDGE_ID));
    }
    assert_eq!(
        all.edges
            .iter()
            .flat_map(|edge| edge.context_document_ids.iter().copied())
            .collect::<BTreeSet<_>>(),
        expected_document_ids
    );
}
