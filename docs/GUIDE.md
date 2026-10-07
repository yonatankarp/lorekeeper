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

Change them in **Settings > Shortcuts**. The save-selection shortcut must include `⌘` / `Ctrl`.

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

- **Sessions** open as a **Timeline** (every note with its time). **Journal** shows a recap of the session grouped by kind: what happened, NPCs, loot, quests, mysteries and quotes. Open quests are listed at the top.
- **+ New page** (`⌘N`): pick what you're making (NPC, PC, Location, Item, Faction, Quest, Lore or a plain note) and name it. It goes into the right folder with the right starting text. Clicking a `[[link]]` to a page that doesn't exist yet does the same.
- **Quests** have a `status`: change it to `done` or `failed` when the party finishes one.
- **Edit** (`⌘E`) is a live-preview Markdown editor. Your text is never reformatted, and hotkey notes that arrive while you type are kept.
- **Rename** (`F2`) changes a page's name, and every `[[link]]` to it, in sessions and pages alike, follows the new name. **Edit > Undo** (`⌘Z`) renames it back and puts the links back too.
- **Move** a page to another folder: drag it in the sidebar onto a folder (onto the list outside any folder for the top level), or use **Move** (**File > Move to…**, or right-click it) and pick the **Folder**. The page stays open, `[[Name]]` links keep working, and a link with a folder in it (`[[NPCs/Vex]]`) follows the page. Links to another page with the same name get its folder, so they keep pointing there. A page never replaces one with the same name, sessions stay in `Sessions/` (and only sessions go there), and templates stay in `Templates/`. A moved page's icon follows its new folder unless it has a `type` property. **Edit > Undo** moves it back.
- **Right-click** a page in the sidebar for **Open**, **Open in Obsidian**, **Show Vault in Finder** (**Show Vault in Explorer** on Windows, **Open Vault Folder** on Linux), **Copy Link** (`[[Name]]`), **Rename…**, **Move to…** and **Delete…**, or a folder for **New Page Here…** (**New Session** on `Sessions/`). Sessions keep their "Session N" names and stay in `Sessions/`, so they don't offer **Rename…** or **Move to…**, and a shared session holds everyone's notes, so it doesn't offer **Delete…** either. In the editor and text fields, right-click gives the usual **Cut**, **Copy** and **Paste**.
- **Images** (maps, handouts): paste or drop one into the editor. It's saved in the notes folder's `Attachments/` and embedded as `![[Pasted image 20261005143012.png]]`, the way Obsidian does it; the page shows the image. Add `|300` for a width (`![[map.png|300]]`); `![alt](Maps/map.png)` works too. PNG, JPG, GIF, WebP and SVG, up to 20 MB.
- **Linked from** at the bottom of every page lists the sessions and pages that mention it.
- `⌘K` searches, `⌘,` opens Settings, `⌘+` / `⌘−` zoom.

## Characters from D&D Beyond

PC pages can come straight from D&D Beyond character sheets:

1. **Sign in** (once): **Settings > General > D&D Beyond > Sign in…** opens D&D Beyond's own sign-in page. Your password goes to D&D Beyond, never to Lorekeeper. Google sign-in is often refused in app windows like this one; use a Wizards, Apple or Twitch login instead. Public characters work without signing in; campaigns and "Campaign Only" characters need it.
2. **Import:** **+ New page**, choose **PC**, then **Import from D&D Beyond…**. Paste a character link (`dndbeyond.com/characters/…`) or a campaign link (`dndbeyond.com/campaigns/…`) and click **Look up**. A character brings the rest of its campaign's party along. Tick the characters you want and click **Import**. Each one updates its page in `PCs/` (the one linking to its sheet, else the one with its name) or gets a new page from the PC template.
3. **Refresh:** on a PC page, **Refresh** next to **Character sheet** reads the character again, say after a level up. **Edit > Undo** puts the page back, after an import too.

What D&D Beyond updates: `race`, `class` (with subclass), `level`, `background`, `alignment` and the `dndbeyond` sheet link. `player` and the `portrait` (saved into `Attachments/`) are only filled in when empty, so a player's real name you typed stays. **Appearance** and **Personality** are written from the sheet only while those sections are empty. Nothing else changes: your other properties and notes are never touched.

## Your notes

Notes are plain Markdown. `Documents/Lorekeeper` holds your campaigns, one folder each, and the templates they share:

```
Documents/Lorekeeper/
  Templates/
  Scale & Frost/   Sessions/  PCs/  NPCs/  Locations/  Items/  Factions/  Quests/  Lore/  Attachments/
```

