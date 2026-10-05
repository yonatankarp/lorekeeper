# Fonts

Lorekeeper's type comes from Solbera's free D&D 5e font package, as collected (with the community fixes and remakes) in
https://github.com/jonathonf/solbera-dnd-fonts (commit `17223764`). The fonts are licensed under the
Creative Commons Attribution-ShareAlike 4.0 International licence (CC BY-SA 4.0); the full text is in
[LICENSE-Solbera.txt](LICENSE-Solbera.txt) and at https://creativecommons.org/licenses/by-sa/4.0/.

| File | Font | Used for |
| --- | --- | --- |
| `bookinsanity-regular.woff2`, `-italic`, `-bold`, `-bold-italic` | Bookinsanity (Remake) | Page text, the editor, the timeline, the quick-note box |
| `mr-eaves-small-caps.woff2` | Mr Eaves Small Caps (Remake) | Headings, session titles, section rubrics, buttons |
| `scaly-sans-regular.woff2`, `-italic`, `-bold` | Scaly Sans (Remake) | Sidebar, search, settings labels, small print |
| `solbera-imitation.woff2` | Solbera Imitation | The drop cap on session titles |

Credits: the fonts are by Solbera. The Remake versions and fixes are by Ners, with earlier fixes by Ryrok and minor
adjustments by LUCASTUCIOUS; jonathonf collected them in the repository above.

Changes made for Lorekeeper: the OpenType files were converted to WOFF2 and subset to Latin characters with fontTools,
and their vertical metrics were made consistent (the line-height values in the `hhea` and `OS/2` tables now agree, with
USE_TYPO_METRICS set; Scaly Sans regular uses the same ascent and descent as its italic and bold). The glyph outlines are
unchanged. These modified files are shared under the same CC BY-SA 4.0 licence.
