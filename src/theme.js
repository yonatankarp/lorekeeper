// The theme from Settings: "light" (Tome) or "dark" (Dungeon) overrides the OS, "system" follows it (tokens in styles.css).
export function applyTheme(theme) {
  const root = document.documentElement;
  if (theme === "light" || theme === "dark") root.dataset.theme = theme;
  else delete root.dataset.theme;
}

/** The native window appearance for a theme ("light" / "dark"), or null to follow the OS. */
export const nativeTheme = (theme) => (theme === "light" || theme === "dark" ? theme : null);
