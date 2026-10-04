# dv: design spec

The chosen direction is **Glass**: dv's Aura colours on the operating system's window material.
Pixel reference: row 2 of the design canvas, (a private canvas) —
artboards `GlassReview`, `GlassSubmit`, `GlassOnboarding` (each has `material` and `mode` tweaks).
Behaviour lives in [PLAN.md](PLAN.md); this file is tokens, layout and interaction. Where a board and
this file disagree, this file wins (§6 lists the known differences).

---

## 1. Tokens

Colours are the board's `tokens()` values. Every surface token has three values — **Acrylic / Mica /
Opaque** — because translucent surfaces need more tint over Mica (which samples only the wallpaper)
and none over Opaque. Solid tokens are the same in all three.

### 1.1 Dark (default; derived from Aura Dark)

| Token | Acrylic | Mica | Opaque | Use |
|---|---|---|---|---|
| `chrome` | `rgba(26,24,36,.56)` | `rgba(28,26,37,.82)` | `#110f18` | title bar, status bar, PR strip |
| `panel` | `rgba(20,19,29,.44)` | `rgba(24,22,32,.76)` | `#13121a` | file list / comments column |
| `code` | `rgba(21,20,27,.94)` | `rgba(21,20,27,.975)` | `#15141b` | diff surface (near-opaque, legibility) |
| `codeHead` | `rgba(28,26,37,.95)` | `rgba(27,26,35,.98)` | `#1b1a23` | file header row in the diff |
| `layer` | `rgba(255,255,255,.045)` | `rgba(255,255,255,.04)` | `#1a1922` | cards on material |
| `layerStrong` | `rgba(32,30,44,.82)` | `rgba(32,30,43,.90)` | `#1e1c28` | selected/active cards |
| `thread` | `rgba(31,29,41,.98)` | `rgba(31,29,41,.99)` | `#1d1b26` | inline thread card |
| `overlay` | `rgba(31,29,42,.965)` | `rgba(31,29,42,.975)` | `#1d1b27` | sheets, palette, menus |
| `ctrl` | `rgba(255,255,255,.06)` | `rgba(255,255,255,.055)` | `#1f1d29` | button fill |
| `inputBg` | `rgba(0,0,0,.24)` | `rgba(0,0,0,.22)` | `#0f0e15` | text inputs |

Solid: `fg #edecee`, `fg2 #c6c4d8`, `fg3 #9d9ab3` (lowest text tier; ≥4.5:1 on every surface's
Opaque value; on Acrylic and Mica it also depends on the desktop behind the window, §6),
`accent #a277ff` (fill) with `accentFg #15141b`, `accentText #c4a9ff` (accent as text),
`green #61ffca`, `red #ff6767`, `yellow #ffca85`, `blue #82e2ff`; soft fills at 12–18% of each
(`accentSoft`, `greenSoft`, `redSoft`, `yellowSoft`, `blueSoft`); `line rgba(255,255,255,.08)`,
`line2 rgba(255,255,255,.14)`, `ctrlLine rgba(255,255,255,.11)`, `overlayLine rgba(255,255,255,.13)`,
`scrim rgba(6,5,10,.56)`, `sel rgba(162,119,255,.22)`.

Diff: `addBg rgba(97,255,202,.085)`, `addGut .15`, `addWord .30`; `delBg rgba(255,103,103,.09)`,
`delGut .16`, `delWord .34`; `hunk rgba(130,226,255,.06)`. Added and removed rows differ in lightness
as well as hue.

Syntax: `kw #a277ff`, `fn #ffca85`, `str #61ffca`, `num #61ffca`, `type #a277ff`, `cst #82e2ff`,
`prop #f694ff`, `punc`/`plain #edecee`.

### 1.2 Light (new; "Aura Light")

