# Patched dependencies

## glib 0.18.5

Tauri v2's Linux stack (GTK 3 bindings, gtk-rs 0.18) depends on `glib` 0.18, which has
[RUSTSEC-2024-0429 / GHSA-wrw7-89jp-8q8g](https://github.com/advisories/GHSA-wrw7-89jp-8q8g):
undefined behaviour in `VariantStrIter`. The fix ([gtk-rs-core#1343](https://github.com/gtk-rs/gtk-rs-core/pull/1343))
shipped only in glib 0.20, which the GTK 3 bindings can't use, and was never released for 0.18.

`glib-0.18.5/` is the unmodified crates.io source (MIT, see its LICENSE) with that two-line fix applied to
`src/variant_iter.rs`, wired in through `[patch.crates-io]` in `../Cargo.toml`.

Remove this folder and the `[patch.crates-io]` entry once Tauri moves to a glib >= 0.20 (GTK 4).
