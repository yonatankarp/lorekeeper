//! Backups to Dropbox and Google Drive (see dropbox.rs, gdrive.rs).
//!
//! Sign-in opens the browser (OAuth authorization code with PKCE, no client secret) and receives the
//! code on a one-request web server at 127.0.0.1. The refresh token lives in the OS credential store.
//! A manifest per provider (cloud-<provider>.json in the config folder) records what was uploaded,
//! so each backup sends only new and changed files and removes the ones deleted here.

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap},
    fs,
    io::{Read, Write},
    net::{Ipv4Addr, Ipv6Addr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine as _};
use ring::rand::SecureRandom as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{dropbox, gdrive, github};

/// Public client IDs of the Lorekeeper apps registered with each provider (see docs/DEVELOPMENT.md).
/// Empty: the Settings window says that backup isn't set up in this build.
pub const DROPBOX_APP_KEY: &str = "m7ph6dy364xcvi4";
pub const GOOGLE_CLIENT_ID: &str = "159297619332-dqarcjgn49pg93g5k6ouidvtfrnjgrbv.apps.googleusercontent.com";

/// Dropbox only accepts redirect URIs registered with their exact port.
const DROPBOX_PORT: u16 = 47219;
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const CANCELLED: &str = "Sign-in cancelled.";

#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Dropbox,
    Google,
}

pub const ALL: [Provider; 2] = [Provider::Dropbox, Provider::Google];

impl Provider {
    /// The credential store account and the manifest's file name.
    pub fn key(self) -> &'static str {
        match self {
            Provider::Dropbox => "dropbox",
            Provider::Google => "google",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Provider::Dropbox => "Dropbox",
            Provider::Google => "Google Drive",
        }
    }

    pub fn client_id(self) -> &'static str {
        match self {
            Provider::Dropbox => DROPBOX_APP_KEY,
            Provider::Google => GOOGLE_CLIENT_ID,
        }
    }

    /// The largest file each provider's one-request upload takes; bigger ones are skipped.
    pub fn max_file(self) -> u64 {
        match self {
            Provider::Dropbox => 150 << 20,
            Provider::Google => 5 << 20,
        }
    }

    fn token_url(self) -> &'static str {
        match self {
            Provider::Dropbox => "https://api.dropboxapi.com/oauth2/token",
            Provider::Google => "https://oauth2.googleapis.com/token",
        }
    }

    /// The authorization URL without the PKCE, state and redirect parameters.
    fn auth_url(self) -> tauri::Url {
        let (base, extra): (&str, &[(&str, &str)]) = match self {
            Provider::Dropbox => ("https://www.dropbox.com/oauth2/authorize", &[("token_access_type", "offline")]),
            Provider::Google => (
                "https://accounts.google.com/o/oauth2/v2/auth",
                &[("scope", "https://www.googleapis.com/auth/drive.file"), ("access_type", "offline"), ("prompt", "consent")],
            ),
        };
        tauri::Url::parse_with_params(base, extra).expect("valid URL")
    }

    /// Google wants the loopback IP; Dropbox allows plain http only for localhost.
    fn redirect_host(self) -> &'static str {
        if self == Provider::Google { "127.0.0.1" } else { "localhost" }
    }
}

fn not_set_up(p: Provider) -> String {
    format!("{} backup isn't set up in this build yet.", p.name())
}

/// The Settings window offers "Sign in again" for errors that end like this.
pub fn sign_in_again(p: Provider) -> String {
    format!("Your {} sign-in has expired or was removed. Sign in again.", p.name())
}

// ---------- refresh token in the OS credential store ----------

fn save_token(p: Provider, token: &str) -> Result<(), String> {
    github::keychain(p.key())?.set_password(token).map_err(|e| format!("Couldn't save the {} sign-in: {e}", p.name()))
}

fn load_token(p: Provider) -> Result<String, String> {
    github::keychain(p.key())?.get_password().map_err(|e| match e {
        keyring::Error::NoEntry => sign_in_again(p),
        e => format!("Couldn't read the {} sign-in: {e}", p.name()),
    })
}

pub fn delete_token(p: Provider) -> Result<(), String> {
    match github::keychain(p.key())?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("Couldn't remove the {} sign-in: {e}", p.name())),
    }
}

// ---------- HTTP ----------

