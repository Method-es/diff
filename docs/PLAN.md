# dv: rewrite plan

dv is a native desktop app (Rust + GPUI) for reviewing diffs locally — above all an AI coding agent's
work before it is committed — and for reviewing and submitting large GitHub PRs that github.com handles
badly. Windows first with first-class WSL; macOS and Linux supported. Coding agents read and answer the
review through the `dv` CLI.

- **Visual spec:** [DESIGN.md](DESIGN.md), plus the design canvas (row 2, "Glass", is the chosen
  direction): (a private canvas)
- **Status:** plan, written 2026-10-04 after a full review of the first implementation (§15). Nothing
  here is built yet. Sections are cited from code and commits as `PLAN §N` / `D<n>`.

Comment and doc rule for the rewrite: code comments explain *what something is and why*, in the present
tense, citing `PLAN §N` / `DESIGN §N` where a spec decision is involved. History (review findings, phase
labels, "used to…") belongs in commit messages, never in comments.

---

## 0. Goals and non-goals

**Goals (v1)**
1. Review an agent's uncommitted or branch work in a local or WSL repo: fast diff, viewed tracking,
   line/range comments, threads, rounds with "changes since last round" (§2, §3).
2. A CLI that lets agents in Windows *or* inside WSL list, answer and address review threads and wait
   for the next round (§10).
3. Open a GitHub PR as a review, comment on it, pull GitHub's own threads, and submit one GitHub review
   with a verdict (§9).
4. WSL repos behave exactly like local ones: native speed, live updates while the agent edits (§5).
5. First-run onboarding and a re-runnable setup/doctor that install the WSL backend, put `dv` on PATH,
   install agent skills and check `gh` — per environment (§12).
6. A per-user installer with delta auto-updates (Velopack); Linux builds; macOS kept compiling (§13).
7. Hermetic automated testing: fixture repos, a fake `gh`, GUI e2e scenarios (§14).
8. Read the code around a change as an editor would: select and find text, view whole files, and
   hover, go to definition and find references through the project's own language servers, run
   where the repo lives (§18).

**Non-goals (v1):** AI features inside dv, editing files (and so a language server's diagnostics,
completion, rename and code actions, §18.1), committing or any other mutation of the user's repo
(§2.4), cross-machine review sync, GitHub Enterprise-specific work
beyond what `gh` gives for free, merge-conflict badges.

---

## 1. Decisions

### D1. UI stack: `gpui-pre` + `gpui-component` 0.6, exact pins (same as the sibling app)
`gpui = { package = "gpui-pre", version = "=0.3.6" }` (+ `gpui-pre-platform`), `gpui-component =0.6.6`,
declared once in `[workspace.dependencies]`, bumped together. The first implementation sat on zed git
main + `gpui-component 0.5.2`, a line that is no longer maintained, and every `cargo update` was a
big-bang risk. Same stack as the sibling app means the sibling app's `gpui_automation` crate and its lessons carry over
directly (§14). Grep APIs under `~/.cargo/registry/src/*/gpui-pre-0.3.6/` and `gpui-component-0.6.6/`.

### D2. Rewrite in place, salvage deliberately
Same repository, work on a `rewrite` branch with fresh crates (§4). Port code from the first
implementation only through the salvage list (§15) — read it, keep what is listed, rewrite the rest.
The old crates are deleted when v1 reaches parity on workflows A–C (§3).

### D3. The review is the unit you open
A review owns exactly one fixed target. There is no "open a repo and guess which review" step.
Full model in §2.

