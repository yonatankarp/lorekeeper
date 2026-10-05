# Lorekeeper

Lorekeeper is a tray / menu bar app for taking D&D session notes from a hotkey, then pasting a tidy recap into the shared D&D Beyond journal.

## Hotkeys

| Shortcut | What it does |
|---|---|
| `Cmd+Option+N` (Windows: `Ctrl+Alt+N`) | Opens a one-line note box. Enter saves, Esc cancels |
| `Cmd/Ctrl+Shift+S` | Saves the **selected text**, or the **clipboard** if nothing is selected |
| *(off until you set one)* | **New session**: starts the next session file without opening a window |
| *(off until you set one)* | **New page**: opens Lorekeeper with the New page dialog |

Set the last two in **Settings > Hotkeys** (`⌃⌥⌘S` / `⌃⌥⌘P` on a Mac, `Ctrl+Alt+Shift+S` / `Ctrl+Alt+Shift+P` elsewhere, are usually free). Inside the window, `⌘N` / `Ctrl+N` and `⇧⌘N` / `Ctrl+Shift+N` do the same.

Start a note with a prefix to file it under a section:

| Prefix | Section |
|---|---|
| `@` | NPCs |
| `#` | Loot |
| `!` | Quests |
| `?` | Mysteries |
| `"` | Quotes |

Anything without a prefix goes under "What happened".

In the quick-note box, typing `@Mir` or `[[Mir` suggests existing page names; press Tab to turn it into a link (`@[[Mirela]]`), and ↑/↓ to pick another match. After Enter the box shows which session the note went to.

While the app runs, these shortcuts are taken over system-wide, so `Cmd/Ctrl+Shift+S` stops working as "Save As" in other apps. To change a shortcut, use **Settings** (below). The capture shortcut must include `Cmd` (Windows/Linux: `Ctrl`): it reuses that held key to send the copy command.

## The Lorekeeper window

