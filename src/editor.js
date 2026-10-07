// Live-preview Markdown editor (CodeMirror 6), Obsidian-style. The document is always the file's
// exact text: decorations only change how it looks, so nothing is ever reformatted.
import {
  acceptCompletion, Annotation, autocompletion, Decoration, defaultKeymap, deleteMarkupBackward, EditorSelection,
  EditorState, EditorView, history, historyKeymap, indentLess, indentMore, insertNewlineContinueMarkup, keymap,
  Language, LanguageSupport, markdownLanguage, placeholder, redo, startCompletion, syntaxTree, Transaction, undo,
  ViewPlugin, WidgetType,
} from "./vendor/codemirror.js";
import { PREFIX as KINDS } from "./notes.js";
import { splitFrontmatter } from "./vault.js";
import { cycleHeading, diff, linkTarget, nameQuery, togglePrefix } from "./editor-text.js";
import { readable } from "./cheatsheet.js";

const external = Annotation.define(); // setValue/append: text from disk, not the user's edit
const quiet = [external.of(true), Transaction.addToHistory.of(false)]; // and undo never removes it

// [[Target|Label]] as WikiLink nodes, parsed before Link (which would read "[Target|Label]" as a link).
const wikiLinks = {
  defineNodes: ["WikiLink", "WikiLinkMark"],
  parseInline: [{
    name: "WikiLink",
    before: "Link",
    parse(cx, next, pos) {
      if (next !== 91 || cx.char(pos + 1) !== 91) return -1;
      const m = /^\[\[[^[\]\n]+\]\]/.exec(cx.slice(pos, cx.end));
      if (!m) return -1;
      const end = pos + m[0].length;
      return cx.addElement(cx.elt("WikiLink", pos, end, [cx.elt("WikiLinkMark", pos, pos + 2), cx.elt("WikiLinkMark", end - 2, end)]));
    },
  }],
};
// Shares markdownLanguage's data, so its list commands (Enter, Backspace) still recognise it.
const markdown = new Language(markdownLanguage.data, markdownLanguage.parser.configure([wikiLinks]), [], "markdown");

class Checkbox extends WidgetType {
  constructor(checked) {
    super();
    this.checked = checked;
  }
  eq(other) {
    return other.checked === this.checked;
  }
  toDOM() {
    const box = document.createElement("input");
    box.type = "checkbox";
    box.className = "cm-md-task";
    box.checked = this.checked;
    box.tabIndex = -1; // keyboard users edit the [ ] text
    box.setAttribute("aria-label", "Done");
    return box;
  }
  ignoreEvent() {
    return false;
  }
}

/** Line numbers touched by the cursor or selection: their Markdown marks stay visible. */
function activeLines(state) {
  const lines = new Set();
  for (const r of state.selection.ranges)
    for (let l = state.doc.lineAt(r.from).number; l <= state.doc.lineAt(r.to).number; l++) lines.add(l);
  return lines;
}

const hidden = Decoration.replace({});
const dimmed = Decoration.mark({ class: "cm-md-mark" });
const mark = (cls, attributes) => Decoration.mark({ class: cls, attributes });
const lineClass = (cls) => Decoration.line({ class: cls });

