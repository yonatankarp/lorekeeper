// The connections map (graph.js) drawn on a <canvas>: each page as its picture, else its kind's icon; people round,
// places and factions square. Zoomed out it's coloured dots. Scrolling or a pinch zooms, dragging moves, a click opens.
// Pictures are shrunk to thumbnails once and kept in the app's cache folder (lib.rs).
import { layout } from "./graph.js";
import { baseName } from "./vault.js";
import { icon } from "./icons.js";

const { invoke } = window.__TAURI__.core;
const R = 22; // a node's radius in layout units
const THUMB = 96; // thumbnail size in pixels
const PEOPLE = new Set(["npc", "pc"]);

// ---------- pictures ----------

/**
 * Image path -> thumbnail, or null when it can't be decoded (SVG in some webviews, a broken file): the icon then.
 * ponytail: an image replaced under the same name keeps its old thumbnail here until restart (the cache on disk
 * notices); key by modified time if that bites.
 */
const pictures = new Map();
const queue = [];
let loading = false;
let campaign = 0; // forgetPictures() moves on: a picture still loading then belongs to the last campaign

/** A vault image's thumbnail: the cached one, else made from the image once and cached. */
async function thumbnail(path) {
  const cached = await invoke("thumbnail", { path });
  if (cached.byteLength) return createImageBitmap(new Blob([cached]));
  const full = await createImageBitmap(new Blob([await invoke("read_image", { path })]));
  const side = Math.min(full.width, full.height);
  const c = document.createElement("canvas");
  c.width = c.height = THUMB;
  // A square from the middle across and nearer the top down, so portraits keep their faces.
  c.getContext("2d").drawImage(full, (full.width - side) / 2, (full.height - side) / 4, side, side, 0, 0, THUMB, THUMB);
  full.close();
  const data = c.toDataURL("image/png"); // PNG: token art with a transparent background stays transparent
  invoke("save_thumbnail", { path, data: data.slice(data.indexOf(",") + 1) }).catch(() => {}); // the cache only saves time
  return c;
}

/** One image at a time, so a big campaign never holds many full-size pictures at once. */
async function loadPictures() {
  if (loading) return;
  loading = true;
  while (queue.length) {
    const path = queue.shift(), from = campaign;
    const pic = await thumbnail(path).catch(() => null);
    if (from === campaign) pictures.set(path, pic);
    redrawAll();
  }
  loading = false;
}

/** The thumbnail to draw for an image, or undefined while it's still being made. */
function picture(path) {
  if (!pictures.has(path) && !queue.includes(path)) {
    queue.push(path);
    loadPictures();
  }
  return pictures.get(path);
}

/** Another campaign's "Attachments/Map.png" is another picture. */
export function forgetPictures() {
  pictures.clear();
  queue.length = 0;
  campaign++;
}

const icons = new Map(); // kind and colour -> <img> of its icon

/** A page kind's icon as an image, in `colour`; null until it has loaded. */
function kindIcon(kind, colour) {
  const key = `${kind} ${colour}`;
  if (!icons.has(key)) {
    const img = new Image();
    img.onload = redrawAll;
    // icons.js styles its icons with CSS; an image needs the stroke spelled out.
    const svg = icon(kind).replace('<svg class="icon"', `<svg xmlns="http://www.w3.org/2000/svg" color="${colour}" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"`);
    img.src = `data:image/svg+xml,${encodeURIComponent(svg)}`;
    icons.set(key, img);
  }
  const img = icons.get(key);
  return img.complete && img.naturalWidth ? img : null;
}

// ---------- drawing ----------

const layouts = new Map(); // pages and links -> positions; the same map re-drawn never moves
const views = new Map(); // which map ("home" or a page) -> its zoom and pan, kept while the page re-renders
const maps = new Map(); // canvas -> { graph, pos, view, focus, onOpen, hover, at }
const ro = new ResizeObserver((entries) => entries.forEach((e) => draw(e.target)));

function positions({ nodes, edges }) {
  const key = JSON.stringify([nodes.map((n) => n.path), edges]);
  if (!layouts.has(key)) {
    if (layouts.size > 20) layouts.clear();
    layouts.set(key, layout(nodes, edges));
  }
  return layouts.get(key);
}

let frame = 0;
function redrawAll() {
  frame ||= requestAnimationFrame(() => {
    frame = 0;
    for (const canvas of maps.keys()) draw(canvas);
  });
}
matchMedia("(prefers-color-scheme: dark)").addEventListener("change", redrawAll);
document.fonts?.ready.then(redrawAll);

/** The colour behind the canvas, for the halo that keeps labels readable over links. */
function backdrop(el) {
  for (; el; el = el.parentElement) {
    const bg = getComputedStyle(el).backgroundColor;
    if (bg && bg !== "transparent" && !/rgba\(.*,\s*0\)$/.test(bg)) return bg;
  }
  return "transparent";
}

