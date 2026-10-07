# Development

Tauri v2 app: Rust in `src-tauri/`, plain HTML/JS/CSS in `src/` (no bundler). Node and pnpm are only build tools; nothing from `node_modules` ships.

```bash
pnpm install
pnpm tauri dev     # run
pnpm test          # node tests + cargo test
pnpm vendor        # rebuild src/vendor (marked, CodeMirror) after changing editor-src/ or those versions
```

Google Drive sign-in also needs the Desktop client's secret at build time (Google requires it even with PKCE; for installed apps it isn't treated as confidential, but it's kept out of the source). Release builds get it from the Actions secret `LOREKEEPER_GOOGLE_CLIENT_SECRET`; for local builds keep it in `~/.tauri/lorekeeper-google-client-secret` and prefix the build with `LOREKEEPER_GOOGLE_CLIENT_SECRET="$(cat ~/.tauri/lorekeeper-google-client-secret)"`. Without it, Settings says Google Drive backup isn't set up.

Local release builds need the update signing key, or skip the update files:

```bash
pnpm tauri build --bundles app --config '{"bundle":{"createUpdaterArtifacts":false}}'
```

## CI and releases

- `.github/workflows/ci.yml` runs `pnpm test` on pushes to `main` and on pull requests.
- `.github/workflows/release.yml` runs on `v*` tags and creates a **draft** release: one job makes the draft, the platforms build in parallel into it, and a last job writes `latest.json` from their `.sig` files (`scripts/latest-json.mjs`), so builds can't make two drafts or drop each other's platforms. It builds a universal `.dmg`, Windows `-setup.exe`, Linux `.AppImage`, plus the updater files (`latest.json`, `.app.tar.gz`, `.sig`).
- `.github/workflows/sync-server.yml` tests `sync-protocol/` and `sync-server/` when they change, and on `main` publishes the server image to `ghcr.io/yonatankarp/lorekeeper-sync`.

## Sync server

Shared campaigns sync through `sync-server/` (axum, SQLite), with `sync-protocol/` holding what the app and the server share. Both are standalone crates outside `pnpm test`; [SYNC.md](SYNC.md) is the design and the API.

```bash
cargo test --manifest-path sync-protocol/Cargo.toml
cargo test --manifest-path sync-server/Cargo.toml
docker build -f sync-server/Dockerfile -t lorekeeper-sync .  # from the repository root
```

The app's side is `src-tauri/src/sync.rs` (the engine, no Tauri in it) and `src-tauri/src/shared.rs` (keychain, settings, commands, the open campaign's engine). `sync_tests.rs` runs two engines against the real server in-process (`pnpm test` includes it).

### Testing sync locally

Never point a test at the real server or your own profile. Run a server on loopback:

```bash
LOREKEEPER_SYNC_DB=/tmp/lk-sync.db LOREKEEPER_SYNC_ADDR=127.0.0.1:18081 \
  LOREKEEPER_SYNC_CREATE_KEY=local-test-create-key-0123456789abcdef \
  LOREKEEPER_SYNC_PUBLIC_URL=http://127.0.0.1:18081 \
  cargo run --manifest-path sync-server/Cargo.toml
# or: docker run --rm -p 127.0.0.1:18081:8080 -e LOREKEEPER_SYNC_CREATE_KEY=... -e LOREKEEPER_SYNC_PUBLIC_URL=http://127.0.0.1:18081 lorekeeper-sync
```

Then start two copies of the app, each with its own test profile:

```bash
cargo build --manifest-path src-tauri/Cargo.toml
LOREKEEPER_PROFILE=dm     LOREKEEPER_SYNC_SERVER=http://127.0.0.1:18081 src-tauri/target/debug/lorekeeper &
LOREKEEPER_PROFILE=player LOREKEEPER_SYNC_SERVER=http://127.0.0.1:18081 src-tauri/target/debug/lorekeeper &
```

