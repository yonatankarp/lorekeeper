//! `lorekeeper://join?link=<invite link, URL-encoded>`: how the sync server's join page hands an invite to the app, so
//! the browser asks "Open Lorekeeper?" (docs/SYNC.md). Any web page can open such a URL, so its content is hostile until
//! proven otherwise: it's parsed strictly (size, shape, then `Invite::parse`, the parser pasted links go through) and
//! only ever fills in the Join dialog. It never joins, opens another URL or runs anything; the player still sees the
//! server and clicks Join.
//!
//! The link holds the campaign's key, so it never travels in an event: the settings window is only told to ask, and
//! takes it with `take_join_link`. Test profiles (`LOREKEEPER_PROFILE`) ignore deep links (see DEVELOPMENT.md).

use std::sync::Mutex;

use serde::Serialize;
use sync_protocol::Invite;
use tauri::{AppHandle, Emitter};
use tauri_plugin_deep_link::DeepLinkExt;

const PREFIX: &str = "lorekeeper://join?link=";
/// Longest URL looked at. An invite link is under 200 characters, a little more URL-encoded.
const MAX_LEN: usize = 2048;
const NOT_AN_INVITE: &str = "That Lorekeeper link isn't an invite link. Ask for the invite again, or paste it here.";

/// What the Join dialog gets: the invite link to check (`{"link": ...}`), or why the deep link wasn't one
/// (`{"error": ...}`).
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Pending {
    Link(String),
    Error(String),
}

/// The last deep link, until the settings window takes it. One at a time: a newer link replaces it.
static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

/// The invite link a deep link carries, as `Invite::parse` reads it; anything else is refused with one short message.
pub(crate) fn invite_link(url: &str) -> Result<String, &'static str> {
    if url.len() > MAX_LEN {
        return Err(NOT_AN_INVITE);
    }
    let encoded = url.strip_prefix(PREFIX).ok_or(NOT_AN_INVITE)?;
    // What encodeURIComponent makes of a link: unreserved characters and %XX. No second parameter, no fragment.
    if !encoded.bytes().all(|b| b.is_ascii_alphanumeric() || b"%-._~".contains(&b)) {
        return Err(NOT_AN_INVITE);
    }
    let link = percent_decode(encoded).ok_or(NOT_AN_INVITE)?;
    Ok(Invite::parse(&link).map_err(|_| NOT_AN_INVITE)?.link())
}

/// Strict percent-decoding: every % starts two hex digits, and the result is UTF-8.
fn percent_decode(s: &str) -> Option<String> {
    let mut out = Vec::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b != b'%' {
            out.push(b);
            continue;
        }
        let (hi, lo) = (bytes.next()?, bytes.next()?);
        let digit = |c: u8| (c as char).to_digit(16);
        out.push((digit(hi)? * 16 + digit(lo)?) as u8);
    }
    String::from_utf8(out).ok()
}

/// Keeps what a deep link said for the settings window and returns whether there was a lorekeeper:// URL at all.
fn remember(urls: &[tauri::Url]) -> bool {
    let Some(url) = urls.iter().find(|u| u.scheme() == "lorekeeper") else { return false };
    let pending = match invite_link(url.as_str()) {
        Ok(link) => Pending::Link(link),
        Err(e) => Pending::Error(e.into()),
    };
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(pending);
    true
}

/// A deep link arrived (at start or while running): bring the settings window forward and have it open Join.
fn open(app: &AppHandle, urls: &[tauri::Url]) {
    if remember(urls) {
        crate::show_window(app, "settings");
        let _ = app.emit_to("settings", "join-link", ());
    }
}

/// Listens for lorekeeper:// links; not for a test profile, which can't own the scheme next to the real app.
pub(crate) fn start(app: &AppHandle) {
    if crate::profile().is_some() {
        return;
    }
    // An AppImage has no installer to register the scheme. Best effort: Windows installers and macOS bundles declare it.
    #[cfg(target_os = "linux")]
    let _ = app.deep_link().register_all();
    let handle = app.clone();
    app.deep_link().on_open_url(move |event| open(&handle, &event.urls()));
    if let Ok(Some(urls)) = app.deep_link().get_current() {
        open(app, &urls); // started by a link (Windows, Linux; macOS sends it as an event once running)
    }
}

