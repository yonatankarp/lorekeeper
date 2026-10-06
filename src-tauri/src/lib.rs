use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write},
    path::{Component, Path, PathBuf},
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, RunEvent, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use tauri_plugin_notification::NotificationExt;

mod backup;
mod cloud;
mod dndbeyond;
mod dropbox;
mod gdrive;
mod github;
mod obsidian;
mod restore;
mod updater;
mod watch;

// ---------- notes on disk: an Obsidian-compatible vault (default <Documents>/Lorekeeper) ----------
//   Sessions/Session N.md   written by the hotkeys
//   Sessions/Session N/<PC>.md   the same in a campaign shared with your party: one file per player
//   PCs/ NPCs/ Locations/ Items/ Factions/ Quests/ Lore/   your pages
//   Templates/   starting text for "New page" (Obsidian's {{title}} / {{date}} syntax)
//   Templates/.seeded   the default templates written so far, one name per line

const VAULT_FOLDERS: [&str; 9] = ["Sessions", "PCs", "NPCs", "Locations", "Items", "Factions", "Quests", "Lore", "Templates"];

const TEMPLATES: [(&str, &str); 7] = [
    ("PC", "---\ntype: pc\nplayer:\nclass:\nrace:\nlevel:\nbackground:\nalignment:\nportrait:\ndndbeyond:\n---\n# {{title}}\n\n## Appearance\n\n## Personality\n\n## Goals\n\n## Relationships\n\n## Notes\n"),
    ("NPC", "---\ntype: npc\nrace:\nrole:\nlocation:\nstatus: alive\nfirst-met: {{date}}\n---\n# {{title}}\n\n## Description\n\n## Notes\n"),
    ("Location", "---\ntype: location\nregion:\n---\n# {{title}}\n\n## Description\n\n## Notable people\n\n## Notes\n"),
    ("Item", "---\ntype: item\nrarity:\nowner:\n---\n# {{title}}\n\n## Description\n\n## Notes\n"),
    ("Faction", "---\ntype: faction\nleader:\nbase:\n---\n# {{title}}\n\n## Goals\n\n## Members\n\n## Notes\n"),
    ("Quest", "---\ntype: quest\nstatus: open\ngiver:\nlocation:\nreward:\nstarted: {{date}}\n---\n# {{title}}\n\n## Objective\n\n## Leads\n\n## Log\n"),
    ("Lore", "---\ntype: lore\ncategory:\nsource:\nlearned: {{date}}\n---\n# {{title}}\n\n## Description\n\n## Notes\n"),
];

/// The templates every vault had before Templates/.seeded existed.
const FIRST_TEMPLATES: [&str; 5] = ["PC", "NPC", "Location", "Item", "Faction"];

/// Passed by the launch-at-login entry so a login start doesn't pop the window open.
const LOGIN_ARG: &str = "--from-login";

/// Serializes writes from the hotkeys and the editor so neither loses the other's change.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// The notes folder chosen in Settings.
pub(crate) fn notes_dir(app: &AppHandle) -> PathBuf {
    PathBuf::from(&app.state::<Mutex<Settings>>().lock().unwrap().vault_path)
}

/// Documents/Lorekeeper: the default notes folder, and the folder that holds your campaigns and their shared Templates/.
fn default_vault(app: &AppHandle) -> PathBuf {
    let base = app.path().document_dir().or_else(|_| app.path().home_dir());
    base.expect("no home directory").join("Lorekeeper")
}
pub(crate) use default_vault as library_dir;

/// The folder whose Templates/ a campaign uses: the Lorekeeper folder's, shared by every campaign in it, unless the
/// campaign lives elsewhere and has a Templates/ of its own.
pub(crate) fn templates_home(vault: &Path, library: &Path) -> PathBuf {
    let elsewhere = vault != library && vault.parent() != Some(library);
    if elsewhere && vault.join("Templates").is_dir() { vault.to_path_buf() } else { library.to_path_buf() }
}

/// Moves a campaign's own Templates/ into the Lorekeeper folder's, once, for a campaign inside it. A template the shared
/// folder already has with other text stays where it was; the record of seeded defaults is merged.
fn share_templates(vault: &Path, library: &Path) -> io::Result<()> {
    let own = vault.join("Templates");
    if vault.parent() != Some(library) || !own.is_dir() {
        return Ok(());
    }
    let shared = library.join("Templates");
    fs::create_dir_all(&shared)?;
    for entry in fs::read_dir(&own)?.flatten() {
        let (from, to) = (entry.path(), shared.join(entry.file_name()));
        if !to.exists() {
            fs::rename(&from, &to)?;
        } else if entry.file_name() == ".seeded" {
            let mut names: Vec<String> = fs::read_to_string(&to)?.lines().map(str::to_owned).collect();
            names.extend(fs::read_to_string(&from)?.lines().map(str::to_owned).filter(|n| !names.contains(n)).collect::<Vec<_>>());
            fs::write(&to, names.join("\n") + "\n")?;
            fs::remove_file(&from)?;
        } else if fs::read(&from)? == fs::read(&to)? {
            fs::remove_file(&from)?;
        }
    }
    let _ = fs::remove_dir(&own); // only goes when empty
    Ok(())
}

/// Creates the standard folders and writes each default template into `home`/Templates (see templates_home) at most once
/// ever (recorded in Templates/.seeded), so a template you delete stays deleted and new defaults still reach old vaults.
fn create_vault_folders(dir: &Path, home: &Path) -> io::Result<()> {
    let fresh_templates = !home.join("Templates").exists();
    VAULT_FOLDERS.iter().filter(|f| **f != "Templates").try_for_each(|f| fs::create_dir_all(dir.join(f)))?;
    fs::create_dir_all(home.join("Templates"))?;
    let dir = home;
    let record = dir.join("Templates/.seeded");
    let mut seeded: Vec<String> = match fs::read_to_string(&record) {
        Ok(text) => text.lines().map(str::to_owned).collect(),
        Err(e) if e.kind() == io::ErrorKind::NotFound && !fresh_templates => FIRST_TEMPLATES.map(String::from).to_vec(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    let before = seeded.len();
    for (name, body) in TEMPLATES {
        if seeded.iter().any(|s| s == name) {
            continue;
        }
        let file = dir.join("Templates").join(format!("{name}.md"));
        if !file.exists() {
            fs::write(file, body)?; // never over a template of your own with the same name
        }
        seeded.push(name.to_owned());
    }
    if seeded.len() != before || !record.exists() {
        fs::write(&record, seeded.join("\n") + "\n")?;
    }
    Ok(())
}

/// "Session 3.md" (a file) or "Session 3" (a shared session's folder): 3.
fn session_number(name: &str, is_dir: bool) -> Option<u32> {
    let n = name.strip_prefix("Session ")?;
    if is_dir { n } else { n.strip_suffix(".md")? }.parse().ok()
}

/// The newest session's number, whether it's a Session N.md file or a Session N/ folder.
fn latest_session(dir: &Path) -> io::Result<u32> {
    let sessions = dir.join("Sessions");
    fs::create_dir_all(&sessions)?;
    Ok(fs::read_dir(sessions)?
        .filter_map(|e| {
            let e = e.ok()?;
            session_number(e.file_name().to_str()?, e.file_type().ok()?.is_dir())
        })
        .max()
        .unwrap_or(0))
}

fn session_path(dir: &Path, n: u32) -> PathBuf {
    dir.join("Sessions").join(format!("Session {n}.md"))
}

/// A shared session: Session N/ holds one file per player, so synced copies never collide.
fn session_folder(dir: &Path, n: u32) -> PathBuf {
    dir.join("Sessions").join(format!("Session {n}"))
}

/// Where your quick notes for session `n` go: Session N.md, or in a shared campaign (`me` = your PC's name)
/// your own file in its folder, Session N/<PC>.md.
fn notes_file(dir: &Path, n: u32, me: Option<&str>) -> PathBuf {
    match me {
        Some(me) => session_folder(dir, n).join(format!("{me}.md")),
        None => session_path(dir, n),
    }
}

/// "Session 4" for Session 4.md, or for a player's file in Session 4/.
fn session_name(path: &Path) -> String {
    let folder = path.parent().filter(|d| d.file_name().and_then(|n| n.to_str()).and_then(|n| session_number(n, true)).is_some());
    let name = folder.map_or_else(|| path.file_stem(), |d| d.file_name());
    name.unwrap_or_default().to_string_lossy().into_owned()
}

/// Session `n`'s notes file (see notes_file), made when it isn't there yet with Obsidian properties, so sessions can
/// be listed as a table. A player's file also names its author.
fn start_file(dir: &Path, n: u32, me: Option<&str>) -> io::Result<PathBuf> {
    let path = notes_file(dir, n, me);
    fs::create_dir_all(path.parent().unwrap())?;
    let date = chrono::Local::now().format("%Y-%m-%d");
    let author = me.map(|me| format!("author: \"[[{me}]]\"\n")).unwrap_or_default();
    match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut f) => {
            f.write_all(format!("---\nsession: {n}\ndate: {date}\n{author}---\n# Session {n} - {date}\n\n").as_bytes())?;
            watch::wrote(&path);
        }
        Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e),
        Err(_) => {}
    }
    Ok(path)
}

/// Whether anyone wrote in session folder `dir` in the last 12 hours: a session that's still going.
fn going(dir: &Path, now: std::time::SystemTime) -> bool {
    let files = fs::read_dir(dir).into_iter().flatten().flatten().filter(|e| !e.file_name().to_string_lossy().starts_with('.')); // not .DS_Store
    let mut times = files.filter_map(|e| e.metadata().ok()?.modified().ok());
    times.any(|t| stale_hours(t, now).is_none())
}

/// Starts the next session and returns your notes file in it. In a shared campaign, a session a teammate started in
/// the last 12 hours that you haven't written in yet is joined instead, so a party pressing New session together
/// stays in one session.
fn new_session(dir: &Path, me: Option<&str>) -> io::Result<PathBuf> {
    let n = latest_session(dir)?;
    let join = me.is_some() && n > 0 && !notes_file(dir, n, me).exists() && going(&session_folder(dir, n), std::time::SystemTime::now());
    start_file(dir, if join { n } else { n + 1 }, me)
}

/// Your notes file in the newest session. Sessions only roll over via "New session", never by date, so a game running
/// past midnight stays in one session. A shared campaign never writes to a Session N.md file: when the newest session
/// is one (from before sharing), your notes start the next session, as a folder.
fn current_session(dir: &Path, me: Option<&str>) -> io::Result<PathBuf> {
    match latest_session(dir)? {
        0 => new_session(dir, me),
        n if me.is_some() && !session_folder(dir, n).is_dir() => start_file(dir, n + 1, me),
        n => start_file(dir, n, me),
    }
}

/// Appends one note as a single line; multi-line selections are collapsed so the
/// one-note-per-line format survives. Returns the saved text (empty = nothing saved).
fn append_note(dir: &Path, me: Option<&str>, text: &str) -> io::Result<String> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if !text.is_empty() {
        let _guard = WRITE_LOCK.lock().unwrap();
        let time = chrono::Local::now().format("%H:%M");
        let path = current_session(dir, me)?;
        let mut file = fs::OpenOptions::new().append(true).open(&path)?;
        writeln!(file, "- {time} {text}")?;
        watch::wrote(&path);
    }
    Ok(text)
}

/// The time and text of a quick-note line, "- HH:MM text".
fn note_line(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix("- ")?;
    let (time, text) = (rest.get(..5)?, rest.get(5..)?.strip_prefix(' ')?);
    let b = time.as_bytes();
    (b[2] == b':' && [0, 1, 3, 4].iter().all(|&i| b[i].is_ascii_digit())).then_some((time, text))
}

/// The last quick-note line in a session: its byte range (without the line break), time and text.
fn last_note_line(content: &str) -> Option<(std::ops::Range<usize>, &str, &str)> {
    let (mut start, mut found) = (0, None);
    for raw in content.split_inclusive('\n') {
        let line = raw.trim_end_matches(['\n', '\r']);
        if let Some((time, text)) = note_line(line) {
            found = Some((start..start + line.len(), time, text));
        }
        start += raw.len();
    }
    found
}

/// The session with its last note rewritten to `new`, keeping its time, or None if that note no longer reads `old`.
fn replace_last_note(content: &str, old: &str, new: &str) -> Option<String> {
    let (range, time, text) = last_note_line(content)?;
    (text == old).then(|| format!("{}- {time} {new}{}", &content[..range.start], &content[range.end..]))
}

/// ↑ in the quick box: rewrites the last note in place if it still reads `old`. If another note arrived since,
/// the text is appended as a new note instead, so nothing is lost. Returns whether it was fixed in place;
/// empty text keeps the note as it was.
fn fix_last_note(dir: &Path, me: Option<&str>, old: &str, text: &str) -> io::Result<bool> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return Ok(true);
    }
    let _guard = WRITE_LOCK.lock().unwrap();
    let path = current_session(dir, me)?;
    watch::wrote(&path);
    if let Some(fixed) = replace_last_note(&fs::read_to_string(&path)?, old, &text) {
        fs::write(&path, fixed)?;
        return Ok(true);
    }
    let time = chrono::Local::now().format("%H:%M");
    let mut file = fs::OpenOptions::new().append(true).open(&path)?;
    writeln!(file, "- {time} {text}")?;
    Ok(false)
}

/// Whole hours since `modified` when that is 12 or more: time to offer a new session.
fn stale_hours(modified: std::time::SystemTime, now: std::time::SystemTime) -> Option<u64> {
    let hours = now.duration_since(modified).ok()?.as_secs() / 3600;
    (hours >= 12).then_some(hours)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StaleSession {
    next: u32,
    idle_hours: u64,
}

/// The newest session when your notes file in it hasn't changed in 12 hours. A fresh vault, or a session you have
/// no notes in yet, has none (and no file gets created).
fn stale_session(dir: &Path, me: Option<&str>, now: std::time::SystemTime) -> io::Result<Option<StaleSession>> {
    let n = latest_session(dir)?;
    let path = notes_file(dir, n, me);
    let Ok(text) = fs::read_to_string(&path) else { return Ok(None) };
    if n == 0 || !text.lines().any(|l| l.starts_with("- ")) {
        return Ok(None);
    }
    let modified = fs::metadata(path)?.modified()?;
    Ok(stale_hours(modified, now).map(|idle_hours| StaleSession { next: n + 1, idle_hours }))
}

/// Turns a path from the webview into a file inside the vault. Only plain relative `.md`
/// paths pass: no `..`, no absolute paths or drive prefixes.
fn vault_file(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let p = Path::new(rel);
    let ok = p.extension().is_some_and(|e| e == "md") && p.components().all(|c| matches!(c, Component::Normal(_)));
    if ok { Ok(root.join(p)) } else { Err(format!("Not a note in the vault: {rel}")) }
}

/// Images the page view shows and the editor saves (pasted or dropped); the same list as isImage in images.js.
const IMAGE_EXTS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "svg"];
const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

