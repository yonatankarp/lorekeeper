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

Change them in **Settings > Shortcuts** with the pencil next to each (**Change**). The save-selection shortcut must include `⌘` / `Ctrl`.

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

Every symbol, link and shortcut is on one page: **Help > Cheat Sheet** (`⌘/` / `Ctrl+/`), or the **Cheat sheet** button on an empty session. The same list is in [Cheat sheet](#cheat-sheet) below.

If the current session's last note is over 12 hours old, the box offers the next session: `⌘Enter` / `Ctrl+Enter` saves the note there, Enter still saves to the current one.

## After the game

- **Sessions** open as a **Timeline** (every note with its time). **Journal** shows a recap of the session grouped by kind: what happened, NPCs, loot, quests, mysteries and quotes. Open quests are listed at the top.
- **New page** (bottom of the sidebar, `⌘N` or `Ctrl+N`): pick what you're making (NPC, PC, Location, Item, Faction, Quest, Lore or a plain note) and name it. It goes into the right folder with the right starting text. Clicking a `[[link]]` to a page that doesn't exist yet does the same.
- **Quests** have a **Status** dropdown at the top of the page: **open**, **in progress**, **done** or **failed**. Open and in-progress quests are listed on **Home** with their status, and at the top of every session; done and failed ones get a check or a cross in the sidebar. **Edit > Undo** (`⌘Z` or `Ctrl+Z`) puts the old status back.
- **NPCs** have a **Status** dropdown (**alive**, **dead**, **missing** or **unknown**) and an **Attitude** dropdown (**friendly**, **neutral**, **hostile** or **unknown**) at the top of the page. A page that doesn't say yet shows **unknown**. Friendly NPCs are marked in green and hostile ones in red, and dead ones get a cross in the sidebar. **Edit > Undo** (`⌘Z` or `Ctrl+Z`) puts the old value back.
- A quest's `reward` shows coins with their metal's icon: write amounts like `25gp`, `3 sp` or `120 cp` (`pp`, `gp`, `ep`, `sp` and `cp`).
- **Edit** (the pencil at the top right, `⌘E` or `Ctrl+E`; **Done**, the check mark, goes back) is a live-preview Markdown editor. Your text is never reformatted, and hotkey notes that arrive while you type are kept.
- **Rename** (`F2`, **File > Rename…**, or right-click the page) changes a page's name, and every `[[link]]` to it, in sessions and pages alike, follows the new name. **Edit > Undo** (`⌘Z`) renames it back and puts the links back too.
- **Move** a page to another folder: drag it in the sidebar onto a folder (onto the list outside any folder for the top level), or use **File > Move to…** (or right-click it) and pick the **Folder**. The page stays open, `[[Name]]` links keep working, and a link with a folder in it (`[[NPCs/Vex]]`) follows the page. Links to another page with the same name get its folder, so they keep pointing there. A page never replaces one with the same name, sessions stay in `Sessions/` (and only sessions go there), and templates stay in `Templates/`. A moved page's icon follows its new folder unless it has a `type` property. **Edit > Undo** moves it back.
- **Right-click** a page in the sidebar for **Open**, **Show Vault in Finder** (**Show Vault in Explorer** on Windows, **Open Vault Folder** on Linux), **Copy Link** (`[[Name]]`), **Rename…**, **Move to…** and **Delete…**, or a folder for **New Page Here…** (**New Session** on `Sessions/`). Sessions keep their "Session N" names and stay in `Sessions/`, so they don't offer **Rename…** or **Move to…**, and a shared session holds everyone's notes, so it doesn't offer **Delete…** either, unless every note in it is yours (see "Playing with your party"). In the editor and text fields, right-click gives the usual **Cut**, **Copy** and **Paste**.
- **Images** (maps, handouts): paste or drop one into the editor. It's saved in the notes folder's `Attachments/` and embedded as `![[Pasted image 20261005143012.png]]` (the same syntax Obsidian uses); the page shows the image. Add `|300` for a width (`![[map.png|300]]`); `![alt](Maps/map.png)` works too. PNG, JPG, GIF, WebP and SVG, up to 20 MB.
- **Linked from** at the bottom of every page lists the sessions and pages that mention it.
- `⌘K` searches, `⌘,` opens Settings, `⌘+` / `⌘−` zoom.

## Characters from D&D Beyond

PC pages can come straight from D&D Beyond character sheets:

1. **Sign in** (once): **Settings > General > D&D Beyond > Sign in…** opens D&D Beyond's own sign-in page. Your password goes to D&D Beyond, never to Lorekeeper. Google sign-in is often refused in app windows like this one; use a Wizards, Apple or Twitch login instead. Public characters work without signing in; campaigns and "Campaign Only" characters need it.
2. **Import:** **New page**, choose **PC**, then **Import from D&D Beyond…**. Paste a character link (`dndbeyond.com/characters/…`) or a campaign link (`dndbeyond.com/campaigns/…`) and click **Look up**. A character brings the rest of its campaign's party along. Tick the characters you want and click **Import**. Each one updates its page in `PCs/` (the one linking to its sheet, else the one with its name) or gets a new page from the PC template.
3. **Refresh:** on a PC page, **Refresh** (the circling arrow next to **Character sheet**) reads the character again, say after a level up. **Edit > Undo** puts the page back, after an import too.

What D&D Beyond updates: `race`, `class` (with subclass), `level`, `background`, `alignment` and the `dndbeyond` sheet link. `player` and the `portrait` (saved into `Attachments/`) are only filled in when empty, so a player's real name you typed stays. **Appearance** and **Personality** are written from the sheet only while those sections are empty. Nothing else changes: your other properties and notes are never touched.

## Your notes

Notes are plain Markdown. `Documents/Lorekeeper` holds your campaigns, one folder each, and the templates they share:

```
Documents/Lorekeeper/
  Templates/
  Scale & Frost/   Sessions/  PCs/  NPCs/  Locations/  Items/  Factions/  Quests/  Lore/  Attachments/
```

`Lore/` is for anything that isn't a person, place or thing: gods, history, legends.

`Templates/` holds the starting text for new pages (`{{title}}` and `{{date}}` work), shared by every campaign in `Documents/Lorekeeper` and backed up as its own `Templates` folder. Edit them in any text editor. A campaign kept somewhere else uses its own `Templates/` if it has one. A template of your own, say `Templates/Monster.md`, adds **Monster** to **New page**; its pages go into a `Monsters` or `Monster` folder if you make one.

**Connections:** Home maps your NPCs, PCs, places and factions and the `[[links]]` between them; each of those pages shows the ones a link away. A page shows its `portrait`, else the first picture in it, else its kind's icon. Click a page to open it; scroll or pinch over it to zoom, drag to move, **Fit** (the corners button, or double-click) to see it all again. Sessions are left out, since they link to everyone.

## Campaigns

**Your first campaign:** the first time you open Lorekeeper, the window offers two ways in. Type a **Campaign name** and click **Create campaign** to make its folder in `Documents/Lorekeeper`, or click **Join a shared campaign…** to join one a friend shares (see [Playing with your party](#playing-with-your-party)). Until then, the hotkeys say there's no campaign yet instead of saving a note.

Running more than one game? Add another in **Settings > General > Add campaign…**: type a **Name** and click **New campaign** to make a new folder for it in `Documents/Lorekeeper`, or click **Choose existing folder…** to use a folder you already have. A name can't use `/ \ : * ? " < > |` or start or end with a dot, and can't be one of the folders inside a campaign (`Templates`, `Sessions` and so on). Lorekeeper never uses a folder that's already there for a new campaign. Switch with the campaign name at the top of the sidebar or the tray's **Campaign** menu. The window, the hotkey notes and the backups all follow the campaign you switch to. **Remove from list** (under **Rename or remove**) only forgets a campaign in Lorekeeper; its notes folder and backups stay. **Delete campaign**, next to it, asks first, then moves the campaign's folder, with every note in it, to the Trash (the Recycle Bin on Windows), where you can still get it back, and takes it off the list; backups stay. A shared campaign stops syncing on this computer first. The open campaign can't be removed or deleted: it says **Switch to another campaign to remove this one** instead. Lorekeeper won't delete the Lorekeeper folder itself (where older versions kept the first campaign), its Templates, or a folder that holds another campaign or your backup folder: remove those from the list instead. A shared campaign never syncs the folder of another campaign inside it (the first campaign of an older version, kept in the Lorekeeper folder itself, holds every campaign made or joined since), even a shared one you removed from the list.

Each campaign backs up on its own, so switching never overwrites or deletes another campaign's backup. Every campaign, your first one included, is a folder named after the campaign. A campaign is called after its notes folder unless you give it a name (say `Strahd`). All your campaigns share one place per backup, each in its own folder:

- **Folder backup:** a `Strahd` folder inside your backup folder.
- **Dropbox:** `Apps/Lorekeeper/Strahd`.
- **Google Drive:** a `Strahd` folder inside the `Lorekeeper` folder in My Drive.
- **GitHub:** a `Strahd` folder in your private `lorekeeper-notes` repository.

**Notes from older versions:** older versions kept your first campaign right in `Documents/Lorekeeper`, next to `Templates/`. The window then shows an offer, once: check the **Folder name** (your campaign's name if you gave it one, else `My campaign`) and click **Move notes** to move the campaign into `Documents/Lorekeeper/<Folder name>/`. `Templates/`, your other campaigns and Obsidian's settings stay where they are, nothing is overwritten (the folder must be new), and if anything can't be moved, what moved goes back. Its backups carry on under the same name, so a campaign you never named keeps showing as `Lorekeeper` until you name it. Click **Keep as is** to leave the notes where they are; Lorekeeper won't ask again. A campaign you share with your party isn't offered the move.

Backups made by older versions of Lorekeeper (in `Apps/Lorekeeper/Campaigns`, `Lorekeeper - <campaign>` folders or `lorekeeper-notes-<campaign>` repositories) stay where they are; new backups go to the places above.

Name a campaign under **Rename or remove** next to it in **Settings > General**; the name shows in the sidebar, on Home and in the tray. Each campaign needs its own name, since the names keep their backups apart. A new name starts a new backup: the old one stays where it was, and changing the name back carries it on. A campaign you remove keeps its name, so adding the folder again carries on its backups; so does a folder you add later with the name of a removed campaign, which is what you want after moving a folder (otherwise give it another name).

## Playing with your party

The whole party can keep one campaign together: everyone jots notes during the game, and every session shows all of them, each with who wrote it. Lorekeeper syncs a shared campaign between your computers through the Lorekeeper sync server at `lorekeeper.yonatankarp.com`, run by Lorekeeper's maintainer. There's nothing to install or sign up for. Notes are encrypted on your computer before they leave it, so the server stores only data it can't read.

**Sharing your campaign** (one person, the DM or any player; whoever shares is the campaign's **owner**):

1. Open the campaign, then in **Settings > General** click **Share with party…** next to it and pick your character under **I play** (characters are the pages in `PCs/`, so make or import them first; one person can import the whole party from D&D Beyond). A DM who plays no character picks **I'm the DM (no character)** (see "The DM's own notes" below).
2. Click **Share**. The sync server asks for its creation key the first time: ask the maintainer for it. Lorekeeper keeps it in your system's password storage and never asks again. Everything in the campaign then uploads, right away, whether or not it's the campaign that's open.
3. Click **Invite a player** and send the link to one player, privately (a message, not a public channel). Each link works once, for 7 days, and holds the campaign's key, so make one per player. Pick **Invite as: DM** first for your DM (the button then says **Invite a DM**). The same dialog, **Party…** next to the campaign from then on, lists the pending invites so you can cancel one, and the players. Both lists keep up while the dialog is open: when a player joins or leaves, a used invite goes and the players change within a few seconds, and changes made elsewhere (another DM's invites) show when you come back to the Settings window.

**Joining** (everyone else): in **Settings > General**, click **Join a shared campaign…**, paste your link, check which server it's for, then **Join**. Lorekeeper makes the campaign's folder in your Lorekeeper folder, named after the campaign, opens it and downloads everything. The dialog shows how it's going: **Joining Scale & Frost…**, then **Downloading 120 of 340 files…**, then **Done**. Click **Pick your character…**, and pick your character under **I play** in the **Party** dialog. An invite as a DM picks **I'm the DM (no character)** for you; you can still pick a character instead. Players don't get that choice (see "The DM's own notes" below). You can **Close** the dialog while it downloads; the campaign in Settings and the bottom of the sidebar show the progress too.

If the owner's notes haven't reached the sync server yet (say the owner shared the campaign and closed Lorekeeper right away), the dialog says **Waiting for the owner's notes to upload…**. After a minute it asks for a **Folder name**: ask the owner to open Lorekeeper and click **Join** again to wait some more, or type a name for the campaign's folder and click **Join**. Either way you don't need a new invite.

Or just click the link: the invite page asks your browser to open Lorekeeper (your browser may ask **Open Lorekeeper?** first; if nothing happens, click **Open in Lorekeeper** on the page), and Lorekeeper opens **Join a shared campaign** with the link filled in and its server shown. Nothing happens until you click **Join**. This works on a fresh install too, before you have a campaign. It needs Lorekeeper 0.7 or later; the page also has a **Download Lorekeeper** link. If your browser can't open Lorekeeper, paste the link as above.

What changes in a shared campaign:

- Your quick notes go to a file of your own: `Sessions/Session 4/Sibling 5.md` for Sibling 5. Two players never write the same file, so taking notes together never conflicts. Until you pick your character, the note box refuses to save and keeps your note.
- A session shows everyone's notes as one timeline, in time order, each with its author (and their portrait, if their PC page has one). A party in several time zones works too: each player's file records their time zone (`utc-offset: +02:00` in its properties), so the timeline is in the order the notes were written, and every time shows in your own time zone. A note written at 20:05 in Berlin shows as 14:05 in New York. The file itself keeps the writer's own times, so **Edit** shows your notes as you wrote them. The Journal shows them grouped, with authors too. You can delete your own notes from the Timeline; **Edit** opens your own file. Below the editor, a switch picks what you edit: **My notes** (your file in the session) or **Private notes** (your private notes for it, `Private/Sessions/Session 4/Sibling 5.md`, marked with a padlock and who reads them). A session from before sharing also has **Session notes**, its own `Session 4.md`, which **Edit** opens first. A file you start this way gets the session's date, not today's.
- **New session** starts the next session for everyone. If someone already started one in the last 12 hours and you haven't written in it yet, New session joins that one instead, so a party pressing it together stays in one session. Simplest: let one person (the DM, say) start sessions, and everyone else just writes.
- `↑` in the note box and the "start a new session?" offer only look at your own notes.
- Sessions from before you turned sharing on stay as they were. If the newest one was written in the last 12 hours (you shared the campaign mid-game, say), your notes continue it: they go to your own file next to it, `Sessions/Session 4/Sibling 5.md` beside `Sessions/Session 4.md`, dated like the session, and the session shows both. **Edit** on it opens **Session notes**, the notes from before sharing; switch to **My notes** or **Private notes** below the editor to edit yours. Private notes go to the same session. If it's older, the first note starts the next session.
- A session that holds only your notes (your file in it, and your private notes if you have any) can be deleted: **Delete…** on it (right-click it in the sidebar, the trash button, or **File > Move to Trash…**, `⌘⌫` / `Ctrl+Backspace`) says which files go and moves them all to the Trash, and **Edit > Undo** (`⌘Z` / `Ctrl+Z`) brings them all back. A session with anyone else's notes in it, or one from before sharing, can't be deleted; delete your own notes from its Timeline instead.
- Everything else (NPCs, places, quests, your PC page, images) is shared as it is: anyone can edit any page. Templates stay your own, and so do files Lorekeeper doesn't use (only pages and images sync, up to about 22 MB each).
- Every shared campaign syncs continuously, whether it's open or not, so switching campaigns never waits for a download. Lorekeeper syncs up to 8 shared campaigns at once, the open one always among them; any more say **Waiting: Lorekeeper syncs 8 shared campaigns at once. Open this one to sync it now**. Each campaign in Settings, and the open one at the bottom of the sidebar, show the sync status (**Synced**, **Syncing 3…**, **Downloading 120 of 340 files…**, **Offline, will sync when the server is back**) and who else is online. Online means Lorekeeper is running on their computer, whichever campaign they have open, and the party sees you the same way. Lorekeeper works offline as usual and catches up when the server answers again.
- A page someone else deletes goes to the campaign's hidden `.trash` folder on your computer, so you can bring it back.

**Private notes.** In a shared campaign, you can keep notes that are yours alone: what your character suspects, plans the party shouldn't know yet. On disk they're in the campaign's `Private` folder, but the sidebar shows them in their usual places with a padlock: a private NPC under **NPCs**, next to a shared page of the same name if there is one, a folder that only has private pages under its own name, and your private session notes inside their session (a session with only your private notes so far shows under **Sessions** with a padlock). They sync, encrypted, to your other computers only, or also to the DM if the campaign's owner allows it; the other players never get them, and neither does the owner unless the owner is the DM. Every place that shows them says which: **Private: only you** or **Private: you and the DM**.

- **New page** has a **Private** switch: the page goes into `Private/` (an NPC into `Private/NPCs/`). It starts on only from a folder that has nothing but private pages. Moving a page (dragging it onto a folder, or **Move…**) keeps it private or shared as it was.
- In the note box, start a note with `~` to make it private: `~@Halia lies about the mine` goes into your private notes for this session (`Private/Sessions/Session 4/Sibling 5.md`), filed under NPCs as usual. The box shows who will read it.
- The same works while you edit a shared page in Lorekeeper: a line that starts with `~` (after a `- ` and a time, if it has them) moves to your private notes when the page saves, once you've left that line (or clicked **Done**: nothing is saved while you're still typing it, so half a line never reaches the party), and the status says **Moved 1 note to your private notes**. Typing `~@[[Lorelei]] has a secret deal with [[Halia Thornton]]` into Session 1 leaves Session 1's shared notes without it and puts it in your private notes for Session 1, where its Timeline shows it with a padlock (it keeps its time, if it had one), and on Lorelei's and Halia's pages. On any other page, the line goes into your private notes for the latest session, with a link to the page added when it has none: `~ She had a secret deal with [[Lorelei]]` on Halia's page becomes `@[[Halia Thornton]] She had a secret deal with [[Lorelei]]`. A `~` in the middle of a line, in a code block or in the page's properties stays as typed, and so does every `~` until you pick your character. This happens only in Lorekeeper's editor: a `~` line typed in Obsidian stays in the shared page.
- A session shows your private notes in its Timeline and Journal with a padlock, next to everyone's shared ones.
- A private note that links a page shows at the bottom of that page, under **Private notes**, with a padlock and who reads it, and only for you: `~@[[Lorelei]] might be in a secret society` shows on Lorelei's page as "Session 1, 20:14: might be in a secret society" (in the note box, Tab turns `@Lor` into the link). A private page that links Lorelei shows there too, by its name. Click where it's from to open it. Plain `@Lorelei` text, without the link, doesn't show on her page. These never show under **Linked from** or on the map, so no other player learns of them.
- **Make private** / **Make shared** (the padlock in a page's header) moves it into `Private/` or out of it (making it shared asks first: the whole party gets it, and **Edit > Undo** (`⌘Z` or `Ctrl+Z`) never shares a page). Links keep working. Making a page private keeps your changes from then on to yourself, but doesn't take back the version players already synced: it stays in their trash.
- Private notes are files in your campaign folder, so your backups include them, and other apps on your computer can read them too.

**Roles: owner, DM and player.** Whoever shares the campaign is its **owner**, which on its own is just a player who looks after the party. Everyone else joins as a **Player** or a **DM**, as their invite said. In **Party…**, the owner can change anyone's role next to their name, and invite a DM who **Can manage players**: invites players and DMs, cancels invites, removes or re-invites players. Under **Private notes**, the owner chooses **Private notes visible to: the player only** or **the player and the DM**, and turns on **I'm the DM** if they run the game themselves (only then do they read private notes as the DM). "The DM" means everyone with the DM role, so with the player and the DM chosen, whoever the owner makes a DM (or a DM who manages players invites as one) reads private notes too. Everyone else sees their role and that setting there; when it changes, Lorekeeper tells them once.

**The DM's own notes:** a DM who plays no character picks **I'm the DM (no character)** under **I play**. Only the owner and members with the DM role see that choice. A player who had it picked (say the owner made them a player) is asked to pick their character instead, as if they hadn't picked one: **Pick your character: click Party…**. Their quick notes then go to `Sessions/Session 4/DM.md` (private ones to `Private/Sessions/Session 4/DM.md`), the Timeline and Journal show them as **DM**, without a portrait, and the Players list and **Online:** show **DM**. Because of that, a PC page named `DM` can't be the character you play. It's separate from the owner's **I'm the DM** switch under **Private notes**: picking **I'm the DM (no character)** only names your notes, and an owner who runs the game also turns on that switch to read the players' private notes (the dialog suggests it). Two DMs without characters share the one `DM.md` file, so their notes can end up in conflict copies; give each a character page if you have two.

**For the DM:** when the owner lets DMs read private notes, each player's appear under **Players' private notes** in the sidebar, a folder per player, read-only, and in the session pages with the player's name and a padlock. A player's private note that links a page shows in that page's **Private notes** too, with the player's name. When that's turned off, or you stop being a DM, Lorekeeper deletes them from your computer.

**A new computer:** keys and sign-ins stay on each computer, so a player moving to a new one asks the owner (or a DM who manages players) for a **Re-invite** (next to their name in **Party…**). They paste it into **Join a shared campaign…** on the new computer, which becomes them: same role, same private notes. Their old computer stops syncing and says **Signed in on another computer** (or **Removed** if it was offline at the time); its files stay. A re-invite makes whoever opens it that player, private notes included: the owner or a DM who manages players could, in principle, read your private notes this way, and you'd see **Signed in on another computer** on your own computer. So it's only ever sent to the player it's for.

It's not a live co-editor: two people changing the same page at the same time (say an NPC page) end up with two versions. Lorekeeper keeps yours and saves theirs next to it, named like `Mirela (conflict 2026-10-06 2015).md`, and lists it under **Sync conflicts** on Home. Open both, keep what you want in the original, then delete the copy. Your version is what the others get.

**Removing a player:** the owner (or a DM who manages players) opens **Party…** next to the campaign and clicks **Remove** next to the player. The button turns into **Remove Vex? Click again** (with the player's name): click it again within about 5 seconds to remove them, or wait or move away and it goes back to **Remove**. Their sync stops at once and Lorekeeper tells them they were removed; the notes already on their computer stay there, and their private notes are deleted from the sync server. Removing a joined campaign from your own list (**Remove from list** next to it) stops its sync and deletes its token from your computer; the owner still sees you under Players until they remove you.

**Leaving:** to leave a campaign someone else shares, open **Party…** next to it and click **Leave**, then **Leave** in the dialog that asks. This computer stops syncing it, you're gone from the owner's Players list, and your private notes are deleted from the sync server. Your copy stays on your computer as a campaign of your own. To come back, ask the owner for a new invite.

**Stop sharing** (the owner): **Party…**, then **Stop sharing**, and **Stop sharing** again in the dialog that asks. The campaign is deleted from the sync server, with everyone's private notes there, and every player's sync stops at once: Lorekeeper tells them "The owner stopped sharing" the campaign, and their copy stays on their computer. Yours stays too, as a campaign of your own; share it again later and everyone needs a new invite. A player whose computer was off at the time sees **Removed from this campaign** when it next connects. **Delete campaign** on a campaign you share offers **Stop sharing and delete** first; **Delete only** leaves it on the server, where you can't manage it any more.

Keep the campaign folder where it is: Lorekeeper syncs one folder per campaign, and a folder it can't find doesn't sync until it's back. You can still keep the folder in a cloud drive or open it in another app.

## Backups

**Settings > Backups**: pick one under **Back up to**. Setting up a new one turns off the one you had (what's already backed up there stays).

- **Back up to a folder:** pick a folder in Google Drive, Dropbox, OneDrive, iCloud Drive or on a USB drive. Lorekeeper keeps a dated copy for each of the last 30 days.
- **Dropbox or Google Drive:** click **Sign in**, then sign in and allow access in your browser (your password never goes through Lorekeeper). Lorekeeper only sees its own folders: **Apps/Lorekeeper** in Dropbox, and the **Lorekeeper** folder it made in Google Drive, with a folder for each campaign in both. Only new and changed notes are uploaded, and a note you delete is deleted there too. A deleted or overwritten note can be brought back from Dropbox's **Deleted files** or a file's **Version history**, or the Google Drive **Trash** (30 days or more).
- **Back up to GitHub:** sign in with a code (no password in Lorekeeper). Every backup is a version in one private repository, `lorekeeper-notes`, with a folder for each campaign; a campaign's backup only ever changes its own folder. A file's **History** on github.com shows every older version.

**To restore**, click **Restore…** next to a backup (it restores the campaign that's open), pick a day (folder), a version (GitHub) or the current backup (Dropbox, Google Drive), and choose where the restored notes go. They're downloaded into a new folder (by default `<campaign> restored <date>` next to your notes folder); your notes folder and the backup are never changed. Then **Show in Finder** (Explorer on Windows) to copy back what you need, or **Open as a new campaign** to switch to it. It then backs up on its own like any other campaign, so it never touches the backup it came from. You can also restore by hand: copy from a dated folder, download from the Dropbox or Google Drive website, or use **Code > Download ZIP** on GitHub (the campaign is its own folder in it).

To look at a backup of the campaign that's open: the folder button (**Show in Finder**, or Explorer on Windows) for the backup folder, or the arrow button (**Open in Dropbox**, **Open in Google Drive** or **Open on GitHub**), which opens it in your browser.

Backups run on each new session, every 30 minutes while notes change, and once a day. Keeping the notes folder itself in a cloud drive syncs it, but sync isn't a backup: deletions sync too.

## Settings

- **General:** campaigns (notes folders), sharing them with your party, the D&D Beyond sign-in, launch at login, updates, and under **Advanced** the sync server.
- **Shortcuts:** change the global shortcuts; a shortcut another app already uses is refused and the old one keeps working. **Reset to defaults** puts every shortcut back at once (`⌘⌥N` / `Ctrl+Alt+N` and `⌘⇧S` / `Ctrl+Shift+S`, with New session and New page off).
- **Appearance:** Light (Tome), Dark (Dungeon) or Match my computer, and text size.
- **Backups:** see above.

## Cheat sheet

In the app: **Help > Cheat Sheet**, `⌘/` / `Ctrl+/`, or **Show cheat sheet** in **Settings > Shortcuts**. It shows the global shortcuts as you set them.

### Note symbols

Start a note with a symbol to file it. It works in the note box and at the start of any line on a session page. Notes show with their kind in the **Timeline** and grouped in the **Journal**.

| Start with | Goes to |
|---|---|
| `@` | **NPCs**: `@[[Mirela]] runs the inn` |
| `#` | **Loot**: `#Silver dagger` |
| `!` | **Quests**: `!Find the missing miners` |
| `?` | **Mysteries**: `?Who sent the letter` |
| `"` or `“` | **Quotes**: `"Run!" - Vex`. Quotes keep their marks. |
| no symbol | **What happened** |
| `~` | **Private**, in the note box of a shared campaign: the note goes to your private notes for the session, still filed by the symbol after it (`~@[[Halia]] lies`). The box says who reads it. In a campaign of your own the `~` is dropped and the note is saved as usual. |

On a session page, write loot as a list item (`- #Silver dagger`): a line starting with `#` is read as a heading and doesn't show as a note.

### Links and images

| Type | What it does |
|---|---|
| `[[Mirela]]` | A link to the page named Mirela, in any folder. Clicking a link to a page that doesn't exist yet opens **New page** to make it. |
| `[[Baron Vex\|the Baron]]` | A link to Baron Vex that reads "the Baron". |
| `[[NPCs/Vex]]` | A link to the Vex in NPCs, when two pages share a name. |
| `@[[Mirela]]` | A mention: the `@` files the note under NPCs, the link lists it on Mirela's page under **Linked from**. `@Mirela` without brackets is not a link. |
| `@Mir` or `[[Mir` | Suggests page names. Tab picks one and makes the link. |
| `![[map.png]]` | Shows an image from the notes folder. `![[map.png\|300]]` sets its width, `![[map.png\|300x200]]` width and height. Paste or drop an image into the editor to save it in `Attachments/` and embed it. |
| `![Map](Maps/map.png)` | A Markdown image, which works too. |
| `[Site](https://example.com)` | A web link: it opens in your browser. |

### Anywhere

These work even while Lorekeeper is in the background. Change them in **Settings > Shortcuts**.

| Keys | What it does |
|---|---|
| `⌘⌥N` / `Ctrl+Alt+N` | **Quick note**: a one-line note box, over any app |
| `⌘⇧S` / `Ctrl+Shift+S` | **Save selection**: saves the selected text, or the clipboard |
| off until you set it | **New session**, without opening a window |
| off until you set it | **New page**: opens the New page dialog |

### In the note box

| Keys | What it does |
|---|---|
| `Enter` | Saves the note |
| `⌘Enter` / `Ctrl+Enter` | Saves it to a new session, when the box offers one (the last note is over 12 hours old) |
| `↑` in the empty box | Brings back the last note to fix: Enter saves the fix in place, Esc cancels |
| `Tab` | Takes the suggested page name |
| `↑` `↓` | Picks another suggestion |
| `Esc` | Drops the suggestion, then closes the box without saving |

### In Lorekeeper

| Keys | What it does |
|---|---|
| `⌘K` / `Ctrl+K` | Search |
| `⌘E` / `Ctrl+E` | Edit / Preview |
| `⌘N` / `Ctrl+N` | New page |
| `⌘⇧N` / `Ctrl+Shift+N` | New session |
| `⌘⇧H` / `Ctrl+Shift+H` | Home |
| `⌘[` `⌘]` / `Ctrl+[` `Ctrl+]` | Back, Forward |
| `F2` | Rename the page |
| `⌘⌫` / `Ctrl+Backspace` | Move the page to the Trash (outside text) |
| `⌘Z` / `Ctrl+Z` | Undo, including deleting, renaming and moving pages |
| `⌘⇧Z` / `Ctrl+Shift+Z` or `Ctrl+Y` | Redo |
| `⌘,` / `Ctrl+,` | Settings |
| `⌘+` `⌘-` `⌘0` / `Ctrl++` `Ctrl+-` `Ctrl+0` | Zoom in, zoom out, actual size |
| `⌘/` / `Ctrl+/` | The cheat sheet |
| `Esc` | Closes a dialog |

### In the editor

| Keys | What it does |
|---|---|
| `⌘B` / `Ctrl+B` | Bold |
| `⌘I` / `Ctrl+I` | Italic |
| `Tab` or `Enter` | Takes the suggested page name (type `[[` or `@` for suggestions) |
| `Tab` `⇧Tab` / `Tab` `Shift+Tab` | Indents or outdents a list item |
| `Enter` | Continues a list |
| `⌘`-click / `Ctrl`-click | Follows a link on the line you're editing (elsewhere a click does) |

### In search

| Keys | What it does |
|---|---|
| `↓` `↑` | Moves through the results |
| `Enter` | Opens the result |
| `Esc` | Clears the search |

## Limits

- A session's notes file keeps the time zone it was started in. Notes written after the clocks change (daylight saving time) or after a flight during that session show an hour (or the flight's time difference) off to the rest of the party.
- When saving a selection, the previous clipboard is restored as text only.
