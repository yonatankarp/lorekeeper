// Classic script in <head>, so it runs before the first paint: tags <html> with the OS for platform-scoped CSS.
(() => {
  const root = document.documentElement;
  const ua = navigator.userAgent;
  root.dataset.platform = ua.includes("Mac") ? "macos" : ua.includes("Windows") ? "windows" : "linux";
  // The Mica window effect (tauri.conf.json) exists only on Windows 11, which reports platformVersion 13 or later.
  if (root.dataset.platform === "windows") {
    navigator.userAgentData?.getHighEntropyValues(["platformVersion"]).then(
      (v) => { if (parseInt(v.platformVersion, 10) >= 13) root.dataset.mica = ""; },
      () => {},
    );
  }
})();
