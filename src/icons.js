// Monochrome note-kind icons (original drawings, 16x16, stroked in currentColor). Decorative: labels carry the meaning.
const PATHS = {
  // hooded figure, face in shadow
  npc: `<path d="M8 1.5C6 1.5 4.6 3.6 4.4 6.5c-.1 1.5-.8 2.5-2.2 4.1-.6.7-.7 1.9-.7 3.9h13c0-2-.1-3.2-.7-3.9-1.4-1.6-2.1-2.6-2.2-4.1C11.4 3.6 10 1.5 8 1.5z"/><path d="M4.6 9.6c1 1.9 2.1 2.9 3.4 3s2.4-1.1 3.4-3"/><path d="M8 4.6c-1.3 0-2 1.4-2 3s.9 3 2 3.4c1.1-.4 2-1.8 2-3.4s-.7-3-2-3z" fill="currentColor"/>`,
  // coin pouch
  loot: `<path d="M6.1 4.6 4.9 2.4c-.2-.4.1-.9.6-.9h5c.5 0 .8.5.6.9L9.9 4.6"/><path d="M6.1 4.6h3.8"/><path d="M6.1 4.6C3.6 6 2.6 8.7 2.8 11.4c.1 2 2.1 3.1 5.2 3.1s5.1-1.1 5.2-3.1c.2-2.7-.8-5.4-3.3-6.8"/><circle cx="8" cy="10.3" r="1.6"/>`,
  // rolled scroll
  quest: `<path d="M4.5 3h7.75a1.5 1.5 0 0 1 0 3H11v6.5a2 2 0 0 1-2 2H3.25a1.5 1.5 0 0 1 0-3H4.5z"/><path d="M4.5 3a1.5 1.5 0 0 1 1.5 1.5V6h5M4.5 11.5V3M7 8.25h2.25M7 10.5h2.25"/>`,
  // open eye
  mystery: `<path d="M1.5 8c1.8-3 4-4.5 6.5-4.5s4.7 1.5 6.5 4.5c-1.8 3-4 4.5-6.5 4.5S3.3 11 1.5 8z"/><circle cx="8" cy="8" r="2.3"/><circle cx="8" cy="8" r=".8" fill="currentColor"/>`,
  // quill
  quote: `<path d="M14 2c-4.6.2-7.9 3.2-8.9 8.4l.6.6c1.8-.3 3.4-1 4.7-2.2l-1.6-.3 2.5-1.3C12.6 5.7 13.6 4 14 2z"/><path d="M2 14l6.5-6.5"/>`,
  // short sword, point up
  pc: `<path d="M13.5 2.5h-2.6L5.6 7.8l2.6 2.6 5.3-5.3z"/><path d="M3.8 7.6l4.6 4.6M5.4 10.6 2.6 13.4"/>`,
  // crenellated tower with a door
  location: `<path d="M3.5 14.5V2.5H5.5V4.5h1.5v-2h2v2h1.5v-2h2v12z"/><path d="M6.8 14.5v-2.8a1.2 1.2 0 0 1 2.4 0v2.8"/>`,
  // round potion flask
  item: `<path d="M6.3 1.5h3.4M7 1.5v4.1a4.5 4.5 0 1 0 2 0V1.5"/><path d="M4 10.5h8"/>`,
  // padlock: private notes, and Make private
  lock: `<rect x="3.5" y="7" width="9" height="7.5" rx="1.2"/><path d="M5.5 7V5a2.5 2.5 0 0 1 5 0v2"/><circle cx="8" cy="10.6" r=".9" fill="currentColor"/>`,
  // open padlock: Make shared
  unlock: `<rect x="3.5" y="7" width="9" height="7.5" rx="1.2"/><path d="M5.5 7V5a2.5 2.5 0 0 1 4.9-.7"/><circle cx="8" cy="10.6" r=".9" fill="currentColor"/>`,
  // pennant on a pole
  faction: `<path d="M3.5 14.5v-13M3.5 2.5h9l-2 3 2 3h-9"/>`,
  // closed tome with a clasp
  lore: `<path d="M3.5 2.5a1 1 0 0 1 1-1h8v11h-8a1 1 0 0 0 0 2h8"/><path d="M3.5 2.5v11M12.5 6h-2v2.5h2"/>`,
  // page with a folded corner
  note: `<path d="M3.5 1.5h6l3 3v10h-9z"/><path d="M9.5 1.5v3h3M5.5 8h5M5.5 10.5h5"/>`,
  // quest outcomes in the sidebar
  done: `<path d="M3 8.5l3 3 7-7"/>`,
  failed: `<path d="M4.5 4.5l7 7M11.5 4.5l-7 7"/>`,
  // hourglass: a quest in progress
  progress: `<path d="M4 1.5h8M4 14.5h8M5 1.5c0 3 3 4 3 6.5S5 11.5 5 14.5M11 1.5c0 3-3 4-3 6.5s3 3.5 3 6.5"/><path d="M6.2 13.2 8 11.6l1.8 1.6z" fill="currentColor"/>`,
  // coin, coloured by its metal (.coin-gp and so on): rewards
  coin: `<circle cx="8" cy="8" r="6"/><circle cx="8" cy="8" r="3.6"/>`,
  // open book: Home, the campaign's contents
  home: `<path d="M8 3.6C6.4 2.5 4.3 2 1.5 2.2v10.4c2.8-.2 4.9.3 6.5 1.4 1.6-1.1 3.7-1.6 6.5-1.4V2.2C11.7 2 9.6 2.5 8 3.6z"/><path d="M8 3.6V14"/>`,
  // Back / Forward
  back: `<path d="M10 3 5 8l5 5"/>`,
  forward: `<path d="M6 3l5 5-5 5"/>`,
  // pencil: Edit
  edit: `<path d="M11 2.5l2.5 2.5-8 8H3v-2.5z"/><path d="M9.5 4l2.5 2.5"/>`,
  // four corners: fit the map
  fit: `<path d="M2.5 6V2.5H6M10 2.5h3.5V6M13.5 10v3.5H10M6 13.5H2.5V10"/>`,
  // circling arrow: refresh
  refresh: `<path d="M13 8a5 5 0 1 1-1.5-3.6"/><path d="M11.8 1.8v2.8H9"/>`,
  // two sheets: copy
  copy: `<rect x="5.5" y="5.5" width="8" height="8" rx="1"/><path d="M10.5 5.5v-3h-8v8h3"/>`,
  // arrow out of a box: open in the browser
  external: `<path d="M9 2.5h4.5V7M13.5 2.5 7.5 8.5"/><path d="M11.5 9.5v4h-9v-9h4"/>`,
  // folder: show in Finder / Explorer
  folder: `<path d="M1.5 3.5h4.5l1.5 1.5h7v8.5h-13z"/>`,
  // cross: dismiss, turn off
  close: `<path d="M4.5 4.5l7 7M11.5 4.5l-7 7"/>`,
  // waste bin: delete
  trash: `<path d="M2.5 4.5h11M6.5 4.5V2.5h3v2M4 4.5l.7 9h6.6l.7-9M6.7 7v4.5M9.3 7v4.5"/>`,
  // d20, face on
  session: `<path d="M8 1.5l5.6 3.25v6.5L8 14.5l-5.6-3.25v-6.5z"/><path d="M8 4.75l3.1 5.35H4.9z"/><path d="M2.4 4.75 8 4.75l5.6 0M4.9 10.1l-2.5 1.15M11.1 10.1l2.5 1.15M4.9 10.1 8 14.5l3.1-4.4M8 1.5v3.25"/>`,
};

/** `<svg>` markup for a name above (note kinds, page types, "session", "done", "failed", actions), hidden from screen readers. */
export const icon = (name) =>
  `<svg class="icon" viewBox="0 0 16 16" aria-hidden="true" focusable="false">${PATHS[name] ?? ""}</svg>`;
