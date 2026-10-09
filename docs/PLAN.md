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

**Non-goals (v1):** LSP / go-to-definition, AI features inside dv, editing files, committing or any
other mutation of the user's repo (§2.4), cross-machine review sync, GitHub Enterprise-specific work
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
  `vpk` is a .NET tool: the build machine needs the .NET SDK (not installed yet; only Phase 8 needs it).
- **In the app:** the `velopack` crate. `VelopackApp::build().run()` is the first call in `main` of
  both Windows binaries; its install/uninstall hooks run `dv setup path --yes` / remove the PATH entry,
  so PATH stays app-owned (§12) and the installer has no logic of its own.
- **Updates:** `UpdateManager` checks a release feed, downloads the delta in the background and
  applies it on the next restart ("Restart to update" in the status bar). Until there is a release
  workflow (§16 phase 9) the feed is a local folder of `vpk pack` output, so a locally built release is
  picked up by the installed copy; later the same code points at GitHub Releases.
- Chosen over cargo-packager (the sibling app's tool; a classic NSIS wizard and an updater that re-downloads the
  whole installer and needs manifest signing keys) for delta updates and the simpler install flow.

### D10. Code signing: none
The owner builds and installs locally, which SmartScreen doesn't challenge (it checks files carrying a
downloaded-from-the-internet mark). Revisit only if dv is ever distributed as a download.

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
4. The agent (blocked in `dv review wait`) wakes, runs `dv comment list --status open --json`, fixes,
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
| `dv_backend` | lib | The stdio protocol (§5), server loop, `RemoteBackend` client, distro install | never |
| `dv_cli` | lib | clap command tree, JSON output DTOs, setup/doctor checks (§10, §12) | never |
| `dv_ui` | lib | views + services (§11) | yes |
| `dv` | bin | entry: GUI, CLI, and on Linux `dv backend`; on Windows also the console/GUI split (§13.1) | yes (GUI feature) |
| `gpui_automation` | lib + bin `e2e` | ported from the sibling app (§14) | yes |
| `mock_gh` | bin | fake `gh` for tests (§14.2) | no |
| `dv_test_support` | lib (dev) | fixture repo builder, shared by every crate's tests | no |

Direction: `dv` → `dv_ui` → {`dv_cli`, `dv_backend`, `dv_core`, `gpui-component`};
`dv_cli` → {`dv_backend`, `dv_core`}; `dv_backend` → `dv_core`. A gpui import outside `dv_ui`, `dv`
and `gpui_automation` is a build failure (`script/check` verifies it with `cargo tree`, §13.2).

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

