# Playing with your party

Lorekeeper can share one campaign with your whole party: everyone jots notes during the game, and everyone sees the same NPCs, quests, places and a single merged session log. Notes you want to keep to yourself stay private on your own computer.

There's no Lorekeeper server and no account to make. The shared campaign is a folder in Google Drive that your party already has access to, and your private notes never leave your computer except in your own backups.

## What's shared and what's private

| | Shared with the party | Private to you |
|---|---|---|
| NPCs, PCs, locations, items, factions, quests, lore | yes | pages you mark private |
| Session notes | your notes, merged with everyone else's | notes you start with `~` |
| Where it lives | the campaign folder in Google Drive | a private folder next to it, on your computer |
| Backed up | by every player who turns backups on | by your own backups |

## Setting it up

You need Google Drive for desktop on every computer (it's free with a Google account), and Lorekeeper 0.7 or later.

**One person, once:**

1. In Google Drive, create a folder for the campaign, for example `Scale & Frost`. If you already keep the campaign in Lorekeeper, move its folder into Drive instead.
2. Share the folder with everyone in the party, with **Editor** access.

**Everyone else:**

1. In Google Drive, open **Shared with me**, right-click the campaign folder and choose **Organize > Add shortcut**, then put the shortcut in **My Drive**. Drive for desktop now syncs it to your computer.
2. Check it's there: on a Mac it's under **Google Drive > My Drive** in Finder; on Windows, under the Google Drive letter (usually `G:`).

**Everyone, in Lorekeeper:**

1. Open **Settings > General > Add campaign…** and choose the campaign folder inside Google Drive.
2. Next to the campaign, turn on **Shared with my party**.
3. Under **I play**, pick your character. That's how your notes get your character's name and portrait. If your character has no page yet, import it from D&D Beyond first (**+ New page > PC > Import from D&D Beyond…**).

That's it. Notes you take from now on are shared.

## During the game

Take notes exactly as before, with the note box (`⌘⌥N` / `Ctrl+Alt+N`) and the same prefixes. Each player's notes go into a file of their own, so two people writing at once never get in each other's way.

The session page shows everyone's notes in one timeline, in the order they were written, each marked with the character who wrote it. Notes from the rest of the party appear by themselves while Lorekeeper is open, usually within a few seconds to a minute, depending on how fast Google Drive syncs. **New session** starts the next session for everyone; whoever writes first opens it.

You can fix (`↑` in the empty note box) or delete only your own notes. **Edit** on a session opens your own part of it.

## Private notes

Start a note with `~` and it goes to your private session notes instead of the shared ones:

> `~ I think Halia is lying about the deal`

Private notes show in your session timeline with a lock, mixed in with everyone's notes, but only on your computer.

For private pages, turn on **Private** in the **+ New page** dialog. They're kept in a private folder next to the campaign, show with a lock in the sidebar and in search, and can link to shared pages as usual. A link from a shared page to one of your private pages only works for you; for everyone else it shows as a missing page.

## Backups

Sync isn't a backup: if someone deletes a page, the deletion reaches everyone. Turn on a backup in **Settings > Backups** (it only takes one player to keep the party safe, and more is better). Your backups include the shared campaign and your own private notes, each in its own folder, so a deleted or overwritten note can be brought back from any player's backup.

## When something looks wrong

- **Someone's notes don't appear.** Check that Drive for desktop is running and signed in on both computers, and that the folder in Drive shows the new file. Lorekeeper shows whatever Drive has delivered.
- **"Sync conflicts" on Home.** Two people changed the same shared page at almost the same moment, and Drive kept both versions. Open both, keep what you want in the original, and delete the copy.
- **Notes went to the wrong character.** Check **I play** in **Settings > General**.
- **Slow to show up.** Drive can take up to a minute. That's fine for notes, but Lorekeeper isn't a live co-editor like Google Docs: avoid editing the same page at the same time as someone else.
