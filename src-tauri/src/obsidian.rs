//! Obsidian: which vault holds the notes folder, and opening notes in it. Obsidian has no API for
//! adding a vault, so outside one the window explains how instead.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Serialize;

/// What `open_in_obsidian` did: `{"opened":true}`, or `{"needsVault":true,"path":...,"installed":...}`
/// when the notes folder isn't in a vault yet and the window shows how to add it.
#[derive(Serialize, Debug, PartialEq)]
#[serde(untagged)]
pub enum Opened {
    Opened { opened: bool },
    #[serde(rename_all = "camelCase")]
    NeedsVault { needs_vault: bool, path: String, installed: bool },
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn env_dir(var: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).unwrap_or_default()
}

/// Where Obsidian keeps its list of vaults (obsidian.json).
fn registry_files() -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        vec![home().join("Library/Application Support/obsidian/obsidian.json")]
    } else if cfg!(target_os = "windows") {
        vec![env_dir("APPDATA").join("obsidian/obsidian.json")]
    } else {
        let config = std::env::var_os("XDG_CONFIG_HOME").map_or_else(|| home().join(".config"), PathBuf::from);
        vec![config.join("obsidian/obsidian.json"), home().join(".var/app/md.obsidian.Obsidian/config/obsidian/obsidian.json")]
    }
}

/// The usual install locations, for someone who installed Obsidian but never opened it.
fn app_files() -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        vec!["/Applications/Obsidian.app".into(), home().join("Applications/Obsidian.app")]
    } else if cfg!(target_os = "windows") {
        vec![env_dir("LOCALAPPDATA").join("Programs/Obsidian/Obsidian.exe"), env_dir("ProgramFiles").join("Obsidian/Obsidian.exe")]
    } else {
        ["/usr/bin/obsidian", "/opt/Obsidian/obsidian", "/snap/bin/obsidian", "/var/lib/flatpak/app/md.obsidian.Obsidian"]
            .into_iter()
            .map(PathBuf::from)
            .chain([home().join(".local/share/flatpak/app/md.obsidian.Obsidian")])
            .collect()
    }
}

/// Best effort: Obsidian has run here before, or sits where installers put it.
pub fn installed() -> bool {
    registry_files().iter().chain(&app_files()).any(|p| p.exists())
}

/// Vault folders listed in an obsidian.json (`{"vaults": {"<id>": {"path": ...}}}`); unreadable means none.
fn registered(json: &str) -> Vec<PathBuf> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return vec![] };
    let Some(vaults) = v.get("vaults").and_then(|v| v.as_object()) else { return vec![] };
    vaults.values().filter_map(|v| v.get("path")?.as_str()).map(PathBuf::from).collect()
}

