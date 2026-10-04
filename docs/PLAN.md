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
| `branch <b> [vs <base>]` | `merge-base(<base>, <b>)` | `<b>`'s tip, or the working tree when `<b>` is checked out in this worktree | yes |
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

### 2.3 Rounds
A round is the target as it stood when the reviewer last handed the review over.

- **Local targets:** when the reviewer finishes a round ("Request changes", "Approve", or explicitly
  "Snapshot round"), dv snapshots the new side: a temporary index (`GIT_INDEX_FILE`) + `git add -A` +
  `write-tree` + `commit-tree`, stored at `refs/dv/reviews/<id>/<n>`. Round n+1 defaults to the interdiff
  *round n → now*, with a toggle to the full diff.
- **PR targets:** each submitted GitHub review closes a round at the PR head it was submitted against.
  New commits on the PR offer "Start round n+1" (a new draft, interdiff since the last submitted head).

### 2.4 Status and invariants
- **Local review:** `open` → `changes_requested` ⇄ `in_progress` (agent ran `dv review done` →
  `ready`) → `approved` | `closed`. Never "submitted". Any of `open`, `changes_requested`,
  `in_progress` and `ready` may move to any other of `changes_requested`, `in_progress`, `ready`,
  `approved` and `closed`; agents set `in_progress` and `ready`, the reviewer `changes_requested`,
  `approved` and `closed`. Nothing returns to `open`.
- **PR review:** `draft` → `submitted` (read-only, owned by GitHub from then on: every later event is
  refused). The next round is a new draft review linked to the same PR. A draft may be abandoned
  (`closed`).
- `approved`, `closed` and `submitted` are final; every other status counts as open.
- **At most one open review per target.** Opening a target resumes it; "New review" on a target that has
  one asks "Resume or start fresh?". `working` and `staged` targets are per worktree (each worktree has
  its own changes); the others are the same target from every worktree, and a `pr` target is the same
  PR whatever head it was fetched at (`dv_core::review::ops::open_review`).
- A repo has many reviews; a PR has many reviews over time; local reviews need no PR.
- **dv never mutates the user's repo.** Allowed writes: `git fetch`, `.git/dv/**`, `refs/dv/**` and the
  objects those refs need. No commit, checkout, stash, reset or index change, ever.

### 2.5 Threads
- **Anchor:** round, path (and old path for renames), side (old/new), line range, **plus the anchored
  lines' text and 3 lines of context either side** (§8).