/**
 * Draws `graph` ({ nodes, edges } from graph.js) on `canvas`, and keeps it drawn as pictures arrive and the canvas
 * resizes. `focus` is the open page (ringed); `onOpen(path)` runs when a page is clicked.
 */
export function drawGraph(canvas, graph, { focus = null, onOpen }) {
  if (!maps.has(canvas)) {
    listen(canvas);
    ro.observe(canvas);
  }
  const id = focus ?? "home";
  if (!views.has(id)) views.set(id, { zoom: 1, panX: 0, panY: 0 });
  maps.set(canvas, { ...maps.get(canvas), graph, pos: positions(graph), view: views.get(id), focus, onOpen });
  draw(canvas);
}

function draw(canvas) {
  const m = maps.get(canvas);
  if (!m) return;
  if (!canvas.isConnected) {
    ro.unobserve(canvas);
    return maps.delete(canvas);
  }
  const w = canvas.clientWidth, h = canvas.clientHeight, dpr = devicePixelRatio || 1;
  if (!w || !h) return; // hidden
  if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(h * dpr)) {
    canvas.width = Math.round(w * dpr);
    canvas.height = Math.round(h * dpr);
  }
  const ctx = canvas.getContext("2d");
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, w, h);
  const css = getComputedStyle(canvas);
  const token = (name) => css.getPropertyValue(name).trim();
  const [fg, muted, ring, panel, font] = ["--fg", "--muted", "--accent-text", "--panel", "--font-ui"].map(token);

  // Fit the whole map, leaving room for the labels, then apply the zoom and pan.
  const pts = [...m.pos.values()];
  const minX = Math.min(...pts.map((p) => p.x)) - R - 40, maxX = Math.max(...pts.map((p) => p.x)) + R + 40;
  const minY = Math.min(...pts.map((p) => p.y)) - R - 4, maxY = Math.max(...pts.map((p) => p.y)) + R + 22;
  const fitX = (w - 16) / (maxX - minX), fitY = (h - 16) / (maxY - minY);
  const scale = Math.min(fitX, fitY, 1.5);
  // A wide or tall frame spreads the map that way too (up to 1.6 times), rather than leaving it empty.
  const sx = Math.min(fitX, scale * 1.6) * m.view.zoom, sy = Math.min(fitY, scale * 1.6) * m.view.zoom;
  const cx = (minX + maxX) / 2, cy = (minY + maxY) / 2;
  m.at = (path) => {
    const p = m.pos.get(path);
    return { x: (p.x - cx) * sx + w / 2 + m.view.panX, y: (p.y - cy) * sy + h / 2 + m.view.panY };
  };
  const r = R * scale * m.view.zoom;
  const button = fitButton(canvas);
  if (button) button.disabled = m.view.zoom === 1 && !m.view.panX && !m.view.panY;
  const dots = r < 9; // too small for a picture or a name
  m.r = dots ? Math.max(r, 3) : r;

  ctx.strokeStyle = muted;
  ctx.globalAlpha = 0.45;
  ctx.lineWidth = dots ? 1 : 1.5;
  ctx.beginPath();
  for (const [a, b] of m.graph.edges) {
    const p = m.at(a), q = m.at(b);
    ctx.moveTo(p.x, p.y);
    ctx.lineTo(q.x, q.y);
  }
  ctx.stroke();
  ctx.globalAlpha = 1;

  const halo = backdrop(canvas);
  const size = Math.round(Math.min(14, Math.max(11, r * 0.55)));
  for (const n of m.graph.nodes) {
    const { x, y } = m.at(n.path);
    const s = m.r;
    const shape = () => {
      ctx.beginPath();
      if (PEOPLE.has(n.kind)) ctx.arc(x, y, s, 0, 2 * Math.PI);
      else ctx.roundRect(x - s, y - s, 2 * s, 2 * s, s * 0.3);
    };
    const marked = n.path === m.focus || n.path === m.hover;
    if (dots) {
      shape();
      ctx.fillStyle = marked ? ring : PEOPLE.has(n.kind) ? fg : muted;
      ctx.fill();
      continue;
    }
    shape();
    ctx.fillStyle = panel;
    ctx.fill();
    const pic = n.image && picture(n.image);
    if (pic) {
      ctx.save();
      shape();
      ctx.clip();
      ctx.drawImage(pic, x - s, y - s, 2 * s, 2 * s);
      ctx.restore();
    } else {
      const img = kindIcon(n.kind, muted);
      if (img) ctx.drawImage(img, x - s * 0.6, y - s * 0.6, s * 1.2, s * 1.2);
    }
    shape();
    ctx.lineWidth = marked ? 3 : 1.5;
    ctx.strokeStyle = marked ? ring : muted;
    ctx.stroke();

    ctx.font = `${n.path === m.focus ? "700 " : ""}${size}px ${font}`;
    ctx.textAlign = "center";
    ctx.textBaseline = "top";
    ctx.lineJoin = "round";
    ctx.lineWidth = 4;
    ctx.strokeStyle = halo;
    ctx.strokeText(baseName(n.path), x, y + s + 4);
    ctx.fillStyle = marked ? ring : fg;
    ctx.fillText(baseName(n.path), x, y + s + 4);
  }
}