function decorate(view) {
  const { state } = view;
  const { doc } = state;
  const active = activeLines(state);
  const isActive = (pos) => active.has(doc.lineAt(pos).number);
  const out = [];
  const add = (deco, from, to = from) => from <= to && out.push(deco.range(from, to));
  // Dimmed near the cursor, hidden elsewhere. Never hidden across a line break (plugins may not replace those).
  const markup = (from, to) => add(isActive(from) || doc.lineAt(from).to < to ? dimmed : hidden, from, to);
  const lines = (from, to, cls) => {
    for (let pos = from; pos <= to; ) {
      const line = doc.lineAt(pos);
      add(lineClass(cls), line.from);
      pos = line.to + 1;
    }
  };
  const spaceAfter = (pos) => (doc.sliceString(pos, pos + 1) === " " ? pos + 1 : pos);

  // Frontmatter is shown as is: muted monospace, never hidden.
  const fmEnd = doc.sliceString(0, 3) === "---" ? doc.length - splitFrontmatter(doc.toString()).body.length : 0;
  if (fmEnd) lines(0, fmEnd - 1, "cm-md-frontmatter");

  for (const { from, to } of view.visibleRanges) {
    syntaxTree(state).iterate({
      from,
      to,
      enter(n) {
        if (n.name !== "Document" && n.from < fmEnd) return false;
        const heading = /^ATXHeading(\d)$/.exec(n.name);
        if (heading) return add(lineClass(`cm-md-h${heading[1]}`), doc.lineAt(n.from).from);
        switch (n.name) {
          case "HeaderMark":
            if (n.node.parent?.name.startsWith("ATX")) markup(n.from, spaceAfter(n.to));
            break;
          case "StrongEmphasis": add(mark("cm-md-strong"), n.from, n.to); break;
          case "Emphasis": add(mark("cm-md-em"), n.from, n.to); break;
          case "Strikethrough": add(mark("cm-md-strike"), n.from, n.to); break;
          case "InlineCode": add(mark("cm-md-code"), n.from, n.to); break;
          case "EmphasisMark":
          case "StrikethroughMark":
            markup(n.from, n.to);
            break;
          case "CodeMark":
            if (n.node.parent?.name === "InlineCode") markup(n.from, n.to);
            else add(dimmed, n.from, n.to); // code fences stay visible
            break;
          case "CodeInfo": add(dimmed, n.from, n.to); break;
          case "FencedCode": lines(n.from, n.to, "cm-md-codeblock"); break;
          case "Blockquote": lines(n.from, n.to, "cm-md-quote"); break;
          case "QuoteMark": markup(n.from, spaceAfter(n.to)); break;
          case "HorizontalRule":
            if (!isActive(n.from)) add(lineClass("cm-md-hr"), doc.lineAt(n.from).from);
            markup(n.from, n.to);
            break;
          case "Link": {
            // [text](url) and [text][ref]; a bare [text] is just brackets.
            const marks = n.node.getChildren("LinkMark");
            if (marks.length < 3 && !n.node.getChild("LinkLabel")) return false;
            add(mark("cm-md-link"), marks[0].to, marks[1].from);
            markup(n.from, marks[0].to);
            markup(marks[1].from, n.to);
            return false;
          }
          case "Image": return false;
          case "Autolink":
          case "URL":
            add(mark("cm-md-link"), n.from, n.to);
            return false;
          case "WikiLink": {
            const inner = doc.sliceString(n.from + 2, n.to - 2);
            const pipe = inner.indexOf("|");
            add(mark("cm-md-wikilink", { "data-target": linkTarget(inner) }), n.from, n.to);
            markup(n.from, pipe < 0 ? n.from + 2 : n.from + 3 + pipe); // [[Target| shows only the label
            markup(n.to - 2, n.to);
            return false;
          }
          case "TaskMarker": {
            const checked = doc.sliceString(n.from + 1, n.from + 2) !== " ";
            if (checked) add(mark("cm-md-done"), n.to, n.node.parent.to);
            if (!isActive(n.from)) add(Decoration.replace({ widget: new Checkbox(checked) }), n.from, n.to);
            break;
          }
          case "ListMark": {
            // Hotkey notes: "- 20:15 @Mirela": muted time, tinted prefix.
            if (!/[-*+]/.test(doc.sliceString(n.from, n.to))) break;
            const rest = doc.sliceString(n.to, doc.lineAt(n.to).to);
            const time = /^ (\d{1,2}:\d{2}) /.exec(rest);
            if (time) add(mark("cm-md-time"), n.to + 1, n.to + 1 + time[1].length);
            const p = n.to + (time ? time[0].length : 1);
            const kind = KINDS[doc.sliceString(p, p + 1)];
            if (kind) add(mark(`cm-md-kind kind-${kind}`), p, p + 1);
            break;
          }
        }
      },
    });
  }
  return Decoration.set(out, true);
}

const preview = ViewPlugin.fromClass(
  class {
    constructor(view) {
      this.decorations = decorate(view);
    }
    update(u) {
      if (u.docChanged || u.selectionSet || u.viewportChanged || syntaxTree(u.startState) !== syntaxTree(u.state))
        this.decorations = decorate(u.view);
    }
  },
  { decorations: (v) => v.decorations },
);

// ---------- commands ----------

/** Cmd+B / Cmd+I: wraps each selection in `marker` ("**" or "*"), or unwraps it when already wrapped. */
const wrap = (marker) => (view) => {
  const { state } = view;
  const n = marker.length;
  // Wrapped in this marker: n stars each side, or 3 (bold italic). Stops Cmd+I from eating half of "**bold**".
  const lead = (s) => /^\**/.exec(s)[0].length;
  const trail = (s) => /\**$/.exec(s)[0].length;
  const wrapped = (left, right) => left === right && (left === n || left === 3);
  view.dispatch(state.changeByRange((r) => {
    const text = state.sliceDoc(r.from, r.to);
    if (text.length > 2 * lead(text) && wrapped(lead(text), trail(text)))
      return { changes: { from: r.from, to: r.to, insert: text.slice(n, -n) }, range: EditorSelection.range(r.from, r.to - 2 * n) };
    if (wrapped(trail(state.sliceDoc(Math.max(0, r.from - 3), r.from)), lead(state.sliceDoc(r.to, r.to + 3))))
      return { changes: [{ from: r.from - n, to: r.from }, { from: r.to, to: r.to + n }], range: EditorSelection.range(r.from - n, r.to - n) };
    return { changes: [{ from: r.from, insert: marker }, { from: r.to, insert: marker }], range: EditorSelection.range(r.from + n, r.to + n) };
  }), { userEvent: "input", scrollIntoView: true });
  return true;
};

