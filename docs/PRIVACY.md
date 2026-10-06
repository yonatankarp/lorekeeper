---
title: Privacy policy
---

# Lorekeeper privacy policy

*Last updated: 7 October 2026*

Lorekeeper is a free, open-source desktop app for keeping D&D session notes. It is made by Yonatan Karp-Rudin ([yonvata@gmail.com](mailto:yonvata@gmail.com)). The source code is at [github.com/yonatankarp/lorekeeper](https://github.com/yonatankarp/lorekeeper).

## The short version

Lorekeeper has no accounts and no analytics. Your notes stay on your computer, unless you share a campaign with your party: then it syncs through a Lorekeeper sync server, end-to-end encrypted, so the server can't read it. If you turn on a backup, they go straight from your computer to your own Dropbox, Google Drive or GitHub account, and nowhere else. If you sign in to D&D Beyond, Lorekeeper only reads the characters you import from there; your notes are never sent to D&D Beyond.

## What Lorekeeper stores

- **Your notes** are plain text files in the folder you choose on your computer (by default `Documents/Lorekeeper`).
- **Settings** are stored in the app's configuration folder on your computer.
- **Sign-in tokens** for the backups you turn on, your D&D Beyond sign-in session if you sign in there, and the keys and tokens of shared campaigns, are stored in your system's password storage (macOS Keychain, Windows Credential Manager or the Secret Service on Linux). They are only ever sent to the service they belong to.

Nothing is sent to the developer. Lorekeeper does not collect usage data, crash reports or personal information.

## Backups to Dropbox, Google Drive and GitHub

Backups are optional and off until you sign in. When you do, Lorekeeper uploads your notes from your computer directly to your own account:

- **Dropbox:** Lorekeeper can only access its own folder, `Apps/Lorekeeper`.
- **Google Drive:** Lorekeeper uses the `drive.file` permission, so it can only see and change files it created itself, in a `Lorekeeper` folder with a folder for each campaign. It cannot see the rest of your Drive.
- **GitHub:** Lorekeeper creates a private repository called `lorekeeper-notes` in your account and backs up each campaign to a folder in it. GitHub's sign-in grants access to your repositories in general; Lorekeeper only uses the repository it created for your notes.

Lorekeeper only uses this access to back up your notes and, when you click Restore, to download that backup to your computer. It does not read other data, and nothing is shared with, sold to or used by anyone else, including the developer.

Lorekeeper's use and transfer of information received from Google APIs adheres to the [Google API Services User Data Policy](https://developers.google.com/terms/api-services-user-data-policy), including the Limited Use requirements.

## Characters from D&D Beyond

Importing characters is optional. When you sign in to D&D Beyond in Settings, you sign in on D&D Beyond's own page, so your password goes to D&D Beyond and never to Lorekeeper. Lorekeeper keeps the sign-in session D&D Beyond gives it in your system's password storage, sends it only to D&D Beyond, and uses it only to read the characters (and campaign pages) you import or refresh. What it reads is written into your PC pages on your computer; nothing goes to anyone else.

## Shared campaigns

Sharing a campaign with your party is optional. A shared campaign syncs through a Lorekeeper sync server: by default `lorekeeper.yonatankarp.com`, run by the developer, or another server an invite link points to (Lorekeeper shows which before you join).

- **End-to-end encrypted:** notes, images, file names, the campaign's name and players' character names are encrypted on your computer with the campaign's key before they're sent. The key is made on the sharing player's computer and travels only inside invite links (in the part after `#`, which never reaches the server). The server stores and passes on encrypted data it can't read.
- **What the server sees:** that a campaign exists, how many files it has and how big they are, when they change, and the IP addresses of the computers that connect. It keeps no logs of notes, keys, tokens or invites.
- **Secrets:** the campaign's key, your sign-in token for it, and the server's creation key (if you share a campaign) are stored only in your system's password storage, never in settings or notes, and only ever sent to that server (the key never is).
- When the campaign's owner removes you, or you remove a joined campaign from Lorekeeper, its token is deleted from your computer; your notes stay.

## Other network requests

## Removing access and data

- Sign out of a backup in **Settings > Backups**, or of D&D Beyond in **Settings > General**. This deletes the stored token or session from your computer.
- You can also revoke Lorekeeper's access in your [Google account](https://myaccount.google.com/permissions), [Dropbox settings](https://www.dropbox.com/account/connected_apps) or [GitHub settings](https://github.com/settings/applications).
- Backed-up notes stay in your own account until you delete them there.
- Uninstalling Lorekeeper leaves your notes folder in place; delete it yourself if you want.

## Changes

If this policy changes, the new version will be published here with a new date.

## Contact

Questions: [yonvata@gmail.com](mailto:yonvata@gmail.com) or [open an issue](https://github.com/yonatankarp/lorekeeper/issues).
