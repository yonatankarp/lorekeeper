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

/// Stored as bytes: Windows caps a password at 1280 UTF-16 characters but a secret at 2560 bytes.
fn save_session(cookie: &str) -> Result<(), String> {
    keychain("dndbeyond")?.set_secret(cookie.as_bytes()).map_err(|e| format!("Couldn't save the D&D Beyond sign-in: {e}"))
}

/// The stored session, or None when signed out.
fn load_session() -> Result<Option<String>, String> {
    match keychain("dndbeyond")?.get_secret() {
        Ok(cookie) => Ok(Some(String::from_utf8_lossy(&cookie).into_owned())),
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
                url = next_hop(&url, &location)
                    .ok_or("D&D Beyond didn't show that campaign while signed in. Paste a link to a character in it instead: its party comes along.")?;
            }
            401 | 403 => return Err(EXPIRED.into()),
            404 => return Err(format!("D&D Beyond has no campaign {id}, or you aren't in it.")),
            _ => return Err(format!("D&D Beyond said: {status}")),
        }
    }
    Err("D&D Beyond kept redirecting that campaign page.".into())
}

// ---------- links and replies (pure) ----------

/// Where a campaign page redirect may be followed with the cookie: https on www.dndbeyond.com, not the sign-in pages.
fn next_hop(current: &Url, location: &str) -> Option<Url> {
    let next = current.join(location).ok()?;
    let ok = next.scheme() == "https" && next.host_str() == Some("www.dndbeyond.com") && !next.path().starts_with("/sign-in") && !next.path().starts_with("/login");
    ok.then_some(next)
}

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
    pub background: String,
    /// "Chaotic Good"; "" when not set.
    pub alignment: String,
    /// Markdown for the page's Appearance section: the sheet's looks and appearance text ("" when none).
    pub appearance: String,
    /// Markdown for the Personality section: traits, ideals, bonds and flaws ("" when none).
    pub personality: String,
    /// The character's portrait on D&D Beyond ("" when none), for dndbeyond_portrait.
    pub portrait: String,
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
    let classes: Vec<(String, u64, String)> = d["classes"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|c| (text(&c["definition"]["name"]), c["level"].as_u64().unwrap_or(0), text(&c["subclassDefinition"]["name"])))
        .filter(|(name, _, _)| !name.is_empty())
        .collect();
    let sub = |s: &str| if s.is_empty() { String::new() } else { format!(" ({s})") };
    let names = match classes.as_slice() {
        [(name, _, s)] => format!("{name}{}", sub(s)),
        many => many.iter().map(|(name, level, s)| format!("{name} {level}{}", sub(s))).collect::<Vec<_>>().join(" / "),
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
        background: Some(text(&d["background"]["definition"]["name"])).filter(|b| !b.is_empty()).unwrap_or_else(|| text(&d["background"]["customBackground"]["name"])),
        alignment: d["alignmentId"].as_u64().and_then(|a| ALIGNMENTS.get(a.wrapping_sub(1) as usize)).unwrap_or(&"").to_string(),
        appearance: appearance(d),
        personality: personality(&d["traits"]),
        portrait: Some(text(&d["decorations"]["avatarUrl"])).filter(|u| portrait_url(u).is_some()).unwrap_or_default(),
        error: String::new(),
    }
}

/// D&D Beyond's alignmentId, 1 to 9.
const ALIGNMENTS: [&str; 9] = [
    "Lawful Good", "Neutral Good", "Chaotic Good", "Lawful Neutral", "Neutral", "Chaotic Neutral", "Lawful Evil", "Neutral Evil", "Chaotic Evil",
];

/// "Age 25 · Height 6'2" ..." from the sheet's looks, then its appearance text.
fn appearance(d: &Value) -> String {
    let looks: Vec<String> = [("Gender", "gender"), ("Age", "age"), ("Height", "height"), ("Weight", "weight"), ("Eyes", "eyes"), ("Hair", "hair"), ("Skin", "skin")]
        .iter()
        .filter_map(|(label, key)| {
            let v = d[*key].as_str().map(str::trim).map(String::from).or_else(|| d[*key].as_u64().map(|n| n.to_string()))?;
            (!v.is_empty()).then(|| format!("{label} {v}"))
        })
        .collect();
    [looks.join(" · "), text(&d["traits"]["appearance"])].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n\n")
}