### D4. git computes the diff
Per file, dv runs `git diff --no-ext-diff --no-textconv --diff-algorithm=myers --indent-heuristic
--submodule=short --ignore-submodules=untracked --no-color --src-prefix=a/ --dst-prefix=b/ -U<N> -M
<old> <new> -- :(literal)<path>` (an untracked file: `git diff --no-index … -- /dev/null <path>`,
where exit code 1 means "differs", and a file named `-`, which would mean stdin there, is passed as
`./-`) and parses the result. `N` exceeds the file's line count — a file of `n` bytes has at most
`n + 1` lines, so `N` comes from the size check (§6 step 2) — and with context larger than the file
the output *is* both complete, aligned files, so dv collapses unchanged runs into expandable gaps
itself. This gives git's semantics for free: `core.autocrlf`, `.gitattributes` (eol, `-diff`,
`binary`, clean filters) and rename detection. `--no-textconv`, because comment anchors and GitHub's
line numbers refer to the real file's lines, not converted text; `--diff-algorithm=myers` overrides
a user's `diff.algorithm` and a driver's `diff.<driver>.algorithm` (which beats the git policy's
`-c diff.algorithm=myers`, §5.1), so dv shows what GitHub shows; `--indent-heuristic` (git's
default) overrides `diff.indentHeuristic=false`, which would mark a different run of a block that
can slide than GitHub marks, and so move comment anchors off the lines GitHub shows as changed;
`--submodule=short` keeps a gitlink's patch the `Subproject commit` lines dv reads whatever
`diff.submodule` says, and `--ignore-submodules=untracked` (git's own default) keeps a
`diff.ignoreSubmodules` or `.gitmodules` `ignore` setting from hiding a submodule bump; the fixed
prefixes override `diff.noprefix` / `diff.mnemonicPrefix`. The file list (§6 step 1) carries the
same pinned flags, so its totals match the hunks. Binary detection still follows `.gitattributes`.
dv adds only intraline word ranges and syntax highlighting. (The first implementation diffed raw
bytes itself and showed every edited CRLF file as fully rewritten.)

### D5. Reviews are append-only event logs
`<git-common-dir>/dv/reviews/<id>.jsonl`, one JSON event per line, appended with a single `write` on a
file opened `O_APPEND`. Readers fold events into state. No lock file, no read-modify-write, so the GUI,
the CLI and any number of agents can write concurrently. Details in §7.

### D6. WSL: one Linux `dv`, mandatory backend
The Linux build of `dv` (static musl x86_64 + aarch64) is both the CLI inside the distro and the backend:
`dv backend --stdio`. The Windows GUI talks to it over the stdio of one `wsl.exe -d <distro> --exec`
child per distro. For a WSL repo this is the **only** path — no per-command `wsl.exe` fallback, no
optional accelerator. If it can't start, the repo shows an actionable banner (§12.3). The same `Backend`
interface is implemented in-process for local repos (§5), so there is one code path above it.

### D7. GitHub through `gh`, explicitly
All GitHub calls go through the `gh` CLI (auth, SSO, enterprise for free; no token storage), with
`GH_PROMPT_DISABLED=1`, and through a `DV_GH` override that tests point at `mock_gh` (§14). The GitHub
repository is resolved from `gh repo set-default`, else `upstream`, else `origin` — never hard-coded.
Every list is paginated.

### D8. Window material: Mica by default
Windows 11: `MicaBackdrop` by default, `Blurred` (Acrylic) as a setting, `Opaque` automatically on
Windows 10, when "transparency effects" is off, on Linux, and as a setting. macOS: vibrancy via
`Blurred`. GPUI only blurs what is *behind* the window; in-app overlays are opaque tinted layers over a
scrim (DESIGN §3). Acrylic uses the undocumented composition API and can lag while dragging on some
Windows builds — hence Mica as the default. gpui sets Mica through `DWMWA_SYSTEMBACKDROP_TYPE`, which
only build 22621+ has, so earlier Windows 11 builds (22000–22620) resolve Mica to Opaque too; a
material the platform can't show always resolves to Opaque (`dv_ui::platform`).

### D9. Packaging and updates: Velopack
- **Packages:** `vpk pack` produces a one-click per-user `Setup.exe` on Windows (no wizard, no
  admin, installs under `%LOCALAPPDATA%\dv\`), a `.pkg` on macOS and an AppImage on Linux, plus the
  full and delta update packages and a release feed, all from a build without the automation feature.
  `vpk` is a .NET tool: the build machine needs the .NET SDK (not installed yet; only Phase 7 needs it).
- **In the app:** the `velopack` crate. `VelopackApp::build().run()` is the first call in `main` of
  both Windows binaries; its install/uninstall hooks run `dv setup path --yes` / remove the PATH entry,
  so PATH stays app-owned (§12) and the installer has no logic of its own.
- **Updates:** `UpdateManager` checks a release feed, downloads the delta in the background and
  applies it on the next restart ("Restart to update" in the status bar). Until there is a release
  workflow (§16 phase 10) the feed is a local folder of `vpk pack` output, so a locally built release is
  picked up by the installed copy; later the same code points at GitHub Releases.
- Chosen over cargo-packager (the sibling app's tool; a classic NSIS wizard and an updater that re-downloads the
  whole installer and needs manifest signing keys) for delta updates and the simpler install flow.

### D10. Code signing: none
The owner builds and installs locally, which SmartScreen doesn't challenge (it checks files carrying a
downloaded-from-the-internet mark). Revisit only if dv is ever distributed as a download.

### D11. Code intelligence through the project's own language servers
Hover, go to definition and find references come from standard language servers (LSP) that dv starts
where the repo lives, through its backend (a WSL repo's run inside the distro), found in the project
first (TypeScript 7's native server ships in the `typescript` package) and never installed by dv
(§18). Each language is a spec in `dv_lsp`, so adding one doesn't touch the rest. Chosen over
tree-sitter navigation (quick, but by name only: no types, wrong across files) and over an embedded
editor (dv reads, it never edits).

---

## 2. Concept model

### 2.1 Entities

```
Repo          a clone at a Location: Local(<path>) | Wsl{distro, path}
 └─ Review    the unit of work; id "r-<ulid>"
     ├─ Target          fixed at creation, never changes (§2.2)
     ├─ Rounds          1..n snapshots of the target (§2.3)
     ├─ Viewed          per-round set of paths the reviewer marked viewed
     ├─ Threads         anchored comment + replies + state (§2.5)
     └─ Status          §2.4
```

### 2.2 Targets

| Target | Old side | New side | Moves? |
|---|---|---|---|
| `working` | `HEAD` (the empty tree on an unborn branch) | working tree incl. untracked (not ignored) | yes, live |
| `staged` | `HEAD` (the empty tree on an unborn branch) | index | yes, live |
| `branch <b> [vs <base>]` | `merge-base(<base>, <b>)` | `<b>`'s tip, or the working tree of the worktree that has `<b>` checked out | yes |
| `commit <rev>` | `<rev>`'s first parent (the empty tree for a root commit) | `<rev>` | no |
| `range <a>..<b>` / `<a>...<b>` | `<a>` / merge-base | `<b>` | no |
| `pr <owner/repo#n>` | merge-base(base, head) | PR head | on push |

`<base>` defaults to the repo's default branch: what `origin/HEAD` points at (`origin/main`), else a
local `main`, else `master`. `<b>` is the ref its name stands for however it is spelled (`feat`,
`heads/feat`, `refs/heads/feat`; a local branch before a tag of the same name, as `git switch`
picks), and that one ref gives both the tip and the checked-out test. Unrelated histories (no merge
base) are an error, not an empty diff.
`dv_core::target::Target::resolve` turns a target into a `DiffSpec` of two sides — a commit id, the
empty tree, the index or the working tree. A target stores its spec *and* the resolved oids at each
round, so a review never silently changes meaning (the first implementation froze `base` but left
`head` symbolic).

**A review's home worktree** (`dv_core::review::home`) is where its target resolves, whichever
worktree reads it: a `working` or `staged` review's is the worktree it was made in (its
`created` event's `worktree`, §7); a `branch` review's is the worktree that has `<b>` checked out:
the worktree asking, when it has; else the one the review was made in, when it has; else one
`git worktree list --porcelain` lists with it (git checks a branch out in one worktree only
unless told otherwise, `git worktree add --force`; the bare repository, worktrees whose
directory is gone, and the git dir git lists as the main worktree of a repository whose git dir
is kept apart from its files — `git init --separate-git-dir`, a submodule — are left out). A
branch checked out nowhere has none: its tip, the same from everywhere. `commit`, `range` and `pr` targets have none either.
The GUI (`Reviews::open`: every open, by id or by target, from the switcher, the source picker,
`dv open --review`, automation's `open_review`) and the CLI (`Repo::for_review`) both bind to it,
so a review lists the same files, places its threads at the same lines and snapshots the same
content from every worktree. A `working` or `staged` review whose worktree is no longer one of
the repository's (removed, moved, or deleted and waiting for `git worktree prune`) has no home
(`HomeError::Gone`): the GUI's main-area banner, the CLI's exit 1 (`target`), never a resolution
in some other worktree. In the GUI the home is decided as the review opens; a branch checked out
in another worktree after that shows once the review is opened again.

### 2.3 Rounds
A round is the target as it stood when the reviewer last handed the review over.

- **Local targets:** when the reviewer finishes a round ("Request changes", "Approve", or explicitly
  "Snapshot round"), dv snapshots the new side: a temporary index (`GIT_INDEX_FILE`) + `git add -A` +
  `write-tree` + `commit-tree`, stored at `refs/dv/reviews/<id>/<n>`. Round n+1 defaults to the interdiff
  *round n → now*, with a toggle to the full diff. The interdiff is `DiffSpec::since(&round)`: the
  round's snapshot (`Side::Commit(round.new_oid)`) as the old side, the target's new side as it
  resolves now (§6 step 6). `DiffSpec::since_checked` first makes sure the snapshot commit is still
  in the repository (`git cat-file -e <oid>^{commit}`); a missing one (a ref deleted by hand and
  pruned) is `TargetError::MissingSnapshot`, for the GUI to show as a banner while it falls back
  to all changes. A fixed target (`commit`, `range`) has an empty interdiff.
- **The comparison in the GUI** (`dv_ui::session::Comparison`: since round n, or all changes)
  is session state, never persisted. A review opens on *since its last closed round* when it
  has one, isn't finished and its target is live (`working`, `staged`, a branch), otherwise on
  all changes; a fixed target and a PR review never offer the toggle (DESIGN §4.1). A round
  that closes moves it to *since* that round, a review that finishes to all changes. Switching
  (`I`, the panel's toggle, an "all changes" row of the comments list, "Show all changes")
  re-resolves the spec and lists again under a new generation (§11), keeping the reading
  position where the same file and line still show. Viewed marks stay per round: an
  interdiff's files are marked in the round in progress. The CLI always works on all changes
  (§10.1).
- **Snapshot detail** (`dv_core::review::snapshot`): a commit side is kept as it is. For the index
  and the working tree, the worktree's index file is copied byte for byte, modification time
  included (git's racy-entry check compares against it), into the store (`tmp/index-<ulid>`,
  `Backend::copy_index_to_store`; no index yet starts an empty one). The working tree then runs
  `git add -A --ignore-errors` into the copy (tracked + untracked, ignores honoured, the same clean
  filters and eol conversion `git diff` applies; unresolved conflicts stage as the files stand,
  markers included; a nested repo without a commit, which git can't stage, is left out, but any
  other file git can't read — one another program holds open without sharing — fails the round
  with `SnapshotError::Unreadable` naming it, because the copy would keep a version the reviewer
  wasn't shown),
  and both run `write-tree` on it (an index with unresolved conflicts has no tree:
  `SnapshotError::Unmerged`). Both calls pass `-c core.splitIndex=false` (a split index would
  otherwise write a new shared index into the git dir) and `-c core.safecrlf=false`.
  `commit-tree --no-gpg-sign` with `-c user.name=dv -c user.email=dv@dv.invalid` (works without a
  configured identity; `GIT_AUTHOR_*`/`GIT_COMMITTER_*` variables still win), the old side's
  commit as parent when it is one, message `dv review <id> round <n>`. The ref is created with
  `git update-ref <ref> <new> <zero-oid>`, never blindly overwritten: a ref at the same commit, or
  at a commit of the same tree and parent (an earlier attempt whose events never landed), is
  reused; any other commit at the ref of the round in progress (such an attempt, made before the
  content changed, or before a branch moved on from the commit the last round closed at) is
  replaced with a compare-and-swap `git update-ref <ref> <new> <old>`; the ref of a closed round
  is never replaced (`SnapshotError::RefExists`). The temp index and its `.lock` are removed
  afterwards, also on failure, and each snapshot first sweeps `tmp/index-*` older than an hour
  (by the ULID's time), which a dv killed mid-snapshot leaves. `review::ops::finish_round`
  (reviewer only; not for a finished or a PR review; the
  verdict is `approve`, `request_changes` or none) resolves the target, snapshots, then appends
  `round_closed` (old side, snapshot, verdict, summary) and the verdict's status
  (`changes_requested` / `approved`; none leaves it), both checked before either is written.
- **PR targets:** each submitted GitHub review closes a round at the PR head it was submitted against.
  New commits on the PR offer "Start round n+1" (a new draft, interdiff since the last submitted head).

### 2.4 Status and invariants
- **Local review:** `open` → `changes_requested` ⇄ `in_progress` (agent ran `dv review done` →
  `ready`) → `approved` | `closed`. Never "submitted". Any of `open`, `changes_requested`,
  `in_progress` and `ready` may move to any other of `changes_requested`, `in_progress`, `ready`,
  `approved` and `closed`; agents set `in_progress` and `ready`, the reviewer `changes_requested`,
  `approved` and `closed`. Nothing returns to `open`. An agent's first write on a
  `changes_requested` review (a reply, an address or a new comment through the CLI) moves it to
  `in_progress`: one more `status` event after the write, best effort, written only when the
  review is still `changes_requested` as it stands then (`ops::start_work`) and conditional on it
  in the log itself (`if_status: changes_requested`, §7), so a status set meanwhile — the
  reviewer's verdict, the agent's own `dv review done` run beside the write, even one landing
  between that check and the append — wins; the switcher's "Agent working" chip then means the
  agent has started.
- **An agent never hands back a round it hasn't seen** (`ops::hand_back`, `dv review done`).
  `done --round <n>` names the round the agent worked on (`round.n` from the `dv review wait`
  that gave it the round, or the last of the review's rounds when it started without waiting),
  which must be the last closed one: an older one is `OpError::UnseenRound` (the CLI's exit 1
  `unseen_round`, nothing written) for the newest, one that hasn't closed exit 2. Without
  `--round`, the fold's derived `Round::closed_from` (the status the review had when the round
  closed; no event of its own) decides: a round the reviewer finished while the review was
  `in_progress` (`Review::unseen_round`) came while the agent was still working on the one
  before, whose `done` would carry a note about that round and whose next `review wait` would
  block without ever hearing of the new one, so it is `unseen_round` until the agent names the
  round. The agent's writes since don't count as having seen it: nothing in the log tells a
  reply about the new round from one still going down the old round's list. A round closed from
  `open` (the first) or from `ready` (after a handback, a summary-only round fixed in code
  alone) came while the agent waited for it and needs no number, nor does a second `done` once
  the review is `ready`. The `status` event `done` writes is conditional on the closed rounds
  it was checked against (`if_round: n`, §7), so a round that closes between the check and the
  append wins: the fold makes the late `ready` a no-op, and `hand_back` reads the log back after
  appending and reports `finished` when that round finished the review, else `unseen_round`.
- **PR review:** `draft` → `submitted` (read-only, owned by GitHub from then on: every later event is
  refused). The next round is a new draft review linked to the same PR. A draft may be abandoned
  (`closed`).
- `approved`, `closed` and `submitted` are final; every other status counts as open. A finished
  local review is read-only for everyone, the CLI included: `review::ops` refuses a new thread,
  a reply, a thread state change, an edit, a deletion or a viewed mark on an `approved` or
  `closed` review (`OpError::Finished`, the CLI's exit 1 `finished`), as the GUI shows it
  (DESIGN §4.4); a submitted PR review refuses every later event in the fold itself. A finished
  local review moving to another status (`ops::set_status`: an agent's `ready` racing the
  approval, a `closed` after it) and a round finished on one are `OpError::Finished` too.
- **A verdict's status always lands after its round.** `ops::finish_round` writes the verdict's
  `status` event even when the review already had that status as it read it: a status set while
  the round's snapshot is taken (the agent's `dv review done`) would otherwise stand after
  `round_closed` and outlive the verdict. With nothing between, the fold reads the repeat as a
  no-op.
- **At most one open review per target.** Opening a target resumes it; "New review" on a target that has
  one asks "Resume or start fresh?". `working` and `staged` targets are per worktree (each worktree has
  its own changes); the others are the same target from every worktree, and a `pr` target is the same
  PR whatever head it was fetched at (`dv_core::review::ops::open_review`). Wherever a review was
  opened, it is read in its home worktree (§2.2).
- A repo has many reviews; a PR has many reviews over time; local reviews need no PR.
- **dv never mutates the user's repo.** Allowed writes: `git fetch`, `.git/dv/**`, `refs/dv/**` and the
  objects those refs need. No commit, checkout, stash, reset or index change, ever.

### 2.5 Threads
- **Anchor:** round, path (and old path for renames), side (old/new), line range, **plus the anchored
  lines' text and 3 lines of context either side** (§8).
- **States:** `open` (waiting on the agent/author) → `addressed` (agent says done, with an optional note)
  → `resolved` (reviewer confirmed) or back to `open` ("Reopen"). Only the reviewer resolves and
  reopens (`ops::set_thread_state` refuses an agent's resolve or reopen as `NotAllowed`, whatever
  the thread's state, saying it is the reviewer's). Agents (and the reviewer) address; addressing
  again replaces the note; a resolved thread is reopened before it can be addressed. Replies by anyone. Only an item's author edits or deletes it
  (a deletion is a tombstone).
- **Authors:** `reviewer` (git identity), `agent` (`--author`, default from the skill: `claude-code`,
  `codex`, …), `github` (pulled threads, read-only: only GitHub's own events change them, and a
  `github` author changes no other thread).

### 2.6 Promotion (later, not v1)
When a branch reviewed as `branch feat vs main` becomes PR #n, dv offers to carry open threads into the
PR's draft review, re-anchored against the PR diff.

---

## 3. Workflows

### A. Agent's local work (the core loop)
1. The agent works in `~/code/api-server` (WSL). dv's switcher shows that repo with "uncommitted
   changes · 7 files".
2. The reviewer picks **Review** → a `working` review opens (or resumes).
3. They read, mark files viewed (`V`), comment (select lines, `C`), then **Request changes**
   (`Ctrl Enter`). Round 1 is snapshotted; status `changes_requested`.
4. The agent (blocked in `dv review wait`) wakes, runs `dv comment list --status open`, fixes,
   `dv comment reply` / `dv comment address <id> --note …`, then `dv review done`.
5. dv shows round 2 as *round 1 → now*, addressed threads first with **Resolve** / **Reopen**.
6. **Approve** closes the review. The reviewer (or the agent, told by the skill) commits.

### B. Your branch before a PR
Workflow A with target `branch feat vs main` (includes uncommitted changes while `feat` is checked out).
Finish with `gh pr create`; promotion (§2.6) later.

### C. A GitHub PR
1. `dv open #482` / a pasted URL / `Ctrl G`. dv fetches base and head, resumes or creates the PR's draft.
2. GitHub's existing threads appear read-only alongside the reviewer's drafts (paginated, §9.2).
3. **Finish review** (`Ctrl Enter`) → summary, verdict, the list of comments to post. Each is validated
   against GitHub's hunks; anything outside them offers *Post as file comment* or *Keep local* (§9.3).
4. Submit posts one GitHub review; the draft becomes read-only, round 1 closes.
5. The author pushes; dv marks the PR updated and offers *Start round 2*.

### D. Agent-launched review (later)
An optional Claude Code `Stop` hook (installed by `dv setup agents`) runs `dv open --review-current`,
bringing dv to the front on the agent's working-tree review when the agent finishes a turn.

---

## 4. Workspace layout

| Crate | Kind | Role | gpui? |
|---|---|---|---|
| `dv_core` | lib | Location, the `Backend` trait and `LocalBackend` (§5.1), Target and its labels, git runner, diff pipeline (§6), review events + fold (§7), anchors, relocation and placement (§8), GitHub via `gh` (§9) | **never** |
| `dv_backend` | lib | The stdio protocol (§5), server loop, `Connection`s (launch, reconnect) and the `Connections` registry, `RemoteBackend` client, distro install | never |
| `dv_cli` | lib | clap command tree, JSON output DTOs, setup/doctor checks, a WSL repository's commands run inside its distro (§10, §12) | never |
| `dv_ui` | lib | views + services (§11) | yes |
| `dv` | bin | entry: GUI (the `gui` feature), CLI and `dv backend --stdio`; on Windows also the console/GUI split; three shapes, one of them headless (§13.1) | with `gui` |
| `gpui_automation` | lib + bin `e2e` | ported from the sibling app (§14) | yes |
| `mock_gh` | bin | fake `gh` for tests (§14.2) | no |
| `dv_test_support` | lib (dev) | fixture repo builder, shared by every crate's tests | no |

Direction: `dv` → `dv_ui` → {`dv_cli`, `dv_backend`, `dv_core`, `gpui-component`};
`dv_cli` → {`dv_backend`, `dv_core`}; `dv_backend` → `dv_core`. A gpui import outside `dv_ui`, `dv`
and `gpui_automation`, or in `dv` built without `gui`, is a build failure (`script/check` verifies
it with `cargo tree`, §13.2).

Rules: errors are `anyhow` in app/CLI code and `thiserror` enums in libraries; no `unwrap()`/`expect()`
outside tests unless the message states the invariant; files stay under ~1,500 lines (split by
responsibility, `mod.rs` / `render.rs` / `tests.rs` per view).

---

## 5. The backend

### 5.1 Interface
One trait, `dv_core::backend::Backend`, two implementations, so everything above it is
location-agnostic. The trait and `LocalBackend` live in `dv_core`, because the diff pipeline and the
review store there are written against it; `dv_backend` (which depends on `dv_core`) holds the stdio
protocol, the server loop, `RemoteBackend` and the distro install.

A backend is bound to one worktree, opened from a `Location`: git runs in its root, worktree paths
are relative to that root, store paths are relative to `<git-common-dir>/dv/` (§7). Every method
blocks and takes and returns plain data, so the stdio protocol serialises calls 1:1:

```rust
pub trait Backend: Send + Sync {
    fn repo(&self) -> &RepoInfo;     // location, toplevel, git dir, common dir, object format, git version
    fn git(&self, args: &[&str], opts: &GitOpts) -> Result<GitOutput, BackendError>; // stdin, timeout, ok codes
    fn worktree_read(&self, rel: &str) -> Result<Option<Vec<u8>>, BackendError>;
    fn worktree_len(&self, rel: &str) -> Result<Option<u64>, BackendError>;
    fn worktree_text_stats(&self, rel: &str) -> Result<Option<TextStats>, BackendError>; // size, lines, binary sniff (§6)
    fn store_append(&self, rel: &str, line: &[u8]) -> Result<(), BackendError>;   // one write, O_APPEND (D5)
    fn store_read(&self, rel: &str) -> Result<Option<Vec<u8>>, BackendError>;
    fn store_list(&self, dir: &str) -> Result<Vec<String>, BackendError>;
    fn store_write_atomic(&self, rel: &str, bytes: &[u8]) -> Result<(), BackendError>; // temp + rename
    fn store_remove(&self, rel: &str) -> Result<bool, BackendError>;
    fn copy_index_to_store(&self, rel: &str) -> Result<bool, BackendError>; // §2.3 snapshots
    fn watch(&self, spec: WatchSpec, sink: WatchSink) -> Result<WatchHandle, BackendError>; // §5.3
}
```

`GitOpts::index_file` names a store file (a store path, validated as one) that a single `git` call
uses as its index: the backend resolves it under `<git-common-dir>/dv/` and exports it as
`GIT_INDEX_FILE` after the policy has removed any inherited one. Round snapshots stage into a copy
of the index this way (§2.3), so the user's index is never written.

- **Paths** are validated before use: no absolute paths, no `..`, `.` or empty components, no
  backslashes (separators on Windows); store paths use only `[A-Za-z0-9._-]` components that don't
  start with `.`. Worktree reads see a file as git does: a symbolic link is never followed and
  reads as its target path, which is what git stores and diffs for a link.
- **Git invocation policy** (`dv_core::git::command`, the only place a git process is built):
  `LC_ALL=C`, `--no-pager` and `GIT_PAGER=cat`, `GIT_TERMINAL_PROMPT=0`, `GIT_OPTIONAL_LOCKS=0`,
  `-c core.quotepath=off -c color.ui=never -c color.diff=never -c diff.algorithm=myers
  -c diff.autoRefreshIndex=false`, the inherited variables that point git elsewhere or change how
  pathspecs and diffs are read (`GIT_DIR`, `GIT_INDEX_FILE`, `GIT_CONFIG_PARAMETERS`,
  `GIT_LITERAL_PATHSPECS`, `GIT_DIFF_OPTS`, `GIT_ATTR_SOURCE`, …) removed, and `CREATE_NO_WINDOW` on
  Windows. `color.diff` needs its own override because a configured `color.diff=always` beats
  `color.ui`. A driver's `diff.<driver>.algorithm` beats `-c diff.algorithm`, and only flags beat a
  `.gitmodules` `ignore` setting, so the diff commands also pin git's diff presentation with flags
  (D4). `diff.autoRefreshIndex=false` because the worktree forms of `git diff` rewrite the index
  after a stat refresh even under `GIT_OPTIONAL_LOCKS=0` (checked 2026-10, git 2.53), which would
  break §2.4 and race an agent's `git add` for `index.lock`. Errors carry the arguments, exit code
  and stderr; their `Display` is a one-line summary (DESIGN §4.4).
- The user's own git configuration otherwise applies (autocrlf, attributes, excludes). Tests are
  hermetic instead: `LocalBackendOptions::env` overlays the environment of every git the backend
  spawns, and tests pass `FixtureRepo::env_overlay` (no global or system config or attributes, fixed
  identity), so results don't depend on the developer's machine (§14.1).
- `LocalBackend` — in-process: spawns `git` directly, `notify` for watching (local repos get live
  updates too, which the first implementation lacked).
- `RemoteBackend` — JSON-lines RPC to `dv backend --stdio` in the distro; that server is a
  `LocalBackend` running natively on Linux. Binary payloads are base64. Requests carry ids;
  responses and watch events interleave on stdout.

**The wire** (`dv_backend::proto`, `PROTO = 1`): JSON, one object per line (`\n`), UTF-8, over the
server's stdin and stdout. A request is `{"id": <u64>, "op": "<name>", …args}`; a response
`{"id": n, "ok": <value>}` or `{"id": n, "err": <WireError>}`; a watch batch `{"watch": <u32>,
"event": <WatchEvent>}`. The server runs each request on a thread of its own (a long `git` must
not hold up a `store_read`) and answers in the order calls finish; the client matches ids. Every
byte field (git's stdin, stdout and stderr, file contents, store lines and bytes) is base64
(standard alphabet) in dedicated wire types: `GitOpts` and `GitOutput`, whose bytes would
serialise as JSON number arrays, never cross as they are; `RepoInfo`, `WatchSpec`, `WatchEvent`
and `TextStats` do. The ops:
- `hello {proto, build}` → `{proto, build_ok, pid, os, arch, version}`: the client's first request.
  `build_ok` says whether the sha256 of the server's own executable is `build` (§5.2), `null` when
  none was sent; the executable is hashed only when asked.
- `open {path, env}` → `{repo, info}`: a `LocalBackend::open_with(path, {env})` kept under the
  handle `repo` (`env` is the overlay: the loopback tests send the fixture's, the app none);
  `close {repo}` forgets it.
- One op per `Backend` method, naming its `repo`: `git {args, opts}`, `worktree_read`,
  `worktree_len`, `worktree_text_stats`, `store_append`, `store_read`, `store_list`,
  `store_write_atomic`, `store_remove`, `copy_index_to_store`; `watch {repo, spec}` →
  `{watch, worktree_error}`, whose batches the server holds back until that answer is out;
  `unwatch {watch}`.

`WireError` carries every `BackendError` variant with its fields (an `io::Error` as `{kind,
message}`), so the client rebuilds the same variant, whose `Display` and `detail()` read as they
did on the server; `protocol` answers a line that isn't a request (id 0 when none can be read),
after which the server serves on, and reaches the caller as `Unavailable`. One writer thread
owns the server's stdout and writes whole lines (logging goes to stderr). When stdin ends the
server stops its watches, writes out what it has queued and exits, whatever calls are still in
git: that is how a server dies with its client.

**The client** (`dv_backend::client`): a `Connection` (one per distro, or a named loopback)
starts its server through its `Launcher` (`dv_backend::launch`) on the first call and shakes
hands; every `RemoteBackend` of that server shares it, each call blocking its thread until its
reply comes. `Launcher::Command` runs a child process with piped stdio (`CREATE_NO_WINDOW` on
Windows, where the detached app has no console), such as a loopback `dv backend --stdio`, and
may name the build hash `hello` checks; `Launcher::InProcess` serves on a thread over in-memory
pipes; `Launcher::Wsl` starts a distro's server through `wsl.exe`, installing this app's Linux
build there first when it is missing (§5.2). A launcher also answers whether its host runs
(`host_running`: a distro's state, listed afresh; `None` for this machine), which every
automatic start asks first (§5.2's connecting rule). Every process a launcher starts, and every
`wsl.exe` dv runs, joins a job object this process holds with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`
(`dv_backend::job`), so it dies with dv however dv ends. A reader thread routes replies by id and watch batches to their watch's channel, which
delivers to the sink on a thread of the watch's own, so the `WatchHandle` contract (§5.3) holds
over the wire. Before the first reply, lines that aren't the protocol are kept as diagnostics
(`wsl.exe` can print warnings on stdout first); after it, such a line, or a reply nobody asked
for, is a protocol failure. `repo().location` is `Location::Wsl { distro, path: <toplevel> }`
through a distro and the server's own path through a loopback. A server that dies (its output
ends, a read or write fails, a protocol failure, its process exits — watched on its own, since
on Windows a git the server started inherits its stdout and can keep it open) fails every call
in flight at once with `BackendError::Disconnected` ("lost the connection to dv in Ubuntu: …");
what follows is §5.2's reconnect. A connection's `ConnectionState` (`Idle`, `Connecting`,
`Installing { done, total }`, `Connected`, `Reconnecting { attempt }`, `Stopped`, `Failed {
error }`) is readable and observable (`subscribe`, called in order on a thread of the connection's own,
which the UI forwards to its executor); `diagnostics()` gives the launcher's command line, the
state history and the servers' last stderr lines, and `kill()` kills the server as a crash
would (a test seam). The app holds its connections in a `Connections` registry, one per distro
(names compared without case, as `wsl.exe` takes them) and one per named loopback; the CLI
never uses one (§10.4). Dropping the last handle on a connection (every `RemoteBackend` and
remote watch holds one) closes its server's stdin, which ends the server, and reaps its
process; so does a server with no live watch and no call in flight for 60 s, counted from when
it last became unused — its last call ended, or its last watch went (the **idle close**: the
state is `Idle` again, the `RemoteBackend`s stay valid, and the next call starts a server
automatically), so a connection made for a listing keeps no process in a distro and WSL can
shut an unused one down; an open review holds a watch, so its server stays (one whose watch
couldn't start, an inotify limit in the distro say, holds none and may idle out: the next call,
its refresh on focus, starts the server again, and its status bar shows the review's own state
meanwhile, an idle connection being up). The
conformance suite (§14.2) runs the same cases against both implementations.

### 5.2 Lifecycle (WSL)
- **The connecting rule: only an explicit open boots a distro.** One connection per distro.
  A start is either *explicit* — the user opening a repo or a review in it (`dv open`, Open
  folder…, the switcher's **Start** and **Retry** below, all `Locations::open`) or the banner's
  Retry or Start, each `Connection::retry` — or *automatic*: the first call of a listing
  (`Locations::reach`, for the switcher's rows), the first call after an idle close (§5.1), and
  a respawn after a death (below). An explicit start connects whatever the state, booting a
  stopped distro and starting a failed or stopped connection once more. An automatic start
  first asks `wsl::running_now` — `wsl.exe --list` afresh, not the 2 s cache, since the death
  that prompts a respawn just happened — and when the distro isn't running launches nothing:
  the connection is `Stopped` (its calls fail with `Unavailable`, "Ubuntu isn't running") until
  an explicit start. So `wsl --shutdown` or `wsl -t Ubuntu` stays done, and nothing connects at
  launch for its own sake. A listing never starts a connection that failed or is reconnecting,
  and makes a new one only for a distro that is running (from `wsl::distros`); a `Stopped` one
  it starts again only once the distro runs again (the user started it). A listing asks a
  stopped or failed connection's distro from `wsl::running` — the 2 s cache, one `wsl.exe
  --list` shared by every listing asking at once — not afresh, so a stopped distro's rows don't
  each list WSL. An explicit start made while a reconnect is under way waits for that
  reconnect and takes its outcome; an open that fails only because its connection is down keeps
  its `RemoteBackend` for when it is back. **A picked review is never lost and never reported
  twice:** an open (`Reviews::open`: the connection, finding and loading the review, its home
  worktree) whose connection is lost under any step waits for the reconnect, starting nothing
  itself (`Connection::wait_for_reconnect`; a `retry` there would restart a failed connection,
  and a server that dies on every open would then loop), and runs once more. If it still fails
  for its connection (lost again, or the connection failed or found its distro stopped) the
  shell keeps it as the connection's **pending pick** (one at most; a newer pick or the
  Dismiss of its own banner drops it, another banner's Dismiss leaving it waiting): while the
  open review goes through the same connection the main area raises nothing, its backend
  banner and status bar saying what is wrong; otherwise the main area's banner reads "couldn't
  open <repo>" (the connection's error and diagnostics in Details) with Retry. When the
  connection comes back (or is back already when the open fails) the pick runs again by
  itself, its banner going, keeping open menus and the keyboard where they are. **Running by
  itself, it never replaces a review that holds unsaved text:** while the "Finish round" sheet
  is open or a comment composer (a new thread or a reply) holds a draft, the pick waits on, and
  runs once the sheet closes or the draft is posted or cancelled, if its connection is still up
  then (connected, or closed for idleness, which the next call starts again); one that went
  down again keeps it waiting. Its banner's Retry is the user's own pick and runs at once (an
  explicit pick replaces the open review as `Ctrl O` does, DESIGN §4.2). One whose open kills
  its connection every time runs again by itself at most three times, then fails in the banner
  (DESIGN §4.4). What a WSL repo's
  header says (DESIGN §4.1): a distro that isn't running, whatever its connection did before,
  "Ubuntu isn't running" and one row, **Start Ubuntu**, which connects (booting it) and then
  lists the repo as usual; a connection that failed, why, and one row, **Retry**; one that is
  reconnecting, "Ubuntu · reconnecting…", keeping the rows of its last listing. A loopback
  (§14.2) has no host to ask; the automation host's stands in for one (`backend_stop`). An
  automation run reaches WSL only through the sandbox seam: without `DV_WSL_HOME` naming a
  scratch home it gets a stand-in (`WslAccess::Scripted`) that lists the distros a scenario
  names (`wsl_distros`, running or not as it says, which its connections' host check reads) and
  refuses to start anything in one, and dv_ui's tests can't reach the real one at all
  (CLAUDE.md).
- **The Linux build** (`script/build-linux`, §13.2) ships beside `dv.exe` as `dv-linux-x86_64`
  and `dv-linux-aarch64` (`DV_LINUX_DIR` names another folder; `dv_backend::install`). Its
  **build hash** is the lowercase hex sha256 of that file, computed off the UI thread once per
  process per file (cached by path, length and modification time).
- **Launch** (`Launcher::Wsl`, `dv_backend::wsl`): `wsl.exe -d <distro> --cd / --exec /bin/sh -c
  <launch script> sh <x86_64 hash|-> <aarch64 hash|->`, with no shell on the Windows side and
  no login shell inside, so the arguments arrive as passed (checked against a real distro:
  spaces, `$`, both quotes, backslashes, newlines, an empty one). The script picks the hash for
  `uname -m` and, when `$HOME/.local/share/dv/bin/dv-<hash>` is there, prints `dv-exec <arch>`
  and execs it as `backend --stdio`; otherwise it prints one line and exits: `dv-missing <arch>`
  (120: the client installs it, then launches again), `dv-arch <machine>` (121: dv has no
  build for that architecture) or `dv-no-build <arch>` (122: this app has none; `Unavailable`
  naming the missing file and that `script/build-linux` makes it). The marker line tells a
  server that started from `wsl.exe` failing to start the distro (no such distro, WSL not
  installed): a launch whose output ends without one is `Unavailable` with `wsl.exe`'s own
  words (UTF-16LE, decoded). `--cd /` keeps `wsl.exe` from translating the app's working
  directory, which it warns about when it can't.
- Handshake: `hello` carries the hash of the build the launch picked; the server hashes its own
  executable. Mismatch → install it again once (the server's `arch` says which build) → if still
  mismatched, `Failed` ("the dv in Ubuntu doesn't match this app") and the banner. A launch
  that can't succeed until someone acts (no build for the architecture, no such distro) fails
  the connection at once, a respawn too.
- **Install** (`dv_backend::install`): one `wsl.exe … /bin/sh -c <install script> sh <hash>`
  with the build streamed on its stdin, the connection `Installing { done, total }` meanwhile:
  into a temporary file beside `~/.local/share/dv/bin/dv-<hash>`, `chmod 755`, its sha256
  checked (a copy that isn't the build is exit 3, nothing installed), renamed into place; then
  `~/.local/bin/dv` becomes a symbolic link to it, a temporary link renamed over whatever file
  was there (the first implementation's regular file too), so the dv on the distro's `PATH` is
  this app's. The link is a convenience — the launch and the CLI's run use the build itself —
  so one that can't be made (a directory at `~/.local/bin/dv`, a `~/.local/bin` that isn't a
  folder dv can write) is a warning on stderr (kept in the connection's diagnostics, printed
  by the CLI), never a failed install. A script that fails before it reads all of the build
  says why itself; the pipe it closed is no part of the error.
- **Prune** after the first successful handshake per process and distro, in the background,
  best effort: every `~/.local/share/dv/bin/dv-*` but this app's builds and the one
  `~/.local/bin/dv` links to (another app's install may have pointed it there since; a running
  server keeps its deleted file), and temporary copies an install left more than an hour ago.
  `~/.local/share/dv`'s other folders are never touched.
- **Sandbox** `DV_WSL_HOME` (a POSIX path): every launch, install, prune and check runs as `…
  --exec /usr/bin/env HOME=<it> /bin/sh -c …`, so tests and agents' runs install into a scratch
  folder under the distro's `/tmp`, never the user's own home. One check decides what it means,
  everywhere (`dv_backend::wsl::sandbox`: the launcher, the CLI, `dv setup wsl`, the automation
  guards): unset is the distro user's own home; set to a plain absolute POSIX path naming a
  folder other than `/` (no `..` part, no backslash, no line break, nothing around it) it is
  the scratch home, normalised (`.` parts and repeated slashes dropped); set to anything else —
  blank, whitespace, relative, `/` in any spelling (`//`, `/.`, `/./`), a path Git Bash turned
  into `C:/…`, a value that isn't Unicode — it is an error, never a fallback to the real home,
  and no sandbox an automation run may reach a real distro with.
- **Distros** (`wsl::distros`): `wsl.exe --list --quiet` and `wsl.exe --list --running --quiet`
  (UTF-16LE), never the localised `--verbose` table, reused for 2 s, everyone asking while a
  listing runs sharing it (`wsl::running_now` lists afresh); a `--running` that fails counts as
  none running. Listing boots no distro. Every
  `wsl.exe` dv starts is made without a console window (`CREATE_NO_WINDOW`, one builder,
  `wsl::wsl_exe`), the CLI's crossing alone sharing its caller's console (§10.4). `dv setup wsl
  <distro>` does the install ahead of time (§10.1).
- Death (§5.1): in-flight requests fail with `BackendError::Disconnected`, the connection is
  `Reconnecting { attempt }` and the UI shows "reconnecting…"; the client respawns after 1 s, 2 s
  and 5 s, each an automatic start (the connecting rule above: a distro the user stopped ends the
  connection `Stopped` at once, launching nothing), calls failing at once with `Disconnected`
  meanwhile, also while an attempt installs the build again (its progress is still the state).
  When a server answers, each `RemoteBackend` opens its worktree on it again with its next call
  (a worktree's handle belongs to the server generation that issued it, and a worktree gone
  meanwhile is that call's error), every live watch subscribes again and its sink gets one
  synthetic `WatchEvent { rescan: true, .. }` for what changed while nobody watched. A watch the
  new server refuses (its worktree gone), or watches differently than its handle says (the store
  alone now, its `worktree_error`, or the worktree again), ends instead: its sink is dropped, and
  its consumer starts a new watch, whose handle says what it covers; one interrupted by another
  death waits for the next server. When the third attempt fails, or a server answers as another
  protocol or build, the connection is `Failed`: calls fail with `Unavailable` and nothing
  starts until `retry()` (the banner's Retry, an explicit start), which starts one server and
  blocks until it answers or fails. A server that dies within 10 s of a respawn continues that
  count instead of starting a new one, so one that dies on every start ends `Failed` rather
  than looping; any other start (the first, an explicit one, the first after an idle close)
  begins a fresh count. A server that can't start the first time fails the connection at
  once.

### 5.3 Watching
Worktree watch honours git's ignore rules — every `.gitignore` (nearest wins, nothing re-included
inside an ignored directory), `info/exclude`, the global excludes file — through the `ignore` crate's
matcher, loaded by a walk that skips ignored directories and re-read as `.gitignore` files change;
tracked files that match a pattern still count, as they do for git. A removed path can't be asked
whether it was a directory, so the watch remembers the ignored directories it has seen and drops the
removal of `target/` under a `target/` pattern too. A symbolic link is one path and is never
followed, by the OS watch or the watch's own listings, because git stores a link as its target path
and never descends into one (nor does a `foo/` pattern match a link named `foo`). The git dir is
dropped except `dv/` and the git state below; a linked worktree's `<git-common-dir>/info/` lies
outside its tree and is watched for `info/exclude` edits on its own. **Git state** (`WatchSpec { worktree, store, git }`):
with `git`, a batch sets `WatchEvent::git_changed` when this worktree's `HEAD` or `index`, or the
common dir's `refs/**` (except `refs/dv/**`, dv's own snapshots), `packed-refs` or `reftable/**`
change. A `working` target's old side moves when the agent commits without touching a worktree
file, and a `staged` target lives in the index. Only final names count: git writes each through a
`*.lock` renamed into place, so lock churn alone changes nothing. The directories holding them are
watched on their own where the worktree's recursive watch doesn't cover them (a linked worktree's
git dir and common dir, or a watch without the worktree); in a reftable repo a `refs/dv/` update
can't be told apart and fires too. Events coalesce for 150ms and arrive as *paths*
(`WatchEvent { worktree_paths, store_paths, git_changed, rescan }`: relative, `/`-separated,
de-duplicated), so the UI refreshes only the affected files and the file list; `rescan` means the
OS may have dropped events (or a batch would list more than 10,000 paths). A directory that appears
is listed before its batch goes out, because inotify watches a new directory only after reporting
it and a directory moved in brings its files without an event each. `<git-common-dir>/dv/` is watched separately when it lies outside the worktree (a
linked worktree's common dir). Dropping the handle stops the watch before `drop` returns. The OS
watch registers ignored directories too (inotify needs a watch per directory), so on Linux a huge
`node_modules/` can exhaust the per-user limit; a watch that includes the store then keeps
watching the store alone and its handle says why the worktree and the git state aren't watched,
so the UI falls back to refreshing those itself instead of also losing review updates.

---

## 6. Diff pipeline

1. **File list** (`dv_core::diff::file_list`): one `git diff --raw --numstat -z -M --no-abbrev
   --no-ext-diff --no-textconv --diff-algorithm=myers --indent-heuristic --submodule=short
   --ignore-submodules=untracked <old> <new>` call (D4's pinned flags) gives every file's status,
   modes and object ids and its +/− up front (the first implementation only counted files already
   opened), from one look at the working tree. Rename detection only: `-M` also overrides a
   configured `diff.renames=copies`, so a copy lists as added, as on GitHub. A working tree new side
   adds the untracked files (`git ls-files --others --exclude-standard -z`): + is the file's line
   count, read where the file lives (`Backend::worktree_text_stats`, streamed), binary by the `diff`
   attribute (`git check-attr`; `-diff`/`binary` → binary, `diff` → text, a driver name → that
   driver's `diff.<driver>.binary` when set, as git decides) or else git's heuristic (a NUL in the
   first 8000 bytes); a file git converts before it counts or sniffs is counted by git
   (`git diff --no-index --numstat`), so it agrees with the same file staged: one whose `filter`
   driver has a `clean` or `process` command (a git-lfs asset is a three-line text pointer to git,
   whatever its bytes) or one with a `working-tree-encoding` (UTF-16, say); a filter driver without
   a command converts nothing, as in git; an untracked symbolic link is one line, its target path; a
   nested repository (`dir/`) lists as a submodule with one addition, as `--numstat` counts the
   gitlink it becomes once staged or snapshotted (§2.3). A worktree-side entry that `--raw` lists as
   modified with an unhashed new side but `--numstat` doesn't list is a stat-dirty file whose
   content is unchanged (the git policy's `diff.autoRefreshIndex=false`, §5.1) and is dropped. An
   untracked file the old side holds too (a round snapshot's, step 6; or one `git rm --cached`
   left on disk) is part of the working tree side like any untracked file, but git's diff sees the
   working tree only through the index: it lists such a file as deleted (or as the source of a
   rename to a similar file added since) while `ls-files` lists it as new. When the list finds
   any, it lists again through a temp copy of the index (`<git-common-dir>/dv/tmp/`) in which
   `git update-index --index-info` registers each at its old-side mode and blob, writing no
   object: git then compares them with the files on disk, and each lists once, as modified when it
   changed, marked `FileEntry::new_untracked` so `file_diff` builds the same temp index for it.
   The list is sorted by path.
2. **Per file, on demand** (`dv_core::diff::file_diff`): cheap checks first. An unmerged entry on an
   index side is `Unmerged` (the index holds no single version); a submodule change (every side a
   gitlink) is its gitlink ids (on a working tree side git's own `Subproject commit <oid>[-dirty]`
   line). Then both sides' sizes (one `git cat-file --batch-check` for the blobs, the backend for a
   working tree file git didn't hash or hashed only in memory, as rename detection does):
   binary files (known from the list) are `Binary` with their sizes, whatever their size; over the
   cap (2 MB or 20k lines per side — the list's +/− and the output's length catch most of it before
   any line is built) is `TooLarge`, shown with a "Load anyway" row that diffs again without the
   caps. Otherwise `git diff -U<N>` (D4) is parsed into aligned lines — hunk bodies read by their
   header counts, never by what a line looks like, because a reviewed file may itself be a patch —
   with each side's "no newline at end of file", and CRLF line endings stripped for display and
   reported. A listed file whose content didn't change (renamed, or only its mode changed) gets
   no hunk from git; its text is read from the old side's blob and given as context lines on
   both sides, so the diff still holds both sides' complete text for threads to be placed
   against (§8), while the stream shows "No content change". Under `core.autocrlf` or `text eol=crlf` git compares the normalised LF form, so an
   edited CRLF checkout is exactly its edited lines. A type change (file ↔ symlink, file ↔ gitlink)
   prints as a deletion plus an addition of one path and becomes one diff.
3. **Intraline:** word-level ranges for paired changed lines (kept from the first implementation:
   the i-th removed line of a change block pairs with the i-th added one, token diff with
   histogram, pairs over 10,000 token pairs or more than 70% rewritten left unmarked).
4. **Highlight:** tree-sitter on both full sides, off the UI thread, producing **token kinds**, not
   colours. Colours resolve at paint time from the theme, so theme changes repaint without recomputing
   and cached rows never go stale. Rows appear unhighlighted first and colour in when ready.
   `dv_ui::syntax` runs gpui-component's highlighter (every grammar of its `tree-sitter-languages`
   set except SQL, whose crate pins `cc` below the version gpui's build locks) once per loaded file
   side, in the language its path names (extension or a well-known name such as `Makefile`; none:
   plain). Capture names map to ten kinds — keyword, function, string, number, type, constant,
   property, punctuation, comment, plain — through one table (`kind_for_capture`), and the
   highlighter's style resolver encodes the kind so nothing colour-shaped is cached. Caps: a side
   over 2 MB isn't highlighted, a line over 10,000 bytes keeps no tokens.
5. **Rows:** built lazily for the current view mode only; unchanged runs beyond the context setting
   become gap rows that expand in 20-line steps from either edge or fully, from the already-loaded
   content (no refetch; `dv_core::diff::{segments, expand_gap}`). A run shorter than two lines stays
   shown, since its gap row would take the same space. A gap row also heads the run that follows
   it with that run's `@@ -a,b +c,d @@` (`dv_core::diff::hunk_range`, git's header for the same
   lines). In split view, within a run of changed lines the i-th removed line faces the i-th
   added one (`dv_core::diff::split_rows`, the pairing intraline marks use).
6. **Interdiff (rounds):** the same pipeline between round n's snapshot and the current new side
   (`DiffSpec::since`, §2.3). A working tree snapshot holds the untracked files of its round, so the
   step 1 rule for untracked files the old side holds is what keeps an interdiff honest: an
   untracked file unchanged since the round isn't listed, a changed one lists once as modified,
   and a near copy staged since lists as added, not as a rename of the untracked original.
7. **Stream:** the diff pane is one virtualised list over *all* files (file header rows pinned while
   scrolling), so `J`/`K` scroll to a file instead of swapping views. `dv_ui::diff_pane::rows`
   builds each file's rows as plain data — header, code rows (one line, or one split pair), gap
   rows, a notice instead of lines, the next-file footer, and the insertion points thread cards
   and the composer take — and the gpui `list` holds one item per row, plus a spacer that lets the
   last header reach the top (the viewport less the last file's rows, a card not measured yet
   counted at the least a card can be, so the spacer is never short). A file's rows are rebuilt
   when its diff lands, a gap expands, it collapses (viewed) or the view mode changes, and only
   they are spliced; the list scrolls by item, so a file loading above the viewport moves
   nothing, and rows rebuilt under the reading position keep it on the same line. A file whose
   diff loads again with other lines (the agent saved it) has its lines followed into the new
   diff by number (`dv_core::diff::remap_lines`: on a side whose text didn't change every line
   keeps its number): the cursor, the selection, the composer's lines, the reading position, the
   context the reviewer opened and the lines on screen stay with their lines. Every row but a
   thread card, the composer and an opened failure banner has a fixed height, and those count as
   last measured, so scrolling to a line, paging and the pinned header work from heights without
   waiting for a layout. The composer's row carries its field's focus handle into the list, so a
   field scrolled out of view keeps the keyboard; one left without a row (its file collapsed or
   gone) hands the keyboard back to the stream. The stream asks
   the session for the files in view plus two ahead and one behind; a code row shapes only the
   columns in view, so a minified line costs no more than a short one.

---

## 7. Review store

`dv_core::review`: `id`, `event`, `model` (the folded state), `fold`, `store` (`ReviewStore`) and
`ops` (the checked changes the UI and the CLI call).

- **Location:** `<git-common-dir>/dv/reviews/<id>.jsonl` (resolved with `git rev-parse
  --git-common-dir`, never by hand), shared by every worktree of the repo. The `created` event records
  the worktree root it was made in (`worktree`), which is what a `working` or `staged` target means.
- **Events (v1):** `created{target, author, worktree}`, `round_closed{n, new_oid, old_oid, verdict?,
  body?}` (`body`: the round's summary, Markdown),
  `viewed{round, path, on}`, `thread_added{id, anchor, body, author}`, `reply_added{thread, id, body,
  author, anchor?}`, `thread_state{thread, state, note?, author, anchor?}`, `body_edited{target_id,
  body}`, `deleted{target_id}`, `pr_linked{owner, repo, number}`, `submitted{github_review_id, url,
  event}`, `status{status, note?, author?, if_status?, if_round?}`. A reply's or state change's
  `anchor` is the thread's relocated anchor, recorded once it moved or its file was renamed (§8);
  a status's `note` is what the one who set it says, Markdown (the agent's `dv review done --note`,
  §3 workflow A step 4), `author` who that was, and `if_status` and `if_round` conditions: the
  move applies only to a review that has that status (the agent's automatic `in_progress`), or
  exactly that many closed rounds (the agent's `dv review done`), where the event lands, and is a
  no-op otherwise (§2.4). Each line is `{"v":1, "eid", "at" (ms), "kind": …, …}`: the format version, a
  unique event id and the time, beside the kind and its fields (snake_case). Optional fields are left
  out when empty; unknown fields are ignored, so an optional addition needs no version bump.
- **Writing:** one `store_append` per event (D5). A log whose last line lacks its newline ends in a
  write a crash cut off; the next append first writes a lone newline, so the new event isn't glued
  onto the torn line. Only `ReviewStore::create` writes `created`, as a log's first line.
- **Fold:** the file order is the total order. Unknown event kinds and lines with a higher `v` are
  skipped and counted (forward compatible); a line that fails to parse is reported with its line
  number and skipped (a torn final line loses nothing before it). The fold checks every event against
  the state it lands on with the rules of §2.4 and §2.5 — ids that exist and aren't deleted, allowed
  status and thread-state moves, rounds closing in order, viewed rounds that exist, nothing after
  `submitted`, an event id seen once — and skips one that doesn't apply with a warning. Writers don't
  lock, so two may race on stale state (an agent addresses a thread the reviewer just resolved); the
  fold is the final word. An event that changes nothing applies as a no-op. A log without a valid
  `created` event is unreadable. A relocated `anchor` on a reply or state change moves the thread
  there and keeps its first anchor (§8). The last `status` event that changed the review is
  `Review::status_change {status, note, author, at}`; repeating the status with another note while
  the review is open replaces the note (as addressing a thread again does), with the same note or
  none it is a no-op. The status each round closed from is derived, never written
  (`Round::closed_from`): it says when a hand-back would be of a round the agent may not have
  seen (§2.4).
- **Operations** (`review::ops`) load the review, run the fold's own check on the new event, add the
  checks that need to know who is asking (§2.4's status rules, author-only edits and deletes), and
  only then append, so an operation never writes an event the fold would skip on that state.
- **List:** every `<review id>.jsonl` in `reviews/`, newest first; a log that fails to load is listed
  as `{id, error}` — one bad file never hides the others — while one deleted between the listing and
  its load is simply gone. Listing also removes snapshots whose log is gone, which a load racing a
  delete writes back after the delete removed them.
- **Ids:** `r-<ulid>`, `t-<ulid>`, `c-<ulid>`, `e-<ulid>` in lowercase Crockford base32 (no `i l o u`,
  first digit `0`–`7`), validated by a strict parser before any path is built from one; `../x`,
  uppercase and a wrong kind letter are refused.
- **Snapshots** of the fold are a cache only (`<id>.state.json`): the folded state, its warnings and
  the log's complete-line count (the event count), length and FNV-1a hash. A snapshot answers a load
  only when all three match the log as it is now (checking them is far cheaper than parsing every
  line, and catches a log that grew or was edited in place); otherwise the log is folded and the
  snapshot rewritten atomically. Its format version changes whenever the folded state does, and a
  snapshot of another version is rebuilt, so one folded by a dv that ignored an event field is
  never taken for this build's fold. Operations fold without rewriting it: their own append makes a
  snapshot of the state they loaded stale at once. A snapshot that can't be read or written is
  ignored, logged at debug only: readers woken by one append (an agent's `dv review wait`, the
  app) rewrite it at once, and on Windows the losers' replace fails while another holds the
  file, which must not reach a successful command's stderr.
- **Deletion** of a review removes its log, its snapshot and its `refs/dv/reviews/<id>/*` (one
  `git update-ref --stdin` over what `for-each-ref` lists), log first, and finishes an interrupted
  delete when run again. A round finished while its review is deleted can make its snapshot ref
  after the delete listed the refs; its `round_closed` then finds no log, and `finish_round`
  removes that ref again (`git update-ref -d <ref> <its commit>`), so nothing is left pinning
  the snapshot.

## 8. Anchoring and relocation

A new thread's anchor comes from the selection and the side's full text
(`dv_core::review::anchor_selection`: the lines' text and up to 3 lines of context either side);
`Anchor::matches` says whether its text still stands at its recorded lines.

**Relocation** (`dv_core::review::relocate(anchor, lines) -> Relocation`, against the side's
current full text, `side_lines`) runs on every refresh, one pure function for the GUI and the CLI:
1. the anchored text still stands at its recorded lines → `Unchanged`;
2. exact matches of the anchored lines within ±200 lines of the recorded start: the one whose
   context lines also match best wins (the count of `context_before`/`context_after` lines that
   match where they belong), then the nearest, then the one above → `Moved { how: Nearby }`;
3. else an exact match anywhere in the file, if it is the only one → `Moved { how: Unique }`;
4. else (and before a slight anchor's bare matches count, below), when the anchor's whole
   context still stands together somewhere (its `context_before`
   lines, then its `context_after` lines, nothing or a few lines between; a context cut short by
   the file's edge must meet that edge; the nearest to the recorded start, then the one above),
   the anchored lines are gone from between it: with nothing between they were deleted →
   **outdated**; with up to the anchored count plus 10 lines between (`REWRITE_SLACK`) they were
   rewritten there, found at those lines when they score at least 0.80 as step 5 scores a window
   (the same number of lines compared line by line, any other number as one text each) →
   `Moved { how: Fuzzy { score } }`, else **outdated**. Never at a like line elsewhere: list-shaped
   code (match arms, rows of data, flag lists) has neighbours that score as well as the line a
   reviewer asked to remove, and a fuzzy window would land the thread on one of them;
5. else fuzzy, on whitespace-normalised lines (trimmed, inner runs of whitespace collapsed to one
   space): every window of the anchor's shape (its context before, as many lines as it anchors,
   its context after, clipped at the file's edges, where a missing line scores 0) scores the
   weighted mean of its lines' similarity to the anchor's, anchored lines weighing 2 and context
   lines 1; an anchor of more than 16 lines is scored on 16 of them, its first and last among
   them and the rest spread evenly (`FUZZY_MAX_ANCHORED`), so a window costs the same whatever
   the range's length. Line similarity is the Dice coefficient of the two lines' character
   bigrams (equal lines 1, two empty lines 1, a line shorter than 2 characters compared by
   equality). Of the windows scoring ≥ 0.80, the nearest (then the one above) within 0.05 of the
   best score wins (`FUZZY_MARGIN`: in repetitive text — rows of data, a flag list — an
   untouched window far away can score a little better than the anchored lines edited where they
   stand) → `Moved { how: Fuzzy { score } }`. A side over 20,000 lines skips this step.
6. else **outdated**: shown with its original code snippet, not inline (its card heads its file,
   placement below), and flagged in the comments list.

An exact match from step 2 or 3 wins at once when its context stands around it too (every
`context_before` and `context_after` line where it belongs). One standing among other lines (a
bare match) may be no more than the anchored text repeated somewhere (`return Ok(());`, `}`, a
second debug print), and which it is depends on the anchor:
- a **slight** anchor (`review::is_slight`: fewer than 32 non-whitespace characters in all its
  lines, `SLIGHT_CHARS`; a longer line says enough to be followed where it moves, alone or not)
  says too little to tell a move from a copy that was always there, so step 4 runs before its
  bare matches count: the line a reviewer asked to remove, deleted while a twin stands
  elsewhere, is outdated, and one rewritten where it stood is found there or outdated, never
  moved onto the twin (whose lines the next event would record). A match standing between the
  closed-up context is the anchored text itself, with lines inserted beside it, and still wins
  at exactly its own lines. At a file's edge (its first or last lines: no context on that side)
  the closed-up context runs to the edge, so code added above a file's last line (a route
  before `module.exports = router;`) or below its first leaves the thread on that line. The
  price: a slight line cut and pasted out of its context is outdated, and a slight last line
  deleted while code ending in a copy of it is appended (the last `}` deleted, a function
  appended) lands on the copy, which has the same shape as code inserted above the last line
  and is far rarer;
- a fuller anchor found elsewhere was moved there, so for it step 4 is only for text found
  nowhere: a block cut and pasted elsewhere leaves its context closed up behind it and is still
  found where it went (a block moved and edited at once, whose old place closed up, is outdated:
  dv can't tell it from a deletion).

Step 5 then runs as well, and a window scoring better than the bare match's own window wins over
it, so an edit to the anchored line itself is found where it was made. An anchor of blank lines only
would match any blank run by text alone, so in steps 2–3 its matches count only where at least one
context line matches too. Each line's bigrams are computed once per side and shared by every
thread placed on it (`review::FuzzyLines`); in a release build a 20,000-line side takes about
16 ms for a 3-line anchor and about 30 ms for a 200-line one that moved far or is gone (measured
2026-10-09).

**Placement** (`review::place_thread(thread, entries, text) -> Placement`) decides the file and side
a thread shows on now. Renames carry anchors: its file is the entry at the anchor's path, else the
entry renamed from it (`entry.old_path == anchor.path`), else the entry renamed from the path the
anchor's file had been renamed from (`entry.old_path == anchor.old_path`: renamed again since,
which git reports as one rename from the first name; an added file renamed later has no such
path, and its threads go outdated). The side's text comes from a callback (the GUI's loaded diff
— every file a thread shows in loads whatever the stream's viewport; where the diff shown holds
no text, a file too large to show, the review's own diff of the file without caps, read in the
background for the threads anchored there —, the CLI's `file_diff` without caps; a file renamed
or with its mode changed and nothing else carries its text as context lines, §6 step 2), which
can say it isn't loaded yet (`Pending`) or that the side has no lines (binary, a submodule,
unmerged: outdated). A thread
whose file left the list, or whose side the file no longer has (a new-side thread on a deleted
file, an old-side one on an added file), is `Outdated`; otherwise `relocate` decides `Unchanged`,
`Moved` or `Outdated`. The GUI and the CLI place threads only through it, against the review's
own file list (all changes), so they never disagree (the GUI's interdiff adds one rule after it:
a thread whose file the interdiff doesn't show, and an old-side thread, isn't drawn, §2.3, §11;
it is still placed, against the review's own diff of its file read in the background, so the
comments list, "Copy as prompt" and the sheet name where it stands in all changes, as the CLI
does). An outdated thread whose file is still listed heads that file in the stream with the lines
it was written on (DESIGN §4.1), so it can still be answered and resolved; one whose file left
the list (`Outdated` without a file) heads the stream itself, as a full card in its "Not in the
diff" section before the first file, whichever comparison is shown, so it too can be answered
and resolved there (and from its comments-list row's menu).

**Recording a move.** Relocation is display-only until the next event on the thread, which records
the new anchor: `reply_added` and `thread_state` carry an optional `anchor` (§7), the thread's
anchor re-taken at its relocated lines in the round in progress with fresh context
(`review::relocated_anchor`, through `anchor_selection`); callers pass one for `Moved`, and for a
thread standing at its lines in a file renamed since it was written (re-taken at the same lines
under the file's name now, so the thread keeps its file once the rename becomes part of the old
side — a commit, a rebase — rather than going outdated) (`ops::reply` / `ops::set_thread_state`
take `relocated: Option<Anchor>`). The fold checks it like a
`thread_added` anchor (an invalid one skips the event with a warning), replaces `Thread::anchor`
with it and keeps the anchor the thread was opened with as `Thread::original_anchor` (set on the
first move only); an event that changes nothing records no anchor either. dv versions without
relocation ignore the field. The CLI's `comment reply | address | resolve | reopen` place the
thread first and pass the anchor the same way the GUI's writes do. A thread shows as moved (the
GUI's "· moved", the CLI's `placement: "moved"`) when the lines it shows at now start elsewhere
than the first line it was written on (`review::shows_moved`), whether relocation finds it there
or an event recorded the move: code edited where it stands (a fuzzy match starting at its own
first line, however many lines it covers now: a line rewritten into a block) and a renamed file
move nothing, so recording a move, an edit in place or a rename changes nothing anyone sees.
Code edited where it stands is `placement: "edited"` in the CLI (`review::shows_edited`: found
there by a fuzzy match, or standing at recorded lines whose text isn't the text it was written
on), so an agent reads `current_code` rather than take the comment's code as still there; the
GUI marks nothing for it. `dv comment list --json` reports `anchor.current` (relocated) and
`anchor.original`, plus the anchored text, so agents never act on a stale line number.

---

## 9. GitHub

### 9.1 Opening a PR
`gh pr view <n> --json number,title,body,state,isDraft,baseRefName,headRefName,baseRefOid,headRefOid,
headRepository,author,url,reviewDecision,statusCheckRollup` → `git fetch <remote>
+refs/pull/<n>/head:refs/dv/pr/<n>/head <baseRef>:refs/dv/pr/<n>/base` → verify the fetched oids
match → target `pr`.

### 9.2 Pulling GitHub threads
GraphQL `reviewThreads` with cursor pagination (threads and comments), mapped to read-only threads
(author `github`), re-pulled on open and on `R` (refresh). Resolving a GitHub thread from dv is out of
scope for v1.

### 9.3 Submitting
- **Line validation** uses GitHub's own unit: hunks of `git diff -U3 <merge_base> <head> -- <path>`
  (context lines count as inside the diff). GitHub's web UI can comment on unchanged lines anywhere in
  a changed file, but its API still rejects those with 422 "line is not part of the diff" (checked
  2026-10). Comments outside the hunks offer **Post as file comment** (`subject_type: "file"`, the
  body prefixed with a quoted `path:line` and the anchored line) or **Keep local**; comments on files
  the PR doesn't touch can only be kept local.
- One `POST /repos/{o}/{r}/pulls/{n}/reviews` with `event`, `body`, `comments[]` (`path`, `side`,
  `line`, `start_side`/`start_line` for ranges). Duplicate-submit guard: the review's `submitted` event
  is written before the POST returns its id to the UI; a retry after a crash checks
  `GET …/reviews` for a review with our marker before posting again.
- A 422 from GitHub is shown per comment where it can be attributed, never as "maybe force-pushed".

### 9.4 Viewed state
GitHub's per-file "viewed" (GraphQL `markFileAsViewed`) is synced both ways for PR reviews
*(stretch for v1)*.

---

## 10. CLI

### 10.1 Command tree

```
dv                                 open the GUI on the current repo
dv open [<path> | <PR-url> | #<n>] [--review <id>]
                                   the ONLY GUI entry points; every other command is headless

dv status                          repo, open reviews, the current one, its round and thread counts
dv review list [--all] [--status …]
dv review new [--staged | --branch [<b>] [--base <base>] | --commit <rev> | --range <a..b> | --pr <n>]
dv review show [<id>]
dv review wait [<id>] [--timeout <s>]      block until the reviewer finishes a round (exit 3 on timeout)
dv review done [<id>] [--note <t> | --note-file <f|->] [--round <n>]
                                           agent: round addressed, ready for re-review
dv review submit [<id>] --verdict approve|request-changes [--body <t> | --body-file <f|->]
                                           reviewer (`comment`: PR reviews, phase 8)
dv review close | delete <id>              reviewer

dv comment list [--status open|addressed|resolved|outdated]… [--file <p>] [--oneline]
dv comment show <id>                       the thread with the diff around its code now
dv comment add --file <p> --lines N[:M] [--side old|new] (--body <t> | --body-file <f|->)
dv comment reply <id> (--body <t> | --body-file <f|->)
dv comment address <id> [--note <t> | --note-file <f|->]
                                           agent
dv comment resolve | reopen <id>           reviewer

dv setup [path | agents | wsl <distro> | all] [--yes]
dv doctor [--json]
dv backend --stdio                         (every build; started by the app, §5.1)
```

Global flags: `--repo <path>` (also `\\wsl.localhost\<distro>\<path>`), `--wsl <distro>:<path>`,
`--review <id>`, `--json`, `--author <name>`; they go anywhere on the line.

- **One clap tree.** `dv_cli::args` holds the global flags and the headless commands; the `dv`
  binary keeps the GUI's options (`--data-dir`, `--automation…`, `--window-size`, the hidden
  `--no-detach`) and `open`, and flattens `dv_cli::Command` into its own subcommands, so `dv
  --help` lists every command. A headless command goes to `dv_cli::run(&global, command, &mut
  Context)` before any gpui code runs. `Context` carries the command line (without the app's
  options: what a command run inside a WSL distro passes on, §10.4), the working directory, the
  environment (`dv_cli::Env`: `$DV_REVIEW`, `$DV_AUTHOR` and `CLAUDECODE` are read through it,
  never by clap), stdin, stdout, stderr, whether they are the process's own (a child then
  shares them) and whether they are terminals, and the environment overlay every git the
  command spawns gets (`LocalBackendOptions::env`), so tests run commands in-process and
  hermetically (`dv_cli::main_with` parses too); a few binary tests in `crates/dv/tests/` check
  the real process.
- **The repository** is `--wsl`, else `--repo` (relative to the working directory, the path
  that makes read as a location again, so a relative path from a working directory inside a
  distro, `\\wsl.localhost\…`, is a WSL location too), else the working directory's. Not
  inside a work tree: exit 1. On Windows a command for a WSL location (`\\wsl.localhost\…`,
  `\\wsl$\…`, `--wsl`) runs inside its distro (§10.4); elsewhere, a location of the distro dv
  runs in (Linux, `$WSL_DISTRO_NAME`) is its local path — a `--file` spelled through that
  distro's share (`\\wsl.localhost\Ubuntu\…`) too — and a Windows drive's path given there
  (`--repo D:\code\api`, `d:/code/api`, `dv open`'s path in a Linux build with the app) is that
  drive where WSL's default automount root mounts it (`/mnt/d/code/api`,
  `dv_core::location::mount_path`, the inverse of the headless `dv open`'s suggestion below)
  rather than a folder under the working directory; any other WSL location is exit 4
  (`backend_unavailable`), without starting anything, saying to run the command from Windows
  (`dv.exe --wsl <distro>:<path> …`, the location quoted to paste there by the rule below),
  the one dv that reaches every distro (inside a distro `wsl.exe -d` would need a dv installed
  in the other one).
- **Identity (§2.5):** an agent named by `--author <name>`, else `$DV_AUTHOR`, else
  `claude-code` when `CLAUDECODE` is set (Claude Code exports it to the commands it runs);
  otherwise the reviewer, by the repo's `user.name` (`review::reviewer_identity`, as the GUI).
  What each may do is `review::ops`'s rule. `dv status` says who a command acts as.
- **Placement:** `comment list` and `comment show` place every thread with
  `review::place_thread` (§8) against the review's target resolved now (all changes, never an
  interdiff) in its home worktree (§2.2: a `working` or `staged` review's own worktree, a
  `branch` review's the one with the branch checked out), whichever worktree the command runs in
  (a home worktree that is gone is exit 1, `target`), reading each anchored side's full text through `file_diff`
  without caps, so the CLI says what the GUI shows (a target that no longer resolves, such as a
  deleted branch, is exit 1, `target`, with git's words; `review submit` too). A thread found
  there carries the excerpt of that same diff around its lines (`review::excerpt`, §10.3), with
  no further git call. `--status outdated` keeps the threads whose code can't
  be found; `--file` matches a thread's file in the current diff, where its code is now, or any
  path it was written on. A `--file` (here and in `comment add`) is a path from the worktree
  root; an absolute one inside the worktree the review was made in, or the one the command runs
  in, loses that root (the longer, when both hold it), so the reviewed file's own path works from
  any worktree.
- **Writes** go through `review::ops` (or the store's delete), so the CLI writes what the GUI
  would and nothing else (§2.4: only `.git/dv/**` and `refs/dv/**`). A finished review (approved,
  closed) takes no comment writes (`add`, `reply`, `address`, `resolve`, `reopen`: exit 1
  `finished`, §2.4), as the GUI shows it read-only, and moves to no other status (`review done
  <id>` racing the reviewer's approval, `review close` after it, `review submit`: exit 1
  `finished` too, so an agent always hears the one stop signal). A finished review is no longer
  open, so no longer in context (§10.2): a command that names none finds no review (exit 2
  `no_review`), which is why the skill passes the review's id to `done` and `wait`:
  - `review new` opens the target's review as whoever the command acts as, or resumes its open
    one (§2.4), once the target resolves now (a mistyped revision is exit 1, not a review of
    nothing); `--branch` alone is the branch checked out (a detached `HEAD` is exit 2), compared
    with `--base` or the default branch, the switcher's own target.
  - `review done [--note | --note-file] [--round <n>]` is an agent's (exit 2 without an agent
    identity, saying to pass `--author`): status `ready` with the note (§7), through
    `ops::hand_back`. `--round <n>` names the round the agent worked on, which must be the last
    closed one; an older one, or none when the reviewer finished the last round while the agent
    was working (§2.4), is exit 1 `unseen_round`, nothing written, with what to do: read round
    n's summary (the `body` of the last of `rounds` in `dv review show <id> --json`) and its
    open threads (`dv comment list --status open`), then `dv review done <id> --round
    <n>`; one that hasn't closed is exit 2. The event is conditional on the closed rounds
    (`if_round`), and a round that closed between the check and the append makes it a no-op,
    reported as `finished` when that round finished the review, else `unseen_round`.
  - `review submit` is the reviewer's `ops::finish_round` with the verdict and the summary
    (`--body`, `--body-file`), snapshotting the review's home worktree (§2.2) whichever worktree
    it runs in; `--verdict comment` on a local review is exit 2 ("a local round approves or requests
    changes"), a PR review exit 2 (`unsupported`, phase 8). `review close <id>` sets `closed`;
    `review delete <id>` is the store's delete (log, snapshot cache, round refs), reviewer only
    (an agent is exit 1, `not_allowed`), and deleting again finishes an interrupted delete, then
    says the review isn't there (exit 1).
  - `comment add` anchors on a file of the review's current diff (the full target, never an
    interdiff), by its path or its rename's old path: a file the review doesn't change is exit 1
    `not_found`; a side the file lacks (`old` of an added file, `new` of a deleted one), lines
    past the side's end, or a file without lines (binary, a submodule, conflicted) exit 1
    `rejected`. The anchor is `anchor_selection` over that side's full text in the round in
    progress.
  - `comment reply | address | resolve | reopen` place the thread first and record its
    relocated anchor when it moved (§8); a target that no longer resolves is exit 1 before
    anything is written, as for `comment list`.
  - **Text** comes from `--body <t>`, or `--body-file <path>` (relative to the working directory)
    / `-` (stdin): a leading byte order mark, `\r\n` and trailing whitespace go, anything else is
    kept; empty text is exit 2, a file that can't be read or isn't UTF-8 exit 1 (`io`). A note
    (`comment address`, `review done`) comes from `--note <t>` or `--note-file <path> | -`, read
    and cleaned the same way, and one left empty is no note. An inline `--body` or `--note` may
    start with `-` (a Markdown list, "-1, I disagree"): clap takes the next word as the text
    whatever it starts with, except a lone flag (`--json`, `-v`, `--author=x`: one word, a
    letter after the dashes), which means the text was left out, and a lone `-` or `--` (stdin
    meant through the inline option, or the end of the options), which the `-file` forms are
    for: both exit 2 (`usage`). A `--` taken as such a value doesn't end the options, so a
    `--json` after it still asks for the JSON error document.
  - **An agent's first write** on a `changes_requested` review moves it to `in_progress` (§2.4).
  - Each prints what it changed (`--json`: the review or thread as it is now, `changed: false`
    when a status or thread state was already so and nothing was written).
- **`review wait [<id>] [--timeout <s>]`** waits on the review's state, not for a change (the
  first implementation reported changes; state can't lose a wakeup): it returns at once, exit 0,
  when the status is one the agent acts on (`changes_requested`, `approved`, `closed`;
  `submitted` for a PR review); otherwise it registers a store watch (`WatchSpec::STORE`) first,
  then loads the review again, then blocks until a batch touches `reviews/<id>.jsonl` (or is a
  rescan) and the reloaded status is one of those, also re-checking 5 s after each load however
  many other batches (other reviews, temp indexes) arrive meanwhile (every 1 s when the store
  can't be watched). A review deleted meanwhile is exit 1 (`not_found`); `--timeout`
  (seconds, fractions allowed) running out is exit 3. It prints why it returned: `{"schema": 1,
  "outcome": <the status>, "review": …, "round": <the last closed round> | null, "threads":
  {open, addressed, resolved}}`, or `{"schema": 1, "outcome": "timeout"}`; the text is the
  review's line, the last closed round (`round 1 closed: request_changes`) with its summary in
  full, indented (§10.3), and what to do next ("1 open thread: run `dv comment list --status
  open`, …, then `dv review done <id> --round <n>`", naming the review and the round; with no
  open thread, the round's summary or the reviewer's replies, since a reviewer may request
  changes in the summary alone; on a timeout, "run `dv review wait <id>` again", naming the
  review). A
  finished review is no
  longer in context (§10.2), so a wait that may start after the verdict names the review (the
  skill passes the id `dv status` gave).
- **`setup agents [--yes]`** installs the skill (§10.5).
- **`setup wsl <distro> [--yes]`** (Windows; elsewhere exit 2, `unsupported`) installs this
  build of dv in the distro, or checks that it is installed and intact (its sha256), and points
  the distro's `~/.local/bin/dv` at it: §5.2's install, ahead of the app's first open there. It
  finds the Linux build beside `dv.exe` (`DV_LINUX_DIR`) and the distro without booting it
  (`wsl.exe --list`), then boots it to look. It asks as `setup agents` does (a terminal is shown
  the plan and asked; a pipe or `--json` without `--yes` is exit 2; "no" is exit 1,
  `cancelled`; nothing to change needs no answer). `--json`: `{distro, arch, build, installed,
  linked, path}`, `installed` saying whether this run installed it and `linked` whether
  `~/.local/bin/dv` runs it now: a link the install couldn't make (§5.2) is a warning on stderr
  saying how to fix it, and the command still succeeds. WSL, the distro or the Linux build
  unavailable (and a `DV_WSL_HOME` that isn't a POSIX path) is exit 4; an install that fails
  exit 1 (`io`).
- **`backend --stdio`** serves the protocol (§5.1) on stdin and stdout until stdin ends, then exits
  0 at once; the app starts it (inside a distro, and as the tests' loopback). `dv backend` without
  `--stdio` is exit 2 (`usage`).
- **Recognised, not yet run:** `review new --pr` and `dv open` of a PR (a URL or `#n`: "GitHub
  PRs arrive in a later version", before any window opens), `review submit` of a PR review,
  `setup` other than `agents` and `wsl`, and `doctor`, exit 2 (`unsupported`) with what to do
  instead.
- **`dv` and `dv open`** start the GUI detached (§13.1) and return 0 at once; `--automation`
  and `--no-detach` run it in the same process. It opens on the repository named (the path,
  `--repo` or `--wsl`; `dv open` alone: the working directory's): the repository becomes a known
  one and the switcher opens on its uncommitted changes, as the automation `open` command does
  (`Shell::open_repo`); or, with `--review <id>` (the flag only), straight on that review through
  `Shell::open`. Bare `dv` opens the working directory's repository when it is one and nothing
  otherwise (an automation run opens nothing). Before detaching, a path that isn't a repository
  and a review that isn't there are errors in the terminal (exit 1); in the app they are the main
  area's banner (DESIGN §4.4). The headless commands' `--json` and `--author` mean nothing to the
  app, which prints no document and always acts as the reviewer: on a `dv` or `dv open` line
  they are exit 2 (`usage`, a JSON error document with `--json`), so an agent that asked for JSON
  gets JSON rather than a window. Starting detached hands the app none of this process's
  standard handles (§13.1), so a caller capturing `dv open`'s output gets its end at once. A
  build without the app (§13.1's headless shape, the Linux `dv` inside WSL) has none of the app's
  options, and its `dv` and `dv open` exit 2 (`unsupported`): inside a distro
  (`$WSL_DISTRO_NAME`) saying to open the repository from Windows, `dv open <path>` with the
  path as Windows spells it: `--wsl`'s, a path through a distro's share as it is (any
  distro's), a Windows path as it is, else the POSIX path named (or the working directory's)
  made absolute, as `\\wsl.localhost\<distro>\<path>` — but a path on a Windows drive under
  WSL's default automount root (`/mnt/d/code/api`, in `--wsl` too) as that drive's path
  (`D:\code\api`), which the app opens locally rather than through the distro over 9P; quoted
  only when it must be, in double quotes, which PowerShell and cmd read alike, or in
  PowerShell's literal single quotes for a path with `$`, a backtick or a double quote, which
  PowerShell expands or ends inside double quotes (`dv_cli::output::windows_arg`: the one rule
  for every command dv suggests to paste on Windows, the "another distro" error's too).
  Elsewhere it says that this build has no window.

### 10.2 Which review a command means
1. `--review <id>` (or the command's own `<id>` argument; naming two different reviews is a usage
error); 2. else `$DV_REVIEW`; 3. else the single open review whose target matches the current
context, the one open review whose home (§2.2) is the cwd's worktree
(`dv_core::review::ops::reviews_in_context` — a `working` or `staged` review made in this worktree,
or a `branch` review whose branch, however it is spelled, is checked out here: the rule a review's
target resolves by, so the review in context here is the one read here); 4. else exit 2 with
the candidates listed: every open review of the repository (`id`, label, status, round and the
`worktree` it was made in; a `working` or `staged` review labelled by its own worktree's `HEAD`,
"working tree vs feature", whichever worktree the command runs in), in the message and as
`error.candidates`, with the code `ambiguous_review` (several matched) or
`no_review` (none did). Never a silent "newest draft" guess. A named review that doesn't exist is
exit 1 (`not_found`); a finished one may still be named.

Commands that take a thread id (`comment show | reply | address | resolve | reopen`) find the
thread's review themselves (thread ids are unique across a repository's reviews), so an agent
never needs `--review` for them; `--review` narrows the search to that review, `$DV_REVIEW`
doesn't. A deleted thread is not found.

### 10.3 Output and exit codes
- `--json` prints one JSON document on stdout, compact on one line, `"schema": 1` first: the
  documented DTOs of `dv_cli::dto` (its module docs list every document and field), never the
  internal event or state structs. snake_case throughout; timestamps in ms since the epoch;
  every key present (`null` when empty). A review: `id`, `target` (`kind`, the spec's fields,
  `label`; a `working` or `staged` target is labelled with the `HEAD` of the worktree the review
  was made in, `dv_core::label::Labels`, as the GUI's switcher labels it), `status`, `round`, `rounds` (`{n, verdict, body, closed_at}`), `threads` (`{open,
  addressed, resolved}`), `worktree`, `created_at`, `updated_at`, `status_note` (the note the last
  status change carried, `{status, note, author, at}`, or `null`). A thread: `id`, `review_id`,
  `state`, `placement` (`unchanged`; `moved` — relocated now, or standing where an earlier
  write recorded its move, `review::shows_moved`; `edited` — standing where it was written, its
  text changed there, `review::shows_edited`; or `outdated`), `outdated`, `path` (its file
  in the current diff), `side`, `anchor: {original: {round, path, side, start_line, end_line},
  current: {path, side, start_line, end_line} | null}`, `code` (the lines it was written on, as
  they were), `code_context` (`{before, after}`: up to `ANCHOR_CONTEXT` lines that stood around
  `code` then, from its first anchor; non-null exactly when `anchor.current` is `null`),
  `current_code` (the lines at `anchor.current` now), `excerpt` (the review's diff around
  `anchor.current` now, the rows the text prints, below, joined by `\n`; non-null exactly when
  `anchor.current` is), `body`, `author: {kind, name}`, `created_at`, `note` (the last state
  change's), `replies: [{id, author, body, created_at}]`. The writes print: `review new` `{created, review}`; `review done` and `close`
  `{changed, review}`; `review submit` `{review, round}`; `review delete` `{deleted}`; `comment
  add` `{thread}`; `comment reply` `{reply, thread}`; `comment address | resolve | reopen`
  `{changed, thread}`; `setup agents` `{agents: [{agent, path, action: installed | updated |
  unchanged | skipped, reason}]}`; `setup wsl` `{distro, arch, build, installed, linked,
  path}`;
  `review wait` as in §10.1.
- Within a schema a document only grows: readers ignore keys they don't know; a key removed,
  renamed or retyped, or a value whose meaning changes, is a new schema number.
- Exit codes: `0` ok, `1` operation error, `2` usage / ambiguous review, `3` wait timeout, `4` backend
  unavailable (the repository's WSL distro can't be reached: WSL, the distro or this app's Linux
  build unavailable, §10.4), `5` GitHub error (with `gh`'s message). A command run inside a
  distro (§10.4) exits with the code of the dv there, whatever it is.
- Errors: stderr always (`dv: <message>`, then the candidates and the failed git command); with
  `--json` also `{"schema": 1, "error": {"code", "message", "detail"?, "candidates"?}}` on stdout.
  Codes (`dv_cli::error::code`): `usage`, `unsupported`, `ambiguous_review`, `no_review`,
  `not_found`, `not_a_repository`, `not_allowed`, `rejected`, `finished`, `unseen_round`,
  `cancelled`, `backend_unavailable` (exit 4), `target` (also a review whose home worktree is gone),
  `corrupt`, `git`, `io`, `internal`. clap's own parse errors
  are `usage` documents too whenever `--json` (or a `--json=…` spelling, which clap refuses for a
  flag) is anywhere on the line before a bare `--`, their message naming
  what is wrong (the missing arguments after "the following required arguments were not
  provided:"; a command group named alone, `dv --json review`, for which clap prints the group's
  help on stderr, "'dv review' requires a subcommand but one was not provided"); `--help` and
  `--version` print on stdout and exit 0.
- Human output: short stable lines, fields two spaces apart, no colour. A thread's place reads
  `path:12`, `path:14-16 (moved from 12-14)` or `path (outdated, was 12)`, and a thread on the old
  side says so first (`path:12 (old)`, `path:14 (old, moved from 12)`), as the GUI names it
  (DESIGN §4.1). The candidates of an `ambiguous_review` / `no_review` error follow the message,
  one per line: `id  label  status  round n  worktree`. No line ends in whitespace.
- **A thread's block** (`comment list`, one per thread after the review's line, each after a
  blank line; `comment show`: the review's line, a blank line, the block; `comment list
  --oneline`: one `id  state  place  <the comment's first line>` per thread instead):
  - The header `id  state  place`, the place as above.
  - The excerpt (`dv_core::review::excerpt`), one row per line: `>` on a commented line or a
    space, the line's number on the review's new side now (right-aligned to the widest; blank
    on a removed line), a space, `+` / `-` / a space, ` |`, then a space and the line, its
    whitespace at the end left out (an empty line, or one of only whitespace, ends at `|`; the
    JSON `excerpt` is these same rows). Every number printed beside code is a line as it is now; old numbers
    appear only as words in the header. The window is `ANCHOR_CONTEXT` (3) lines of the
    thread's side before and after its lines, plus every row of the other side between them;
    an old-side thread's `>` rows are its old lines (a removed one unnumbered). A run of more
    than 8 other-side rows keeps its first and last 3 with `⋮ N more removed lines` (`added`
    for an old-side thread) between. A commented span of more than 24 lines of the thread's
    side keeps its first and last 10 of them, with the other side's rows among those kept, and
    hides the rows from just after the 10th to just before the last 10th: `⋮ N lines` between
    (N of the thread's side), or `⋮ N lines, M removed` (`added` for an old-side thread) when
    the hidden rows hold M of the other side too; so a 20-line range with removed rows among
    them is shown whole. A line is cut at 240 characters, ending in `…`. A scope
    row leads: the nearest line of the thread's side above the window that starts with an
    ASCII letter, `_` or `$` (git's default hunk-header rule, language-blind), followed by a
    lone `⋮` when lines lie between; it is left out when such a line already stands in the
    window above the commented lines.
  - When the code changed since the comment (`current_code` isn't `code`: edited, or found by a
    fuzzy match), `as commented in round N:` follows, with the lines as written as `> | line`,
    unnumbered; over 24 of them, the first and last 10 with `  ⋮ N lines` between, the span
    rule above.
  - An outdated thread has no excerpt: only `as commented in round N (not found in the file
    now):`, then `code_context.before` as `  | line`, the code as `> | line` (bounded by the
    same span rule), and `code_context.after`, unnumbered: lines to search for. These rows, too,
    end without whitespace.
  - Who wrote what: `<kind> <name>, round <n>:` above the comment, `<kind> <name>:` above each
    reply, `<state> note:` above the last state change's note, all at column 0; every body is
    indented 4 spaces, each line's whitespace at its end left out and its blank lines left
    empty, so nothing anyone wrote reaches column 0,
    where it could pass for a header or a code row.
- A closed round, where `review wait` and `review show` print it: `round N closed: <verdict>`,
  then its summary in full, indented 4 the same way (`review show` prints earlier rounds on one
  line each, `round N closed: <verdict>: <the summary's first line>`).

### 10.4 WSL routing
Inside a distro, `dv` is the native Linux binary and runs every command itself; it never starts
`wsl.exe`. On Windows, a command that reads a repository (`status`, `review …`, `comment …`; not
`setup`, `doctor`, `backend`, nor `review new --pr`, refused first) whose repository is a WSL
location (§10.1: `--wsl`, `--repo \\wsl.localhost\…` or `\\wsl$\…`, or a working directory in
a distro) runs inside the distro instead (`dv_cli::reexec`): one crossing for the command, no
per-git-command spawns (D6), and the app's `Connections` play no part.
- **The line:** `wsl.exe -d <distro> --cd / --exec [/usr/bin/env HOME=<DV_WSL_HOME>] /bin/sh -c
  <run script> sh <x86_64 hash|-> <aarch64 hash|-> <folder> <args…>` (`dv_backend::wsl::RUN`,
  §5.2's launch with its choice of build): it runs `~/.local/share/dv/bin/dv-<hash>`, this
  build exactly, never whatever `~/.local/bin/dv` is, in `<folder>`: the working directory's
  path when that is in the distro, else the repository's (`/` when the folder isn't there:
  `wsl.exe`'s own `--cd` prints an error and runs in `/` anyway, so the script changes folder
  itself). The exit code is the command's, passed through. When stdout and stderr are both
  terminals the child shares the caller's console (no console flags), so the distro's dv sees
  a terminal. Otherwise (a pipe, a file, an agent's shell) dv pipes the child's stdout and
  stderr and copies them into its own: `wsl.exe` relays the two streams with a write position
  each, so a caller sending both to one file (`> out 2>&1`, Claude Code's shell on Windows)
  would get them written over each other. stderr is copied as it arrives, stdout whole once the
  child has exited (no command streams its stdout; `review wait` prints once, at its end). It
  joins dv's kill-on-close job
  (§5.1), so a `dv` that is killed (a tool's timeout, a `taskkill` without `/T`) takes its
  `wsl.exe` with it, and with that the distro's `dv review wait`; Ctrl-C reaches it through the
  console as before. The distro is named as given (`wsl.exe` takes names without case): no
  `wsl.exe --list` runs first.
- **The arguments** are the command line as parsed (without the app's own options, which a
  build without the app lacks), the repository first as `--repo <its path in the distro>` and the
  `--repo` and `--wsl` given dropped, a `--file` as one word, `--file=<path>` (so a path that
  starts with `-` stays the option's value there), a path spelled through the distro's share
  (`\\wsl.localhost\Ubuntu\…`) as its path there. A `--body-file` or `--note-file` is read on
  Windows (stdin for `-`, else the file relative to the working directory, so a Windows path
  works; a file must be UTF-8, checked here so the error names it) and handed to the child's
  stdin as `-`, which also lets the second run after an install lose nothing; otherwise the
  child's stdin is empty.
- **Who it acts as** crosses too: `CLAUDECODE`, `DV_AUTHOR` and `DV_REVIEW`, those set, join
  `WSLENV` as `NAME/u` after whatever else it lists, in its order. An entry it already has for
  one of them, in any case or with any flags, gives way to that `NAME/u` (`/w` wouldn't pass the
  value in, `/p` would translate it as a path, `dv_author` would export another name), so each
  crosses verbatim under its own name.
- **Missing pieces:** the run script exits 120 when this build isn't installed there; dv then
  installs it (§5.2's install, the distro's architecture from the install check; "dv:
  installing dv in Ubuntu…" on stderr) and runs the command once more. No `wsl.exe`, no such
  distro, no Linux build beside `dv.exe` (or in `DV_LINUX_DIR`), none for the distro's
  architecture, a `DV_WSL_HOME` that isn't a POSIX path, an install that fails, and `wsl.exe`
  failing to start the distro (an exit code no Linux program has) are exit 4
  (`backend_unavailable`, a JSON error under `--json`) saying what is missing. The Linux build
  and `DV_WSL_HOME` are checked before any `wsl.exe` starts; only once `wsl.exe` couldn't start
  the distro does `wsl.exe --list` (which boots nothing) word why: no such distro, or WSL not
  installed. What `wsl.exe` itself printed then (UTF-16LE, on its stdout) is the error's
  detail ("wsl.exe said: …"), never dv's stdout, which under `--json` is the error document
  alone; on a console it shows there as `wsl.exe` printed it. An install whose
  `~/.local/bin/dv` link couldn't be made is a warning on stderr, and the command runs.
- `DV_WSL_HOME` is the sandbox seam of §5.2 here too, and the automation host's `cli` children
  get a `DV_LINUX_DIR` without a Linux build unless the app is sandboxed itself, so no test or
  scenario runs a command in a real distro home (§14.2).

### 10.5 The agent skill
The `dv-review` skill is `crates/dv_cli/skill/SKILL.md`, embedded in the binary
(`dv_cli::skill::SKILL_MD`), frontmatter `name: dv-review` and a description that triggers on
"address review comments", "dv review" and "the reviewer left comments". It teaches the loop
`dv status --json` (keep `current.id`; no current review and none clearly the one asked about:
tell the user, never guess or start one) → read the round's summary (`current.rounds`' last
`body`, `round.body` in the wait's output: a reviewer may ask for more there than in the
threads) → `dv comment list --status open`, read as text (each thread's block, §10.3: the
diff around its code now, every number a line as it is now, what each row's marks mean), and
`--status addressed` for the reviewer's replies on threads already addressed → for each
thread, `dv comment show <id>` again before the fix only when the agent has edited that file
since it listed it (its numbers go stale then), acting on the numbered rows (`anchor.current`
/ `current_code` / `excerpt` in `--json`) and never on a header's old numbers → fix → `dv comment reply <id> --body-file -` (a quoted heredoc
in a POSIX shell, a PowerShell here-string piped in, both shown in the loop) → `dv comment address
<id> --note '…'` → once every open thread is answered and the summary acted on, `dv review done
<id> --round <n> --note-file -` (a Markdown list piped in; the id, because a review approved
meanwhile is no longer in context, and only named does `done` hear it is `finished`; `<n>` the
round the agent worked on, `round.n` from the wait or the last of `current.rounds`; on
`unseen_round`, read the round it names and run `done` again with its number) → `dv review wait <id> --timeout 600 --json` → again on
`changes_requested` (also with `threads.open` 0), stop on `approved` (committing only if the
user's instructions say so) or `closed`. A thread whose `author.kind` is `agent` is a question an
agent asked the reviewer, not a request: left open while it waits for their reply, acted on once
they replied (`review wait`'s text sets such questions apart from the threads to address; its
`threads.open` counts them). It says what `target.kind` means for where the agent's fixes show: a `working` review
is the files on disk (work in its `worktree`), a `branch` one too in whichever worktree has the
branch checked out (its tip when none has), a `staged` one what is staged (fixes show only once staged: ask the user before `git
add`), and a `commit` or `range` one fixed history (tell the user rather than hand back). Its
rules: threads read as dv prints them, `--json` when parsing (`status`, `review show`,
`review wait`, whose ids and round numbers the agent keeps), `--author <agent>` on every write, `<agent>` being the agent's own
name, which every example writes as that placeholder (Claude Code is recognised by `CLAUDECODE`
anyway), never `resolve` or `reopen`, no `dv review new` unless the
user asks, text through `--body-file -` / `--note-file -` and a short note inline only in single
quotes (double quotes run backticks in a POSIX shell, and a backtick is PowerShell's escape), a
disagreement is a reply with reasons and still an address with a note saying so;
and what each exit code means (1 `finished`: stop; 1 `unseen_round`: read the round, then `done
--round`; 2 with candidates: pick with `--review`, with none: tell the user; 4: dv couldn't
reach the WSL distro the repository is in, so tell the user what the message says is missing).
Every
command line in it works as written: a test parses each fenced command and runs the loop
against a fixture repo (`crates/dv_cli/src/tests/skill.rs`).

`dv setup agents [--yes]` installs it, idempotently, repairing a copy that drifted, for each agent
set up in the home directory (`%USERPROFILE%` / `$HOME`): Claude Code when `~/.claude` exists,
at `~/.claude/skills/dv-review/SKILL.md`; Codex when its home (`$CODEX_HOME`, else `~/.codex`)
exists, at `~/.agents/skills/dv-review/SKILL.md`, the user-level skills folder Codex's
documentation names (checked 2026-10-09; `$CODEX_HOME` holds Codex's own state). Each file is
written beside itself and renamed into place. Without `--yes` a terminal is shown the plan and
asked; anything else (a pipe, `--json`) is shown the plan and refused with exit 2 ("pass
`--yes`"); a "no" is exit 1 (`cancelled`); nothing to change needs no answer. `--json` reports
`{agent, path, action: installed | updated | unchanged | skipped, reason}` per agent. A hidden
`--home <dir>` stands in for the home directory, so tests never write the real one. Cursor and
the skill inside WSL distros come with phase 6 (`dv setup wsl` installs dv itself there).

---

## 11. UI architecture (`dv_ui`)

The sibling app's shape: per-view directories, a services layer of entities installed as a gpui `Global`,
typed events, and `is_settled()` on every service for automation.

**Services** (`dv_ui::services`): `Locations` (one `Arc<dyn Backend>` per repo, opened off the UI
thread and cached by the location asked for and its worktree root; a route can be registered for
a path, an environment overlay and optionally a loopback connection, which the automation host
does for its seeded repos; it holds the app's `Connections` (§5.1) and opens a WSL location
through its distro's, a `RemoteBackend`, following each connection's state so the status bar
repaints as it changes, and telling its subscribers when one goes down (reconnecting, stopped,
failed) or comes back (`LocationsEvent::ConnectionChanged`). An explicit open (`open`) is the
connection's explicit start, booting a stopped distro and starting a failed or stopped
connection once more, and waiting out a reconnect; the listing's `reach` only ever starts one
automatically, and reports a distro that isn't running, a connection reconnecting and one that
failed instead (§5.2's connecting rule). A cached local backend is reused while its worktree is
still on disk; a cached remote one is kept, its connection started as the open allows, also
while that connection is down, a repo gone inside the distro surfacing as the next call's
error. Which connection a location goes through (`key_for`, asked on the UI thread) is known
without the file system: a distro's, else what the last open of it found, its route matched by
canonical paths in that open's background job. A distro is named as WSL spells it
(`canonical`): `wsl.exe` takes names without case, so `\\wsl$\ubuntu\…` and
`\\wsl.localhost\Ubuntu\…` are one repo through one connection, which is named from `wsl.exe
--list` — an explicit open of a distro dv hasn't seen lists WSL's distros first, off the UI
thread — so every location it hands back is spelled WSL's way), `Reviews` (the known repos and, per repo, its
reviews (`ReviewStore::list`, an unreadable log a row of its own) and its uncommitted changes,
computed in the background when a repo is added, when the switcher opens, and when a
connection a row goes through goes down or comes back, whether or not the switcher is open (one
that comes back clears what its rows said about it at once), through `Locations::reach`: a
stopped distro's repo a row of its own whose **Start** connects to the distro, a failed
connection's whose **Retry** connects again, a reconnecting one's keeping its last listing
(§5.2); it opens or
resumes a target's review as the reviewer, whose name is the repo's `user.name`, or loads a review
by id (the switcher's rows for reviews of other targets; a remote repo that can't be reached
fails as "couldn't open Ubuntu:/home/kyle/api", the connection's error and diagnostics its
detail), either way bound to the review's home
worktree (§2.2: its backend, opened through `Locations`, which the session resolves, watches and
snapshots, while the repo it was opened from is the one marked used, the row the switcher
refreshes and the one the source picker offers); a row records why a repo can't be opened
at all and the worktree root its reviews name; a known repo is a worktree root, its distro
spelled WSL's way, and one added before it could be resolved (a subfolder added while its
connection was down, so its row could say why) or under another spelling of its distro becomes
that root, row and `known_repos.json` re-keyed (or dropped when the root is known already), the
first time a listing or an open resolves it; repos compare with the distro folded
(`Location::same_place`), so a spelling WSL couldn't confirm never makes a second row; so the switcher shows each target beside its open
review by the rule `open_review` resumes by (`review::ops::is_review_of`); it watches nothing
itself), `ReviewUi` (what the reviewer chose per review: split or unified), `GitHub` (gh calls, PR
cache), `Settings` (material, mode, panel width). The **system seam**
(open URL, reveal, folder picker, clipboard writes — recorded instead of performed under automation)
is its own global, `dv_ui::system`, because automation swaps the whole thing for a recorder.

**Data files** (the data dir, §13.3): `settings.json` (`material`, `mode`, `panel_width`),
`known_repos.json` (`{"v": 1, "repos": [{"location", "added_at", "opened_at"}]}`, ms; deliberately
not the first implementation's `repos.json`, which sits in the same default dir with another shape)
and `review_ui.json` (`{"v": 1, "reviews": {"<review id>": {"view_mode": "split" | "unified"}}}`).
Each is read leniently (unknown keys kept and written back), written atomically in the background,
and never written over when it exists but couldn't be read (`dv_ui::services::persist`).

**Views** (each its own entity and directory): `shell` (title bar, status bar, layout, window
material), `switcher` (repos + reviews, `Ctrl O`), `palette` (`Ctrl K`: files, reviews, PRs, commands —
the single fuzzy surface), `file_list`, `comments_list`, `diff_pane`, `thread` + `composer`, `submit`
(sheet), `onboarding`, `settings`, `banner` (errors and backend state, inline, never over content).

**Review session:** `ReviewSession` entity (`dv_ui::session`) = one review, its target fixed for
life: the repo and its store, the folded review, the comparison (§2.3: the interdiff since a
round, or all changes) and the `DiffSpec` it resolves to, the file list and each
file's state (unloaded, loading, loaded with its diff and, once they land, its highlights, or
failed), and the watch. The file list loads first; diffs load on demand (the stream asks for the
files around its viewport, at most 4 in flight, then every file a thread shows in, wherever it
is, so threads are placed against their files' current text, §8; "Load anyway" reloads one
without caps). A live
target (`working`, `staged`, a branch) watches the worktree when its new side is the working tree,
the git state and the store (§5.3); others the store only. Worktree and git batches are coalesced
again for 150 ms and refresh the spec and the list, keeping every loaded diff whose entry didn't
change and isn't in the batch (a working tree side's object id becoming known or unknown, which
`git add` alone does, is no change); the branch labels are read again only for a git batch, a
rescan or a moved spec; an append to the review's own log reloads the fold. The watch starts with
the session, and once it is registered whatever was read before it (the list, loaded diffs, the
fold) is read again, since a change made before then is never reported; the same after every
restart. A file that loads again keeps showing the diff it had until the new one lands, so its
cards and an open composer stay put; a switch of comparison lists everything again that way, the
diffs of the other comparison shown until theirs land (an old side listed under the other
comparison is another text, so no thread is placed and no comment anchored against it
meanwhile). A draft comment survives a switch: one whose file the comparison doesn't list, or on
an old side listed under the other comparison, keeps its text and lines, undrawn, until all
changes show it again (`C` switches there and gives it the keyboard); only a file that left the
review drops it. Stopping a watch joins its thread, so the session drops
handles on a thread of their own, never the UI's. A watch that can't cover the worktree leaves
a notice and the session refreshes on window activation instead. Writes go through `review::ops`
off the UI thread and reload the fold; a reply, Resolve or Reopen on a thread that shows moved
records its relocated anchor (§8; a thread whose file was renamed records the name it has now); a failed viewed toggle leaves a banner saying why. The shell holds the open
session; switching reviews swaps sessions (a small LRU of 3 may keep the last ones warm). Every open
goes through `Shell::open` — the switcher's rows, the source picker, automation's `open_review` —
in the background: the main area shows that it is opening, a failure becomes its banner (with
Retry), and resuming the review already open keeps its session; a pick whose connection is the
trouble waits for it (`shell::pending`, §5.2), never replacing by itself an open sheet or a
composer's draft. A review whose backend is remote
(a WSL distro's) follows its connection (`session::connection`): while it reconnects, a call that
fails with `BackendError::Disconnected` raises no banner and loads wait, the status bar saying
"reconnecting…"; the reconnect's synthetic rescan reads the list, the diffs and the fold again,
and a watch that didn't survive starts again. Once the connection has failed for good the main
area shows the backend-down banner (DESIGN §4.4), whose Retry is `Connection::retry` off the UI
thread, and the review's own refresh banner stays down. Paths above the `Backend` trait are never
read as this machine's for a WSL repo: worktree roots compare as the distro spells them
(`review::same_worktree` takes the repo's `Location`), and "Reveal in folder" goes through the
distro's share (`Location::file_path`). The shell hands the session to the
panel's `file_list` and to the `diff_pane`; the file list highlights the stream's current file and
asks the stream to show a file (`DiffPane::reveal_file` / `current_file`, the seam the stream
implements; the current file is the cursor's while it is on screen, else the one at the top).
The keyboard rests on the main area, which always renders with the diff pane's focus handle and
the `DiffPane` key context, so the shell's keys and the stream's both work there (DESIGN §5).
Overlays (the switcher, the source picker, the "Finish round" sheet) take the keyboard while open
and give it back where it was (the main area when that place is gone, such as a filter the panel
took away with it); a menu opens over the sheet rather than in its place, so its unsaved summary
survives, and should the keyboard ever rest on an element no longer drawn, the shell's
focus-lost listener hands it to the main area. Keys come from one table (`dv_ui::keymap`, actions
in `dv_ui::actions`). Viewed toggles flip the state last asked for until the fold reads it back,
one write per file at a time, so quick presses never ask for the same state twice. A load
generation counter on the session is the only stale-completion guard needed, because the target
never mutates: a list result applies only if no newer refresh started, and each file load carries a
token a refresh replaces.

Comment bodies render through `dv_ui::markdown` (gpui-component's `TextView` over the sanitiser of
§15: no image or raw HTML survives to be fetched; links open through the system seam).

**Threads, the composer and finishing a round.** The diff pane places the review's threads as
rows of its stream where `ReviewSession::placement` puts them — `review::place_thread` (§8) over
the review's files (in an interdiff, the target's own list, listed beside it) and the diffs
shown, on the UI thread; since relocation can scan a whole side, each result is kept until
something it read changes (its anchor, the comparison, its file's entry, the diff read), so a
refresh that keeps a file's diff keeps its threads' placements, and a side's fuzzy table is
shared by its threads; a file shown without text to place against (too large to show) has the
review's own diff of it read in the background for the threads anchored in it, kept while the
review's entry for the file stands as read — (`diff_pane::rows`: an insert after the last
line of a thread's range on its side, those lines kept shown rather than folded into a gap, or
an outdated card under the file's header; a thread whose file left the list in the stream's
"Not in the diff" section, a block of its own before the first file, `diff_pane::section`) and
draws them with `dv_ui::thread`'s card. In an
interdiff, old-side threads and threads whose file isn't in it are "not in the comparison":
not drawn, and listed with an "all changes" tag (DESIGN §4.1), placed against the review's own
diff of their file, read in the background, for where they stand in all changes; it also holds the one composer (a new thread under the selection, or a reply inside its
card), the focused thread (`]`/`[`, `R`, `X`) and which resolved threads are open. Posting,
replying, resolving and reopening are the session's `review::ops` writes; the pane counts them
until their follow-up lands, for `settled`. `comments_list` (the panel, under the files) lists
the waiting threads, addressed ones first, each group in stream order, and builds "Copy as prompt" (`thread::prompt_markdown`)
for the clipboard seam; a row's right-click menu resolves, reopens, replies (through the pane)
or copies that one thread. `Ctrl Enter` opens `submit::FinishSheet`, an overlay the shell holds
like the switcher, whose Submit is `ops::finish_round` (§2.3); while that runs the sheet can't be
closed or submitted again, so its outcome (or failure) always shows in it. A review whose status is
final is read-only — the shell's banner offers "Start a new review", which opens the target again (§2.4).

Keyboard map and states: DESIGN §5.

---

## 12. Onboarding, setup and doctor

### 12.1 Shape
- One list of **checks**, each with `detect()` (read-only, fast) and `apply()` (idempotent), per
  environment (Windows / each WSL distro / macOS / Linux). The same list backs the first-run screen,
  Settings › Setup, `dv setup` and `dv doctor`.
- **Setup** stays reachable after the first run (Settings › Setup, and the palette's "Set up dv"):
  the same screen with every environment's checks, each row its state and its fix, and **Check
  again**. A skill installed by an older dv, or edited since, shows as out of date with **Update**;
  when any environment has one, the status bar says so once ("dv-review skill out of date in
  Ubuntu · Update"), since an agent working from an old skill misuses the CLI.
- Never blocks local use: a local repo opens immediately. Only opening a WSL repo whose distro isn't
  set up shows the onboarding for that distro (the VS Code/Zed remote pattern, scoped to where it's
  needed).

### 12.2 Checks
1. **Environments:** Windows; WSL installed? distros, version, running state (no distro is booted
   just to list it).
2. **Backend in distro:** install/verify the Linux `dv` (§5.2), with progress.
3. **CLI on PATH:** Windows (HKCU `Path` + `WM_SETTINGCHANGE`), each distro (`~/.local/bin`, plus a
   shell-rc line if `~/.local/bin` isn't on its PATH), macOS (`/usr/local/bin/dv` symlink, admin prompt
   or `~/.local/bin`), Linux (`~/.local/bin`).
4. **Agent integrations:** per environment, detect Claude Code / Codex / Cursor and offer the skill;
   optional Claude Code `Stop` hook (§3 D). In a distro the check runs as that distro's Linux `dv`
   (`dv setup agents --json`, through the distro's backend), so it reads and writes the distro
   user's own `~/.claude`, `~/.agents` and `$CODEX_HOME`, and a sandboxed `DV_WSL_HOME` covers it
   in tests like every other distro write.
5. **git identity** per environment (comment author).
6. **GitHub:** `gh` present and `gh auth status` per environment — WSL's `gh` is a separate login.
   dv never runs `gh auth login` itself; it shows the command to run.
7. **Later:** default editor for "Open in editor" (VS Code / Cursor / Zed, with WSL remote URIs),
   `dv://` URL handler and Explorer/Finder "Review in dv" entry come with polish (phase 9); the
   update channel with packaging (phase 7).

### 12.3 Failure surface
Every failed check renders as one row: what failed, the exact error, a primary fix action, and "Copy
diagnostics". The repo banner for a dead WSL backend uses the same row.

---

## 13. Platforms and packaging

### 13.1 Binaries
- **Windows:** `dv.exe` (console subsystem: CLI, and launches the GUI detached) + `dv-gui.exe`
  (windows subsystem). The split exists only because a Windows exe is one or the other.
  `dv.exe` is a console program in every build, so a headless command prints and returns its
  exit code in a terminal. `dv` and `dv open` start the GUI detached (`crates/dv/src/launch.rs`):
  the same binary again with the hidden `--no-detach`, `DETACHED_PROCESS |
  CREATE_NEW_PROCESS_GROUP` on Windows (its own process group elsewhere), stdio closed, while
  the first returns 0 at once; `--automation` and `--no-detach` run the GUI in the process.
  A Windows child inherits every inheritable handle of its parent, the caller's stdout pipe
  among them, so the first makes its own standard handles non-inheritable before it starts the
  second: a caller capturing the output (`$(dv open .)`, an agent's shell tool) sees its end at
  once, not when the window closes. The detached GUI has no console, so a fatal startup error is
  a message box (`fatal.rs`): one before the app starts (the data dir) and one inside it (the
  window can't open, the fonts don't load) alike.
  `dv-gui.exe` comes with packaging (phase 7, for shortcuts and the installer); until then a
  double-clicked `dv.exe` flashes a console window while it detaches the GUI.
- **macOS:** `dv.app` containing one `dv` binary; the CLI is a symlink to it.
- **Linux:** one `dv` binary (GUI + CLI + backend), plus the static musl build used inside WSL.

**Shapes.** The `dv` crate's `gui` feature brings the app (`dv_ui`, gpui, gpui's platform with
its Linux `x11` feature); `automation` implies it. Three shapes, each linted by `script/check`
step 2:
- **default** (`gui` + `automation`): development, the tests and the e2e runner;
- **the shipped app** (`-p dv --no-default-features --features gui`): Windows, macOS, a Linux
  desktop;
- **headless** (`-p dv --no-default-features`): the CLI and `dv backend --stdio`, no gpui at all
  (step 3 checks its tree), which is the static musl build installed into WSL distros. It has
  none of the app's options, and `dv` and `dv open` exit 2 there (§10.1).

The unit tests in `dv` that need gpui are gated on `gui`; `crates/dv/tests/*` run in every shape.

### 13.2 Builds: local only
**No GitHub Actions runs until phase 10** (owner's rule, 2026-10-04: a Windows gpui build is slow and
burns paid minutes). Phase 0 deletes `.github/workflows/`; nothing is pushed expecting CI to check it.
Every gate runs locally through one script, `script/check` (bash; also used by agents):

1. `cargo fmt --all --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`, then `-p dv` again in the other two
   shapes (§13.1): `--no-default-features --features gui` (the shipped app) and
   `--no-default-features` (headless)
3. the gpui-boundary check (`cargo tree -e normal --target all`, §4): the headless crates with
   every feature, and `dv` without its default features
4. `cargo test --workspace`
5. a plain `cargo build -p dv -p mock_gh` (step 4's test build links the binaries with gpui's test
   support), then the e2e scenarios (`cargo run -p gpui_automation --bin e2e -- tests/e2e`),
   background mode; `--fast` skips this step. `--foreground` also runs `tests/e2e/foreground/`,
   the scenarios that must take the focus (screen captures of the window materials, §14.2)
6. `--linux`: inside WSL Ubuntu with `CARGO_TARGET_DIR` on ext4, `cargo test -p dv_core -p
   dv_backend -p dv_cli -p dv_test_support -p mock_gh`, `cargo test -p dv --no-default-features`
   (the backend loopback tests among them) and `cargo clippy -p dv_backend -p dv_cli -p dv
   --no-default-features --all-targets -- -D warnings`; then `script/build-linux`; then, on
   Windows, the real WSL (`crates/dv_backend/tests/wsl.rs`, `dv setup wsl`'s check and the CLI
   run inside the distro, in-process and as the built `dv`, §14.2) with `DV_TEST_WSL_DISTRO=Ubuntu`
   and `DV_WSL_HOME` a fresh folder under the distro's `/tmp`, `--test-threads=1`. Run inside a
   distro (Linux first: interop puts `wsl.exe` on its `PATH` too), the tests, the lint and the
   build run right there and the real-WSL step, which needs Windows, is skipped with a message;
   `script/build-linux` likewise builds where it runs on Linux and hops through `wsl.exe` only
   from Windows. The Linux GUI build needs X11/Wayland/fontconfig development packages,
   which take `sudo` to install in the distro, so it comes with phase 9's Linux GUI build and AppImage

**The Linux build** WSL distros run (§5.2) is `script/build-linux [--aarch64]` (bash, like
`script/check`): inside WSL Ubuntu, `rustup target add x86_64-unknown-linux-musl`, then `cargo
build -p dv --no-default-features --release --target x86_64-unknown-linux-musl` with the target
dir on ext4 and `CARGO_PROFILE_RELEASE_STRIP=symbols` (the release profile itself stays as the
Windows build has it); the result, a static ELF of about 5 MB that `--version` runs, is copied
beside the Windows builds as `target/debug/dv-linux-x86_64` and `target/release/dv-linux-x86_64`.
`--aarch64` adds `aarch64-unknown-linux-musl` linked with `rust-lld` (`dv-linux-aarch64`; it links
here, checked 2026-10-10; when it can't, the script says so and the x86_64 build stands). No
dependency of the headless shape needs a C toolchain beyond what the musl target ships. **The
Linux builds ship beside `dv.exe`, not embedded in it**: an embedded build would force a WSL
build into every Windows build. The app looks in `DV_LINUX_DIR` when set, else its own folder;
packaging (phase 7) puts them beside the installed `dv.exe`.

macOS has no local machine: macOS-only code is kept compiling by `cargo check --target
aarch64-apple-darwin` where that works without the SDK, and is otherwise unverified until a Mac or
phase 10's CI is available.


### 13.3 Install layout
Windows per-user `%LOCALAPPDATA%\dv\` (Velopack), settings in `%APPDATA%\dv\`; macOS
`/Applications/dv.app`, `~/Library/Application Support/dv/`; Linux AppImage or tarball,
`$XDG_CONFIG_HOME/dv/`. Every data path honours `--data-dir` / `DV_DATA_DIR` (tests, §14).

---

## 14. Automation and testing

### 14.1 Principles
Hermetic by default: every test and scenario uses its own data dir, its own fixture repos, `mock_gh`,
and a local backend. Nothing touches `%APPDATA%\dv` or GitHub unless an owner-only live switch is
set. Nothing touches a real distro but the real-WSL tests, which `DV_TEST_WSL_DISTRO` turns on and
which work only in a scratch home under the distro's `/tmp` (`DV_WSL_HOME`, §14.2), and the
owner's live check (§14.3).

### 14.2 Pieces
- **`dv_test_support::FixtureRepo`** — builds real git repos in temp dirs from a short script: files,
  commits, branches, CRLF + `autocrlf=true`, `.gitattributes`, renames, binaries, a 50k-line file,
  untracked files, worktrees. `FixtureRepo::persistent(path)` builds one at a given path and keeps
  it (its `HOME` and side directories in `<path>.fixture/`), for the automation host's seeded repos,
  which outlive the call that built them; `FixtureRepo::reopen(path)` finds one again (a later
  process, after a scenario's `restart`).
- **`mock_gh`** — reads a scenario file (`argv` pattern → stdout/stderr/exit), serves responses
  recorded from `kylekz/difftest` (scrubbed), and appends every call + stdin to a log tests assert on
  ("exactly these 5 comments were posted with `REQUEST_CHANGES`"). Error scenarios: 422 line-not-in-diff,
  auth failure, rate limit.
- **Backend conformance** — one suite (`crates/dv_backend/tests/conformance/`) runs the same cases
  against `LocalBackend` and against `RemoteBackend` over a server on in-memory pipes
  (`Launcher::InProcess`): repo info, git (failures, timeouts, ok codes, stdin, an index file),
  worktree reads (missing, links, binary, a 5 MB file), the store (concurrent appends, atomic
  writes, the index copy), invalid paths, every error's `Display` and `detail()`, and watches
  (worktree, store, git state; dropping the handle, the sink gone when `drop` returns and a sink
  in progress finishing first; from inside the sink too).
- **Backend loopback** — `crates/dv/tests/backend_loopback.rs` starts the built `dv backend
  --stdio` as a child (`Launcher::Command`, no WSL), in every shape of `dv`, so the protocol,
  reconnect and watch paths run on Windows and inside WSL with no distro setup: the handshake with
  and without a build (and a wrong one refused), a repository's git, store and watch, a kill
  mid-call (`Disconnected` at once, the reconnect, the synthetic rescan, calls working again),
  three blocked respawns ending `Failed` and `retry()` bringing it all back, and the server's end
  when the last connection goes. Each test checks every server it met has exited (no child
  leaks). `dv_backend`'s own unit tests cover the same lifecycle against in-process and fake
  servers with a shortened backoff, and the connecting rule with a stand-in host check: a death
  on a stopped host launching nothing and ending `Stopped` (on a running one, the respawn), a
  first call on a stopped host likewise, an explicit start booting it, the idle close and the
  automatic start after it (the idle period counted from the last watch's end, and a fresh
  count of respawn attempts after it), a listing's host check taken from the cache, an
  explicit start during a reconnect waiting for its outcome, calls failing at once while a
  reconnect installs, and a watch the new server takes back differently (degraded, refused)
  ending for its consumer; a child tied to the kill-on-close job is in it.
- **WSL without a distro** — `dv_backend`'s unit tests decode `wsl.exe`'s UTF-16LE output, read
  the launch markers and build the command lines everywhere, and on Linux run the launch,
  install, prune and check scripts themselves with `/bin/sh` in a scratch home
  (`wsl::Shell::local`), installing and starting a stand-in build; the connection's reinstall
  and `Installing` progress run against a fake launcher.
- **Real WSL** — `crates/dv_backend/tests/wsl.rs` (Windows; only when `DV_TEST_WSL_DISTRO` names
  a distro, `--test-threads=1`) runs every script through the real `wsl.exe` in a home of each
  test's own under `DV_WSL_HOME`, a fresh folder under the distro's `/tmp` (a value elsewhere
  fails the test rather than touch a real home), removed at its end: the distro listed as
  running, scripts and arguments arriving intact, an install streamed and checked (a wrong copy
  refused), a connection installing then shaking hands (`build_ok: true`) and the next one not
  installing again, a repository made under `/tmp` opening as `Location::Wsl` with its git,
  store and a watch seeing an edit made inside the distro, a build changed in the distro
  installed again, the prune, a `~/.local/bin` dv can't write (the install done with a
  warning, the server started), and a distro that isn't there: an automatic start launching
  nothing (`Stopped`), an explicit one `Unavailable` with `wsl.exe`'s words. `dv setup wsl`'s own check (`crates/dv_cli/src/tests/setup_wsl.rs`) runs the same way,
  and so does the CLI run inside the distro (§10.4): in-process with captured streams
  (`crates/dv_cli/src/tests/reexec.rs`), and as the built `dv` from Windows
  (`crates/dv/tests/wsl_cli.rs`, where cargo builds and names it): a first command installing
  this build in the scratch home, then `--wsl`, the share's spelling and a working directory
  inside the distro naming the repository, an exit code passed through (`review wait --timeout
  1` → 3, an error's 1 with its JSON), `--body-file -` over a pipe, `CLAUDECODE`, `DV_AUTHOR` and
  `DV_REVIEW` crossing through `WSLENV`, a `dv` killed during `review wait` taking the distro's
  `dv` with it (the kill-on-close job), stdout and stderr sent to one file coming out whole, a
  `~/.local/bin` dv can't write leaving the crossing working, and a distro that isn't there as
  exit 4 with its error document alone on stdout. Without a
  distro, `dv_cli`'s tests cover which commands cross, the arguments, the folder and `WSLENV`,
  the exit-4 paths that start no `wsl.exe`, and on Linux the run itself through `/bin/sh` with a
  stand-in build (installed, then run). `script/check --linux` runs all of these after
  `script/build-linux`.

- **Remote reviews in the GUI** — `dv_ui`'s tests open repos through a loopback connection over
  an in-process server (`Launcher::InProcess`, a route registered with `Locations`) and kill it,
  block its respawns and retry it: the session riding out a lost connection and catching up
  through the rescan, the status bar's states and the backend-down banner (also for a host found
  stopped, through a launcher's stand-in host check, `Launcher::with_host_check`), the
  switcher's reconnecting, Retry and Start rows, and the listing that never starts a stopped
  distro (WSL's stand-in, `WslAccess::Scripted`), even after a death. The e2e scenario
  `backend_loopback` does the same against this `dv`'s own `backend --stdio` as a child of the
  app (`seed_repo` with `"backend": "loopback"`, `backend_kill`, `backend_block`,
  `backend_stop`).
- **`gpui_automation` + `e2e` runner** — ported from the sibling app: JSONL scenarios in `tests/e2e/`, named
  targets (`.automation_target("file_list/src/routes/users.ts")`), frame-synced replies,
  `wait_for`/`assert` JSON-pointer checks, background runs that never take focus, per-scenario output
  dir with numbered screenshots and `failure.png`. dv adds custom commands `set_setting` (window
  material and mode), `seed_repo` / `repo_edit` (fixture repos at `<data_dir>/repos/<name>`, built
  and edited with `FixtureRepo` steps, a `write` step's `repeat` making a large file in one line,
  a `worktree` step adding a linked worktree; their environment overlays are registered with
  `Locations` for the work tree and each of its linked worktrees, wherever they are,
  and found again with `FixtureRepo::reopen` after a restart, so scenarios never depend on the
  owner's git configuration), `open` (adds a repo and opens the switcher on it, as `dv open`
  does), `open_review`, `review_op` (an agent's reply, address or "done" straight through
  `review::ops`, without the CLI's rules), and `cli` / `cli_spawn` / `cli_join`, which play the
  agent through the real CLI: the same `dv` executable started as a child process with a
  headless command (never one that opens a window), off the UI thread, in the seeded repo's
  fixture environment (also from one of its linked worktrees) with the app's data dir, a home of its own under it (`HOME`,
  `USERPROFILE`, `CODEX_HOME`: `dv setup agents` never reaches the owner's), and without the
  inherited `CLAUDECODE`, `DV_AUTHOR` and `DV_REVIEW` (a scenario sets them on purpose); `cli`
  runs one to its end and replies `{exit, stdout, stderr, json}`, `cli_spawn` starts one that
  blocks (`dv review wait`) and `cli_join` collects it, and a child still running when the app
  quits is killed. `seed_repo` with `"backend": "loopback"` reaches its repo through a loopback
  `dv backend --stdio` the scenario can kill (`backend_kill`), stop from starting
  (`backend_block`) and stop the host of, as `wsl --shutdown` stops a distro (`backend_stop`);
  `wsl_distros` tells WSL's stand-in which distros there are and which run; `state.backends`
  lists the connections.
  `workflow_a_agent` runs §3 workflow A end to end that way. The runner wires `mock_gh` itself:
  every launch gets `DV_GH` = `mock_gh` and `DV_DATA_DIR` = the scenario's data dir and no inherited
  `DV_WSL_HOME` (only a header's `env` names one, so dv reaches WSL through its stand-in), a scenario
  header's `gh` names the `mock_gh` scenario file (`DV_MOCK_GH`), and `{"gh_log": {}}` lines
  assert on its call log (`DV_MOCK_GH_LOG`) (tests/e2e/README.md).
- **Screenshots of the window material:** the default capture (`PrintWindow`) includes Mica but
  not Acrylic's blur-behind, and a background window is inactive. A `screenshot` with `"source":
  "screen"` copies the window from the desktop instead; it only works in front, so the scenarios
  that use it live in `tests/e2e/foreground/` and run on demand (`script/check --foreground`),
  never by default (tests/e2e/README.md, "Screenshots and window materials").
- **View tests** with gpui's `TestAppContext`/`VisualTestContext` next to each view (`tests.rs`).
- **CLI tests** run the commands in-process (`dv_cli::main_with` with a `Context` over captured
  streams, an explicit environment and the fixture's git environment) against fixture repos and
  `mock_gh`, from the agent's point of view; a few binary tests run the built `dv`
  (`CARGO_BIN_EXE_dv`) for dispatch, exit codes and real pipes. Neither ever starts the GUI, and
  every one removes `CLAUDECODE`, `DV_AUTHOR` and `DV_REVIEW` (which Claude Code's environment
  may carry) unless it sets them on purpose.

### 14.3 Owner-only live checks
`DV_LIVE_WSL=1` and `DV_LIVE_GH=1` (open + submit a review on `kylekz/difftest`). No agent, script
or CI run sets these.

The live WSL check is `crates/dv_backend/tests/live_wsl.rs`, `#[ignore]` and inert without
`DV_LIVE_WSL=1`: after `script/build-linux`, `DV_LIVE_WSL=1 cargo test -p dv_backend --test
live_wsl -- --ignored --test-threads=1` installs this build into the real home of
`DV_LIVE_WSL_DISTRO` (default `Ubuntu`) through the app's own connection (replacing the first
implementation's `~/.local/bin/dv` with the link, pruning other builds of `~/.local/share/dv/bin`,
leaving its other folders), checks `~/.local/bin/dv --version` there, opens a scratch repository
under `/tmp` (removed at the end) with a watch round trip, runs the distro's `dv` on it, and
then the Windows `dv` (`target/debug/dv.exe`, so `cargo build -p dv` first) for it, which runs
the command inside the distro (§10.4).

---

## 15. What carries over from the first implementation

Reviewed 2026-10-04 (live automation runs + four code reviews). Keep (port, then tidy):
- `dv_core` review model shapes (flat review/comment/reply, schema `v`, forward-compatible optional
  fields) → re-expressed as events (§7).
- `anchor_spec` mapping (source × side → which blob an anchor belongs to).
- Intraline word diff (`crates/core/src/diff/intraline.rs`).
- Persistent `git cat-file --batch` reader (`crates/core/src/git/batch.rs`), `:(literal)` pathspecs,
  `LC_ALL=C` for parsed git output.
- PR prep: merge-base semantics and verify-after-fetch (`crates/cli/src/pr.rs`).
- Submit: validate-everything-before-posting, the blocked-with-violations result, the do-not-resubmit
  guidance on writeback failure (`crates/cli/src/submit.rs`).
- Markdown body sanitiser (no remote fetches from comment bodies) (`crates/app/src/markdown.rs`).
- The SKILL.md workflow and its `dv review wait` loop (the wait itself is state-based now,
  §10.1).
- The four themes' colour values (DESIGN §1).

Do not carry over: the `Workspace`/`AppShell` god objects, the epoch/generation machinery, the
optional-host WSL routing and its fallbacks, the review lock, the hand-rolled arg parsing, the LSP
client and its node/vtsls provisioning (§18 is a new design), conflict badges, the PR diff cache and workspace LRU, comment archaeology.

Known bugs in the first implementation that v1 must not repeat (each gets a test): CRLF diffs (§6),
visited-only totals (§6.1), Histogram-based PR line validation (§9.3), one bad review file failing
`list` (§7), unpaginated GitHub threads (§9.2), hard-coded `origin` (D7), WSL route frozen at repo
open (§5.2), automation writing the real data dir (§14.1), raw git errors drawn over the sidebar
(DESIGN §4).

---

## 16. Phases

Each phase ends with its e2e scenarios green and a screenshot review against DESIGN.md.

| Phase | Scope | Done when |
|---|---|---|
| 0 | Workspace on `gpui-pre`, crates per §4, `script/check` (§13.2) and `.github/workflows/` removed, `gpui_automation` + `e2e` ported, `FixtureRepo`, `mock_gh` skeleton, window with Mica/Acrylic/Opaque | an empty Glass shell screenshots in all three materials, light and dark |
| 1 | `dv_core`: targets, git diff pipeline (§6 steps 1–3), event store (§7), `LocalBackend` | unit + fixture tests incl. CRLF/attributes/renames/binary |
| 2 | Review screen: switcher, file list (viewed, totals), diff stream (split/unified, gaps, highlight), threads + composer, comments list | Workflow A steps 1–3 by keyboard, e2e |
| 3 | CLI (§10) + skill + rounds/interdiff + relocation (§8) | Workflow A end to end with a scripted "agent" in an e2e scenario |
| 4 | `dv backend`, `RemoteBackend`, WSL install + routing + watch (§5), CLI re-exec (§10.4) | loopback tests pass on Windows and in WSL, and the real WSL ones (the app's connection, `dv setup wsl`, the CLI run inside the distro) in a sandboxed home (`script/check --linux`); the owner's live WSL check (§14.3) is ready for the owner to run |
| 5 | Code intelligence (§18): text selection and `Ctrl F`, the file view and its history, language servers where the repo lives (`spawn_stdio`, `dv_lsp`, `mock_lsp`), TypeScript 7, then Tailwind CSS and CSS: hover, go to definition, find references | a scenario against `mock_lsp` hovers, goes to a definition in the stream and in a file view, steps back and lists references; the real TypeScript check passes on a fixture project; the owner reviews a WSL TypeScript branch with it |
| 6 | Onboarding, setup and doctor (§12): the first-run screen and Settings › Setup over one list of checks per environment (Windows and each distro): dv in the distro, the CLI on PATH, the agent skills (Claude Code, Codex, Cursor) and the out-of-date notice, git identity, `gh`; `dv setup` and `dv doctor` | first run on a fresh data dir → onboarding → review, on Windows with a WSL distro (sandboxed home); an out-of-date skill in the distro is noticed and updated from Settings › Setup |
| 7 | Packaging + updates on Windows with Velopack (D9), local feed: `Setup.exe`, `dv-gui.exe`, dv's icon on the executables, the window and the shortcuts, the install hooks running `dv setup path` (§12), the Linux builds beside the installed `dv.exe` (§13.2); a development build never replaces the installed copy's `dv` in a distro | install from `Setup.exe` → PATH set, icon in the taskbar and Start → a WSL repo opens from the installed copy → a second local release updates it |
| 8 | GitHub: open PR, pull threads, submit sheet with validation and file-comment fallback (§9) | Workflow C against `mock_gh`; owner's live check |
| 9 | Polish: settings, palette, light themes, empty states, performance pass on a 2,000-file PR; the Linux GUI build and its AppImage, built in WSL (§13.2) | DESIGN review of every screen; perf budget (§17) met; the AppImage starts on a Linux desktop |
| 10 | *(last, optional)* GitHub Actions: CI matrix incl. macOS, release workflow publishing to GitHub Releases, update feed switched over | owner asks for it |

---

## 17. Risks and open questions

- **Budgets:** time-to-first-diff < 300 ms on a warm local repo, < 800 ms for a WSL repo with the
  backend connected; scrolling a 20k-line file at 120 fps; opening a 2,000-file PR's file list < 2 s.
- **`git diff -U<all>` cost on huge files** — bounded by the size cap (§6.2); measure in Phase 1.
- **Working-tree snapshots** write objects into the user's object store (§2.3). They are unreachable
  except via `refs/dv/**` and removed with the review; `git gc` handles the rest. Acceptable; document
  in the README.
- **GitHub API may gain unchanged-line comments** — when it does, §9.3's fallback becomes the normal
  path; keep validation behind one function.
- **WSL on arm64** — build `aarch64-unknown-linux-musl` from day one; test is owner-only.
- **A second GUI on the same data dir** is allowed for now: `dv open` always starts a new app.
  Handing the request to a running one (single instance) comes later; until then two apps share
  `settings.json` and `known_repos.json`, each written atomically, the last write winning.
- **macOS is unverified** until a Mac or phase 10's CI exists (§13.2); keep platform code behind small
  modules so the gap is contained.
- Window material (D8) is available: `gpui-pre` 0.3.6 has `WindowBackgroundAppearance::{Blurred,
  MicaBackdrop, MicaAltBackdrop}` (`gpui-pre-windows/src/window.rs`), checked 2026-10-04.

---

## 18. Code intelligence

Goal 8, D11, phase 5. Reviewing a change means reading the code around it: what a changed call
returns, where a renamed function is still used, what a type holds. dv answers those questions in
the review itself, with the project's own language servers, so the reviewer never opens an editor
to check one.

### 18.1 What the reviewer gets

- **Text.** The stream and the file view (below) select text with the mouse: a drag, a
  double-click for a word, a triple-click for a line. `Ctrl C` copies it through
  `dv_ui::system::write_clipboard`. A click in a line's code puts the cursor on that line (as a
  click on its number does) and a **caret** at the character, which `F12`, `Shift F12` and `Ctrl
  I` act on. A drag from a line's `+` still selects lines for a comment (DESIGN §4.1); a drag in
  the code selects text.
- **Find.** `Ctrl F` opens a find bar over the main area. In a file view it searches that file. In
  the stream it searches what the review shows, both sides, the text inside gaps included (loaded
  on demand); a match inside a gap opens the gap around it. Case-insensitive unless the query has
  a capital; `Enter` / `Shift Enter` go to the next / previous match; the bar counts them ("3 of
  17").
- **The file view.** Any file of the repo, whole, at the review's new side (or its old side, for
  a file the review deletes), drawn like the stream: highlighted, numbered, the review's changed
  lines marked with the diff's colours and its threads inline. Any new-side line takes a comment
  (its `+`, `C`); an anchor on a line the review didn't change is an ordinary anchor (§8). It opens
  from a file header's menu (**View file**), from `F` on the current file, from `Ctrl P` (every
  tracked file, filtered as the file list filters), and from go to definition and references.
  One file view shows at a time, over the stream; `Esc` returns to the stream. The stream and the
  file view share one **history** of positions: `Alt ←` / `Alt →` (and the mouse's back and
  forward buttons) step through it, so a jump and back returns to the line it left.
- **Hover.** The pointer resting 350 ms on an identifier, or `Ctrl I` at the caret, asks the
  file's servers. The card under the token shows the signature as highlighted code and the docs
  as Markdown (`dv_ui::markdown`, sanitised; a link to a file opens the file view, a web link goes
  through `system::open_url`). It stays while the pointer is over the token or the card, so its
  text can be selected, and offers **Go to definition** and **Find references**.
- **Go to definition.** `Ctrl`+click, or `F12` at the caret. A location the stream shows (a
  changed region of a listed file) scrolls the stream there; any other opens the file view there.
  Several locations list in the references panel.
- **Find references.** `Shift F12` at the caret, or the hover card's button. The panel's
  **References** tab (beside Files and Comments) lists them grouped by file in path order, each
  row its line's text, highlighted, with the symbol marked. Rows on lines the review changed carry
  the changed marker, and **In this review** keeps only those: "who else calls the function this
  change rewrote, and did the agent update them?" A row goes there as go to definition does.
- **Not included:** diagnostics (the agent's own checks report errors), completion, rename,
  code actions and formatting. dv never edits (§2.4).

### 18.2 Which text a server reads

A language server reads the project from disk, the working tree, and takes any open document's
text from its client instead. Mapped onto a review's sides:

- **The working tree.** A `working` review's new side, and a `branch` review's whose branch is
  checked out in its home worktree (§2.2), the usual case, are the files on disk. dv opens the
  document as it stands, and every answer is exact.
- **Any other text** (an old side, an interdiff's snapshot column, a `staged` review's index, a
  branch that isn't checked out, a file deleted since): dv opens that side's text as the document
  at the file's path (`didOpen`, which overrides the disk for that file only), so hover and
  definition read the text under the pointer. Other files are still read from the working tree,
  so a location in another file opens that file as it stands now. A hover card or references list
  answered this way says "read against the working tree".
- One version of a path is open at a time. Hovering the other side of the same file closes the
  document and opens that side's text.

### 18.3 Where servers run

A server runs where its repo lives, beside the repo's backend. `LocalBackend` spawns it on this
machine. For a WSL repo, the distro's `dv backend --stdio` spawns it inside the distro, so it
reads the distro's files natively; no file crosses 9P.

The `Backend` trait (§5.1) gains one generic capability rather than an LSP one:

```rust
fn spawn_stdio(&self, spec: &ProcessSpec, sink: ProcessSink) -> Result<ProcessHandle, BackendError>;
// ProcessSpec: the executable (an absolute path a resolver found, §18.4), args, a cwd relative to
// the worktree root, an env overlay. ProcessHandle: write stdin bytes, kill; dropping it kills.
// ProcessSink: stdout chunks, stderr lines (kept as diagnostics), the exit status.
```

On the wire (`PROTO = 2`): `spawn {repo, spec}` → `{proc}`, `proc_write {proc, bytes}`,
`proc_kill {proc}`, and `{"proc": n, "out": <base64>}` / `{"proc": n, "exit": <code>}`
interleaved like watch batches. A running process counts as in use for the idle close (§5.1).
Locally it joins the job object (`dv_backend::job`); in a distro the server puts it in a process
group of its own and kills that group when it is killed or when the backend's stdin ends, so a
language server never outlives dv. Nothing runs through a shell: the executable is one a resolver
found, and its arguments come from the language's spec.

### 18.4 Languages

A new crate, **`dv_lsp`** (never gpui), holds:

- **The client.** Content-Length framing; requests with ids and cancellation (`$/cancelRequest`
  when a hover is abandoned); answers to the server's own requests (`workspace/configuration`
  from the spec's settings, `client/registerCapability` recorded, `window/workDoneProgress/create`
  turned into the status bar's progress). It uses `lsp-types` 0.97, already in the tree through
  gpui-component. Positions are UTF-8 (`general.positionEncodings`); the client converts from
  UTF-16 for a server that offers only that.
- **The languages.** A language server is data, `LanguageServerSpec`: its id and name, the
  documents it serves (language ids and extensions), when it applies to a project (root markers,
  a dependency), how its executable is found (resolvers, in order), its arguments, initialization
  options and settings. Each spec is one module in `dv_lsp::languages`, registered in one list.
  Adding a language is one module and its tests. Several servers can serve one file (a `.tsx`:
  TypeScript and Tailwind CSS). Hover cards stack their answers in the list's order; definition
  and references take the first server that answers with anything.

**Resolvers** run in order, each a function of the repo's files read through the backend, so the
answer is the same wherever the repo lives:

1. **Project.** The project's own copy, at the version its lockfile pins. For TypeScript this is
   Node's resolution of `typescript` from the file's package directory, following symlinks (the
   npm, pnpm and bun layouts). TypeScript 7 or later gives its native executable, from
   `@typescript/typescript-<platform>-<arch>`'s `lib/`, run as `tsc --lsp --stdio`. No Node is
   needed. Checked 2026-10-13 on the owner's bun monorepo, with TypeScript 7.0.2 in WSL:
   initialize took 0.09 s, the first hover 0.9 s (the project loading), definition was immediate,
   references took 0.05 s across the workspace, and the server offers UTF-8 positions.
2. **Settings.** A command configured for that server (`settings.json`, `lsp.<id>.command`): an
   argument list, never a shell string.
3. **PATH.** The executable on `PATH`. In a distro this is the login shell's `PATH`, read once
   per connection (`$SHELL -lc 'printf %s "$PATH"'`): the backend runs under `wsl.exe --exec`
   without the user's profile, and that profile is where bun, nvm and the like add their tools.
4. **None.** The status bar names the missing server and how to get it (§18.5); so does the
   doctor (§12).

**JavaScript servers.** A server written in JavaScript (a spec marks it `runtime: js`) resolves
to its package's entry script, the file its `bin` names, not to the `bin` shim, whose `#!/usr/bin/env
node` line would pick the runtime. dv runs that script with a runtime of its own choosing: **bun**
first, then **node**, each found through settings (`lsp.runtime`), then `PATH` as above, then bun's
default install (`~/.bun/bin/bun`). A package installed with `bun add -g` is found in bun's global
folder (`~/.bun/install/global/node_modules`) as well as through `PATH`. A spec whose server is
known to break under bun can name node only.

dv installs neither language servers nor runtimes. The first implementation's Node and vtsls
provisioning stays behind (§15).

The first languages:

| Spec | Files | Server | Resolvers |
|---|---|---|---|
| `typescript` | `.ts .tsx .mts .cts .js .jsx .mjs .cjs` | TypeScript 7+'s native `tsc --lsp --stdio` | project, settings |
| `tailwindcss` | `.tsx .jsx .html .css` in a project whose CSS imports `tailwindcss` (v4) or with a `tailwind.config.*` (v3) | `tailwindcss-language-server --stdio` (`@tailwindcss/language-server`, JavaScript) | project, settings, PATH, bun global |
| `css` | `.css .scss .less` | `vscode-css-language-server --stdio` (`vscode-langservers-extracted`, JavaScript) | settings, PATH, bun global |

A project on TypeScript 6 or older needs `typescript-language-server` (JavaScript), found through
settings, `PATH` or bun's global folder. It is a spec like the others; its tests come when a project needs it.

### 18.5 Lifecycle

- One server per worktree and spec. When a review opens, after its file list shows, dv starts in
  the background the servers whose specs apply to the review's files, so the first hover doesn't
  wait for a project load. A worktree's servers stop 5 minutes after its last review closes.
- A file changed on disk is announced to servers that registered for it
  (`workspace/didChangeWatchedFiles`), from the backend's watch (§5.3).
- A server that exits restarts after 1, 2 and 5 s, then shows as failed until **Retry**, like a
  connection (§5.2). Requests in flight fail quietly: a hover just doesn't show.
- A hover times out after 5 s, a definition after 10 s, references after 30 s.
- The status bar names each server and its state: loading (with its progress), ready, failed (a
  click shows the command line, the last stderr lines and **Retry**), or missing (how to get it).

### 18.6 In the app

`dv_ui::intel` holds a `CodeIntel` entity per open review: its servers (through the review's
backend), the open documents, requests and their cancellation, and the last hover. The views are
`file_view`, the `references` panel tab, the hover card and the find bar. The stream and the file
view map the pointer to a line and a UTF-8 column with the same shaped lines they paint. Under
automation, `state` shows the open hover's text, the file view (path, side, top line), the
references rows, the find bar (query, current match, count) and the selected text. A token is a
target, `code/<path>/<old|new>/<line>/<column>`, for hovering and clicking.

### 18.7 Testing

- `dv_lsp`'s own tests use an in-process fake server: framing, cancellation, the encoding
  conversion, server requests, restart.
- **`mock_lsp`** (a bin, like `mock_gh`): a scripted server, a JSON scenario of the requests it
  expects and its answers. An override (`DV_LSP_<ID>`, like `DV_GH`) points a spec at it, so e2e
  scenarios cover hover, definition into the stream and into a file view, back, references, a
  server that crashes and restarts, and one that is missing, with no bun, Node or TypeScript on the
  machine.
- Resolver tests run on fixture trees with the npm, pnpm and bun layouts (symlinks, `.bun/`), on
  Windows and in WSL.
- `spawn_stdio` is tested over the loopback (in-process and as a child process): output, stdin,
  kill, exit, and the idle-close accounting. The real-WSL tests spawn a process in the sandboxed
  home.
- **A real TypeScript check**, opt-in: with `DV_TEST_TSC` naming a TypeScript 7 native `tsc`, a
  fixture project gets hover, definition and references from the real server. It is read-only
  and local, so `script/check` runs it whenever the variable is set; it is not an owner-only live
  check.

### 18.8 Order within phase 5

Each step is its own commit with its own scenarios:

1. Text selection and copy, and `Ctrl F` in the stream.
2. The file view, the history, `Ctrl P`, `Ctrl F` in a file view, and comments on any line.
3. `spawn_stdio` in both backends, `dv_lsp`, `mock_lsp` and the `typescript` spec: hover, go to
   definition, references, the status bar.
4. The JavaScript runtime (bun, then node), the `tailwindcss` and `css` specs, and stacked
   hover cards. Both run under bun (checked 2026-10-13 with bun 1.4.2 on the owner's Tailwind v4
   project): Tailwind's hover shows a class's CSS, its first answer after 9 s while it scans the
   project, so it starts with the review (§18.5); the CSS server's hover documents properties, but
   its go to definition found no custom property in a Tailwind v4 stylesheet, even one defined in
   the same file, so `css` comes last and may stay hover-only.

**Budgets:** a hover shows within 150 ms of the delay ending, once its server is ready; a file
view opens within 100 ms for a local repo and 300 ms for a WSL one.
