# Lorekeeper

Desktop app (Tauri: `src/` web UI, `src-tauri/` Rust) for D&D session notes, plus a sync server (`sync-server/`, `sync-protocol/`) for shared campaigns.

- Build, test, release, website and service setup: `docs/DEVELOPMENT.md`. Sync design: `docs/SYNC.md`.
- `pnpm test` runs the JS and Rust tests. `pnpm site` builds the website into `site/` (git-ignored).

## Keep the docs and website current

The website is https://yonatankarp.com/lorekeeper/. It is built from two sources that drift apart in different ways:

- **`docs/GUIDE.md` and `docs/PRIVACY.md`** render to `GUIDE.html` and `PRIVACY.html`. Any push to main that touches `docs/**` deploys them right away.
- **The home page is hand-written** in `scripts/build-site.mjs` (`FEATURES`, `STEPS`, `FAQ`, the hero copy and the meta descriptions). It never picks up doc changes on its own. When a feature, label or shortcut changes, grep this file too.

Rules:

- **A user-facing change updates `docs/GUIDE.md` in the same commit.** Use the exact labels the UI shows (`src/*.html`, `src/*.js`), in **bold**, with both `⌘` and `Ctrl` shortcuts.
- **The home page only advertises what's in the latest release** (`gh release view`). A feature that's on main but not released isn't on the home page at all: not in `FEATURES`, the steps, the FAQ or the meta description. It's added at release. Check with `git log <latest tag>..HEAD`.
- **Check every website claim against the code or the docs at that release tag.** Don't describe planned or draft features, for example anything that's only in `docs/PARTY.md`, which is an unshipped draft.
- **`docs/PRIVACY.md` changes whenever what Lorekeeper stores, sends or connects to changes.** That covers new network requests, services, stored secrets and sync behaviour. Bump its "Last updated" date. Never drop a section by accident: diff it before committing.
- **Keep these URLs working:** `/lorekeeper/`, `GUIDE.html` and `PRIVACY.html` are on Google's OAuth consent screen. Keep heading ids stable too, because the home page links to `GUIDE.html#installing`.
- **The site makes no third-party requests** other than the GitHub releases API call behind the download button. No analytics, fonts from a CDN or embeds.
- **Before pushing site changes,** run `pnpm site` and serve it (`python3 -m http.server 18431 -d site`; 8765 is taken by the `ui-static` preview). Check light and dark mode, and a 375px width with no sideways scroll.

## Releasing

Follow "CI and releases" in `docs/DEVELOPMENT.md`, then update the website for the new version:

1. Add the features that shipped to `FEATURES` in `scripts/build-site.mjs`.
2. Add the new features to the `FAQ` and the meta description if they're worth it.
3. Update `docs/GUIDE.md`. The release notes are written from the pull requests: preview them before tagging (step 2 of "To release" in `docs/DEVELOPMENT.md`) and fix a line by editing that pull request's `## Release note` or title.
4. If the release changes how the app looks, retake `docs/screenshots/light.jpg` and `dark.jpg` (the home page, its link preview and the README use them). They're 1120×740 macOS window captures of the real app with a demo campaign's Home open, in the Light and Dark themes. Retake them only for a release that's out, never for work on main.

## Commits

- Commit messages and pull request titles: `kind(Area): what changed, in user terms` (`feat(Sessions): …`, `fix(Sync): …`, `docs(Website): …`). Pull requests are squash-merged, so the title becomes the commit subject, and the release notes are written from the titles (a check fails a title without a kind):
  - `feat` is a new feature, `change` an improvement and `fix` a bug fix: each goes in the release notes, under New features, Improvements or Bug fixes.
  - `docs`, `chore`, `ci`, `test` and `refactor` are left out of them.
- A `feat`, `change` or `fix` pull request can have a `## Release note` section in its description: the first paragraph is published instead of the title. One line, about 20 words, starting with a short **bold lead**, written for players, no file names. `none` leaves the pull request out of the notes.
- Stage files by path. The working tree often holds unrelated work in progress. Never commit `site/` or `src-tauri/target-preview/`.
- Ask before pushing. Main is protected (pull request and 2 checks), the owner can bypass it, and a push to main deploys the website.
