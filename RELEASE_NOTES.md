What's new in 0.7.1:
- Private notes show on the pages they link: write ~@[[Lorelei]] … and it appears under Private notes at the bottom of her page, for you only (and the DM, if the owner allows it), never on the map
- A line starting with ~ typed into a shared page moves to your private notes when it saves
- Private pages and notes sit in their usual places in the sidebar, with a padlock
- Edit on a session switches between Session notes, My notes and Private notes

Fixed:
- A quick note no longer starts a new session when the session you're playing is from before sharing
- A session that's all yours can be deleted
- Move… never makes a private page shared, or a shared one private: Make shared and Make private do, and ask first
- Settings > Shortcuts has Reset to defaults

Sharing a campaign the first time needs the sync server's creation key: ask Lorekeeper's maintainer. Joining only needs the invite link and Lorekeeper 0.7 or later.

Downloads: macOS `.dmg` (Apple Silicon and Intel), Windows `-setup.exe`, Linux `.AppImage`. Lorekeeper 0.2.0 and later update themselves; the `.tar.gz`, `.sig` and `latest.json` files are for that.

The builds are not code-signed yet:
- macOS: if it says the app is damaged or can't be opened, run `xattr -dr com.apple.quarantine /Applications/Lorekeeper.app`, or right-click > Open.
- Windows: SmartScreen may warn; choose More info > Run anyway.
