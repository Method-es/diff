# Icons

Single-colour SVGs that `dv_ui` paints as masks tinted with the text colour (`dv_ui::icons`,
DESIGN §1.6). Every file here is listed in `crates/dv_ui/src/icons.rs`, which embeds it.

| File | Source |
|---|---|
| `folder.svg`, `search.svg`, `chevron-down.svg` | [Lucide](https://lucide.dev), ISC licence (`LICENSE-LUCIDE`); `search` and `chevron-down` derive from Feather, MIT, same file |
| `window-minimize.svg`, `window-maximize.svg`, `window-restore.svg`, `window-close.svg` | Windows caption glyphs on a 10 grid at a 1px stroke |
| `dv-mark.svg` | dv's mark (design canvas, `GlassReview` title bar) |

The Lucide icons keep Lucide's 24 grid with the stroke scaled to match the design canvas, which
draws its icons on a 16 grid at 1.5–1.6 (DESIGN §1.6): `2.25` for 1.5 and `2.4` for 1.6, so they
come out at the same weight at 14–16px.
