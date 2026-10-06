// Builds the website (https://yonatankarp.com/lorekeeper/) into site/: a designed home page plus docs/GUIDE.md and
// docs/PRIVACY.md, in the app's own look. No dependencies: the vendored marked renders the Markdown, the theme tokens
// and fonts come straight from the app (src/styles.css, src/fonts), the kind icons from src/icons.js.
// Run `pnpm site` (or `node scripts/build-site.mjs`); .github/workflows/pages.yml deploys site/ from main.
import { cpSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { marked } from "../src/vendor/marked.esm.js";
import { icon } from "../src/icons.js";

const root = new URL("..", import.meta.url);
const read = (path) => readFileSync(new URL(path, root), "utf8");
const out = new URL("site/", root);
const SITE = "https://yonatankarp.com/lorekeeper/";
const REPO = "https://github.com/yonatankarp/lorekeeper";
const RELEASES = `${REPO}/releases/latest`; // asset names carry the version, so link the release page, not files

const esc = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
const slug = (html) =>
  html.replace(/<[^>]+>/g, "").replace(/&\w+;|&#\d+;/g, "").toLowerCase().replace(/[^\w\s-]/g, "").trim().replace(/\s+/g, "-");

// Heading ids as GitHub makes them, so links like GUIDE.html#installing work; the page's h1 gets the drop cap.
marked.use({
  renderer: {
    heading({ tokens, depth }) {
      const text = this.parser.parseInline(tokens);
      return `<h${depth} id="${slug(text)}"${depth === 1 ? ' class="title"' : ""}>${text}</h${depth}>\n`;
    },
  },
});

/** A docs/ Markdown file: its title (front matter, else the first heading) and HTML. */
function doc(name) {
  const src = read(`docs/${name}`);
  const front = src.match(/^---\n([\s\S]*?)\n---\n/);
  const md = front ? src.slice(front[0].length) : src;
  const title = front?.[1].match(/^title:\s*(.+)$/m)?.[1] ?? md.match(/^# (.+)$/m)[1];
  // Tables scroll sideways on a phone instead of widening the page.
  const html = marked.parse(md).replace(/<table>/g, '<div class="table-wrap"><table>').replace(/<\/table>/g, "</table></div>");
  return { title, html };
}

const NAV = [
  ["GUIDE.html", "Guide"],
  ["PRIVACY.html", "Privacy"],
  [REPO, "GitHub"],
];
const nav = (current) =>
  `<ul class="nav">${NAV.map(([href, label]) => `<li><a href="${href}"${href === current ? ' aria-current="page"' : ""}>${label}</a></li>`).join("")}</ul>`;

function layout({ file, title, description, body }) {
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${esc(title)}</title>
<meta name="description" content="${esc(description)}">
<meta property="og:title" content="${esc(title)}">
<meta property="og:description" content="${esc(description)}">
<meta property="og:image" content="${SITE}screenshots/light.jpg">
<meta property="og:url" content="${SITE}${file === "index.html" ? "" : file}">
<meta name="color-scheme" content="light dark">
<link rel="icon" href="favicon.png" sizes="32x32" type="image/png">
<link rel="apple-touch-icon" href="icon.png">
<link rel="preload" href="fonts/mr-eaves-small-caps.woff2" as="font" type="font/woff2" crossorigin>
<link rel="preload" href="fonts/bookinsanity-regular.woff2" as="font" type="font/woff2" crossorigin>
<link rel="stylesheet" href="style.css">
</head>
<body>
<a class="skip" href="#main">Skip to content</a>
<header class="band">
  <div class="wrap bar">
    <a class="brand" href="./"${file === "index.html" ? ' aria-current="page"' : ""}><img src="icon.png" width="36" height="36" alt="">Lorekeeper</a>
    <nav aria-label="Site">${nav(file)}</nav>
  </div>
</header>
<main id="main">
${body}
</main>
<footer class="band">
  <div class="wrap foot">
    <nav aria-label="Footer">${nav(file)}</nav>
    <p>Set in Solbera's D&amp;D 5e fonts (<a href="https://creativecommons.org/licenses/by-sa/4.0/">CC BY-SA 4.0</a>, <a href="${REPO}/blob/main/src/fonts/README.md">credits and changes</a>).</p>
    <p>Lorekeeper is a fan-made, open-source app. Not affiliated with Wizards of the Coast or D&amp;D Beyond.</p>
  </div>
</footer>
</body>
</html>
`;
}

const FEATURES = [
  ["quote", "Notes from anywhere", `<p>Press <kbd>⌘⌥N</kbd> (<kbd>Ctrl+Alt+N</kbd>) mid-fight and a one-line note box opens over Discord or any other app. <kbd>⌘⇧S</kbd> saves the text you selected.</p>`],
  [
    "npc",
    "Notes sort themselves",
    `<p>Start a note with a symbol and it lands in the right pile:</p>
<ul class="prefixes">
${[["@", "npc", "NPC"], ["#", "loot", "Loot"], ["!", "quest", "Quest"], ["?", "mystery", "Mystery"], ['"', "quote", "Quote"]]
  .map(([key, kind, label]) => `<li><kbd>${esc(key)}</kbd><span class="kind kind-${kind}">${icon(kind)}${label}</span></li>`)
  .join("\n")}
</ul>`,
  ],
  ["session", "Sessions as a timeline", `<p>Every note keeps its time, so a session reads like the tale it was. <strong>Copy for D&amp;D Beyond</strong> turns it into a tidy recap for the party's shared journal.</p>`],
  ["location", "The campaign vault", `<p>NPCs, PCs, places, items, factions, quests and lore, joined by <code>[[links]]</code> with backlinks on every page. It's all plain Markdown, so Obsidian opens it as a vault.</p>`],
  ["loot", "Backups, your way", `<p>To a folder, Dropbox, Google Drive or a private GitHub repository, straight from your computer to your own account. No servers, no accounts, <a href="PRIVACY.html">no analytics</a>.</p>`],
  ["home", "Tome or Dungeon", `<p>Aged parchment and red rubrics by day, torchlit stone and candle gold by night, set in the 5e book fonts. It follows your system, or pick one.</p>`],
];

const DOWNLOADS = [
  ["macOS", ".dmg, Apple silicon and Intel"],
  ["Windows", "setup .exe"],
  ["Linux", ".AppImage"],
];

const home = `<section class="hero wrap" aria-labelledby="title">
  <img class="app-icon" src="icon.png" width="128" height="128" alt="">
  <h1 class="title" id="title">Lorekeeper</h1>
  <hr class="fleuron">
  <p class="pitch">Session notes for D&amp;D players</p>
  <p class="lede">Jot things down mid-game without leaving Discord, keep a vault of NPCs, places and quests, and paste a tidy recap into your D&amp;D Beyond journal.</p>
  <div class="downloads" id="download">
${DOWNLOADS.map(([os, file]) => `    <a class="seal" href="${RELEASES}">Download for ${os}<small>${file}</small></a>`).join("\n")}
  </div>
  <p class="fine">Free and open source. The builds aren't code-signed yet, so macOS and Windows warn the first time you open them: <a href="GUIDE.html#installing">here's how to get past it</a>.</p>
</section>
<figure class="plate">
  <div class="frame">
    <picture>
      <source srcset="screenshots/dark.jpg" media="(prefers-color-scheme: dark)">
      <img src="screenshots/light.jpg" width="1120" height="740" alt="Lorekeeper with a campaign open: a session's notes on a timeline, tagged NPC, Loot, Mystery and Quest, and the campaign's factions, places, NPCs and quests in the sidebar.">
    </picture>
  </div>
  <figcaption>Tome by day, Dungeon by night: Lorekeeper follows your system's light or dark mode.</figcaption>
</figure>
<section class="section wrap" aria-labelledby="features">
  <h2 class="section-title" id="features">What's in the tome</h2>
  <hr class="fleuron">
  <div class="cards">
${FEATURES.map(([kind, title, text]) => `    <article class="card">\n      <h3>${icon(kind)}${title}</h3>\n      ${text}\n    </article>`).join("\n")}
  </div>
</section>
<section class="section closing wrap" aria-labelledby="ready">
  <h2 class="section-title" id="ready">Ready for the next session?</h2>
  <p>Get the latest release for macOS, Windows or Linux, then read the <a href="GUIDE.html">guide</a> while the DM sets up.</p>
  <a class="seal" href="${RELEASES}">Download Lorekeeper<small>latest release on GitHub</small></a>
</section>`;

rmSync(out, { recursive: true, force: true });
mkdirSync(new URL("fonts/", out), { recursive: true });

// The app's fonts and both themes' tokens: everything in styles.css before its first layout rule.
const appCss = read("src/styles.css");
const cut = appCss.indexOf("* { box-sizing");
if (cut < 0) throw new Error("build-site: theme tokens not found in src/styles.css");
writeFileSync(new URL("style.css", out), appCss.slice(0, cut) + read("scripts/site.css"));

const pages = {
  "index.html": { title: "Lorekeeper: session notes for D&D players", description: "Free desktop app for D&D session notes: jot things down mid-game, keep a campaign vault, paste a recap into D&D Beyond.", body: home },
};
for (const [file, description] of [
  ["GUIDE.md", "How to install and use Lorekeeper: hotkeys, note prefixes, sessions, the vault, backups and settings."],
  ["PRIVACY.md", "Lorekeeper's privacy policy: no servers, no accounts, no analytics. Backups go straight to your own accounts."],
]) {
  const { title, html } = doc(file);
  const name = file.replace(/\.md$/, ".html"); // keep the published upper-case URLs (PRIVACY.html is on Google's consent screen)
  pages[name] = { title: title.includes("Lorekeeper") ? title : `${title} - Lorekeeper`, description, body: `<article class="prose">\n${html}</article>` };
}
for (const [file, page] of Object.entries(pages)) writeFileSync(new URL(file, out), layout({ file, ...page }));

for (const f of readdirSync(new URL("src/fonts/", root)).filter((f) => f.endsWith(".woff2") || f.startsWith("LICENSE"))) {
  cpSync(new URL(`src/fonts/${f}`, root), new URL(`fonts/${f}`, out));
}
cpSync(new URL("docs/screenshots/", root), new URL("screenshots/", out), { recursive: true });
cpSync(new URL("src-tauri/icons/128x128@2x.png", root), new URL("icon.png", out));
cpSync(new URL("src-tauri/icons/32x32.png", root), new URL("favicon.png", out));
console.log(`Site written to ${out.pathname}`);