Same token names. Key values: `fg #1d1b26`, `fg2 #46434f`, `fg3 #5e5a6f`; `accent #6d43e0`,
`accentFg #ffffff`, `accentText #5a32cc`; `green #0d7553`, `red #bf2537`, `yellow #875600`,
`blue #0b6687`; `chrome` Acrylic `rgba(247,246,251,.58)` / Mica `rgba(239,238,244,.85)` / Opaque
`#efeef4`; `code` `rgba(255,255,255,.93)` / `rgba(254,254,255,.975)` / `#fdfdfe`; `scrim
rgba(36,32,58,.30)`; diff `addBg rgba(18,170,115,.10)`, `delBg rgba(225,45,70,.075)`. Syntax: `kw
#6236d6`, `fn #8a4f00`, `str #0d7553`, `cst #0b6687`, `prop #9a2694`, `punc #3d3a48`, `plain #1d1b26`.
Full set: the board's `tokens()`.

### 1.3 Other themes
Dracula, Claude Dark and Claude Light remain as **opaque-only** themes (their colours from
`themes/*.json`, contrast-corrected as on the Tidy boards). Glass materials apply to the Aura pair; a
non-Aura theme forces `Opaque`. Syntax colours, diff colours and surfaces are one token set per theme;
views never hard-code a colour.

### 1.4 Typography
- **UI:** Geist (bundled, OFL), fallback Segoe UI Variable / system-ui. 13px/18px body; 12px
  secondary; 11px keycaps and badges; 600 weight for titles and the primary button, 500 for controls,
  400 for text. Only 400/500/600 are loaded; the boards' 700 wordmark is drawn at 600.
- **Code:** JetBrains Mono (bundled), 12px on 20px rows; line numbers `fg3`, tabular.
- Headings: sheet title 15px/600; onboarding title 20px/600.

### 1.5 Shape and space
Spacing scale 4 / 6 / 8 / 12 / 16 / 24. Radii: 4 (keycaps, inline code), 5–6 (buttons, chips,
inputs), 7 (segmented control), 10 (thread cards), 12 (sheets). Heights: controls 28, small controls
24/22, title bar 40, status bar 28, file rows 28, code rows 20. Shadows: cards `0 6px 20px
rgba(0,0,0,.28)`; overlays `0 28px 80px rgba(0,0,0,.55), 0 2px 10px rgba(0,0,0,.35)` plus a 1px
inner top highlight `rgba(255,255,255,.06)`.

### 1.6 Icons
Inline stroke icons at 14–16px, 1.5–1.6 stroke, `currentColor`, from one set (Lucide-compatible
geometry, bundled as SVG paths). No emoji anywhere.

---

## 2. Layout

```
┌ title bar (chrome, 40) ─────────────────────────────────────────────────────────────────────┐
│ dv  [repo · review ▾  Ctrl O] [source ▾]   [ Go to file, review or command   Ctrl K ]  PRs Ctrl G  ⋯  – □ × │
├ PR strip (chrome, PR reviews only) ─────────────────────────────────────────────────────────┤
├ panel 280 ─────────────┬ diff stream (code) ────────────────────────────────────────────────┤
│ Changes  4/7 viewed    │ ▸ file header (codeHead, pinned): status · path · ±  Split|Unified ✓ │
│ [ Filter files       ] │   hunk header                                                       │
│ ☑ M src/routes/…  2 ◯  │   old │ new  rows …                                                 │
│ …                      │   ┌ thread card ┐                                                   │
│ Open comments  3       │   …                                                                 │
│  ◯ users.ts:31 …       │   next-file footer: "Next: …   Mark viewed and go next  V J"        │
├ status bar (chrome, 28) ──────────────────────────────────────────────────────────────────┤
│ Ubuntu · backend connected   Draft review · 3 open · 1 resolved        Ctrl Enter finish review │
└──────────────────────────────────────────────────────────────────────────────────────────────┘
```

- Two columns only. Repos and reviews live behind the title-bar switcher (`Ctrl O`) and the palette;
  they never take a permanent column. The panel is resizable 220–420 and collapsible (`Ctrl B`).
