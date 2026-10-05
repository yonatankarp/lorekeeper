//! Backups: a dated copy of the vault in a folder of your choice, a commit to GitHub (github.rs)
//! and uploads to Dropbox, Google Drive or OneDrive (cloud.rs). One background thread runs them,
//! so two never overlap.

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering::Relaxed},
        mpsc, LazyLock, Mutex, OnceLock,
    },
    thread,
    time::Duration,
};

use chrono::{DateTime, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::{cloud, github, notify, Settings};

const SNAPSHOT_PREFIX: &str = "Lorekeeper backup ";
const KEEP: usize = 30;

// ---------- folder backup ----------

pub fn snapshot_name(date: NaiveDate) -> String {
    format!("{SNAPSHOT_PREFIX}{}", date.format("%Y-%m-%d"))
}

/// Exactly "Lorekeeper backup YYYY-MM-DD"; only these folders are ever pruned.
pub fn is_snapshot(name: &str) -> bool {
    name.strip_prefix(SNAPSHOT_PREFIX)
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok().filter(|date| date.format("%Y-%m-%d").to_string() == d))
        .is_some()
}

/// The snapshot folders to delete: every one but the newest KEEP.
pub fn old_snapshots(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut snaps: Vec<String> = names.into_iter().filter(|n| is_snapshot(n)).collect();
    snaps.sort_unstable_by(|a, b| b.cmp(a)); // the date sorts as text
    snaps.split_off(KEEP.min(snaps.len()))
}

/// The path with symlinks resolved as far as it exists, so /var/x and /private/var/x compare equal.
fn resolved(p: &Path) -> PathBuf {
    p.ancestors()
        .find_map(|a| Some(fs::canonicalize(a).ok()?.join(p.strip_prefix(a).ok()?)))
        .unwrap_or_else(|| p.to_path_buf())
}

/// The backup folder must not be the vault or inside it, or every backup would copy the backups.
/// Nor may the vault be one of the dated copies, which backups replace and prune.
pub fn check_folder(vault: &Path, folder: &Path) -> Result<(), String> {
    let (vault, folder) = (resolved(vault), resolved(folder));
    if folder.starts_with(&vault) {
        return Err("The backup folder can't be inside the notes folder. Pick one somewhere else, like Google Drive or a USB drive.".into());
    }
    let first = vault.strip_prefix(&folder).ok().and_then(|rest| rest.components().next());
    if first.is_some_and(|c| is_snapshot(&c.as_os_str().to_string_lossy())) {
        return Err("Your notes folder is one of the dated backup copies. Copy it somewhere else first (for example to Documents), then choose the copy.".into());
    }
    Ok(())
}

/// Every folder and file in the vault, each folder before its contents, skipping hidden entries
/// (.obsidian, .git, .DS_Store). Symlinked folders are skipped. Any read error fails the whole walk.
pub fn vault_entries(root: &Path) -> io::Result<Vec<(PathBuf, bool)>> {
    let (mut out, mut stack) = (Vec::new(), vec![root.to_path_buf()]);
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                stack.push(path.clone());
                out.push((path, true));
            } else if path.is_file() {
                out.push((path, false));
            }
        }
    }
    Ok(out)
}

