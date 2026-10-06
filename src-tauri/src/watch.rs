//! Notices changes made outside the app (a teammate's notes arriving through Google Drive, an edit in Obsidian) and
//! tells the windows, which already refresh on "vault-changed".

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    thread,
    time::{Duration, SystemTime},
};

use tauri::{AppHandle, Emitter};

/// Every file in the open campaign (hidden ones skipped) with its size and modification time.
pub(crate) type Stamps = BTreeMap<PathBuf, (u64, Option<SystemTime>)>;

/// Files the app itself wrote since the last look; their changes are already on screen.
static OWN: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

pub(crate) fn wrote(path: &Path) {
    OWN.lock().unwrap().push(path.to_path_buf());
}

pub(crate) fn stamps(root: &Path) -> Stamps {
    fn add(dir: &Path, out: &mut Stamps) {
        for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if meta.is_dir() {
                add(&entry.path(), out);
            } else {
                out.insert(entry.path(), (meta.len(), meta.modified().ok()));
            }
        }
    }
    let mut out = Stamps::new();
    add(root, &mut out);
    out
}

/// Whether any file was added, removed or changed between `old` and `new` other than the app's `own` writes.
pub(crate) fn changed_by_others(old: &Stamps, new: &Stamps, own: &[PathBuf]) -> bool {
    old.keys().chain(new.keys()).any(|p| old.get(p) != new.get(p) && !own.contains(p))
}

/// Looks at the open campaign every 3 seconds (only file sizes and times, so it's cheap). A campaign switch starts over
/// quietly, since switching refreshes the windows itself.
/// ponytail: an outside change to a file the app wrote in the same 3 seconds is missed until the next change or focus.
pub(crate) fn start(app: AppHandle) {
    thread::spawn(move || {
        let (mut root, mut last) = (PathBuf::new(), Stamps::new());
        loop {
            thread::sleep(Duration::from_secs(3));
            let dir = crate::notes_dir(&app);
            let own = std::mem::take(&mut *OWN.lock().unwrap());
            let now = stamps(&dir);
            if dir == root && changed_by_others(&last, &now, &own) {
                crate::backup::mark_changed(); // a teammate's notes get backed up too
                let _ = app.emit("vault-changed", ());
            }
            if dir == root && last != now {
                crate::shared::poke(); // anything changed, the app's own writes too: a shared campaign syncs it
            }
            (root, last) = (dir, now);
        }
    });
}
