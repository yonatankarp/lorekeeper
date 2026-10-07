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
  let html = marked.parse(md).replace(/<table>/g, '<div class="table-wrap"><table>').replace(/<\/table>/g, "</table></div>");
  // A long page gets a contents list under its title.
  const sections = [...html.matchAll(/<h2 id="([^"]+)">(.*?)<\/h2>/g)];
  if (sections.length >= 4) {
    const toc = `<nav class="toc" aria-labelledby="toc-title"><p class="toc-title" id="toc-title">Contents</p><ol>${sections.map(([, id, text]) => `<li><a href="#${id}">${text}</a></li>`).join("")}</ol></nav>\n`;
    html = html.replace(/<\/h1>\n/, (end) => end + toc);
  }
  return { title, html };
}

const NAV = [
  ["./#download", "Download"],
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
  ["session", "Sessions as a timeline", `<p>Every note keeps its time, so a session reads like the tale it was. The Journal groups it into a recap: who you met, what you found, the quests and mysteries. <strong>Copy for D&amp;D Beyond</strong> puts it on your clipboard.</p>`],
  ["location", "The campaign vault", `<p>NPCs, PCs, places, items, factions, quests and lore, joined by <code>[[links]]</code> with backlinks on every page. Paste in maps and handouts. It's all plain Markdown, so Obsidian opens it as a vault.</p>`],
  ["loot", "Backups, your way", `<p>To a folder, Dropbox, Google Drive or a private GitHub repository, straight from your computer to your own account. No accounts, <a href="PRIVACY.html">no analytics</a>.</p>`],
  ["home", "Tome or Dungeon", `<p>Aged parchment and red rubrics by day, torchlit stone and candle gold by night, set in the 5e book fonts. It follows your system, or pick one.</p>`],
];

// Built but not in a release yet. When NEXT ships, move these into FEATURES (and drop "coming in" from the FAQ).
const NEXT = "0.7";
const SOON = [
  ["pc", "Play with your party", `<p>Share a campaign with an invite link and everyone's notes land in one session timeline, each with who wrote it, and private notes stay with you (and the DM, if your party allows it). It syncs end-to-end encrypted, so the server can't read a word.</p>`],
  ["faction", "Characters and connections", `<p>Import the whole party from D&amp;D Beyond: class, level, background and portrait, refreshed after a level up. Home maps your NPCs, places and factions and who's linked to whom.</p>`],
  ["note", "A cheat sheet", `<p>Every note symbol, link and shortcut on one page: <strong>Help &gt; Cheat Sheet</strong>, or <kbd>⌘/</kbd> (<kbd>Ctrl+/</kbd>).</p>`],
];

const STEPS = [
  ["Install it", `Download Lorekeeper and open it. It waits in the menu bar (the system tray on Windows and Linux) and keeps itself up to date.`],
  ["Jot during the game", `Press <kbd>⌘⌥N</kbd> (<kbd>Ctrl+Alt+N</kbd>), type, press Enter. You're back in Discord before the next initiative roll.`],
  ["Read it back", `Each session reads as a timeline, or as a Journal recap of who you met and what you found, ready to paste into D&amp;D Beyond.`],
];

const FAQ = [
  ["Is it really free?", `Yes. Lorekeeper is free and <a href="${REPO}">open source</a>: no ads, no subscription, no account.`],
  ["Why does my computer warn me when I open it?", `The builds aren't code-signed yet, so macOS and Windows don't recognise who made them. <a href="GUIDE.html#installing">The guide shows how to open it anyway</a>; you only have to do it once.`],
  ["Where are my notes kept?", `On your computer, as plain Markdown files in <code>Documents/Lorekeeper</code>, a folder for each campaign. They only leave your computer if you turn on a backup or share the campaign.`],
  ["Does it work with Obsidian?", `Yes. Open the notes folder as an Obsidian vault and the <strong>Obsidian</strong> button opens pages there. Links, embedded images and templates work the Obsidian way.`],
  ["Do I need to be online?", `No. Your notes are on your computer, so Lorekeeper works offline. Backups and updates wait for a connection.`],
  ["Does my party need Lorekeeper too?", `Not for your own notes. To share a campaign (coming in ${NEXT}), everyone who writes in it needs Lorekeeper and an invite link from whoever shared it. There's no account to make; the first time you share, ask Lorekeeper's maintainer for the server's creation key.`],
  ["What does Lorekeeper collect?", `Nothing. No analytics, no crash reports, no accounts. The <a href="PRIVACY.html">privacy policy</a> has the details.`],
  ["Which computers does it run on?", `macOS (Apple silicon and Intel), 64-bit Windows, and Linux as an AppImage. On Linux, the global shortcuts need an X11 session.`],
];

// [os, label, what you get, the release asset's name ends with]
const DOWNLOADS = [
  ["mac", "macOS", ".dmg, Apple silicon and Intel", "_universal.dmg"],
  ["windows", "Windows", "setup .exe", "_x64-setup.exe"],
  ["linux", "Linux", ".AppImage", "_amd64.AppImage"],
];