/// Resolves symlinks (macOS /var -> /private/var) where the path exists.
fn canon(p: &Path) -> PathBuf {
    fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// The vault holding `notes_dir`: the nearest folder (itself or above) with a `.obsidian` folder,
/// else the innermost vault in the registry texts that contains it.
fn vault_root_in(notes_dir: &Path, registries: &[String]) -> Option<PathBuf> {
    let dir = canon(notes_dir);
    if let Some(root) = dir.ancestors().find(|a| a.join(".obsidian").is_dir()) {
        return Some(root.to_path_buf());
    }
    registries
        .iter()
        .flat_map(|r| registered(r))
        .map(|v| canon(&v))
        .filter(|v| dir.starts_with(v)) // whole components: /x/Lore doesn't contain /x/Lorekeeper
        .max_by_key(|v| v.components().count())
}

/// The Obsidian vault containing the notes folder, if any.
pub fn vault_root(notes_dir: &Path) -> Option<PathBuf> {
    let registries: Vec<String> = registry_files().iter().filter_map(|f| fs::read_to_string(f).ok()).collect();
    vault_root_in(notes_dir, &registries)
}

/// `obsidian://open?path=...`, which opens any file inside a vault Obsidian knows.
fn open_uri(file: &Path) -> String {
    let url = tauri::Url::parse_with_params("obsidian://open", [("path", file.to_string_lossy())]).expect("constant URL");
    // Form encoding writes spaces as '+' ("Session 3.md"); Obsidian wants %20. A real '+' is already %2B.
    url.as_str().replace('+', "%20")
}

pub fn open(file: &Path) {
    crate::open_external(open_uri(file));
}

/// Starts Obsidian (or brings it to the front).
pub fn launch() {
    if cfg!(target_os = "macos") {
        let _ = std::process::Command::new("open").args(["-a", "Obsidian"]).spawn();
    } else {
        crate::open_external("obsidian://open"); // ponytail: relies on the obsidian:// handler being registered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dnd-obsidian-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn registry(paths: &[&Path]) -> String {
        let vaults: serde_json::Map<_, _> = paths
            .iter()
            .enumerate()
            .map(|(i, p)| (format!("id{i}"), serde_json::json!({"path": p, "ts": 1, "open": i == 0})))
            .collect();
        serde_json::json!({ "vaults": vaults }).to_string()
    }

    #[test]
    fn vault_from_a_dot_obsidian_folder() {
        let dir = temp_dir("dot");
        let notes = dir.join("Campaign/Lorekeeper");
        fs::create_dir_all(notes.join("NPCs")).unwrap();
        assert_eq!(vault_root_in(&notes, &[]), None, "no vault anywhere");

        fs::create_dir_all(dir.join(".obsidian")).unwrap();
        assert_eq!(vault_root_in(&notes, &[]), Some(canon(&dir)), "a vault above the notes folder");
        fs::create_dir_all(notes.join(".obsidian")).unwrap();
        assert_eq!(vault_root_in(&notes, &[]), Some(canon(&notes)), "the notes folder itself, nearest first");
        assert_eq!(vault_root_in(&notes.join("NPCs"), &[]), Some(canon(&notes)), "from a subfolder");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn vault_from_the_registry() {
        let dir = temp_dir("registry");
        let notes = dir.join("Vault/Lorekeeper");
        fs::create_dir_all(&notes).unwrap();
        let real = canon(&dir);

        // Registered as the canonical path, asked for through the symlinked one (macOS /var -> /private/var).
        let reg = registry(&[Path::new("/elsewhere"), &real.join("Vault")]);
        assert_eq!(vault_root_in(&notes, &[reg]), Some(real.join("Vault")));
        // The innermost vault wins; the notes folder itself counts.
        let reg = registry(&[&dir, &notes]);
        assert_eq!(vault_root_in(&notes, &[reg]), Some(real.join("Vault/Lorekeeper")));
        // A vault whose name only starts the same, or below the notes folder, doesn't contain it.
        let reg = registry(&[&dir.join("Vault/Lore"), &notes.join("NPCs")]);
        assert_eq!(vault_root_in(&notes, &[reg]), None);
        // Broken or odd registries are ignored, and a good one alongside still counts.
        let bad = ["{oops", "", "null", r#"{"vaults": []}"#, r#"{"vaults": {"a": {"path": 3}, "b": {}}}"#].map(String::from);
        assert_eq!(vault_root_in(&notes, &bad), None);
        let mut regs = bad.to_vec();
        regs.push(registry(&[&dir]));
        assert_eq!(vault_root_in(&notes, &regs), Some(real));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_uri_encodes_spaces_as_percent_20() {
        assert_eq!(open_uri(Path::new("/v/Sessions/Session 3.md")), "obsidian://open?path=%2Fv%2FSessions%2FSession%203.md");
        assert_eq!(open_uri(Path::new("/v/C++ & D&D.md")), "obsidian://open?path=%2Fv%2FC%2B%2B%20%26%20D%26D.md");
    }

    #[test]
    fn opened_json_shape() {
        use serde_json::json;
        assert_eq!(serde_json::to_value(Opened::Opened { opened: true }).unwrap(), json!({"opened": true}));
        let needs = Opened::NeedsVault { needs_vault: true, path: "/n".into(), installed: false };
        assert_eq!(serde_json::to_value(needs).unwrap(), json!({"needsVault": true, "path": "/n", "installed": false}));
    }
}
