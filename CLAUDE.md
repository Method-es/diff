# dv

Native desktop app (Rust + GPUI) for reviewing diffs locally, above all a coding agent's work before
it is committed, and for reviewing and submitting large GitHub PRs. Windows first with first-class
WSL; macOS and Linux supported. Agents read and answer reviews through the `dv` CLI.

- Spec: `docs/PLAN.md` (decisions D1–D10, architecture, phases; cited as `PLAN §N` / `D<n>`) and
  `docs/DESIGN.md` (the Glass tokens, layout, screens, keyboard map; cited as `DESIGN §N`). Read the
  relevant sections before changing anything; when code and spec disagree, fix one of them in the
  same change.
- `legacy/` holds the first implementation: **read-only reference**. Never build it, never edit it,
  never add it to the workspace. Code comes across only through the salvage list (PLAN §15): read
  it, keep what is listed, rewrite the rest.
- Reference app for patterns: the owner's sibling GPUI app (where it lives: CLAUDE.local.md), same GPUI stack. Its code may
  be copied freely (`gpui_automation` came from there); never modify anything in that checkout.

## Toolchain and platform

- Rust 1.98.1 (pinned in `rust-toolchain.toml`), edition 2024, resolver 3.
- Windows 11 is the primary platform. Linux is built and tested inside WSL Ubuntu (below). macOS is
  kept compiling where cheap and is otherwise unverified (PLAN §13.2).
- `git` and `gh` are on `PATH`. dv shells out to both and never links a git library (PLAN D4, D7).

## Crates (`crates/`)

