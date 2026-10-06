# Playing with your party

Lorekeeper can share one campaign with your whole party: everyone jots notes during the game, and everyone sees the same NPCs, quests, places and a single merged session log. Notes you want to keep to yourself stay private on your own computer.

Lorekeeper syncs a shared campaign between your computers through the Lorekeeper sync server, so there's nothing to install or sign up for. Notes are encrypted on your computer before they leave it, so the server stores only data it can't read. Your private notes never leave your computer except in your own backups.

## What's shared and what's private

| | Shared with the party | Private to you |
|---|---|---|
| NPCs, PCs, locations, items, factions, quests, lore | yes | pages you mark private |
| Session notes | your notes, merged with everyone else's | notes you start with `~` |
| Where it lives | the campaign folder on each player's computer, synced through the server | a private folder next to it, on your computer |
| Backed up | by every player who turns backups on | by your own backups |

## Setting it up

**One person (say the DM), once:**

1. Open the campaign, then in **Settings > General** click **Share with party…** next to it.
2. Under **I play**, pick your character. That's how your notes get your character's name and portrait. If your character has no page yet, import it from D&D Beyond first (**+ New page > PC > Import from D&D Beyond…**).
3. Click **Share**. The first time, the server asks for its creation key: ask the maintainer for it.
4. Click **Invite a player** and send the link to that player, privately. Make a new link for each player.

**Everyone else:** in **Settings > General**, click **Join a shared campaign…**, paste your link, then **Join**. When the campaign has downloaded, pick your character under **I play**.

That's it. Notes you take from now on are shared. The [guide](GUIDE.md#playing-with-your-party) has the details.

## During the game

Take notes exactly as before, with the note box (`⌘⌥N` / `Ctrl+Alt+N`) and the same prefixes. Each player's notes go into a file of their own, so two people writing at once never get in each other's way.

The session page shows everyone's notes in one timeline, in the order they were written, each marked with the character who wrote it. Notes from the rest of the party appear by themselves while Lorekeeper is open, usually within a few seconds. **New session** starts the next session for everyone; whoever writes first opens it.

You can fix (`↑` in the empty note box) or delete only your own notes. **Edit** on a session opens your own part of it.

## Private notes

Start a note with `~` and it goes to your private session notes instead of the shared ones:

> `~ I think Halia is lying about the deal`

Private notes show in your session timeline with a lock, mixed in with everyone's notes, but only on your computer.

For private pages, turn on **Private** in the **+ New page** dialog. They're kept in a private folder next to the campaign, show with a lock in the sidebar and in search, and can link to shared pages as usual. A link from a shared page to one of your private pages only works for you; for everyone else it shows as a missing page.

## Backups

Sync isn't a backup: if someone deletes a page, the deletion reaches everyone. Turn on a backup in **Settings > Backups** (it only takes one player to keep the party safe, and more is better). Your backups include the shared campaign and your own private notes, each in its own folder, so a deleted or overwritten note can be brought back from any player's backup.

## When something looks wrong

- **Someone's notes don't appear.** Check the sync status next to the campaign in **Settings > General** on both computers. The campaign that's open syncs; another shared campaign catches up when you switch to it.
- **"Sync conflicts" on Home.** Two people changed the same shared page at almost the same moment, and Lorekeeper kept both versions. Open both, keep what you want in the original, and delete the copy.
- **Notes went to the wrong character.** Check **I play** under **Party…** next to the campaign in **Settings > General**.
- **Editing the same page together.** Lorekeeper isn't a live co-editor like Google Docs: avoid editing the same page at the same time as someone else.
