# Development

Tauri v2 app: Rust in `src-tauri/`, plain HTML/JS/CSS in `src/` (no bundler). Node and pnpm are only build tools; nothing from `node_modules` ships.

```bash
pnpm install
pnpm tauri dev     # run
pnpm test          # node tests + cargo test
pnpm vendor        # rebuild src/vendor (marked, CodeMirror) after changing editor-src/ or those versions
```

Local release builds need the update signing key, or skip the update files:

```bash
pnpm tauri build --bundles app --config '{"bundle":{"createUpdaterArtifacts":false}}'
```

## CI and releases

- `.github/workflows/ci.yml` runs `pnpm test` on pushes to `main` and on pull requests.
- `.github/workflows/release.yml` runs on `v*` tags and creates a **draft** release: universal `.dmg`, Windows `-setup.exe`, Linux `.AppImage`, plus the updater files (`latest.json`, `.app.tar.gz`, `.sig`).

To release:

1. Bump `version` in `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` and `package.json`.
2. Update `RELEASE_NOTES.md`. The update dialog shows its first ~400 characters, so keep "What's new" first.
3. Commit, then `git tag vX.Y.Z && git push origin vX.Y.Z`.
4. In the draft, check that `latest.json` lists `darwin-aarch64`, `darwin-x86_64`, `windows-x86_64` and `linux-x86_64` (re-run a job if one is missing), then publish. Installed apps only see published releases.

## Update signing key

The private key is `~/.tauri/lorekeeper-updater.key` (password in `~/.tauri/lorekeeper-updater.key.password`), stored as the Actions secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. The public key is in `tauri.conf.json` under `plugins.updater.pubkey`.

Back it up and never commit it. If it's lost or leaks, a new key means everyone has to download the next version by hand.

## GitHub backup

Needs a GitHub OAuth app with **Enable Device Flow**; its client ID goes in `GITHUB_CLIENT_ID` in `src-tauri/src/github.rs`. Without it, Settings says GitHub backup isn't set up.

## Patched dependencies

`src-tauri/patches/glib-0.18.5` carries a backported security fix; see [its README](../src-tauri/patches/README.md).

## Manual test checklist

- Accessibility permission on first `⌘⇧S` (a rebuilt unsigned app can lose it).
- Note box over Discord: Enter saves and focus returns.
- Save selection keeps your previous clipboard text; with nothing selected it saves the clipboard.
- "Saved" notification (test in a built app, not `tauri dev`).
- Pasting into D&D Beyond keeps headings and bullets.
- Launch at login survives a restart; the tray item and Settings stay in step.
- Backup folder gets today's copy right away; a removed USB drive gives one failure notification.
- GitHub backup (needs the client ID): sign in, commit on backup, deleted note disappears from the repo.
- Obsidian button: guide before the folder is a vault, opens the page after.
- Settings: new hotkey works at once; a taken one is refused; theme applies to every window.