| Crate | Role | gpui? |
|---|---|---|
| `dv` | bin: entry for the GUI (the `gui` feature), the CLI and `dv backend --stdio`, in three shapes: default (`gui` + `automation`), the shipped app (`--no-default-features --features gui`) and headless (`--no-default-features`: no gpui, the static musl build for WSL distros, whose `dv` and `dv open` exit 2 saying to open the repo from Windows, `windowless`); one clap tree (`cli`, the headless commands flattened in from `dv_cli` and run before any gpui code), the GUI started detached and what `dv open` opens (`launch`); automation host (`automation` feature, on by default), which also seeds scenario repos with `dv_test_support` (in process, or through a loopback `dv backend --stdio` it can kill and block: `backends`) and plays a scenario's agent through its own CLI (`cli`, `cli_spawn`, `cli_join`: this executable as a child process) | with `gui` |
| `dv_ui` | views (`shell`, `switcher`, `file_list`, `comments_list`, `diff_pane`, `thread`, `submit`, …) and their keys (`keymap`, `actions`), services (`Settings`, `Locations` with the app's `Connections` and how WSL is reached, `WslAccess`; `Reviews`, `ReviewUi`), the open review (`session::ReviewSession`: its comparison, interdiff or all changes, where each thread shows, through `dv_core`'s placement, and its remote backend's connection, `session::connection`), highlighting (`syntax`, token kinds) and comment Markdown (`markdown`, sanitised), the file view (`diff_pane::file_view`: one file whole in the stream's pane, a one-file scope), the file finder (`file_finder`, `Ctrl P`) and the history of positions (`history`, `Alt ←`/`→`), code intelligence (`intel`: `CodeIntel` per review, the hover card, the References tab, and the `LanguageServers` service, one server per worktree, spec and executable, through `dv_lsp`; the shell's navigation and status bar items, `shell::intel`), the Glass theme (`theme`), window material (`appearance`, `platform`), assets, the `system` and `automation` seams (PLAN §11) | yes |
| `dv_core` | locations, the `Backend` trait and `LocalBackend` (PLAN §5.1; `probe_path`, the one read-only op for an absolute path outside the worktree, for the language servers' resolvers) with the processes a backend runs where its repo lives (`backend::process`: `spawn_stdio`, a process group or job of its own, PLAN §18.3), the job object children that die with dv join on Windows (`job`), targets and their labels (`label`), git runner, diff pipeline, one side's files whole (`diff::side_file`: a side's paths and one file's text as the pipeline decodes it), review event store, anchors, relocation and placement (`review::relocate`, a file the review doesn't change included: `place_unlisted`), a thread's round snapshot read for renames and where its code went (`review::history`), the diff around a thread's code now (`review::excerpt`), GitHub via `gh` (PLAN §6–§9) | **never** |
| `dv_backend` | the stdio protocol (`proto`: JSON lines, bytes base64, `WireError` for every `BackendError`), the `dv backend --stdio` server loop (`server`), how a server starts (`launch`: `Launcher::Command` a child process such as a loopback `dv backend --stdio`, `Launcher::Wsl` a distro's through `wsl.exe`, `Launcher::InProcess` a thread over in-memory pipes for tests), `RemoteBackend` over a `Connection` with its lifecycle (`client`: state and `subscribe`, reconnect with backoff 1/2/5 s then `Failed` until `retry`, worktrees re-opened and watches resubscribed with a synthetic rescan, remote processes ended by a death, `kill` as a test seam) and the app's `Connections` registry, build hashes (`build`), WSL (`wsl`: the distros, the `/bin/sh` scripts run through `wsl.exe` as a `Shell`, the launch with its markers) and this app's Linux builds and their install, prune and check in a distro (`install`) (PLAN §5); a test helper bin, never shipped: `dv_test_child`, the process the conformance suite spawns and its child-process loopback server | never |
| `dv_cli` | the headless commands (`args`, clap) and what they run with (`Context`: the command line, cwd, env, streams, git env, so tests run them in-process), the repository and which review a command means (PLAN §10.2), a WSL repository's command run inside its distro on Windows (`reexec`, PLAN §10.4), who it acts as, thread placement, JSON DTOs (`dto`), exit and error codes (`error`), the `dv-review` agent skill (`skill/SKILL.md`, PLAN §10.5), setup (`setup agents`, `setup wsl`) and doctor checks (PLAN §10, §12) | never |
| `dv_lsp` | language servers where the repo lives (PLAN §18.4): the LSP client (`client`: `LspClient` over a `Backend::spawn_stdio` process, Content-Length framing (`framing`), requests with timeouts and `$/cancelRequest` on drop, the server's own requests answered, progress, registered file watchers, UTF-8 columns with UTF-16 converted (`text`), worktree paths and `file:` URIs (`paths`)), `LanguageServerSpec` as data (`spec`) and the registry (`languages`: `typescript`, TypeScript 7's native `tsc --lsp --stdio`; `tailwindcss` and `css`, JavaScript servers), the `Resolver` (`resolve`: the `DV_LSP_<ID>` override, the project's own copy by Node's resolution through the backend, above the worktree root and through links that leave it (`resolve::node`, `Backend::probe_path`; npm, pnpm and bun layouts), the native CPU first (`resolve::platform`), `settings.json`'s `lsp.<id>.command` and `lsp.runtime` (`settings`), `PATH` and the home (`resolve::host`; a distro's read from its user's interactive login shell through `spawn_stdio`, `resolve::shell`), JavaScript servers' entry scripts, bun's global folder and the runtime, bun then node (`resolve::js`), `files_changed` / `resolve_fresh` for installs), the restart schedule (`restarts`) | never |
| `gpui_automation` | JSONL automation over stdio + the `e2e` runner bin (PLAN §14) | yes |
| `mock_gh` | bin: fake `gh` for tests (PLAN §14.2) | never |
| `mock_lsp` | bin: a scripted language server for tests and scenarios (`DV_LSP_<ID>` names it; scenario `--scenario` / `DV_MOCK_LSP`, log `--log` / `DV_MOCK_LSP_LOG`, PLAN §18.7) | never |
| `dv_test_support` | dev-only lib: `FixtureRepo` and shared test helpers (PLAN §14.2) | never |

Direction: `dv` → {`dv_ui`, `dv_cli`, `dv_core`}; `dv_ui` → {`dv_cli`, `dv_backend`, `dv_lsp`,
`dv_core`, `gpui-component`}; `dv_cli` → {`dv_backend`, `dv_core`}; `dv_lsp` → `dv_core`; `dv_backend` →
`dv_core`. `dv` and
`dv_ui` reach `gpui_automation` only through their `automation` features; `dv`'s also brings
`dv_test_support` (gpui-free) for the scenario repos it seeds and `dv_backend` for their
loopback, never in the shipped build. A gpui
dependency in a "never" crate, or in `dv` built without `gui`, fails `script/check` (step 3,
`cargo tree`), because those run headless: in the CLI, as the static musl backend inside WSL, and
in fast tests.

GPUI naming: `gpui` = crates.io `gpui-pre =0.3.6`, `gpui_platform` = `gpui-pre-platform =0.3.6`,
components = `gpui-component =0.6.6` and its base `gpui-base =0.6.6` (the window's text
selection, which `dv_ui` copies through the clipboard seam) (PLAN D1). Don't guess APIs: grep the sources under
`~/.cargo/registry/src/index.crates.io-*/gpui-pre-0.3.6/` and `gpui-component-0.6.6/`.

## Rules

- **Dependencies.** Declare every dependency once in the root `[workspace.dependencies]` (keep it
  alphabetical) and use `foo.workspace = true` in crates. Exact pins for the gpui family; bump them
  together, then build and run the e2e scenarios before keeping the new `Cargo.lock`.
- **Licensing.** `gpui` and `gpui-component` are Apache-2.0; learn from and build on them freely.
  Most other Zed crates (editor, project, worktree, …) are GPL/AGPL: read them for architecture,
  never copy their code here.
- **Errors.** `anyhow` in app/CLI code, `thiserror` enums in libraries. No `unwrap()`/`expect()`
  outside tests unless the message states the invariant.
- **Style.** rustfmt defaults (a hook runs `cargo fmt --all` after every edit); clippy clean with
  `-D warnings` in dv's three shapes (default, the shipped app, headless). Files stay under
  ~1,500 lines, split by responsibility
  (`mod.rs` / `render.rs` / `tests.rs` per view).
- **Comments** say what something is and why, in the present tense, citing `PLAN §N` / `DESIGN §N`
  for spec decisions. History (review findings, phase labels, "used to…") belongs in commit
  messages, never in comments.
- **The user's repo is read-only.** dv writes only `git fetch`, `.git/dv/**`, `refs/dv/**` and the
  objects those refs need; no commit, checkout, stash, reset or index change, ever (PLAN §2.4).
- **GitHub only through `gh`** (PLAN D7), with `GH_PROMPT_DISABLED=1` and the `DV_GH` override.
  Tests and scenarios point `DV_GH` at `mock_gh`; nothing automated talks to GitHub. If `gh auth
  status` fails, ask the owner to run `gh auth login`.
- **No CI.** There are deliberately no GitHub Actions workflows until PLAN §16 phase 10 (a Windows
  gpui build burns paid minutes). Don't add `.github/workflows/` and don't push expecting anything
  to check the change; verify locally with `script/check`.
- **Windows and the OS go through seams.** Every window opens through
  `dv_ui::automation::open_window`, and nothing calls `App::activate`, `Window::activate_window`,
  `Window::resize` or `Window::zoom_window` while `dv_ui::automation::is_background()` (each one
  raises and activates the window on Windows). Every browser link, reveal-in-folder, folder
  picker and clipboard write goes through `dv_ui::system` (`open_url`, `reveal`, `pick_folder`,
  `write_clipboard`), never gpui's `open_url` / `reveal_path` / `prompt_for_paths` /
  `write_to_clipboard`: under automation it records the request (`state.system`) instead of
  opening anything, so a test run never replaces the owner's clipboard.
- **Colours come from the theme.** Views read `cx.dv_theme()` (`dv_ui::theme`, DESIGN §1 tokens
  for the appearance × material in use) and never hard-code a colour. Shared sizes (DESIGN §1.5:
  control and bar heights, radii, the spacing scale, type) come from the theme too; a one-off
  pixel value copied from the design board may stay inline next to the element it sizes.
- **Live checks are the owner's.** `DV_LIVE_WSL=1` and `DV_LIVE_GH=1` (PLAN §14.3) touch a real
  distro's home and the `kylekz/difftest` repo; no agent, script or scenario sets them.
- **The owner's WSL home is off limits.** Nothing an agent, a test or a script runs installs
  into, writes or deletes anything under the distro home's `~/.local/bin/dv`,
  `~/.local/share/dv/**` or `~/.claude` / `~/.agents` / `~/.codex` (the first implementation's
  install lives there). Every real `wsl.exe` run that installs or launches dv goes through the
  sandbox seam `DV_WSL_HOME` (beside `DV_GH`): a POSIX path, a fresh folder under the distro's
  `/tmp`, which every launch, install, prune and check then uses as `$HOME` (`… --exec
  /usr/bin/env HOME=<it> /bin/sh -c …`, PLAN §5.2), removed afterwards. One check decides what
  its value means (`dv_backend::wsl::sandbox`): set to anything but a plain absolute POSIX path
  naming a folder other than `/` (blank, whitespace, `..` parts and `/.` included) it is an
  error, never the real home, and no sandbox for an automation run. Tests that hand the CLI a
  stand-in Linux build to check a guard name a distro that can't exist (`dv-no-such-distro`),
  so a guard that regresses reaches no real distro. Never `wsl --shutdown`,
  `-t`, `--unregister`, `--install` or `--set-default`, never `sudo`/`apt` in the distro, and
  never run anything in a distro other than `Ubuntu`, even one already running (Docker
  Desktop's `docker-desktop` is; `--wsl docker-desktop:…` crosses into it). The app holds to this under automation itself: an
  `--automation` run without `DV_WSL_HOME` naming a scratch home reaches WSL only through a
  stand-in that lists the distros a scenario names (`wsl_distros`) and refuses to start dv in
  any (the e2e runner drops an inherited `DV_WSL_HOME`; only a scenario header's `env` sets one,
  which dv checks as above), and `dv_ui`'s tests can't reach the real WSL at all (`WslAccess`,
  PLAN §5.2). The app itself never boots a distro the owner stopped: only an explicit open does
  (PLAN §5.2's connecting rule).

## Commands

```bash
script/check                 # every gate (PLAN §13.2): fmt, clippy x3, gpui boundary, tests, e2e
script/check --fast          # the same without the e2e scenarios
script/check --foreground    # also tests/e2e/foreground: takes the focus for a few seconds
script/check --linux         # also step 6: the headless crates and dv tested and linted in WSL
                             # Ubuntu, script/build-linux --debug-only, the real WSL sandboxed under /tmp
script/build-linux [--aarch64] [--debug-only]   # the static musl dv for WSL:
                             # target/{debug,release}/dv-linux-x86_64 (--debug-only: debug's alone)

cargo build                  # the dv binary (default member)
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p dv --no-default-features --features gui --all-targets -- -D warnings   # the shipped app
cargo clippy -p dv --no-default-features --all-targets -- -D warnings   # headless (the WSL build)
cargo test -p dv --no-default-features   # headless tests; relinks target/debug/dv without the app
cargo test -p dv --test backend_loopback # the built `dv backend --stdio` as a child: the
                             # handshake, a kill and the reconnect, Failed and retry, no leaked server
cargo fmt --all
cargo build -p dv -p mock_gh -p mock_lsp                           # the runner starts these; it doesn't build them
cargo run -p gpui_automation --bin e2e -- tests/e2e    # background run; output in target/e2e/<scenario>/
cargo test -p gpui_automation --test harness_e2e -- --ignored
                             # the automation library's own e2e proof against its example harness
DV_TEST_TSC=<TypeScript 7's lib/tsc> cargo test -p dv_lsp --test real_tsc
                             # the real TypeScript check (PLAN §18.7): read-only, a temp fixture;
                             # in WSL, a project's node_modules/.bun/@typescript+typescript-linux-x64@*/…/lib/tsc
DV_TEST_JS_RUNTIME=<bun or node> DV_TEST_TAILWIND=<its bin script> cargo test -p dv_lsp --test real_js
                             # the same for Tailwind CSS (DV_TEST_CSS: the CSS server's)
```

`dv` has three shapes (PLAN §13.1): the default one (`gui` + `automation`) for development and
the e2e runner, the app as it ships (`--no-default-features --features gui`, automation off) and
the headless build (`--no-default-features`: the CLI and `dv backend --stdio`, no gpui). Each
drifts silently if only the default shape is linted, so `script/check` lints all three. Unit
tests in `dv` that need gpui are gated on `gui`; `crates/dv/tests/*` run in every shape.

Run `cargo build -p dv -p mock_gh -p mock_lsp` right before the e2e runner: a `cargo test` build relinks the
binaries with gpui's test support, and a headless build (`--no-default-features`) relinks `dv`
without the app, neither of which the runner may start. `script/check` does this itself.

### The dv CLI

```bash
dv status --json             # the repo, who you act as, the open reviews and the current one
dv review list [--all]       # dv review show [<id>], dv comment list [--status open] [--file p]
dv comment show <thread>     # --json on any command: one {"schema": 1, …} document on stdout
dv comment reply <thread> --body-file -    # also add, address, resolve, reopen (PLAN §10.1)
dv review new | done | submit | close | delete | wait     # wait: exit 3 on --timeout
dv setup agents --home <dir> --yes         # the dv-review skill; never without --home in tests
dv setup wsl <distro> --yes  # Windows: this build into the distro; only with DV_WSL_HOME=/tmp/… here
dv --wsl Ubuntu:/tmp/x status # Windows: runs inside the distro (installs dv there first); the same rule
dv open <path>               # the GUI, detached; `dv` alone opens the working directory's repo
```

Every command but `dv` and `dv open` is headless; its exit codes are PLAN §10.3's (2 usage or
an unclear review, 3 a wait's timeout, 4 a WSL distro dv can't reach). On Windows a command for
a WSL repository (`--wsl`, `--repo \\wsl.localhost\…`, or a working directory there) runs
inside the distro as this build, installing it into the distro home first when it is missing
(PLAN §10.4): from this shell that is the owner's real home, so run such a command only with
`DV_WSL_HOME=/tmp/…` (and `MSYS_NO_PATHCONV=1`, WSL below). Unless stdout and stderr are both
a terminal, the crossing copies the distro's output into dv's own streams (`wsl.exe` would
write the two over each other in one file, as this shell captures them), so `--json` output is
whole here. The CLI's tests can't cross by
accident: `dv_cli`'s `Fixture` and `crates/dv/tests/cli.rs` give every command a
`DV_LINUX_DIR` without a Linux build, and the automation host's `cli` children get one unless
the app is sandboxed. Test commands in-process (`dv_cli::main_with` with a `Context`, as
`crates/dv_cli/src/tests/` does), or spawn the built binary with a headless subcommand only:
`dv` alone or `dv open` opens a window on the owner's desktop, so no test runs them (the
startup decisions are pure functions in `crates/dv/src/launch.rs`). `dv setup agents`
writes into the home directory: tests pass the hidden `--home <temp dir>`, and nothing runs it
against the real home (`~/.claude`, `~/.agents`, or `~/.codex` / `$CODEX_HOME`, which it reads
to see whether Codex is installed). `dv setup wsl` writes into a distro's home: tests set
`DV_WSL_HOME` to a folder under its `/tmp` (WSL below). Every command line in
`crates/dv_cli/skill/SKILL.md` is run by `crates/dv_cli/src/tests/skill.rs`; change both
together.

This shell runs inside Claude Code, which exports `CLAUDECODE=1`: any `dv` command run from it
acts as the agent `claude-code` (PLAN §10.1). Run the CLI as the reviewer with
`env -u CLAUDECODE dv …`, and have every test and scenario that runs it remove `CLAUDECODE`,
`DV_AUTHOR` and `DV_REVIEW` from the child's environment, setting them only on purpose. A
scenario plays the agent with the automation host's `cli` / `cli_spawn` / `cli_join`, which do
this themselves (`"env": {"CLAUDECODE": "1"}` makes the agent Claude Code;
`tests/e2e/workflow_a_agent.jsonl` is PLAN §3 workflow A end to end that way).

## GUI runs

App windows open on the owner's real desktop.

- Run the app only through the e2e runner (background mode, the default: windows never take focus
  and stay behind everything) or `--automation` scripts that end with `quit`. Never leave `dv.exe`
  or `mock_gh.exe` running; if a run fails, kill what you started (`taskkill //F //IM dv.exe` from
  bash). Scenarios that must take the focus (`tests/e2e/foreground/`) run only on demand.
- Background screenshots show the window inactive and miss Acrylic's blur (translucent surfaces
  come out over black); judge Acrylic and Mica's active tint from the foreground scenario's screen
  captures (tests/e2e/README.md, "Screenshots and window materials").
- Never point the app at the real data dir. `%APPDATA%\dv` holds the first implementation's files.
  Every run uses `--data-dir` / `DV_DATA_DIR` under `target/` (the runner gives each scenario its
  own); under `--automation` without one, dv uses a fresh temp dir (removed when it exits), never
  the real one. dv reads that folder's `settings.json` leniently and keeps keys it doesn't know,
  because the first implementation's file has the same name; dv's repo list is
  `known_repos.json`, never the first implementation's `repos.json`, and its per-review choices
  (split or unified) are `review_ui.json`.
- Verify UI by looking: read the PNGs a run writes and inspect them against DESIGN.md. `state` is
  the assertion surface; screenshots are for style. The dev display runs at 150% scaling, so
  screenshots are physical pixels (a 1440×900 window captures as ~2160×1350).
- Exploratory runs (looking at something no scenario covers yet) are scratch scenarios outside
  `tests/e2e`, e.g. `target/tmp/explore.jsonl`, run with the runner and their own `--out`.
  `tests/e2e/README.md` documents the protocol, the scenario format, the runner and dv's `state`.
  The connection states (reconnecting, failed, the banner) are the loopback's in a scenario
  (`seed_repo` with `"backend": "loopback"`, `backend_kill`, `backend_block`); a real WSL repo
  only in an exploratory run (WSL below).

## WSL

- Build and test Linux code inside Ubuntu, never on Windows paths through `/mnt/d` for the target
  dir: 9P makes the build itself punishingly slow. Keep the target dir on the distro's ext4 disk:

  ```bash
  wsl.exe -d Ubuntu --cd 'D:\Software\diff' --exec bash -lc \
    'CARGO_TARGET_DIR=$HOME/.cache/dv-target cargo test -p dv_core'
  ```

  `script/check --linux` runs the headless crates' and the headless `dv`'s tests and clippy this
  way. rustup installs the pinned toolchain inside the distro on first use. Run inside a distro,
  `script/check --linux` and `script/build-linux` work there directly (they test for Linux
  first) and skip the real-WSL step, which needs Windows.
- A WSL repo always goes through the Linux `dv`: in the app as `dv backend --stdio` in the
  distro, one `wsl.exe` child per distro, with no per-command `wsl.exe` fallback (PLAN D6, §5);
  in the CLI on Windows as the whole command run there, one `wsl.exe` for it (PLAN §10.4).
- **The Linux build** is `script/build-linux` (`--aarch64` adds the arm build): the headless `dv`
  for `x86_64-unknown-linux-musl`, built in Ubuntu, copied beside the Windows builds as
  `target/debug/dv-linux-x86_64` and `target/release/dv-linux-x86_64` (`--debug-only`, as
  `script/check --linux` runs it, leaves the release copy, a release app's, alone). The app and `dv setup
  wsl` find it beside their executable, or in `DV_LINUX_DIR` (a Windows folder), and install it
  into a distro by its sha256 (PLAN §5.2, §13.2).
- **The real-WSL tests** run only when `DV_TEST_WSL_DISTRO` names a distro, and then only in
  scratch homes under `DV_WSL_HOME`, which must be a folder under the distro's `/tmp` (anything
  else fails the test rather than touch a real home); `script/check --linux` makes a fresh one
  and runs them after `script/build-linux`. By hand, from Git Bash (`MSYS_NO_PATHCONV=1` keeps it
  from rewriting `/tmp/…` into a Windows path, which the tests refuse):

  ```bash
  script/build-linux
  MSYS_NO_PATHCONV=1 DV_TEST_WSL_DISTRO=Ubuntu DV_WSL_HOME=/tmp/dv-check-$$ \
    cargo test -p dv_backend --test wsl -- --test-threads=1
  # the same for `dv setup wsl` and the CLI run inside the distro:
  #   cargo test -p dv_cli --lib -- setup_wsl reexec --test-threads=1
  #   cargo test -p dv --test wsl_cli -- --test-threads=1
  #   cargo test -p dv_lsp --test wsl_shell -- --test-threads=1
  ```

  Each test removes its own folder; remove the empty `DV_WSL_HOME` with `wsl.exe -d Ubuntu
  --exec rmdir /tmp/dv-check-…`. Exploring the app against WSL: after `script/build-linux`, a
  scratch scenario whose header sets `"env": {"DV_WSL_HOME": "/tmp/dv-explore-home"}` (a fresh
  folder; the header's env reaches the app without Git Bash rewriting it) and opens a scratch
  repo made under the distro's `/tmp` (`\\wsl.localhost\Ubuntu\tmp\…`); remove both folders
  inside the distro afterwards.
- **The owner's live check** (`crates/dv_backend/tests/live_wsl.rs`, PLAN §14.3) installs into
  the real home: `DV_LIVE_WSL=1 cargo test -p dv_backend --test live_wsl -- --ignored
  --test-threads=1` after `cargo build -p dv` and `script/build-linux` (it ends with
  `target/debug/dv.exe` running a command inside the distro), `DV_LIVE_WSL_DISTRO` naming the
  distro (default `Ubuntu`). The owner runs it; no agent does.

## Disk

D: holds the checkout and every target dir. Check `df -h /d` before anything big (a fresh build,
copying a target dir) and stop if D: would drop below 30 GB free. `target/debug/incremental` is the
first thing to delete when space runs low. Never `cargo clean` without the owner's say-so (gpui
takes many minutes to rebuild).
