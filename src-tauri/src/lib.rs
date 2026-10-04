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
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tauri_plugin_notification::NotificationExt;

// ---------- notes on disk: an Obsidian-compatible vault at <Documents>/Lorekeeper ----------
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

fn notes_dir(app: &AppHandle) -> PathBuf {
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
    fs::write(&path, format!("---\nsession: {n}\ndate: {date}\n---\n# Session {n} — {date}\n\n"))?;
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

// ---------- settings.json (lives next to the notes) ----------

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    quick_note: String,
    capture: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            quick_note: "CmdOrCtrl+Alt+N".into(),
            capture: "CmdOrCtrl+Shift+S".into(),
        }
    }
}

/// Writes the defaults only when the file is missing, so a typo never wipes your settings.
fn load_settings(dir: &Path) -> Result<Settings, String> {
    let path = dir.join("settings.json");
    match fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).map_err(|e| format!("settings.json: {e}")),
        Err(_) => {
            let s = Settings::default();
            let _ = fs::write(&path, serde_json::to_string_pretty(&s).unwrap());
            Ok(s)
        }
    }
}

// ---------- helpers ----------

fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}

fn preview(text: &str) -> String {
    let mut p: String = text.chars().take(60).collect();
    if text.chars().count() > 60 {
        p.push('…');
    }
    p
}

fn show_window(app: &AppHandle, label: &str) {
    // dismiss() hides the whole app on macOS; a hidden app's windows won't show until it's unhidden.
    #[cfg(target_os = "macos")]
    {
        let _ = app.show();
        // The notes window behaves like a normal app (Dock icon, Cmd+Tab) while it's open.
        if label == "main" {
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

/// Opens the current session in Obsidian once the notes folder is a vault, otherwise the folder itself.
fn open_notes(app: &AppHandle) {
    let dir = notes_dir(app);
    let session = current_session(&dir).ok().filter(|_| dir.join(".obsidian").is_dir());
    match session.and_then(|p| tauri::Url::parse_with_params("obsidian://open", [("path", p.to_string_lossy())]).ok()) {
        Some(url) => open_external(url.as_str()),
        None => open_external(&dir),
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
            notify(app, title, &preview(&saved));
        }
        Err(e) => notify(app, "Couldn't save note", &e.to_string()),
    }
}

fn register_shortcuts(app: &AppHandle) {
    let s = load_settings(&notes_dir(app)).unwrap_or_else(|e| {
        notify(app, "Using default shortcuts", &e);
        Settings::default()
    });
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
    for (keys, result) in [(&s.quick_note, quick), (&s.capture, capture)] {
        if let Err(e) = result {
            notify(app, "Shortcut unavailable", &format!("{keys}: {e}. Change it in Lorekeeper/settings.json."));
        }
    }
}

// ---------- commands for the two windows ----------

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
    if !app.get_webview_window("main").is_some_and(|w| w.is_visible().unwrap_or(false)) {
        let _ = app.hide();
    }
}

fn emit_changed(app: &AppHandle) {
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
    vault.has_obsidian = root.join(".obsidian").is_dir();
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
    f.write_all(content.as_bytes()).map_err(|e| e.to_string())
}

#[tauri::command]
fn start_session(app: AppHandle) -> Result<String, String> {
    let root = notes_dir(&app);
    let path = new_session(&root).map_err(|e| e.to_string())?;
    emit_changed(&app);
    Ok(rel_path(&root, &path))
}

#[tauri::command]
fn open_in_obsidian(app: AppHandle, path: String) -> Result<(), String> {
    let root = notes_dir(&app);
    let file = vault_file(&root, &path)?;
    if !root.join(".obsidian").is_dir() {
        return Err("Open “Lorekeeper” in Obsidian once first (Open folder as vault).".into());
    }
    let url = tauri::Url::parse_with_params("obsidian://open", [("path", file.to_string_lossy())]).map_err(|e| e.to_string())?;
    open_external(url.as_str());
    Ok(())
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

// ---------- app ----------

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let autostart = app.autolaunch().is_enabled().unwrap_or(false);
    let review = MenuItem::with_id(app, "review", "Open Lorekeeper…", true, None::<&str>)?;
    let new = MenuItem::with_id(app, "new", "New Session", true, None::<&str>)?;
    let folder = MenuItem::with_id(app, "folder", "Open in Obsidian", true, None::<&str>)?;
    let login = CheckMenuItem::with_id(app, "login", "Launch at Login", true, autostart, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&review, &new, &folder, &sep, &login, &quit])?;

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
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "review" => show_window(app, "main"),
            "new" => match new_session(&notes_dir(app)) {
                Ok(p) => {
                    emit_changed(app);
                    notify(app, "New session started", &p.file_name().unwrap().to_string_lossy());
                }
                Err(e) => notify(app, "Couldn't start session", &e.to_string()),
            },
            "folder" => open_notes(app),
            "login" => {
                let al = app.autolaunch();
                let on = al.is_enabled().unwrap_or(false);
                let _ = if on { al.disable() } else { al.enable() };
                let _ = login.set_checked(al.is_enabled().unwrap_or(!on));
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
            copy_html
        ])
        .setup(|app| {
            // Menu-bar app: no Dock icon.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            create_vault_folders(&notes_dir(app.handle()))?;
            build_tray(app.handle())?;
            register_shortcuts(app.handle());
            // Opened by hand: show the notes. Started at login: stay quietly in the menu bar.
            if !std::env::args().any(|a| a == LOGIN_ARG) {
                show_window(app.handle(), "main");
            }
            Ok(())
        })
        // Closing the review window only hides it; the app keeps running in the tray.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                #[cfg(target_os = "macos")]
                if window.label() == "main" {
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
        assert!(s1.contains("\n# Session 1 — "));
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

        // A broken settings file is reported, not overwritten.
        fs::write(dir.join("settings.json"), "{oops").unwrap();
        assert!(load_settings(&dir).is_err());
        assert_eq!(fs::read_to_string(dir.join("settings.json")).unwrap(), "{oops");

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
}
