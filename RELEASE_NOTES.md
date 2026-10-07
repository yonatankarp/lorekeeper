What's new in 0.7.2:
- Playing across time zones: note times in a shared session show in your own time zone, and the Timeline is in the right order whoever wrote what
- A player who joins sees old sessions as old, so their notes go to the same session as everyone else's
- A lighter look: icons instead of bulky buttons, and shortcut hints that match your computer (⌘ on macOS, Ctrl elsewhere)

Also changed:
- The Obsidian buttons are gone; your notes are still plain Markdown that Obsidian can open
- Rename and Move left the page header: use F2, the File menu, right-click or drag
- Session notes record your UTC offset, which only your party can see

Sharing a campaign the first time needs the sync server's creation key: ask Lorekeeper's maintainer. Joining only needs the invite link and Lorekeeper 0.7 or later.

Downloads: macOS `.dmg` (Apple Silicon and Intel), Windows `-setup.exe`, Linux `.AppImage`. Lorekeeper 0.2.0 and later update themselves; the `.tar.gz`, `.sig` and `latest.json` files are for that.

The builds are not code-signed yet:
- macOS: if it says the app is damaged or can't be opened, run `xattr -dr com.apple.quarantine /Applications/Lorekeeper.app`, or right-click > Open.
- Windows: SmartScreen may warn; choose More info > Run anyway.