- **States:** `open` (waiting on the agent/author) → `addressed` (agent says done, with an optional note)
  → `resolved` (reviewer confirmed) or back to `open` ("Reopen"). Only the reviewer resolves and
  reopens. Agents (and the reviewer) address; addressing again replaces the note; a resolved thread is
  reopened before it can be addressed. Replies by anyone. Only an item's author edits or deletes it
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
| `dv_core` | lib | Location, the `Backend` trait and `LocalBackend` (§5.1), Target, git runner, diff pipeline (§6), review events + fold (§7), anchors (§8), GitHub via `gh` (§9) | **never** |
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
    fn watch(&self, spec: WatchSpec, sink: WatchSink) -> Result<WatchHandle, BackendError>; // §5.3
}
```

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
dropped except `dv/`; a linked worktree's `<git-common-dir>/info/` lies outside its tree and is
watched for `info/exclude` edits on its own. Events coalesce for 150ms and arrive as *paths*
(`WatchEvent { worktree_paths, store_paths, rescan }`: relative, `/`-separated, de-duplicated), so
the UI refreshes only the affected files and the file list; `rescan` means the OS may have dropped events (or a batch would list more than
10,000 paths). A directory that appears is listed before its batch goes out, because inotify
watches a new directory only after reporting it and a directory moved in brings its files without
an event each. `<git-common-dir>/dv/` is watched separately when it lies outside the worktree (a
linked worktree's common dir). Dropping the handle stops the watch before `drop` returns. The OS
watch registers ignored directories too (inotify needs a watch per directory), so on Linux a huge
`node_modules/` can exhaust the per-user limit; a watch of the worktree and the store then keeps
watching the store and its handle says why the worktree isn't watched, so the UI falls back to
refreshing the worktree itself instead of also losing review updates.

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
   nested repository (`dir/`) lists as a submodule. A worktree-side entry that `--raw` lists as
   modified with an unhashed new side but `--numstat` doesn't list is a stat-dirty file whose
   content is unchanged (the git policy's `diff.autoRefreshIndex=false`, §5.1) and is dropped. The
   list is sorted by path.
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
   reported. Under `core.autocrlf` or `text eol=crlf` git compares the normalised LF form, so an
   edited CRLF checkout is exactly its edited lines. A type change (file ↔ symlink, file ↔ gitlink)
   prints as a deletion plus an addition of one path and becomes one diff.
3. **Intraline:** word-level ranges for paired changed lines (kept from the first implementation:
   the i-th removed line of a change block pairs with the i-th added one, token diff with
   histogram, pairs over 10,000 token pairs or more than 70% rewritten left unmarked).
4. **Highlight:** tree-sitter on both full sides, off the UI thread, producing **token kinds**, not
   colours. Colours resolve at paint time from the theme, so theme changes repaint without recomputing
   and cached rows never go stale. Rows appear unhighlighted first and colour in when ready.
5. **Rows:** built lazily for the current view mode only; unchanged runs beyond the context setting
   become gap rows that expand in 20-line steps from either edge or fully, from the already-loaded
   content (no refetch; `dv_core::diff::{segments, expand_gap}`). A run shorter than two lines stays
   shown, since its gap row would take the same space.
6. **Interdiff (rounds):** the same pipeline between round n's snapshot and the current new side.
7. **Stream:** the diff pane is one virtualised list over *all* files (file header rows pinned while
   scrolling), so `J`/`K` scroll to a file instead of swapping views.

---

## 7. Review store

`dv_core::review`: `id`, `event`, `model` (the folded state), `fold`, `store` (`ReviewStore`) and
`ops` (the checked changes the UI and the CLI call).

- **Location:** `<git-common-dir>/dv/reviews/<id>.jsonl` (resolved with `git rev-parse
  --git-common-dir`, never by hand), shared by every worktree of the repo. The `created` event records
  the worktree root it was made in (`worktree`), which is what a `working` or `staged` target means.
- **Events (v1):** `created{target, author, worktree}`, `round_closed{n, new_oid, old_oid, verdict?}`,
  `viewed{round, path, on}`, `thread_added{id, anchor, body, author}`, `reply_added{thread, id, body,
  author}`, `thread_state{thread, state, note?, author}`, `body_edited{target_id, body}`,
  `deleted{target_id}`, `pr_linked{owner, repo, number}`, `submitted{github_review_id, url, event}`,
  `status{status}`. Each line is `{"v":1, "eid", "at" (ms), "kind": …, …}`: the format version, a
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
  `created` event is unreadable.
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
  snapshot rewritten atomically. Operations fold without rewriting it: their own append makes a
  snapshot of the state they loaded stale at once. A snapshot that can't be read or written is
  ignored.
- **Deletion** of a review removes its log, its snapshot and its `refs/dv/reviews/<id>/*` (one
  `git update-ref --stdin` over what `for-each-ref` lists), log first, and finishes an interrupted
  delete when run again.

## 8. Anchoring and relocation

On every refresh, each thread whose anchored text no longer matches at its recorded lines is relocated:
1. exact match of the anchored lines within ±200 lines, nearest wins;
2. else exact match anywhere in the file, if unique;
3. else fuzzy match (anchored + context lines, normalised whitespace, similarity ≥ 0.8), best wins;
4. else the thread is **outdated**: shown in the comments list with its original code snippet, not
   inline.

Renames carry anchors via the old path. Relocation is display-only until the next event on the thread,
which records the new anchor. `dv comment list --json` reports `anchor.current` (relocated) and
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
dv review done [<id>] [--note <t>]         agent: round addressed, ready for re-review
dv review submit [<id>] --verdict comment|approve|request-changes [--body <t> | --body-file <f|->]
dv review close | delete <id>

dv comment list [--status open|addressed|resolved|outdated] [--file <p>]
dv comment show <id>                       thread, anchor (current + original), anchored code
dv comment add --file <p> --lines N[:M] [--side old|new] (--body <t> | --body-file <f|->)
dv comment reply <id> (--body <t> | --body-file <f|->)
dv comment address <id> [--note <t>]       agent
dv comment resolve | reopen <id>           reviewer

dv setup [path | agents | wsl <distro> | all] [--yes]
dv doctor [--json]
dv backend --stdio                         (Linux build only; started by the GUI)
```

Global flags: `--repo <path>` (also `\\wsl.localhost\<distro>\<path>`), `--wsl <distro>:<path>`,
`--review <id>`, `--json`, `--author <name>`.

### 10.2 Which review a command means
1. `--review <id>`; 2. else `$DV_REVIEW`; 3. else the single open review whose target matches the
current context (working tree of the cwd's worktree, or its checked-out branch); 4. else exit 2 with
the candidates listed. Never a silent "newest draft" guess.

### 10.3 Output and exit codes
- `--json` emits documented DTOs (`dv_cli::dto`), versioned with a top-level `"schema": 1` — never the
  internal event or state structs. snake_case throughout.
- Exit codes: `0` ok, `1` operation error, `2` usage / ambiguous review, `3` wait timeout, `4` backend
  unavailable (WSL), `5` GitHub error (with `gh`'s message).
- Errors: stderr always; `{"error": {"code", "message"}}` on stdout with `--json`.

### 10.4 WSL routing
Inside a distro, `dv` is the native Linux binary. On Windows, a command whose repo is a WSL location
re-executes itself as `wsl.exe -d <distro> --cd <path> --exec ~/.local/bin/dv <args>` and passes
through stdio and the exit code. One crossing, no per-git-command spawns.

### 10.5 The agent skill
`dv setup agents` installs the `dv-review` skill for each detected agent and environment (Claude Code
`~/.claude/skills/dv-review/`, Codex, Cursor) — content embedded in the binary, idempotent, drift
repaired. The skill teaches: `dv status` → `dv comment list --status open --json` → fix →
`reply`/`address` → `dv review done` → `dv review wait`; never `resolve`; bodies via `--body-file -`.

---

## 11. UI architecture (`dv_ui`)

The sibling app's shape: per-view directories, a services layer of entities installed as a gpui `Global`,
typed events, and `is_settled()` on every service for automation.

**Services** (`dv_ui::services`): `Locations` (detected environments, backend connections),
`Reviews` (index of reviews across known repos; fold + watch), `Diff` (per open review: file list,
loaded files, highlight queue), `GitHub` (gh calls, PR cache), `Settings`. The **system seam** (open
URL, reveal, folder picker — recorded instead of performed under automation) is its own global,
`dv_ui::system`, because automation swaps the whole thing for a recorder.

**Views** (each its own entity and directory): `shell` (title bar, status bar, layout, window
material), `switcher` (repos + reviews, `Ctrl O`), `palette` (`Ctrl K`: files, reviews, PRs, commands —
the single fuzzy surface), `file_list`, `comments_list`, `diff_pane`, `thread` + `composer`, `submit`
(sheet), `onboarding`, `settings`, `banner` (errors and backend state, inline, never over content).

**Review session:** `ReviewSession` entity = one review, its target fixed for life, holding the `Diff`
service handle and the folded review. Switching reviews swaps sessions (a small LRU of 3 may keep the
last ones warm). A load generation counter on the session is the only stale-completion guard needed,
because the target never mutates.

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
6. `--linux`: `cargo test` of the headless crates (`dv_core`, `dv_test_support`, `mock_gh`) inside
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
  untracked files, worktrees.
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
  material and mode), `seed_repo`, `open`, `open_review`. The runner wires `mock_gh` itself:
  every launch gets `DV_GH` = `mock_gh` and `DV_DATA_DIR` = the scenario's data dir, a scenario
  header's `gh` names the `mock_gh` scenario file (`DV_MOCK_GH`), and `{"gh_log": {}}` lines
  assert on its call log (`DV_MOCK_GH_LOG`) (tests/e2e/README.md).
- **Screenshots of the window material:** the default capture (`PrintWindow`) includes Mica but
  not Acrylic's blur-behind, and a background window is inactive. A `screenshot` with `"source":
  "screen"` copies the window from the desktop instead; it only works in front, so the scenarios
  that use it live in `tests/e2e/foreground/` and run on demand (`script/check --foreground`),
  never by default (tests/e2e/README.md, "Screenshots and window materials").
- **View tests** with gpui's `TestAppContext`/`VisualTestContext` next to each view (`tests.rs`).
- **CLI tests** run the built `dv` binary against fixture repos and `mock_gh`, from the agent's point
  of view.

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
- `dv review wait` semantics and the SKILL.md workflow.
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
- **macOS is unverified** until a Mac or phase 9's CI exists (§13.2); keep platform code behind small
  modules so the gap is contained.
- Window material (D8) is available: `gpui-pre` 0.3.6 has `WindowBackgroundAppearance::{Blurred,
  MicaBackdrop, MicaAltBackdrop}` (`gpui-pre-windows/src/window.rs`), checked 2026-10-04.
