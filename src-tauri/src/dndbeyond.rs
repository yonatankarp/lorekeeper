//! Player characters from D&D Beyond. Signing in opens D&D Beyond's own sign-in page in a window of its own
//! (private browsing, no access to the app) and keeps its CobaltSession cookie in the OS credential store.
//! That cookie buys short-lived tokens for the character service; it is only ever sent to D&D Beyond.

use std::{
    thread,
    time::{Duration, Instant},
};

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, Url};

use crate::github::{keychain, AGENT};

/// The sign-in window's label; lib.rs lets it close for real (other windows only hide).
pub const LABEL: &str = "dndbeyond";
const SITE: &str = "https://www.dndbeyond.com";
const COOKIE: &str = "CobaltSession";
const CANCELLED: &str = "Sign-in cancelled.";
const EXPIRED: &str = "Your D&D Beyond sign-in expired. Sign in again in Settings.";
const NOT_SHARED: &str = "D&D Beyond won't share this character. Sign in to D&D Beyond in Settings, or set the character to Public.";
const SIGN_IN_FOR_CAMPAIGN: &str = "Sign in to D&D Beyond in Settings to import from a campaign.";
const BAD_LINK: &str = "That isn't a link to a D&D Beyond character or campaign.";
/// Enough for any party; stops a runaway list of linked characters.
const MAX_CHARACTERS: usize = 30;

// ---------- session in the OS credential store ----------

fn save_session(cookie: &str) -> Result<(), String> {
    keychain("dndbeyond")?.set_password(cookie).map_err(|e| format!("Couldn't save the D&D Beyond sign-in: {e}"))
}

/// The stored session, or None when signed out.
fn load_session() -> Result<Option<String>, String> {
    match keychain("dndbeyond")?.get_password() {
        Ok(cookie) => Ok(Some(cookie)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("Couldn't read the D&D Beyond sign-in: {e}")),
    }
}

fn delete_session() -> Result<(), String> {
    match keychain("dndbeyond")?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("Couldn't remove the D&D Beyond sign-in: {e}")),
    }
}

// ---------- HTTP ----------