/** Rewrites every selected line with `fn`, changing only the part that differs. */
const eachLine = (fn) => (view) => {
  const { state } = view;
  const changes = [];
  for (const number of activeLines(state)) {
    const line = state.doc.line(number);
    const d = diff(line.text, fn(line.text));
    if (d.from !== d.to || d.insert) changes.push({ from: line.from + d.from, to: line.from + d.to, insert: d.insert });
  }
  const set = state.changes(changes);
  view.dispatch({ changes: set, selection: state.selection.map(set, 1), userEvent: "input" }); // cursor lands after a new prefix
  return true;
};

/** Toolbar Link: "[[]]" (or "[[selection]]") with the cursor inside, then page suggestions. */
function insertLink(view) {
  view.dispatch(view.state.changeByRange((r) => ({
    changes: [{ from: r.from, insert: "[[" }, { from: r.to, insert: "]]" }],
    range: EditorSelection.cursor(r.to + 2),
  })), { userEvent: "input" });
  startCompletion(view);
  return true;
}

const LIST_LINE = /^\s*([-*+]|\d+[.)]) /;
const inList = (view) => LIST_LINE.test(view.state.doc.lineAt(view.state.selection.main.head).text);

/** "@Mir" -> "@[[Mirela]]", "[[Mir" -> "[[Mirela]]" from the current page names. */
const pageCompletions = (pageNames) => (cx) => {
  const line = cx.state.doc.lineAt(cx.pos);
  const q = nameQuery(cx.state.sliceDoc(line.from, cx.pos));
  if (!q) return null;
  const apply = (name) => (view, _completion, from, to) => {
    const close = q.link && view.state.sliceDoc(to, to + 2) === "]]" ? 2 : 0; // the toolbar's [[]]
    const insert = (q.link ? "" : "[[") + name + "]]";
    view.dispatch({ changes: { from, to: to + close, insert }, selection: { anchor: from + insert.length }, userEvent: "input.complete" });
  };
  return {
    from: line.from + q.from,
    options: [...new Set(pageNames())].map((name) => ({ label: name, apply: apply(name) })),
    validFor: q.link ? /^[^[\]|#\n]*$/ : /^[^\s@[\]]*$/,
  };
};

const TOOLS = [
  ["Bold", "CmdOrCtrl+B", "B", wrap("**")],
  ["Italic", "CmdOrCtrl+I", "I", wrap("*")],
  ["Heading", "", "H", eachLine(cycleHeading)],
  ["Bulleted list", "", "•", eachLine((t) => togglePrefix(t, "- "))],
  ["Checkbox", "", "☐", eachLine((t) => togglePrefix(t, "- [ ] "))],
  ["Link", "", "[[ ]]", insertLink],
  ["Quote", "", "❝", eachLine((t) => togglePrefix(t, "> "))],
];

/** Formatting buttons; arrow keys move between them (one Tab stop). */
function toolbar(view) {
  const bar = document.createElement("div");
  bar.className = "editor-toolbar";
  bar.setAttribute("role", "toolbar");
  bar.setAttribute("aria-label", "Formatting");
  for (const [label, shortcut, text, run] of TOOLS) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "ghost";
    b.textContent = text;
    b.title = shortcut ? `${label} (${readable(shortcut, document.documentElement.dataset.platform === "macos")})` : label;
    b.setAttribute("aria-label", label);
    b.tabIndex = bar.children.length ? -1 : 0;
    b.addEventListener("click", () => {
      run(view);
      view.focus();
    });
    bar.append(b);
  }
  bar.addEventListener("keydown", (e) => {
    const buttons = [...bar.children];
    const step = { ArrowRight: 1, ArrowLeft: -1 }[e.key];
    if (!step) return;
    const next = buttons[(buttons.indexOf(document.activeElement) + step + buttons.length) % buttons.length];
    buttons.forEach((b) => (b.tabIndex = b === next ? 0 : -1));
    next.focus();
  });
  return bar;
}

/**
 * Mounts the toolbar and editor in `parent`.
 * onChange(): the user edited the text. onFollowLink(target): a [[link]] was clicked.
 * pageNames(): page names for [[ and @ suggestions.
 * onImage(file, pasted): saves a pasted or dropped image, resolving to the name to embed (null: not saved).
 */
