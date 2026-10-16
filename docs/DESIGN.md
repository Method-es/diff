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
`scrim rgba(6,5,10,.56)`, `sel rgba(162,119,255,.22)` (rows: the cursor's line, a line
selection); `textSel rgba(162,119,255,.42)` (text selected in code); `findMatch
rgba(255,202,133,.22)` and `findCurrent rgba(255,202,133,.32)` (find matches in code, the
current one outlined in `yellow`).

Diff: `addBg rgba(97,255,202,.085)`, `addGut .15`, `addWord .30`; `delBg rgba(255,103,103,.09)`,
`delGut .16`, `delWord .34`; `hunk rgba(130,226,255,.06)`. Added and removed rows differ in lightness
as well as hue.

Syntax: `kw #a277ff`, `fn #ffca85`, `str #61ffca`, `num #61ffca`, `type #a277ff`, `cst #82e2ff`,
`prop #f694ff`, `punc`/`plain #edecee`, `comment` = `fg3` (`#9d9ab3`). Highlighting produces these
token kinds, never colours (PLAN §6 step 4); each paints in its token.

### 1.2 Light (new; "Aura Light")

Same token names. Key values: `fg #1d1b26`, `fg2 #46434f`, `fg3 #5e5a6f`; `accent #6d43e0`,
`accentFg #ffffff`, `accentText #5a32cc`; `green #0d7553`, `red #bf2537`, `yellow #875600`,
`blue #0b6687`; `chrome` Acrylic `rgba(247,246,251,.58)` / Mica `rgba(239,238,244,.85)` / Opaque
`#efeef4`; `code` `rgba(255,255,255,.93)` / `rgba(254,254,255,.975)` / `#fdfdfe`; `scrim
rgba(36,32,58,.30)`; `sel rgba(109,67,224,.13)`, `textSel rgba(109,67,224,.28)`, `findMatch
rgba(196,128,0,.18)`, `findCurrent rgba(196,128,0,.30)`; diff `addBg rgba(18,170,115,.10)`, `delBg rgba(225,45,70,.075)`. Syntax: `kw
#6236d6`, `fn #8a4f00`, `str #0d7553`, `cst #0b6687`, `prop #9a2694`, `punc #3d3a48`, `plain #1d1b26`,
`comment` = `fg3` (`#5e5a6f`).
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
│ Ubuntu · watching for changes   Draft review · 3 open · 1 resolved     Ctrl Enter finish review │
└──────────────────────────────────────────────────────────────────────────────────────────────┘
```

- Two columns only. Repos and reviews live behind the title-bar switcher (`Ctrl O`) and the palette;
  they never take a permanent column. The panel is resizable 220–420 (its width is remembered, in
  `settings.json`) and collapsible (`Ctrl B`).
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
- **Title bar:** dv mark; switcher button "`api-server` · working tree vs main" (the repo's name at
  600, the target label with its "vs" in `fg3`) with the location chip (`Ubuntu`, WSL repos only) and
  its `Ctrl O` keycap; source picker (`⇄ ▾`); centred command field (`Ctrl K`); `PRs Ctrl G`;
  overflow; caption buttons (Windows) / traffic lights (macOS, left). The left group never runs under
  the centred field: at narrow widths the target label truncates at its end, and once it has no room
  left the repo's name does. With no review open the
  button reads "No review open"; while one opens, "`repo` · opening…", then the full label at once
  (the labels are read as the review opens, not after its files are listed).
- **Switcher** (`Ctrl O`, or a click on its button): an overlay 560 wide hung 8px under the button,
  over the scrim (which covers everything below the title bar; a click on it closes the switcher),
  radius 12. A filter field (46 high, search glyph) has the keyboard: typing filters (every word must
  appear in the repo's name, path or the row's label), `↑`/`↓` move the selection (wrapping), `Enter`
  opens, `Esc` closes. Per known repo (one per worktree root, however its distro is spelled: a
  subfolder added before it could be reached becomes its root's row once a listing reaches it,
  PLAN §11), most recently used first: a header — folder glyph, name at 600,
  the location chip, the path in `fg3` (truncated at its start; a WSL repo's path inside its
  distro, `/home/kyle/api`, the chip naming the distro) — over one line: "uncommitted changes ·
  7 files `+n −m`", "Working tree clean", a skeleton bar while it is computed, or why it can't be
  opened (a triangle in `yellow` for an unreachable repo: "Ubuntu isn't running", "Ubuntu · the
  connection to dv failed: …", "Ubuntu · reconnecting…"; in `red` for a read error). A WSL repo
  whose distro isn't running, whatever its connection did before, offers one row, **Start
  Ubuntu** (play glyph): it connects, booting the distro (PLAN §5.2: listing never boots one),
  the line reading "Starting Ubuntu…" in `fg3` meanwhile and the row staying, dimmed (`fg3`,
  no hover) and inert, with the selection — `Enter` or a click on it again does nothing, and
  the selection never drops to the footer — then lists the repo as usual, its first row taking
  the selection; a start that fails leaves the line saying why ("Ubuntu · the connection to dv
  failed: …" once the distro runs, else "Ubuntu isn't running" again), the row's Start or Retry
  keeping the selection. A repo whose connection failed, its distro running, offers one row,
  **Retry** (rotate glyph), which connects again the same way ("Ubuntu · connecting…" in `fg3`
  meanwhile, the row inert as Start's). One whose connection is reconnecting reads "Ubuntu ·
  reconnecting…" and keeps the rows its last listing had, and picking one of them opens it once
  the connection is back. A repo's rows follow its connection going down or coming back
  whether or not the switcher is open, so it never opens on a stale Start or "reconnecting…". Then its rows (30 high, indented): **Uncommitted changes**, **Staged
  changes** and, with a branch other than the default checked out, **Branch `feat` vs `main`**, each
  beside its open review (status chip, "round n", open-comment badge; "New review" on the selected row
  without one), then the repo's other open reviews by their target label. Picking a row opens or
  resumes that review (PLAN §2.4). A repo that can't be opened offers no rows. The footer holds **Open
  folder…** (the system's folder picker; the folder's repo is added and selected) and the keys.
  Opened over the "Finish round" sheet (§4.2), it and the source picker hang above the sheet, which
  the scrim then covers too; closing them returns to the sheet as it was. Status
  chips: `open`/`draft` accent, `ready` yellow, `changes_requested` red, `in_progress` ("Agent
  working": the agent's first reply, address or comment after changes were requested, PLAN §2.4)
  blue, `approved`/`submitted` green, `closed` grey; always worded.
- **Source picker** (`⇄ ▾`): a 340-wide menu under its button with the open review's repo's targets
  (the switcher's three rows) and their open reviews; the current target is checked. `↑`/`↓`/`Enter`/
  `Esc`; picking one opens or resumes its review.
- **Panel:** "Changes", the file count and the review's `+n −m`; under it, while the review offers
  it (a live target — `working`, `staged`, a branch — not finished, with a closed round; PLAN §2.3),
  a full-width segmented control **Since round n | All changes** with its `I` keycap (the
  header's Split | Unified style; each segment as wide as its label, sharing the room left; in
  a panel narrower than 270 the second reads **All**, so the first keeps its round; the choice
  shows at once, the list follows when it lands); a thin
  progress track with "`viewed` of `total` viewed"; filter input (`Enter` shows the first file left, `Esc` clears it; both hand the
  keyboard back to the diff); file rows (28 high, radius 6) — viewed checkbox (16, accent filled with
  a check when on; dimmed and inert on a finished review), status letter (M modified, A added, U
  untracked, D deleted, R renamed, C copied, T type changed, `!` unmerged; colour + letter, never
  colour alone), path with dimmed directory (truncated at its start; a viewed file's name dims to
  `fg3`), open-comment badge, `+n −m`. The current file's row is `sel`, its name at 600; a click on a
  row shows that file in the diff, a click on the checkbox toggles viewed. Then, under a hairline,
  "Open comments" with the number of threads still waiting (open, and addressed but not yet
  resolved) and "Copy as prompt" — one row per thread, the addressed ones first (the reviewer's
  turn, PLAN §3 workflow A step 5), then the open ones, each group in stream order (the "Not in
  the diff" section's threads first, in the order they were added, then by file; an accent
  ring dot for `open`, a blue dot and "addressed" for `addressed`, `path:line` in mono where the
  thread stands now (an old-side thread's ends in "(old)", as the CLI prints it, so its lines
  aren't taken for the file's lines now; an outdated thread with a trail, PLAN §8, where its
  code went, `path:40–45`, still flagged "outdated"; a comment on the whole file `path (file)`,
  "(file)" in `fg3` where `:line` would be, under its file's name now; while its file's text hasn't landed — briefly, since
  every file a thread shows in loads at once — the line it was written on, in `fg3`), the first
  line of the
  comment as plain text — its Markdown dropped, a fenced block's first line of code, as in every
  one-line summary of a comment; an outdated thread says "outdated" in `yellow` and shows its original first line in
  mono; a thread the interdiff doesn't show carries a quiet "all changes" tag in `fg3`, and its
  `path:line` and "outdated" are where it stands in all changes), then
  "n resolved · show", which lists the resolved ones too; with none waiting, "No open
  comments". A click on a row shows and focuses the thread in the diff — an "all changes" row
  switches the stream to all changes first; the focused thread's row is `sel`. A right click on
  a row opens its menu at the pointer, in the file menu's style (the `overlay` panel, 200 wide,
  items a control high with their glyphs; the row keeps its hover tint meanwhile): the card's
  actions for the thread's state — **Resolve** (open), **Resolve** and **Reopen** (addressed),
  **Reopen** (resolved) — then **Reply** (the thread shown and focused, its reply composer open
  in its card, as `R`) and, under a hairline, **Copy as prompt** (that thread alone, in the
  words "Copy as prompt" uses, headed for its state: "Open review comment", "Addressed review
  comment" or "Resolved review comment on …"). On a finished review the writes are there,
  dimmed and inert, as on the card. A click outside the menu or on an item, `Esc`, an overlay
  opening (the switcher, the source picker, the sheet), the panel hiding, or its row leaving
  the list closes it. "Copy as prompt" puts the `open` threads on the clipboard as Markdown (`path:line` where each stands now, as its row,
  "(where it was written; dv hasn't placed it yet)" beside one not placed yet, the thread id,
  the anchored code, the comment and its replies), for pasting to an agent; in a
  panel narrower than 270 it is its copy glyph alone, with the words in a tooltip, so the header
  never runs past the panel. The list grows to 240 high, then scrolls. The panel's right edge is a resize handle (a 2px accent line while hovered; a double click restores 280).
- **Not in the diff:** threads whose file left the review's list (PLAN §8: the agent deleted
  the file, reverted it, or renamed one git can't pair with its first name; or the reviewer
  commented on a file the review never changed, from the file view) head the stream, in
  a section before the first file: a 44-high `codeHead` bar like a file header's — chevron, a
  file-diff glyph in `fg3`, "Not in the diff" at 600, the count badge, and "comments on files
  the review doesn't change now" in `fg3` (truncating) — then one full thread card per
  thread, in the order they were added, each as an outdated card heading a file looks (the
  "Outdated" chip, its original lines in the `recess` block, Resolve / Reopen, the reply field)
  with its anchor naming the path it was written on (`src/hooks/use-can.ts · new · line 12`,
  truncating at its start). A thread waiting for its file's read (briefly, as the review opens)
  shows neutrally: no chip, the lines it was written on. A thread found in its file as the
  review's new side holds it (`Placement::Unlisted`, or a comment on its whole) wears an
  "Unchanged file" chip (a file glyph, `fg2` on `layer`) in
  place of "Outdated", its anchor the lines it stands at now, and **View file** (a small button
  before Resolve) opens the file view at those lines. Resolved ones collapse as anywhere. The section shows whichever
  comparison is shown (in an interdiff too, its threads never having a file there), collapses
  to its bar with its chevron, its bar pins at the top while its cards scroll as a file's
  header does, and is absent while no thread needs it. It isn't a file: while it is at the top
  no file is current, and `J`/`K`, the headers' arrows and the footer pass over it, while
  `]`/`[`, `R`, `X` and the comments list reach its cards like any other. With no file listed
  (the agent reverted everything, or an empty interdiff) it keeps the stream rather than the
  main area's empty state (§4.4), and ends with that state as a quiet 48-high row under its
  cards (shown collapsed too): a file-diff glyph in `fg3`, the heading at 500 in `fg2`, its line
  in `fg3`, and for an empty interdiff **Show all changes** with its `I` keycap. Before the
  first file list lands, or while it fails, no thread has left the review: they wait, as
  threads whose file's text hasn't landed (PLAN §8), and the section doesn't show.
- **Diff:** one stream over every file (PLAN §6.7). Each file starts with its header: a 44-high
  `codeHead` bar — chevron, path (directory in `fg3`, name at 600; a renamed file shows
  `old → new`), status pill, totals, a quiet "Loading…" once its diff takes longer than 300 ms,
  Split|Unified segmented control with its `S` keycap, "Viewed" checkbox with `V`, previous / next
  file, and `⋯` (a menu in the comments row menu's style: **View file** with the file glyph (the
  file view, at the cursor's line when the cursor is in the file), **Comment on file** with the
  comment-plus glyph, dimmed and inert where the file takes none — a finished review, an
  interdiff's deleted file —, then under a hairline Copy path and Reveal in folder) — over
  24-high side labels (`main` /
  `working tree`, one per column in split view, `main → working tree` in unified; in an
  interdiff `round 1` / `working tree` and `round 1 → working tree`, the round by a commit glyph
  since its column is the round's snapshot) when the file shows lines. The header of the file being read stays pinned at the top while its rows scroll
  under it, and the next file's header pushes it up. **Viewed files collapse to their header bar**
  (as on GitHub), the chevron pointing right; the chevron opens one again (and collapses any).
  Rows per §1.1: 20 high, a 44-wide line number (`fg3`) and a 16-wide `+`/`−` marker per side;
  split view pairs the i-th removed line of a change with the i-th added one and fills a missing
  side with `recess`; unified shows both numbers then the marker. Code is JetBrains Mono 12 with
  syntax colours, intraline words on `addWord`/`delWord`, tabs as four spaces and no ligatures
  (`=>` stays two characters). Long lines clip with a fade at the right edge and scroll sideways
  together, one offset for every row and both sides (`Shift`+wheel, a trackpad); scrolled, the
  left edge fades too. Gaps are one 28-high row on `hunk`: the next hunk's `@@ -a,b +c,d @@` in
  `blue`, "··· 27 unchanged lines", "Expand 20 · Expand all" (a click reveals 20 lines next to
  the hunk below, or above for the end of the file); the board's separate hunk header and
  "Expand context below" bar are this one row. Files without lines show one notice row instead:
  binary (sizes), too large ("Load anyway"), submodule (old → new commit, `dirty`), unresolved
  conflict, no content change (mode change, rename without edits, empty file), skeleton rows
  while loading, and a failed load's banner (§4.4: one line, Details, Retry, Copy diagnostics).
  Each open file ends with the next-file footer. The **cursor** marks its line number in
  `accentText` at 600 with a 2px accent bar, and its line on `sel`; a selection is `sel` over the
  selected lines of one side. **Commenting with the mouse** is GitHub's gesture: the pointer over a
  line (a unified row, a split row's half) shows a `+` over its marker column, an 18 square on
  `accent` with a 12px plus in `accentFg` (radius `small`); a click opens the composer on that
  line, and pressing it and dragging over other lines of the same file and side selects them as
  the drag goes, the composer opening on them when the button comes up (anywhere). A line takes
  no `+` where `C` takes no comment: a finished review, an interdiff's old column. Inline thread
  cards go between rows, and the lines a card sits
  on stay shown (a thread relocation moved can stand on lines no change is near, which would
  otherwise fold into a gap).
- **Text in code** (PLAN §18.1): a press in a line's code (the pointer an I-beam there) puts the
  cursor on its line, as a click on its number does, and the **caret** at the character: before
  the one under the pointer, after it from 60% of its width on. Characters are where their
  glyphs are drawn: a wide one (CJK, an emoji) takes two cells, a combining mark goes with the
  letter before it (the caret never falls between them), a tab four cells. The caret is a 2px
  `accent` bar 16 high, painted while the stream has the keyboard; the keyboard's line moves
  (`↑`/`↓`, `J`/`K`, `N`/`P`, pages, `E`), `S` and `I` take it away. Dragging with the button
  down selects characters, the caret and the cursor following the pointer; a double-click
  selects the word under it (a run of identifier characters — alphanumerics and `_` —, of
  whitespace, or of any other character) and a drag after it extends by whole words, a
  triple-click the whole line and a drag after it by whole lines; a shift-click extends from
  the selection's start (else the caret). A selection is one file's: in split view one column's
  (the one the press was in), in unified view the rows in order, removed and added lines alike;
  dragging past the file's first or last row clamps there, onto a card or a gap row takes the
  end of the line before it, and within 24 of the stream's top (below the header's 44px bar) or
  bottom edge, or past it, scrolls the stream, faster the further, until it can't move. Selected
  text paints on `textSel` behind the characters, a line the selection goes on past one column
  further; it stays through scrolling either way and gaps opening, and goes with a press
  anywhere else in the stream, `Esc`, switching split / unified or the comparison (`I`), and
  its file's diff changing to other lines (the agent's edit; the caret too). A drag the window
  stops being active during ends there. `Ctrl C` copies it as the source text (real tabs, lines
  joined with `\n`, the lines a gap inside it hides included); with no text selected, a line
  selection copies its lines; with neither, nothing. A drag from a line's `+` still selects
  lines for a comment, never text.
- **Find** (`Ctrl F`, PLAN §18.1): a bar over the stream's top right, 8 below the file header's
  44px bar (the pinned header's controls stay in reach) and 16 in from the right edge, 400 wide:
  the `overlay` surface with its outline and shadow (as menus), radius `card` (10), 6 padding.
  One line holds the field (a control high, `inputBg`, `ctrlLine` outline, the search glyph, "Find
  in review"), the count in `fg2` at 12 ("3 of 17"; "3 of 17+" while files still load;
  "Searching…" before a first match while they do, "No results" in `fg3`, nothing for an empty
  query), then 24-square ghost buttons ↑ (previous, `Shift Enter`), ↓ (next, `Enter`) and ×
  (close, `Esc`) with their keys in tooltips; under it, when some file couldn't be searched,
  "2 files not searched" in `fg3`. `Ctrl F` opens it with the field focused (the selected text
  in it when that is one line), and again focuses the field with its text selected. Typing
  searches once it pauses (120 ms): every listed file in stream order, both sides, the text
  inside gaps and in viewed files included, through what the stream shows (an interdiff's
  diffs in an interdiff); a file not loaded yet loads for it (after the viewport's), and a file
  without text to search (binary, too large to show, a submodule, unmerged, a failed load) is
  counted as not searched; one whose content didn't change shows a notice, not lines, and isn't
  searched. Plain text, case-insensitive unless the query has a capital (smart case). In
  unified view a context line matches once; in split view once per column, rows top to bottom
  and the old column before the new. The search goes to the first match at or after the cursor's
  row (else the current file's top) in that order; while later files are still loading it waits
  for a match after the cursor rather than going back behind it, and wraps to the first match
  once every file has been searched. Going to a match opens its file if it is viewed, opens the
  gap that hides it around it (3 lines either side), scrolls it to the middle of the stream
  unless it is in view below the pinned header and the bar, and puts the cursor on its line and
  the caret at its start; the keyboard stays in the field. `Enter` before typing has paused
  searches the new query and goes to its first match. A file loaded only for the search isn't
  highlighted until the stream shows it. While the bar is open every match in view
  paints on `findMatch` and the current one on `findCurrent` with a 1px `yellow` outline, radius
  2. `Esc` (in the field or the stream) or × closes it: the matches go and the keyboard returns
  to the stream, the cursor on the current match's line. A press on the bar leaves the stream's
  selection alone.
- **File view** (PLAN §18.1): one file whole over the main area in place of the stream, which
  keeps its place under it. Its header is a 44-high `codeHead` bar, pinned at the top while the
  lines scroll: a file glyph in `fg3`, the path (mono 13, directory in `fg3` at 500, name at
  600, truncating at its start), a chip with the side's label ("working tree", "main"; a file
  glyph, `fg2` on `layer`), the review's status pill (as a file header's: "Modified", "Added",
  "Deleted" — a deleted file shows its old side) or, for a file the review doesn't change,
  "Unchanged in this review" in `fg3`, then at the right a ghost **Close** with an × glyph and its
  `Esc` keycap (tooltip "Back to the review (Esc)"). No side labels, no gaps, no footer. Its rows
  are the stream's unified rows with one 44-wide line number (the side's) and the marker: every
  line of the side, highlighted, the review's added lines on `addBg` with their `+` and
  intraline words as in the stream; the cursor, a line selection, the comment `+`, text
  selection, the caret and find matches look as in the stream. Thread cards sit under their last
  line, past the line number; comments on the whole file and outdated cards head the lines.
  While the text loads, skeleton rows; a file the side doesn't hold or a read that failed shows
  the one-line banner (§4.4), binary and too-large files their notices, an empty file
  "Empty file · nothing to show". `Ctrl F` opens the find bar over it ("Find in file"), searching
  that file only. A file read at its side over the size caps shows "Too large to show" with
  **Load anyway** (a small button), as the stream's notice does. Its side, status pill and side
  label are the review's own (all changes, `main` / `working tree`) whichever comparison shows;
  in an interdiff, after the pill, "changes since round n" in `fg3` (its marked lines those the
  stream marks) or "unchanged since round n" (read at the side, unmarked), and a file deleted in
  all changes shows its old side, every line on `delBg`. A composer holding a draft keeps it:
  another file opened (`Ctrl P`, View file, `F`), the close button, `Alt ←` / `Alt →` and a panel
  row that shows the stream do nothing but give the draft the keyboard; `Esc` in the composer
  discards it.
- **File view at the working tree** (PLAN §18.1 "Go to definition"): in a review whose new
  side isn't the working tree, a definition or reference elsewhere opens the file view of the
  file as the working tree holds it now. Its header's side chip reads "working tree" and, in
  place of the status pill, "As it stands now, not this review's side · read-only" in `fg3`; its
  lines carry no changed marks, no threads and no comment `+` (`C` does nothing).
- **File finder** (`Ctrl P`, PLAN §18.1): the switcher's look — an `overlay` panel 560 wide,
  radius 12, over the scrim, hung 8 under the title bar's command field and centred on it. A
  46-high filter field with the search glyph ("Go to file") has the keyboard; the list (up to 14
  rows, then it scrolls) holds 30-high rows radius 6: a file glyph (`fg` when selected, else
  `fg3`), the review's status letter in its colour for a file it changes (a blank column
  otherwise), and the path, directory in `fg3`, name at 500, truncating at its start; the selected
  row on `sel`. A 40-high footer: the count in `fg3` ("2 files") and the keys (`↑` `↓` move, `Enter`
  open, `Esc` close). No match: "No files match “x”" with "Every word must be in the file's path."
  in `fg3`. A click on a row or `Enter` opens it in the file view; `Esc`, a click on the scrim or
  `Ctrl P` again closes it.
- **Interdiff** (round n+1 of a live target, PLAN §2.3): the stream, the file list and its
  totals are what changed since round n (its snapshot → now); `I` or the panel's toggle switches
  to all changes and back, the reading position staying where the same file and line still
  show. Its new side is the same text as in all changes, so new-side threads show there as in
  all changes; old-side threads, and threads whose file isn't in the interdiff (but is among the
  review's changes: one that left them is in the "Not in the diff" section), aren't drawn in
  the stream (`]`/`[` pass over them) and carry the comments list's "all changes" tag, named
  where they stand in all changes.
- **Thread card:** avatar (reviewer round, agent square with the agent's icon), name, agent tag, time,
  anchor (`new · line 31`: where the thread shows now; "`new · lines 34–36 · moved`" while
  those are other lines than the ones it was written on, found there by relocation or recorded
  there by an event — never for code edited where it stands, PLAN §8), rendered
  markdown body, replies, state chip + actions, reply input. States
  and their chips: `open` (accent ring dot), `addressed` (blue "Addressed by agent — Resolve /
  Reopen"), `resolved` (green check, collapsed by default), `outdated` (yellow, shown with its
  original snippet). The card (`thread`, radius 10, `line2` outline, the card shadow; the focused
  thread's outline `accentRing`) sits under the last line of its range on its side: in split
  view in that side's half over the line's tint, 62 in from the half's edge, the other half
  `recess`; in unified view past both line numbers. Its 32-high header holds the state chip
  ("Open", "Addressed by claude-code", "Resolved"), "Outdated" when it is, the anchor in `fg3` ("file" for a comment on the whole file)
  and the reviewer's actions as small buttons (Resolve on an open thread, Resolve and Reopen on
  an addressed one, Reopen and a chevron on a resolved one; in a narrow card the anchor, then the
  chip's agent name, truncate, never the actions); an addressed thread's note follows
  on `blueSoft` with the agent's glyph. A resolved thread collapses to that header with its
  comment's first line until its chevron opens it (or while a reply to it is being written). A
  thread whose code moved (lines inserted above it, its block moved, a small edit) follows it to
  its new lines. An outdated thread (relocation can't find its code at all, PLAN §8) has no line
  to follow: its card heads its file, under the header, with the lines it was anchored on in a
  `recess` block. When its round's snapshot tells where that code went (its trail: a new-side
  thread of a closed round, PLAN §8 "Round history"), the card stands there instead, inline
  as any card, under the last line that replaced the code (or, for code removed, under the line
  before where it stood; heading the file only when it stood at the top), with the "Outdated"
  chip, the anchor "code changed since round 1" and its original lines in the `recess` block;
  those lines stay shown as any card's. A thread whose file was renamed since its round, one
  git can't pair (an untracked file renamed), follows it by content into the new file; one whose file left the review heads the stream instead, in the "Not in the
  diff" section above. A **comment on the whole file** (PLAN §8) has no line either: its card
  heads its file, under the header (and its side labels) and above its first row or its notice
  (also while the file loads, since it needs none of its text), with the anchor "file" and no
  `recess` block; a file's comments on the whole file come first, in the order they were
  added, then the composer of a new one, then its outdated cards. One on a file deleted since
  keeps heading it with "Outdated" (its new side is gone); one whose file left the review is
  in the "Not in the diff" section, reading `path · file`. In a file shown without lines (too large to show, or "No content change": a
  file only renamed, or only its mode changed), a thread still found in it (PLAN §8: placed
  against the file read in full, or against the unchanged text the diff carries) heads the file
  too, above its notice, with the anchor where it stands and no "Outdated". The
  reply field reads "Reply…" (with `R` on the focused thread) and opens the reply composer in
  its place. On a finished review the actions and the field stay, disabled ("Replies are closed
  on a finished review").
- **Composer:** a card under the selected lines (or the cursor's line), outlined in
  `accentRing`: "Comment on new · lines 12–14" with the comment glyph, a multi-line Markdown
  field (3 rows, growing to 12; `Enter` is a newline), why the last post failed in `red`, and a
  footer — "Markdown", Cancel with `Esc`, the primary "Comment" with `Ctrl Enter`. As a reply it
  is the field and the footer alone, inside the card ("Reply"). It takes the keyboard while
  open, also scrolled out of view (typing brings it back); posting or cancelling gives it back to
  the diff, posting focuses the thread. **Comment on file** (the header's `⋯`, or `Shift C` on the
  current file) opens it heading the file instead, across its width, "Comment on file", on the
  file's new side (a deleted file's old side). Hidden by its file collapsing, it keeps its text and
  gives the keyboard back to the diff (`C` opens the file and takes it up again). A
  comparison that can't hold it — an interdiff that doesn't list its file, or (it being on the
  old side) whose old column is the round's snapshot — leaves it undrawn with its text and
  lines, until all changes show it again; `C` then switches to all changes and takes it up. Its
  file leaving the review (the agent reverted it) drops it.
- **Hover card** (PLAN §18.1 "Hover"): the pointer resting 350 ms on code (the stream or the
  file view; any character but whitespace and quotes), or `Ctrl I` at the caret, hangs a card 2
  below what it is about, the servers' range (a whole class name) or else the token, starting
  at its left edge (moved in to stay 8 inside the window): the `overlay` surface
  with its `overlayLine` outline and the overlay shadow, radius `card` (10), 280 to 560 wide. Its
  body (12/8 padding, scrolling past 360 high) is the server's answer as Markdown, its fenced
  signature highlighted as code, inline code on `icodeBg`, links in `accentText` (a file link opens
  the file there, a web link the browser); the text can be selected. When several servers serve
  the file (a `.tsx`: TypeScript and Tailwind CSS) their answers stack in the registry's order
  (PLAN §18.4), the card opening with the first that says anything and growing as the others
  land: a hairline (`line`, 8 above and below) between two answers and, while more than one is
  shown, each headed by its server's name ("TypeScript", "Tailwind CSS") in `fg3` at 12 (the
  secondary text), 4 above its answer; one answer alone has no heading. Under a hairline, two small
  buttons — **Go to definition** `F12` and **Find references** `Shift F12` with their keycaps — and,
  when the server read a side other than the working tree, "Read against the working tree" in
  `fg3` at the right. It stays while the pointer is over the token or the card and closes 300 ms
  after it left both (closing only the card: resting on the next token still asks about it),
  at once on a scroll, a press in the code, `Esc` (first in the stream's chain) or its file's
  diff changing. A hover asked for and not answered yet is cancelled as soon as the pointer
  leaves its token. Text selected in the card takes the keyboard; when the card goes, the
  keyboard returns where it was before the press in it. Nothing shows for an empty answer, or
  without a server.
- **References tab** (PLAN §18.1 "Find references"): while a references list (or several
  definitions) is open, the panel heads its column with a segmented pair 8 below its top, 16 in
  from its sides (the `ctrl` fill and `ctrlLine` outline of the Split | Unified control, radius 7,
  3 padding; each tab half the width, 26 high, 13/500, the chosen one on `segOn` in `fg`, the other
  `fg2`): **Review** (the file list and the comments, as without the tabs) and **References n**
  (n once it answered). `Shift F12` and a go to definition with several places choose References.
  Its body: a header — "References" (or "Definitions") at 600, the symbol in mono `accentText`,
  the count in `fg3`, a 22-square × at the right ("Close the references") — then **In this review**
  (the panel's checkbox and its label in `fg2` at 12) and, when it is so, "Read against the working
  tree" in `fg3`, over a hairline. Then each file in path order: a 26-high row with the file glyph,
  its path (directory `fg3`, name `fg` at 600; "outside the repository" in `fg3` after a path
  outside it) and its row count in `fg3`; under it a 24-high row per place: a 3-wide changed marker
  (`green`, on rows the review changed, else empty), the line number in mono 11 `fg3`
  right-aligned in 36, then the line's text in mono 11.5 without its indentation, highlighted, the
  symbol on `findCurrent` at 600; `controlHover` under the pointer. Loading, four skeleton bars;
  none, "No references found" (or with **In this review**, "None on lines this review changed") in
  `fg3`. A click on a row goes there as go to definition does.
- **Next-file footer:** "Next: `src/routes/users.test.ts`" + "Mark viewed and go next `V` `J`"
  ("Go to next file `J`" once viewed; the last file says "Last file" with "Mark viewed `V`").
- **Status bar:** a dot and the location with the backend's state — green "Local · watching for
  changes", yellow "· refreshes when focused" (the watch can't follow the worktree), red "· refresh
  failed" or "· can't be opened", `fg3` "No repo" / "· opening…". A remote repo is named by its
  connection (the distro: "Ubuntu · watching for changes"), whose state comes first: `fg3`
  "Ubuntu · installing dv (3.1 / 7.4 MB)…" (also while its review opens) and "· connecting…",
  yellow "· reconnecting…", red "· stopped" (the distro was shut down, which dv leaves so until
  asked) and "· disconnected" (each with the banner, §4.4); a connection closed for idleness
  (PLAN §5.1) is up, so the review's own state shows, "· refreshes when focused" when its watch
  couldn't start; then, after a `|`, the review's
  status, round and thread counts ("Changes requested · round 2 · 3 open · 1 resolved", "no comments"
  when there are none; an interdiff adds "· since round 1"); at the right, each language
  server of the review's worktree (PLAN §18.5) — a 7 dot and its name and state in `fg2`, 22 high,
  `controlHover` under the pointer: green "TypeScript ready", yellow "TypeScript loading 40%" (its
  progress), "TypeScript restarting…" and "TypeScript waiting for the connection", `fg3` "TypeScript starting…", red
  "failed", and `fg3` "TypeScript missing" for a server the project lacks — then `Ctrl Enter
  finish review` while the review can still be finished. A click on a server opens its details
  over the status bar's right end (a menu panel 420 wide, 16 padding): the dot, the name at 600 and
  its state, **Retry** (a small button) on a failed one with why it failed in `red`, then "Command"
  and the command line in mono on `icodeBg`, "From DV_LSP_TYPESCRIPT" (or "the project's own
  (7.0.2)", "settings.json") in `fg3`, and "Last output" with its last stderr lines in mono; a
  missing one's say "TypeScript isn't available", why (where it looked in `fg3` under it), "How
  to get it", and a small **Check again** button (the ↻ glyph) 6 below. A click on the item again, or `Esc`, closes them.
- **Code notice** (PLAN §18.1 "Go to definition"): a place that can't be shown (outside the
  repository) heads the main area as a one-line banner (§4.4), "Defined outside the repository:
  `<path>:<line>`", with its detail and **Dismiss**; going to another place takes it away.

### 4.2 Finish review / Submit (`GlassSubmit`)
- Sheet, 980 wide, centred, over `scrim`. Header: "Finish review", `PR #482`, checks state, close.
- Left: **Summary** with Write/Preview tabs (markdown), then **Verdict** as three radio cards —
  Comment / Approve / Request changes (local reviews show Approve / Request changes only).
- Right: **Comments to post** — each with an include checkbox, `path:line`, body excerpt. A comment
  outside GitHub's hunks shows a yellow row: "Unchanged line outside the diff — GitHub's API can't
  anchor it here", explanation, **Post as file comment** / **Skip this one**, with Undo after choosing.
- Footer: "Posting as `kylekz` via gh", blocker text when something needs fixing, Cancel, **Submit
  review `Ctrl Enter`** (disabled until every comment is postable or skipped).
- Local reviews use the same sheet titled "Finish round" with no GitHub rows: "Finish round n"
  over "`repo` · `working tree vs main` · 7 files `+n −m`" (the review's changes, which the round
  finishes, also while the stream shows an interdiff); the Summary (a 150-high field in
  mono 12.5, "Markdown · optional", `Tab` to move between it and the verdicts); the Verdict as
  two cards, **Approve** (`1`, "The work is done; the review closes") and **Request changes**
  (`2`, "The agent has more to do; the round goes back to it"), Request changes chosen while a
  thread is open, else Approve; on the right "Comments" — those opened this round and earlier
  ones not yet resolved (with their round: the one a thread was opened in, which a move recorded
  in a later round, PLAN §8, doesn't change), each with its state glyph, `path:line` where it
  shows now (as the comments list names it, `fg3` while not placed yet) and first line; the footer "Finishing round n as `name`", why the last attempt failed in `red`, Cancel
  and the primary button named by the verdict with `Ctrl Enter`. The verdicts have the keyboard
  when it opens (`↑`/`↓`, `1`/`2`); `Esc` closes it. While the round is being finished the
  primary reads "Finishing…" and the sheet stays: `Esc`, Cancel (dimmed) and the close button do
  nothing until the round lands, when it closes, or fails, when it shows why. It holds unsaved
  text, so `Ctrl O` and the source picker open over it, not in its place. It hangs 92 below the
  title bar in a 900-high window, less in a shorter one (24 at 700).

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
  quiet progress line in the file header. While a review opens or lists its files, the main area
  shows the file header's bar with "Opening the review…" / "Listing changes…" over skeleton bars; the
  panel shows skeleton rows, then "Listing changes…". That is the first listing only: a refresh
  keeps the list, the stream or the empty state on screen until it lands, and a file whose diff
  loads again keeps showing the one it had (with the header's "Loading…" after 300 ms).
- **Errors:** an inline banner at the top of the affected region — icon, one-line message, the
  command that failed in a disclosure ("Details"), a fix action, "Copy diagnostics". Never drawn over
  other regions, never raw stderr as the headline (the first implementation painted git's stderr across
  the sidebar). The banner is `redSoft` with a `red` outline at 35%, radius 6, 12 in from the
  region's edges. A review that didn't open shows one at the top of the main area (Retry, Dismiss;
  the review open before stays open); one in a WSL repo whose connection is the trouble reads
  "couldn't open Ubuntu:/home/kyle/api", the connection's error and diagnostics (its launcher,
  state history, the server's last output) in Details and Copy diagnostics. A review picked
  whose connection is the trouble is never lost and never reported twice (PLAN §5.2): it waits
  for the connection as the pending pick, and opens by itself once the connection is back (its
  banner going, menus and the keyboard left where they are), but never in place of unsaved
  text: while the "Finish round" sheet is open or a composer holds a draft, it waits on until
  the sheet closes or the draft is posted or cancelled; meanwhile the main area raises nothing
  while the open review goes through the same connection (its status bar and the backend-down
  banner below say it), and otherwise shows the "couldn't open …" banner, whose Retry runs the
  pick at once (the reviewer's own pick, as from the switcher) and whose Dismiss drops it; the
  Dismiss of any other banner there leaves the pick waiting. A
  refresh that failed shows one there too (Retry), and so
  does a folder (or a `dv open` path) that isn't a repository (Dismiss). A review whose worktree
  is gone (PLAN §2.2: a working review of a linked worktree removed since) says so in its
  headline, "review r-… can't be opened: its worktree is gone (removed, moved or deleted)", the
  worktree's path in Details.
  A round's snapshot missing from the repository (its ref deleted by hand and pruned) is a banner
  there too, "Showing all changes: round 1's snapshot (…) is missing from the repository"
  (Dismiss), over the all-changes stream it falls back to.
- **Handoff:** while the review is `ready` the agent's handoff heads the main area: an info
  banner on `blueSoft` over `code` made opaque (on Mica and Acrylic the material doesn't show
  through it, so its note's fade ends in exactly its colour), with a `blue` outline at 35%,
  radius 6, 12 in from the region's edges —
  the agent's glyph in `blue`, "`claude-code` marked round 2 ready" (the name at 600; a long
  name truncates, "marked round n ready" and the disclosure never do), the first line of its
  note (`dv review done --note`, PLAN §7) as plain text in `fg2`, and a **Show note** disclosure
  that opens the whole note under it as Markdown (the first line then leaves the header),
  growing to 240 high, then scrolling, so the stream keeps its room; its `md` inset at the foot
  stays below the scrolled text, so a clipped line stops short of the outline, and a 28-high
  fade into the banner's colour sits there while more of the note lies below. It goes once the
  reviewer finishes the round. Each handoff (the next round's, or the agent's `dv review done`
  again with another note) is a new banner, its note closed.
- **Backend down (WSL):** while the connection to the distro's dv reconnects (PLAN §5.2) the status
  bar says "Ubuntu · reconnecting…" in `yellow` and nothing else changes: the stream, the list and
  the threads stay, no banner is raised for the calls that fail meanwhile, and the reconnect reads
  everything again. Once it has failed for good, the status bar says "Ubuntu · disconnected" in
  `red` and the error banner heads the main area: "Lost the connection to dv in Ubuntu", the
  error and the connection's diagnostics (its launcher, state history and the server's last
  output) in Details, **Retry** (connects again; the banner goes while it tries and comes back if
  it fails) and Copy diagnostics; the review's own "refresh failed" banner stays down meanwhile.
  When the distro was shut down (`wsl --shutdown`, which dv never undoes by itself, PLAN §5.2),
  the status bar says "Ubuntu · stopped" in `red` and the banner reads "Ubuntu stopped", why and
  the diagnostics in Details, with **Start Ubuntu** (boots it and connects; the review then reads
  everything again) and Copy diagnostics. A banner that comes back after its action opens with
  its Details closed. **Open setup** joins it with the setup screen (phase 6).
- **Empty:** no changes ("Working tree clean — nothing to review" for a `working` review, "No changes
  — nothing to review" for other targets), an empty interdiff ("No changes since round 1",
  "Nothing changed after the round was handed over.", and **Show all changes** with its `I`
  keycap), no reviews ("Pick a repo to start a review"), no open comments. While threads whose
  files left the review are in the "Not in the diff" section (§4.1), no changes and an empty
  interdiff show the stream with that section instead, the same words (and Show all changes) a
  row under its cards. The switcher with no
  known repo says "No repositories yet"; a filter that matches nothing says so.
- **Read-only (submitted PR review):** banner "Submitted to GitHub · view on GitHub", composer and
  thread actions disabled, not hidden. A finished local review says "Approved · this review is
  finished" (on `greenSoft` with a `green` outline at 35%; "Closed · …" in `fg2` on `layer`) with
  **Start a new review**, which opens a fresh review of the same target; `C`, `R`, `X` and
  `Ctrl Enter` do nothing on it.

---

## 5. Keyboard map

| Key | Action |
|---|---|
| `J` / `K` | next / previous file: its header at the top, the cursor on its first change (the "Not in the diff" section isn't a file: `J` from it goes to the first file) |
| `N` / `P` | next / previous block of changed lines, on into the next files |
| `]` / `[` | focus the next / previous thread that isn't resolved (open or addressed), scrolling to it, on into the next files (in an interdiff, the threads it shows) |
| `I` | switch between the interdiff since the last round and all changes (the panel's toggle; while the review offers it, PLAN §2.3) |
| `V` | toggle viewed on the current file (`V` then `J`: mark viewed and go next); each press flips the state the last one asked for, however fast they come |
| `C` | comment on the selected lines, else the cursor's line (in the file view, any line of the file, changed or not), else (no cursor yet) the first line in view, where the cursor goes too (click a line number to put the cursor there, shift-click to select to it; a line's `+` does it all with the mouse, §4.1); a composer holding a draft takes the keyboard instead, wherever the cursor is. In an interdiff it comments on the new side only: on the old column (the round's snapshot, which no anchor names) it does nothing |
| `Shift C` | comment on the whole of the current file: the composer heads the file (the header menu's Comment on file); a composer holding a draft takes the keyboard instead, as for `C`. Nothing on a finished review, or in an interdiff on a deleted file (its old column is the round's snapshot) |
| `R` | reply to the focused thread (the reply composer opens in its card) |
| `X` | resolve the focused thread, or reopen it when it is resolved |
| `↑` / `↓` | move the cursor a line (the stream scrolls to keep it in view; it crosses into the next file) |
| `Shift ↑` / `Shift ↓` | extend a selection from the cursor, within its file and side; a gap in the way opens a step towards the cursor rather than being selected unseen |
| `←` / `→` | in split view, put the cursor on the old / new side (the new side until one is chosen, or a line number clicked). A file without lines on that side (a deletion, an addition) moves it to the other side while it is in that file only: the next file that has lines on the chosen side puts it back |
| `PageUp` / `PageDown` | scroll a page, the cursor keeping its place on screen |
| `E` / `Shift E` | expand the gap nearest the cursor by one step towards it / fully |
| `S` | toggle split / unified (remembered per review) |
| `Ctrl Enter` | finish review / submit: the "Finish round" sheet, while no text field has the keyboard (in the composer: post comment) |
| `Ctrl K` | palette: files, reviews, PRs, commands |
| `Ctrl O` | switch repo / review (the switcher; again, or `Esc`, closes it) |
| `Ctrl G` | open a PR |
| `Ctrl B` | toggle panel |
| `Ctrl ,` | settings |
| `Esc` | close the top overlay, else a comments row's menu, else the language server's details, else the hover card, else the file menu, else cancel the composer, else close the find bar, else clear the selection (the lines and the text both), else, in the file view, close it (back to the stream where it was); in the find field it closes the bar first (the field has the keyboard, so the stream's chain doesn't run) |
| `F` | the current file whole in the file view, at the cursor's line when the cursor is in it (a line on the other side only, a removed one, maps to the next line of the side shown) (the header menu's View file); nothing in the file view itself; with a draft in the file view's composer, the draft takes the keyboard instead |
| `Ctrl P` | the file finder: every file of the review's new side (again, or `Esc`, closes it) |
| `Alt ←` / `Alt →` | back / forward through the history of jumps between the stream and the file view, restoring the view, the file and the line; the mouse's back and forward buttons too. A step that would replace or close a file view holding a draft gives the draft the keyboard instead, the history left as it was |
| `Ctrl C` | copy the selected text, else the selected lines (§4.1 "Text in code"); in a text field (the find field, the composer, the filters, the summary) its selection — that and `Ctrl X` going through the clipboard seam (`dv_ui::system`) like every copy |
| `Ctrl I` | the hover card for the symbol at the caret (§4.1 "Hover card") |
| `F12` / `Ctrl`+click | go to the definition of the symbol at the caret / under the pointer (one place: the stream or the file view there, a jump the history keeps; several: the References tab) |
| `Shift F12` | find the references of the symbol at the caret: the panel's References tab |
| `Ctrl F` | the find bar, its field focused (the selected text in it when that is one line); again, the field focused with its text selected (§4.1 "Find") |
| `Enter` / `Shift Enter`, `Esc` | in the find bar: the next / previous match, wrapping; close it (the keyboard back to the stream) |
| `↑` / `↓`, `Enter` | in the switcher, the source picker and the file finder: move (wrapping), open |
| `Enter` / `Esc` | in the file filter: show the first file left / clear the filter (both return to the diff) |
| `Ctrl Enter` / `Esc` | in the composer: post / discard (both return to the diff; `Enter` is a newline) |
| `↑` / `↓`, `1` / `2`, `Tab` | in the "Finish round" sheet: choose the verdict (Approve, Request changes) / move between the verdicts and the summary; `Ctrl Enter` finishes the round, `Esc` closes |

macOS uses `Cmd` for every `Ctrl` above. Single-letter keys are inactive while a text field has focus.
Keycaps render with `kbd` style (11px mono, `line2` border).

In the file view the stream's keys act on its one file; `S`, `V`, `I` and the gap keys do
nothing there.

The stream's keys work wherever the main area has the keyboard, which it does whenever no overlay
or text field does (a click in the stream gives it back). The **current file** — the one the panel
highlights, `V` marks and `J`/`K` step from — is the cursor's file while the cursor is on screen,
else the file at the top of the viewport, whose header is the pinned one.

---

## 6. Known differences between the boards and this spec

- Boards say "Resolved by agent"; the model is `addressed` by the agent, `resolved` only by the
  reviewer (PLAN §2.5). Build per this file.
- `GlassOnboarding` says "Installing dv-host 0.3.0"; the backend is the Linux `dv` itself (PLAN D6):
  "Installing dv 0.3.0 in Ubuntu".
- Boards show one file at a time; the diff is a continuous stream (PLAN §6.7) with the same file
  header pinned. The stream adds a chevron to the header (viewed files collapse, §4.1), shows the
  side labels only over files that show lines, and makes the board's hunk header and its "Expand
  context below" bar one gap row that heads the hunk after it.
- `GlassReview` splits the title bar's label across two buttons (`api-server` / `⇄ working tree vs
  main`), groups the panel's files under directory headings, and fills the status bar's left with key
  hints. This file keeps the repo and target together on the switcher button with the source picker
  as `⇄ ▾`, shows each file's full path with its directory dimmed, and gives the status bar the
  location, the review and `Ctrl Enter finish review` (§4.1).
- The board draws the inline thread card at radius 8 and has no composer, no outdated card and
  no local sheet. This file uses the card radius (10, §1.5), adds the composer and the outdated
  card heading its file (§4.1), titles the panel's section "Open comments" rather than the
  board's capitals, and gives the local "Finish round" sheet a "Comments" column without the
  include checkboxes and validation rows `GlassSubmit` shows for a PR (§4.2).
- The boards show a first round only. This file adds what later rounds bring (§4.1, §4.4): the
  panel's Since round n | All changes toggle, the `round n` side label, "· moved" on a card, the
  comments list's "all changes" tag, the empty interdiff and the agent's ready banner.
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
