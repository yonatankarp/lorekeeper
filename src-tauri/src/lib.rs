use std::{
    fs,
    io::{self, Write},
    path::{Component, Path, PathBuf},
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, RunEvent, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use tauri_plugin_notification::NotificationExt;

mod backup;
mod github;
mod obsidian;
mod updater;

// ---------- notes on disk: an Obsidian-compatible vault (default <Documents>/Lorekeeper) ----------
//   Sessions/Session N.md   written by the hotkeys
//   PCs/ NPCs/ Locations/ Items/ Factions/   your pages
//   Templates/   starting text for "New page" (Obsidian's {{title}} / {{date}} syntax)

const VAULT_FOLDERS: [&str; 7] = ["Sessions", "PCs", "NPCs", "Locations", "Items", "Factions", "Templates"];

const TEMPLATES: [(&str, &str); 5] = [
    ("PC", "---\ntype: pc\nplayer:\nclass:\nrace:\nlevel:\n---\n# {{title}}\n\n## Backstory\n\n## Notes\n"),
    ("NPC", "---\ntype: npc\nrace:\nrole:\nlocation:\nstatus: alive\nfirst-met: {{date}}\n---\n# {{title}}\n\n## Description\n\n## Notes\n"),
    ("Location", "---\ntype: location\nregion:\n---\n# {{title}}\n\n## Description\n\n## Notable people\n\n## Notes\n"),
    ("Item", "---\ntype: item\nrarity:\nowner:\n---\n# {{title}}\n\n## Description\n\n## Notes\n"),
    ("Faction", "---\ntype: faction\nleader:\nbase:\n---\n# {{title}}\n\n## Goals\n\n## Members\n\n## Notes\n"),
];

/// Passed by the launch-at-login entry so a login start doesn't pop the window open.
const LOGIN_ARG: &str = "--from-login";

/// Serializes writes from the hotkeys and the editor so neither loses the other's change.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// The notes folder chosen in Settings.
fn notes_dir(app: &AppHandle) -> PathBuf {
    PathBuf::from(&app.state::<Mutex<Settings>>().lock().unwrap().vault_path)
}

fn default_vault(app: &AppHandle) -> PathBuf {
    let base = app.path().document_dir().or_else(|_| app.path().home_dir());
    base.expect("no home directory").join("Lorekeeper")
}

/// Creates the standard folders. Default templates are written only when Templates/ is new,
/// so a template you delete stays deleted.
fn create_vault_folders(dir: &Path) -> io::Result<()> {
    let fresh_templates = !dir.join("Templates").exists();
    VAULT_FOLDERS.iter().try_for_each(|f| fs::create_dir_all(dir.join(f)))?;
    if fresh_templates {
        for (name, body) in TEMPLATES {
            fs::write(dir.join("Templates").join(format!("{name}.md")), body)?;
        }
    }
    Ok(())
}

fn session_number(file_name: &str) -> Option<u32> {
    file_name.strip_prefix("Session ")?.strip_suffix(".md")?.parse().ok()
}

fn latest_session(dir: &Path) -> io::Result<u32> {
    let sessions = dir.join("Sessions");
    fs::create_dir_all(&sessions)?;
    Ok(fs::read_dir(sessions)?
        .filter_map(|e| session_number(e.ok()?.file_name().to_str()?))
        .max()
        .unwrap_or(0))
}

fn session_path(dir: &Path, n: u32) -> PathBuf {
    dir.join("Sessions").join(format!("Session {n}.md"))
}

/// Starts the next session file, with Obsidian properties so sessions can be listed as a table.
fn new_session(dir: &Path) -> io::Result<PathBuf> {
    let n = latest_session(dir)? + 1;
    let path = session_path(dir, n);
    let date = chrono::Local::now().format("%Y-%m-%d");
    fs::write(&path, format!("---\nsession: {n}\ndate: {date}\n---\n# Session {n} - {date}\n\n"))?;
    Ok(path)
}

/// The newest session file. Sessions only roll over via "New Session", never by date,
/// so a game running past midnight stays in one file.
fn current_session(dir: &Path) -> io::Result<PathBuf> {
    match latest_session(dir)? {
        0 => new_session(dir),
        n => Ok(session_path(dir, n)),
    }
}

/// Appends one note as a single line; multi-line selections are collapsed so the
/// one-note-per-line format survives. Returns the saved text (empty = nothing saved).
fn append_note(dir: &Path, text: &str) -> io::Result<String> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if !text.is_empty() {
        let _guard = WRITE_LOCK.lock().unwrap();
        let time = chrono::Local::now().format("%H:%M");
        let mut file = fs::OpenOptions::new().append(true).open(current_session(dir)?)?;
        writeln!(file, "- {time} {text}")?;
    }
    Ok(text)
}

