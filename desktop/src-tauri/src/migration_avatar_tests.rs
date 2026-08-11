use super::*;

#[test]
fn built_in_personas_do_not_supply_default_avatar_bytes() {
    for persona_id in ["builtin:fizz", "builtin:honey", "builtin:bumble"] {
        assert!(crate::managed_agents::built_in_persona_avatar_url(persona_id).is_none());
        assert!(
            crate::managed_agents::built_in_persona_definition(persona_id, "now")
                .is_some_and(|definition| definition.avatar_url.is_none())
        );
    }
}

#[test]
fn refresh_builtin_agent_avatars_preserves_existing_user_values() {
    use sha2::{Digest as _, Sha256};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("managed-agents.json");
    let existing_avatar = "existing-user-avatar-value";
    let avatar_hash = hex::encode(Sha256::digest(existing_avatar.as_bytes()));
    let legacy_avatars = [LegacyBuiltInAvatar {
        persona_id: "builtin:fizz",
        data_url_sha256: avatar_hash.as_str(),
        sanitized_media_sha256: "",
        persona_content_hash: "legacy-persona-version",
    }];
    let records = serde_json::json!([
        {
            "name": "",
            "pubkey": "",
            "slug": "builtin:fizz",
            "persona_id": null,
            "display_name": "User-renamed Fizz",
            "avatar_url": existing_avatar,
            "system_prompt": "User-edited instructions",
            "runtime": null,
            "model": null,
            "provider": null,
            "name_pool": ["User-edited name"],
            "is_builtin": true,
            "is_active": true,
            "source_team": null,
            "source_team_persona_slug": null,
            "env_vars": {},
            "respond_to": null,
            "respond_to_allowlist": [],
            "parallelism": null,
            "created_at": "before",
            "updated_at": "before",
            "future_definition_field": "preserved"
        },
        {
            "name": "fizz-instance",
            "pubkey": "fizz-instance",
            "persona_id": "builtin:fizz",
            "avatar_url": existing_avatar,
            "persona_source_version": "legacy-persona-version",
            "updated_at": "before",
            "future_instance_field": "preserved"
        }
    ]);
    std::fs::write(&path, serde_json::to_vec_pretty(&records).unwrap()).unwrap();
    let before = std::fs::read(&path).unwrap();

    refresh_builtin_agent_avatars_in_file(&path, &legacy_avatars, "after");

    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn refresh_builtin_agent_avatars_preserves_existing_uploaded_media_urls() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("managed-agents.json");
    let media_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let existing_avatar = format!("https://relay.example/media/{media_sha256}.png?download=1");
    let legacy_avatars = [LegacyBuiltInAvatar {
        persona_id: "builtin:fizz",
        data_url_sha256: "not-a-data-url-hash",
        sanitized_media_sha256: media_sha256,
        persona_content_hash: "legacy-persona-version",
    }];
    let records = serde_json::json!([{
        "name": "fizz-instance",
        "pubkey": "fizz-instance",
        "persona_id": "builtin:fizz",
        "avatar_url": existing_avatar,
        "persona_source_version": "legacy-persona-version",
        "updated_at": "before"
    }]);
    std::fs::write(&path, serde_json::to_vec_pretty(&records).unwrap()).unwrap();
    let before = std::fs::read(&path).unwrap();

    refresh_builtin_agent_avatars_in_file(&path, &legacy_avatars, "after");

    assert_eq!(std::fs::read(&path).unwrap(), before);
}
