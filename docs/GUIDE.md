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
| `⌘⌥N` / `Ctrl+Alt+N` | One-line note box. Enter saves, Esc cancels. `↑` in the empty box brings back the last note to fix. |
| `⌘⇧S` / `Ctrl+Shift+S` | Saves the selected text, or the clipboard if nothing is selected. |
| off until you set it | New session, without opening a window. |
| off until you set it | New page, opens the New page dialog. |

Change them in **Settings > Hotkeys**. The save-selection shortcut must include `⌘` / `Ctrl`.

The note box, and on macOS **Settings…** and **Open Lorekeeper…** from the menu bar icon, take you back to the app you were in (Discord, say) when you close them.

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
- **+ New page** (`⌘N`): pick what you're making (NPC, PC, Location, Item, Faction, Quest, Lore or a plain note) and name it. It goes into the right folder with the right starting text. Clicking a `[[link]]` to a page that doesn't exist yet does the same.
- **Quests** have a `status`: change it to `done` or `failed` when the party finishes one.
- **Edit** (`⌘E`) is a live-preview Markdown editor. Your text is never reformatted, and hotkey notes that arrive while you type are kept.
- **Rename** (`F2`) changes a page's name, and every `[[link]]` to it, in sessions and pages alike, follows the new name. **Edit > Undo** (`⌘Z`) renames it back and puts the links back too.
- **Images** (maps, handouts): paste or drop one into the editor. It's saved in the notes folder's `Attachments/` and embedded as `![[Pasted image 20261005143012.png]]`, the way Obsidian does it; the page shows the image. Add `|300` for a width (`![[map.png|300]]`); `![alt](Maps/map.png)` works too. PNG, JPG, GIF, WebP and SVG, up to 20 MB. **Copy for D&D Beyond** leaves images out: upload them there yourself.
- **Linked from** at the bottom of every page lists the sessions and pages that mention it.
- `⌘K` searches, `⌘,` opens Settings, `⌘+` / `⌘−` zoom.

## Your notes

Notes are plain Markdown in `Documents/Lorekeeper` (change it in **Settings > General**):

```
Sessions/  PCs/  NPCs/  Locations/  Items/  Factions/  Quests/  Lore/  Templates/
```

`Lore/` is for anything that isn't a person, place or thing: gods, history, legends.

`Templates/` holds the starting text for new pages (Obsidian's `{{title}}` and `{{date}}` work). Edit them in Obsidian or any editor. A template of your own, say `Templates/Monster.md`, adds **Monster** to **+ New page**; its pages go into a `Monsters` or `Monster` folder if you make one.

**Obsidian:** open the notes folder as a vault (or put it inside an existing vault) and the **Obsidian** button opens pages there. Until then the button shows how.

## Campaigns

Running more than one game? Give each campaign its own notes folder: **Settings > General > Add campaign…**. Switch with the campaign name at the top of the sidebar or the tray's **Campaign** menu. The window, the hotkey notes and the backups all follow the campaign you switch to. **Remove** only forgets a campaign in Lorekeeper; its notes folder and backups stay.

Each campaign backs up on its own, so switching never overwrites or deletes another campaign's backup. Every campaign, your first one included, is a folder named after the campaign. A campaign is called after its notes folder unless you give it a name (say `Strahd`). All your campaigns share one place per backup, each in its own folder:

- **Folder backup:** a `Strahd` folder inside your backup folder.
- **Dropbox:** `Apps/Lorekeeper/Strahd`.
- **Google Drive:** a `Strahd` folder inside the `Lorekeeper` folder in My Drive.
- **GitHub:** a `Strahd` folder in your private `lorekeeper-notes` repository.

Backups made by older versions of Lorekeeper (in `Apps/Lorekeeper/Campaigns`, `Lorekeeper - <campaign>` folders or `lorekeeper-notes-<campaign>` repositories) stay where they are; new backups go to the places above.

Name a campaign next to it in **Settings > General**; the name shows in the sidebar, on Home and in the tray. Each campaign needs its own name, since the names keep their backups apart. A new name starts a new backup: the old one stays where it was, and changing the name back carries it on. A campaign you remove keeps its name, so adding the folder again carries on its backups; so does a folder you add later with the name of a removed campaign, which is what you want after moving a folder (otherwise give it another name).

## Backups

**Settings > Backups**, any of these:

- **Back up to a folder:** pick a folder in Google Drive, Dropbox, OneDrive, iCloud Drive or on a USB drive. Lorekeeper keeps a dated copy for each of the last 30 days.
- **Dropbox or Google Drive:** click **Sign in**, then sign in and allow access in your browser (your password never goes through Lorekeeper). Lorekeeper only sees its own folders: **Apps/Lorekeeper** in Dropbox, and the **Lorekeeper** folder it made in Google Drive, with a folder for each campaign in both. Only new and changed notes are uploaded, and a note you delete is deleted there too. A deleted or overwritten note can be brought back from Dropbox's **Deleted files** or a file's **Version history**, or the Google Drive **Trash** (30 days or more).
- **Back up to GitHub:** sign in with a code (no password in Lorekeeper). Every backup is a version in one private repository, `lorekeeper-notes`, with a folder for each campaign; a campaign's backup only ever changes its own folder. A file's **History** on github.com shows every older version.

**To restore**, click **Restore…** next to a backup (it restores the campaign that's open), pick a day (folder), a version (GitHub) or the current backup (Dropbox, Google Drive), and choose where the restored notes go. They're downloaded into a new folder (by default `<campaign> restored <date>` next to your notes folder); your notes folder and the backup are never changed. Then **Show in Finder** (Explorer on Windows) to copy back what you need, or **Open as a new campaign** to switch to it. It then backs up on its own like any other campaign, so it never touches the backup it came from. You can also restore by hand: copy from a dated folder, download from the Dropbox or Google Drive website, or use **Code > Download ZIP** on GitHub (the campaign is its own folder in it).

To look at a backup of the campaign that's open: **Show in Finder** (Explorer on Windows) for the backup folder, or **Open in Dropbox**, **Open in Google Drive** or **Open on GitHub**, which open it in your browser.

Backups run on each new session, every 30 minutes while notes change, once a day, and on **Back up now**. Keeping the notes folder itself in a cloud drive syncs it, but sync isn't a backup: deletions sync too.

## Settings

- **General:** campaigns (notes folders), launch at login, "saved" notifications, updates.
- **Hotkeys:** record new shortcuts; a shortcut another app already uses is refused and the old one keeps working.
- **Appearance:** Light (Tome), Dark (Dungeon) or System, editor font size, Timeline or Journal by default.
- **Backups:** see above.

## Limits

- Moving pages to other folders and the graph view: use Obsidian or your file manager.
- When saving a selection, the previous clipboard is restored as text only.