/// The sheet's personality traits, ideals, bonds and flaws, one bold-labelled paragraph each.
fn personality(traits: &Value) -> String {
    [("Traits", "personalityTraits"), ("Ideals", "ideals"), ("Bonds", "bonds"), ("Flaws", "flaws")]
        .iter()
        .filter_map(|(label, key)| {
            // One line stays on the label's line; several (one per trait on the sheet) become a list.
            let lines: Vec<String> = text(&traits[*key]).lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect();
            match lines.as_slice() {
                [] => None,
                [one] => Some(format!("**{label}:** {one}")),
                many => Some(format!("**{label}:**\n{}", many.iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n"))),
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// A portrait address worth downloading: https on dndbeyond.com or a subdomain.
fn portrait_url(url: &str) -> Option<Url> {
    let u: Url = url.parse().ok()?;
    let host = u.host_str()?;
    (u.scheme() == "https" && (host == "dndbeyond.com" || host.ends_with(".dndbeyond.com"))).then_some(u)
}

/// The image type from its first bytes; None for anything that isn't a picture the page can show.
fn image_ext(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0xFF, 0xD8, 0xFF, ..] => Some("jpg"),
        [0x89, b'P', b'N', b'G', ..] => Some("png"),
        [b'G', b'I', b'F', b'8', ..] => Some("gif"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some("webp"),
        _ => None,
    }
}

/// Downloads a portrait (at most 10 MB, a real image) as bytes and its extension.
fn download_portrait(url: &str) -> Result<(Vec<u8>, &'static str), String> {
    let url = portrait_url(url).ok_or("That portrait isn't on D&D Beyond.")?;
    let mut resp = AGENT.get(url.as_str()).call().map_err(|e| format!("Couldn't download the portrait ({e})."))?;
    if !resp.status().is_success() {
        return Err(format!("Couldn't download the portrait ({}).", resp.status().as_u16()));
    }
    let bytes = resp.body_mut().with_config().limit(10 << 20).read_to_vec().map_err(|e| format!("Couldn't download the portrait ({e})."))?;
    let ext = image_ext(&bytes).ok_or("D&D Beyond's portrait isn't a picture Lorekeeper can show.")?;
    Ok((bytes, ext))
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
    let mut next_try = Instant::now();
    loop {
        thread::sleep(Duration::from_secs(1));
        let Some(w) = app.get_webview_window(LABEL) else { return Err(CANCELLED.into()) };
        let signed_in_page = w.url().is_ok_and(|u| u.host_str() == Some("www.dndbeyond.com") && !u.path().starts_with("/sign-in"));
        if !signed_in_page || Instant::now() < next_try {
            continue;
        }
        // All cookies, not cookies_for_url: that matches the host exactly, and the session is set for
        // dndbeyond.com (every subdomain), not www.dndbeyond.com.
        let Ok(cookies) = w.cookies() else { continue };
        let Some(cookie) = cookies.iter().find(|c| c.name() == COOKIE && on_dndbeyond(c.domain())).map(|c| c.value().to_string()) else { continue };
        if cookie.is_empty() || cookie.contains(['\r', '\n', ';']) {
            continue;
        }
        if let Err(e) = cobalt_token(&cookie) {
            next_try = Instant::now() + Duration::from_secs(5); // not a signed-in session (yet)
            let _ = app.emit("dndbeyond-waiting", e); // shown in Settings, so a refusal isn't silent
            continue;
        }
        let saved = save_session(&cookie);
        let _ = w.destroy();
        return saved;
    }
}

/// A cookie set for D&D Beyond: dndbeyond.com or one of its subdomains (a leading dot is allowed).
fn on_dndbeyond(domain: Option<&str>) -> bool {
    domain.map(|d| d.trim_start_matches('.')).is_some_and(|d| d == "dndbeyond.com" || d.ends_with(".dndbeyond.com"))
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

/// Saves a character's portrait into the notes folder as `<stem>.<ext>` (`<stem> 2.<ext>` and so on when taken; never
/// overwrites) and returns its path in the vault, for the page's `portrait` property.
#[tauri::command]
pub async fn dndbeyond_portrait(app: AppHandle, url: String, stem: String) -> Result<String, String> {
    let root = crate::notes_dir(&app);
    crate::off_main(move || {
        let (bytes, ext) = download_portrait(&url)?;
        for n in 1..100 {
            let rel = if n == 1 { format!("{stem}.{ext}") } else { format!("{stem} {n}.{ext}") };
            let path = crate::vault_image(&root, &rel)?;
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    std::io::Write::write_all(&mut f, &bytes).map_err(|e| e.to_string())?;
                    crate::backup::mark_changed();
                    return Ok(rel);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        Err("Too many portraits with that name.".into())
    })
    .await
}

#[cfg(test)]
mod tests {
    #[test]
    fn session_cookie_domains() {
        for ok in ["dndbeyond.com", ".dndbeyond.com", "www.dndbeyond.com", "auth-service.dndbeyond.com"] {
            assert!(on_dndbeyond(Some(ok)), "{ok}");
        }
        for bad in ["evildndbeyond.com", "dndbeyond.com.evil.net", ""] {
            assert!(!on_dndbeyond(Some(bad)), "{bad}");
        }
        assert!(!on_dndbeyond(None));
    }

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
    fn campaign_redirects_keep_the_cookie_on_dndbeyond_https() {
        let here: Url = "https://www.dndbeyond.com/campaigns/1".parse().unwrap();
        assert_eq!(next_hop(&here, "/campaigns/1/overview").map(|u| u.to_string()), Some("https://www.dndbeyond.com/campaigns/1/overview".into()));
        assert_eq!(next_hop(&here, "/sign-in?returnUrl=%2Fcampaigns%2F1"), None);
        assert_eq!(next_hop(&here, "https://evil.com/campaigns/1"), None);
        assert_eq!(next_hop(&here, "http://www.dndbeyond.com/campaigns/1"), None);
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
            "background": { "definition": { "name": "Folk Hero" } }, "alignmentId": 3,
            "age": 52, "eyes": "Grey", "hair": "", "traits": { "appearance": "A braided beard.", "ideals": "Family.\nAlways.", "flaws": " " },
            "decorations": { "avatarUrl": "https://www.dndbeyond.com/avatars/1/2/demus.jpeg" },
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
                classes: "Fighter 3 (Champion) / Rogue 1".into(),
                level: 4,
                player: "demus_player".into(),
                url: "https://www.dndbeyond.com/characters/5".into(),
                background: "Folk Hero".into(),
                alignment: "Chaotic Good".into(),
                appearance: "Age 52 · Eyes Grey\n\nA braided beard.".into(),
                personality: "**Ideals:**\n- Family.\n- Always.".into(),
                portrait: "https://www.dndbeyond.com/avatars/1/2/demus.jpeg".into(),
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
        assert_eq!((empty.alignment.as_str(), empty.portrait.as_str()), ("", ""));
        let custom = parse_character(9, &json!({ "data": { "background": { "customBackground": { "name": "Exile" } }, "alignmentId": 10,
            "decorations": { "avatarUrl": "http://evil.example/x.jpg" } } }));
        assert_eq!((custom.background.as_str(), custom.alignment.as_str(), custom.portrait.as_str()), ("Exile", "", ""));
    }

    #[test]
    fn portraits_only_from_dndbeyond_and_only_pictures() {
        assert!(portrait_url("https://www.dndbeyond.com/avatars/1.jpeg").is_some());
        for bad in ["http://www.dndbeyond.com/a.jpg", "https://dndbeyond.com.evil.net/a.jpg", "https://evil.net/a.jpg", "nope"] {
            assert!(portrait_url(bad).is_none(), "{bad}");
        }
        assert_eq!(image_ext(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        assert_eq!(image_ext(b"\x89PNG\r\n"), Some("png"));
        assert_eq!(image_ext(b"RIFF1234WEBPVP8"), Some("webp"));
        assert_eq!(image_ext(b"<html>"), None);
    }
}
