// The theme from Settings: "light", "dark", "tome" or "dungeon" overrides the OS, "system" follows it (see the tokens in styles.css).
// Tome and Dungeon also set data-ornate, which turns on the shared D&D styling (fonts, textures, ornaments).
const NATIVE = { light: "light", dark: "dark", tome: "light", dungeon: "dark" };

export function applyTheme(theme) {
  const root = document.documentElement;
  if (NATIVE[theme]) root.dataset.theme = theme;
  else delete root.dataset.theme;
  root.toggleAttribute("data-ornate", theme === "tome" || theme === "dungeon");
}

/** The native window appearance for a theme ("light" / "dark"), or null to follow the OS. */
export const nativeTheme = (theme) => NATIVE[theme] ?? null;