export function createEditor(parent, { onChange, onFollowLink, pageNames, onImage }) {
  /** Saves each image, then inserts ![[name]] at `at()` (asked after saving, since the text may have changed), one after another. */
  async function embedImages(view, files, pasted, at) {
    let next = null;
    for (const file of files) {
      const name = await onImage(file, pasted);
      if (!name) continue;
      const pos = Math.min(next ?? at(), view.state.doc.length);
      const insert = `![[${name}]]`;
      view.dispatch({ changes: { from: pos, insert }, selection: { anchor: pos + insert.length }, userEvent: "input" });
      next = pos + insert.length;
    }
  }
  const extensions = [
    new LanguageSupport(markdown),
    EditorState.lineSeparator.of("\n"), // keep \r\n files byte-for-byte
    history(),
    EditorView.lineWrapping,
    EditorView.contentAttributes.of({ "aria-label": "Page text", spellcheck: "true" }),
    placeholder("Start writing. Type [[ or @ to link a page."),
    autocompletion({ override: [pageCompletions(pageNames)], icons: false }),
    keymap.of([
      { key: "Mod-b", run: wrap("**") },
      { key: "Mod-i", run: wrap("*") },
      { key: "Enter", run: insertNewlineContinueMarkup },
      { key: "Backspace", run: deleteMarkupBackward },
      { key: "Tab", run: (v) => acceptCompletion(v) || (inList(v) && indentMore(v)), shift: (v) => inList(v) && indentLess(v) },
      // ⌘⇧K jumps to search, ⌘[ / ⌘] go Back / Forward and ⌘/ opens the cheat sheet (app.js), so CodeMirror doesn't take them.
      ...defaultKeymap.filter((b) => !["Shift-Mod-k", "Mod-[", "Mod-]", "Mod-/"].includes(b.key)),
      // Undo and redo come through app.js (keys and the Edit menu alike), which calls undo() / redo() below.
      ...historyKeymap.filter((b) => b.run !== undo && b.run !== redo),
    ]),
    preview,
    EditorView.domEventHandlers({
      mousedown(e, view) {
        if (e.target.matches?.("input.cm-md-task")) {
          const pos = view.posAtDOM(e.target);
          const insert = view.state.sliceDoc(pos + 1, pos + 2) === " " ? "x" : " ";
          view.dispatch({ changes: { from: pos + 1, to: pos + 2, insert }, userEvent: "input" });
          e.preventDefault();
          return true;
        }
        const link = e.target.closest?.(".cm-md-wikilink");
        if (!link) return false;
        const onActiveLine = activeLines(view.state).has(view.state.doc.lineAt(view.posAtDOM(link)).number);
        if (onActiveLine && !e.metaKey && !e.ctrlKey) return false;
        e.preventDefault();
        onFollowLink(link.dataset.target);
        return true;
      },
      // Pasted or dropped images are saved into the vault and embedded where they land, as Obsidian does.
      paste(e, view) {
        const file = [...(e.clipboardData?.items ?? [])].find((i) => i.kind === "file" && i.type.startsWith("image/"))?.getAsFile();
        if (!file || !onImage) return false;
        e.preventDefault();
        embedImages(view, [file], true, () => view.state.selection.main.head);
        return true;
      },
      drop(e, view) {
        const files = [...(e.dataTransfer?.files ?? [])].filter((f) => f.type.startsWith("image/"));
        if (!files.length || !onImage) return false;
        e.preventDefault();
        const pos = view.posAtCoords({ x: e.clientX, y: e.clientY }) ?? view.state.selection.main.head;
        embedImages(view, files, false, () => Math.min(pos, view.state.doc.length));
        return true;
      },
    }),
    EditorView.updateListener.of((u) => {
      if (u.docChanged && u.transactions.some((tr) => !tr.annotation(external))) onChange();
    }),
  ];
  const view = new EditorView({ extensions });
  parent.replaceChildren(toolbar(view), view.dom);

  return {
    getValue: () => view.state.doc.toString(),
    /** Replaces the text, keeping the cursor and undo history. `reset` starts fresh (another page). */
    setValue(text, { reset = false } = {}) {
      if (reset) return view.setState(EditorState.create({ doc: text, extensions }));
      const d = diff(view.state.doc.toString(), text);
      if (d.from !== d.to || d.insert) view.dispatch({ changes: d, annotations: quiet });
    },
    /** Adds text at the end (hotkey notes) without moving the cursor or entering undo history. */
    append(text) {
      view.dispatch({ changes: { from: view.state.doc.length, insert: text }, annotations: quiet });
    },
    focus: () => view.focus(),
    undo: () => undo(view),
    redo: () => redo(view),
    setFontSize(px) {
      parent.style.setProperty("--editor-font-size", `${px}px`);
      view.requestMeasure();
    },
    destroy() {
      view.destroy();
      parent.replaceChildren();
    },
  };
}
