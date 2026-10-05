//! In-app updates from GitHub Releases. The endpoint and public key are in tauri.conf.json
//! (plugins > updater); release.yml signs the downloads and uploads latest.json.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    thread,
    time::{Duration, SystemTime},
};

use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;

use crate::Settings;

const FIRST_CHECK: Duration = Duration::from_secs(30);
const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// How often the scheduler looks at the clock. Wall-clock time, so a laptop that sleeps still checks daily.
const TICK: Duration = Duration::from_secs(60 * 60);
const MAX_NOTES: usize = 400;

static CHECKING: AtomicBool = AtomicBool::new(false);

/// Due when there was no successful check yet, the last one is a day old, or the clock went backwards.
pub fn due(last: Option<SystemTime>, now: SystemTime) -> bool {
    last.is_none_or(|l| now.duration_since(l).map_or(true, |d| d >= INTERVAL))
}

/// The dialog text; long release notes are cut so the buttons stay on screen.
pub fn prompt(version: &str, current: &str, notes: Option<&str>) -> String {
    let notes = notes.map(str::trim).filter(|n| !n.is_empty()).map(|n| {
        let mut short: String = n.chars().take(MAX_NOTES).collect();
        if n.chars().count() > MAX_NOTES {
            short.push('…');
        }
        format!("\n\n{short}")
    });
    format!("Lorekeeper {version} is available. You have {current}.{}\n\nInstall and restart now?", notes.unwrap_or_default())
}

fn message(app: &AppHandle, kind: MessageDialogKind, text: impl Into<String>) {
    app.dialog().message(text).title("Lorekeeper").kind(kind).blocking_show();
}

/// Checks on a worker thread (the dialogs block) and returns right away. Manual checks always answer;
/// automatic ones only speak up when there's an update.
pub fn check(app: &AppHandle, manual: bool) {
    let app = app.clone();
    thread::spawn(move || {
        run(&app, manual);
    });
}

/// "Check now" in Settings.
#[tauri::command]
pub fn check_for_updates(app: AppHandle) {
    check(&app, true);
}

/// Returns false when the check itself failed.
fn run(app: &AppHandle, manual: bool) -> bool {
    if CHECKING.swap(true, Ordering::SeqCst) {
        if manual {
            message(app, MessageDialogKind::Info, "Lorekeeper is already checking for updates.");
        }
        return true;
    }
    struct Done;
    impl Drop for Done {
        fn drop(&mut self) {
            CHECKING.store(false, Ordering::SeqCst);
        }
    }
    let _done = Done;

    let found = tauri::async_runtime::block_on(async { app.updater()?.check().await });
    let update = match found {
        Ok(Some(update)) => update,
        Ok(None) => {
            if manual {
                message(app, MessageDialogKind::Info, format!("You're up to date (version {}).", app.package_info().version));
            }
            return true;
        }
        Err(e) => {
            eprintln!("update check failed: {e}");
            if manual {
                message(app, MessageDialogKind::Warning, failure(&e, &app.package_info().version.to_string()));
            }
            return false;
        }
    };

    let install = app
        .dialog()
        .message(prompt(&update.version, &update.current_version, update.body.as_deref()))
        .title("Update available")
        .buttons(MessageDialogButtons::OkCancelCustom("Install".into(), "Later".into()))
        .blocking_show();
    if !install {
        return true;
    }
    // On Windows the installer takes over and restarts the app, so this only returns elsewhere.
    match tauri::async_runtime::block_on(update.download_and_install(|_, _| {}, || {})) {
        Ok(()) => app.restart(),
        Err(e) => {
            message(app, MessageDialogKind::Error, format!("Couldn't install the update: {e}"));
            true
        }
    }
}

/// Plain-language text for a failed manual check.
fn failure(e: &tauri_plugin_updater::Error, version: &str) -> String {
    use tauri_plugin_updater::Error::*;
    match e {
        // No published release with update info (yet), or GitHub didn't answer with one.
        ReleaseNotFound => format!("No update information is available right now. You have version {version}. Try again later."),
        Reqwest(_) | Network(_) => "Couldn't reach GitHub to check for updates. Check your internet connection and try again.".into(),
        _ => format!("Couldn't check for updates ({e})."),
    }
}

/// Automatic checks: 30 s after launch, then once a day while "Check for updates automatically" is on.
/// A failed check (offline after waking up) is retried on the next tick.
pub fn start(app: AppHandle) {
    thread::spawn(move || {
        thread::sleep(FIRST_CHECK);
        let mut last = None;
        loop {
            let now = SystemTime::now();
            if app.state::<Mutex<Settings>>().lock().unwrap().auto_update && due(last, now) && run(&app, false) {
                last = Some(now);
            }
            thread::sleep(TICK);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_are_due_daily() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        assert!(due(None, now));
        assert!(!due(Some(now - Duration::from_secs(23 * 60 * 60)), now));
        assert!(due(Some(now - INTERVAL), now));
        assert!(due(Some(now - Duration::from_secs(3 * 24 * 60 * 60)), now));
        assert!(due(Some(now + Duration::from_secs(60)), now), "clock went backwards");
    }

    #[test]
    fn failed_check_messages() {
        let msg = failure(&tauri_plugin_updater::Error::ReleaseNotFound, "0.2.0");
        assert!(msg.starts_with("No update information is available right now. You have version 0.2.0."));
        assert!(failure(&tauri_plugin_updater::Error::Network("offline".into()), "0.2.0").contains("internet connection"));
    }

    #[test]
    fn update_prompt() {
        assert_eq!(prompt("0.3.0", "0.2.0", None), "Lorekeeper 0.3.0 is available. You have 0.2.0.\n\nInstall and restart now?");
        assert_eq!(prompt("0.3.0", "0.2.0", Some("  \n")), prompt("0.3.0", "0.2.0", None));
        assert!(prompt("0.3.0", "0.2.0", Some("Faster search.\n")).contains("0.2.0.\n\nFaster search.\n\nInstall"));
        let long = prompt("0.3.0", "0.2.0", Some(&"x".repeat(1000)));
        assert!(long.contains(&format!("{}…\n\nInstall", "x".repeat(MAX_NOTES))) && !long.contains(&"x".repeat(MAX_NOTES + 1)));
    }
}