### 5.2 Lifecycle (WSL)
- One connection per distro, opened when the user first opens a repo in it (never at launch, never for
  the switcher's listing — a stopped distro stays stopped until asked).
- Handshake: the client sends its build hash; the server must match exactly (both come from the same
  build — §13). Mismatch → reinstall once → if still mismatched, the banner.
- Install: `~/.local/share/dv/bin/dv-<hash>` + `~/.local/bin/dv` symlink, verified by sha256; old hashes
  pruned after a successful switch.
- Death: in-flight requests fail with a typed error, the UI shows "Reconnecting…", the client respawns
  with backoff (1s, 2s, 5s, then manual Retry), watches resubscribe and fire one synthetic change.

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
6. else **outdated**: shown in the comments list with its original code snippet, not inline.

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
the list is in the comments list alone.

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
                                           reviewer (`comment`: PR reviews, phase 5)
dv review close | delete <id>              reviewer

dv comment list [--status open|addressed|resolved|outdated]… [--file <p>]
dv comment show <id>                       thread, anchor (current + original), anchored code
dv comment add --file <p> --lines N[:M] [--side old|new] (--body <t> | --body-file <f|->)
dv comment reply <id> (--body <t> | --body-file <f|->)
dv comment address <id> [--note <t> | --note-file <f|->]
                                           agent
dv comment resolve | reopen <id>           reviewer

dv setup [path | agents | wsl <distro> | all] [--yes]
dv doctor [--json]
dv backend --stdio                         (Linux build only; started by the GUI)
```

Global flags: `--repo <path>` (also `\\wsl.localhost\<distro>\<path>`), `--wsl <distro>:<path>`,
`--review <id>`, `--json`, `--author <name>`; they go anywhere on the line.

- **One clap tree.** `dv_cli::args` holds the global flags and the headless commands; the `dv`
  binary keeps the GUI's options (`--data-dir`, `--automation…`, `--window-size`, the hidden
  `--no-detach`) and `open`, and flattens `dv_cli::Command` into its own subcommands, so `dv
  --help` lists every command. A headless command goes to `dv_cli::run(&global, command, &mut
  Context)` before any gpui code runs. `Context` carries the working directory, the environment
  (`dv_cli::Env`: `$DV_REVIEW`, `$DV_AUTHOR` and `CLAUDECODE` are read through it, never by
  clap), stdin, stdout, stderr, whether they are terminals, and the environment overlay every git
  the command spawns gets (`LocalBackendOptions::env`), so tests run commands in-process and
  hermetically (`dv_cli::main_with` parses too); a few binary tests in `crates/dv/tests/` check
  the real process.
- **The repository** is `--wsl`, else `--repo` (relative to the working directory, the path
  that makes read as a location again, so a relative path from a working directory inside a
  distro, `\\wsl.localhost\…`, is a WSL location too), else the working directory's. Not
  inside a work tree: exit 1. A WSL location (`\\wsl.localhost\…`,
  `\\wsl$\…`, `--wsl`) is exit 4 until §10.4's routing exists (phase 4), without starting
  `wsl.exe`.
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
  deleted branch, is exit 1, `target`, with git's words; `review submit` too). `--status outdated` keeps the threads whose code can't
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
    open threads (`dv comment list --status open --json`), then `dv review done <id> --round
    <n>`; one that hasn't closed is exit 2. The event is conditional on the closed rounds
    (`if_round`), and a round that closed between the check and the append makes it a no-op,
    reported as `finished` when that round finished the review, else `unseen_round`.
  - `review submit` is the reviewer's `ops::finish_round` with the verdict and the summary
    (`--body`, `--body-file`), snapshotting the review's home worktree (§2.2) whichever worktree
    it runs in; `--verdict comment` on a local review is exit 2 ("a local round approves or requests
    changes"), a PR review exit 2 (`unsupported`, phase 5). `review close <id>` sets `closed`;
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
  {open, addressed, resolved}}`, or `{"schema": 1, "outcome": "timeout"}`; the text ends with what
  to do next ("1 open thread: run `dv comment list --status open`, …"; with no open thread, the
  round's summary or the reviewer's replies, since a reviewer may request changes in the
  summary alone; on a timeout, "run `dv review wait <id>` again", naming the review). A
  finished review is no
  longer in context (§10.2), so a wait that may start after the verdict names the review (the
  skill passes the id `dv status` gave).
- **`setup agents [--yes]`** installs the skill (§10.5).
- **Recognised, not yet run:** `review new --pr` and `dv open` of a PR (a URL or `#n`: "GitHub
  PRs arrive in a later version", before any window opens), `review submit` of a PR review,
  `setup` other than `agents`, `doctor` and `backend` exit 2 (`unsupported`) with what to do
  instead; `setup wsl` is exit 4.
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
  standard handles (§13.1), so a caller capturing `dv open`'s output gets its end at once.

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
  they were), `current_code` (the lines at `anchor.current` now), `body`, `author: {kind,
  name}`, `created_at`, `note` (the last state change's), `replies: [{id, author, body,
  created_at}]`. The writes print: `review new` `{created, review}`; `review done` and `close`
  `{changed, review}`; `review submit` `{review, round}`; `review delete` `{deleted}`; `comment
  add` `{thread}`; `comment reply` `{reply, thread}`; `comment address | resolve | reopen`
  `{changed, thread}`; `setup agents` `{agents: [{agent, path, action: installed | updated |
  unchanged | skipped, reason}]}`; `review wait` as in §10.1.
- Exit codes: `0` ok, `1` operation error, `2` usage / ambiguous review, `3` wait timeout, `4` backend
  unavailable (WSL), `5` GitHub error (with `gh`'s message).
- Errors: stderr always (`dv: <message>`, then the candidates and the failed git command); with
  `--json` also `{"schema": 1, "error": {"code", "message", "detail"?, "candidates"?}}` on stdout.
  Codes (`dv_cli::error::code`): `usage`, `unsupported`, `ambiguous_review`, `no_review`,
  `not_found`, `not_a_repository`, `not_allowed`, `rejected`, `finished`, `unseen_round`,
  `cancelled`, `backend_unavailable`, `target` (also a review whose home worktree is gone),
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
  one per line: `id  label  status  round n  worktree`.

### 10.4 WSL routing
Inside a distro, `dv` is the native Linux binary. On Windows, a command whose repo is a WSL location
re-executes itself as `wsl.exe -d <distro> --cd <path> --exec ~/.local/bin/dv <args>` and passes
through stdio and the exit code. One crossing, no per-git-command spawns. Until that lands (phase
4) such a command is exit 4, saying how to run dv inside the distro instead.

### 10.5 The agent skill
The `dv-review` skill is `crates/dv_cli/skill/SKILL.md`, embedded in the binary
(`dv_cli::skill::SKILL_MD`), frontmatter `name: dv-review` and a description that triggers on
"address review comments", "dv review" and "the reviewer left comments". It teaches the loop
`dv status --json` (keep `current.id`; no current review and none clearly the one asked about:
tell the user, never guess or start one) → read the round's summary (`current.rounds`' last
`body`, `round.body` in the wait's output: a reviewer may ask for more there than in the
threads) → `dv comment list --status open --json`, and `--status addressed` for the reviewer's
replies on threads already addressed → for each thread, `dv comment show <id> --json` again
before the fix (line numbers go stale once the agent edits that file) and act on
`anchor.current` / `current_code` → fix → `dv comment reply <id> --body-file -` (a quoted heredoc
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
rules: `--json` when parsing, `--author <agent>` on every write, `<agent>` being the agent's own
name, which every example writes as that placeholder (Claude Code is recognised by `CLAUDECODE`
anyway), never `resolve` or `reopen`, no `dv review new` unless the
user asks, text through `--body-file -` / `--note-file -` and a short note inline only in single
quotes (double quotes run backticks in a POSIX shell, and a backtick is PowerShell's escape), a
disagreement is a reply with reasons and still an address with a note saying so;
and what each exit code means (1 `finished`: stop; 1 `unseen_round`: read the round, then `done
--round`; 2 with candidates: pick with `--review`, with none: tell the user; 4: run dv inside the
distro). Every
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
per-environment setup (WSL distros) come with phase 6.

---

## 11. UI architecture (`dv_ui`)

The sibling app's shape: per-view directories, a services layer of entities installed as a gpui `Global`,
typed events, and `is_settled()` on every service for automation.

**Services** (`dv_ui::services`): `Locations` (one `Arc<dyn Backend>` per repo, opened off the UI
thread and cached by the location asked for and its worktree root; an environment overlay can be
registered for a path, which the automation host does for its seeded repos; a WSL location is
`BackendError::Unavailable` until Phase 4 and never boots a distro), `Reviews` (the known repos and,
per repo, its reviews (`ReviewStore::list`, an unreadable log a row of its own) and its uncommitted
changes, computed in the background when a repo is added and when the switcher opens; it opens or
resumes a target's review as the reviewer, whose name is the repo's `user.name`, or loads a review
by id (the switcher's rows for reviews of other targets), either way bound to the review's home
worktree (§2.2: its backend, opened through `Locations`, which the session resolves, watches and
snapshots, while the repo it was opened from is the one marked used, the row the switcher
refreshes and the one the source picker offers); a row records why a repo can't be opened
at all and the worktree root its reviews name, so the switcher shows each target beside its open
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
Retry), and resuming the review already open keeps its session. The shell hands the session to the
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
an outdated card under the file's header) and draws them with `dv_ui::thread`'s card. In an
interdiff, old-side threads and threads whose file isn't in it are "not in the comparison":
not drawn, and listed with an "all changes" tag (DESIGN §4.1), placed against the review's own
diff of their file, read in the background, for where they stand in all changes; it also holds the one composer (a new thread under the selection, or a reply inside its
card), the focused thread (`]`/`[`, `R`, `X`) and which resolved threads are open. Posting,
replying, resolving and reopening are the session's `review::ops` writes; the pane counts them
until their follow-up lands, for `settled`. `comments_list` (the panel, under the files) lists
the waiting threads, addressed ones first, each group in stream order, and builds "Copy as prompt" (`thread::prompt_markdown`)
for the clipboard seam. `Ctrl Enter` opens `submit::FinishSheet`, an overlay the shell holds
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
   optional Claude Code `Stop` hook (§3 D).
5. **git identity** per environment (comment author).
6. **GitHub:** `gh` present and `gh auth status` per environment — WSL's `gh` is a separate login.
   dv never runs `gh auth login` itself; it shows the command to run.
7. **Optional:** default editor for "Open in editor" (VS Code / Cursor / Zed, with WSL remote URIs),
   `dv://` URL handler, Explorer/Finder "Review in dv" entry, update channel.

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
  `dv-gui.exe` comes with packaging (phase 8, for shortcuts and the installer); until then a
  double-clicked `dv.exe` flashes a console window while it detaches the GUI.
- **macOS:** `dv.app` containing one `dv` binary; the CLI is a symlink to it.
- **Linux:** one `dv` binary (GUI + CLI + backend), plus the static musl build used inside WSL.

### 13.2 Builds: local only
**No GitHub Actions runs until phase 9** (owner's rule, 2026-10-04: a Windows gpui build is slow and
burns paid minutes). Phase 0 deletes `.github/workflows/`; nothing is pushed expecting CI to check it.
Every gate runs locally through one script, `script/check` (bash; also used by agents):

1. `cargo fmt --all --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`, and again with `-p dv --no-default-features`
   (the shipped shape, automation off)
3. the gpui-boundary check (`cargo tree`, §4)
4. `cargo test --workspace`
5. a plain `cargo build -p dv -p mock_gh` (step 4's test build links the binaries with gpui's test
   support), then the e2e scenarios (`cargo run -p gpui_automation --bin e2e -- tests/e2e`),
   background mode; `--fast` skips this step. `--foreground` also runs `tests/e2e/foreground/`,
   the scenarios that must take the focus (screen captures of the window materials, §14.2)
6. `--linux`: `cargo test` of the headless crates (`dv_core`, `dv_cli`, `dv_test_support`, `mock_gh`) inside
   WSL Ubuntu with `CARGO_TARGET_DIR` on ext4; phase 4 adds the musl backend, the Linux GUI build
   and the backend loopback tests

macOS has no local machine: macOS-only code is kept compiling by `cargo check --target
aarch64-apple-darwin` where that works without the SDK, and is otherwise unverified until a Mac or
phase 9's CI is available. The Windows bundle embeds the musl builds it installs into distros, keyed by
the same build hash (§5.2).

### 13.3 Install layout
Windows per-user `%LOCALAPPDATA%\dv\` (Velopack), settings in `%APPDATA%\dv\`; macOS
`/Applications/dv.app`, `~/Library/Application Support/dv/`; Linux AppImage or tarball,
`$XDG_CONFIG_HOME/dv/`. Every data path honours `--data-dir` / `DV_DATA_DIR` (tests, §14).

---

## 14. Automation and testing

### 14.1 Principles
Hermetic by default: every test and scenario uses its own data dir, its own fixture repos, `mock_gh`,
and a local backend. Nothing touches `%APPDATA%\dv`, GitHub or a real distro unless an owner-only live
switch is set.

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
- **Backend loopback** — `RemoteBackend` tests spawn the native `dv backend --stdio` locally (no WSL),
  so the protocol, reconnect and watch paths run locally on Windows and inside WSL, no distro setup needed.
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
  quits is killed.
  `workflow_a_agent` runs §3 workflow A end to end that way. The runner wires `mock_gh` itself:
  every launch gets `DV_GH` = `mock_gh` and `DV_DATA_DIR` = the scenario's data dir, a scenario
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
`DV_LIVE_WSL=1` (real distro smoke: install, open, watch, CLI inside the distro) and `DV_LIVE_GH=1`
(open + submit a review on `kylekz/difftest`). No agent or CI run sets these.

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
optional-host WSL routing and its fallbacks, the review lock, the hand-rolled arg parsing, LSP and
node/vtsls provisioning, conflict badges, the PR diff cache and workspace LRU, comment archaeology.

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
| 4 | `dv backend`, `RemoteBackend`, WSL install + routing + watch (§5), CLI re-exec | loopback tests pass on Windows and in WSL (`script/check --linux`); owner's live WSL check passes |
| 5 | GitHub: open PR, pull threads, submit sheet with validation and file-comment fallback (§9) | Workflow C against `mock_gh`; owner's live check |
| 6 | Onboarding/setup/doctor (§12) | first run on a fresh data dir → onboarding → review, on Windows with a WSL distro |
| 7 | Polish: settings, palette, light themes, empty states, performance pass on a 2,000-file PR | DESIGN review of every screen; perf budget (§17) met |
| 8 | Packaging + updates with Velopack (D9), local feed; Linux AppImage built in WSL | install from `Setup.exe` → PATH set → a second local release updates the installed copy |
| 9 | *(last, optional)* GitHub Actions: CI matrix incl. macOS, release workflow publishing to GitHub Releases, update feed switched over | owner asks for it |

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
- **macOS is unverified** until a Mac or phase 9's CI exists (§13.2); keep platform code behind small
  modules so the gap is contained.
- Window material (D8) is available: `gpui-pre` 0.3.6 has `WindowBackgroundAppearance::{Blurred,
  MicaBackdrop, MicaAltBackdrop}` (`gpui-pre-windows/src/window.rs`), checked 2026-10-04.
