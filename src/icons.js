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
  // d20, face on
  session: `<path d="M8 1.5l5.6 3.25v6.5L8 14.5l-5.6-3.25v-6.5z"/><path d="M8 4.75l3.1 5.35H4.9z"/><path d="M2.4 4.75 8 4.75l5.6 0M4.9 10.1l-2.5 1.15M11.1 10.1l2.5 1.15M4.9 10.1 8 14.5l3.1-4.4M8 1.5v3.25"/>`,
};

/** `<svg>` markup for a kind ("npc", "loot", "quest", "mystery", "quote") or "session", hidden from screen readers. */
export const icon = (name) =>
  `<svg class="icon" viewBox="0 0 16 16" aria-hidden="true" focusable="false">${PATHS[name] ?? ""}</svg>`;