Opening the app (from Applications, Spotlight or the tray's **Open Lorekeeper…**) shows the whole vault:

- **Sidebar:** every folder and page. `⌘K` searches names and text.
- **Pages:** rendered Markdown with clickable `[[links]]` and properties. **Edit** (`⌘E`) opens the editor (below); it saves as you type.
- **Linked from:** every page and session line that links to the open page.
- **+ New page** (`⌘N`): pick a template from `Templates/` and it goes to the matching folder (template "NPC" → `NPCs/`). Clicking a link to a page that doesn't exist yet opens this with the name filled in. Templates use Obsidian's `{{title}}` and `{{date}}`, so you can edit them, add your own, or delete them.
- **Sessions** (newest first) open as a **Timeline**: every note in order with its time and a badge for its kind. **Journal** shows them grouped exactly as **Copy for D&D Beyond** will paste them. **+ Session** starts the next one.
- Empty folders offer **+ New NPC**, **+ New Location** and so on. `⌘+` / `⌘−` zoom the window.
- If hotkey notes arrive while you're editing a session, they're kept. If a page changes in Obsidian while you have unsaved edits, the app asks which version to keep instead of overwriting.

Not in the app (use Obsidian or Finder): renaming, moving or deleting pages, images, graph view.

### Obsidian

The notes are plain Markdown, so Obsidian can open them. Once the notes folder is in an Obsidian vault (the folder itself or a folder inside a vault), the **Obsidian** button, the page's right-click menu and the tray's **Open in Obsidian** open the page or the current session there. Obsidian has no way for another app to add a vault, so until then:

- The **Obsidian** button (shown when Obsidian is installed) explains how: in Obsidian's vault switcher choose **Open folder as vault** and pick the notes folder. The guide shows the folder's path with a **Copy path** button.
- The tray's **Open in Obsidian** starts Obsidian and copies the folder's path, ready to paste. Without Obsidian it shows the folder in Finder or Explorer.

### Editor

Edit mode is a live-preview Markdown editor, like Obsidian's: headings, **bold**, links and lists look formatted, and the Markdown marks (`#`, `**`, `[[ ]]`) show only on the line you're on. The file's text is never reformatted, so pages stay exactly as Obsidian wrote them, and hotkey notes that arrive while you type appear at the end without moving your cursor.

- **Toolbar:** bold, italic, heading (cycles H1, H2, H3, none), bulleted list, checkbox, link, quote. `⌘B` / `⌘I` for bold and italic.
- **Links:** typing `[[` or `@` plus a few letters suggests page names (Enter or Tab inserts `[[Name]]`). `⌘`-click a link to open it; on other lines a plain click works.
- **Lists:** Enter continues a list or checklist (Enter on an empty item ends it), Tab / Shift+Tab indent. Click a checkbox to tick it.
- The editor font size is in **Settings**. The editor is CodeMirror 6, vendored as `src/vendor/codemirror.js`; after changing `editor-src/codemirror.js` or the `@codemirror/*` versions, run `pnpm vendor` and commit the result.

## Settings

Open **Settings…** from the tray menu (or `⌘,` in the Lorekeeper window). Changes apply right away.

- **Hotkeys:** press **Record**, then the new combination (Esc cancels). If another app already uses it, the old shortcut stays and the window says so.
- **Notes folder:** where notes are kept (default `Documents/Lorekeeper`). **Choose…** picks another folder and adds the standard folders and templates there. Existing notes are not moved; move them in Finder if you want them in the new folder. A folder inside your Obsidian vault or iCloud Drive works well.
- **Appearance:** theme (System, Light, Dark), editor font size, and whether sessions open as Timeline or Journal.
- **Backups:** back up to a folder, to GitHub, or both. See [Backups](#backups).
- **Startup & notifications:** Launch at login (same as the tray item), and whether a notification appears when a note is saved. Errors always show.
- **Updates:** the version you have, **Check now**, and whether to check automatically. See [Updates](#updates).

Settings are stored in `settings.json` in the app's config folder (macOS: `~/Library/Application Support/com.yonatankarp.dndnotes/`, Windows: `%APPDATA%\com.yonatankarp.dndnotes\`, Linux: `~/.config/com.yonatankarp.dndnotes/`). An older `Documents/Lorekeeper/settings.json` is moved there on first start. If the file can't be read, the app reports it and uses the defaults without overwriting it; changing a setting in the window replaces it.

## Backups

Turn backups on in **Settings > Backups**. You can use one option or both.

**What gets backed up:** everything in your notes folder (sessions, pages, templates, images), except hidden files like `.obsidian`, `.git` and `.DS_Store`. A backup runs when you start a new session, every 30 minutes if Lorekeeper changed a note, once a day anyway (so edits made in Obsidian are included), and when you press **Back up now**. If a backup fails you get one notification, and the reason shows in Settings.

### Back up to a folder

Press **Choose…** and pick a folder inside Google Drive, Dropbox, OneDrive or iCloud Drive (their desktop apps upload it for you), or on a USB drive. Each day gets its own copy, `Lorekeeper backup 2026-10-05`, and the last 30 days are kept. Older dated copies are deleted; nothing else in that folder is touched. The backup folder can't be inside your notes folder.

**To restore:** open the backup folder, find the day you want, and copy the notes you need back into your notes folder. To use a whole day's copy as your notes folder, first copy that dated folder somewhere else (for example to Documents), then pick the copy in **Settings > Notes folder > Choose…**. Lorekeeper won't use a dated folder in place, because backups replace and clean up those folders.

### Back up to GitHub

Every backup is saved as a version, so you can see or restore your notes from any point in time. You need a free [GitHub](https://github.com) account.

1. Press **Sign in with GitHub**. A short code appears.
2. Press **Copy code**, then **Open github.com**, paste the code and approve.
3. Lorekeeper creates a private repository called `lorekeeper-notes` in your account and backs up to it.

You never type your GitHub password into Lorekeeper. The sign-in is kept in your system's password storage (macOS Keychain, Windows Credential Manager, or the Secret Service keyring on Linux), not in a file. **Sign out** removes it. Files over 50 MB are skipped, and the status line lists them.

**To restore:** open the repository on github.com (the link is in Settings). **Code > Download ZIP** gives you all notes as they were at the last backup. To get an older version of one note, open the file and press **History**, pick a version, then view or download it.

### Or keep your notes folder in the cloud

You can also put the notes folder itself inside Google Drive, iCloud Drive, Dropbox or OneDrive (**Settings > Notes folder > Choose…**). Your notes are then synced to your other computers, and Obsidian on your phone can open them. Sync is not a backup, though: if a note is deleted or overwritten by mistake, the mistake syncs too. Use it together with one of the backups above.

## Updates

Lorekeeper 0.2.0 and later update themselves. About 30 seconds after it starts, and then once a day, it asks GitHub whether a newer version has been published. If there is one, a window shows the new version and asks **Install** or **Later**. **Install** downloads it, replaces the app and restarts it (on Windows the installer runs with a progress bar and reopens the app). Turn the automatic check off in **Settings > General > Updates**; **Check now** there, or **Check for Updates…** in the tray menu, checks right away and also tells you when you're up to date. Older versions (0.1.x) don't have this: download 0.2.0 once by hand.

How it works: each release includes a `latest.json` file and a signed copy of each download. The app reads `latest.json` from the newest **published** release (`releases/latest`), so drafts and unpublished builds are never offered. Before installing, it checks the download's signature against the public key in `src-tauri/tauri.conf.json` (`plugins > updater > pubkey`) and refuses anything not signed with the matching private key.

**For the maintainer:** the private signing key is `~/.tauri/lorekeeper-updater.key` (its password is in `~/.tauri/lorekeeper-updater.key.password`), and both are stored as the GitHub Actions secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Keep a backup of them somewhere safe and never commit them. If the key is lost, existing installs can't update any more: a new key means a new public key in the app, and everyone has to download that version by hand. If it leaks, others could sign updates for your users; replacing it (`pnpm tauri signer generate`) has the same cost.

Because the updater is set up, a local `pnpm tauri build` stops at the end asking for `TAURI_SIGNING_PRIVATE_KEY`. Either set `TAURI_SIGNING_PRIVATE_KEY` to the key file's path and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` to its password, or skip the update files with `pnpm tauri build --config '{"bundle":{"createUpdaterArtifacts":false}}'`.

## Develop

```bash
pnpm install
pnpm tauri dev      # run
pnpm test           # vault logic, export, saving and path-safety tests
pnpm tauri build    # .app/.dmg on macOS; .msi/.exe when run on Windows
```

The Windows installer must be built on a Windows machine. Install Rust, Node and pnpm there, then run `pnpm tauri build`. The Windows build hasn't been compiled or tested yet.

## CI and releases

- **CI** (`.github/workflows/ci.yml`) runs `pnpm test` on every push to `main` and every pull request.
- **Release** (`.github/workflows/release.yml`) builds installers when you push a version tag, and attaches them to a draft GitHub Release. One download per system:
  - macOS: universal `.dmg` (Apple Silicon + Intel)
  - Windows: setup `.exe`
  - Linux: `.AppImage`

  Next to them are the files the in-app updater uses: `latest.json`, the macOS `.app.tar.gz`, and a `.sig` signature for each update file. See [Updates](#updates).

To release, bump `version` in `src-tauri/tauri.conf.json` (and `package.json`), commit, then:

```bash
git tag v0.2.0 && git push origin v0.2.0
```

Review the draft release on GitHub and publish it. Installed apps are offered the update only once it's published. Before publishing, open `latest.json` in the draft and check it lists `darwin-aarch64`, `darwin-x86_64`, `windows-x86_64` and `linux-x86_64` (the three build jobs each add theirs, and two finishing at the same moment can drop one); if one is missing, re-run that job. The installers are unsigned for now, so macOS and Windows show a warning on first launch (the release notes explain how to get past it). On Linux, the global shortcuts need an X11 session; under Wayland use the window instead.

## Manual test checklist

These can't be tested automatically:

- [ ] macOS: on the first capture, grant **Accessibility** access in System Settings → Privacy & Security. In `tauri dev` the permission goes to your terminal app, not to Lorekeeper. A rebuilt unsigned `.app` can lose the permission; if capture saves the old clipboard instead of the selection, remove the app from the Accessibility list and add it again.
- [ ] The quick-note box appears over Discord or the VTT, Enter saves, and focus goes back to the app you were in.
- [ ] Selecting text in Discord and pressing the capture shortcut saves it. Your previous clipboard text comes back afterwards.
- [ ] With nothing selected, the capture shortcut saves the clipboard.
- [ ] A "Saved" notification appears. Allow notifications the first time; test this in the built app, not `tauri dev`.
- [ ] Neither shortcut clashes with Discord or the VTT. If one does, a "Shortcut unavailable" notification appears at launch.
- [ ] Pasting into the D&D Beyond journal keeps the headings and bullet points.
- [ ] Launch at Login (tray menu) survives a restart, and the tray item and the Settings checkbox stay in step.
- [ ] Backups: choosing a backup folder makes `Lorekeeper backup <today>` there right away; pulling out a USB drive used as the backup folder shows one failure notification.
- [ ] GitHub: with a real client ID in `src-tauri/src/github.rs`, sign in, approve on github.com, and check that the private `lorekeeper-notes` repo gets a commit; edit a note, press **Back up now**, and check for a second commit; delete a note and check it disappears from the repo.
- [ ] Obsidian: before the notes folder is a vault, the tray's **Open in Obsidian** starts Obsidian and copies the path, and the window's **Obsidian** button shows the guide. After **Open folder as vault**, both open the page directly (also for a notes folder inside an existing vault).
- [ ] Settings: recording a new hotkey works right away; a shortcut used by another app shows an error and the old one keeps working. Theme changes apply to every window.

## Known limits

- GitHub backup needs a GitHub OAuth app with **Enable Device Flow** ticked; put its client ID in `GITHUB_CLIENT_ID` in `src-tauri/src/github.rs`. Without it the Settings window says GitHub backup isn't set up.
- The first GitHub backup of a very large vault (thousands of files) can take several runs, because GitHub limits how many files can be uploaded per hour.
- When capturing, the app restores your previous clipboard as text only. An image on the clipboard is lost.
- Open quests are not carried over between sessions.