fn is_image(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGE_EXTS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// vault_file for images: only plain relative paths with an image extension.
pub(crate) fn vault_image(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let p = Path::new(rel);
    let ok = is_image(p) && p.components().all(|c| matches!(c, Component::Normal(_)));
    if ok { Ok(root.join(p)) } else { Err(format!("Not an image in the vault: {rel}")) }
}

/// Lets the page view load images from the notes folder through the asset protocol. Every campaign's folder
/// stays allowed (they're all your own notes), since Tauri's scope can only grow and switching back must work.
fn allow_vault_images(app: &AppHandle, dir: &str) {
    let _ = app.asset_protocol_scope().allow_directory(dir, true);
}

fn rel_path(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

#[derive(Serialize)]
struct Note {
    path: String,
    content: String,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct Vault {
    folders: Vec<String>,
    notes: Vec<Note>,
    /// Image files, for ![[map.png]] embeds.
    images: Vec<String>,
    current_session: String,
    has_obsidian: bool,
    obsidian_installed: bool,
}

/// Every folder and `.md` note, skipping hidden entries (.obsidian, .trash, .DS_Store).
fn walk(root: &Path, dir: &Path, vault: &mut Vault) -> io::Result<()> {
    for entry in fs::read_dir(dir)?.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            vault.folders.push(rel_path(root, &path));
            walk(root, &path, vault)?;
        } else if path.extension().is_some_and(|e| e == "md") {
            if let Ok(content) = fs::read_to_string(&path) {
                vault.notes.push(Note { path: rel_path(root, &path), content });
            }
        } else if is_image(&path) {
            vault.images.push(rel_path(root, &path));
        }
    }
    Ok(())
}

/// Names offered as suggestions in the quick box: every note except templates and sessions,
/// sorted case-insensitively, each name once.
fn page_names_of<'a>(paths: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut names: Vec<String> = paths
        .into_iter()
        .filter(|p| !p.starts_with("Templates/") && !p.starts_with("Sessions/"))
        .filter_map(|p| Some(Path::new(p).file_stem()?.to_string_lossy().into_owned()))
        .collect();
    names.sort_by_cached_key(|n| (n.to_lowercase(), n.clone()));
    names.dedup();
    names
}

/// What to write when the editor saves `content`, given the file it loaded (`base`) and what is
/// on disk now. Lines appended in the meantime (hotkey notes during a game) are kept; any other
/// outside change is a conflict (None) so the editor never silently overwrites it.
fn merge_save(disk: &str, base: &str, content: &str) -> Option<String> {
    if disk == base {
        return Some(content.to_string());
    }
    let tail = disk.strip_prefix(base)?;
    let sep = if content.is_empty() || content.ends_with('\n') { "" } else { "\n" };
    Some(format!("{content}{sep}{tail}"))
}

// ---------- settings.json (in the app's config folder) ----------

const THEMES: [&str; 3] = ["system", "light", "dark"];
const SESSION_VIEWS: [&str; 2] = ["timeline", "journal"];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
struct Settings {
    quick_note: String,
    capture: String,
    /// Empty in older files; load_settings fills in the default folder.
    vault_path: String,
    /// Every campaign's notes folder, vault_path (the active one) among them. Older files have none;
    /// load_settings makes vault_path the only one.
    campaigns: Vec<String>,
    /// Notes folder -> the name its backups go under, when you gave it one (see backup::backup_name).
    /// Kept after a campaign is removed, so adding the folder again carries on its backups.
    backup_names: BTreeMap<String, String>,
    /// Notes folder -> how it's shared with your party; a campaign not in here is yours alone.
    sharing: BTreeMap<String, Sharing>,
    theme: String,
    editor_font_size: u32,
    session_view: String,
    notifications: bool,
    /// Kept by the OS (autostart), never in the file; see current_settings.
    launch_at_login: bool,
    /// Where dated copies of the vault go; "" = off.
    backup_folder: String,
    github_repo: String,
    /// Set by signing in to GitHub, cleared by signing out; the settings window can't change it.
    github_user: String,
    /// The Dropbox, Google and Microsoft accounts backed up to; set and cleared like github_user.
    dropbox_user: String,
    google_user: String,
    /// Check GitHub Releases for a new version at launch and once a day (see updater.rs).
    auto_update: bool,
    /// Optional global shortcuts; "" = off.
    new_session: String,
    new_page: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            quick_note: "CmdOrCtrl+Alt+N".into(),
            capture: "CmdOrCtrl+Shift+S".into(),
            vault_path: String::new(),
            campaigns: Vec::new(),
            backup_names: BTreeMap::new(),
            sharing: BTreeMap::new(),
            theme: "system".into(),
            editor_font_size: 15,
            session_view: "timeline".into(),
            notifications: true,
            launch_at_login: false,
            backup_folder: String::new(),
            github_repo: "lorekeeper-notes".into(),
            github_user: String::new(),
            dropbox_user: String::new(),
            google_user: String::new(),
            auto_update: true,
            new_session: String::new(),
            new_page: String::new(),
        }
    }
}

/// A campaign folder the party shares (say through Google Drive): each player's quick notes go to a file of their own.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
struct Sharing {
    shared: bool,
    /// The PC you play in it ("PCs/Sibling 5.md"); its name names your files.
    me: String,
}

const PICK_PC: &str = "Pick your character in Settings > General first";

/// In a campaign shared with your party, your PC's name (the stem of `me`), which your notes files are named after;
/// None in a campaign of your own. Shared but no character picked yet is an error, so no note lands in the wrong place.
fn author(s: &Settings) -> Result<Option<String>, String> {
    match s.sharing.get(&s.vault_path) {
        Some(c) if c.shared => Path::new(&c.me).file_stem().map(|n| Some(n.to_string_lossy().into_owned())).ok_or_else(|| PICK_PC.into()),
        _ => Ok(None),
    }
}

impl Settings {
    fn cloud_user(&self, p: cloud::Provider) -> &String {
        match p {
            cloud::Provider::Dropbox => &self.dropbox_user,
            cloud::Provider::Google => &self.google_user,
        }
    }

    fn with_cloud_user(mut self, p: cloud::Provider, account: String) -> Self {
        *match p {
            cloud::Provider::Dropbox => &mut self.dropbox_user,
            cloud::Provider::Google => &mut self.google_user,
        } = account;
        self
    }
}

fn settings_file(app: &AppHandle) -> tauri::Result<PathBuf> {
    Ok(app.path().app_config_dir()?.join("settings.json"))
}

fn write_settings(file: &Path, s: &Settings) -> io::Result<()> {
    let mut json = serde_json::to_value(s)?;
    json.as_object_mut().unwrap().remove("launchAtLogin");
    fs::create_dir_all(file.parent().unwrap())?;
    fs::write(file, serde_json::to_string_pretty(&json)?)
}

/// Reads `file`, writing the defaults only when it's missing so a typo never wipes your settings.
/// The first time, an older `<default vault>/settings.json` (hotkeys only) is moved over instead.
fn load_settings(file: &Path, default_vault: &Path) -> Result<Settings, String> {
    let legacy = default_vault.join("settings.json");
    let migrate = !file.exists() && legacy.exists();
    let src = if migrate { &legacy } else { file };
    let mut s: Settings = match fs::read_to_string(src) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", src.display()))?,
        Err(_) => Settings::default(),
    };
    if s.vault_path.is_empty() {
        s.vault_path = default_vault.to_string_lossy().into_owned();
    }
    if s.campaigns.is_empty() {
        s.campaigns = vec![s.vault_path.clone()];
    }
    // 0.3.0 had Tome and Dungeon next to plain Light and Dark; now they are the light and dark themes.
    match s.theme.as_str() {
        "tome" => s.theme = "light".into(),
        "dungeon" => s.theme = "dark".into(),
        _ => {}
    }
    if !file.exists() && write_settings(file, &s).is_ok() && migrate {
        let _ = fs::remove_file(&legacy);
    }
    Ok(s)
}

/// Checks settings from the settings window; returns them with the font size clamped.
fn validate(mut s: Settings) -> Result<Settings, String> {
    let parse = |name: &str, keys: &str| keys.parse::<Shortcut>().map_err(|_| format!("{name}: \"{keys}\" isn't a valid shortcut."));
    let quick = parse("Quick note", &s.quick_note)?;
    let capture = parse("Save selection", &s.capture)?;
    let mut all = vec![("Quick note", quick), ("Save selection", capture)];
    for (name, keys) in [("New session", &s.new_session), ("New page", &s.new_page)] {
        if !keys.is_empty() {
            all.push((name, parse(name, keys)?));
        }
    }
    for (i, (a, ka)) in all.iter().enumerate() {
        if let Some((b, _)) = all[i + 1..].iter().find(|(_, kb)| kb == ka) {
            return Err(format!("{a} and {b} need different shortcuts."));
        }
    }
    // The capture hotkey's Cmd/Ctrl is reused for the copy keystroke (see send_copy).
    let (primary, key) = if cfg!(target_os = "macos") { (Modifiers::SUPER, "⌘") } else { (Modifiers::CONTROL, "Ctrl") };
    if !capture.mods.contains(primary) {
        return Err(format!("The Save selection shortcut must include {key}."));
    }
    if !Path::new(&s.vault_path).is_absolute() {
        return Err(format!("The notes folder must be a full path, not \"{}\".", s.vault_path));
    }
    if let Some(c) = s.campaigns.iter().find(|c| !Path::new(c).is_absolute()) {
        return Err(format!("A campaign's folder must be a full path, not \"{c}\"."));
    }
    if !s.campaigns.contains(&s.vault_path) {
        return Err("The campaign you're in can't be removed. Switch to another one first.".into());
    }
    s.backup_names = s.backup_names.into_iter().map(|(k, v)| (k, v.trim().to_string())).filter(|(_, v)| !v.is_empty()).collect();
    backup::check_campaigns(&s)?;
    if let Some(c) = s.sharing.values().find(|c| !c.me.is_empty() && !(c.me.starts_with("PCs/") && vault_file(Path::new("/"), &c.me).is_ok())) {
        return Err(format!("\"{}\" isn't a page in PCs/.", c.me));
    }
    if !THEMES.contains(&s.theme.as_str()) {
        return Err(format!("Unknown theme \"{}\".", s.theme));
    }
    if !SESSION_VIEWS.contains(&s.session_view.as_str()) {
        return Err(format!("Unknown session view \"{}\".", s.session_view));
    }
    if !s.backup_folder.is_empty() {
        if !Path::new(&s.backup_folder).is_absolute() {
            return Err(format!("The backup folder must be a full path, not \"{}\".", s.backup_folder));
        }
        for vault in &s.campaigns {
            backup::check_folder(Path::new(vault), Path::new(&s.backup_folder))?;
        }
    }
    let repo_ok = |r: &str| !r.is_empty() && r != "." && r != ".." && r.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    if !repo_ok(&s.github_repo) {
        return Err(format!("\"{}\" isn't a valid GitHub repository name.", s.github_repo));
    }
    s.editor_font_size = s.editor_font_size.clamp(11, 24);
    Ok(s)
}

fn current_settings(app: &AppHandle) -> Settings {
    let mut s = app.state::<Mutex<Settings>>().lock().unwrap().clone();
    s.launch_at_login = app.autolaunch().is_enabled().unwrap_or(false);
    s
}

/// Turns launch at login on or off and keeps the tray's checkmark in step.
fn set_launch_at_login(app: &AppHandle, on: bool) -> Result<(), String> {
    let al = app.autolaunch();
    let result = if on { al.enable() } else { al.disable() };
    if on && result.is_ok() {
        remember_login_item(app);
    }
    let _ = app.state::<CheckMenuItem<tauri::Wry>>().set_checked(al.is_enabled().unwrap_or(false));
    result.map_err(|e| format!("Launch at login: {e}"))
}

/// The login item stores this program's path, which moves when the app is renamed or reinstalled
/// elsewhere. Records the path it was registered with, so refresh_login_item only acts once.
fn login_item_marker(app: &AppHandle) -> Option<(PathBuf, String)> {
    let exe = std::env::current_exe().ok()?.to_string_lossy().into_owned();
    Some((app.path().app_config_dir().ok()?.join("login-item"), exe))
}

fn remember_login_item(app: &AppHandle) {
    if let Some((marker, exe)) = login_item_marker(app) {
        let _ = fs::write(marker, exe);
    }
}

/// Re-registers an existing login item that points at an old program path (once per move).
fn refresh_login_item(app: &AppHandle) {
    let al = app.autolaunch();
    let Some((marker, exe)) = login_item_marker(app) else { return };
    if !al.is_enabled().unwrap_or(false) || fs::read_to_string(&marker).ok().as_deref() == Some(exe.as_str()) {
        return;
    }
    if al.disable().is_ok() && al.enable().is_ok() {
        remember_login_item(app);
    }
}

// ---------- helpers ----------

fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}

/// Success messages, which can be turned off in Settings. Errors and warnings use notify.
fn notify_saved(app: &AppHandle, title: &str, body: &str) {
    if app.state::<Mutex<Settings>>().lock().unwrap().notifications {
        notify(app, title, body);
    }
}

fn preview(text: &str) -> String {
    let mut p: String = text.chars().take(60).collect();
    if text.chars().count() > 60 {
        p.push('…');
    }
    p
}

/// Windows that make Lorekeeper a normal app (Dock icon, Cmd+Tab) while one of them is open.
#[cfg(target_os = "macos")]
const APP_WINDOWS: [&str; 2] = ["main", "settings"];

#[cfg(target_os = "macos")]
fn app_window_visible(app: &AppHandle, except: &str) -> bool {
    APP_WINDOWS
        .iter()
        .filter(|l| **l != except)
        .any(|l| app.get_webview_window(l).is_some_and(|w| w.is_visible().unwrap_or(false)))
}

fn show_window(app: &AppHandle, label: &str) {
    // dismiss() hides the whole app on macOS; a hidden app's windows won't show until it's unhidden.
    #[cfg(target_os = "macos")]
    {
        let _ = app.show();
        if APP_WINDOWS.contains(&label) {
            let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
        }
    }
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Opens a path or URL with the system's default handler.
fn open_external(target: impl AsRef<std::ffi::OsStr>) {
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(target_os = "windows")]
    let cmd = "explorer";
    #[cfg(target_os = "linux")]
    let cmd = "xdg-open";
    let _ = std::process::Command::new(cmd).arg(target).spawn();
}

/// A file of the newest session to open, without making one (in a shared campaign that would start a session for
/// everyone): your own, else its Session N.md, else a teammate's.
fn latest_file(dir: &Path, me: Option<&str>) -> Option<PathBuf> {
    let n = latest_session(dir).ok().filter(|&n| n > 0)?;
    let first = || fs::read_dir(session_folder(dir, n)).ok()?.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "md"));
    [notes_file(dir, n, me), session_path(dir, n)].into_iter().find(|p| p.is_file()).or_else(first)
}

/// Opens the current session in Obsidian once the notes folder is in a vault. Otherwise starts Obsidian
/// with the folder's path on the clipboard, ready for "Open folder as vault", or shows the folder.
fn open_notes(app: &AppHandle) {
    let dir = notes_dir(app);
    if obsidian::vault_root(&dir).is_some() {
        let me = author(&app.state::<Mutex<Settings>>().lock().unwrap()).ok().flatten();
        match latest_file(&dir, me.as_deref()) {
            Some(session) => obsidian::open(&session),
            None => open_external(&dir),
        }
    } else if obsidian::installed() {
        obsidian::launch();
        let _ = app.clipboard().write_text(dir.to_string_lossy());
        notify(app, "Add your notes to Obsidian", "Click “Open folder as vault” and choose the folder (its path is copied).");
    } else {
        open_external(&dir);
        notify(app, "Obsidian isn't installed", "Get it free at obsidian.md. Showing the notes folder instead.");
    }
}

/// Presses Cmd+C (macOS) / Ctrl+C in whatever app is in front.
fn send_copy() -> Result<(), String> {
    use enigo::{Direction::*, Enigo, Key, Keyboard, Settings};
    let mut e = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
    let primary = if cfg!(target_os = "macos") { Key::Meta } else { Key::Control };
    // The capture hotkey's Shift/Alt may still be physically held; release them so the
    // front app sees a plain copy (on Windows a held Shift would make it Ctrl+Shift+C).
    let keys = [(Key::Shift, Release), (Key::Alt, Release), (primary, Press), (Key::Unicode('c'), Click), (primary, Release)];
    keys.into_iter().try_for_each(|(k, d)| e.key(k, d)).map_err(|e| e.to_string())
}