/// Status, Location header and body text. Replies of any status come back as Ok.
fn send(resp: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<(u16, String, String), String> {
    let mut resp = resp.map_err(|e| format!("Couldn't reach D&D Beyond. Check your internet connection. ({e})"))?;
    let location = resp.headers().get("location").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let text = resp.body_mut().with_config().limit(20 << 20).read_to_string().map_err(|e| format!("D&D Beyond's reply was cut off ({e})."))?;
    Ok((resp.status().as_u16(), location, text))
}

/// A short-lived token for the character service, from the stored session cookie.
fn cobalt_token(cookie: &str) -> Result<String, String> {
    let req = AGENT.post("https://auth-service.dndbeyond.com/v1/cobalt-token").header("Cookie", format!("{COOKIE}={cookie}"));
    let (status, _, text) = send(req.send_empty())?;
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    match (status, v["token"].as_str()) {
        (200..=299, Some(token)) if !token.is_empty() => Ok(token.to_string()),
        (401 | 403, _) | (200..=299, _) => Err(EXPIRED.into()),
        _ => Err(format!("D&D Beyond's sign-in service said: {status}")),
    }
}

fn character_json(id: u64, token: Option<&str>) -> Result<Value, String> {
    let mut req = AGENT.get(format!("https://character-service.dndbeyond.com/character/v5/character/{id}"));
    if let Some(token) = token {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    let (status, _, text) = send(req.call())?;
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    match status {
        200..=299 if v["data"].is_object() => Ok(v),
        401 | 403 => Err(NOT_SHARED.into()),
        404 => Err(format!("D&D Beyond has no character {id}.")),
        _ => Err(format!("D&D Beyond said: {} ({status})", v["message"].as_str().unwrap_or("unexpected reply"))),
    }
}

/// The campaign page (it needs the session), for the characters it links to. Redirects are followed by hand,
/// since a redirect drops the cookie; one to the sign-in page means D&D Beyond didn't take the session.
fn campaign_ids(id: u64, cookie: &str) -> Result<Vec<u64>, String> {
    let mut url: Url = format!("{SITE}/campaigns/{id}").parse().expect("valid URL");
    for _ in 0..3 {
        let req = AGENT.get(url.as_str()).header("Cookie", format!("{COOKIE}={cookie}")).config().max_redirects(0).build();
        let (status, location, html) = send(req.call())?;
        match status {
            200..=299 => {
                let ids = character_ids(&html);
                return if ids.is_empty() { Err("No characters found in that campaign. Paste a link to a character in it instead.".into()) } else { Ok(ids) };
            }
            300..=399 => {
                let next = url.join(&location).map_err(|_| BAD_LINK.to_string())?;
                if next.path().starts_with("/sign-in") || next.path().starts_with("/login") || next.host_str() != Some("www.dndbeyond.com") {
                    return Err("D&D Beyond didn't show that campaign while signed in. Paste a link to a character in it instead: its party comes along.".into());
                }
                url = next;
            }
            401 | 403 => return Err(EXPIRED.into()),
            404 => return Err(format!("D&D Beyond has no campaign {id}, or you aren't in it.")),
            _ => return Err(format!("D&D Beyond said: {status}")),
        }
    }
    Err("D&D Beyond kept redirecting that campaign page.".into())
}

// ---------- links and replies (pure) ----------

#[derive(Debug, PartialEq)]
pub enum Link {
    Character(u64),
    Campaign(u64),
}

fn number(s: &str) -> Option<u64> {
    let digits = !s.is_empty() && s.len() <= 15 && s.bytes().all(|b| b.is_ascii_digit());
    if digits { s.parse().ok() } else { None }
}

/// A character link (/characters/<id>, /profile/<user>/characters/<id>, anything after the id), a campaign
/// link (/campaigns/<id>) on dndbeyond.com, or a bare character id.
pub fn parse_link(link: &str) -> Result<Link, String> {
    let link = link.trim();
    if let Some(id) = number(link) {
        return Ok(Link::Character(id));
    }
    let full = if link.contains("://") { link.to_string() } else { format!("https://{link}") };
    let url = Url::parse(&full).map_err(|_| BAD_LINK.to_string())?;
    if !matches!(url.scheme(), "https" | "http") || !matches!(url.host_str(), Some("www.dndbeyond.com" | "dndbeyond.com")) {
        return Err(BAD_LINK.into());
    }
    let parts: Vec<&str> = url.path_segments().map(|s| s.filter(|p| !p.is_empty()).collect()).unwrap_or_default();
    let found = match parts.as_slice() {
        ["characters", id, ..] | ["profile", _, "characters", id, ..] => number(id).map(Link::Character),
        ["campaigns", "join", ..] => return Err("That's a campaign invite. Paste the campaign's own page link (dndbeyond.com/campaigns/<number>).".into()),
        ["campaigns", id, ..] => number(id).map(Link::Campaign),
        _ => None,
    };
    found.ok_or_else(|| BAD_LINK.into())
}

/// The character ids a campaign page links to, each once, in page order.
pub fn character_ids(html: &str) -> Vec<u64> {
    let mut ids = Vec::new();
    for (at, m) in html.match_indices("/characters/") {
        let rest = &html[at + m.len()..];
        let digits = &rest[..rest.bytes().take_while(u8::is_ascii_digit).count()];
        if let Some(id) = number(digits).filter(|id| !ids.contains(id)) {
            ids.push(id);
        }
    }
    ids
}

#[derive(Serialize, Default, Debug, PartialEq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Character {
    pub id: u64,
    pub name: String,
    pub race: String,
    /// "Fighter" for one class, "Fighter 3 / Rogue 1" for several.
    pub classes: String,
    pub level: u64,
    pub player: String,
    pub url: String,
    /// Why this one couldn't be read ("" when it was).
    pub error: String,
}

pub fn sheet_url(id: u64) -> String {
    format!("{SITE}/characters/{id}")
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or("").trim().to_string()
}

/// The other characters in the character's campaign: (id, name, D&D Beyond username).
pub fn party(v: &Value) -> Vec<(u64, String, String)> {
    let members = v["data"]["campaign"]["characters"].as_array().map(Vec::as_slice).unwrap_or_default();
    members.iter().filter_map(|m| Some((m["characterId"].as_u64()?, text(&m["characterName"]), text(&m["username"])))).collect()
}

/// A character-service v5 reply as a Character; anything missing stays empty.
pub fn parse_character(id: u64, v: &Value) -> Character {
    let d = &v["data"];
    let race = Some(text(&d["race"]["fullName"])).filter(|r| !r.is_empty()).unwrap_or_else(|| text(&d["race"]["baseName"]));
    let classes: Vec<(String, u64)> = d["classes"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|c| (text(&c["definition"]["name"]), c["level"].as_u64().unwrap_or(0)))
        .filter(|(name, _)| !name.is_empty())
        .collect();
    let names = match classes.as_slice() {
        [(name, _)] => name.clone(),
        many => many.iter().map(|(name, level)| format!("{name} {level}")).collect::<Vec<_>>().join(" / "),
    };
    let own_entry = party(v).into_iter().find(|m| m.0 == id).map(|m| m.2).unwrap_or_default();
    let player = Some(text(&d["username"])).filter(|p| !p.is_empty()).unwrap_or(own_entry);
    Character {
        id,
        name: text(&d["name"]),
        race,
        classes: names,
        level: classes.iter().map(|c| c.1).sum(),
        player,
        url: sheet_url(id),
        error: String::new(),
    }
}

// ---------- looking characters up ----------

/// A token when signed in; None reads public characters only.
fn token() -> Result<Option<String>, String> {
    load_session()?.map(|cookie| cobalt_token(&cookie)).transpose()
}

/// Every character a link leads to: the character or the campaign's characters, then the rest of their
/// party. One that can't be read comes back with its error instead of failing the lookup.
fn lookup(link: &str) -> Result<Vec<Character>, String> {
    let token = token()?;
    let mut queue: Vec<(u64, String, String)> = match parse_link(link)? {
        Link::Character(id) => vec![(id, String::new(), String::new())],
        Link::Campaign(id) => {
            let cookie = load_session()?.ok_or(SIGN_IN_FOR_CAMPAIGN)?;
            campaign_ids(id, &cookie)?.into_iter().map(|id| (id, String::new(), String::new())).collect()
        }
    };
    let mut found = Vec::new();
    let mut next = 0;
    while next < queue.len() && found.len() < MAX_CHARACTERS {
        let (id, name, user) = queue[next].clone();
        next += 1;
        found.push(match character_json(id, token.as_deref()) {
            Ok(v) => {
                for member in party(&v) {
                    if !queue.iter().any(|q| q.0 == member.0) {
                        queue.push(member);
                    }
                }
                let c = parse_character(id, &v);
                Character { player: if c.player.is_empty() { user } else { c.player.clone() }, ..c }
            }
            Err(error) => Character { id, name: if name.is_empty() { format!("Character {id}") } else { name }, player: user, url: sheet_url(id), error, ..Default::default() },
        });
    }
    Ok(found)
}

// ---------- sign-in window ----------

fn open_window(app: &AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.set_focus();
        return Err("Finish signing in in the D&D Beyond window.".into());
    }
    let url: Url = format!("{SITE}/sign-in?returnUrl=%2F").parse().expect("valid URL");
    // No capability names this window, and Tauri refuses IPC from remote pages without one.
    // Private browsing: nothing of the session stays in the app's web storage once the window closes.
    tauri::WebviewWindowBuilder::new(app, LABEL, tauri::WebviewUrl::External(url))
        .title("Sign in to D&D Beyond")
        .inner_size(480.0, 720.0)
        .center()
        .incognito(true)
        .build()
        .map_err(|e| format!("Couldn't open the sign-in window: {e}"))?;
    Ok(())
}

