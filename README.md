# Lorekeeper

Lorekeeper is a tray / menu bar app for taking D&D session notes from a hotkey, then pasting a tidy recap into the shared D&D Beyond journal.

## Hotkeys

| Shortcut | What it does |
|---|---|
| `Cmd+Option+N` (Windows: `Ctrl+Alt+N`) | Opens a one-line note box. Enter saves, Esc cancels |
| `Cmd/Ctrl+Shift+S` | Saves the **selected text**, or the **clipboard** if nothing is selected |

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
- **Startup & notifications:** Launch at login (same as the tray item), and whether a notification appears when a note is saved. Errors always show.

Settings are stored in `settings.json` in the app's config folder (macOS: `~/Library/Application Support/com.yonatankarp.dndnotes/`, Windows: `%APPDATA%\com.yonatankarp.dndnotes\`, Linux: `~/.config/com.yonatankarp.dndnotes/`). An older `Documents/Lorekeeper/settings.json` is moved there on first start. If the file can't be read, the app reports it and uses the defaults without overwriting it; changing a setting in the window replaces it.

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
- **Release** (`.github/workflows/release.yml`) builds installers when you push a version tag, and attaches them to a draft GitHub Release:
  - macOS: universal `.dmg` (Apple Silicon + Intel)
  - Windows: `.msi` and setup `.exe`
  - Linux: `.AppImage`, `.deb`, `.rpm`

To release, bump `version` in `src-tauri/tauri.conf.json` (and `package.json`), commit, then:

```bash
git tag v0.2.0 && git push origin v0.2.0
```

Review the draft release on GitHub and publish it. The installers are unsigned for now, so macOS and Windows show a warning on first launch (the release notes explain how to get past it). On Linux, the global shortcuts need an X11 session; under Wayland use the window instead.

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
- [ ] Settings: recording a new hotkey works right away; a shortcut used by another app shows an error and the old one keeps working. Theme changes apply to every window.

## Known limits

- When capturing, the app restores your previous clipboard as text only. An image on the clipboard is lost.
- Open quests are not carried over between sessions.