/// Copies the vault to `<folder>/Lorekeeper backup <today>`, replacing today's copy only once the new
/// one is complete, then deletes all but the newest 30 dated copies.
pub fn backup_to_folder(vault: &Path, folder: &Path, today: NaiveDate) -> Result<(), String> {
    check_folder(vault, folder)?;
    // Never created here: an unplugged drive's mount point would quietly fill the internal disk.
    if !folder.is_dir() {
        return Err(format!("The backup folder isn't available ({}). Is the drive connected?", folder.display()));
    }
    let entries = vault_entries(vault).map_err(|e| format!("Couldn't read the notes folder: {e}"))?;
    if entries.iter().all(|(_, is_dir)| *is_dir) {
        return Err("The notes folder is empty, so there's nothing to back up.".into());
    }
    let name = snapshot_name(today);
    let (dest, tmp, old) = (folder.join(&name), folder.join(format!(".{name}.partial")), folder.join(format!(".{name}.old")));
    let _ = fs::remove_dir_all(&tmp);
    let copy = || -> io::Result<()> {
        fs::create_dir_all(&tmp)?;
        for (path, is_dir) in &entries {
            let to = tmp.join(path.strip_prefix(vault).unwrap());
            if *is_dir { fs::create_dir_all(&to)? } else { fs::copy(path, &to).map(drop)? }
        }
        Ok(())
    };
    if let Err(e) = copy() {
        let _ = fs::remove_dir_all(&tmp);
        return Err(format!("Couldn't copy the notes to the backup folder: {e}"));
    }
    // Windows can't rename onto an existing folder, so move today's old copy aside first.
    let _ = fs::remove_dir_all(&old);
    if dest.exists() {
        fs::rename(&dest, &old).map_err(|e| format!("Couldn't replace today's backup: {e}"))?;
    }
    if let Err(e) = fs::rename(&tmp, &dest) {
        let _ = fs::rename(&old, &dest);
        return Err(format!("Couldn't finish the backup: {e}"));
    }
    let _ = fs::remove_dir_all(&old);
    let dirs = fs::read_dir(folder).map_err(|e| e.to_string())?.flatten();
    let names = dirs.filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.file_name().to_string_lossy().into_owned());
    for name in old_snapshots(names) {
        let _ = fs::remove_dir_all(folder.join(name));
    }
    Ok(())
}

// ---------- status and scheduling ----------

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Target {
    /// RFC 3339 time of the last successful backup, "" if none.
    last_ok: String,
    last_error: String,
    warning: String,
    /// The vault version that last_ok captured; see VERSION.
    #[serde(skip)]
    done: u64,
    /// A failure notification was shown for the current failure streak.
    #[serde(skip)]
    notified: bool,
}

/// Kept in backup-status.json in the config folder so "once a day" survives restarts.
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Status {
    folder: Target,
    github: Target,
    dropbox: Target,
    google: Target,
    onedrive: Target,
    #[serde(skip_deserializing)]
    running: bool,
    /// Whether this build has the app IDs each sign-in needs.
    #[serde(skip_deserializing)]
    github_available: bool,
    #[serde(skip_deserializing)]
    dropbox_available: bool,
    #[serde(skip_deserializing)]
    google_available: bool,
    #[serde(skip_deserializing)]
    onedrive_available: bool,
}

/// Where a backup goes.
#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Folder,
    Github,
    Cloud(cloud::Provider),
}

impl Status {
    fn target(&mut self, kind: Kind) -> &mut Target {
        match kind {
            Kind::Folder => &mut self.folder,
            Kind::Github => &mut self.github,
            Kind::Cloud(cloud::Provider::Dropbox) => &mut self.dropbox,
            Kind::Cloud(cloud::Provider::Google) => &mut self.google,
            Kind::Cloud(cloud::Provider::Onedrive) => &mut self.onedrive,
        }
    }
}

static STATUS: LazyLock<Mutex<Status>> = LazyLock::new(Mutex::default);
/// Bumped whenever the app writes a note; a target whose `done` differs has changes to back up.
static VERSION: AtomicU64 = AtomicU64::new(0);
/// Wakes the backup thread; true = back up even if nothing changed.
static WAKE: OnceLock<mpsc::Sender<bool>> = OnceLock::new();

pub fn mark_changed() {
    VERSION.fetch_add(1, Relaxed);
}

/// Asks the backup thread to run the backups that are due (or all of them when `force`).
pub fn request(force: bool) {
    if let Some(tx) = WAKE.get() {
        let _ = tx.send(force);
    }
}

pub fn status() -> Status {
    let mut s = STATUS.lock().unwrap().clone();
    s.github_available = !github::GITHUB_CLIENT_ID.is_empty();
    s.dropbox_available = !cloud::DROPBOX_APP_KEY.is_empty();
    s.google_available = !cloud::GOOGLE_CLIENT_ID.is_empty();
    s.onedrive_available = !cloud::ONEDRIVE_CLIENT_ID.is_empty();
    s
}

fn status_file(app: &AppHandle) -> Option<PathBuf> {
    Some(app.path().app_config_dir().ok()?.join("backup-status.json"))
}

