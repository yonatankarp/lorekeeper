// The connections map: which people, places and factions link to each other, the picture each one shows, and where
// each sits. No DOM, tested in node; graph-view.js draws it.
import { WIKILINK, kindOf, naturally, resolve, splitFrontmatter } from "./vault.js";
import { IMAGE_EMBED, imageTarget, resolveImage } from "./images.js";

/** The kinds of page on the map. Sessions link to everything and would tangle it; quests, items and lore are left out. */
export const GRAPH_KINDS = ["npc", "pc", "location", "faction"];

/** The image an embed points at: ![[map.png|300]] or ![alt](path). */
const embedTarget = (embed) => (embed.startsWith("![[") ? embed.slice(3, -2).split(/[|#]/)[0] : embed.slice(embed.indexOf("](") + 2, -1));

/** A page's picture: its `portrait` property, else the first image in its text that's in the vault; null for none. */
export function pageImage(content, images) {
  const { props, body } = splitFrontmatter(content);
  const portrait = props.find(([k]) => k.toLowerCase() === "portrait")?.[1];
  const targets = [portrait && imageTarget(portrait), ...[...body.matchAll(IMAGE_EMBED)].map(([m]) => embedTarget(m))];
  for (const t of targets) {
    const path = t && resolveImage(t, images);
    if (path) return path;
  }
  return null;
}

/**
 * The map's pages ({ path, kind, image }, by path) and links ([from, to], each pair once, either direction).
 * ponytail: resolve() scans every path per link, O(links × pages); index names in a Map if big vaults feel slow.
 */
export function connections(notes, images) {
  const paths = notes.map((n) => n.path);
  const shown = notes.filter((n) => GRAPH_KINDS.includes(kindOf(n.path))).sort((a, b) => naturally(a.path, b.path));
  const nodes = shown.map((n) => ({ path: n.path, kind: kindOf(n.path), image: pageImage(n.content, images) }));
  const onMap = new Set(nodes.map((n) => n.path));
  const seen = new Set();
  const edges = [];
  for (const n of shown) {
    for (const [, embed, target] of n.content.matchAll(WIKILINK)) {
      const to = !embed && resolve(target, paths);
      const key = to && [n.path, to].sort().join("\n");
      if (!to || to === n.path || !onMap.has(to) || seen.has(key)) continue;
      seen.add(key);
      edges.push([n.path, to]);
    }
  }
  return { nodes, edges };
}

/** `path` and the pages one link away, with the links between them; no nodes when `path` isn't on the map. */
export function neighbourhood({ nodes, edges }, path) {
  const near = new Set([path]);
  for (const [a, b] of edges) if (a === path) near.add(b); else if (b === path) near.add(a);
  const kept = nodes.filter((n) => near.has(n.path));
  return kept.some((n) => n.path === path) ? { nodes: kept, edges: edges.filter(([a, b]) => near.has(a) && near.has(b)) } : { nodes: [], edges: [] };
}

/**
 * Where each node sits (path -> { x, y }, in node radii of about 22): a force layout from a fixed spiral start, so
 * the same pages and links always land in the same places and re-drawing never makes the map jump.
 * ponytail: every pair repels every tick, O(pages²); fine to a few hundred pages, use a quadtree past that.
 */
export function layout(nodes, edges, ticks = 300) {
  const at = new Map(nodes.map((n, i) => [n.path, i]));
  const p = nodes.map((_, i) => {
    const r = 40 * Math.sqrt(i + 0.5), a = i * 2.399963; // golden angle
    return { x: r * Math.cos(a), y: r * Math.sin(a), vx: 0, vy: 0 };
  });
  const links = edges.map(([a, b]) => [at.get(a), at.get(b)]);
  for (let t = 0; t < ticks; t++) {
    const heat = 1 - t / ticks;
    for (let i = 0; i < p.length; i++) {
      for (let j = i + 1; j < p.length; j++) {
        const dx = p[j].x - p[i].x, dy = p[j].y - p[i].y;
        const f = (800 * heat) / Math.max(dx * dx + dy * dy, 1);
        p[i].vx -= dx * f; p[i].vy -= dy * f;
        p[j].vx += dx * f; p[j].vy += dy * f;
      }
    }
    for (const [i, j] of links) {
      const dx = p[j].x - p[i].x, dy = p[j].y - p[i].y;
      const d = Math.hypot(dx, dy) || 1, f = ((d - 80) / d) * 0.1 * heat;
      p[i].vx += dx * f; p[i].vy += dy * f;
      p[j].vx -= dx * f; p[j].vy -= dy * f;
    }
    for (const q of p) {
      q.vx -= q.x * 0.1 * heat;
      q.vy -= q.y * 0.1 * heat;
      const v = Math.hypot(q.vx, q.vy), max = 40;
      if (v > max) (q.vx *= max / v), (q.vy *= max / v);
      q.x += q.vx;
      q.y += q.vy;
      q.vx *= 0.5;
      q.vy *= 0.5;
    }
  }
  return new Map(nodes.map((n, i) => [n.path, { x: p[i].x, y: p[i].y }]));
}