/// Turns a path from the webview into a file inside the vault. Only plain relative `.md`
/// paths pass: no `..`, no absolute paths or drive prefixes.
fn vault_file(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let p = Path::new(rel);
    let ok = p.extension().is_some_and(|e| e == "md") && p.components().all(|c| matches!(c, Component::Normal(_)));
    if ok { Ok(root.join(p)) } else { Err(format!("Not a note in the vault: {rel}")) }
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
            theme: "system".into(),
            editor_font_size: 15,
            session_view: "timeline".into(),
            notifications: true,
            launch_at_login: false,
            backup_folder: String::new(),
            github_repo: "lorekeeper-notes".into(),
            github_user: String::new(),
            auto_update: true,
            new_session: String::new(),
            new_page: String::new(),
        }
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
        backup::check_folder(Path::new(&s.vault_path), Path::new(&s.backup_folder))?;
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

/// Opens the current session in Obsidian once the notes folder is in a vault. Otherwise starts Obsidian
/// with the folder's path on the clipboard, ready for "Open folder as vault", or shows the folder.
fn open_notes(app: &AppHandle) {
    let dir = notes_dir(app);
    if obsidian::vault_root(&dir).is_some() {
        match current_session(&dir) {
            Ok(session) => obsidian::open(&session),
            Err(_) => open_external(&dir),
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
    match append_note(&notes_dir(app), &text) {
        Ok(saved) if saved.is_empty() => notify(app, "Nothing to save", "No text selected or copied."),
        Ok(saved) => {
            emit_changed(app);
            notify_saved(app, title, &preview(&saved));
        }
        Err(e) => notify(app, "Couldn't save note", &e.to_string()),
    }
}

/// Registers both hotkeys. Returns the ones that couldn't be registered, e.g. "Quick note (CmdOrCtrl+Alt+N)".
fn register_shortcuts(app: &AppHandle, s: &Settings) -> Vec<String> {
    let gs = app.global_shortcut();
    let quick = gs.on_shortcut(s.quick_note.as_str(), |app, _, e| {
        if e.state == ShortcutState::Pressed {
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
    match new_session(&notes_dir(app)) {
        Ok(p) => {
            emit_changed(app);
            backup::request(false); // captures the session that just ended
            notify_saved(app, "New session started", &p.file_stem().unwrap_or_default().to_string_lossy());
        }
        Err(e) => notify(app, "Couldn't start session", &e.to_string()),
    }
}

/// The New Page hotkey: brings up the notes window with the New page dialog open.
fn open_new_page(app: &AppHandle) {
    show_window(app, "main");
    let _ = app.emit("new-page", ());
}

// ---------- commands for the windows ----------

/// From the quick box. Returns the session the note went into ("Session 3") for the box to show.
#[tauri::command]
fn save_note(app: AppHandle, text: String) -> Result<String, String> {
    let dir = notes_dir(&app);
    match append_note(&dir, &text).and_then(|_| current_session(&dir)) {
        Ok(path) => {
            emit_changed(&app);
            Ok(path.file_stem().unwrap_or_default().to_string_lossy().into_owned())
        }
        Err(e) => {
            notify(&app, "Couldn't save note", &e.to_string());
            Err(e.to_string())
        }
    }
}

/// Hides the quick-note box and hands focus back to the app you were in.
#[tauri::command]
fn dismiss(app: AppHandle) {
    if let Some(w) = app.get_webview_window("capture") {
        let _ = w.hide();
    }
    #[cfg(target_os = "macos")]
    if !app_window_visible(&app, "capture") {
        let _ = app.hide();
    }
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
        vault.current_session = rel_path(&root, &session_path(&root, n));
    }
    vault.has_obsidian = obsidian::vault_root(&root).is_some();
    vault.obsidian_installed = obsidian::installed();
    Ok(vault)
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
    backup::mark_changed();
    Ok(merged)
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
    backup::mark_changed();
    Ok(())
}

#[tauri::command]
fn start_session(app: AppHandle) -> Result<String, String> {
    let root = notes_dir(&app);
    let path = new_session(&root).map_err(|e| e.to_string())?;
    emit_changed(&app);
    backup::request(false); // captures the session that just ended
    Ok(rel_path(&root, &path))
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
fn copy_html(app: AppHandle, html: String, text: String) -> Result<(), String> {
    app.clipboard().write_html(html, Some(text)).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_settings(app: AppHandle) -> Settings {
    current_settings(&app)
}

/// Checks and applies every setting, saves them, then tells all windows. On error nothing changes.
#[tauri::command]
fn save_settings(app: AppHandle, settings: Settings) -> Result<Settings, String> {
    let mut new = validate(settings)?;
    let old = current_settings(&app);
    new.github_user = old.github_user.clone();
    // Existing notes stay in the old folder; the new one gets the standard folders and templates.
    if new.vault_path != old.vault_path {
        create_vault_folders(Path::new(&new.vault_path)).map_err(|e| format!("Notes folder: {e}"))?;
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
    if new.vault_path != old.vault_path {
        emit_changed(&app);
    }
    if new.backup_folder != old.backup_folder {
        backup::reset(&app, false); // a new place: back up there right away
    }
    Ok(new)
}

/// Saves settings as they are (no checks) and tells every window.
fn store_settings(app: &AppHandle, new: &Settings) -> Result<(), String> {
    let file = settings_file(app).map_err(|e| e.to_string())?;
    write_settings(&file, new).map_err(|e| format!("Couldn't save settings: {e}"))?;
    *app.state::<Mutex<Settings>>().lock().unwrap() = new.clone();
    let _ = app.emit("settings-changed", new);
    Ok(())
}

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

// ---------- backups (see backup.rs and github.rs) ----------

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
    backup::reset(&app, true); // first backup right away
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
    backup::reset(&app, true);
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
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&review, &new, &folder, &sep, &settings, &update, &login, &quit])?;
    app.manage(login); // for set_launch_at_login

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
            "review" => show_window(app, "main"),
            "new" => start_new_session(app),
            "folder" => open_notes(app),
            "settings" => show_window(app, "settings"),
            "update" => updater::check(app, true),
            "login" => {
                let on = app.autolaunch().is_enabled().unwrap_or(false);
                if let Err(e) = set_launch_at_login(app, !on) {
                    notify(app, "Couldn't change Launch at Login", &e);
                }
                let _ = app.emit("settings-changed", current_settings(app));
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec![LOGIN_ARG])))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            save_note,
            dismiss,
            read_vault,
            page_names,
            save_file,
            create_file,
            start_session,
            open_in_obsidian,
            open_url,
            copy_html,
            get_settings,
            save_settings,
            open_settings,
            pick_folder,
            open_vault_folder,
            backup_now,
            backup_status,
            github_sign_in_start,
            github_sign_in_wait,
            github_sign_in_cancel,
            github_sign_out,
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
                Settings { vault_path: vault.to_string_lossy().into_owned(), ..Settings::default() }
            });
            // A folder on an unplugged drive shouldn't stop the app from starting.
            if let Err(e) = create_vault_folders(Path::new(&settings.vault_path)) {
                notify(handle, "Notes folder unavailable", &format!("{}: {e}", settings.vault_path));
            }
            app.manage(Mutex::new(settings.clone()));
            build_tray(handle)?;
            refresh_login_item(handle);
            backup::start(handle.clone());
            updater::start(handle.clone());
            for keys in register_shortcuts(handle, &settings) {
                notify(handle, "Shortcut unavailable", &format!("{keys} couldn't be registered. Change it in Settings."));
            }
            // Opened by hand: show the notes. Started at login: stay quietly in the menu bar.
            if !std::env::args().any(|a| a == LOGIN_ARG) {
                show_window(app.handle(), "main");
            }
            Ok(())
        })
        // Closing a window only hides it; the app keeps running in the tray.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                #[cfg(target_os = "macos")]
                if !app_window_visible(window.app_handle(), window.label()) {
                    let _ = window.app_handle().set_activation_policy(tauri::ActivationPolicy::Accessory);
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| match event {
            RunEvent::ExitRequested { api, code: None, .. } => api.prevent_exit(),
            // Clicking the app in the Dock, Finder or Spotlight while it's running.
            #[cfg(target_os = "macos")]
            RunEvent::Reopen { .. } => show_window(app, "main"),
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
        create_vault_folders(&dir).unwrap();
        assert!(dir.join("NPCs").is_dir());
        assert!(fs::read_to_string(dir.join("Templates/NPC.md")).unwrap().contains("{{title}}"));
        fs::remove_file(dir.join("Templates/NPC.md")).unwrap();
        create_vault_folders(&dir).unwrap();
        assert!(!dir.join("Templates/NPC.md").exists(), "deleted template must stay deleted");

        // First note creates Session 1 (with properties); multi-line text becomes one line.
        assert_eq!(append_note(&dir, "  @Mirela\n the  innkeeper ").unwrap(), "@Mirela the innkeeper");
        assert_eq!(append_note(&dir, " \n ").unwrap(), "");
        let s1 = fs::read_to_string(dir.join("Sessions/Session 1.md")).unwrap();
        assert!(s1.starts_with("---\nsession: 1\ndate: "));
        assert!(s1.contains("\n# Session 1 - "));
        assert_eq!(s1.lines().filter(|l| l.starts_with("- ")).count(), 1);
        assert!(s1.trim_end().ends_with(" @Mirela the innkeeper"));

        // New session: notes go to the newest file; other files are ignored. Session 10 sorts after 9.
        fs::write(dir.join("Sessions/Ideas.md"), "x").unwrap();
        new_session(&dir).unwrap();
        append_note(&dir, "#potion").unwrap();
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
    fn settings_load_defaults_partial_files_and_migrate() {
        let dir = temp_dir("settings");
        let (config, vault) = (dir.join("config/settings.json"), dir.join("Lorekeeper"));
        let vault_path = vault.to_string_lossy().into_owned();

        // First run: defaults, pointing at the default vault, written to the config folder.
        let s = load_settings(&config, &vault).unwrap();
        assert_eq!(s, Settings { vault_path: vault_path.clone(), ..Settings::default() });
        assert_eq!((s.theme.as_str(), s.editor_font_size, s.session_view.as_str(), s.notifications), ("system", 15, "timeline", true));
        let written = fs::read_to_string(&config).unwrap();
        assert!(written.contains("\"editorFontSize\": 15") && !written.contains("launchAtLogin"));

        // A partial file keeps what it has and fills in the rest.
        fs::write(&config, r#"{"theme":"dark","editorFontSize":18}"#).unwrap();
        let s = load_settings(&config, &vault).unwrap();
        assert_eq!((s.theme.as_str(), s.editor_font_size, s.quick_note.as_str()), ("dark", 18, "CmdOrCtrl+Alt+N"));
        assert!(s.auto_update, "files from before automatic updates turn them on");
        assert_eq!(s.vault_path, vault_path);

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
        let ok = Settings { vault_path: std::env::temp_dir().to_string_lossy().into_owned(), ..Settings::default() };
        assert_eq!(validate(ok.clone()).unwrap(), ok);
        assert_eq!(validate(Settings { editor_font_size: 99, ..ok.clone() }).unwrap().editor_font_size, 24);
        assert_eq!(validate(Settings { editor_font_size: 2, ..ok.clone() }).unwrap().editor_font_size, 11);
        // Optional shortcuts: off by default, any distinct valid combination when set.
        let extra = Settings { new_session: "Ctrl+Alt+CmdOrCtrl+S".into(), new_page: "Ctrl+Alt+CmdOrCtrl+P".into(), ..ok.clone() };
        assert_eq!(validate(extra.clone()).unwrap(), extra);
        let bad = [
            Settings { theme: "purple".into(), ..ok.clone() },
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
        ];
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