/// The settings window takes the last deep link (once; it's gone after). No other window gets it.
#[tauri::command]
pub(crate) fn take_join_link(window: tauri::Window) -> Option<Pending> {
    if window.label() != "settings" {
        return None;
    }
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).take()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sync_protocol::random_id;

    fn link(server: &str) -> String {
        Invite { server: server.into(), room: random_id(), invite: random_id(), key: [7; 32] }.link()
    }

    /// What the join page's script makes: encodeURIComponent of origin, path and hash.
    fn encode(s: &str) -> String {
        s.bytes()
            .map(|b| if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
            .collect()
    }

    #[test]
    fn a_deep_link_carries_exactly_one_invite_link() {
        for server in ["https://lorekeeper.yonatankarp.com", "https://sync.example.org:8443", "http://127.0.0.1:18081"] {
            let l = link(server);
            assert_eq!(invite_link(&format!("{PREFIX}{}", encode(&l))), Ok(l.clone()), "{server}");
            // Through the URL type the deep-link plugin hands over, unchanged.
            let url: tauri::Url = format!("{PREFIX}{}", encode(&l)).parse().unwrap();
            assert_eq!(invite_link(url.as_str()), Ok(l));
        }
        // The host is lowercased as for a pasted link.
        let l = link("https://lorekeeper.yonatankarp.com");
        assert_eq!(invite_link(&format!("{PREFIX}{}", encode(&l.replace("lorekeeper.", "LOREKEEPER.")))), Ok(l));
    }

    #[test]
    fn anything_else_is_refused() {
        let good = encode(&link("https://lorekeeper.yonatankarp.com"));
        let refused = [
            String::new(),
            "lorekeeper://".into(),
            "lorekeeper://join".into(),
            format!("lorekeeper://JOIN?link={good}"),
            format!("lorekeeper://join/?link={good}"),
            format!("lorekeeper://evil@join?link={good}"),
            format!("lorekeeper://join:80?link={good}"),
            format!("lorekeeper://join?url={good}"),
            format!("lorekeeper://join?link={good}&link={good}"),
            format!("lorekeeper://join?link={good}&x=1"),
            format!("lorekeeper://join?link={good}#more"),
            format!("https://join?link={good}"),
            // Unencoded, or damaged encoding.
            format!("lorekeeper://join?link={}", link("https://lorekeeper.yonatankarp.com")),
            format!("lorekeeper://join?link={good}%"),
            format!("lorekeeper://join?link={good}%2"),
            format!("lorekeeper://join?link={good}%+1"),
            format!("lorekeeper://join?link={good}%G0"),
            "lorekeeper://join?link=%FF%FE".into(),
            // Links the paste parser refuses: not https, a query, no key, the wrong path, an http host on the internet.
            format!("{PREFIX}{}", encode("javascript:alert(1)")),
            format!("{PREFIX}{}", encode("file:///etc/passwd#AAAA")),
            format!("{PREFIX}{}", encode(&link("http://example.org"))),
            format!("{PREFIX}{}", encode(&link("https://lorekeeper.yonatankarp.com").replace("#", "?x=1#"))),
            format!("{PREFIX}{}", encode(link("https://lorekeeper.yonatankarp.com").split('#').next().unwrap())),
            format!("{PREFIX}{}", encode(&link("https://lorekeeper.yonatankarp.com").replace("/join/", "/v1/rooms/"))),
            format!("{PREFIX}{}", encode(&link("https://user@lorekeeper.yonatankarp.com"))),
            // Too long, even if it were otherwise fine.
            format!("{PREFIX}{good}{}", "A".repeat(MAX_LEN)),
        ];
        for url in &refused {
            assert_eq!(invite_link(url), Err(NOT_AN_INVITE), "{url}");
        }
    }

    /// Only lorekeeper:// URLs count, the last one wins, and the settings window takes it once. A malformed one is
    /// kept as a message (the dialog says so) rather than dropped silently.
    #[test]
    fn the_settings_window_takes_the_last_link_once() {
        let l = link("https://lorekeeper.yonatankarp.com");
        let url = |s: &str| -> tauri::Url { s.parse().unwrap() };
        assert!(!remember(&[url("https://example.org/")]));
        assert!(remember(&[url("https://example.org/"), url(&format!("{PREFIX}{}", encode(&l)))]));
        assert_eq!(PENDING.lock().unwrap().take(), Some(Pending::Link(l.clone())));
        assert!(remember(&[url(&format!("{PREFIX}{}", encode(&l)))]));
        assert!(remember(&[url("lorekeeper://join?link=nope")]));
        assert_eq!(PENDING.lock().unwrap().take(), Some(Pending::Error(NOT_AN_INVITE.into())));
        assert_eq!(PENDING.lock().unwrap().take(), None);
        assert_eq!(serde_json::to_value(Pending::Link("x".into())).unwrap(), serde_json::json!({ "link": "x" }));
    }
}