/// Saves the selected text, or the clipboard if nothing is selected.
fn capture_selection(app: &AppHandle) {
    const SENTINEL: &str = "lorekeeper: waiting for copy";
    let cb = app.clipboard();
    let old = cb.read_text().unwrap_or_default();
    let _ = cb.write_text(SENTINEL);
    if let Err(e) = send_copy() {
        let _ = cb.write_text(&old);
        notify(app, "Can't read selection", &format!("{e}. On macOS, allow Accessibility access."));
        return;
    }
    let start = Instant::now();
    let mut selected = None;
    while start.elapsed() < Duration::from_millis(400) {
        thread::sleep(Duration::from_millis(20));
        match cb.read_text() {
            Ok(t) if t != SENTINEL => {
                selected = Some(t);
                break;
            }
            _ => {}
        }
    }
    // ponytail: restores text only; an image on the clipboard is lost. Use read_image/write_image if that matters.
    let _ = cb.write_text(&old);

    // Distinct titles make a silently dropped keystroke (missing permission) visible.
    let (title, text) = match selected.filter(|t| !t.trim().is_empty()) {
        Some(t) => ("Saved selection", t),
        None => ("Saved from clipboard", old),
    };
    match note_target(app).and_then(|(dir, me)| append_note(&dir, me.as_deref(), &text).map_err(|e| e.to_string())) {
        Ok(saved) if saved.is_empty() => notify(app, "Nothing to save", "No text selected or copied."),
        Ok(saved) => {
            emit_changed(app);
            notify_saved(app, title, &preview(&saved));
        }
        Err(e) => notify(app, "Couldn't save note", &e),
    }
}

/// Registers both hotkeys. Returns the ones that couldn't be registered, e.g. "Quick note (CmdOrCtrl+Alt+N)".
fn register_shortcuts(app: &AppHandle, s: &Settings) -> Vec<String> {
    let gs = app.global_shortcut();
    let quick = gs.on_shortcut(s.quick_note.as_str(), |app, _, e| {
        if e.state == ShortcutState::Pressed {
            remember_front_app();
            show_window(app, "capture");
        }
    });
    // On release, so the hotkey's own letter key is no longer down when we press copy.
    let capture = gs.on_shortcut(s.capture.as_str(), |app, _, e| {
        if e.state == ShortcutState::Released {
            let app = app.clone();
            thread::spawn(move || capture_selection(&app));
        }
    });
    let mut failed: Vec<String> = [("Quick note", &s.quick_note, quick), ("Save selection", &s.capture, capture)]
        .into_iter()
        .filter(|(_, _, result)| result.is_err())
        .map(|(name, keys, _)| format!("{name} ({keys})"))
        .collect();
    // Optional shortcuts ("" = off).
    let mut optional = |name: &str, keys: &str, action: fn(&AppHandle)| {
        let pressed = move |app: &AppHandle, _: &_, e: tauri_plugin_global_shortcut::ShortcutEvent| {
            if e.state == ShortcutState::Pressed {
                action(app);
            }
        };
        if !keys.is_empty() && gs.on_shortcut(keys, pressed).is_err() {
            failed.push(format!("{name} ({keys})"));
        }
    };
    optional("New session", &s.new_session, start_new_session);
    optional("New page", &s.new_page, open_new_page);
    failed
}

/// The New Session hotkey and tray item: starts the next session file without opening a window.
fn start_new_session(app: &AppHandle) {
    match note_target(app).and_then(|(dir, me)| new_session(&dir, me.as_deref()).map_err(|e| e.to_string())) {
        Ok(p) => {
            emit_changed(app);
            backup::request(false); // captures the session that just ended
            notify_saved(app, "New session started", &session_name(&p));
        }
        Err(e) => notify(app, "Couldn't start session", &e),
    }
}

/// The New Page hotkey: brings up the notes window with the New page dialog open.
fn open_new_page(app: &AppHandle) {
    show_window(app, "main");
    let _ = app.emit("new-page", ());
}

// ---------- commands for the windows ----------

/// The open campaign's folder and, when it's shared with your party, your PC's name (see author).
fn note_target(app: &AppHandle) -> Result<(PathBuf, Option<String>), String> {
    let s = app.state::<Mutex<Settings>>();
    let s = s.lock().unwrap();
    Ok((PathBuf::from(&s.vault_path), author(&s)?))
}

/// From the quick box. Returns the session the note went into ("Session 3") for the box to show.
#[tauri::command]
fn save_note(app: AppHandle, text: String, start_new: Option<bool>) -> Result<String, String> {
    let saved = note_target(&app).and_then(|(dir, me)| {
        let me = me.as_deref();
        let fresh = if start_new == Some(true) { new_session(&dir, me).map(|_| backup::request(false)) } else { Ok(()) };
        fresh.and_then(|_| append_note(&dir, me, &text)).and_then(|_| current_session(&dir, me)).map_err(|e| e.to_string())
    });
    match saved {
        Ok(path) => {
            emit_changed(&app);
            Ok(session_name(&path))
        }
        Err(e) => {
            notify(&app, "Couldn't save note", &e);
            Err(e)
        }
    }
}

/// Your last note in the current session, without its "- HH:MM ", for ↑ in the quick box.
#[tauri::command]
fn last_note(app: AppHandle) -> Option<String> {
    let (dir, me) = note_target(&app).ok()?;
    let n = latest_session(&dir).ok().filter(|&n| n > 0)?;
    last_note_line(&fs::read_to_string(notes_file(&dir, n, me.as_deref())).ok()?).map(|(_, _, text)| text.to_owned())
}

/// From the quick box after ↑: fixes the last note. Returns what the box shows.
#[tauri::command]
fn fix_note(app: AppHandle, old: String, text: String) -> Result<String, String> {
    let fixed = note_target(&app).and_then(|(dir, me)| {
        let me = me.as_deref();
        fix_last_note(&dir, me, &old, &text).and_then(|fixed| Ok((fixed, current_session(&dir, me)?))).map_err(|e| e.to_string())
    });
    match fixed {
        Ok((fixed, path)) => {
            emit_changed(&app);
            let session = session_name(&path);
            Ok(if fixed { format!("Fixed in {session}") } else { format!("Saved to {session} as a new note (the last one changed)") })
        }
        Err(e) => {
            notify(&app, "Couldn't save note", &e);
            Err(e)
        }
    }
}

/// Hides the quick-note box and hands focus back to the app you were in.
#[tauri::command]
fn dismiss(app: AppHandle, restore_focus: Option<bool>) {
    if let Some(w) = app.get_webview_window("capture") {
        let _ = w.hide();
    }
    // Enter/Esc: back to the app the box opened over, even with Lorekeeper's window open behind.
    // Clicking away (restore_focus false) leaves focus where the click went.
    let restored = return_to_front_app(restore_focus.unwrap_or(true));
    // Nothing to go back to: on macOS, step aside if no other Lorekeeper window is open.
    #[cfg(target_os = "macos")]
    if !restored && !app_window_visible(&app, "capture") {
        let _ = app.hide();
    }
    #[cfg(not(target_os = "macos"))]
    let _ = restored;
}

/// The app in front when the quick-note box opened (macOS process id, Windows window handle).
static FRONT_APP: Mutex<Option<isize>> = Mutex::new(None);

/// The app in front now, unless it's Lorekeeper itself (macOS process id, Windows window handle).
fn front_app() -> Option<isize> {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSRunningApplication, NSWorkspace};
        let me = NSRunningApplication::currentApplication().processIdentifier();
        let front = NSWorkspace::sharedWorkspace().frontmostApplication().map(|a| a.processIdentifier());
        front.filter(|&pid| pid != me).map(|pid| pid as isize)
    }
    #[cfg(windows)]
    {
        let hwnd = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
        (!hwnd.is_invalid()).then_some(hwnd.0 as isize)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    None
}

/// Brings an app from front_app() back to the front. False if it couldn't.
fn activate(front: isize) -> bool {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
        NSRunningApplication::runningApplicationWithProcessIdentifier(front as libc::pid_t)
            .is_some_and(|a| a.activateWithOptions(NSApplicationActivationOptions::empty()))
    }
    #[cfg(windows)]
    {
        use windows::Win32::{Foundation::HWND, UI::WindowsAndMessaging::SetForegroundWindow};
        unsafe { SetForegroundWindow(HWND(front as *mut core::ffi::c_void)) }.as_bool()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = front;
        false
    }
}

fn remember_front_app() {
    *FRONT_APP.lock().unwrap() = front_app();
}

/// Forgets the remembered app, and with `restore` brings it back to the front. False if nothing was restored.
fn return_to_front_app(restore: bool) -> bool {
    FRONT_APP.lock().unwrap().take().filter(|_| restore).is_some_and(activate)
}

/// A window opened from the tray menu and the app that was in front then (macOS: opening one
/// activates Lorekeeper, and closing it would leave you there instead of where you were).
#[cfg(target_os = "macos")]
static MENU_FRONT: Mutex<Option<(&str, isize)>> = Mutex::new(None);

/// From the tray menu: shows the window, and on macOS closing it takes you back to the app you were in.
fn show_from_menu(app: &AppHandle, label: &'static str) {
    #[cfg(target_os = "macos")]
    {
        *MENU_FRONT.lock().unwrap() = front_app().map(|front| (label, front));
    }
    show_window(app, label);
}

/// After the app writes notes: tells the windows and marks the vault as needing a backup.
fn emit_changed(app: &AppHandle) {
    backup::mark_changed();
    let _ = app.emit("vault-changed", ());
}

#[tauri::command]
fn read_vault(app: AppHandle) -> Result<Vault, String> {
    let root = notes_dir(&app);
    let mut vault = Vault::default();
    walk(&root, &root, &mut vault).map_err(|e| e.to_string())?;
    let n = latest_session(&root).map_err(|e| e.to_string())?;
    if n > 0 {
        let folder = session_folder(&root, n);
        vault.current_session = rel_path(&root, &if folder.is_dir() { folder } else { session_path(&root, n) });
    }
    vault.has_obsidian = obsidian::vault_root(&root).is_some();
    vault.obsidian_installed = obsidian::installed();
    add_shared_templates(&root, &library_dir(&app), &mut vault);
    Ok(vault)
}

/// The shared templates (see templates_home), listed as the campaign's Templates/ so "New page" finds them.
fn add_shared_templates(root: &Path, library: &Path, vault: &mut Vault) {
    let home = templates_home(root, library);
    if home == root {
        return; // in the campaign itself: walk listed them
    }
    let Ok(entries) = fs::read_dir(home.join("Templates")) else { return };
    vault.folders.push("Templates".into());
    for path in entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "md")) {
        if let (Some(name), Ok(content)) = (path.file_name(), fs::read_to_string(&path)) {
            vault.notes.push(Note { path: format!("Templates/{}", name.to_string_lossy()), content });
        }
    }
}

/// Shares a campaign's templates (see share_templates), then makes its folders and seeds the templates.
fn prepare_campaign(app: &AppHandle, vault: &Path) -> io::Result<()> {
    let library = library_dir(app);
    share_templates(vault, &library)?;
    create_vault_folders(vault, &templates_home(vault, &library))
}

/// Note names for the quick box's suggestions; an unreadable vault just means no suggestions.
#[tauri::command]
fn page_names(app: AppHandle) -> Vec<String> {
    let root = notes_dir(&app);
    let mut vault = Vault::default();
    let _ = walk(&root, &root, &mut vault);
    page_names_of(vault.notes.iter().map(|n| n.path.as_str()))
}

/// Saves an edited note; see merge_save. Returns what was written.
#[tauri::command]
fn save_file(app: AppHandle, path: String, content: String, base: String) -> Result<String, String> {
    let file = vault_file(&notes_dir(&app), &path)?;
    let _guard = WRITE_LOCK.lock().unwrap();
    let disk = fs::read_to_string(&file).unwrap_or_else(|_| base.clone()); // deleted elsewhere: recreate
    let merged = merge_save(&disk, &base, &content).ok_or("conflict")?;
    fs::write(&file, &merged).map_err(|e| e.to_string())?;
    watch::wrote(&file);
    backup::mark_changed();
    Ok(merged)
}

/// Moves a note to the system Trash / Recycle Bin, so a mistake can be undone from there.
#[tauri::command]
fn delete_file(app: AppHandle, path: String) -> Result<(), String> {
    let file = vault_file(&notes_dir(&app), &path)?;
    if !file.is_file() {
        return Err(format!("{path} doesn't exist."));
    }
    {
        let _guard = WRITE_LOCK.lock().unwrap();
        // The file-manager call needs no "control Finder" permission prompt; the file can still be dragged back out.
        #[cfg(target_os = "macos")]
        let trash = {
            use trash::macos::{DeleteMethod, TrashContextExtMacos};
            let mut trash = trash::TrashContext::default();
            trash.set_delete_method(DeleteMethod::NsFileManager);
            trash
        };
        #[cfg(not(target_os = "macos"))]
        let trash = trash::TrashContext::default();
        trash.delete(&file).map_err(|e| format!("Couldn't move {path} to the Trash: {e}"))?;
        watch::wrote(&file);
        // The last file of a shared session (undoing New session): the empty folder goes too, so it isn't the newest session.
        if let Some(dir) = file.parent().filter(|d| d.file_name().and_then(|n| n.to_str()).and_then(|n| session_number(n, true)).is_some()) {
            let _ = fs::remove_dir(dir); // only goes when empty
        }
    }
    emit_changed(&app);
    Ok(())
}

/// Creates a new note; never overwrites an existing one.
#[tauri::command]
fn create_file(app: AppHandle, path: String, content: String) -> Result<(), String> {
    let file = vault_file(&notes_dir(&app), &path)?;
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&file).map_err(|e| match e.kind() {
        io::ErrorKind::AlreadyExists => format!("{path} already exists."),
        _ => e.to_string(),
    })?;
    f.write_all(content.as_bytes()).map_err(|e| e.to_string())?;
    watch::wrote(&file);
    backup::mark_changed();
    Ok(())
}

/// Renames a note; never overwrites another one. A case-only rename ("mirela.md" to "Mirela.md")
/// finds the same file on a case-insensitive disk, so it goes through a temporary name.
fn rename_note(root: &Path, from: &str, to: &str) -> Result<(), String> {
    let (src, dst) = (vault_file(root, from)?, vault_file(root, to)?);
    // Hotkey notes find the current session by its "Session N" name, so sessions keep theirs, and players' files their PC's.
    let session = |rel: &str| {
        let r = rel.strip_prefix("Sessions/").unwrap_or_default();
        r.split_once('/').map_or(session_number(r, false), |(folder, _)| session_number(folder, true)).is_some()
    };
    if session(from) || session(to) {
        return Err("Sessions keep their \"Session N\" names, so hotkey notes find the current one.".into());
    }
    let _guard = WRITE_LOCK.lock().unwrap();
    if !src.is_file() {
        return Err(format!("{from} doesn't exist."));
    }
    // Listed under its exact name: a file of its own, not the source seen through case-insensitivity.
    let listed = dst.parent().and_then(|d| fs::read_dir(d).ok()).is_some_and(|mut entries| {
        entries.any(|e| e.is_ok_and(|e| Some(e.file_name().as_os_str()) == dst.file_name()))
    });
    let case_only = from.to_lowercase() == to.to_lowercase() && !listed;
    if dst.exists() && !case_only {
        return Err(format!("{to} already exists."));
    }
    let tmp = dst.with_extension("md.renaming");
    let renamed = if case_only && !tmp.exists() {
        fs::rename(&src, &tmp).and_then(|_| fs::rename(&tmp, &dst))
    } else {
        fs::rename(&src, &dst)
    };
    watch::wrote(&src);
    watch::wrote(&dst);
    renamed.map_err(|e| format!("Couldn't rename {from}: {e}"))
}