`LOREKEEPER_PROFILE=<name>` (letters, digits, `-`, `_`) gives a copy its own settings and sync state (`<config>/profiles/<name>`), keychain entries (service `com.yonatankarp.dndnotes.profile.<name>`) and default notes folder (`~/Documents/Lorekeeper (<name>)`). It leaves launch at login alone, skips the automatic update check, puts the profile in the window titles and tray tooltip, and a hotkey the other copy already holds is only a notification. A test profile also skips the single-instance plugin (Windows and Linux; the real profile uses it so a second launch or a `lorekeeper://` link goes to the running copy), so two copies run side by side; run the binary directly (or `open -n` a built `.app`): macOS `open` brings back the copy that's running instead of starting another. `LOREKEEPER_SYNC_SERVER` replaces the default server (the Sync server setting replaces both). Share in one copy (creation key above), invite, and join in the other by pasting the link into **Join a shared campaign…**.

Test profiles ignore `lorekeeper://` invite links: two copies can't both own the URL scheme, and on macOS the system hands a link to whichever registered Lorekeeper bundle it picks (usually the installed one, the real profile), not to a binary run from `target/`. So don't click invite links, or `open lorekeeper://...`, while testing: that would start or wake the real profile. The link handling is covered by unit tests (`deeplink.rs`) and the join page by the server's tests. To try the real thing, use a built `.app` in a throwaway macOS user, or a Windows/Linux VM. `tauri dev` doesn't register the scheme on macOS (only a bundle's Info.plist does); on Linux the real profile registers itself at start (`register_all`), best effort.

Clean up afterwards: delete the profiles' notes folders, `<config>/profiles/`, and their keychain entries (`security delete-generic-password -s com.yonatankarp.dndnotes.profile.dm -a "sync-room <room>"` and so on, or Keychain Access).

To release:

1. Bump `version` in `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` and `package.json`.
2. Update `RELEASE_NOTES.md`. The update dialog shows its first ~400 characters, so keep "What's new" first.
3. Commit, then `git tag vX.Y.Z && git push origin vX.Y.Z`.
4. Check there's exactly one draft for the tag, holding the `.dmg`, `.app.tar.gz`, `-setup.exe` and `.AppImage` (each with its `.sig`), and that its `latest.json` lists `darwin-aarch64`, `darwin-x86_64`, `windows-x86_64` and `linux-x86_64` (re-run a job if one is missing), then publish. Installed apps only see published releases.

## Update signing key

The private key is `~/.tauri/lorekeeper-updater.key` (password in `~/.tauri/lorekeeper-updater.key.password`), stored as the Actions secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. The public key is in `tauri.conf.json` under `plugins.updater.pubkey`.

Back it up and never commit it. If it's lost or leaks, a new key means everyone has to download the next version by hand.

## Windows code signing

Off until its secrets exist; until then the Windows installer ships unsigned and SmartScreen says "Unknown publisher". Uses an SSL.com IV (individual) code signing certificate with eSigner cloud signing (Azure Artifact Signing only takes individuals in the US or Canada). Signing happens inside `tauri build` via `bundle.windows.signCommand` (jsign), so the update `.sig` matches the signed installer. Never sign the `-setup.exe` after the build: that breaks Windows updates.

1. Buy an [IV code signing certificate](https://www.ssl.com/products/software-integrity/code-signing/iv/) with **eSigner** storage and an eSigner plan; pass the ID check.
2. Enroll the certificate in eSigner and save the **TOTP secret code** shown during enrollment (the base64 secret, not the PIN or just the QR code). Note the certificate's **credential ID**.
3. Add Actions secrets: `ESIGNER_USERNAME`, `ESIGNER_PASSWORD`, `ESIGNER_CREDENTIAL_ID`, `ESIGNER_TOTP_SECRET`. Delete `ESIGNER_CREDENTIAL_ID` to turn signing off again.

Each release uses about 8 signatures (app, 5 NSIS plugins, uninstaller, installer), and so does every re-run of the Windows job. Check: Properties > **Digital Signatures** on the `-setup.exe` and the installed `Lorekeeper.exe`, or `signtool verify /pa /v Lorekeeper_X.Y.Z_x64-setup.exe`. SmartScreen can still warn for new releases until downloads build reputation; the warning then names the publisher.

## GitHub backup

Needs a GitHub OAuth app with **Enable Device Flow**; its client ID goes in `GITHUB_CLIENT_ID` in `src-tauri/src/github.rs`. Without it, Settings says GitHub backup isn't set up.

## Cloud backup apps

Dropbox and Google Drive backups each need an app registered by the owner. Only its public client ID goes in the code: the constants `DROPBOX_APP_KEY` and `GOOGLE_CLIENT_ID` at the top of `src-tauri/src/cloud.rs`. Never add a client secret. While a constant is empty, Settings says that backup isn't set up. Sign-in uses OAuth with PKCE in the system browser and a one-request web server on the user's computer for the redirect.

**Dropbox** ([App Console](https://www.dropbox.com/developers/apps))

1. **Create app** > **Scoped access** > **App folder**. Name it `Lorekeeper` (Dropbox names the app folder after the app; if the name is taken, pick another and change "Apps/Lorekeeper" in `src/settings.js` and `docs/GUIDE.md`).
2. **Settings** tab > **OAuth 2**: **Allow public clients (Implicit Grant & PKCE)**: Allow. **Redirect URIs**: add exactly `http://localhost:47219/` (Dropbox matches the port exactly; it is `DROPBOX_PORT` in `cloud.rs`).
3. **Permissions** tab: tick `files.content.write` (and `account_info.read`, on by default), then **Submit**.
4. Copy the **App key** into `DROPBOX_APP_KEY`.
5. A new app is in Development and only works for a limited number of users: use **Apply for production** on the Settings tab before releasing.

**Google Drive** ([Google Cloud console](https://console.cloud.google.com/))

1. Create a project, then **APIs & Services > Library > Google Drive API > Enable**.
2. **Google Auth Platform** (the OAuth consent screen): **Branding**: app name `Lorekeeper` and a support email. **Audience**: External. **Data Access > Add or remove scopes**: `https://www.googleapis.com/auth/drive.file` only. Google lists it as non-sensitive, so no security review is needed (Google may still ask to verify the app name and logo).
3. **Clients > Create client**: application type **Desktop app**. Desktop clients have no redirect URI field: Google allows `http://127.0.0.1:<any port>/`. Copy the **Client ID** into `GOOGLE_CLIENT_ID`; leave the client secret out of the code.
4. **Audience > Publish app** so the status is **In production**. In Testing, only listed test users can sign in and their sign-in expires after 7 days.

**Registered apps (owner: yonvata@gmail.com):** GitHub OAuth app "Lorekeeper" (device flow, tokens don't expire); Dropbox app "Lorekeeper" (development status, up to 500 users; **Apply for production** before passing that); Google Cloud project `lorekeeper-510711`, consent screen **In production** with `drive.file` only. The consent screen links to the home page and privacy policy, https://yonatankarp.com/lorekeeper/ and https://yonatankarp.com/lorekeeper/PRIVACY.html (see "Website" below); keep those URLs working. Keep the policy accurate when backups change, and don't add a logo to the consent screen unless you want to go through Google's brand verification.

Test each with a real account: sign in, first backup, a changed note uploads, a deleted note goes to the provider's trash, sign out and in again (no second upload of everything).

## Website

https://yonatankarp.com/lorekeeper/ is built by `scripts/build-site.mjs` (no dependencies): a home page designed in the script, plus `docs/GUIDE.md` and `docs/PRIVACY.md` rendered to `GUIDE.html` and `PRIVACY.html`, in the app's fonts, icons and themes (`scripts/site.css` on top of the tokens in `src/styles.css`). `.github/workflows/pages.yml` deploys it on pushes to main (Settings > Pages > Source: GitHub Actions). Preview it locally:

```bash
pnpm site                                  # writes site/ (git-ignored)
python3 -m http.server 8000 -d site        # then open http://localhost:8000
```

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
- Dropbox and Google Drive backups: see "Cloud backup apps" above. Also Cancel during sign-in then Sign in again right away, and Dropbox with port 47219 taken.
- Settings: new hotkey works at once; a taken one is refused; theme applies to every window.
- Shared campaign (two test profiles, see "Testing sync locally"): Share with party… then Share asks for the creation key once; Invite a player and Copy; Join shows the server and downloads into a new folder; I play; notes from each side appear on the other; the same page changed on both makes one conflict copy; quitting one copy and editing, then starting it again, catches up; Remove stops the player's sync and says so.
