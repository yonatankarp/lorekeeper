// Pure text helpers for the Markdown editor (src/editor.js). No DOM, tested in node.

/** Page a [[link]] points at: "Mirela#Past|she" -> "Mirela". */
export const linkTarget = (inner) => inner.split(/[|#]/)[0].trim();

/** Toolbar Heading button: none -> H1 -> H2 -> H3 -> none. */
export function cycleHeading(line) {
  const [mark, hashes = ""] = line.match(/^(#{1,6}) +/) ?? [""];
  const next = hashes.length >= 3 ? "" : "#".repeat(hashes.length + 1) + " ";
  return next + line.slice(mark.length);
}

const MARKER = /^(\s*)([-*+] \[[ xX]\] |[-*+] |> )?/;

/** Toolbar list/checkbox/quote buttons: toggles `prefix` ("- ", "- [ ] ", "> "), replacing another marker. */
export function togglePrefix(line, prefix) {
  const [all, indent, marker = ""] = line.match(MARKER);
  const kind = (m) => (m.startsWith(">") ? ">" : m.includes("[") ? "task" : m ? "-" : "");
  return indent + (kind(marker) === kind(prefix) ? "" : prefix) + line.slice(all.length);
}

/**
 * A page name being typed just before the cursor: "@Mir" or an unclosed "[[Mir" (or a bare "[[").
 * Returns where the name starts in `before` and whether "[[" is already typed, else null.
 */
export function nameQuery(before) {
  const m = before.match(/\[\[([^[\]|#\n]*)$/) || before.match(/(?:^|\s)@([^\s@[\]]+)$/);
  return m && { from: before.length - m[1].length, link: m[0].includes("[[") };
}

/** The smallest single change turning `a` into `b`, so cursors and undo history outside it stay put. */
export function diff(a, b) {
  const max = Math.min(a.length, b.length);
  let start = 0;
  while (start < max && a[start] === b[start]) start++;
  let end = 0;
  while (end < max - start && a[a.length - 1 - end] === b[b.length - 1 - end]) end++;
  return { from: start, to: a.length - end, insert: b.slice(start, b.length - end) };
}