- Split view gets ≈575px per side at 1440px; long lines clip with a fade and scroll horizontally
  together (`Shift`+wheel). Unified is offered and remembered per review.
- The diff is one stream over all files (PLAN §6.7); the panel's file rows scroll the stream.
- Below 1100px window width the panel auto-collapses into an overlay.

---

## 3. Materials

| Surface | Acrylic / Mica | Opaque |
|---|---|---|
| Window background | OS material (`Blurred` / `MicaBackdrop`) | `chrome` solid |
| Title bar, status bar, PR strip | `chrome` translucent | solid |
| Panel | `panel` translucent | solid |
| Diff surface, thread cards | near-opaque (`code` ≥ 94%, `thread` ≥ 98%) | solid |
| Sheets, palette, menus, popovers | `overlay` ≥ 96.5% over `scrim`; no blur | solid over `scrim` |

GPUI blurs only what is behind the window, so in-app overlays never "frost" app content (PLAN D8).
Opaque is chosen automatically on Windows 10, with system transparency off, on Linux, and for
non-Aura themes; it must look finished in its own right, not like a degraded mode. Window
inactive: material falls back to its inactive appearance (OS-managed); dv dims nothing itself.

---

## 4. Screens

### 4.1 Review (`GlassReview`)
- **Title bar:** dv mark; switcher button "`api-server` · working tree vs main" with the location chip
  (`Ubuntu`); source picker; centred command field (`Ctrl K`); `PRs Ctrl G`; overflow; caption
  buttons (Windows) / traffic lights (macOS, left).
- **Panel:** "Changes" with `viewed/total` and a thin progress track; filter input; file rows — viewed
  checkbox, status letter (M/A/D/R, colour + letter, never colour alone), path with dimmed directory,
  `+n −m`, open-comment badge. Then "Open comments" — one row per open thread (dot, `path:line`, first
  line of the body), and "Copy as prompt".
- **Diff:** pinned file header (status pill, path, totals, Split|Unified segmented control, "Viewed"
  checkbox, prev/next file, overflow); side labels (`main` / `working tree`); hunk headers in `hunk`;
  rows per §1.1; gaps as "··· 27 unchanged lines — Expand 20 · Expand all"; inline thread cards
  between rows.