/// Sends a request (again, when the provider says it's busy) and returns the status and JSON reply.
pub fn call(p: Provider, send: impl Fn() -> Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<(u16, Value), String> {
    let mut tries = 0;
    loop {
        tries += 1;
        let mut resp = send().map_err(|e| format!("Couldn't reach {}. Check your internet connection. ({e})", p.name()))?;
        let retry_after = resp.headers().get("retry-after").and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
        let text = resp.body_mut().with_config().limit(100 << 20).read_to_string().map_err(|e| format!("{}'s reply was cut off ({e}).", p.name()))?;
        let status = resp.status().as_u16();
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        // Google Drive reports its rate limits as 403 rateLimitExceeded / userRateLimitExceeded.
        let busy = matches!(status, 429 | 503) || (status == 403 && v["error"]["errors"][0]["reason"].as_str().is_some_and(|r| r.ends_with("ateLimitExceeded")));
        if !busy || tries == 3 {
            return Ok((status, v));
        }
        thread::sleep(Duration::from_secs(retry_after.unwrap_or(2 << tries).min(60)));
    }
}

pub fn ok(p: Provider, (status, v): (u16, Value)) -> Result<Value, String> {
    match status {
        200..=299 => Ok(v),
        401 => Err(sign_in_again(p)),
        _ => {
            let e = &v["error"];
            let msg = e["message"].as_str().or(v["error_summary"].as_str()).or(v["error_description"].as_str()).or(e.as_str()).unwrap_or("unknown error");
            Err(format!("{} said: {msg} ({status})", p.name()))
        }
    }
}

pub fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

fn token_request(p: Provider, form: &[(&str, &str)]) -> Result<Value, String> {
    let reply = call(p, || github::AGENT.post(p.token_url()).header("Accept", "application/json").send_form(form.iter().copied()))?;
    if reply.0 == 400 && reply.1["error"] == "invalid_grant" {
        return Err(sign_in_again(p)); // revoked, expired or the password changed
    }
    ok(p, reply)
}

fn field(p: Provider, v: &Value, name: &str) -> Result<String, String> {
    v[name].as_str().filter(|s| !s.is_empty()).map(String::from).ok_or_else(|| format!("{} sent an unexpected reply.", p.name()))
}

/// A fresh access token from the stored refresh token (each backup asks for one; they last an hour or more).
pub fn access_token(p: Provider) -> Result<String, String> {
    let refresh = load_token(p)?;
    let form = [("client_id", p.client_id()), ("grant_type", "refresh_token"), ("refresh_token", refresh.as_str())];
    let v = token_request(p, &form)?;
    // Keep a replacement refresh token if the provider rotates it.
    if let Some(new) = v["refresh_token"].as_str().filter(|t| !t.is_empty() && *t != refresh) {
        save_token(p, new)?;
    }
    field(p, &v, "access_token")
}

// ---------- sign-in ----------

/// 32 random bytes as URL-safe text: PKCE verifiers, state values, multipart boundaries.
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    ring::rand::SystemRandom::new().fill(&mut bytes).expect("the system's random number generator failed");
    B64.encode(bytes)
}

/// RFC 7636 S256: BASE64URL(SHA256(verifier)).
pub fn pkce_challenge(verifier: &str) -> String {
    B64.encode(ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes()))
}

