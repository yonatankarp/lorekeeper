// The theme from Settings: "light" or "dark" overrides the OS, "system" follows it (see the tokens in styles.css).
export function applyTheme(theme) {
  if (theme === "light" || theme === "dark") document.documentElement.dataset.theme = theme;
  else delete document.documentElement.dataset.theme;
}