/// Waits, while the window is open, for the signed-in session cookie: once the window has left the sign-in
/// pages and the cookie gets a token, it is stored and the window closed. Closing the window cancels.
fn wait_sign_in(app: &AppHandle) -> Result<(), String> {
    let site: Url = SITE.parse().expect("valid URL");
    let mut next_try = Instant::now();
    loop {
        thread::sleep(Duration::from_secs(1));
        let Some(w) = app.get_webview_window(LABEL) else { return Err(CANCELLED.into()) };
        let signed_in_page = w.url().is_ok_and(|u| u.host_str() == Some("www.dndbeyond.com") && !u.path().starts_with("/sign-in"));
        if !signed_in_page || Instant::now() < next_try {
            continue;
        }
        let Ok(cookies) = w.cookies_for_url(site.clone()) else { continue };
        let Some(cookie) = cookies.iter().find(|c| c.name() == COOKIE).map(|c| c.value().to_string()) else { continue };
        if cookie.is_empty() || cookie.contains(['\r', '\n', ';']) {
            continue;
        }
        if cobalt_token(&cookie).is_err() {
            next_try = Instant::now() + Duration::from_secs(5); // not a signed-in session (yet)
            continue;
        }
        save_session(&cookie)?;
        let _ = w.destroy();
        return Ok(());
    }
}

// ---------- commands ----------

/// Opens D&D Beyond's sign-in page and resolves once signed in (returns "", no username is read), or fails
/// when the window is closed first.
#[tauri::command]
pub async fn dndbeyond_sign_in(app: AppHandle) -> Result<String, String> {
    open_window(&app)?;
    let handle = app.clone();
    crate::off_main(move || wait_sign_in(&handle)).await?;
    let _ = app.emit("dndbeyond-changed", ());
    Ok(String::new())
}

