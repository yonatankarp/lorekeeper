//! Restore from a backup into a new folder. It only lists and downloads: nothing is written to a
//! backup, the notes folder is never touched, and nothing is deleted or overwritten. Every name
//! from a backup is checked before a file is written, so none can land outside the new folder.

use std::{
    fs,
    io::{self, Write as _},
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering::SeqCst},
        Mutex,
    },
    thread,
    time::Duration,
};

use base64::Engine as _;
use chrono::{DateTime, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::{
    backup,
    cloud::{self, bearer, call, ok, Provider},
    dropbox, gdrive, github,
    github::AGENT,
    Settings,
};

const DRIVE_FILES: &str = "https://www.googleapis.com/drive/v3/files";
const DRIVE_FOLDER: &str = "application/vnd.google-apps.folder";

#[derive(Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Folder,
    Github,
    Dropbox,
    Google,
}

/// One backup to pick from: a dated copy, a commit, or a provider's current backup.
#[derive(Serialize, Debug)]
pub struct Choice {
    id: String,
    label: String,
}

#[derive(Serialize, Debug)]
pub struct Done {
    target: String,
    files: usize,
    /// Names that couldn't be saved here (unsafe or duplicate), not restored.
    skipped: Vec<String>,
}

// ---------- where the open campaign's backups are ----------

/// The open campaign's backups (see backup::backup_name): the main campaign's are the places from
/// before there were campaigns, every other campaign's are its own. Restore only reads these, and
/// the Settings window shows them.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Places {
    /// The open campaign's name.
    campaign: String,
    /// Where its dated copies are; "" when folder backup is off.
    folder: String,
    /// Its GitHub repository.
    repo: String,
    /// Its folder in the Dropbox app folder: "" (the app folder itself) or "Campaigns/<name>".
    dropbox: String,
    /// The name of its folder in My Drive.
    drive: String,
    /// The main campaign only: the Dropbox folders of other campaigns, which aren't its notes.
    #[serde(skip)]
    others: Vec<String>,
}

pub fn places(s: &Settings) -> Places {
    let name = backup::backup_name(s, &s.vault_path);
    let dropbox_dir = |name: &str| dropbox::root(name).trim_start_matches('/').to_string();
    // The main campaign's Dropbox backup is the app folder itself, with the other campaigns' in
    // Campaigns/: all of that is left out, renamed and removed campaigns' too, unless the notes have
    // a Campaigns folder of their own; then only the current campaigns' folders are.
    let others = match name.as_str() {
        "" if Path::new(&s.vault_path).join("Campaigns").is_dir() => {
            s.campaigns.iter().map(|c| backup::backup_name(s, c)).filter(|n| !n.is_empty()).map(|n| dropbox_dir(&n)).collect()
        }
        "" => vec!["Campaigns".to_string()],
        _ => Vec::new(),
    };
    let folder = match (s.backup_folder.as_str(), name.as_str()) {
        ("", _) => String::new(),
        (base, "") => base.to_string(),
        (base, name) => Path::new(base).join(name).to_string_lossy().into_owned(),
    };
    Places {
        campaign: backup::campaign_name(&s.vault_path),
        folder,
        repo: backup::campaign_repo(&s.github_repo, &name),
        dropbox: dropbox_dir(&name),
        drive: gdrive::top_folder(&name),
        others,
    }
}

// ---------- names from a backup ----------

/// A backup's file name ("NPCs/Vex.md") as a path inside the restore folder, or None when it could
/// escape it or can't be a file name here: absolute, `..`, empty parts, drive prefixes.
pub fn safe_rel(name: &str) -> Option<PathBuf> {
    let part_ok = |p: &str| {
        let windows_ok = || {
            let stem = p.split('.').next().unwrap_or("").to_ascii_uppercase();
            let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
                || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit());
            !reserved && !p.ends_with(['.', ' ']) && !p.chars().any(|c| c.is_control() || "\\:*?\"<>|".contains(c))
        };
        !p.is_empty() && p != "." && p != ".." && !p.contains('\0') && (!cfg!(windows) || windows_ok())
    };
    if !name.split('/').all(part_ok) {
        return None;
    }
    let path = PathBuf::from(name);
    path.components().all(|c| matches!(c, Component::Normal(_))).then_some(path)
}

/// The part of a backup path under `root`: everything when root is "".
pub fn under_root<'a>(rel: &'a str, root: &str) -> Option<&'a str> {
    if root.is_empty() {
        return Some(rel);
    }
    rel.strip_prefix(root)?.strip_prefix('/').filter(|r| !r.is_empty())
}

fn rooted<T>(files: Vec<(String, T)>, root: &str) -> Vec<(String, T)> {
    files.into_iter().filter_map(|(rel, x)| Some((under_root(&rel, root)?.to_string(), x))).collect()
}

