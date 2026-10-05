---
title: Privacy policy
---

# Lorekeeper privacy policy

*Last updated: 5 October 2026*

Lorekeeper is a free, open-source desktop app for keeping D&D session notes. It is made by Yonatan Karp-Rudin ([yonvata@gmail.com](mailto:yonvata@gmail.com)). The source code is at [github.com/yonatankarp/lorekeeper](https://github.com/yonatankarp/lorekeeper).

## The short version

Lorekeeper has no servers, no accounts and no analytics. Your notes stay on your computer. If you turn on a backup, they go straight from your computer to your own Dropbox, Google Drive or GitHub account, and nowhere else.

## What Lorekeeper stores

- **Your notes** are plain text files in the folder you choose on your computer (by default `Documents/Lorekeeper`).
- **Settings** are stored in the app's configuration folder on your computer.
- **Sign-in tokens** for the backups you turn on are stored in your system's password storage (macOS Keychain, Windows Credential Manager or the Secret Service on Linux). They are only ever sent to the service they belong to.

Nothing is sent to the developer. Lorekeeper does not collect usage data, crash reports or personal information.

## Backups to Dropbox, Google Drive and GitHub

Backups are optional and off until you sign in. When you do, Lorekeeper uploads your notes from your computer directly to your own account:

- **Dropbox:** Lorekeeper can only access its own folder, `Apps/Lorekeeper`.
- **Google Drive:** Lorekeeper uses the `drive.file` permission, so it can only see and change files it created itself, in a `Lorekeeper` folder (and a `Lorekeeper - <name>` folder for each further campaign). It cannot see the rest of your Drive.
- **GitHub:** Lorekeeper creates a private repository called `lorekeeper-notes` in your account and backs up to it, plus a private `lorekeeper-notes-<name>` repository for each further campaign. GitHub's sign-in grants access to your repositories in general; Lorekeeper only uses the repositories it created for your notes.

Lorekeeper only uses this access to back up your notes. It does not read other data, and nothing is shared with, sold to or used by anyone else, including the developer.

Lorekeeper's use and transfer of information received from Google APIs adheres to the [Google API Services User Data Policy](https://developers.google.com/terms/api-services-user-data-policy), including the Limited Use requirements.

## Other network requests

- **Updates:** Lorekeeper checks GitHub for new versions once a day. Like any web request, GitHub sees your IP address. You can turn this off in Settings.
- **Links you click** open in your browser.

## Removing access and data

- Sign out of a backup in **Settings > Backups**. This deletes the stored token from your computer.
- You can also revoke Lorekeeper's access in your [Google account](https://myaccount.google.com/permissions), [Dropbox settings](https://www.dropbox.com/account/connected_apps) or [GitHub settings](https://github.com/settings/applications).
- Backed-up notes stay in your own account until you delete them there.
- Uninstalling Lorekeeper leaves your notes folder in place; delete it yourself if you want.

## Changes

If this policy changes, the new version will be published here with a new date.

## Contact

Questions: [yonvata@gmail.com](mailto:yonvata@gmail.com) or [open an issue](https://github.com/yonatankarp/lorekeeper/issues).
