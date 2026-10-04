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
| `dv` | bin: entry for the GUI, the CLI and (Linux) `dv backend --stdio`; automation host (`automation` feature, on by default) | yes |
| `dv_ui` | views (`shell`, …) and services (`services::Settings`), the Glass theme (`theme`), window material (`appearance`, `platform`), assets, the `system` and `automation` seams (PLAN §11) | yes |
| `dv_core` | locations, targets, git runner, diff pipeline, review event store, anchors, GitHub via `gh` (PLAN §6–§9) | **never** |
| `dv_backend` | the `Backend` trait, `LocalBackend`, the stdio protocol, `RemoteBackend`, distro install (PLAN §5) | never |
| `dv_cli` | clap command tree, JSON DTOs, setup/doctor checks (PLAN §10, §12) | never |
| `gpui_automation` | JSONL automation over stdio + the `e2e` runner bin (PLAN §14) | yes |
| `mock_gh` | bin: fake `gh` for tests (PLAN §14.2) | never |
| `dv_test_support` | dev-only lib: `FixtureRepo` and shared test helpers (PLAN §14.2) | never |

Direction: `dv` → `dv_ui` → {`dv_cli`, `dv_backend`, `dv_core`, `gpui-component`};
`dv_cli` → {`dv_backend`, `dv_core`}; `dv_backend` → `dv_core`. `dv` and `dv_ui` reach
`gpui_automation` only through their `automation` features. A gpui dependency in a "never" crate
fails `script/check` (step 3, `cargo tree`), because those crates run headless: in the CLI, as the
static musl backend inside WSL, and in fast tests.

GPUI naming: `gpui` = crates.io `gpui-pre =0.3.6`, `gpui_platform` = `gpui-pre-platform =0.3.6`,
components = `gpui-component =0.6.6` (PLAN D1). Don't guess APIs: grep the sources under
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
  `-D warnings` in both feature shapes. Files stay under ~1,500 lines, split by responsibility
  (`mod.rs` / `render.rs` / `tests.rs` per view).
- **Comments** say what something is and why, in the present tense, citing `PLAN §N` / `DESIGN §N`
  for spec decisions. History (review findings, phase labels, "used to…") belongs in commit
  messages, never in comments.
- **The user's repo is read-only.** dv writes only `git fetch`, `.git/dv/**`, `refs/dv/**` and the
  objects those refs need; no commit, checkout, stash, reset or index change, ever (PLAN §2.4).
- **GitHub only through `gh`** (PLAN D7), with `GH_PROMPT_DISABLED=1` and the `DV_GH` override.
  Tests and scenarios point `DV_GH` at `mock_gh`; nothing automated talks to GitHub. If `gh auth
  status` fails, ask the owner to run `gh auth login`.
- **No CI.** There are deliberately no GitHub Actions workflows until PLAN §16 phase 9 (a Windows
  gpui build burns paid minutes). Don't add `.github/workflows/` and don't push expecting anything
  to check the change; verify locally with `script/check`.
- **Windows and the OS go through seams.** Every window opens through
  `dv_ui::automation::open_window`, and nothing calls `App::activate`, `Window::activate_window`,
  `Window::resize` or `Window::zoom_window` while `dv_ui::automation::is_background()` (each one
  raises and activates the window on Windows). Every browser link, reveal-in-folder and folder
  picker goes through `dv_ui::system` (`open_url`, `reveal`, `pick_folder`), never gpui's
  `open_url` / `reveal_path` / `prompt_for_paths`: under automation it records the request
  (`state.system`) instead of opening anything.
- **Colours come from the theme.** Views read `cx.dv_theme()` (`dv_ui::theme`, DESIGN §1 tokens
  for the appearance × material in use) and never hard-code a colour. Shared sizes (DESIGN §1.5:
  control and bar heights, radii, the spacing scale, type) come from the theme too; a one-off
  pixel value copied from the design board may stay inline next to the element it sizes.
- **Live checks are the owner's.** `DV_LIVE_WSL=1` and `DV_LIVE_GH=1` (PLAN §14.3) touch a real
  distro and the `kylekz/difftest` repo; no agent, script or scenario sets them.

## Commands

```bash
script/check                 # every gate (PLAN §13.2): fmt, clippy x2, gpui boundary, tests, e2e
script/check --fast          # the same without the e2e scenarios
script/check --foreground    # also tests/e2e/foreground: takes the focus for a few seconds
script/check --linux         # also the headless crates' tests inside WSL Ubuntu

cargo build                  # the dv binary (default member)
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p dv --no-default-features --all-targets -- -D warnings   # the shipped shape
cargo fmt --all
cargo build -p dv -p mock_gh                           # the runner starts these; it doesn't build them
cargo run -p gpui_automation --bin e2e -- tests/e2e    # background run; output in target/e2e/<scenario>/
cargo test -p gpui_automation --test harness_e2e -- --ignored
                             # the automation library's own e2e proof against its example harness
```

The clippy run without default features is the build that ships (automation off). It drifts
silently if only the default shape is linted, so `script/check` runs both.

Run `cargo build -p dv -p mock_gh` right before the e2e runner: a `cargo test` build relinks the
binaries with gpui's test support, which the runner must not start. `script/check` does this itself.

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
  because the first implementation's file has the same name.
- Verify UI by looking: read the PNGs a run writes and inspect them against DESIGN.md. `state` is
  the assertion surface; screenshots are for style. The dev display runs at 150% scaling, so
  screenshots are physical pixels (a 1440×900 window captures as ~2160×1350).
- Exploratory runs (looking at something no scenario covers yet) are scratch scenarios outside
  `tests/e2e`, e.g. `target/tmp/explore.jsonl`, run with the runner and their own `--out`.
  `tests/e2e/README.md` documents the protocol, the scenario format, the runner and dv's `state`.

## WSL

- Build and test Linux code inside Ubuntu, never on Windows paths through `/mnt/d` for the target
  dir: 9P makes the build itself punishingly slow. Keep the target dir on the distro's ext4 disk:

  ```bash
  wsl.exe -d Ubuntu --cd 'D:\Software\diff' --exec bash -lc \
    'CARGO_TARGET_DIR=$HOME/.cache/dv-target cargo test -p dv_core'
  ```

  `script/check --linux` runs the headless crates' tests this way. rustup installs the pinned
  toolchain inside the distro on first use.
- A WSL repo always goes through the Linux `dv` running as `dv backend --stdio` in the distro, one
  `wsl.exe` child per distro; there is no per-command `wsl.exe` fallback (PLAN D6, §5).

## Disk

D: holds the checkout and every target dir. Check `df -h /d` before anything big (a fresh build,
copying a target dir) and stop if D: would drop below 30 GB free. `target/debug/incremental` is the
first thing to delete when space runs low. Never `cargo clean` without the owner's say-so (gpui
takes many minutes to rebuild).
