// Images in the vault (maps, handouts), Obsidian-style: ![[map.png]], ![[map.png|300]], ![alt](path). No DOM, tested in node.
import { naturally } from "./vault.js";

/** The image types Lorekeeper shows and saves; the same list as IMAGE_EXTS in lib.rs. */
export const isImage = (name) => /\.(png|jpe?g|gif|webp|svg)$/i.test(name.trim());

/** Image embeds, for the D&D Beyond export (local files can't paste there): ![[map.png|300]] and ![alt](path). */
export const IMAGE_EMBED = /!\[\[[^\]|#]*\.(?:png|jpe?g|gif|webp|svg)\s*(?:[|#][^\]]*)?\]\]|!\[[^\]]*\]\([^)]*\)/gi;

/** Where pasted and dropped images go. */
export const ATTACHMENTS = "Attachments";

/**
 * The vault image `target` points at, Obsidian-style: an exact vault-relative path, else a path suffix ("map.png",
 * "Maps/map.png"), preferring an Attachments folder, then the shortest path. Case-insensitive; leading ./ ../ / are
 * dropped, so a relative link still finds the file by name. Markdown links may be %-encoded ("Pasted%20image.png").
 */
export function resolveImage(target, images) {
  let t = target.trim();
  try {
    t = decodeURIComponent(t);
  } catch {} // a stray % is just part of the name
  t = t.replace(/^(?:\.{1,2}\/|\/)+/, "").toLowerCase();
  if (!t || !isImage(t)) return null;
  const exact = (p) => p.toLowerCase() === t;
  const attached = (p) => /(^|\/)attachments\//i.test(p);
  const hits = images.filter((p) => exact(p) || p.toLowerCase().endsWith("/" + t));
  return hits.sort((a, b) => exact(b) - exact(a) || attached(b) - attached(a) || a.length - b.length || naturally(a, b))[0] ?? null;
}

/** An image property's value as a path to look up: "[[Attachments/Demus.jpg|200]]" (quoted or not) is "Attachments/Demus.jpg". */
export const imageTarget = (value) => value.trim().replace(/^["']|["']$/g, "").replace(/^!?\[\[|\]\]$/g, "").split("|")[0].trim();

/** "300", "300x200" or "Map|300" -> { alt, width, height }; any other label is all alt text. Sizes are digits only. */
export function imageLabel(label, fallbackAlt = "") {
  const m = /^(?:(.*)\|)?\s*(\d+)(?:x(\d+))?\s*$/.exec(label);
  if (!m) return { alt: label, width: "", height: "" };
  return { alt: m[1] ?? fallbackAlt, width: m[2], height: m[3] ?? "" };
}

const EXT = { "image/png": "png", "image/jpeg": "jpg", "image/gif": "gif", "image/webp": "webp", "image/svg+xml": "svg" };

/** Obsidian's name for a pasted image: "Pasted image 20261005143012.png" (local time); null for a type not allowed (HEIC, TIFF...). */
export function pastedName(date, type) {
  if (!EXT[type]) return null;
  const pad = (n) => String(n).padStart(2, "0");
  const stamp = `${date.getFullYear()}${pad(date.getMonth() + 1)}${pad(date.getDate())}${pad(date.getHours())}${pad(date.getMinutes())}${pad(date.getSeconds())}`;
  return `Pasted image ${stamp}.${EXT[type]}`;
}

/** A dropped file's own name, minus characters that break [[links]] or file names. */
export const safeName = (name) => name.replace(/[\\/:*?"<>|#^[\]]/g, "-").trim();

/** `name`, or "name 1.png", "name 2.png"... the first one no image in the vault already has (any folder, any case). */
export function freeName(name, images) {
  const taken = new Set(images.map((p) => p.split("/").pop().toLowerCase()));
  for (let i = 0; ; i++) {
    const n = i ? name.replace(/(\.[^.]+)$/, ` ${i}$1`) : name;
    if (!taken.has(n.toLowerCase())) return n;
  }
}
