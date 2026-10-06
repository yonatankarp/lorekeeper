//! "Sign in with GitHub" (OAuth device flow) and backups as commits through the Git Data API,
//! so no git install is needed. The token lives in the OS credential store, never in settings.json.

use std::{
    collections::HashSet,
    fs,
    path::Path,
    sync::{LazyLock, Mutex},
    thread,
    time::{Duration, Instant},
};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Public client ID of the Lorekeeper GitHub OAuth app (with Device Flow enabled).
/// Empty: the Settings window says GitHub backup isn't set up in this build.
pub const GITHUB_CLIENT_ID: &str = "Ov23liwcKkQK0w1365N5";
const NOT_SET_UP: &str = "GitHub backup isn't set up in this build yet.";
const CANCELLED: &str = "Sign-in cancelled.";
const MAX_FILE: u64 = 50 * 1024 * 1024;

/// Shared with the cloud backups (cloud.rs). Replies of any status come back as Ok.
pub(crate) static AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .user_agent("Lorekeeper")
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(120)))
        .build()
        .into()
});

// ---------- token in the OS credential store ----------

/// One entry per account ("github", "dropbox", "google", sync rooms) under the app's identifier; a test profile's
/// entries have a service of their own.
pub(crate) fn keychain(account: &str) -> Result<keyring::Entry, String> {
    let service = match crate::profile() {
        Some(p) => format!("com.yonatankarp.dndnotes.profile.{p}"),
        None => "com.yonatankarp.dndnotes".to_string(),
    };
    keyring::Entry::new(&service, account).map_err(|e| format!("Can't use the system's password storage: {e}"))
}

pub fn save_token(token: &str) -> Result<(), String> {
    keychain("github")?.set_password(token).map_err(|e| format!("Couldn't save the GitHub sign-in: {e}"))
}

pub fn load_token() -> Result<String, String> {
    keychain("github")?.get_password().map_err(|e| match e {
        keyring::Error::NoEntry => "You're signed out of GitHub. Sign in again in Settings.".into(),
        e => format!("Couldn't read the GitHub sign-in: {e}"),
    })
}

pub fn delete_token() -> Result<(), String> {
    match keychain("github")?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("Couldn't remove the GitHub sign-in: {e}")),
    }
}

// ---------- HTTP ----------