/// What the browser's request to the loopback server says: None for anything that isn't the
/// redirect (like /favicon.ico), else the code or why sign-in failed.
pub fn parse_redirect(request: &str, state: &str) -> Option<Result<String, String>> {
    let target = request.lines().next()?.strip_prefix("GET ")?.split(' ').next()?;
    if !target.starts_with('/') {
        return None;
    }
    let url = tauri::Url::parse(&format!("http://localhost{target}")).ok()?;
    let q: HashMap<Cow<str>, Cow<str>> = url.query_pairs().collect();
    let get = |k: &str| q.get(k).map(|v| v.as_ref());
    if get("code").is_none() && get("error").is_none() {
        return None;
    }
    if get("state") != Some(state) {
        return Some(Err("Sign-in failed a security check. Try again.".into()));
    }
    Some(match (get("code"), get("error")) {
        (Some(code), None) if !code.is_empty() => Ok(code.into()),
        (_, Some("access_denied")) => Err("Sign-in was cancelled in the browser.".into()),
        _ => Err(format!("Sign-in failed: {}", get("error_description").or(get("error")).unwrap_or("no code"))),
    })
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn page(title: &str, text: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Lorekeeper</title>\
         <style>body{{font:18px/1.5 system-ui,sans-serif;max-width:32em;margin:15vh auto;padding:0 1em;color:#2b2118;background:#f6f0e4}}\
         @media (prefers-color-scheme:dark){{body{{color:#e8dcc8;background:#1d1915}}}}</style></head>\
         <body><h1>{}</h1><p>{}</p></body></html>",
        html_escape(title),
        html_escape(text)
    )
}

/// Reads one request from the browser and answers it with a page to close.
fn answer(mut stream: TcpStream, state: &str) -> Option<Result<String, String>> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let (mut buf, mut n) = (vec![0u8; 16 * 1024], 0);
    // Only the request line matters; browsers sometimes open a connection without sending anything.
    while n < buf.len() && !buf[..n].windows(2).any(|w| w == b"\r\n") {
        match stream.read(&mut buf[n..]) {
            Ok(0) | Err(_) => break,
            Ok(k) => n += k,
        }
    }
    let result = parse_redirect(&String::from_utf8_lossy(&buf[..n]), state);
    let (status, body) = match &result {
        None => ("404 Not Found", String::new()),
        Some(Ok(_)) => ("200 OK", page("You're signed in", "You can close this tab and return to Lorekeeper.")),
        Some(Err(e)) => ("200 OK", page("Sign-in didn't finish", &format!("{e} You can close this tab and return to Lorekeeper."))),
    };
    let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    result
}

/// The loopback server: 127.0.0.1 (and [::1] for "localhost", which browsers may try first).
fn listen(p: Provider) -> Result<Vec<TcpListener>, String> {
    let port = if p == Provider::Dropbox { DROPBOX_PORT } else { 0 };
    let mut v4 = TcpListener::bind((Ipv4Addr::LOCALHOST, port));
    // A sign-in cancelled a moment ago lets go of the fixed port within its 200 ms poll.
    for _ in 0..10 {
        if port == 0 || !v4.as_ref().is_err_and(|e| e.kind() == std::io::ErrorKind::AddrInUse) {
            break;
        }
        thread::sleep(Duration::from_millis(100));
        v4 = TcpListener::bind((Ipv4Addr::LOCALHOST, port));
    }
    let v4 = v4.map_err(|e| match port {
        0 => format!("Couldn't start signing in: {e}"),
        _ => format!("Couldn't start signing in: another program is using port {port}. Quit other apps that might use it, or restart your computer, and try again."),
    })?;
    let port = v4.local_addr().map_err(|e| e.to_string())?.port();
    let mut all = vec![v4];
    if p.redirect_host() == "localhost" {
        all.extend(TcpListener::bind((Ipv6Addr::LOCALHOST, port)).ok());
    }
    for l in &all {
        l.set_nonblocking(true).map_err(|e| e.to_string())?;
    }
    Ok(all)
}

/// Bumped by every sign-in and by cancel; a sign-in stops waiting once it isn't the latest.
static ATTEMPT: Mutex<u64> = Mutex::new(0);

pub fn cancel_sign_in() {
    *ATTEMPT.lock().unwrap() += 1;
}

/// Signs in through the browser, keeps the refresh token and returns the account (email or name).
/// The manifest is kept for the same account, so signing out and in again doesn't upload everything.
pub fn sign_in(p: Provider, config_dir: &Path) -> Result<String, String> {
    if p.client_id().is_empty() {
        return Err(not_set_up(p));
    }
    let attempt = {
        let mut a = ATTEMPT.lock().unwrap();
        *a += 1;
        *a
    };
    let listeners = listen(p)?;
    let port = listeners[0].local_addr().map_err(|e| e.to_string())?.port();
    // Exactly as registered: Dropbox "http://localhost:47219/"; Google takes any loopback port.
    let redirect = format!("http://{}:{port}/", p.redirect_host());
    let (verifier, state) = (random_token(), random_token());
    let mut url = p.auth_url();
    url.query_pairs_mut()
        .append_pair("client_id", p.client_id())
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", &redirect)
        .append_pair("code_challenge", &pkce_challenge(&verifier))
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state);
    crate::open_external(url.as_str());

    let deadline = Instant::now() + SIGN_IN_TIMEOUT;
    let code = 'wait: loop {
        if *ATTEMPT.lock().unwrap() != attempt {
            return Err(CANCELLED.into());
        }
        if Instant::now() > deadline {
            return Err("Sign-in took too long. Try again.".into());
        }
        for l in &listeners {
            if let Ok((stream, _)) = l.accept() {
                if let Some(result) = answer(stream, &state) {
                    break 'wait result?;
                }
            }
        }
        thread::sleep(Duration::from_millis(200));
    };
    drop(listeners);

    let form = [("client_id", p.client_id()), ("grant_type", "authorization_code"), ("code", &code), ("redirect_uri", &redirect), ("code_verifier", &verifier)];
    let v = token_request(p, &form)?;
    // Google lets people untick Drive access on the consent screen.
    if p == Provider::Google && v["scope"].as_str().is_some_and(|s| !s.contains("auth/drive.file")) {
        return Err("Lorekeeper needs permission to add files to your Google Drive. Sign in again and tick that box.".into());
    }
    let refresh = v["refresh_token"].as_str().filter(|t| !t.is_empty()).ok_or_else(|| format!("{} didn't let Lorekeeper stay signed in. Try again.", p.name()))?;
    let access = field(p, &v, "access_token")?;
    let account = match p {
        Provider::Dropbox => dropbox::account(&access),
        Provider::Google => gdrive::account(&access),
    }?;
    if *ATTEMPT.lock().unwrap() != attempt {
        return Err(CANCELLED.into());
    }
    save_token(p, refresh)?;
    let file = manifest_file(config_dir, p);
    if load_manifest(&file).account != account {
        let _ = fs::remove_file(&file);
    }
    Ok(account)
}

// ---------- what was uploaded ----------

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Uploaded {
    /// git_blob_sha of the content that was uploaded.
    pub hash: String,
    /// Google Drive's file id; Dropbox works by path.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id: String,
}

#[derive(Serialize, Deserialize, Default, Debug)]
#[serde(default)]
pub struct Manifest {
    /// The account it belongs to; another account starts from scratch.
    pub account: String,
    /// Vault path ("NPCs/Vex.md") -> what is there now.
    pub files: BTreeMap<String, Uploaded>,
    /// Google Drive only: folder path -> id, "" being the Lorekeeper folder.
    pub folders: BTreeMap<String, String>,
}

fn manifest_file(config_dir: &Path, p: Provider) -> PathBuf {
    config_dir.join(format!("cloud-{}.json", p.key()))
}

fn load_manifest(file: &Path) -> Manifest {
    fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub struct Local {
    pub rel: String,
    pub path: PathBuf,
    pub hash: String,
}

/// Every file to back up with its hash (hidden entries skipped, as in the other backups), and the
/// ones over `max` bytes, which are skipped without being read.
pub fn scan(vault: &Path, max: u64) -> Result<(Vec<Local>, Vec<String>), String> {
    let unreadable = |e: std::io::Error| format!("Couldn't read the notes folder: {e}");
    let (mut files, mut skipped) = (Vec::new(), Vec::new());
    for (path, is_dir) in crate::backup::vault_entries(vault).map_err(unreadable)? {
        let rel = crate::rel_path(vault, &path);
        if is_dir {
            continue;
        } else if fs::metadata(&path).map_err(unreadable)?.len() > max {
            skipped.push(rel);
        } else {
            let hash = github::git_blob_sha(&fs::read(&path).map_err(unreadable)?);
            files.push(Local { rel, path, hash });
        }
    }
    Ok((files, skipped))
}

pub struct Plan<'a> {
    pub upload: Vec<&'a Local>,
    pub delete: Vec<String>,
}

/// New and changed files go up; uploaded files that are gone here come down. A skipped (too big)
/// file still exists, so an older copy of it stays.
pub fn diff<'a>(local: &'a [Local], skipped: &[String], done: &BTreeMap<String, Uploaded>) -> Plan<'a> {
    let upload = local.iter().filter(|f| done.get(&f.rel).is_none_or(|u| u.hash != f.hash)).collect();
    let here = |rel: &String| local.iter().any(|f| &f.rel == rel) || skipped.contains(rel);
    let delete = done.keys().filter(|rel| !here(rel)).cloned().collect();
    Plan { upload, delete }
}

/// The text after "Skipped files over": "4 MB".
fn size(bytes: u64) -> String {
    format!("{} MB", bytes >> 20)
}

/// One backup run. What finished is recorded even when the run fails partway, so the next one
/// carries on from there. Returns a warning for the status line ("" if none).
pub fn backup(p: Provider, account: &str, config_dir: &Path, vault: &Path) -> Result<String, String> {
    // Read the vault first: a missing folder must never turn into deleting everything.
    let (local, skipped) = scan(vault, p.max_file())?;
    if local.is_empty() {
        return Err("The notes folder is empty, so there's nothing to back up.".into());
    }
    let token = access_token(p)?;
    let file = manifest_file(config_dir, p);
    let mut m = load_manifest(&file);
    m.account = account.into();
    let plan = diff(&local, &skipped, &m.files);
    let result = match p {
        Provider::Dropbox => dropbox::push(&token, &plan, &mut m),
        Provider::Google => gdrive::push(&token, &plan, &mut m),
    };
    let _ = fs::write(&file, serde_json::to_string_pretty(&m).unwrap_or_default());
    result?;
    Ok(if skipped.is_empty() { String::new() } else { format!("Skipped files over {}: {}", size(p.max_file()), skipped.join(", ")) })
}

/// Reads a file for upload, with the hash of exactly what is sent (it may have changed since the scan).
pub fn read(f: &Local) -> Result<(Vec<u8>, String), String> {
    let bytes = fs::read(&f.path).map_err(|e| format!("Couldn't read {}: {e}", f.rel))?;
    let hash = github::git_blob_sha(&bytes);
    Ok((bytes, hash))
}