`Lore/` is for anything that isn't a person, place or thing: gods, history, legends.

`Templates/` holds the starting text for new pages (Obsidian's `{{title}}` and `{{date}}` work), shared by every campaign in `Documents/Lorekeeper` and backed up as its own `Templates` folder. Edit them in Obsidian or any editor. A campaign kept somewhere else uses its own `Templates/` if it has one. A template of your own, say `Templates/Monster.md`, adds **Monster** to **+ New page**; its pages go into a `Monsters` or `Monster` folder if you make one.

**Connections:** Home maps your NPCs, PCs, places and factions and the `[[links]]` between them; each of those pages shows the ones a link away. A page shows its `portrait`, else the first picture in it, else its kind's icon. Click a page to open it; pinch (Windows and Linux: Ctrl+scroll) to zoom, drag to move, **Fit** (or double-click) to see it all again. Sessions are left out, since they link to everyone.

**Obsidian:** open the notes folder as a vault (or put it inside an existing vault) and the **Obsidian** button opens pages there. Until then the button shows how.

## Campaigns

Running more than one game? Give each campaign its own notes folder: **Settings > General > Add campaign…**. Switch with the campaign name at the top of the sidebar or the tray's **Campaign** menu. The window, the hotkey notes and the backups all follow the campaign you switch to. **Remove** only forgets a campaign in Lorekeeper; its notes folder and backups stay.

Each campaign backs up on its own, so switching never overwrites or deletes another campaign's backup. Every campaign, your first one included, is a folder named after the campaign. A campaign is called after its notes folder unless you give it a name (say `Strahd`). All your campaigns share one place per backup, each in its own folder:

- **Folder backup:** a `Strahd` folder inside your backup folder.
- **Dropbox:** `Apps/Lorekeeper/Strahd`.
- **Google Drive:** a `Strahd` folder inside the `Lorekeeper` folder in My Drive.
- **GitHub:** a `Strahd` folder in your private `lorekeeper-notes` repository.

Backups made by older versions of Lorekeeper (in `Apps/Lorekeeper/Campaigns`, `Lorekeeper - <campaign>` folders or `lorekeeper-notes-<campaign>` repositories) stay where they are; new backups go to the places above.

Name a campaign under **Rename or remove** next to it in **Settings > General**; the name shows in the sidebar, on Home and in the tray. Each campaign needs its own name, since the names keep their backups apart. A new name starts a new backup: the old one stays where it was, and changing the name back carries it on. A campaign you remove keeps its name, so adding the folder again carries on its backups; so does a folder you add later with the name of a removed campaign, which is what you want after moving a folder (otherwise give it another name).

## Playing with your party

The whole party can keep one campaign together: everyone jots notes during the game, and every session shows all of them, each with who wrote it. Lorekeeper syncs a shared campaign between your computers through the Lorekeeper sync server at `lorekeeper.yonatankarp.com`, run by Lorekeeper's maintainer. There's nothing to install or sign up for. Notes are encrypted on your computer before they leave it, so the server stores only data it can't read.

**Sharing your campaign** (one person, say the DM):

1. Open the campaign, then in **Settings > General** click **Share with party…** next to it and pick your character under **I play** (characters are the pages in `PCs/`, so make or import them first; one person can import the whole party from D&D Beyond).
2. Click **Share**. The sync server asks for its creation key the first time: ask the maintainer for it. Lorekeeper keeps it in your system's password storage and never asks again. Everything in the campaign then uploads.
3. Click **Invite a player** and send the link to one player, privately (a message, not a public channel). Each link works once, for 7 days, and holds the campaign's key, so make one per player. The same dialog, **Party…** next to the campaign from then on, lists the pending invites so you can cancel one, and the players.

**Joining** (everyone else): in **Settings > General**, click **Join a shared campaign…**, paste your link, check which server it's for, then **Join**. Lorekeeper makes the campaign's folder in your Lorekeeper folder (named after the campaign), opens it and downloads everything. The **Party** dialog then opens: pick your character under **I play**. Opening the link in a browser only shows a page that says to paste it into Lorekeeper.

What changes in a shared campaign:

- Your quick notes go to a file of your own: `Sessions/Session 4/Sibling 5.md` for Sibling 5. Two players never write the same file, so taking notes together never conflicts. Until you pick your character, the note box refuses to save and keeps your note.
- A session shows everyone's notes as one timeline, in time order, each with its author (and their portrait, if their PC page has one). The Journal shows them grouped, with authors too. You can delete your own notes from the Timeline; **Edit** opens your own file.
- **New session** starts the next session for everyone. If someone already started one in the last 12 hours and you haven't written in it yet, New session joins that one instead, so a party pressing it together stays in one session. Simplest: let one person (the DM, say) start sessions, and everyone else just writes.
- `↑` in the note box and the "start a new session?" offer only look at your own notes.
- Sessions from before you turned sharing on stay as they were; the first note after it starts the next session.
- Everything else (NPCs, places, quests, your PC page, images) is shared as it is: anyone can edit any page. Templates stay your own, and so do files Lorekeeper doesn't use (only pages and images sync, up to about 22 MB each).
- The campaign that's open syncs continuously; another shared campaign catches up when you switch to it. The bottom of the sidebar and the campaign in Settings show the sync status (**Synced**, **Syncing 3…**, **Offline, will sync when the server is back**) and who else is online. Lorekeeper works offline as usual and catches up when the server answers again.
- A page someone else deletes goes to the campaign's hidden `.trash` folder on your computer, so you can bring it back.

It's not a live co-editor: two people changing the same page at the same time (say an NPC page) end up with two versions. Lorekeeper keeps yours and saves theirs next to it, named like `Mirela (conflict 2026-10-06 2015).md`, and lists it under **Sync conflicts** on Home. Open both, keep what you want in the original, then delete the copy. Your version is what the others get.

**Removing a player:** the owner opens **Party…** next to the campaign and clicks **Remove** (twice, to confirm). Their sync stops at once and Lorekeeper tells them they were removed; the notes already on their computer stay there. Removing a joined campaign from your own list (**Remove** next to it) stops its sync and deletes its token from your computer; the owner still sees you under Players until they remove you.

Keep the campaign folder where it is: Lorekeeper syncs one folder per campaign, and a folder it can't find doesn't sync until it's back. You can still keep the folder in a cloud drive or open it in Obsidian.

## Backups

**Settings > Backups**: pick one under **Back up to**. Setting up a new one turns off the one you had (what's already backed up there stays).

- **Back up to a folder:** pick a folder in Google Drive, Dropbox, OneDrive, iCloud Drive or on a USB drive. Lorekeeper keeps a dated copy for each of the last 30 days.
- **Dropbox or Google Drive:** click **Sign in**, then sign in and allow access in your browser (your password never goes through Lorekeeper). Lorekeeper only sees its own folders: **Apps/Lorekeeper** in Dropbox, and the **Lorekeeper** folder it made in Google Drive, with a folder for each campaign in both. Only new and changed notes are uploaded, and a note you delete is deleted there too. A deleted or overwritten note can be brought back from Dropbox's **Deleted files** or a file's **Version history**, or the Google Drive **Trash** (30 days or more).
- **Back up to GitHub:** sign in with a code (no password in Lorekeeper). Every backup is a version in one private repository, `lorekeeper-notes`, with a folder for each campaign; a campaign's backup only ever changes its own folder. A file's **History** on github.com shows every older version.

**To restore**, click **Restore…** next to a backup (it restores the campaign that's open), pick a day (folder), a version (GitHub) or the current backup (Dropbox, Google Drive), and choose where the restored notes go. They're downloaded into a new folder (by default `<campaign> restored <date>` next to your notes folder); your notes folder and the backup are never changed. Then **Show in Finder** (Explorer on Windows) to copy back what you need, or **Open as a new campaign** to switch to it. It then backs up on its own like any other campaign, so it never touches the backup it came from. You can also restore by hand: copy from a dated folder, download from the Dropbox or Google Drive website, or use **Code > Download ZIP** on GitHub (the campaign is its own folder in it).

To look at a backup of the campaign that's open: **Show in Finder** (Explorer on Windows) for the backup folder, or **Open in Dropbox**, **Open in Google Drive** or **Open on GitHub**, which open it in your browser.

Backups run on each new session, every 30 minutes while notes change, and once a day. Keeping the notes folder itself in a cloud drive syncs it, but sync isn't a backup: deletions sync too.

## Settings

- **General:** campaigns (notes folders), sharing them with your party, the D&D Beyond sign-in, launch at login, updates, and under **Advanced** the sync server.
- **Shortcuts:** change the global shortcuts; a shortcut another app already uses is refused and the old one keeps working.
- **Appearance:** Light (Tome), Dark (Dungeon) or Match my computer, and text size.
- **Backups:** see above.

## Limits

- When saving a selection, the previous clipboard is restored as text only.
