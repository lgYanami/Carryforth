use tauri::{Emitter, Manager};
use url::Url;

fn activate_main_window(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };

    if let Err(error) = window.unminimize() {
        eprintln!("carryforth-desktop: failed to unminimize main window: {error}");
    }
    if let Err(error) = window.show() {
        eprintln!("carryforth-desktop: failed to show main window: {error}");
    }
    if let Err(error) = window.set_focus() {
        eprintln!("carryforth-desktop: failed to focus main window: {error}");
    }
}

fn parse_message_deep_link(url: &Url) -> Option<serde_json::Value> {
    let mut channel: Option<String> = None;
    let mut message_id: Option<String> = None;
    let mut thread: Option<String> = None;

    for (key, value) in url.query_pairs() {
        let value = value.into_owned();
        if value.is_empty() {
            continue;
        }
        match key.as_ref() {
            "channel" => channel = Some(value),
            "id" => message_id = Some(value),
            "thread" => thread = Some(value),
            _ => {}
        }
    }

    Some(serde_json::json!({
        "channelId": channel?,
        "messageId": message_id?,
        "threadRootId": thread,
    }))
}

/// Handle Carryforth's local message deep link.
///
/// Remote-community onboarding and identity-binding links are intentionally not
/// part of the Carryforth Desktop product surface.
pub(crate) fn handle_deep_link_url(app: &tauri::AppHandle, url_str: &str) {
    let url = match Url::parse(url_str) {
        Ok(url) => url,
        Err(error) => {
            eprintln!("carryforth-desktop: invalid deep link URL {url_str:?}: {error}");
            return;
        }
    };

    if url.scheme() != "carryforth" {
        eprintln!("carryforth-desktop: ignoring unsupported deep link: {url_str}");
        return;
    }

    match url.host_str() {
        Some("message") => {
            let Some(payload) = parse_message_deep_link(&url) else {
                eprintln!("carryforth-desktop: message deep link missing channel or id: {url_str}");
                return;
            };
            activate_main_window(app);
            let _ = app.emit("deep-link-message", payload);
        }
        Some(action) => {
            eprintln!("carryforth-desktop: unsupported deep link action: {action}");
        }
        None => {
            eprintln!("carryforth-desktop: deep link missing action: {url_str}");
        }
    }
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::parse_message_deep_link;

    #[test]
    fn parses_carryforth_message_link() {
        let url = Url::parse("carryforth://message?channel=abc&id=xyz&thread=root").unwrap();
        let payload = parse_message_deep_link(&url).unwrap();
        assert_eq!(payload["channelId"], "abc");
        assert_eq!(payload["messageId"], "xyz");
        assert_eq!(payload["threadRootId"], "root");
    }

    #[test]
    fn rejects_incomplete_message_link() {
        for raw in [
            "carryforth://message?channel=abc",
            "carryforth://message?id=xyz",
            "carryforth://message?channel=&id=xyz",
            "carryforth://message?channel=abc&id=",
        ] {
            assert!(parse_message_deep_link(&Url::parse(raw).unwrap()).is_none());
        }
    }
}