#[tauri::command]
fn rename_file(app: AppHandle, from: String, to: String) -> Result<(), String> {
    rename_note(&notes_dir(&app), &from, &to)?;
    backup::mark_changed(); // no vault-changed event: the window follows the renamed page itself
    Ok(())
}

/// Saves a pasted or dropped image (base64) into the vault, making its folder; never overwrites a file.
#[tauri::command]
fn save_image(app: AppHandle, path: String, data: String) -> Result<(), String> {
    use base64::Engine as _;
    let file = vault_image(&notes_dir(&app), &path)?;
    if data.len() > MAX_IMAGE_BYTES.div_ceil(3) * 4 {
        return Err("Images can be at most 20 MB.".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| e.to_string())?;
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&file).map_err(|e| match e.kind() {
        io::ErrorKind::AlreadyExists => format!("{path} already exists."),
        _ => e.to_string(),
    })?;
    f.write_all(&bytes).map_err(|e| e.to_string())?;
    watch::wrote(&file);
    backup::mark_changed();
    Ok(())
}

/// An image's bytes, for making its thumbnail: images from the asset protocol can't be read back out of a canvas.
#[tauri::command]
async fn read_image(app: AppHandle, path: String) -> Result<tauri::ipc::Response, String> {
    let file = vault_image(&notes_dir(&app), &path)?;
    off_main(move || {
        if fs::metadata(&file).map_err(|e| e.to_string())?.len() > MAX_IMAGE_BYTES as u64 {
            return Err("Images can be at most 20 MB.".into());
        }
        fs::read(&file).map(tauri::ipc::Response::new).map_err(|e| e.to_string())
    })
    .await
}

/// The name of an image's thumbnail for the connections map: its vault, path, size and modified time, so a
/// replaced image gets a new one.
fn thumbnail_name(root: &Path, rel: &str, len: u64, modified: Option<std::time::SystemTime>) -> String {
    let nanos = modified.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
    let key = format!("{}\n{rel}\n{len}\n{nanos}", root.display());
    format!("{}.png", sha1_smol::Sha1::from(key).digest())
}

/// Where an image's thumbnail is kept: the app's cache folder, never the vault, so backups and Obsidian don't see it.
/// ponytail: thumbnails of deleted or replaced images stay behind; prune the folder if it ever grows big.
fn thumbnail_file(app: &AppHandle, rel: &str) -> Result<PathBuf, String> {
    let root = notes_dir(app);
    let meta = fs::metadata(vault_image(&root, rel)?).map_err(|e| e.to_string())?;
    let dir = app.path().app_cache_dir().map_err(|e| e.to_string())?.join("thumbnails");
    Ok(dir.join(thumbnail_name(&root, rel, meta.len(), meta.modified().ok())))
}

/// An image's cached thumbnail; empty when there's none yet.
#[tauri::command]
async fn thumbnail(app: AppHandle, path: String) -> Result<tauri::ipc::Response, String> {
    off_main(move || Ok(tauri::ipc::Response::new(fs::read(thumbnail_file(&app, &path)?).unwrap_or_default()))).await
}

/// Keeps a thumbnail (base64 PNG) the window made of an image; see thumbnail_file.
#[tauri::command]
fn save_thumbnail(app: AppHandle, path: String, data: String) -> Result<(), String> {
    use base64::Engine as _;
    if data.len() > 1024 * 1024 {
        return Err("Thumbnails are small; this isn't one.".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| e.to_string())?;
    let file = thumbnail_file(&app, &path)?;
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(&file, bytes).map_err(|e| e.to_string())
}

/// Starts the next session (see new_session) and returns your notes file in it.
#[tauri::command]
fn start_session(app: AppHandle) -> Result<String, String> {
    let (root, me) = note_target(&app)?;
    let path = new_session(&root, me.as_deref()).map_err(|e| e.to_string())?;
    emit_changed(&app);
    backup::request(false); // captures the session that just ended
    Ok(rel_path(&root, &path))
}

/// For the quick box: offers "Start Session N?" when your notes in the current session have gone quiet.
#[tauri::command]
fn session_status(app: AppHandle) -> Option<StaleSession> {
    let (dir, me) = note_target(&app).ok()?;
    stale_session(&dir, me.as_deref(), std::time::SystemTime::now()).ok().flatten()
}

/// The PC pages of a campaign ("PCs/Sibling 5.md"), open or not, for picking the one you play in Settings.
#[tauri::command]
fn campaign_pcs(app: AppHandle, path: String) -> Result<Vec<String>, String> {
    if !app.state::<Mutex<Settings>>().lock().unwrap().campaigns.contains(&path) {
        return Err(format!("{path} isn't one of your campaigns."));
    }
    Ok(pc_pages(Path::new(&path)))
}

fn pc_pages(root: &Path) -> Vec<String> {
    let mut vault = Vault::default();
    let _ = walk(root, &root.join("PCs"), &mut vault); // no PCs/ yet: none
    let mut pcs: Vec<String> = vault.notes.into_iter().map(|n| n.path).collect();
    pcs.sort_by_cached_key(|p| p.to_lowercase());
    pcs
}

/// Opens a page in Obsidian. Outside a vault it can't, so the window shows how to add the notes folder;
/// `launch` (the guide's "Open Obsidian" button) also starts Obsidian then.
#[tauri::command]
fn open_in_obsidian(app: AppHandle, path: String, launch: Option<bool>) -> Result<obsidian::Opened, String> {
    let root = notes_dir(&app);
    let file = vault_file(&root, &path)?;
    if obsidian::vault_root(&root).is_some() {
        obsidian::open(&file);
        return Ok(obsidian::Opened::Opened { opened: true });
    }
    let installed = obsidian::installed();
    if installed && launch == Some(true) {
        obsidian::launch();
    }
    Ok(obsidian::Opened::NeedsVault { needs_vault: true, path: root.to_string_lossy().into_owned(), installed })
}

/// Opens web links from notes in the browser; anything else is refused.
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    let parsed = tauri::Url::parse(&url).map_err(|e| e.to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!("Not a web link: {url}"));
    }
    open_external(parsed.as_str());
    Ok(())
}

#[tauri::command]
fn get_settings(app: AppHandle) -> Settings {
    current_settings(&app)
}

/// Checks and applies every setting, saves them, then tells all windows. On error nothing changes.
#[tauri::command]
fn save_settings(app: AppHandle, settings: Settings) -> Result<Settings, String> {
    // The open campaign only changes through switch_campaign, so a window
    // that hasn't heard of a switch yet can't switch back or remove the campaign that's open now.
    let active = app.state::<Mutex<Settings>>().lock().unwrap().clone();
    let mut new = validate(Settings { vault_path: active.vault_path, ..settings })?;
    let old = current_settings(&app);
    new.github_user = old.github_user.clone();
    for p in cloud::ALL {
        new = new.with_cloud_user(p, old.cloud_user(p).clone());
    }
    if new.backup_folder != old.backup_folder && !new.backup_folder.is_empty() {
        fs::create_dir_all(&new.backup_folder).map_err(|e| format!("Backup folder: {e}"))?;
    }
    if (&new.quick_note, &new.capture, &new.new_session, &new.new_page) != (&old.quick_note, &old.capture, &old.new_session, &old.new_page) {
        let gs = app.global_shortcut();
        let _ = gs.unregister_all();
        let failed = register_shortcuts(&app, &new);
        if !failed.is_empty() {
            let _ = gs.unregister_all();
            register_shortcuts(&app, &old);
            return Err(format!("{} couldn't be registered. Another app may be using it.", failed.join(" and ")));
        }
    }
    if new.launch_at_login != old.launch_at_login {
        set_launch_at_login(&app, new.launch_at_login)?;
    }
    store_settings(&app, &new)?;
    if backup::backup_name(&new, &new.vault_path) != backup::backup_name(&old, &old.vault_path) {
        // A new backup name: every backup starts over in its new place, right away.
        for kind in [backup::Kind::Folder, backup::Kind::Github].into_iter().chain(cloud::ALL.map(backup::Kind::Cloud)) {
            backup::reset(&app, kind);
        }
    } else if new.backup_folder != old.backup_folder {
        backup::reset(&app, backup::Kind::Folder); // a new place: back up there right away
    }
    fill_campaign_menu(&app); // names and the list may have changed
    Ok(new)
}

/// Saves settings as they are (no checks) and tells every window.
fn store_settings(app: &AppHandle, new: &Settings) -> Result<(), String> {
    let file = settings_file(app).map_err(|e| e.to_string())?;
    write_settings(&file, new).map_err(|e| format!("Couldn't save settings: {e}"))?;
    *app.state::<Mutex<Settings>>().lock().unwrap() = new.clone();
    let _ = app.emit("settings-changed", new);
    fill_campaign_menu(app);
    Ok(())
}

/// Makes another campaign the active one: the notes, hotkey notes and backups all follow. The main
/// window calls it once the page it has open is saved, so no edit lands in the other campaign.
#[tauri::command]
fn switch_campaign(app: AppHandle, path: String) -> Result<(), String> {
    let old = current_settings(&app);
    if path == old.vault_path {
        return Ok(());
    }
    if !old.campaigns.contains(&path) {
        return Err(format!("{path} isn't one of your campaigns."));
    }
    prepare_campaign(&app, Path::new(&path)).map_err(|e| format!("{}: {e}", backup::campaign_name(&path)))?;
    store_settings(&app, &Settings { vault_path: path.clone(), ..old.clone() })?;
    allow_vault_images(&app, &path);
    backup::left(&old.vault_path); // its last changes still get backed up
    emit_changed(&app);
    backup::request(false);
    Ok(())
}

/// The tray's Campaign menu: every campaign, the active one checked.
fn fill_campaign_menu(app: &AppHandle) {
    let Some(menu) = app.try_state::<Submenu<tauri::Wry>>() else { return };
    let s = app.state::<Mutex<Settings>>().lock().unwrap().clone();
    for item in menu.items().unwrap_or_default() {
        let _ = menu.remove(&item);
    }
    for path in &s.campaigns {
        let id = format!("{CAMPAIGN_ITEM}{path}");
        if let Ok(item) = CheckMenuItem::with_id(app, id, backup::backup_name(&s, path), true, *path == s.vault_path, None::<&str>) {
            let _ = menu.append(&item);
        }
    }
}

/// Tray menu ids of the campaigns: this followed by the folder.
const CAMPAIGN_ITEM: &str = "campaign:";

#[tauri::command]
fn open_settings(app: AppHandle) {
    show_window(&app, "settings");
}

/// None when cancelled. Async because the blocking dialog must not run on the main thread.
#[tauri::command]
async fn pick_folder(app: AppHandle, window: tauri::WebviewWindow, title: String, start: String) -> Option<String> {
    let mut dialog = app.dialog().file().set_title(title);
    if !start.is_empty() {
        dialog = dialog.set_directory(start);
    }
    let path = dialog.set_parent(&window).blocking_pick_folder()?.into_path().ok()?;
    Some(path.to_string_lossy().into_owned())
}

// ---------- backups (see backup.rs, github.rs and cloud.rs) ----------

#[tauri::command]
fn backup_now() {
    backup::request(true);
}

#[tauri::command]
fn backup_status() -> backup::Status {
    backup::status()
}

/// Runs blocking work (network, credential store) off the main thread.
async fn off_main<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

/// Step 1: a code to enter on github.com.
#[tauri::command]
async fn github_sign_in_start() -> Result<github::DeviceCode, String> {
    off_main(github::start_sign_in).await
}

/// Step 2: waits for approval, keeps the token in the credential store and returns the GitHub login.
#[tauri::command]
async fn github_sign_in_wait(app: AppHandle) -> Result<String, String> {
    let login = off_main(|| {
        let token = github::wait_sign_in()?;
        let login = github::user_login(&token)?;
        github::save_token(&token)?;
        Ok(login)
    })
    .await?;
    store_settings(&app, &Settings { github_user: login.clone(), ..current_settings(&app) })?;
    backup::reset(&app, backup::Kind::Github); // first backup right away
    Ok(login)
}

#[tauri::command]
fn github_sign_in_cancel() {
    github::cancel_sign_in();
}

#[tauri::command]
async fn github_sign_out(app: AppHandle) -> Result<(), String> {
    off_main(github::delete_token).await?;
    store_settings(&app, &Settings { github_user: String::new(), ..current_settings(&app) })?;
    backup::reset(&app, backup::Kind::Github);
    Ok(())
}

/// Signs in to Dropbox, Google or Microsoft in the browser and returns the account. Resolves once
/// the browser comes back, or fails when cancelled or after 5 minutes.
#[tauri::command]
async fn cloud_sign_in(app: AppHandle, provider: cloud::Provider) -> Result<String, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let account = off_main(move || cloud::sign_in(provider, &dir)).await?;
    store_settings(&app, &current_settings(&app).with_cloud_user(provider, account.clone()))?;
    backup::reset(&app, backup::Kind::Cloud(provider)); // first backup right away
    Ok(account)
}

#[tauri::command]
fn cloud_sign_in_cancel() {
    cloud::cancel_sign_in();
}

#[tauri::command]
async fn cloud_sign_out(app: AppHandle, provider: cloud::Provider) -> Result<(), String> {
    off_main(move || cloud::delete_token(provider)).await?;
    store_settings(&app, &current_settings(&app).with_cloud_user(provider, String::new()))?;
    backup::reset(&app, backup::Kind::Cloud(provider));
    Ok(())
}

#[tauri::command]
fn open_vault_folder(app: AppHandle) {
    open_external(notes_dir(&app));
}

