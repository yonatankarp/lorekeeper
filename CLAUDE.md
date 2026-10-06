# Lorekeeper

Desktop app (Tauri: `src/` web UI, `src-tauri/` Rust) for D&D session notes, plus a sync server (`sync-server/`, `sync-protocol/`) for shared campaigns.

- Build, test, release, website and service setup: `docs/DEVELOPMENT.md`. Sync design: `docs/SYNC.md`.
- `pnpm test` runs the JS and Rust tests. `pnpm site` builds the website into `site/` (git-ignored).

## Keep the docs and website current

The website is https://yonatankarp.com/lorekeeper/. It is built from two sources that drift apart in different ways:

- **`docs/GUIDE.md` and `docs/PRIVACY.md`** render to `GUIDE.html` and `PRIVACY.html`. Any push to main that touches `docs/**` deploys them right away.
- **The home page is hand-written** in `scripts/build-site.mjs` (`FEATURES`, `SOON`, `STEPS`, `FAQ`, the hero copy and the meta descriptions). It never picks up doc changes on its own. When a feature, label or shortcut changes, grep this file too.

Rules:

- **A user-facing change updates `docs/GUIDE.md` in the same commit.** Use the exact labels the UI shows (`src/*.html`, `src/*.js`), in **bold**, with both `⌘` and `Ctrl` shortcuts.
- **The home page only advertises what's in the latest release** (`gh release view`). A feature that's on main but not released goes in `SOON`, under the `NEXT` version, never in `FEATURES`, the steps or the meta description. Check with `git log <latest tag>..HEAD`.
- **Check every website claim against the code or the docs at that release tag.** Don't describe planned or draft features, for example anything that's only in `docs/PARTY.md`, which is an unshipped draft.
- **`docs/PRIVACY.md` changes whenever what Lorekeeper stores, sends or connects to changes.** That covers new network requests, services, stored secrets and sync behaviour. Bump its "Last updated" date. Never drop a section by accident: diff it before committing.
- **Keep these URLs working:** `/lorekeeper/`, `GUIDE.html` and `PRIVACY.html` are on Google's OAuth consent screen. Keep heading ids stable too, because the home page links to `GUIDE.html#installing`.
- **The site makes no third-party requests** other than the GitHub releases API call behind the download button. No analytics, fonts from a CDN or embeds.
- **Before pushing site changes,** run `pnpm site` and serve it (`python3 -m http.server 18431 -d site`; 8765 is taken by the `ui-static` preview). Check light and dark mode, and a 375px width with no sideways scroll.

## Releasing

Follow "CI and releases" in `docs/DEVELOPMENT.md`, then update the website for the new version:

1. Move the `SOON` entries that shipped into `FEATURES` in `scripts/build-site.mjs`. Set `NEXT` to the following version, or empty `SOON`.
2. Remove "coming in …" from the `FAQ`, and add the new features to the meta description if they're worth it.
3. Update `docs/GUIDE.md` and the release notes in `RELEASE_NOTES.md`.

## Commits

- Commit messages: `Area: what changed, in user terms` (`Settings: …`, `Sync: …`, `Website: …`, `Docs: …`).
- Stage files by path. The working tree often holds unrelated work in progress. Never commit `site/` or `src-tauri/target-preview/`.
- Ask before pushing. Main is protected (pull request and 2 checks), the owner can bypass it, and a push to main deploys the website.