// Puts the visitor's system first and links straight to its file in the latest release. Without JavaScript, or if
// GitHub doesn't answer, every button opens the release page. Phones and unknown systems keep all three buttons.
const downloadScript = `<script>
(() => {
  const box = document.getElementById("downloads");
  const ua = navigator.userAgent;
  // iPads say they're Macs, but have a touch screen.
  const os = /iPhone|iPad|iPod|Android|CrOS/.test(ua) || (/Mac/.test(ua) && navigator.maxTouchPoints > 1) ? null
    : /Mac/.test(ua) ? "mac" : /Win/.test(ua) ? "windows" : /Linux|X11/.test(ua) ? "linux" : null;
  const mine = os && box.querySelector('[data-os="' + os + '"]');
  if (mine) {
    box.classList.add("picked");
    box.prepend(mine);
    const also = document.createElement("p");
    also.className = "also";
    also.append("Also for ");
    [...box.querySelectorAll(".seal")].filter((a) => a !== mine).forEach((a, i) => {
      if (i) also.append(" and ");
      a.className = "other";
      a.firstChild.textContent = a.dataset.label;
      a.querySelector("small").remove();
      also.append(a);
    });
    box.append(also);
  }
  fetch("https://api.github.com/repos/yonatankarp/lorekeeper/releases/latest")
    .then((r) => (r.ok ? r.json() : Promise.reject(r.status)))
    .then((release) => {
      for (const a of box.querySelectorAll("[data-suffix]")) {
        const asset = release.assets.find((f) => f.name.endsWith(a.dataset.suffix));
        if (!asset) continue;
        a.href = asset.browser_download_url;
        const small = a.querySelector("small");
        if (small) small.textContent = release.tag_name.replace(/^v/, "Version ") + " · " + small.textContent;
      }
    })
    .catch(() => {});
})();
</script>`;

const cards = (list, extra = "") =>
  list.map(([kind, title, text]) => `    <article class="card">\n      <h3>${icon(kind)}${title}</h3>\n      ${text}${extra}\n    </article>`).join("\n");

const home = `<section class="hero wrap" aria-labelledby="title">
  <img class="app-icon" src="icon.png" width="128" height="128" alt="">
  <h1 class="title" id="title">Lorekeeper</h1>
  <hr class="fleuron">
  <p class="pitch">Session notes for D&amp;D players</p>
  <p class="lede">Jot things down mid-game without leaving Discord, keep a vault of NPCs, places and quests, and read every session back as a tale.</p>
  <div class="downloads" id="download">
    <div class="seals" id="downloads">
${DOWNLOADS.map(([os, label, file, suffix]) => `      <a class="seal" href="${RELEASES}" data-os="${os}" data-label="${label}" data-suffix="${suffix}">Download for ${label}<small>${file}</small></a>`).join("\n")}
    </div>
    <p class="fine">Free and open source. The builds aren't code-signed yet, so macOS and Windows warn the first time you open them: <a href="GUIDE.html#installing">here's how to get past it</a>.</p>
  </div>
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
<section class="section wrap" aria-labelledby="how">
  <h2 class="section-title" id="how">How it works</h2>
  <hr class="fleuron">
  <ol class="steps">
${STEPS.map(([title, text]) => `    <li><h3>${title}</h3><p>${text}</p></li>`).join("\n")}
  </ol>
</section>
<section class="section wrap" aria-labelledby="features">
  <h2 class="section-title" id="features">What's in the tome</h2>
  <hr class="fleuron">
  <div class="cards">
${cards(FEATURES)}
  </div>
</section>
${SOON.length ? `<section class="section wrap" aria-labelledby="soon">
  <h2 class="section-title" id="soon">Coming in ${NEXT}</h2>
  <hr class="fleuron">
  <p class="section-lede">Built and on their way to the next release. The <a href="GUIDE.html">guide</a> already covers them.</p>
  <div class="cards soon">
${cards(SOON)}
  </div>
</section>` : ""}
<section class="section wrap" aria-labelledby="faq">
  <h2 class="section-title" id="faq">Questions</h2>
  <hr class="fleuron">
  <div class="faq">
${FAQ.map(([q, a]) => `    <details><summary>${q}</summary><p>${a}</p></details>`).join("\n")}
  </div>
</section>
<section class="section closing wrap" aria-labelledby="ready">
  <h2 class="section-title" id="ready">Ready for the next session?</h2>
  <p>Get the latest release for macOS, Windows or Linux, then read the <a href="GUIDE.html">guide</a> while the DM sets up.</p>
  <a class="seal" href="#download">Download Lorekeeper<small>free, for macOS, Windows and Linux</small></a>
</section>
${downloadScript}`;

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
  ["GUIDE.md", "How to install and use Lorekeeper: shortcuts, note prefixes, sessions, the vault, playing with your party, backups and settings."],
  ["PRIVACY.md", "Lorekeeper's privacy policy: no accounts, no analytics. Shared campaigns are end-to-end encrypted; backups go straight to your own accounts."],
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