// ---------- app ----------

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let autostart = app.autolaunch().is_enabled().unwrap_or(false);
    let review = MenuItem::with_id(app, "review", "Open Lorekeeper…", true, None::<&str>)?;
    let new = MenuItem::with_id(app, "new", "New Session", true, None::<&str>)?;
    let folder = MenuItem::with_id(app, "folder", "Open in Obsidian", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let update = MenuItem::with_id(app, "update", "Check for Updates…", true, None::<&str>)?;
    let login = CheckMenuItem::with_id(app, "login", "Launch at Login", true, autostart, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let campaigns = Submenu::with_id(app, "campaigns", "Campaign", true)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&review, &new, &folder, &campaigns, &sep, &settings, &update, &login, &quit])?;
    app.manage(login); // for set_launch_at_login
    app.manage(campaigns); // for fill_campaign_menu
    fill_campaign_menu(app);

    // macOS menu bar: monochrome template icon that follows light/dark. Elsewhere: the colored app icon,
    // since a black icon would vanish on Windows' dark taskbar.
    #[cfg(target_os = "macos")]
    let tray = TrayIconBuilder::new()
        .icon(tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?)
        .icon_as_template(true);
    #[cfg(not(target_os = "macos"))]
    let tray = TrayIconBuilder::new().icon(app.default_window_icon().unwrap().clone());

    tray
        .tooltip("Lorekeeper")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "review" => show_from_menu(app, "main"),
            "new" => start_new_session(app),
            "folder" => open_notes(app),
            "settings" => show_from_menu(app, "settings"),
            "update" => updater::check(app, true),
            "login" => {
                let on = app.autolaunch().is_enabled().unwrap_or(false);
                if let Err(e) = set_launch_at_login(app, !on) {
                    notify(app, "Couldn't change Launch at Login", &e);
                }
                let _ = app.emit("settings-changed", current_settings(app));
            }
            "quit" => app.exit(0),
            // The main window saves the page it has open first, then switches (see switch_campaign).
            id => {
                if let Some(path) = id.strip_prefix(CAMPAIGN_ITEM) {
                    let _ = app.emit_to("main", "switch-campaign", path);
                    fill_campaign_menu(app); // the click toggled the item's check mark
                }
            }
        })
        .build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Release builds abort on panic with no console (Windows), so leave the reason in a file.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = fs::write(std::env::temp_dir().join("lorekeeper-crash.log"), format!("Lorekeeper {}: {info}\n", env!("CARGO_PKG_VERSION")));
        default_hook(info);
    }));
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec![LOGIN_ARG])))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            save_note,
            last_note,
            fix_note,
            dismiss,
            read_vault,
            page_names,
            save_file,
            create_file,
            save_image,
            read_image,
            thumbnail,
            save_thumbnail,
            delete_file,
            rename_file,
            start_session,
            session_status,
            campaign_pcs,
            open_in_obsidian,
            open_url,
            get_settings,
            save_settings,
            open_settings,
            pick_folder,
            open_vault_folder,
            switch_campaign,
            backup_now,
            backup_status,
            github_sign_in_start,
            github_sign_in_wait,
            github_sign_in_cancel,
            github_sign_out,
            cloud_sign_in,
            cloud_sign_in_cancel,
            cloud_sign_out,
            dndbeyond::dndbeyond_sign_in,
            dndbeyond::dndbeyond_sign_out,
            dndbeyond::dndbeyond_status,
            dndbeyond::dndbeyond_lookup,
            dndbeyond::dndbeyond_character,
            dndbeyond::dndbeyond_portrait,
            restore::restore_list,
            restore::restore_target,
            restore::restore_start,
            restore::restore_open,
            restore::campaign_places,
            restore::open_backup,
            updater::check_for_updates
        ])
        .setup(|app| {
            // Menu-bar app: no Dock icon.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let handle = app.handle();
            let vault = default_vault(handle);
            let settings = load_settings(&settings_file(handle)?, &vault).unwrap_or_else(|e| {
                notify(handle, "Using default settings", &format!("{e}. Fix the file, or change a setting to replace it."));
                let vault = vault.to_string_lossy().into_owned();
                Settings { vault_path: vault.clone(), campaigns: vec![vault], ..Settings::default() }
            });
            // A folder on an unplugged drive shouldn't stop the app from starting.
            if let Err(e) = prepare_campaign(handle, Path::new(&settings.vault_path)) {
                notify(handle, "Notes folder unavailable", &format!("{}: {e}", settings.vault_path));
            }
            app.manage(Mutex::new(settings.clone()));
            allow_vault_images(handle, &settings.vault_path);
            build_tray(handle)?;
            // Windows are created here ("create": false in tauri.conf.json), after the state their
            // commands read. On Windows a page can call a command while Tauri is still building windows.
            for config in &handle.config().app.windows {
                tauri::WebviewWindowBuilder::from_config(handle, config)?.build()?;
            }
            refresh_login_item(handle);
            backup::start(handle.clone());
            updater::start(handle.clone());
            watch::start(handle.clone());
            for keys in register_shortcuts(handle, &settings) {
                notify(handle, "Shortcut unavailable", &format!("{keys} couldn't be registered. Change it in Settings."));
            }
            // Opened by hand: show the notes. Started at login: stay quietly in the menu bar.
            if !std::env::args().any(|a| a == LOGIN_ARG) {
                show_window(app.handle(), "main");
            }
            Ok(())
        })
        // Closing a window only hides it; the app keeps running in the tray. The D&D Beyond sign-in window really closes.
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } if window.label() != dndbeyond::LABEL => {
                api.prevent_close();
                let _ = window.hide();
                #[cfg(target_os = "macos")]
                {
                    if !app_window_visible(window.app_handle(), window.label()) {
                        let _ = window.app_handle().set_activation_policy(tauri::ActivationPolicy::Accessory);
                    }
                    // Opened from the tray menu and closed without using another Lorekeeper window: back to where you were.
                    let front = MENU_FRONT.lock().unwrap().take();
                    if let Some((_, front)) = front.filter(|(label, _)| *label == window.label()) {
                        activate(front);
                    }
                }
            }
            // Moving to another Lorekeeper window means you're working in Lorekeeper now (the quick box aside).
            #[cfg(target_os = "macos")]
            WindowEvent::Focused(true) if window.label() != "capture" => {
                let mut menu_front = MENU_FRONT.lock().unwrap();
                if menu_front.is_some_and(|(label, _)| label != window.label()) {
                    *menu_front = None;
                }
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| match event {
            RunEvent::ExitRequested { api, code: None, .. } => api.prevent_exit(),
            // Clicking the app in the Dock, Finder or Spotlight while it's running.
            #[cfg(target_os = "macos")]
            RunEvent::Reopen { .. } => show_window(_app, "main"),
            _ => {}
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_and_notes() {
        let dir = std::env::temp_dir().join(format!("dnd-notes-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        create_vault_folders(&dir, &dir).unwrap();
        assert!(dir.join("NPCs").is_dir());
        assert!(fs::read_to_string(dir.join("Templates/NPC.md")).unwrap().contains("{{title}}"));
        fs::remove_file(dir.join("Templates/NPC.md")).unwrap();
        create_vault_folders(&dir, &dir).unwrap();
        assert!(!dir.join("Templates/NPC.md").exists(), "deleted template must stay deleted");

        // First note creates Session 1 (with properties); multi-line text becomes one line.
        assert_eq!(append_note(&dir, None, "  @Mirela\n the  innkeeper ").unwrap(), "@Mirela the innkeeper");
        assert_eq!(append_note(&dir, None, " \n ").unwrap(), "");
        let s1 = fs::read_to_string(dir.join("Sessions/Session 1.md")).unwrap();
        assert!(s1.starts_with("---\nsession: 1\ndate: "));
        assert!(s1.contains("\n# Session 1 - "));
        assert_eq!(s1.lines().filter(|l| l.starts_with("- ")).count(), 1);
        assert!(s1.trim_end().ends_with(" @Mirela the innkeeper"));

        // New session: notes go to the newest file; other files are ignored. Session 10 sorts after 9.
        fs::write(dir.join("Sessions/Ideas.md"), "x").unwrap();
        new_session(&dir, None).unwrap();
        append_note(&dir, None, "#potion").unwrap();
        assert!(fs::read_to_string(session_path(&dir, 2)).unwrap().contains("#potion"));
        assert!(!fs::read_to_string(session_path(&dir, 1)).unwrap().contains("#potion"));
        fs::write(session_path(&dir, 9), "").unwrap();
        fs::write(session_path(&dir, 10), "").unwrap();
        assert_eq!(latest_session(&dir).unwrap(), 10);

        // The vault walk lists folders and notes with / paths, skipping hidden entries.
        fs::create_dir_all(dir.join(".obsidian")).unwrap();
        fs::write(dir.join(".obsidian/app.md"), "").unwrap();
        fs::create_dir_all(dir.join("NPCs/Villains")).unwrap();
        fs::write(dir.join("NPCs/Villains/Vex.md"), "# Vex").unwrap();
        let mut vault = Vault::default();
        walk(&dir, &dir, &mut vault).unwrap();
        assert!(vault.folders.contains(&"NPCs/Villains".to_string()));
        assert!(vault.notes.iter().any(|n| n.path == "NPCs/Villains/Vex.md" && n.content == "# Vex"));
        assert!(!vault.folders.iter().chain(vault.notes.iter().map(|n| &n.path)).any(|p| p.contains(".obsidian")));
        fs::remove_dir_all(&dir).unwrap();
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dnd-notes-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn last_note_is_found_and_fixed_in_place() {
        // Only "- HH:MM text" lines are notes; prose after the last one is skipped, and the rest is kept byte for byte.
        let s = "# Session 1\r\n- 20:01 @Mirela ïnn\r\n- 1:5 x\n- 20:05 #potion\r\nThe party rests.\n- abcde y\n";
        assert_eq!(last_note_line(s).map(|(_, time, text)| (time, text)), Some(("20:05", "#potion")));
        assert_eq!(
            replace_last_note(s, "#potion", "#potion of healing").unwrap(),
            "# Session 1\r\n- 20:01 @Mirela ïnn\r\n- 1:5 x\n- 20:05 #potion of healing\r\nThe party rests.\n- abcde y\n"
        );
        assert_eq!(replace_last_note(s, "#poison", "x"), None);
        assert_eq!(replace_last_note("# Session 1\n\n", "", "x"), None);
        assert_eq!(last_note_line("- 20:01 ünïcode").unwrap().2, "ünïcode");

        // In a vault: fixed in place keeping its time; empty text keeps it; a newer note means the fix is added as new.
        let dir = temp_dir("fix-last");
        fs::create_dir_all(dir.join("Sessions")).unwrap();
        fs::write(session_path(&dir, 1), "# Session 1\n\n- 19:58 Mirela the innkeper\n").unwrap();
        assert!(fix_last_note(&dir, None, "Mirela the innkeper", "  Mirela the\n innkeeper ").unwrap());
        assert_eq!(fs::read_to_string(session_path(&dir, 1)).unwrap(), "# Session 1\n\n- 19:58 Mirela the innkeeper\n");
        assert!(fix_last_note(&dir, None, "Mirela the innkeeper", "  ").unwrap());
        assert_eq!(fs::read_to_string(session_path(&dir, 1)).unwrap(), "# Session 1\n\n- 19:58 Mirela the innkeeper\n");
        append_note(&dir, None, "#potion").unwrap();
        assert!(!fix_last_note(&dir, None, "Mirela the innkeeper", "Mirela the elf").unwrap());
        let notes: Vec<String> = fs::read_to_string(session_path(&dir, 1)).unwrap().lines().filter_map(|l| Some(note_line(l)?.1.to_owned())).collect();
        assert_eq!(notes, ["Mirela the innkeeper", "#potion", "Mirela the elf"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stale_session_prompt() {
        let t = |h: u64, m: u64| std::time::UNIX_EPOCH + Duration::from_secs(h * 3600 + m * 60);
        assert_eq!(stale_hours(t(0, 0), t(11, 59)), None);
        assert_eq!(stale_hours(t(0, 0), t(12, 0)), Some(12));
        assert_eq!(stale_hours(t(0, 0), t(72, 30)), Some(72));
        assert_eq!(stale_hours(t(5, 0), t(0, 0)), None, "a file from the future isn't stale");

        // A fresh vault gets no prompt and no session file; an old session offers the next one.
        let dir = temp_dir("stale");
        let far = std::time::SystemTime::now() + Duration::from_secs(1000 * 3600);
        assert!(stale_session(&dir, None, far).unwrap().is_none());
        assert_eq!(latest_session(&dir).unwrap(), 0);
        append_note(&dir, None, "the party rests").unwrap();
        assert!(stale_session(&dir, None, std::time::SystemTime::now()).unwrap().is_none());
        let later = std::time::SystemTime::now() + Duration::from_secs(13 * 3600 + 60);
        let s = stale_session(&dir, None, later).unwrap().unwrap();
        assert_eq!((s.next, s.idle_hours), (2, 13));
        new_session(&dir, None).unwrap();
        assert!(stale_session(&dir, None, far).unwrap().is_none(), "a session without notes isn't stale");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn shared_sessions_write_one_file_per_player() {
        let dir = temp_dir("shared");
        let me = Some("Sibling 5");
        // Numbering reads Session N.md files and Session N/ folders alike, and the newest wins whichever form it has.
        fs::create_dir_all(session_folder(&dir, 3)).unwrap();
        fs::create_dir_all(dir.join("Sessions/Session 4 (1)")).unwrap(); // a sync conflict's copy isn't a session
        fs::write(dir.join("Sessions/Session 9"), "").unwrap(); // nor is a file without .md
        fs::write(session_path(&dir, 2), "").unwrap();
        assert_eq!(latest_session(&dir).unwrap(), 3);
        fs::write(session_path(&dir, 4), "").unwrap();
        assert_eq!(latest_session(&dir).unwrap(), 4);
        fs::remove_file(session_path(&dir, 4)).unwrap();

        // Your notes go to your own file in the newest folder, made with its properties on the first one.
        assert_eq!(append_note(&dir, me, " the  party\n rests ").unwrap(), "the party rests");
        let mine = session_folder(&dir, 3).join("Sibling 5.md");
        let text = fs::read_to_string(&mine).unwrap();
        assert!(text.starts_with("---\nsession: 3\ndate: ") && text.contains("\nauthor: \"[[Sibling 5]]\"\n---\n# Session 3 - "), "{text}");
        assert!(text.ends_with(" the party rests\n"), "{text}");
        assert_eq!(text.lines().filter(|l| l.starts_with("- ")).count(), 1);
        assert_eq!((session_name(&mine), session_name(&session_path(&dir, 2))), ("Session 3".into(), "Session 2".into()));

        // ↑ fixes your own last note, even with a teammate's newer one; their file is never touched.
        let theirs = session_folder(&dir, 3).join("Mirela.md");
        fs::write(&theirs, "# Session 3\n- 23:59 the party rests\n").unwrap();
        assert!(fix_last_note(&dir, me, "the party rests", "the party sleeps").unwrap());
        assert!(fs::read_to_string(&mine).unwrap().ends_with(" the party sleeps\n"));
        assert_eq!(fs::read_to_string(&theirs).unwrap(), "# Session 3\n- 23:59 the party rests\n");

        // The stale prompt looks at your own file only.
        let later = std::time::SystemTime::now() + Duration::from_secs(13 * 3600);
        assert_eq!(stale_session(&dir, me, later).unwrap().map(|s| s.next), Some(4));
        assert!(stale_session(&dir, Some("Vex"), later).unwrap().is_none(), "no notes of yours in it yet");

        // New session makes the next folder with your file. A teammate who hasn't written in it yet joins it, rather than
        // starting another; one who has starts the next. A session quiet for 12 hours is over, so nobody joins that.
        assert_eq!(new_session(&dir, me).unwrap(), session_folder(&dir, 4).join("Sibling 5.md"));
        assert_eq!(new_session(&dir, Some("Mirela")).unwrap(), session_folder(&dir, 4).join("Mirela.md"));
        assert_eq!(new_session(&dir, Some("Mirela")).unwrap(), session_folder(&dir, 5).join("Mirela.md"));
        let old = std::time::SystemTime::now() - Duration::from_secs(13 * 3600);
        fs::File::options().write(true).open(session_folder(&dir, 5).join("Mirela.md")).unwrap().set_modified(old).unwrap();
        assert_eq!(new_session(&dir, Some("Vex")).unwrap(), session_folder(&dir, 6).join("Vex.md"));
        assert!(fs::read_to_string(session_folder(&dir, 6).join("Vex.md")).unwrap().contains("session: 6\n"));
        // Open in Obsidian never makes a file: yours, else a teammate's.
        assert_eq!(latest_file(&dir, Some("Vex")), Some(session_folder(&dir, 6).join("Vex.md")));
        assert_eq!(latest_file(&dir, me), Some(session_folder(&dir, 6).join("Vex.md")));
        assert!(!session_folder(&dir, 6).join("Sibling 5.md").exists());

        // Shared never appends to a Session N.md from before sharing: the next session starts as a folder.
        let solo = temp_dir("shared-from-solo");
        append_note(&solo, None, "before").unwrap();
        append_note(&solo, me, "after").unwrap();
        assert!(!fs::read_to_string(session_path(&solo, 1)).unwrap().contains("after"));
        assert!(fs::read_to_string(session_folder(&solo, 2).join("Sibling 5.md")).unwrap().ends_with(" after\n"));
        for d in [dir, solo] {
            fs::remove_dir_all(d).unwrap();
        }
    }

    #[test]
    fn shared_campaigns_need_a_character() {
        let sharing = |shared: bool, me: &str| [("/v".to_string(), Sharing { shared, me: me.into() })].into();
        let s = |sharing| Settings { vault_path: "/v".into(), sharing, ..Settings::default() };
        assert_eq!(author(&s(sharing(true, ""))), Err(PICK_PC.to_string()));
        assert_eq!(author(&s(sharing(true, "PCs/Sibling 5.md"))), Ok(Some("Sibling 5".into())));
        assert_eq!(author(&s(sharing(false, ""))), Ok(None), "solo: as before");
        assert_eq!(author(&s(BTreeMap::new())), Ok(None));
        let other = Settings { vault_path: "/w".into(), ..s(sharing(true, "")) };
        assert_eq!(author(&other), Ok(None), "only the open campaign counts");

        // The PC pages of any campaign, open or not, for picking yours.
        let dir = temp_dir("pcs");
        assert!(pc_pages(&dir).is_empty());
        fs::create_dir_all(dir.join("PCs/Retired")).unwrap();
        fs::write(dir.join("PCs/Sibling 5.md"), "").unwrap();
        fs::write(dir.join("PCs/Retired/arn.md"), "").unwrap();
        fs::write(dir.join("PCs/portrait.png"), "").unwrap();
        assert_eq!(pc_pages(&dir), ["PCs/Retired/arn.md", "PCs/Sibling 5.md"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn outside_changes_are_noticed_but_the_apps_own_are_not() {
        use watch::{changed_by_others, stamps};
        let dir = temp_dir("watch");
        fs::create_dir_all(dir.join("Sessions/Session 1")).unwrap();
        fs::create_dir_all(dir.join(".obsidian")).unwrap();
        let (mine, theirs) = (dir.join("Sessions/Session 1/Sibling 5.md"), dir.join("Sessions/Session 1/Mirela.md"));
        fs::write(&mine, "a").unwrap();
        let before = stamps(&dir);
        assert!(!changed_by_others(&before, &stamps(&dir), &[]));
        fs::write(dir.join(".obsidian/workspace.json"), "{}").unwrap();
        assert!(!changed_by_others(&before, &stamps(&dir), &[]), "hidden files don't count");
        fs::write(&mine, "ab").unwrap();
        assert!(!changed_by_others(&before, &stamps(&dir), std::slice::from_ref(&mine)));
        assert!(changed_by_others(&before, &stamps(&dir), &[]));
        fs::write(&theirs, "x").unwrap();
        assert!(changed_by_others(&before, &stamps(&dir), std::slice::from_ref(&mine)), "a teammate's new file");
        let now = stamps(&dir);
        fs::remove_file(&theirs).unwrap();
        assert!(changed_by_others(&now, &stamps(&dir), &[]), "a file removed");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn campaigns_in_the_lorekeeper_folder_share_its_templates() {
        let library = temp_dir("library");
        let (campaign, outside) = (library.join("Strahd"), temp_dir("outside"));
        // A campaign made before templates were shared: its own templates move up, once.
        fs::create_dir_all(campaign.join("Templates")).unwrap();
        fs::write(campaign.join("Templates/NPC.md"), "# my NPC").unwrap();
        fs::write(campaign.join("Templates/.seeded"), "PC\nNPC\n").unwrap();
        share_templates(&campaign, &library).unwrap();
        assert!(!campaign.join("Templates").exists());
        assert_eq!(fs::read_to_string(library.join("Templates/NPC.md")).unwrap(), "# my NPC");
        assert_eq!(templates_home(&campaign, &library), library);
        create_vault_folders(&campaign, &library).unwrap();
        assert!(campaign.join("NPCs").is_dir() && !campaign.join("Templates").exists(), "no Templates/ of its own any more");
        assert_eq!(fs::read_to_string(library.join("Templates/NPC.md")).unwrap(), "# my NPC", "never over your own");
        assert!(library.join("Templates/Lore.md").exists(), "new defaults go to the shared folder");

        // A second old campaign: a template with other text stays put, the same text goes, the record merges.
        let other = library.join("Side");
        fs::create_dir_all(other.join("Templates")).unwrap();
        fs::write(other.join("Templates/NPC.md"), "# other NPC").unwrap();
        fs::write(other.join("Templates/Lore.md"), fs::read(library.join("Templates/Lore.md")).unwrap()).unwrap();
        fs::write(other.join("Templates/.seeded"), "Weird\n").unwrap();
        share_templates(&other, &library).unwrap();
        assert_eq!(fs::read_to_string(other.join("Templates/NPC.md")).unwrap(), "# other NPC");
        assert!(!other.join("Templates/Lore.md").exists());
        assert!(fs::read_to_string(library.join("Templates/.seeded")).unwrap().ends_with("Weird\n"));

        // The shared templates show as the campaign's Templates/; a campaign elsewhere with its own keeps them.
        let mut vault = Vault::default();
        add_shared_templates(&campaign, &library, &mut vault);
        assert!(vault.notes.iter().any(|n| n.path == "Templates/NPC.md" && n.content == "# my NPC"));
        fs::create_dir_all(outside.join("Templates")).unwrap();
        assert_eq!(templates_home(&outside, &library), outside);
        share_templates(&outside, &library).unwrap();
        assert!(outside.join("Templates").is_dir(), "a campaign outside the Lorekeeper folder keeps its own");
        assert_eq!(templates_home(&library, &library), library);
        fs::remove_dir_all(&library).unwrap();
        fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn default_templates_are_written_once_ever() {
        let names = |dir: &Path| {
            let mut v: Vec<String> = fs::read_dir(dir.join("Templates")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
            v.sort();
            v
        };
        // A fresh vault gets all seven, and Quests/ and Lore/.
        let fresh = temp_dir("seed-fresh");
        create_vault_folders(&fresh, &fresh).unwrap();
        assert_eq!(names(&fresh), [".seeded", "Faction.md", "Item.md", "Location.md", "Lore.md", "NPC.md", "PC.md", "Quest.md"]);
        assert!(fresh.join("Quests").is_dir());
        assert!(fresh.join("Lore").is_dir());
        assert!(fs::read_to_string(fresh.join("Templates/Lore.md")).unwrap().starts_with("---\ntype: lore\n"));
        assert!(fs::read_to_string(fresh.join("Templates/Quest.md")).unwrap().starts_with("---\ntype: quest\nstatus: open\n"));

        // A deleted template is not recreated.
        fs::remove_file(fresh.join("Templates/Quest.md")).unwrap();
        create_vault_folders(&fresh, &fresh).unwrap();
        assert!(!fresh.join("Templates/Quest.md").exists(), "deleted template must stay deleted");

        // A vault from before the record: the original five count as seeded, so only Quest and Lore are added.
        // One of them deleted and one edited earlier stay that way; a Quest.md of your own is kept too.
        let old = temp_dir("seed-old");
        fs::create_dir_all(old.join("Templates")).unwrap();
        for name in ["NPC", "Location", "Item", "Faction"] {
            fs::write(old.join(format!("Templates/{name}.md")), format!("# my {name}")).unwrap();
        }
        create_vault_folders(&old, &old).unwrap();
        assert_eq!(names(&old), [".seeded", "Faction.md", "Item.md", "Location.md", "Lore.md", "NPC.md", "Quest.md"]);
        assert_eq!(fs::read_to_string(old.join("Templates/NPC.md")).unwrap(), "# my NPC");
        assert_eq!(fs::read_to_string(old.join("Templates/.seeded")).unwrap(), "PC\nNPC\nLocation\nItem\nFaction\nQuest\nLore\n");
        let mine = temp_dir("seed-mine");
        fs::create_dir_all(mine.join("Templates")).unwrap();
        fs::write(mine.join("Templates/Quest.md"), "# my quest").unwrap();
        create_vault_folders(&mine, &mine).unwrap();
        assert_eq!(fs::read_to_string(mine.join("Templates/Quest.md")).unwrap(), "# my quest");
        for dir in [fresh, old, mine] {
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn settings_load_defaults_partial_files_and_migrate() {
        let dir = temp_dir("settings");
        let (config, vault) = (dir.join("config/settings.json"), dir.join("Lorekeeper"));
        let vault_path = vault.to_string_lossy().into_owned();

        // First run: defaults, pointing at the default vault, written to the config folder.
        let s = load_settings(&config, &vault).unwrap();
        let one = Settings { vault_path: vault_path.clone(), campaigns: vec![vault_path.clone()], ..Settings::default() };
        assert_eq!(s, one);
        assert_eq!((s.theme.as_str(), s.editor_font_size, s.session_view.as_str(), s.notifications), ("system", 15, "timeline", true));
        let written = fs::read_to_string(&config).unwrap();
        assert!(written.contains("\"editorFontSize\": 15") && !written.contains("launchAtLogin"));

        // A partial file keeps what it has and fills in the rest.
        fs::write(&config, r#"{"theme":"dark","editorFontSize":18}"#).unwrap();
        let s = load_settings(&config, &vault).unwrap();
        assert_eq!((s.theme.as_str(), s.editor_font_size, s.quick_note.as_str()), ("dark", 18, "CmdOrCtrl+Alt+N"));
        assert!(s.auto_update, "files from before automatic updates turn them on");
        assert_eq!(s.vault_path, vault_path);

        // A file from before campaigns: its notes folder becomes the only campaign.
        let old_file = r#"{"vaultPath":"/old/Lore","dropboxUser":"me@x.com"}"#;
        fs::write(&config, old_file).unwrap();
        let s = load_settings(&config, &vault).unwrap();
        assert_eq!((s.campaigns, s.dropbox_user.as_str()), (vec!["/old/Lore".to_string()], "me@x.com"));
        assert_eq!(fs::read_to_string(&config).unwrap(), old_file, "loading never rewrites the file");
        fs::write(&config, r#"{"vaultPath":"/b/Side","campaigns":["/old/Lore","/b/Side"],"mainCampaign":"/old/Lore"}"#).unwrap();
        let s = load_settings(&config, &vault).unwrap();
        assert_eq!((s.campaigns.len(), s.vault_path.as_str()), (2, "/b/Side"));
        assert!(s.sharing.is_empty(), "files from before sharing: every campaign is yours alone");
        fs::write(&config, r#"{"sharing":{"/b/Side":{"shared":true,"me":"PCs/Arn.md"},"/old/Lore":{"shared":true}}}"#).unwrap();
        let s = load_settings(&config, &vault).unwrap();
        assert_eq!(s.sharing["/b/Side"], Sharing { shared: true, me: "PCs/Arn.md".into() });
        assert_eq!(s.sharing["/old/Lore"].me, "");

        // Tome and Dungeon from 0.3.0 become Light and Dark; anything else is left for validate to judge.
        for (old, new) in [("tome", "light"), ("dungeon", "dark"), ("purple", "purple")] {
            fs::write(&config, format!(r#"{{"theme":"{old}"}}"#)).unwrap();
            assert_eq!(load_settings(&config, &vault).unwrap().theme, new);
        }

        // A broken file is reported, not overwritten.
        fs::write(&config, "{oops").unwrap();
        assert!(load_settings(&config, &vault).is_err());
        assert_eq!(fs::read_to_string(&config).unwrap(), "{oops");

        // Old <vault>/settings.json: moved to the config folder, keeping its hotkeys.
        fs::remove_file(&config).unwrap();
        fs::create_dir_all(&vault).unwrap();
        fs::write(vault.join("settings.json"), r#"{"quickNote":"CmdOrCtrl+Alt+Q","capture":"CmdOrCtrl+Shift+K"}"#).unwrap();
        let s = load_settings(&config, &vault).unwrap();
        assert_eq!((s.quick_note.as_str(), s.capture.as_str(), s.vault_path.as_str()), ("CmdOrCtrl+Alt+Q", "CmdOrCtrl+Shift+K", vault_path.as_str()));
        assert!(!vault.join("settings.json").exists());
        assert_eq!(load_settings(&config, &vault).unwrap(), s);

        // A broken old file is reported and left where it is; nothing is written.
        fs::remove_file(&config).unwrap();
        fs::write(vault.join("settings.json"), "{oops").unwrap();
        assert!(load_settings(&config, &vault).is_err());
        assert!(!config.exists());
        assert_eq!(fs::read_to_string(vault.join("settings.json")).unwrap(), "{oops");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn settings_validation() {
        let tmp = std::env::temp_dir().to_string_lossy().into_owned();
        let ok = Settings { vault_path: tmp.clone(), campaigns: vec![tmp.clone()], ..Settings::default() };
        assert_eq!(validate(ok.clone()).unwrap(), ok);
        assert_eq!(validate(Settings { editor_font_size: 99, ..ok.clone() }).unwrap().editor_font_size, 24);
        assert_eq!(validate(Settings { editor_font_size: 2, ..ok.clone() }).unwrap().editor_font_size, 11);
        // Optional shortcuts: off by default, any distinct valid combination when set.
        let extra = Settings { new_session: "Ctrl+Alt+CmdOrCtrl+S".into(), new_page: "Ctrl+Alt+CmdOrCtrl+P".into(), ..ok.clone() };
        assert_eq!(validate(extra.clone()).unwrap(), extra);
        for theme in ["light", "dark"] {
            assert_eq!(validate(Settings { theme: theme.into(), ..ok.clone() }).unwrap().theme, theme);
        }
        let bad = [
            Settings { theme: "purple".into(), ..ok.clone() },
            Settings { theme: "tome".into(), ..ok.clone() }, // migrated by load_settings, never sent by the window
            Settings { session_view: "grid".into(), ..ok.clone() },
            Settings { vault_path: "notes".into(), ..ok.clone() },
            Settings { quick_note: "CmdOrCtrl+Nope".into(), ..ok.clone() },
            Settings { capture: "Alt+Shift+S".into(), ..ok.clone() },
            Settings { capture: ok.quick_note.clone(), ..ok.clone() },
            Settings { backup_folder: "Backups".into(), ..ok.clone() },
            Settings { backup_folder: format!("{}/Backups", ok.vault_path), ..ok.clone() },
            Settings { github_repo: "my notes".into(), ..ok.clone() },
            Settings { github_repo: "".into(), ..ok.clone() },
            Settings { new_session: "CmdOrCtrl+Nope".into(), ..ok.clone() },
            Settings { new_session: ok.capture.clone(), ..ok.clone() },
            Settings { new_session: "Ctrl+Alt+CmdOrCtrl+S".into(), new_page: "Ctrl+Alt+CmdOrCtrl+S".into(), ..ok.clone() },
            // Campaigns: the active one can't be removed, paths are full, and names (and slugs) differ.
            Settings { campaigns: vec!["/x/Other".into()], ..ok.clone() },
            Settings { campaigns: vec![tmp.clone(), "Side".into()], ..ok.clone() },
            Settings { campaigns: vec![tmp.clone(), "/x/Strahd".into(), "/y/strahd".into()], ..ok.clone() },
            Settings { campaigns: vec![tmp.clone(), "/x/My Strahd".into(), "/y/my-strahd".into()], ..ok.clone() },
            Settings { campaigns: vec![tmp.clone(), "/x/Lorekeeper backup 2026-03-07".into()], ..ok.clone() },
            Settings { campaigns: vec![tmp.clone(), "/x/Side".into()], backup_folder: "/x/Side/Backups".into(), ..ok.clone() },
            Settings { campaigns: vec![tmp.clone(), "/".into()], ..ok.clone() },
            // Backup names: they differ from every other campaign's (typed or from its folder), and work as folder names.
            Settings { campaigns: vec![tmp.clone(), "/x/Side".into(), "/x/Strahd".into()], backup_names: [("/x/Side".into(), "strahd".into())].into(), ..ok.clone() },
            Settings { campaigns: vec![tmp.clone(), "/x/Side".into(), "/x/Other".into()], backup_names: [("/x/Side".into(), "Same!".into()), ("/x/Other".into(), "same".into())].into(), ..ok.clone() },
            Settings { campaigns: vec![tmp.clone(), "/x/Side".into()], backup_names: [("/x/Side".into(), "Lorekeeper backup 2026-03-07".into())].into(), ..ok.clone() },
            Settings { campaigns: vec![tmp.clone(), "/x/Side".into()], backup_names: [("/x/Side".into(), "a/b".into())].into(), ..ok.clone() },
            Settings { campaigns: vec![tmp.clone(), "/x/Side".into()], backup_names: [("/x/Side".into(), "Who?".into())].into(), ..ok.clone() },
            Settings { campaigns: vec![tmp.clone(), "/x/Side".into()], backup_names: [("/x/Side".into(), "Side.".into())].into(), ..ok.clone() },
            Settings { campaigns: vec![tmp.clone(), "/x/Side".into()], backup_names: [("/x/Side".into(), "!!!".into())].into(), ..ok.clone() },
            Settings { campaigns: vec![tmp.clone(), "/x/Side".into()], backup_names: [(tmp.clone(), "side".into())].into(), ..ok.clone() },
        ];
        let strahd = std::env::temp_dir().join("Curse of Strahd").to_string_lossy().into_owned();
        let two = Settings { campaigns: vec![tmp.clone(), strahd], ..ok.clone() };
        assert_eq!(validate(two.clone()).unwrap(), two);
        // Every campaign's name names its backups, the first one's too; a backup name fixes an odd one.
        let restored = std::env::temp_dir().join("Lorekeeper backup 2026-03-07").to_string_lossy().into_owned();
        let odd = Settings { vault_path: restored.clone(), campaigns: vec![restored.clone()], ..ok.clone() };
        assert!(validate(odd.clone()).unwrap_err().contains("Give it a name"));
        let fixed_odd = Settings { backup_names: [(restored, "Old".to_string())].into(), ..odd };
        assert_eq!(validate(fixed_odd.clone()).unwrap(), fixed_odd);
        // A backup name fixes a folder name that can't name backups, and is saved trimmed; an empty one is dropped.
        let side = "/x/Side".to_string();
        let names = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<BTreeMap<_, _>>();
        let fixed = Settings { campaigns: vec![tmp.clone(), "/x/Lorekeeper backup 2026-03-07".into()], backup_names: names(&[("/x/Lorekeeper backup 2026-03-07", "Old")]), ..ok.clone() };
        assert_eq!(validate(fixed.clone()).unwrap(), fixed);
        let typed = Settings { campaigns: vec![tmp.clone(), side.clone()], backup_names: names(&[(&side, "  Curse of Strahd "), (&tmp, " ")]), ..ok.clone() };
        assert_eq!(validate(typed).unwrap().backup_names, names(&[(&side, "Curse of Strahd")]));
        // Two folders can trade names; a removed campaign's name is kept for when it's added again.
        let swapped = Settings { campaigns: vec![tmp.clone(), side.clone(), "/x/Strahd".into()], backup_names: names(&[(&side, "Strahd"), ("/x/Strahd", "Side")]), ..ok.clone() };
        assert_eq!(validate(swapped.clone()).unwrap(), swapped);
        let kept = Settings { backup_names: names(&[("/gone/Side", "Side")]), ..ok.clone() };
        assert_eq!(validate(kept.clone()).unwrap(), kept);
        // The PC you play in a shared campaign is a page in its PCs/ folder, or none yet.
        let playing = |me: &str| Settings { sharing: [(tmp.clone(), Sharing { shared: true, me: me.into() })].into(), ..ok.clone() };
        for me in ["", "PCs/Sibling 5.md", "PCs/Retired/Arn.md"] {
            assert_eq!(validate(playing(me)).unwrap(), playing(me));
        }
        for me in ["NPCs/Vex.md", "PCs/../secret.md", "PCs/Arn.txt", "/PCs/Arn.md", "PCs"] {
            assert!(validate(playing(me)).is_err(), "{me} should be refused");
        }
        for s in bad {
            assert!(validate(s.clone()).is_err(), "{s:?} should be refused");
        }
    }

    #[test]
    fn webview_paths_stay_in_the_vault() {
        let root = Path::new("/vault");
        assert_eq!(vault_file(root, "NPCs/Mirela.md").unwrap(), root.join("NPCs/Mirela.md"));
        for bad in ["../secret.md", "NPCs/../../x.md", "/etc/passwd.md", "NPCs/Mirela.txt", "", "NPCs/"] {
            assert!(vault_file(root, bad).is_err(), "{bad} should be refused");
        }
    }

    #[test]
    fn rename_never_overwrites_but_changes_case() {
        let dir = temp_dir("rename");
        assert!(rename_note(&dir, "Sessions/Session 3.md", "Sessions/Ambush.md").is_err());
        assert!(rename_note(&dir, "Notes.md", "Sessions/Session 9.md").is_err());
        fs::create_dir_all(dir.join("NPCs")).unwrap();
        fs::write(dir.join("NPCs/mirela.md"), "m").unwrap();
        fs::write(dir.join("NPCs/Vex.md"), "v").unwrap();
        let names = || {
            let mut v: Vec<String> = fs::read_dir(dir.join("NPCs")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
            v.sort();
            v
        };
        assert!(rename_note(&dir, "NPCs/mirela.md", "NPCs/Vex.md").unwrap_err().contains("already exists"));
        if dir.join("NPCs/VEX.md").exists() {
            // A case-insensitive disk: another page with the name in other case is still another page.
            assert!(rename_note(&dir, "NPCs/mirela.md", "NPCs/vex.md").unwrap_err().contains("already exists"));
        }
        assert!(rename_note(&dir, "NPCs/Nobody.md", "NPCs/X.md").unwrap_err().contains("doesn't exist"));
        for (from, to) in [("../mirela.md", "NPCs/X.md"), ("NPCs/mirela.md", "../X.md"), ("NPCs/mirela.md", "NPCs/X.txt")] {
            assert!(rename_note(&dir, from, to).is_err(), "{from} -> {to} should be refused");
        }
        assert_eq!(fs::read_to_string(dir.join("NPCs/Vex.md")).unwrap(), "v");

        // Case-only works on case-insensitive disks (macOS, Windows) and sensitive ones alike.
        rename_note(&dir, "NPCs/mirela.md", "NPCs/Mirela.md").unwrap();
        assert!(names().contains(&"Mirela.md".to_string()) && !names().contains(&"mirela.md".to_string()));
        rename_note(&dir, "NPCs/Mirela.md", "NPCs/Mira.md").unwrap();
        assert_eq!(fs::read_to_string(dir.join("NPCs/Mira.md")).unwrap(), "m");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_changed_image_gets_a_new_thumbnail() {
        let (root, at) = (Path::new("/vault"), Some(std::time::UNIX_EPOCH + Duration::from_secs(1_760_000_000)));
        let name = thumbnail_name(root, "Attachments/Mirela.jpg", 100, at);
        assert_eq!(name, thumbnail_name(root, "Attachments/Mirela.jpg", 100, at));
        assert!(name.ends_with(".png"));
        for other in [
            thumbnail_name(root, "Attachments/Mirela.jpg", 101, at),
            thumbnail_name(root, "Attachments/Mirela.jpg", 100, at.map(|t| t + Duration::from_nanos(1))),
            thumbnail_name(root, "Attachments/Demus.jpg", 100, at),
            thumbnail_name(Path::new("/other"), "Attachments/Mirela.jpg", 100, at),
        ] {
            assert_ne!(name, other);
        }
    }

    #[test]
    fn image_paths_stay_in_the_vault() {
        let root = Path::new("/vault");
        for ok in ["Attachments/Pasted image 20261005143012.png", "map.JPG", "Maps/a.jpeg", "x.gif", "x.webp", "x.svg"] {
            assert_eq!(vault_image(root, ok).unwrap(), root.join(ok));
        }
        for bad in ["../a.png", "Attachments/../../a.png", "/a.png", "a.md", "a.exe", "a.png.exe", "Attachments/", "", "png"] {
            assert!(vault_image(root, bad).is_err(), "{bad} should be refused");
        }
        // Notes and images stay apart: neither check accepts the other's files.
        assert!(vault_file(root, "map.png").is_err());

        // The vault walk lists images next to notes, hidden folders still skipped.
        let dir = temp_dir("images");
        fs::create_dir_all(dir.join("Attachments")).unwrap();
        fs::create_dir_all(dir.join(".obsidian")).unwrap();
        for f in ["Attachments/map.png", "Handout.JPG", "notes.txt", ".obsidian/icon.png"] {
            fs::write(dir.join(f), "x").unwrap();
        }
        let mut vault = Vault::default();
        walk(&dir, &dir, &mut vault).unwrap();
        vault.images.sort();
        assert_eq!(vault.images, ["Attachments/map.png", "Handout.JPG"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn page_names_skip_templates_and_sessions() {
        let paths = ["NPCs/Villains/Vex.md", "Banana.md", "Locations/apple.md", "PCs/Vex.md", "Templates/NPC.md", "Sessions/Session 1.md"];
        assert_eq!(page_names_of(paths), ["apple", "Banana", "Vex"]);
    }

    #[test]
    fn saving_keeps_notes_appended_meanwhile() {
        let base = "# S1\n- 20:00 a\n";
        // Nothing changed on disk: plain save.
        assert_eq!(merge_save(base, base, "# S1\n- 20:00 A\n").unwrap(), "# S1\n- 20:00 A\n");
        // A hotkey note was appended while editing: keep it after the edit.
        let disk = "# S1\n- 20:00 a\n- 20:05 b\n";
        assert_eq!(merge_save(disk, base, "# S1\n- 20:00 A\n").unwrap(), "# S1\n- 20:00 A\n- 20:05 b\n");
        assert_eq!(merge_save(disk, base, "# S1\n- 20:00 A").unwrap(), "# S1\n- 20:00 A\n- 20:05 b\n");
        // Edited elsewhere (e.g. in Obsidian): refuse.
        assert_eq!(merge_save("# S1\n- 20:00 changed\n", base, "mine"), None);
    }

    #[test]
    fn git_blob_shas() {
        assert_eq!(github::git_blob_sha(b""), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
        assert_eq!(github::git_blob_sha(b"hello\n"), "ce013625030ba8dba906f756967f9e9ca394464a");
    }

    #[test]
    fn device_flow_poll_replies() {
        use github::{parse_poll, Poll};
        use serde_json::json;
        assert!(matches!(parse_poll(&json!({"error": "authorization_pending"})), Poll::Wait));
        assert!(matches!(parse_poll(&json!({"error": "slow_down", "interval": 10})), Poll::SlowDown));
        assert!(matches!(parse_poll(&json!({"access_token": "gho_abc", "token_type": "bearer", "scope": "repo"})), Poll::Token(t) if t == "gho_abc"));
        assert!(matches!(parse_poll(&json!({"error": "expired_token"})), Poll::Failed(e) if e.contains("expired")));
        assert!(matches!(parse_poll(&json!({"error": "access_denied"})), Poll::Failed(e) if e.contains("cancelled")));
        assert!(matches!(parse_poll(&json!({"error": "device_flow_disabled"})), Poll::Failed(e) if e.contains("isn't set up")));
        assert!(matches!(parse_poll(&json!(null)), Poll::Failed(_)));
    }

    #[test]
    fn pkce_challenge_matches_rfc_7636() {
        assert_eq!(cloud::pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let (a, b) = (cloud::random_token(), cloud::random_token());
        assert_ne!(a, b);
        assert_eq!(a.len(), 43, "32 bytes, inside PKCE's 43..128 characters");
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn loopback_redirects() {
        use cloud::parse_redirect;
        let req = |target: &str| format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:5000\r\n\r\n");
        assert_eq!(parse_redirect(&req("/?code=4%2F0Ab_c&state=s1&scope=x"), "s1"), Some(Ok("4/0Ab_c".into())));
        // Not the redirect: keep waiting.
        for other in [req("/favicon.ico"), req("/?state=s1"), "".into(), "POST /?code=a&state=s1 HTTP/1.1".into(), "GET http://evil/?code=a&state=s1 HTTP/1.1".into()] {
            assert_eq!(parse_redirect(&other, "s1"), None, "{other:?}");
        }
        // Wrong or missing state, even with a code.
        assert!(parse_redirect(&req("/?code=a&state=s2"), "s1").unwrap().unwrap_err().contains("security check"));
        assert!(parse_redirect(&req("/?code=a"), "s1").unwrap().is_err());
        assert!(parse_redirect(&req("/?error=access_denied&state=s1"), "s1").unwrap().unwrap_err().contains("cancelled"));
        let err = parse_redirect(&req("/?error=server_error&error_description=Try+later&state=s1"), "s1").unwrap().unwrap_err();
        assert_eq!(err, "Sign-in failed: Try later");
        assert!(parse_redirect(&req("/?code=&state=s1"), "s1").unwrap().is_err());
    }

    #[test]
    fn sign_in_page() {
        use base64::Engine as _;
        let html = cloud::page(false, "Sign-in didn't finish", r#"Sign-in failed: <script>alert("x")</script> & more"#);
        assert!(html.contains("Sign-in failed: &lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt; &amp; more"), "{html}");
        assert!(!html.contains("<script>"));
        assert!(html.contains("mark fail") && html.contains("You can close this tab and return to Lorekeeper."));
        assert!(cloud::page(true, "You're signed in", "").contains("mark ok"));
        // Fonts and icon inlined with the standard base64 alphabet (browsers don't accept URL-safe in data URIs).
        for kind in ["data:font/woff2;base64,", "data:image/png;base64,"] {
            let start = html.find(kind).expect(kind) + kind.len();
            let end = start + html[start..].find(['"', ')']).unwrap();
            assert!(base64::engine::general_purpose::STANDARD.decode(&html[start..end]).is_ok(), "{kind}");
        }
        assert!(html.len() < 150_000, "{} bytes", html.len());
    }

    #[test]
    fn cloud_manifest_diff() {
        use cloud::{diff, Local, Uploaded};
        let local = |rel: &str, hash: &str| Local { rel: rel.into(), path: PathBuf::from(rel), hash: hash.into() };
        let up = |hash: &str| Uploaded { hash: hash.into(), id: String::new() };
        let files = [local("Same.md", "a"), local("NPCs/Changed.md", "b2"), local("New.md", "c")];
        let done = [("Same.md", up("a")), ("NPCs/Changed.md", up("b1")), ("Gone.md", up("d")), ("Big.png", up("e"))].map(|(k, v)| (k.to_string(), v)).into();
        let plan = diff(&files, &["Big.png".to_string()], &done);
        let up: Vec<&str> = plan.upload.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(up, ["NPCs/Changed.md", "New.md"]);
        assert_eq!(plan.delete, ["Gone.md"], "a file too big to upload now still exists, so its old copy stays");
        // Nothing uploaded yet: everything goes up, nothing is deleted.
        let first = diff(&files, &[], &Default::default());
        assert_eq!((first.upload.len(), first.delete.len()), (3, 0));
    }

    #[test]
    fn switching_campaigns_never_deletes_the_others_backup() {
        use cloud::{diff, prepare, Provider, Uploaded};
        let dir = temp_dir("campaign-switch");
        let (config, lore, side) = (dir.join("config"), dir.join("Lore"), dir.join("Side"));
        for (vault, notes) in [(&lore, ["NPCs/Vex.md", "Sessions/Session 1.md"]), (&side, ["NPCs/Bob.md", "Sessions/Session 1.md"])] {
            for rel in notes {
                fs::create_dir_all(vault.join(rel).parent().unwrap()).unwrap();
                fs::write(vault.join(rel), format!("{} {rel}", vault.display())).unwrap();
            }
        }
        fs::create_dir_all(&config).unwrap();
        let path = |p: &Path| p.to_string_lossy().into_owned();
        let s = Settings { vault_path: path(&lore), campaigns: vec![path(&lore), path(&side)], ..Settings::default() };
        let (main, other) = (backup::backup_name(&s, &path(&lore)), backup::backup_name(&s, &path(&side)));
        assert_eq!((main.as_str(), other.as_str()), ("Lore", "Side"));
        // A backup name of your own wins; an empty one doesn't count.
        let named = Settings { backup_names: [(path(&lore), "Phandelver".into()), (path(&side), String::new())].into(), ..s.clone() };
        assert_eq!((backup::backup_name(&named, &path(&lore)), backup::backup_name(&named, &path(&side))), ("Phandelver".into(), "Side".into()));

        // Back up a campaign as a run would, recording every upload in its manifest.
        let back_up = |vault: &Path, name: &str| {
            let (file, mut m, local, skipped) = prepare(Provider::Dropbox, "me@x.com", &config, vault, name).unwrap();
            let plan = diff(&local, &skipped, &m.files);
            let deleted = plan.delete.clone();
            let uploaded: Vec<String> = plan.upload.iter().map(|f| f.rel.clone()).collect();
            for f in &plan.upload {
                m.files.insert(f.rel.clone(), Uploaded { hash: f.hash.clone(), id: String::new() });
            }
            fs::write(file, serde_json::to_string(&m).unwrap()).unwrap();
            (uploaded, deleted)
        };
        // A second backup of the same campaign uploads nothing again.
        assert_eq!(back_up(&lore, &main).0.len(), 2);
        let before = fs::read_to_string(config.join("cloud-dropbox-lore.json")).unwrap();
        assert_eq!(back_up(&lore, &main), (vec![], vec![]));

        // Switch: the other campaign has its own manifest, so the first one's files never look deleted,
        // and it uploads to its own place.
        let (up, deleted) = back_up(&side, &other);
        assert_eq!((up.len(), deleted.len()), (2, 0));
        assert_eq!(fs::read_to_string(config.join("cloud-dropbox-lore.json")).unwrap(), before, "the first campaign's manifest is untouched");
        assert!(config.join("cloud-dropbox-side.json").exists());
        // Switch back: still nothing to upload or delete for either.
        assert_eq!(back_up(&lore, &main), (vec![], vec![]));
        assert_eq!(back_up(&side, &other), (vec![], vec![]));
        // Deleting a note in one campaign only ever deletes that campaign's copy.
        fs::remove_file(side.join("Sessions/Session 1.md")).unwrap();
        assert_eq!(back_up(&side, &other), (vec![], vec!["Sessions/Session 1.md".to_string()]));
        assert_eq!(back_up(&lore, &main), (vec![], vec![]));

        // A manifest is only started over for another account (or place), never for one that names no account.
        fs::write(config.join("cloud-google-lore.json"), r#"{"account":"","place":"google:Lorekeeper/Lore","files":{"NPCs/Vex.md":{"hash":"x"}}}"#).unwrap();
        assert_eq!(prepare(Provider::Google, "me@x.com", &config, &lore, &main).unwrap().1.files.len(), 1);
        fs::write(config.join("cloud-google-lore.json"), r#"{"account":"you@x.com","place":"google:Lorekeeper/Lore","files":{"NPCs/Vex.md":{"hash":"x"}}}"#).unwrap();
        let (_, m, _, _) = prepare(Provider::Google, "me@x.com", &config, &lore, &main).unwrap();
        assert_eq!((m.account.as_str(), m.files.len()), ("me@x.com", 0));

        // Every destination differs: a folder named after each campaign in one shared place.
        assert_eq!((dropbox::root(&main), dropbox::root(&other)), ("/Lore".to_string(), "/Side".to_string()));
        assert_eq!((cloud::place(Provider::Google, &main), cloud::place(Provider::Google, &other)), ("google:Lorekeeper/Lore".to_string(), "google:Lorekeeper/Side".to_string()));
        assert_eq!(backup::slug("Café Ω"), "caf-e9-3a9");

        // GitHub: a backup of one campaign replaces only its own folder in the shared repository.
        let tree = |path: &str, sha: &str| serde_json::json!({ "path": path, "mode": "040000", "type": "tree", "sha": sha, "url": "u" });
        let readme = serde_json::json!({ "path": "README.md", "mode": "100644", "type": "blob", "sha": "r1", "size": 9 });
        let root = [readme.clone(), tree("Lore", "l1"), tree("Side", "s1"), tree("NPCs", "old")];
        let after = github::root_entries(&root, &other, "s2");
        let sha_of = |path: &str| after.iter().filter(|e| e["path"] == path).map(|e| e["sha"].as_str().unwrap()).collect::<Vec<_>>();
        assert_eq!(after.len(), 4);
        assert_eq!((sha_of("README.md"), sha_of("Lore"), sha_of("NPCs"), sha_of("Side")), (vec!["r1"], vec!["l1"], vec!["old"], vec!["s2"]));
        assert!(after.iter().all(|e| e.as_object().unwrap().len() == 4), "only path, mode, type and sha go back to GitHub");
        assert_eq!(after.iter().find(|e| e["path"] == "README.md").unwrap()["mode"], "100644");
        // The first backup of a campaign adds its folder next to the others.
        let first = github::root_entries(&root, "Strahd", "t1");
        assert_eq!(first.len(), 5);
        assert_eq!(first[..4].iter().map(|e| e["sha"].as_str().unwrap()).collect::<Vec<_>>(), ["r1", "l1", "s1", "old"]);
        assert_eq!(first[4], serde_json::json!({ "path": "Strahd", "mode": "040000", "type": "tree", "sha": "t1" }));
        // A name that only starts the same is another campaign.
        assert_eq!(github::root_entries(&[tree("Strahd 2", "x")], "Strahd", "t1").len(), 2);

        // Folder backup: each campaign's dated copy survives the other's backup.
        let (folder, day) = (dir.join("Backups"), chrono::NaiveDate::from_ymd_opt(2026, 3, 7).unwrap());
        fs::create_dir_all(&folder).unwrap();
        backup::backup_campaign_to_folder(&lore, &folder, &main, day).unwrap();
        backup::backup_campaign_to_folder(&side, &folder, &other, day).unwrap();
        let snap = backup::snapshot_name(day);
        assert!(folder.join("Lore").join(&snap).join("NPCs/Vex.md").exists() && !folder.join("Lore").join(&snap).join("NPCs/Bob.md").exists());
        assert!(folder.join("Side").join(&snap).join("NPCs/Bob.md").exists());
        backup::backup_campaign_to_folder(&lore, &folder, &main, day).unwrap();
        assert!(folder.join("Side").join(&snap).join("NPCs/Bob.md").exists(), "one campaign's backup leaves the other's folder alone");
        // A backup folder that isn't there is never created, not even the campaign's folder inside it.
        assert!(backup::backup_campaign_to_folder(&side, &dir.join("Unplugged"), &other, day).is_err());
        assert!(!dir.join("Unplugged").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cloud_scan_skips_hidden_and_big_files() {
        let dir = temp_dir("cloud-scan");
        fs::create_dir_all(dir.join("NPCs")).unwrap();
        fs::create_dir_all(dir.join(".obsidian")).unwrap();
        fs::write(dir.join("NPCs/Vex.md"), "# Vex").unwrap();
        fs::write(dir.join("map.png"), vec![0u8; 2000]).unwrap();
        fs::write(dir.join(".obsidian/app.json"), "{}").unwrap();
        let (files, skipped) = cloud::scan(&dir, 1000).unwrap();
        assert_eq!(files.iter().map(|f| f.rel.as_str()).collect::<Vec<_>>(), ["NPCs/Vex.md"]);
        assert_eq!(files[0].hash, github::git_blob_sha(b"# Vex"));
        assert_eq!(skipped, ["map.png"]);
        assert!(cloud::scan(&dir.join("missing"), 1000).is_err(), "a missing vault must never look empty");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn drive_folders_to_create() {
        use gdrive::{missing_folders, parent};
        use std::collections::BTreeMap;
        assert_eq!((parent("Top.md"), parent("NPCs/Villains/Vex.md")), ("", "NPCs/Villains"));
        let none = BTreeMap::new();
        assert_eq!(missing_folders("Top.md", &none), [""]);
        assert_eq!(missing_folders("NPCs/Villains/Vex.md", &none), ["", "NPCs", "NPCs/Villains"]);
        let known: BTreeMap<String, String> = [("", "root"), ("NPCs", "n1")].map(|(k, v)| (k.into(), v.into())).into();
        assert_eq!(missing_folders("NPCs/Villains/Vex.md", &known), ["NPCs/Villains"]);
        assert!(missing_folders("NPCs/Mirela.md", &known).is_empty());
    }

    #[test]
    fn dropbox_header_and_graph_paths() {
        use serde_json::json;
        assert_eq!(dropbox::header_json(&json!({"path": "/NPCs/Vex.md"})), r#"{"path":"/NPCs/Vex.md"}"#);
        // Non-ASCII becomes \uXXXX, outside the basic plane as a surrogate pair.
        assert_eq!(dropbox::header_json(&json!({"path": "/Café 🐉.md"})), r#"{"path":"/Caf\u00e9 \ud83d\udc09.md"}"#);
    }

    #[test]
    fn backup_snapshot_names_and_retention() {
        use backup::{is_snapshot, old_snapshots, snapshot_name};
        let day = chrono::NaiveDate::from_ymd_opt(2026, 3, 7).unwrap();
        assert_eq!(snapshot_name(day), "Lorekeeper backup 2026-03-07");
        assert!(is_snapshot("Lorekeeper backup 2026-03-07"));
        let others = ["Lorekeeper backup 2026-3-7", "Lorekeeper backup 2026-02-30", "Lorekeeper backup 2026-03-07 copy", "lorekeeper backup 2026-03-07", "Photos", ".Lorekeeper backup 2026-03-07.partial"];
        assert!(others.iter().all(|n| !is_snapshot(n)));
        // 35 dated folders across a year end, in any order, plus unrelated ones: only the 5 oldest dated ones go.
        let dated: Vec<String> = (0..35).map(|i| snapshot_name(day - chrono::Days::new(120 - i * 3))).collect();
        let mut names: Vec<String> = others.iter().map(|n| n.to_string()).chain(dated.iter().rev().cloned()).collect();
        names.swap(3, 20);
        let mut gone = old_snapshots(names);
        gone.sort();
        assert_eq!(gone, dated[..5]);
        assert!(old_snapshots(dated[..30].to_vec()).is_empty());
    }

    #[test]
    fn backup_folder_inside_the_vault_is_refused() {
        let vault = Path::new("/x/Lore");
        assert!(backup::check_folder(vault, Path::new("/x/Lore")).is_err());
        assert!(backup::check_folder(vault, Path::new("/x/Lore/Backups")).is_err());
        assert!(backup::check_folder(vault, Path::new("/x/Lorekeeper")).is_ok(), "a name that only starts the same is fine");
        assert!(backup::check_folder(vault, Path::new("/x")).is_ok(), "the vault may sit inside the backup folder");
        // ...but not inside one of the dated copies, which backups replace and prune.
        assert!(backup::check_folder(Path::new("/b/Lorekeeper backup 2026-09-01"), Path::new("/b")).is_err());
        assert!(backup::check_folder(Path::new("/b/Lorekeeper backup 2026-09-01/sub"), Path::new("/b")).is_err());
        assert!(backup::check_folder(Path::new("/b/Restored/Lorekeeper backup 2026-09-01"), Path::new("/b")).is_ok());
        // Through a symlink (macOS: /var -> /private/var), with a backup folder that doesn't exist yet.
        let dir = temp_dir("backup-inside");
        fs::create_dir_all(&dir).unwrap();
        let real = fs::canonicalize(&dir).unwrap();
        assert!(backup::check_folder(&real, &dir.join("not yet/Backups")).is_err());
        assert!(backup::check_folder(&dir, &real.join("Backups")).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn folder_backup_round_trip() {
        use backup::{backup_to_folder, snapshot_name};
        let dir = temp_dir("backup-roundtrip");
        let (vault, folder) = (dir.join("vault"), dir.join("Drive"));
        let day = chrono::NaiveDate::from_ymd_opt(2026, 3, 7).unwrap();
        fs::create_dir_all(vault.join("NPCs/Villains")).unwrap();
        fs::create_dir_all(vault.join("Items")).unwrap();
        fs::create_dir_all(vault.join(".obsidian")).unwrap();
        fs::write(vault.join("NPCs/Villains/Vex.md"), "# Vex").unwrap();
        fs::write(vault.join("Gone.md"), "soon deleted").unwrap();
        fs::write(vault.join(".obsidian/app.json"), "{}").unwrap();
        fs::write(vault.join(".DS_Store"), "").unwrap();

        // Refused: a backup folder that isn't there (never created at backup time), a missing vault,
        // a folder inside the vault.
        assert!(backup_to_folder(&vault, &folder, day).is_err());
        fs::create_dir_all(&folder).unwrap();
        assert!(backup_to_folder(&dir.join("missing"), &folder, day).is_err());
        fs::create_dir_all(vault.join("Backups")).unwrap();
        assert!(backup_to_folder(&vault, &vault.join("Backups"), day).is_err());
        fs::remove_dir(vault.join("Backups")).unwrap();
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 0);

        backup_to_folder(&vault, &folder, day).unwrap();
        let snap = folder.join("Lorekeeper backup 2026-03-07");
        assert_eq!(fs::read_to_string(snap.join("NPCs/Villains/Vex.md")).unwrap(), "# Vex");
        assert!(snap.join("Gone.md").exists() && snap.join("Items").is_dir());
        assert!(!snap.join(".obsidian").exists() && !snap.join(".DS_Store").exists());

        // Same day again: today's copy is replaced, not merged, and no temp folders are left behind.
        fs::remove_file(vault.join("Gone.md")).unwrap();
        fs::write(vault.join("NPCs/Villains/Vex.md"), "# Vex v2").unwrap();
        backup_to_folder(&vault, &folder, day).unwrap();
        assert_eq!(fs::read_to_string(snap.join("NPCs/Villains/Vex.md")).unwrap(), "# Vex v2");
        assert!(!snap.join("Gone.md").exists());
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 1);

        // Pruning keeps the newest 30 dated copies and never touches anything else.
        fs::create_dir_all(folder.join("Photos")).unwrap();
        fs::write(folder.join("Lorekeeper backup 2020-01-01"), "a file, not a backup").unwrap();
        for i in 1..=31 {
            fs::create_dir_all(folder.join(snapshot_name(day - chrono::Days::new(i)))).unwrap();
        }
        backup_to_folder(&vault, &folder, day).unwrap();
        let left: Vec<String> = fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left.len(), 32, "{left:?}"); // 30 copies + Photos + the file
        assert!(left.contains(&"Photos".to_string()) && left.contains(&"Lorekeeper backup 2020-01-01".to_string()));
        assert!(left.contains(&snapshot_name(day)) && left.contains(&snapshot_name(day - chrono::Days::new(29))));
        assert!(!left.contains(&snapshot_name(day - chrono::Days::new(30))));
        fs::remove_dir_all(&dir).unwrap();
    }
}
