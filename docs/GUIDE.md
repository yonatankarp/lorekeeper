# Lorekeeper guide

## Installing

Download from the [latest release](https://github.com/yonatankarp/lorekeeper/releases/latest). The builds aren't code-signed yet:

- **macOS:** open the `.dmg` and drag Lorekeeper to Applications. If macOS says it's damaged or can't be opened, right-click the app and choose **Open**, or run `xattr -dr com.apple.quarantine /Applications/Lorekeeper.app`.
- **Windows:** run the `-setup.exe`. If SmartScreen warns, choose **More info > Run anyway**.
- **Linux:** make the `.AppImage` executable and run it. The global shortcuts need an X11 session.

From version 0.2.0 on, Lorekeeper updates itself.

On macOS, allow **Accessibility** the first time you use `⌘⇧S` (System Settings > Privacy & Security), and **Documents** access when asked.

## During the game

| Shortcut | What it does |
|---|---|
| `⌘⌥N` / `Ctrl+Alt+N` | One-line note box. Enter saves, Esc cancels. |
| `⌘⇧S` / `Ctrl+Shift+S` | Saves the selected text, or the clipboard if nothing is selected. |
| off until you set it | New session, without opening a window. |
| off until you set it | New page, opens the New page dialog. |

Change them in **Settings > Hotkeys**. The save-selection shortcut must include `⌘` / `Ctrl`.

Start a note with a symbol to file it:

| Prefix | Goes to |
|---|---|
| `@` | NPCs |
| `#` | Loot |
| `!` | Quests |
| `?` | Mysteries |
| `"` | Quotes |
| none | What happened |

In the note box, `@Mir` or `[[Mir` suggests page names; Tab turns it into a link (`@[[Mirela]]`).

If the current session's last note is over 12 hours old, the box offers the next session: `⌘Enter` / `Ctrl+Enter` saves the note there, Enter still saves to the current one.

## After the game

- **Sessions** open as a **Timeline** (every note with its time). **Journal** shows them grouped the way **Copy for D&D Beyond** pastes them. Open quests are listed at the top.
- **+ New page** (`⌘N`): pick what you're making (NPC, PC, Location, Item, Faction, Quest or a plain note) and name it. It goes into the right folder with the right starting text. Clicking a `[[link]]` to a page that doesn't exist yet does the same.
- **Quests** have a `status`: change it to `done` or `failed` when the party finishes one.
- **Edit** (`⌘E`) is a live-preview Markdown editor. Your text is never reformatted, and hotkey notes that arrive while you type are kept.
- **Linked from** at the bottom of every page lists the sessions and pages that mention it.
- `⌘K` searches, `⌘,` opens Settings, `⌘+` / `⌘−` zoom.

## Your notes

Notes are plain Markdown in `Documents/Lorekeeper` (change it in **Settings > General**):

```
Sessions/  PCs/  NPCs/  Locations/  Items/  Factions/  Quests/  Templates/
```

`Templates/` holds the starting text for new pages (Obsidian's `{{title}}` and `{{date}}` work). Edit them in Obsidian or any editor.

**Obsidian:** open the notes folder as a vault (or put it inside an existing vault) and the **Obsidian** button opens pages there. Until then the button shows how.

## Backups

**Settings > Backups**, any of these:

- **Back up to a folder:** pick a folder in Google Drive, Dropbox, OneDrive, iCloud Drive or on a USB drive. Lorekeeper keeps a dated copy for each of the last 30 days. To restore, copy notes back from the day you want.
- **Dropbox or Google Drive:** click **Sign in**, then sign in and allow access in your browser (your password never goes through Lorekeeper). Lorekeeper only sees its own folder: **Apps/Lorekeeper** in Dropbox, **Lorekeeper** in Google Drive. Only new and changed notes are uploaded, and a note you delete is deleted there too. To restore, download the notes from that folder on the website. A deleted or overwritten note can be brought back from Dropbox's **Deleted files** or a file's **Version history**, or the Google Drive **Trash** (30 days or more).
- **Back up to GitHub:** sign in with a code (no password in Lorekeeper). Every backup is a version in a private `lorekeeper-notes` repository. To restore, use **Code > Download ZIP** or a file's **History** on github.com.

Backups run on each new session, every 30 minutes while notes change, once a day, and on **Back up now**. Keeping the notes folder itself in a cloud drive syncs it, but sync isn't a backup: deletions sync too.

## Settings

- **General:** notes folder, launch at login, "saved" notifications, updates.
- **Hotkeys:** record new shortcuts; a shortcut another app already uses is refused and the old one keeps working.
- **Appearance:** Light (Tome), Dark (Dungeon) or System, editor font size, Timeline or Journal by default.
- **Backups:** see above.

## Limits

- Renaming, moving and deleting pages, images and the graph view: use Obsidian or your file manager.
- When saving a selection, the previous clipboard is restored as text only.
