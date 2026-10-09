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
`prop #f694ff`, `punc`/`plain #edecee`, `comment` = `fg3` (`#9d9ab3`). Highlighting produces these
token kinds, never colours (PLAN §6 step 4); each paints in its token.

### 1.2 Light (new; "Aura Light")

Same token names. Key values: `fg #1d1b26`, `fg2 #46434f`, `fg3 #5e5a6f`; `accent #6d43e0`,
`accentFg #ffffff`, `accentText #5a32cc`; `green #0d7553`, `red #bf2537`, `yellow #875600`,
`blue #0b6687`; `chrome` Acrylic `rgba(247,246,251,.58)` / Mica `rgba(239,238,244,.85)` / Opaque
`#efeef4`; `code` `rgba(255,255,255,.93)` / `rgba(254,254,255,.975)` / `#fdfdfe`; `scrim
rgba(36,32,58,.30)`; diff `addBg rgba(18,170,115,.10)`, `delBg rgba(225,45,70,.075)`. Syntax: `kw
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
  turn, PLAN §3 workflow A step 5), then the open ones, each group in stream order (an accent
  ring dot for `open`, a blue dot and "addressed" for `addressed`, `path:line` in mono where the
  thread stands now (an old-side thread's ends in "(old)", as the CLI prints it, so its lines
  aren't taken for the file's lines now; while its file's text hasn't landed — briefly, since
  every file a thread shows in loads at once — the line it was written on, in `fg3`), the first
  line of the
  comment as plain text — its Markdown dropped, a fenced block's first line of code, as in every
  one-line summary of a comment; an outdated thread says "outdated" in `yellow` and shows its original first line in
  mono; a thread the interdiff doesn't show carries a quiet "all changes" tag in `fg3`, and its
  `path:line` and "outdated" are where it stands in all changes), then
  "n resolved · show", which lists the resolved ones too; with none waiting, "No open
  comments". A click on a row shows and focuses the thread in the diff — an "all changes" row
  switches the stream to all changes first; the focused thread's row is `sel`. "Copy as prompt" puts the `open` threads on the clipboard as Markdown (`path:line` where each stands now, as its row,
  "(where it was written; dv hasn't placed it yet)" beside one not placed yet, the thread id,
  the anchored code, the comment and its replies), for pasting to an agent; in a
  panel narrower than 270 it is its copy glyph alone, with the words in a tooltip, so the header
  never runs past the panel. The list grows to 240 high, then scrolls. The panel's right edge is a resize handle (a 2px accent line while hovered; a double click restores 280).
- **Diff:** one stream over every file (PLAN §6.7). Each file starts with its header: a 44-high
  `codeHead` bar — chevron, path (directory in `fg3`, name at 600; a renamed file shows
  `old → new`), status pill, totals, a quiet "Loading…" once its diff takes longer than 300 ms,
  Split|Unified segmented control with its `S` keycap, "Viewed" checkbox with `V`, previous / next
  file, and `⋯` (a menu with Copy path and Reveal in folder) — over 24-high side labels (`main` /
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
  selected lines of one side. Inline thread cards go between rows, and the lines a card sits
  on stay shown (a thread relocation moved can stand on lines no change is near, which would
  otherwise fold into a gap).
- **Interdiff** (round n+1 of a live target, PLAN §2.3): the stream, the file list and its
  totals are what changed since round n (its snapshot → now); `I` or the panel's toggle switches
  to all changes and back, the reading position staying where the same file and line still
  show. Its new side is the same text as in all changes, so new-side threads show there as in
  all changes; old-side threads, and threads whose file isn't in the interdiff, aren't drawn in
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
  ("Open", "Addressed by claude-code", "Resolved"), "Outdated" when it is, the anchor in `fg3`
  and the reviewer's actions as small buttons (Resolve on an open thread, Resolve and Reopen on
  an addressed one, Reopen and a chevron on a resolved one; in a narrow card the anchor, then the
  chip's agent name, truncate, never the actions); an addressed thread's note follows
  on `blueSoft` with the agent's glyph. A resolved thread collapses to that header with its
  comment's first line until its chevron opens it (or while a reply to it is being written). A
  thread whose code moved (lines inserted above it, its block moved, a small edit) follows it to
  its new lines. An outdated thread (relocation can't find its code at all, PLAN §8) has no line
  to follow: its card heads its file, under the header, with the lines it was anchored on in a
  `recess` block. In a file shown without lines (too large to show, or "No content change": a
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
  the diff, posting focuses the thread. Hidden by its file collapsing, it keeps its text and
  gives the keyboard back to the diff (`C` opens the file and takes it up again). A
  comparison that can't hold it — an interdiff that doesn't list its file, or (it being on the
  old side) whose old column is the round's snapshot — leaves it undrawn with its text and
  lines, until all changes show it again; `C` then switches to all changes and takes it up. Its
  file leaving the review (the agent reverted it) drops it.
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
  when there are none; an interdiff adds "· since round 1"); at the right, `Ctrl Enter finish
  review` while the review can still be finished.

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
  keycap), no reviews ("Pick a repo to start a review"), no open comments. The switcher with no
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
| `J` / `K` | next / previous file: its header at the top, the cursor on its first change |
| `N` / `P` | next / previous block of changed lines, on into the next files |
| `]` / `[` | focus the next / previous thread that isn't resolved (open or addressed), scrolling to it, on into the next files (in an interdiff, the threads it shows) |
| `I` | switch between the interdiff since the last round and all changes (the panel's toggle; while the review offers it, PLAN §2.3) |
| `V` | toggle viewed on the current file (`V` then `J`: mark viewed and go next); each press flips the state the last one asked for, however fast they come |
| `C` | comment on the selected lines, else the cursor's line, else (no cursor yet) the first line in view, where the cursor goes too (click a line number to put the cursor there, shift-click to select to it); a composer holding a draft takes the keyboard instead, wherever the cursor is. In an interdiff it comments on the new side only: on the old column (the round's snapshot, which no anchor names) it does nothing |
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
| `Esc` | close the top overlay, else the file menu, else cancel the composer, else clear the selection |
| `↑` / `↓`, `Enter` | in the switcher and the source picker: move (wrapping), open |
| `Enter` / `Esc` | in the file filter: show the first file left / clear the filter (both return to the diff) |
| `Ctrl Enter` / `Esc` | in the composer: post / discard (both return to the diff; `Enter` is a newline) |
| `↑` / `↓`, `1` / `2`, `Tab` | in the "Finish round" sheet: choose the verdict (Approve, Request changes) / move between the verdicts and the summary; `Ctrl Enter` finishes the round, `Esc` closes |

macOS uses `Cmd` for every `Ctrl` above. Single-letter keys are inactive while a text field has focus.
Keycaps render with `kbd` style (11px mono, `line2` border).

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