- **Thread card:** avatar (reviewer round, agent square with the agent's icon), name, agent tag, time,
  anchor (`new · line 31`), rendered markdown body, replies, state chip + actions, reply input. States
  and their chips: `open` (accent ring dot), `addressed` (blue "Addressed by agent — Resolve /
  Reopen"), `resolved` (green check, collapsed by default), `outdated` (yellow, shown with its
  original snippet).
- **Next-file footer:** "Next: `src/routes/users.test.ts`" + "Mark viewed and go next `V` `J`".
- **Status bar:** location + backend state; review status and counts; `Ctrl Enter finish review`.

### 4.2 Finish review / Submit (`GlassSubmit`)
- Sheet, 980 wide, centred, over `scrim`. Header: "Finish review", `PR #482`, checks state, close.
- Left: **Summary** with Write/Preview tabs (markdown), then **Verdict** as three radio cards —
  Comment / Approve / Request changes (local reviews show Approve / Request changes only).
- Right: **Comments to post** — each with an include checkbox, `path:line`, body excerpt. A comment
  outside GitHub's hunks shows a yellow row: "Unchanged line outside the diff — GitHub's API can't
  anchor it here", explanation, **Post as file comment** / **Skip this one**, with Undo after choosing.
- Footer: "Posting as `kylekz` via gh", blocker text when something needs fixing, Cancel, **Submit
  review `Ctrl Enter`** (disabled until every comment is postable or skipped).
- Local reviews use the same sheet titled "Finish round" with no GitHub rows.

### 4.3 Onboarding (`GlassOnboarding`)
- Whole client area is window material; a centred column of step cards (`layer`), "Set up dv" and
  "Step n of m".
- Steps (PLAN §12.2): environments (Windows / each distro with running state and **Start** for
  stopped ones), install backend (progress with bytes), CLI on PATH (per environment), agent
  integrations (agents × environments checkbox grid), GitHub (`gh` account per environment).
- Current step: accent ring + expanded; done: green check, collapsed summary; pending: dimmed.
- Footer: "The install keeps running if you continue. Change any of this later in Settings.",
  **Skip for now**, **Continue `Enter`**.
- Shown on first run, and when a WSL repo is opened in an environment that isn't set up — then only
  that environment's steps.

### 4.4 States every view handles
- **Loading:** skeleton rows in the panel and diff (no spinners over content) for up to 300 ms, then a
  quiet progress line in the file header.
- **Errors:** an inline banner at the top of the affected region — icon, one-line message, the
  command that failed in a disclosure, a fix action, "Copy diagnostics". Never drawn over other
  regions, never raw stderr as the headline (the first implementation painted git's stderr across
  the sidebar).
- **Backend down (WSL):** status bar shows "Ubuntu · reconnecting…" → banner with **Retry** and
  **Open setup**.
- **Empty:** no changes ("Working tree clean — nothing to review"), no reviews ("Pick a repo to start
  a review"), no open comments.
- **Read-only (submitted PR review):** banner "Submitted to GitHub · view on GitHub", composer and
  thread actions disabled, not hidden.

---

## 5. Keyboard map

| Key | Action |
|---|---|
| `J` / `K` | next / previous file |
| `N` / `P` | next / previous hunk |
| `]` / `[` | next / previous open comment |
| `V` | toggle viewed on the current file (`V` then `J`: mark viewed and go next) |
| `C` | comment on the selected lines (click/shift-click gutter to select) |
| `R` | reply to the focused thread |
| `E` | expand the nearest gap |
| `S` | toggle split / unified |
| `Ctrl Enter` | finish review / submit (in the composer: post comment) |
| `Ctrl K` | palette: files, reviews, PRs, commands |
| `Ctrl O` | switch repo / review |
| `Ctrl G` | open a PR |
| `Ctrl B` | toggle panel |
| `Ctrl ,` | settings |
| `Esc` | close the top overlay, else clear selection |

macOS uses `Cmd` for every `Ctrl` above. Single-letter keys are inactive while a text field has focus.
Keycaps render with `kbd` style (11px mono, `line2` border).

---

## 6. Known differences between the boards and this spec

- Boards say "Resolved by agent"; the model is `addressed` by the agent, `resolved` only by the
  reviewer (PLAN §2.5). Build per this file.
- `GlassOnboarding` says "Installing dv-host 0.3.0"; the backend is the Linux `dv` itself (PLAN D6):
  "Installing dv 0.3.0 in Ubuntu".
- Boards show one file at a time; the diff is a continuous stream (PLAN §6.7) with the same file
  header pinned.
- Board copy (file names, comment text, PR #482) is illustrative fixture content; the e2e fixtures
  (PLAN §14) reproduce it so screenshots compare 1:1.
- The boards' wallpaper is light pastel, which flatters the translucent surfaces. On Windows,
  gpui's Acrylic (`Blurred`) is an untinted blur of the desktop, so the token's alpha is the only
  tint, while Mica gets the OS's own tint as well. Over a contrasting desktop, Acrylic's `panel`
  and `chrome` text therefore drops below 4.5:1 (light Acrylic over a dark desktop: `fg3` on
  `panel` ≈2.5:1, `fg2` on `chrome` ≈4.4:1); dark Acrylic over a light desktop fares worse. Mica,
  the default, and Opaque hold. **Open:** keeping `fg3` at 4.5:1 over any desktop needs a
  `panel`/`chrome` alpha of about .85, which removes most of the material; the alternative is to
  accept it for the opt-in material. Until decided, the values stay the board's.
