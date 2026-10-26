# Vendored fontique

fontique 0.10.0 from crates.io (linebender/parley at `v0.10.0`,
`1df9544bf0bd675d304001c0d0b35df2d220cd14`, path `fontique`), with a patch
to its Android backend: fallback faces are found by file and index, the way
Android itself resolves them. fonts.xml before Android 12 names no faces
(so every non-Latin script fell back to nothing and drew as tofu), and later
ones give every face of the CJK collection the first face's
`postScriptName` (so Simplified Chinese drew with Japanese forms).

A second change makes locales pick faces on Android: fallback lookups
compare fonts.xml's `lang` in `FallbackKey`'s canonical form (the key holds
`zh-CN` for `zh-Hans`, so the two never matched and every locale fell back
by script alone), and Han text with no locale takes the zh-Hans face —
`FallbackKey`'s own default — instead of zh-Hant.

The root `Cargo.toml` points `[patch.crates-io]` here. A patch only applies
to builds of this workspace — apps depending on fenestra need the same
`[patch.crates-io] fontique = { path = ".../vendor/fontique" }` entry.

The scan also records each face's PostScript name by file and index
(`ScannedCollection::face_postscript_names`), so the Android backend looks
fallback faces up instead of parsing font files a second time (on a
OnePlus 6, Android 9: `SystemFonts::new` 10.4 ms → 8.4 ms).

Drop this copy once the fix is in a fontique release fenestra uses.