/// Splits a backup's files into the ones to write (name, local path, how to fetch it) and the names skipped.
pub fn plan<T>(target: &Path, files: Vec<(String, T)>) -> (Vec<(String, PathBuf, T)>, Vec<String>) {
    let (mut todo, mut skipped) = (Vec::new(), Vec::new());
    for (rel, x) in files {
        match safe_rel(&rel) {
            Some(p) => todo.push((rel, target.join(p), x)),
            None => skipped.push(rel),
        }
    }
    (todo, skipped)
}

// ---------- the new folder ----------

/// "<campaign> restored 2026-10-05" in `parent` (so each campaign's copy has its own name), or "... (2)" and so on when that exists.
pub fn suggest(parent: &Path, campaign: &str, today: NaiveDate) -> PathBuf {
    let base = format!("{campaign} restored {}", today.format("%Y-%m-%d"));
    (1..).map(|n| parent.join(if n == 1 { base.clone() } else { format!("{base} ({n})") })).find(|p| !p.exists()).unwrap()
}

/// Creates the folder to restore into. It must be new, outside the notes folder, and not inside a
/// dated backup copy (those get replaced and deleted).
pub fn create_target(vault: &Path, backups: &Path, target: &Path) -> Result<(), String> {
    if !target.is_absolute() {
        return Err(format!("Pick a full path for the restored notes, not \"{}\".", target.display()));
    }
    let resolved = backup::resolved(target);
    if resolved.starts_with(backup::resolved(vault)) {
        return Err("The restored notes can't go inside your notes folder. Pick a place outside it.".into());
    }
    if !backups.as_os_str().is_empty() {
        let rest = resolved.strip_prefix(backup::resolved(backups)).ok();
        if rest.is_some_and(|r| r.components().any(|c| backup::is_snapshot(&c.as_os_str().to_string_lossy()))) {
            return Err("The restored notes can't go inside a dated backup copy. Pick another place.".into());
        }
    }
    fs::create_dir(target).map_err(|e| match e.kind() {
        io::ErrorKind::AlreadyExists => format!("{} already exists. Pick a new folder name.", target.display()),
        _ => format!("Couldn't create {}: {e}", target.display()),
    })
}

fn files(n: usize) -> String {
    format!("{n} file{}", if n == 1 { "" } else { "s" })
}

/// Creates `target` and writes every file into it, never over an existing one. On an error what was
/// written stays (nothing is ever deleted) and the message says how far it got.
fn write_all<T>(
    (vault, backups, target): (&Path, &Path, &Path),
    files_in_backup: Vec<(String, T)>,
    mut fetch: impl FnMut(&T) -> Result<Vec<u8>, String>,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Done, String> {
    let (todo, mut skipped) = plan(target, files_in_backup);
    if todo.is_empty() {
        return Err("This backup has no notes to restore.".into());
    }
    create_target(vault, backups, target)?;
    let (total, mut written) = (todo.len(), 0);
    for (i, (rel, path, x)) in todo.iter().enumerate() {
        progress(i, total);
        let stopped = |e: String| format!("{e} {} restored to {} before this.", files(written), target.display());
        let bytes = fetch(x).map_err(stopped)?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| stopped(format!("Couldn't create the folder for {rel}: {e}.")))?;
        }
        match fs::OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(mut f) => f.write_all(&bytes).map_err(|e| stopped(format!("Couldn't write {rel}: {e}.")))?,
            // Two names that are the same file here (Vex.md and vex.md on macOS or Windows): keep the first.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                skipped.push(rel.clone());
                continue;
            }
            Err(e) => return Err(stopped(format!("Couldn't write {rel}: {e}."))),
        }
        written += 1;
    }
    progress(total, total);
    Ok(Done { target: target.to_string_lossy().into_owned(), files: written, skipped })
}

// ---------- folder backup ----------

/// The dated copies in the backup folder, newest first.
pub fn snapshots(folder: &Path) -> Result<Vec<String>, String> {
    let dirs = fs::read_dir(folder).map_err(|e| format!("The backup folder isn't available ({}): {e}", folder.display()))?;
    let mut names: Vec<String> = dirs
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| backup::is_snapshot(n))
        .collect();
    names.sort_unstable_by(|a, b| b.cmp(a));
    Ok(names)
}

fn local_files(dir: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let entries = backup::vault_entries(dir).map_err(|e| format!("Couldn't read the backup: {e}"))?;
    Ok(entries.into_iter().filter(|(_, is_dir)| !is_dir).map(|(p, _)| (crate::rel_path(dir, &p), p)).collect())
}

// ---------- GitHub: a commit's files, one blob per request ----------

fn github_get(token: &str, path: &str) -> Result<Value, String> {
    let reply = github::api(token, "GET", path, None)?;
    if matches!(reply.0, 404 | 409) {
        return Err("There's no GitHub backup to restore yet.".into());
    }
    github::ok(reply)
}