/** Hover names a page, a click opens it, dragging moves the map, scrolling or a pinch zooms, double-click resets. */
function listen(canvas) {
  const m = () => maps.get(canvas);
  const local = (e) => {
    const box = canvas.getBoundingClientRect();
    return [e.clientX - box.left, e.clientY - box.top];
  };
  const hit = ([x, y]) => m().graph.nodes.find((n) => {
    const p = m().at?.(n.path);
    return p && Math.hypot(p.x - x, p.y - y) <= m().r + 3;
  });
  let drag = null;
  canvas.addEventListener("pointerdown", (e) => {
    m().view.fitting = null;
    drag = { x: e.clientX, y: e.clientY, panX: m().view.panX, panY: m().view.panY, moved: false };
    canvas.setPointerCapture(e.pointerId);
  });
  canvas.addEventListener("pointermove", (e) => {
    if (drag) {
      const dx = e.clientX - drag.x, dy = e.clientY - drag.y;
      drag.moved ||= Math.hypot(dx, dy) > 4;
      if (drag.moved) {
        Object.assign(m().view, { panX: drag.panX + dx, panY: drag.panY + dy });
        draw(canvas);
      }
      return;
    }
    const n = hit(local(e));
    if (n?.path === m().hover) return;
    m().hover = n?.path;
    canvas.style.cursor = n ? "pointer" : "";
    canvas.title = n ? baseName(n.path) : "";
    draw(canvas);
  });
  canvas.addEventListener("pointerup", (e) => {
    const was = drag;
    drag = null;
    const n = was && !was.moved && hit(local(e));
    if (n) m().onOpen(n.path);
  });
  canvas.addEventListener("pointerleave", () => {
    if (drag || !m().hover) return;
    m().hover = undefined;
    draw(canvas);
  });
  /** Zooms to `to` (kept between 0.5 and 8), the point under the cursor staying put. */
  const zoomAt = (e, to) => {
    const v = m().view;
    v.fitting = null;
    const zoom = Math.min(8, Math.max(0.5, to));
    const [x, y] = local(e);
    const mx = x - canvas.clientWidth / 2, my = y - canvas.clientHeight / 2;
    const k = zoom / v.zoom;
    Object.assign(v, { zoom, panX: mx - (mx - v.panX) * k, panY: my - (my - v.panY) * k });
    draw(canvas);
  };
  // WebKit (the macOS app) sends a trackpad pinch as gesture events, with the scale since the pinch began.
  let pinch = null; // the zoom when the pinch began
  canvas.addEventListener("gesturestart", (e) => { e.preventDefault(); pinch = m().view.zoom; });
  canvas.addEventListener("gesturechange", (e) => { e.preventDefault(); if (pinch !== null) zoomAt(e, pinch * e.scale); });
  canvas.addEventListener("gestureend", (e) => { e.preventDefault(); pinch = null; });
  // Scrolling over the map zooms it (Chromium, WebView2 on Windows, and Linux send a pinch as Ctrl+scroll too).
  canvas.addEventListener("wheel", (e) => {
    e.preventDefault(); // nor the page scrolling; app.js keeps Ctrl+scroll from zooming the whole window
    if (pinch !== null) return; // the gesture events have it
    // A pinch or trackpad sends many small steps, a mouse wheel notch one big one: capped, a notch zooms about 1.3 times.
    const step = Math.max(-25, Math.min(25, e.deltaY));
    zoomAt(e, m().view.zoom * Math.exp(-step * 0.01));
  }, { passive: false });
  // Glides back to the whole map (zoom eased evenly, as a ratio); at once when reduced motion is asked for.
  const fit = () => {
    const v = m().view, from = { ...v }, start = performance.now(), ms = 300;
    const run = (v.fitting = {}); // a newer fit, a drag or a pinch takes over
    const step = (now) => {
      if (v.fitting !== run) return;
      const t = matchMedia("(prefers-reduced-motion: reduce)").matches ? 1 : Math.min(1, (now - start) / ms);
      const e = 1 - (1 - t) ** 3; // ease out
      Object.assign(v, { zoom: from.zoom ** (1 - e), panX: from.panX * (1 - e), panY: from.panY * (1 - e) });
      if (t === 1) Object.assign(v, { zoom: 1, panX: 0, panY: 0, fitting: null });
      draw(canvas);
      if (t < 1) requestAnimationFrame(step);
    };
    requestAnimationFrame(step);
  };
  canvas.addEventListener("dblclick", fit);
  fitButton(canvas)?.addEventListener("click", fit);
}

/** The Fit button next to a map, if it has one; disabled while the whole map already fits. */
const fitButton = (canvas) => canvas.parentElement?.querySelector(".graph-fit");
