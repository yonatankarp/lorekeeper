// Classic script in <head>, so it runs before the first paint: tags <html> with the OS for platform-scoped CSS.
(() => {
  const ua = navigator.userAgent;
  document.documentElement.dataset.platform = ua.includes("Mac") ? "macos" : ua.includes("Windows") ? "windows" : "linux";
})();