fn github_commits(token: &str, owner: &str, repo: &str) -> Result<Vec<Choice>, String> {
    let v = github_get(token, &format!("/repos/{owner}/{repo}/commits?per_page=10"))?;
    let commits = v.as_array().cloned().unwrap_or_default();
    Ok(commits
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let date = c["commit"]["committer"]["date"].as_str().and_then(|d| DateTime::parse_from_rfc3339(d).ok())?;
            let label = format!("{}{}", date.with_timezone(&Local).format("%Y-%m-%d %H:%M"), if i == 0 { " (latest)" } else { "" });
            Some(Choice { id: c["sha"].as_str()?.to_string(), label })
        })
        .collect())
}

/// (path, blob sha) of every regular file in a commit.
fn github_files(token: &str, base: &str, commit: &str) -> Result<Vec<(String, String)>, String> {
    let tree = github_get(token, &format!("{base}/git/commits/{commit}"))?["tree"]["sha"].as_str().map(String::from).ok_or("GitHub sent an unexpected reply.")?;
    let v = github_get(token, &format!("{base}/git/trees/{tree}?recursive=1"))?;
    if v["truncated"] == true {
        return Err("This backup has too many files to restore here. Use Code > Download ZIP on github.com instead.".into());
    }
    let entries = v["tree"].as_array().cloned().unwrap_or_default();
    // Symlinks (120000) and submodules aren't notes.
    let regular = |e: &&Value| e["type"] == "blob" && matches!(e["mode"].as_str(), Some("100644" | "100755"));
    Ok(entries.iter().filter(regular).filter_map(|e| Some((e["path"].as_str()?.to_string(), e["sha"].as_str()?.to_string()))).collect())
}

fn github_blob(token: &str, base: &str, sha: &str) -> Result<Vec<u8>, String> {
    let v = github::ok(github::api(token, "GET", &format!("{base}/git/blobs/{sha}"), None)?)?;
    let text: String = v["content"].as_str().unwrap_or("").chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD.decode(text).map_err(|_| "GitHub sent an unexpected reply.".into())
}

fn is_sha(id: &str) -> bool {
    (40..=64).contains(&id.len()) && id.chars().all(|c| c.is_ascii_hexdigit())
}

// ---------- Dropbox and Google Drive ----------