fn read(resp: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<(u16, Value), String> {
    let mut resp = resp.map_err(|e| format!("Couldn't reach GitHub. Check your internet connection. ({e})"))?;
    let text = resp.body_mut().with_config().limit(100 << 20).read_to_string().map_err(|e| format!("GitHub's reply was cut off ({e})."))?;
    Ok((resp.status().as_u16(), serde_json::from_str(&text).unwrap_or(Value::Null)))
}

/// The two github.com/login endpoints: a form post, JSON back, no token.
fn login_post(url: &str, form: &[(&str, &str)]) -> Result<Value, String> {
    read(AGENT.post(url).header("Accept", "application/json").send_form(form.iter().copied())).map(|(_, v)| v)
}

fn headers<B>(req: ureq::RequestBuilder<B>, token: &str) -> ureq::RequestBuilder<B> {
    req.header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("Authorization", format!("Bearer {token}"))
}

/// api.github.com: GET without a body, POST (or PATCH) with one.
pub(crate) fn api(token: &str, method: &str, path: &str, body: Option<Value>) -> Result<(u16, Value), String> {
    let url = format!("https://api.github.com{path}");
    read(match (method, body) {
        ("PATCH", Some(b)) => headers(AGENT.patch(&url), token).send_json(b),
        (_, Some(b)) => headers(AGENT.post(&url), token).send_json(b),
        _ => headers(AGENT.get(&url), token).call(),
    })
}

pub(crate) fn ok((status, v): (u16, Value)) -> Result<Value, String> {
    match status {
        200..=299 => Ok(v),
        401 => Err("Your GitHub sign-in has expired. Sign out and sign in again in Settings.".into()),
        _ => Err(format!("GitHub said: {} ({status})", v["message"].as_str().unwrap_or("unknown error"))),
    }
}

fn get(token: &str, path: &str) -> Result<Value, String> {
    ok(api(token, "GET", path, None)?)
}

fn post(token: &str, path: &str, body: Value) -> Result<Value, String> {
    ok(api(token, "POST", path, Some(body))?)
}

fn sha_of(v: &Value) -> Result<String, String> {
    v["sha"].as_str().map(String::from).ok_or_else(|| "GitHub sent an unexpected reply.".into())
}

// ---------- sign-in (device flow) ----------

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct DeviceCode {
    #[serde(skip_serializing)]
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

/// The sign-in in progress; cancelling or starting another clears it, which ends the wait.
static PENDING: Mutex<Option<DeviceCode>> = Mutex::new(None);

fn oauth_error(v: &Value) -> String {
    match v["error"].as_str() {
        Some("expired_token") => "The code expired before it was approved. Try again.".into(),
        Some("access_denied") => "Sign-in was cancelled on GitHub.".into(),
        Some("device_flow_disabled" | "incorrect_client_credentials") => NOT_SET_UP.into(),
        _ => format!("GitHub sign-in failed: {}", v["error_description"].as_str().or(v["error"].as_str()).unwrap_or("no reply")),
    }
}

pub enum Poll {
    Wait,
    SlowDown,
    Token(String),
    Failed(String),
}

/// One reply from the token endpoint. GitHub answers 200 either way, so only the JSON counts.
pub fn parse_poll(v: &Value) -> Poll {
    if let Some(token) = v["access_token"].as_str().filter(|t| !t.is_empty()) {
        return Poll::Token(token.into());
    }
    match v["error"].as_str() {
        Some("authorization_pending") => Poll::Wait,
        Some("slow_down") => Poll::SlowDown,
        _ => Poll::Failed(oauth_error(v)),
    }
}

pub fn start_sign_in() -> Result<DeviceCode, String> {
    if GITHUB_CLIENT_ID.is_empty() {
        return Err(NOT_SET_UP.into());
    }
    let v = login_post("https://github.com/login/device/code", &[("client_id", GITHUB_CLIENT_ID), ("scope", "repo")])?;
    let code: DeviceCode = serde_json::from_value(v.clone()).map_err(|_| oauth_error(&v))?;
    *PENDING.lock().unwrap() = Some(code.clone());
    Ok(code)
}

pub fn cancel_sign_in() {
    *PENDING.lock().unwrap() = None;
}

/// Blocks until the code is approved on github.com (returns the token), denied, expired or cancelled.
pub fn wait_sign_in() -> Result<String, String> {
    let code = PENDING.lock().unwrap().clone().ok_or(CANCELLED)?;
    let waiting = || PENDING.lock().unwrap().as_ref().is_some_and(|p| p.device_code == code.device_code);
    let deadline = Instant::now() + Duration::from_secs(code.expires_in);
    let mut interval = code.interval.max(1);
    loop {
        let next = Instant::now() + Duration::from_secs(interval);
        while Instant::now() < next {
            thread::sleep(Duration::from_millis(200));
            if !waiting() {
                return Err(CANCELLED.into());
            }
        }
        let result = if Instant::now() >= deadline {
            Poll::Failed(oauth_error(&json!({ "error": "expired_token" })))
        } else {
            let form = [("client_id", GITHUB_CLIENT_ID), ("device_code", &code.device_code), ("grant_type", "urn:ietf:params:oauth:grant-type:device_code")];
            login_post("https://github.com/login/oauth/access_token", &form).map_or_else(Poll::Failed, |v| parse_poll(&v))
        };
        match result {
            Poll::Wait => {}
            Poll::SlowDown => interval += 5,
            Poll::Token(token) => {
                cancel_sign_in();
                return Ok(token);
            }
            Poll::Failed(e) => {
                cancel_sign_in();
                return Err(e);
            }
        }
    }
}

pub fn user_login(token: &str) -> Result<String, String> {
    get(token, "/user")?["login"].as_str().map(String::from).ok_or_else(|| "GitHub sent an unexpected reply.".into())
}

// ---------- backup as one commit ----------

/// The SHA-1 git gives a file's contents ("blob <len>\0<bytes>").
pub fn git_blob_sha(bytes: &[u8]) -> String {
    let mut h = sha1_smol::Sha1::new();
    h.update(format!("blob {}\0", bytes.len()).as_bytes());
    h.update(bytes);
    h.digest().to_string()
}

/// "owner/repo/sha" of blobs uploaded this run of the app, so a backup cut short by GitHub's
/// write limits doesn't upload them again on the next try.
static UPLOADED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);

/// The root tree with the campaign folder `name` pointing at `subtree`: every other top-level entry
/// (other campaigns, README.md, older backups) stays exactly as it is.
pub fn root_entries(current: &[Value], name: &str, subtree: &str) -> Vec<Value> {
    let ours = json!({ "path": name, "mode": "040000", "type": "tree", "sha": subtree });
    let mut out: Vec<Value> = current
        .iter()
        .filter(|e| e["path"] != name)
        .map(|e| json!({ "path": e["path"], "mode": e["mode"], "type": e["type"], "sha": e["sha"] }))
        .collect();
    out.push(ours);
    out
}

/// Commits an exact snapshot of the vault (deletions included) to the folder `name` in `owner/repo`,
/// creating the private repo if needed. Nothing outside that folder changes. Returns a warning for
/// the status line ("" if none).
pub fn backup(token: &str, owner: &str, repo: &str, name: &str, vault: &Path) -> Result<String, String> {
    // Read the vault first: a missing folder must never turn into a commit that deletes everything.
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    let unreadable = |e: std::io::Error| format!("Couldn't read the notes folder: {e}");
    for (path, is_dir) in crate::backup::vault_entries(vault).map_err(unreadable)? {
        let rel = crate::rel_path(vault, &path);
        if is_dir {
            continue;
        } else if fs::metadata(&path).map_err(unreadable)?.len() > MAX_FILE {
            skipped.push(rel);
        } else {
            files.push((rel, git_blob_sha(&fs::read(&path).map_err(unreadable)?), path));
        }
    }
    if files.is_empty() {
        return Err("The notes folder is empty, so there's nothing to back up.".into());
    }
    let warning = if skipped.is_empty() { String::new() } else { format!("Skipped files over 50 MB: {}", skipped.join(", ")) };

    let base = format!("/repos/{owner}/{repo}");
    let found = api(token, "GET", &base, None)?;
    let created = found.0 == 404;
    let info = if created {
        UPLOADED.lock().unwrap().clear();
        post(token, "/user/repos", json!({ "name": repo, "private": true, "auto_init": true, "description": "Lorekeeper notes backup" }))?
    } else {
        ok(found)?
    };
    let branch = info["default_branch"].as_str().unwrap_or("main").to_string();

    // Two tries: if the branch moved while we worked (another device), build on the new head.
    for attempt in 0.. {
        // A brand-new repo can take a moment before its first commit shows up.
        let mut tries = if created && attempt == 0 { 5 } else { 1 };
        let head = loop {
            let r = api(token, "GET", &format!("{base}/git/ref/heads/{branch}"), None)?;
            tries -= 1;
            if tries == 0 || !matches!(r.0, 404 | 409) {
                break ok(r)?;
            }
            thread::sleep(Duration::from_secs(2));
        };
        let head = head["object"]["sha"].as_str().ok_or("GitHub sent an unexpected reply.")?.to_string();
        let tree = sha_of(&get(token, &format!("{base}/git/commits/{head}"))?["tree"])?;
        // The top level, read on its own: a truncated listing must never drop another campaign.
        let root = get(token, &format!("{base}/git/trees/{tree}"))?;
        if root["truncated"] == true || !root["tree"].is_array() {
            return Err("GitHub sent an unexpected reply.".into());
        }
        // Everything, only to skip uploading what's already there (a truncated list just skips less).
        let remote = get(token, &format!("{base}/git/trees/{tree}?recursive=1"))?;
        let mut have: HashSet<String> = remote["tree"].as_array().into_iter().flatten().filter(|e| e["type"] == "blob").filter_map(|e| e["sha"].as_str().map(String::from)).collect();
        have.extend(UPLOADED.lock().unwrap().iter().filter_map(|k| k.strip_prefix(&format!("{owner}/{repo}/")).map(String::from)));

        for (_, sha, path) in files.iter_mut() {
            if have.contains(sha.as_str()) {
                continue;
            }
            // ponytail: one request per new file, paced 1s apart as GitHub asks; a first backup of
            // thousands of files hits its hourly write limit and finishes over several runs.
            thread::sleep(Duration::from_secs(1));
            let bytes = fs::read(&*path).map_err(unreadable)?;
            let blob = post(token, &format!("{base}/git/blobs"), json!({ "content": base64::engine::general_purpose::STANDARD.encode(&bytes), "encoding": "base64" }))?;
            *sha = sha_of(&blob)?; // the file may have changed since it was hashed
            UPLOADED.lock().unwrap().insert(format!("{owner}/{repo}/{sha}"));
            have.insert(sha.clone());
        }

        let entries: Vec<Value> = files.iter().map(|(rel, sha, _)| json!({ "path": rel, "mode": "100644", "type": "blob", "sha": sha })).collect();
        let subtree = sha_of(&post(token, &format!("{base}/git/trees"), json!({ "tree": entries }))?)?;
        let top = root_entries(root["tree"].as_array().unwrap(), name, &subtree);
        let new_tree = sha_of(&post(token, &format!("{base}/git/trees"), json!({ "tree": top }))?)?;
        if new_tree == tree {
            return Ok(warning); // nothing changed
        }
        let message = format!("Backup {name} {}", chrono::Local::now().format("%Y-%m-%d %H:%M"));
        let commit = sha_of(&post(token, &format!("{base}/git/commits"), json!({ "message": message, "tree": new_tree, "parents": [head] }))?)?;
        let moved = api(token, "PATCH", &format!("{base}/git/refs/heads/{branch}"), Some(json!({ "sha": commit, "force": false })))?;
        if moved.0 == 422 && attempt == 0 {
            continue;
        }
        ok(moved)?;
        break;
    }
    Ok(warning)
}