/// Forgets the session (the window's private browsing data went when it closed).
#[tauri::command]
pub async fn dndbeyond_sign_out(app: AppHandle) -> Result<(), String> {
    crate::off_main(delete_session).await?;
    let _ = app.emit("dndbeyond-changed", ());
    Ok(())
}

/// Whether a session is stored.
#[tauri::command]
pub async fn dndbeyond_status() -> Result<bool, String> {
    crate::off_main(|| Ok(load_session()?.is_some())).await
}

#[tauri::command]
pub async fn dndbeyond_lookup(link: String) -> Result<Vec<Character>, String> {
    crate::off_main(move || lookup(&link)).await
}

/// One character, for "Refresh from D&D Beyond".
#[tauri::command]
pub async fn dndbeyond_character(id: u64) -> Result<Character, String> {
    crate::off_main(move || Ok(parse_character(id, &character_json(id, token()?.as_deref())?))).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn links_are_checked_strictly() {
        assert_eq!(parse_link(" 12345 "), Ok(Link::Character(12345)));
        assert_eq!(parse_link("https://www.dndbeyond.com/characters/151286614"), Ok(Link::Character(151286614)));
        assert_eq!(parse_link("dndbeyond.com/characters/42/AbCdEf"), Ok(Link::Character(42)));
        assert_eq!(parse_link("https://www.dndbeyond.com/profile/Someone/characters/7?x=1"), Ok(Link::Character(7)));
        assert_eq!(parse_link("https://www.dndbeyond.com/campaigns/6151007"), Ok(Link::Campaign(6151007)));
        assert!(parse_link("https://www.dndbeyond.com/campaigns/join/123").unwrap_err().contains("invite"));
        for bad in [
            "",
            "https://evil.com/characters/1",
            "https://www.dndbeyond.com.evil.com/characters/1",
            "https://www.dndbeyond.com/characters/abc",
            "https://www.dndbeyond.com/characters/-1",
            "https://www.dndbeyond.com/monsters/1",
            "ftp://www.dndbeyond.com/characters/1",
            "https://www.dndbeyond.com/characters/99999999999999999999",
        ] {
            assert_eq!(parse_link(bad), Err(BAD_LINK.into()), "{bad}");
        }
    }

    #[test]
    fn campaign_page_ids_once_in_order() {
        let html = r#"<a href="/characters/12">A</a> <a href="https://www.dndbeyond.com/profile/x/characters/34/builder">B</a>
            <a href="/characters/12">A again</a> <a href="/characters">mine</a> <a href="/characters/x9">no</a>"#;
        assert_eq!(character_ids(html), [12, 34]);
        assert!(character_ids("no links").is_empty());
    }

    #[test]
    fn character_reply_is_read_defensively() {
        let v = json!({ "data": {
            "name": " Demus ", "race": { "fullName": "Hill Dwarf", "baseName": "Dwarf" },
            "classes": [
                { "level": 3, "definition": { "name": "Fighter" }, "subclassDefinition": { "name": "Champion" } },
                { "level": 1, "definition": { "name": "Rogue" } }
            ],
            "campaign": { "characters": [
                { "characterId": 5, "characterName": "Demus", "username": "demus_player" },
                { "characterId": 6, "characterName": "Vex", "username": "vexer" },
                { "characterName": "No id" }
            ] }
        } });
        assert_eq!(
            parse_character(5, &v),
            Character {
                id: 5,
                name: "Demus".into(),
                race: "Hill Dwarf".into(),
                classes: "Fighter 3 / Rogue 1".into(),
                level: 4,
                player: "demus_player".into(),
                url: "https://www.dndbeyond.com/characters/5".into(),
                error: String::new(),
            }
        );
        assert_eq!(party(&v), [(5, "Demus".into(), "demus_player".into()), (6, "Vex".into(), "vexer".into())]);

        // One class shows without its level; baseName stands in for fullName; username wins.
        let one = json!({ "data": { "name": "Vex", "username": "vexer2", "race": { "baseName": "Elf" },
            "classes": [{ "level": 2, "definition": { "name": "Wizard" } }] } });
        let c = parse_character(6, &one);
        assert_eq!((c.race.as_str(), c.classes.as_str(), c.level, c.player.as_str()), ("Elf", "Wizard", 2, "vexer2"));

        // Nothing at all: empty fields, no panic.
        let empty = parse_character(9, &json!({ "data": { "classes": "oops", "race": null } }));
        assert_eq!((empty.name.as_str(), empty.classes.as_str(), empty.level), ("", "", 0));
        assert!(party(&Value::Null).is_empty());
    }
}
