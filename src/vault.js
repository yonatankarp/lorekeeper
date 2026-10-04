// Pure vault logic: tree, [[link]] resolution, backlinks, search, templates. No DOM, tested in node.

// [[Target]], [[Target#Heading]], [[Target|Label]], ![[Embed]]
export const WIKILINK = /(!?)\[\[([^\]|#]*)(#[^\]|]*)?(?:\|([^\]]*))?\]\]/g;

export const baseName = (path) => path.split("/").pop().replace(/\.md$/i, "");
const naturally = (a, b) => a.localeCompare(b, undefined, { numeric: true, sensitivity: "base" });

/** Splits leading `---` properties from the body. Values are kept as raw strings. */
export function splitFrontmatter(md) {
  const m = md.match(/^---\r?\n([\s\S]*?)\r?\n---[ \t]*(?:\r?\n|$)/);
  if (!m) return { props: [], body: md };
  const props = m[1]
    .split(/\r?\n/)
    .map((line) => line.match(/^([^:#\s][^:]*):\s?(.*)$/))
    .filter(Boolean)
    .map(([, key, value]) => [key.trim(), value.trim()]);
  return { props, body: md.slice(m[0].length) };
}

/** Obsidian-style: [[Name]] matches a file name anywhere, [[Folder/Name]] a path suffix; shortest path wins. */
export function resolve(target, paths) {
  const t = target.trim().replace(/\.md$/i, "").toLowerCase();
  if (!t) return null;
  const hits = paths.filter((p) => {
    const q = p.replace(/\.md$/i, "").toLowerCase();
    return q === t || q.endsWith("/" + t);
  });
  return hits.sort((a, b) => a.length - b.length || naturally(a, b))[0] ?? null;
}

/** Nested { name, path, dirs, files } from folder paths and note paths. Sessions/ lists newest first. */
export function buildTree(folders, paths) {
  const root = { name: "", path: "", dirs: [], files: [] };
  const dirAt = (path) => {
    let node = root;
    for (const part of path.split("/").filter(Boolean)) {
      let next = node.dirs.find((d) => d.name === part);
      if (!next) {
        next = { name: part, path: node.path ? `${node.path}/${part}` : part, dirs: [], files: [] };
        node.dirs.push(next);
      }
      node = next;
    }
    return node;
  };
  folders.forEach(dirAt);
  for (const p of paths) dirAt(p.split("/").slice(0, -1).join("/")).files.push(p);
  const sort = (node) => {
    node.dirs.sort((a, b) => naturally(a.name, b.name)).forEach(sort);
    const order = node.path === "Sessions" ? -1 : 1;
    node.files.sort((a, b) => order * naturally(baseName(a), baseName(b)));
  };
  sort(root);
  return root;
}

/** Notes linking to `path`, each with the lines that contain the link. */
export function backlinks(path, notes) {
  const paths = notes.map((n) => n.path);
  const result = [];
  for (const note of notes) {
    if (note.path === path) continue;
    const lines = note.content.split("\n").filter((line) =>
      [...line.matchAll(WIKILINK)].some((m) => resolve(m[2], paths) === path),
    );
    if (lines.length) result.push({ path: note.path, lines: lines.map((l) => l.trim()) });
  }
  return result.sort((a, b) => naturally(a.path, b.path));
}

/** Case-insensitive search over names and content; name matches first. */
export function search(query, notes) {
  const q = query.trim().toLowerCase();
  if (!q) return [];
  const hits = [];
  for (const note of notes) {
    const inName = baseName(note.path).toLowerCase().includes(q);
    const line = note.content.split("\n").find((l) => l.toLowerCase().includes(q));
    if (inName || line) hits.push({ path: note.path, inName, snippet: line?.trim() ?? "" });
  }
  return hits.sort((a, b) => b.inName - a.inName || naturally(a.path, b.path));
}

/** Obsidian template variables. */
export const fillTemplate = (template, title, date) =>
  template.replaceAll("{{title}}", title).replaceAll("{{date}}", date);

/** Template "NPC" goes to folder "NPCs" when it exists, else the vault root. */
export const folderFor = (templateName, folders) =>
  folders.find((f) => f.toLowerCase() === `${templateName}s`.toLowerCase()) ?? "";

/** Characters that break Obsidian links or file names. Returns an error message, or "" when fine. */
export function badName(name) {
  if (!name.trim()) return "Give the page a name.";
  if (/[\\/:*?"<>|#^[\]]/.test(name)) return 'Names can\'t contain \\ / : * ? " < > | # ^ [ ]';
  if (name.trim().startsWith(".")) return "Names can't start with a dot.";
  return "";
}