fn publish(app: &AppHandle) {
    if let Some(file) = status_file(app) {
        let _ = fs::write(file, serde_json::to_string_pretty(&*STATUS.lock().unwrap()).unwrap_or_default());
    }
    let _ = app.emit("backup-changed", status());
}

/// Forgets a target's history (new backup folder, signed out or in) and runs what's due.
pub fn reset(app: &AppHandle, kind: Kind) {
    *STATUS.lock().unwrap().target(kind) = Target::default();
    publish(app);
    request(false);
}

fn ran_today(t: &Target, today: NaiveDate) -> bool {
    DateTime::parse_from_rfc3339(&t.last_ok).is_ok_and(|d| d.with_timezone(&Local).date_naive() == today)
}

fn finish(app: &AppHandle, kind: Kind, version: u64, result: Result<String, String>) {
    let mut s = STATUS.lock().unwrap();
    let t = s.target(kind);
    let first_failure = match result {
        Ok(warning) => {
            *t = Target { last_ok: Local::now().to_rfc3339(), warning, done: version, ..Target::default() };
            None
        }
        Err(e) => {
            let first = !t.notified;
            t.notified = true;
            t.last_error = e.clone();
            first.then_some(e)
        }
    };
    drop(s);
    // Failures always notify, once per streak.
    if let Some(e) = first_failure {
        let title = match kind {
            Kind::Folder => "Couldn't back up to your folder".to_string(),
            Kind::Github => "Couldn't back up to GitHub".to_string(),
            Kind::Cloud(p) => format!("Couldn't back up to {}", p.name()),
        };
        notify(app, &title, &e);
    }
}

/// Backs up each enabled target that has unsaved changes or no backup from today.
fn run(app: &AppHandle, force: bool) {
    let settings = app.state::<Mutex<Settings>>().lock().unwrap().clone();
    let (version, today) = (VERSION.load(Relaxed), Local::now().date_naive());
    let on = [(Kind::Folder, &settings.backup_folder), (Kind::Github, &settings.github_user)]
        .into_iter()
        .chain(cloud::ALL.map(|p| (Kind::Cloud(p), settings.cloud_user(p))))
        .filter(|(_, setting)| !setting.is_empty());
    let due: Vec<Kind> = {
        let mut s = STATUS.lock().unwrap();
        on.map(|(kind, _)| kind).filter(|k| force || s.target(*k).done != version || !ran_today(s.target(*k), today)).collect()
    };
    if due.is_empty() {
        return;
    }
    STATUS.lock().unwrap().running = true;
    publish(app);
    let vault = Path::new(&settings.vault_path);
    for kind in due {
        let result = match kind {
            Kind::Folder => backup_to_folder(vault, Path::new(&settings.backup_folder), today).map(|()| String::new()),
            Kind::Github => github::load_token().and_then(|token| github::backup(&token, &settings.github_user, &settings.github_repo, vault)),
            Kind::Cloud(p) => app
                .path()
                .app_config_dir()
                .map_err(|e| e.to_string())
                .and_then(|dir| cloud::backup(p, settings.cloud_user(p), &dir, vault)),
        };
        finish(app, kind, version, result);
    }
    STATUS.lock().unwrap().running = false;
    publish(app);
}

/// Starts the backup thread: a first check a minute after launch (once the network is up),
/// then every 30 minutes and whenever `request` is called.
pub fn start(app: AppHandle) {
    if let Some(file) = status_file(&app) {
        let saved = fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok());
        *STATUS.lock().unwrap() = saved.unwrap_or_default();
    }
    let (tx, rx) = mpsc::channel();
    let _ = WAKE.set(tx);
    thread::spawn(move || {
        let mut wait = Duration::from_secs(60);
        loop {
            let force = match rx.recv_timeout(wait) {
                Ok(force) => force,
                Err(mpsc::RecvTimeoutError::Timeout) => false,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            };
            let force = rx.try_iter().fold(force, |a, b| a || b); // requests that piled up meanwhile
            wait = Duration::from_secs(30 * 60);
            run(&app, force);
        }
    });
}