/// A file's bytes (again, when the provider says it's busy).
fn download(p: Provider, send: impl Fn() -> Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Vec<u8>, String> {
    let mut tries = 0;
    loop {
        tries += 1;
        let mut resp = send().map_err(|e| format!("Couldn't reach {}. Check your internet connection. ({e})", p.name()))?;
        let status = resp.status().as_u16();
        if (200..300).contains(&status) {
            return resp.body_mut().with_config().limit(200 << 20).read_to_vec().map_err(|e| format!("{}'s reply was cut off ({e}).", p.name()));
        }
        if matches!(status, 429 | 503) && tries < 3 {
            thread::sleep(Duration::from_secs(2 << tries));
            continue;
        }
        let v = serde_json::from_str(&resp.body_mut().read_to_string().unwrap_or_default()).unwrap_or(Value::Null);
        return Err(ok(p, (status, v)).err().unwrap_or_default());
    }
}

/// (path, file id) of every file in the campaign's folder (`root`, see Places), leaving out the `others`.
fn dropbox_files(token: &str, root: &str, others: &[String]) -> Result<Vec<(String, String)>, String> {
    let p = Provider::Dropbox;
    let mut v = ok(p, dropbox::rpc(token, "files/list_folder", &json!({ "path": "", "recursive": true, "limit": 2000 }))?)?;
    let mut out = Vec::new();
    loop {
        for e in v["entries"].as_array().into_iter().flatten().filter(|e| e[".tag"] == "file") {
            if let (Some(path), Some(id)) = (e["path_display"].as_str(), e["id"].as_str()) {
                out.push((path.trim_start_matches('/').to_string(), id.to_string()));
            }
        }
        if v["has_more"] != true {
            out.retain(|(rel, _)| !others.iter().any(|o| under_root(rel, o).is_some()));
            return Ok(rooted(out, root));
        }
        let cursor = v["cursor"].as_str().unwrap_or("").to_string();
        v = ok(p, dropbox::rpc(token, "files/list_folder/continue", &json!({ "cursor": cursor }))?)?;
    }
}

fn dropbox_file(token: &str, id: &str) -> Result<Vec<u8>, String> {
    let arg = dropbox::header_json(&json!({ "path": id }));
    download(Provider::Dropbox, || {
        AGENT.post("https://content.dropboxapi.com/2/files/download").header("Authorization", bearer(token)).header("Dropbox-API-Arg", &arg).send_empty()
    })
}

fn drive_list(token: &str, query: &str, fields: &str) -> Result<Vec<Value>, String> {
    let (p, mut out, mut page) = (Provider::Google, Vec::new(), String::new());
    loop {
        let mut params = vec![("q", query), ("fields", fields), ("pageSize", "1000")];
        if !page.is_empty() {
            params.push(("pageToken", page.as_str()));
        }
        let url = tauri::Url::parse_with_params(DRIVE_FILES, &params).expect("valid URL");
        let v = ok(p, call(p, || AGENT.get(url.as_str()).header("Authorization", bearer(token)).call())?)?;
        out.extend(v["files"].as_array().cloned().unwrap_or_default());
        match v["nextPageToken"].as_str() {
            Some(t) if !t.is_empty() => page = t.to_string(),
            _ => return Ok(out),
        }
    }
}

/// The `name` folders the app made in My Drive (one per computer that backed up), newest first.
fn drive_roots(token: &str, name: &str) -> Result<Vec<Value>, String> {
    let name = name.replace('\\', "\\\\").replace('\'', "\\'"); // quoted for Drive's query language
    let q = format!("name = '{name}' and mimeType = '{DRIVE_FOLDER}' and 'root' in parents and trashed = false");
    let mut roots = drive_list(token, &q, "nextPageToken,files(id,createdTime)")?;
    roots.sort_by(|a, b| b["createdTime"].as_str().cmp(&a["createdTime"].as_str()));
    Ok(roots)
}

fn is_drive_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// (path, file id) of every file under a Drive folder. Google Docs and the like have no file to download.
fn drive_files(token: &str, folder: &str) -> Result<Vec<(String, String)>, String> {
    let (mut out, mut stack) = (Vec::new(), vec![(String::new(), folder.to_string())]);
    while let Some((prefix, id)) = stack.pop() {
        for f in drive_list(token, &format!("'{id}' in parents and trashed = false"), "nextPageToken,files(id,name,mimeType)")? {
            let (Some(name), Some(fid)) = (f["name"].as_str(), f["id"].as_str()) else { continue };
            let rel = if prefix.is_empty() { name.to_string() } else { format!("{prefix}/{name}") };
            match f["mimeType"].as_str().unwrap_or("") {
                DRIVE_FOLDER if is_drive_id(fid) => stack.push((rel, fid.to_string())),
                m if m.starts_with("application/vnd.google-apps.") => {}
                _ => out.push((rel, fid.to_string())),
            }
        }
    }
    Ok(out)
}

fn drive_file(token: &str, id: &str) -> Result<Vec<u8>, String> {
    let url = format!("{DRIVE_FILES}/{id}?alt=media");
    download(Provider::Google, || AGENT.get(&url).header("Authorization", bearer(token)).call())
}

// ---------- listing and restoring ----------

fn current(p: Provider, n: usize) -> Result<Vec<Choice>, String> {
    if n == 0 {
        return Err(format!("Your {} backup is empty.", p.name()));
    }
    Ok(vec![Choice { id: String::new(), label: format!("Current backup ({})", files(n)) }])
}

/// What can be restored from the open campaign's backup (`w`, from places).
pub fn list(source: Source, s: &Settings, w: &Places) -> Result<Vec<Choice>, String> {
    let choices = match source {
        Source::Folder => {
            if w.folder.is_empty() {
                return Err("Folder backup is off.".into());
            }
            let names = snapshots(Path::new(&w.folder))?;
            names.into_iter().map(|n| Choice { label: n.rsplit(' ').next().unwrap_or(&n).to_string(), id: n }).collect()
        }
        Source::Github => github_commits(&github::load_token()?, &s.github_user, &w.repo)?,
        Source::Dropbox => current(Provider::Dropbox, dropbox_files(&cloud::access_token(Provider::Dropbox)?, &w.dropbox, &w.others)?.len())?,
        Source::Google => {
            let token = cloud::access_token(Provider::Google)?;
            let roots = drive_roots(&token, &w.drive)?;
            let mut choices = Vec::new();
            for r in roots.iter().filter_map(|r| Some((r["id"].as_str().filter(|id| is_drive_id(id))?, r["createdTime"].as_str().unwrap_or("")))) {
                let n = drive_files(&token, r.0)?.len();
                let label = if roots.len() == 1 { format!("Current backup ({})", files(n)) } else { format!("{} folder made {} ({})", w.drive, r.1.get(..10).unwrap_or(r.1), files(n)) };
                choices.push(Choice { id: r.0.to_string(), label });
            }
            choices
        }
    };
    if choices.is_empty() {
        return Err("There's no backup to restore yet.".into());
    }
    Ok(choices)
}

/// Restores backup `id` (from `list`) of `source` into the new folder `target`.
pub fn restore(source: Source, id: &str, s: &Settings, w: &Places, target: &Path, progress: &mut dyn FnMut(usize, usize)) -> Result<Done, String> {
    let at = (Path::new(&s.vault_path), Path::new(&s.backup_folder), target);
    let not_found = || Err::<Done, String>("That backup wasn't found. Close this and try again.".into());
    match source {
        Source::Folder => {
            if w.folder.is_empty() || !backup::is_snapshot(id) {
                return not_found();
            }
            let found = local_files(&Path::new(&w.folder).join(id))?;
            write_all(at, found, |path| fs::read(path).map_err(|e| format!("Couldn't read {}: {e}.", path.display())), progress)
        }
        Source::Github => {
            if !is_sha(id) {
                return not_found();
            }
            let token = github::load_token()?;
            let base = format!("/repos/{}/{}", s.github_user, w.repo);
            let found = github_files(&token, &base, id)?;
            // ponytail: one request per file; GitHub allows 5,000 an hour, plenty for a notes vault.
            write_all(at, found, |sha| github_blob(&token, &base, sha), progress)
        }
        Source::Dropbox => {
            let token = cloud::access_token(Provider::Dropbox)?;
            let found = dropbox_files(&token, &w.dropbox, &w.others)?;
            write_all(at, found, |fid| dropbox_file(&token, fid), progress)
        }
        Source::Google => {
            if !is_drive_id(id) {
                return not_found();
            }
            let token = cloud::access_token(Provider::Google)?;
            let found = drive_files(&token, id)?;
            write_all(at, found, |fid| drive_file(&token, fid), progress)
        }
    }
}

// ---------- commands ----------

static RUNNING: AtomicBool = AtomicBool::new(false);
/// The folder the last restore made, for "Show in Finder".
static LAST: Mutex<Option<PathBuf>> = Mutex::new(None);

fn settings(app: &AppHandle) -> Settings {
    app.state::<Mutex<Settings>>().lock().unwrap().clone()
}

#[tauri::command]
pub async fn restore_list(app: AppHandle, source: Source) -> Result<Vec<Choice>, String> {
    let s = settings(&app);
    crate::off_main(move || list(source, &s, &places(&s))).await
}

/// Where the open campaign backs up, for the Settings window.
#[tauri::command]
pub fn campaign_places(app: AppHandle) -> Places {
    places(&settings(&app))
}

/// The web page with the open campaign's backup on GitHub, Dropbox or Google Drive. Drive is opened
/// by folder id (`drive_id`, known after a backup); before that, a search for the folder's name.
fn web_page(source: Source, s: &Settings, w: &Places, drive_id: Option<&str>) -> String {
    let mut url;
    match source {
        Source::Folder => unreachable!("a backup folder is shown in the file manager, not the browser"),
        Source::Github => return format!("https://github.com/{}/{}", s.github_user, w.repo),
        Source::Dropbox => {
            url = tauri::Url::parse("https://www.dropbox.com/home/Apps/Lorekeeper").unwrap();
            url.path_segments_mut().unwrap().extend(w.dropbox.split('/').filter(|p| !p.is_empty()));
        }
        Source::Google => match drive_id {
            Some(id) => return format!("https://drive.google.com/drive/folders/{id}"),
            None => {
                url = tauri::Url::parse("https://drive.google.com/drive/search").unwrap();
                url.query_pairs_mut().append_pair("q", &w.drive);
            }
        },
    }
    url.into()
}

/// Shows the open campaign's backup: the folder in Finder (Explorer on Windows), the others in the browser.
#[tauri::command]
pub fn open_backup(app: AppHandle, source: Source) -> Result<(), String> {
    let s = settings(&app);
    let w = places(&s);
    if source == Source::Folder {
        // The campaign's own folder appears with its first backup; until then, the backup folder.
        let dir = [&w.folder, &s.backup_folder].into_iter().find(|d| !d.is_empty() && Path::new(d).is_dir());
        crate::open_external(dir.ok_or("The backup folder isn't there. If it's on a drive, connect it.")?);
        return Ok(());
    }
    let campaign = backup::backup_name(&s, &s.vault_path);
    let config = app.path().app_config_dir().ok();
    let drive_id = config.and_then(|dir| cloud::drive_folder_id(&dir, &s.google_user, &campaign));
    crate::open_external(web_page(source, &s, &w, drive_id.as_deref()));
    Ok(())
}

/// A new folder to restore into, in `parent` ("" = next to the notes folder).
#[tauri::command]
pub fn restore_target(app: AppHandle, parent: String) -> String {
    let vault = PathBuf::from(settings(&app).vault_path);
    let parent = if parent.is_empty() { vault.parent().unwrap_or(&vault).to_path_buf() } else { PathBuf::from(parent) };
    suggest(&parent, &backup::campaign_name(&vault.to_string_lossy()), Local::now().date_naive()).to_string_lossy().into_owned()
}

/// Downloads a backup into the new folder `target`, sending "restore-progress" [done, total] as it goes.
#[tauri::command]
pub async fn restore_start(app: AppHandle, source: Source, id: String, target: String) -> Result<Done, String> {
    if RUNNING.swap(true, SeqCst) {
        return Err("A restore is already running.".into());
    }
    let s = settings(&app);
    let handle = app.clone();
    let result = crate::off_main(move || {
        let mut progress = |done: usize, total: usize| {
            let _ = handle.emit("restore-progress", (done, total));
        };
        restore(source, &id, &s, &places(&s), Path::new(&target), &mut progress)
    })
    .await;
    RUNNING.store(false, SeqCst);
    if let Ok(done) = &result {
        *LAST.lock().unwrap() = Some(PathBuf::from(&done.target));
    }
    result
}

#[tauri::command]
pub fn restore_open() {
    if let Some(dir) = LAST.lock().unwrap().clone() {
        crate::open_external(dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dnd-notes-restore-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn backup_names_that_could_escape_are_refused() {
        for ok in ["Vex.md", "NPCs/Villains/Vex.md", "Sessions/Session 1.md", "Orte/Köln.md", ".obsidian/app.json", "a..b.md"] {
            assert_eq!(safe_rel(ok), Some(PathBuf::from(ok)), "{ok}");
        }
        for bad in ["", "/etc/passwd", "../x.md", "NPCs/../../x.md", "a//b.md", "./a.md", "a/./b.md", "NPCs/", "..", "a\0b.md"] {
            assert_eq!(safe_rel(bad), None, "{bad:?}");
        }
        // Backslashes, colons and drive letters are separators or prefixes only on Windows; elsewhere they're plain characters.
        for name in ["C:/Windows/x.md", "C:x.md", "..\\x.md", "NPCs\\..\\..\\x.md", "\\\\server\\share\\x.md", "CON.md", "nul", "com1.txt", "x.md.", "What?.md"] {
            assert_eq!(safe_rel(name).is_none(), cfg!(windows), "{name:?}");
        }
    }

    #[test]
    fn backup_paths_under_a_root() {
        assert_eq!(under_root("NPCs/Vex.md", ""), Some("NPCs/Vex.md"));
        assert_eq!(under_root("Curse/NPCs/Vex.md", "Curse"), Some("NPCs/Vex.md"));
        assert_eq!(under_root("Cursed/Vex.md", "Curse"), None, "a name that only starts the same isn't inside");
        assert_eq!(under_root("Curse", "Curse"), None);
        assert_eq!(under_root("Curse/", "Curse"), None);
        let files = vec![("A/x.md".to_string(), 1), ("B/y.md".to_string(), 2)];
        assert_eq!(rooted(files, "A"), vec![("x.md".to_string(), 1)]);
    }

    #[test]
    fn dated_copies_newest_first() {
        let dir = temp_dir("snapshots");
        for d in ["Lorekeeper backup 2026-10-01", "Lorekeeper backup 2026-10-03", "Photos", ".Lorekeeper backup 2026-10-04.partial", "Lorekeeper backup 2026-13-01"] {
            fs::create_dir_all(dir.join(d)).unwrap();
        }
        fs::write(dir.join("Lorekeeper backup 2026-10-05"), "a file, not a copy").unwrap();
        assert_eq!(snapshots(&dir).unwrap(), ["Lorekeeper backup 2026-10-03", "Lorekeeper backup 2026-10-01"]);
        assert!(snapshots(&dir.join("unplugged")).is_err());
        let s = Settings { backup_folder: dir.to_string_lossy().into_owned(), ..Settings::default() };
        let labels: Vec<String> = list(Source::Folder, &s, &places(&s)).unwrap().into_iter().map(|c| c.label).collect();
        assert_eq!(labels, ["2026-10-03", "2026-10-01"]);
        assert!(list(Source::Folder, &Settings::default(), &places(&Settings::default())).is_err(), "folder backup off");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn restore_reads_the_open_campaigns_backup() {
        let dir = temp_dir("campaigns");
        let (lore, side, backups) = (dir.join("Lore"), dir.join("Side"), dir.join("Backups"));
        let day = NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
        fs::create_dir_all(&backups).unwrap();
        for (vault, note, name) in [(&lore, "Vex.md", ""), (&side, "Bob.md", "Side")] {
            fs::create_dir_all(vault).unwrap();
            fs::write(vault.join(note), note).unwrap();
            backup::backup_campaign_to_folder(vault, &backups, name, day).unwrap();
        }
        let path = |p: &Path| p.to_string_lossy().into_owned();
        let main = Settings {
            vault_path: path(&lore),
            campaigns: vec![path(&lore), path(&side)],
            main_campaign: path(&lore),
            backup_folder: path(&backups),
            ..Settings::default()
        };
        let other = Settings { vault_path: path(&side), ..main.clone() };

        // The main campaign: the places from before campaigns, minus the other campaigns' Dropbox folders,
        // all of Campaigns/, so a renamed or removed campaign's old folder isn't restored with it either.
        let w = places(&main);
        assert_eq!((w.folder.as_str(), w.repo.as_str(), w.dropbox.as_str(), w.drive.as_str()), (path(&backups).as_str(), "lorekeeper-notes", "", "Lorekeeper"));
        assert_eq!((w.campaign.as_str(), w.others.clone()), ("Lore", vec!["Campaigns".to_string()]));
        // Notes with a Campaigns folder of their own keep it: then only the campaigns' folders are left out.
        fs::create_dir_all(lore.join("Campaigns")).unwrap();
        assert_eq!(places(&main).others, ["Campaigns/Side"]);
        fs::remove_dir_all(lore.join("Campaigns")).unwrap();
        // Another campaign: only its own places.
        let w = places(&other);
        assert_eq!((w.folder.as_str(), w.repo.as_str(), w.dropbox.as_str(), w.drive.as_str()), (path(&backups.join("Side")).as_str(), "lorekeeper-notes-side", "Campaigns/Side", "Lorekeeper - Side"));
        assert!(w.others.is_empty());
        // A backup name of your own moves every place, the main campaign's too.
        let named = Settings { backup_names: [(path(&side), "Curse of Strahd".to_string())].into(), ..other.clone() };
        let w = places(&named);
        assert_eq!((w.folder.as_str(), w.repo.as_str(), w.dropbox.as_str(), w.drive.as_str(), w.campaign.as_str()),
            (path(&backups.join("Curse of Strahd")).as_str(), "lorekeeper-notes-curse-of-strahd", "Campaigns/Curse of Strahd", "Lorekeeper - Curse of Strahd", "Side"));
        let w = places(&Settings { backup_names: [(path(&lore), "Phandelver".to_string())].into(), ..main.clone() });
        assert_eq!((w.folder.as_str(), w.repo.as_str(), w.dropbox.as_str(), w.others.len()), (path(&backups.join("Phandelver")).as_str(), "lorekeeper-notes-phandelver", "Campaigns/Phandelver", 0));

        // Their pages on the web.
        let signed_in = Settings { github_user: "vex".into(), ..main.clone() };
        let page = |source, s: &Settings, id| web_page(source, s, &places(s), id);
        assert_eq!(page(Source::Github, &signed_in, None), "https://github.com/vex/lorekeeper-notes");
        assert_eq!(page(Source::Dropbox, &main, None), "https://www.dropbox.com/home/Apps/Lorekeeper");
        assert_eq!(page(Source::Dropbox, &other, None), "https://www.dropbox.com/home/Apps/Lorekeeper/Campaigns/Side");
        assert_eq!(page(Source::Google, &other, Some("abc123")), "https://drive.google.com/drive/folders/abc123");
        assert_eq!(page(Source::Google, &other, None), "https://drive.google.com/drive/search?q=Lorekeeper+-+Side");

        for (s, note, other_note) in [(&main, "Vex.md", "Bob.md"), (&other, "Bob.md", "Vex.md")] {
            let w = places(s);
            let id = &list(Source::Folder, s, &w).unwrap()[0].id;
            let target = dir.join(format!("Restored {note}"));
            restore(Source::Folder, id, s, &w, &target, &mut |_, _| {}).unwrap();
            assert!(target.join(note).exists() && !target.join(other_note).exists() && !target.join("Side").exists(), "{note}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn suggested_folder_is_new() {
        let dir = temp_dir("suggest");
        let day = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        assert_eq!(suggest(&dir, "Lorekeeper", day), dir.join("Lorekeeper restored 2026-10-05"));
        fs::create_dir_all(dir.join("Lorekeeper restored 2026-10-05")).unwrap();
        fs::create_dir_all(dir.join("Lorekeeper restored 2026-10-05 (2)")).unwrap();
        assert_eq!(suggest(&dir, "Lorekeeper", day), dir.join("Lorekeeper restored 2026-10-05 (3)"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn remote_files_land_only_inside_the_new_folder() {
        let dir = temp_dir("write");
        let (vault, backups) = (dir.join("Lorekeeper"), dir.join("Backups"));
        fs::create_dir_all(vault.join("NPCs")).unwrap();
        fs::write(vault.join("NPCs/Vex.md"), "live").unwrap();
        fs::create_dir_all(backups.join("Lorekeeper backup 2026-10-01")).unwrap();
        let remote: HashMap<&str, &str> = [("NPCs/Vex.md", "# Vex"), ("Sessions/Session 1.md", "s1"), ("../evil.md", "x"), ("/abs.md", "x"), ("a/../../evil2.md", "x")].into();
        let listing = || {
            let mut v: Vec<(String, String)> = remote.keys().map(|k| (k.to_string(), k.to_string())).collect();
            v.sort();
            v.push(("NPCs/Vex.md".into(), "NPCs/Vex.md".into())); // listed twice
            v
        };
        let fetch = |k: &String| Ok(remote[k.as_str()].as_bytes().to_vec());

        // Refused before anything is written: inside the notes folder, inside a dated copy, an existing folder, a relative path.
        for bad in [vault.join("Restored"), backups.join("Lorekeeper backup 2026-10-01/Restored"), backups.clone(), PathBuf::from("Restored")] {
            assert!(write_all((&vault, &backups, &bad), listing(), fetch, &mut |_, _| {}).is_err(), "{bad:?}");
        }
        assert!(!vault.join("Restored").exists());
        assert!(write_all((&vault, &backups, &dir.join("Empty")), Vec::<(String, String)>::new(), fetch, &mut |_, _| {}).is_err());
        assert!(!dir.join("Empty").exists(), "no folder for a backup with nothing in it");

        let target = dir.join("Lorekeeper restored 2026-10-05");
        let mut seen = Vec::new();
        let done = write_all((&vault, &backups, &target), listing(), fetch, &mut |d, t| seen.push((d, t))).unwrap();
        assert_eq!(done.files, 2);
        assert_eq!(seen.last(), Some(&(3, 3)), "the duplicate counts as done too");
        assert_eq!(fs::read_to_string(target.join("NPCs/Vex.md")).unwrap(), "# Vex");
        assert_eq!(fs::read_to_string(target.join("Sessions/Session 1.md")).unwrap(), "s1");
        let mut skipped = done.skipped.clone();
        skipped.sort();
        assert_eq!(skipped, ["../evil.md", "/abs.md", "NPCs/Vex.md", "a/../../evil2.md"]);
        assert!(!dir.join("evil.md").exists() && !dir.join("evil2.md").exists());
        assert_eq!(fs::read_to_string(vault.join("NPCs/Vex.md")).unwrap(), "live", "the notes folder is untouched");

        // A failed download keeps what was written and says so.
        let target2 = dir.join("Second");
        let mut n = 0;
        let flaky = |k: &String| {
            n += 1;
            if n == 2 { Err("Dropbox said: busy (500)".to_string()) } else { Ok(remote[k.as_str()].as_bytes().to_vec()) }
        };
        let e = write_all((&vault, &backups, &target2), listing(), flaky, &mut |_, _| {}).unwrap_err();
        assert!(e.starts_with("Dropbox said: busy (500) 1 file restored to"), "{e}");
        assert!(target2.join("NPCs/Vex.md").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn folder_restore_round_trip() {
        let dir = temp_dir("folder");
        let (vault, backups) = (dir.join("Lorekeeper"), dir.join("Backups"));
        fs::create_dir_all(vault.join("NPCs/Villains")).unwrap();
        fs::create_dir_all(&backups).unwrap();
        fs::write(vault.join("NPCs/Villains/Vex.md"), "# Vex").unwrap();
        fs::write(vault.join("Loot.md"), "gold").unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
        backup::backup_to_folder(&vault, &backups, day).unwrap();
        fs::write(vault.join("Loot.md"), "changed since").unwrap();

        let vault_path = vault.to_string_lossy().into_owned();
        let s = Settings { main_campaign: vault_path.clone(), vault_path, backup_folder: backups.to_string_lossy().into_owned(), ..Settings::default() };
        let id = &list(Source::Folder, &s, &places(&s)).unwrap()[0].id;
        let target = dir.join("Lorekeeper restored 2026-10-05");
        let done = restore(Source::Folder, id, &s, &places(&s), &target, &mut |_, _| {}).unwrap();
        assert_eq!((done.files, done.skipped.len()), (2, 0));
        assert_eq!(fs::read_to_string(target.join("Loot.md")).unwrap(), "gold");
        assert_eq!(fs::read_to_string(target.join("NPCs/Villains/Vex.md")).unwrap(), "# Vex");
        assert_eq!(fs::read_to_string(vault.join("Loot.md")).unwrap(), "changed since");
        assert!(restore(Source::Folder, id, &s, &places(&s), &target, &mut |_, _| {}).is_err(), "never into an existing folder");
        for bad_id in ["../Lorekeeper", "Lorekeeper backup 2026-10-04/../../Lorekeeper", ""] {
            assert!(restore(Source::Folder, bad_id, &s, &places(&s), &dir.join("x"), &mut |_, _| {}).is_err(), "{bad_id}");
        }
        assert!(restore(Source::Github, "../../user", &s, &places(&s), &dir.join("x"), &mut |_, _| {}).is_err());
        assert!(restore(Source::Google, "x' or name contains '", &s, &places(&s), &dir.join("x"), &mut |_, _| {}).is_err());
        assert!(!dir.join("x").exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
