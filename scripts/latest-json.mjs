// Writes the updater's latest.json for a release from its uploaded bundles and their .sig files, in the shape
// tauri-action makes. release.yml runs it once, after every platform has built, so parallel builds can't overwrite
// each other's platforms. Needs the gh CLI (GH_TOKEN in CI).
// Usage: node scripts/latest-json.mjs <owner/repo> <release id> [--upload]   (without --upload it prints the JSON)
import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";

const [repo, id, upload] = process.argv.slice(2);
const gh = (...args) => execFileSync("gh", args, { encoding: "utf8", maxBuffer: 1 << 26 });
const release = JSON.parse(gh("api", `repos/${repo}/releases/${id}`));

/** The updater entry for the bundle whose name ends with `suffix`: its signature (the .sig file's text) and URL. */
function entry(suffix) {
  const bundle = release.assets.find((a) => a.name.endsWith(suffix));
  const sig = release.assets.find((a) => a.name === `${bundle?.name}.sig`);
  if (!bundle || !sig) throw new Error(`release ${release.tag_name} has no ${suffix} with a .sig`);
  return { signature: gh("api", "-H", "Accept: application/octet-stream", `repos/${repo}/releases/assets/${sig.id}`), url: bundle.url };
}

// The universal macOS app serves both chips; the keys are the ones tauri-action writes.
const mac = entry(".app.tar.gz"), windows = entry("-setup.exe"), linux = entry(".AppImage");
const keys = (names, value) => Object.fromEntries(names.map((n) => [n, value]));
const json = JSON.stringify({
  version: release.tag_name.replace(/^v/, ""),
  notes: release.body ?? "",
  pub_date: new Date().toISOString(),
  platforms: {
    ...keys(["darwin-aarch64", "darwin-x86_64", "darwin-universal", "darwin-aarch64-app", "darwin-x86_64-app", "darwin-universal-app"], mac),
    ...keys(["windows-x86_64", "windows-x86_64-nsis"], windows),
    ...keys(["linux-x86_64", "linux-x86_64-appimage"], linux),
  },
}, null, 2);

if (upload !== "--upload") {
  console.log(json);
} else {
  // A re-run replaces the file it wrote before.
  const old = release.assets.find((a) => a.name === "latest.json");
  if (old) gh("api", "-X", "DELETE", `repos/${repo}/releases/assets/${old.id}`);
  writeFileSync("latest.json", json);
  gh("api", "-X", "POST", "-H", "Content-Type: application/json", "--input", "latest.json",
    `https://uploads.github.com/repos/${repo}/releases/${id}/assets?name=latest.json`);
}
