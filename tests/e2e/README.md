# End-to-end scenarios

Each `*.jsonl` file here is a scenario: a list of automation commands run
against a freshly started `dv`, with assertions on the app's `state` JSON
(docs/PLAN.md §14). The runner is `crates/gpui_automation/src/bin/e2e`, the
protocol lives in `crates/gpui_automation`.

```bash
cargo build -p dv -p mock_gh
cargo run -p gpui_automation --bin e2e -- tests/e2e
cargo run -p gpui_automation --bin e2e -- --filter shell tests/e2e/*.jsonl
cargo run -p gpui_automation --bin e2e -- tests/e2e/foreground
```

`tests/e2e/foreground/` holds the scenarios that take the foreground (their
header says `"foreground": true`): the runner doesn't look into
subdirectories, so `tests/e2e` and `script/check` leave them out, and they run
on demand (the last command) or with `script/check --foreground`. Keep hands
off the machine while they run; see "Screenshots and window materials".

Run that `cargo build` right before a run (`script/check` step 5 does). A
build that compiles `dv`'s bin alongside test targets with unified features
(`cargo build --workspace --all-targets`, or `cargo test` once `crates/dv` has
integration tests) relinks `target/debug/dv.exe` with gpui's `test-support`,
which includes leak detection: that binary panics at exit when an entity
outlives the App, so a scenario can exit 101 after `quit` against it and pass
against a plain one.

The library's own proof runs against its example harness
(`crates/gpui_automation/examples/harness.rs`): `harness.jsonl` covers every
command, restarts and the runner's hermetic environment;
`harness_background.jsonl` (background runs only) a window asked for maximized
bounds opening windowed and inactive; `harness_gh.jsonl` the `mock_gh` wiring
(a `gh` header, `gh_log` lines, the log across a restart); and a second test
drives the harness over the named pipe. The tests build the harness (and
`mock_gh`) first, so they never run stale binaries:

```bash
cargo test -p gpui_automation --test harness_e2e -- --ignored
# or by hand:
cargo build -p gpui_automation --example harness --bin e2e
cargo build -p mock_gh
cargo run -p gpui_automation --bin e2e -- --app target/debug/examples/harness.exe \
    crates/gpui_automation/tests/harness*.jsonl
```

Windows open on the real desktop, but by default in the background (next
section), so runs can go on while someone uses the machine. Apps that render
`gpui_automation::pointer_guard()` (dv's shell and the harness do) drop the
real mouse's events while automation runs, so a cursor crossing the window
can't bend a scripted drag. Keyboard input is not filtered, and the real
cursor still moves gpui's hover position until the next scripted pointer
event, so it is still best to leave the windows alone.

### Background runs

The runner starts the app with `--automation-background` unless it is given
`--foreground`. An app that supports the flag (dv, the harness) calls
`gpui_automation::set_background(true)` and opens **every** window through
`gpui_automation::open_window` (dv: `dv_ui::automation::open_window`), which
in background mode:

- opens the window without activating it (`WindowOptions::focus` off), so it
  never takes the keyboard focus or the foreground;
- on Windows, moves it to the bottom of the z-order without activating it
  (`SetWindowPos(HWND_BOTTOM, SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE)`)
  before its first frame, and makes it a tool window that can't be activated
  (`WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`) without a system menu (with
  `WS_SYSMENU`, DWM draws a tool window's small caption "x" at its top-right,
  which shows through a Mica or Acrylic title bar). It has no taskbar button,
  isn't in Alt+Tab, and Windows never hands it the foreground when the person
  minimises or closes their own window. The window is also subclassed to
  answer `WM_MOUSEACTIVATE` with `MA_NOACTIVATE`, because gpui itself answers
  `MA_ACTIVATE`, so a real click doesn't activate it either. It stays on
  screen, behind everything else: DirectComposition keeps rendering an
  occluded window and `PrintWindow(PW_RENDERFULLCONTENT)` still captures it,
  which a minimised or off-screen window wouldn't allow;
- opens a window asked for `WindowBounds::Maximized` windowed at its restore
  bounds (maximizing activates the window on Windows);
- renders at full rate although it is inactive (gpui throttles inactive
  windows to ~30 fps otherwise, which would slow every step).

The app must not activate or raise anything itself in this mode:
`App::activate` and `Window::activate_window` (gpui's Windows
`activate_window` even injects a synthetic Alt press), `Window::resize` (its
`SetWindowPos` lacks `SWP_NOACTIVATE`/`SWP_NOZORDER`, so it raises and
activates the window; open windows at the size they need, and scenarios use
the `resize` command), and `Window::zoom_window` (`SW_MAXIMIZE` activates).
Fullscreen is fine. Scripted input never needs the OS focus: the driver
dispatches events into the gpui window directly and gpui keeps keyboard focus
per window, so clicks, drags, keys, typing into inputs, file drops,
screenshots, resizing, fullscreen and second windows all work (`harness.jsonl`
proves each in background mode), and screenshots match a foreground run pixel
for pixel apart from text carets (below).

**What differs**: gpui considers a background window inactive
(`Window::is_window_active()` is `false`, and so is `state.window.active`;
`state.window.background` and the `ready` event's `background` are `true`).
Everything gpui ties to activation therefore stays off:

- window focus events: `on_focus`/`on_blur`/`on_focus_in`/`on_focus_out`
  listeners and gpui-component's `InputEvent::Focus`/`Blur` don't fire, while
  `FocusHandle::is_focused` and focus-scoped key dispatch work as usual. Don't
  make a scenario depend on blur (commit an edit with Enter, not by clicking
  away), or give it a foreground header;
- gpui-component inputs don't paint their caret or selection, so a
  screenshot of a focused input lacks the caret;
- `observe_window_activation` callbacks never run.

Faking the activation isn't possible: a posted `WM_ACTIVATE` reaches
`DefWindowProc`, which focuses and so really activates the window, and a
process the runner started may take the foreground (the watch below caught
exactly that). A scenario that needs an active window sets
`"foreground": true` in its header; the runner then starts that one app
normally.

On Linux and macOS only the unfocused, unthrottled part applies.

The runner holds apps to it: on Windows it records the foreground window
before starting each scenario and hooks foreground changes
(`EVENT_SYSTEM_FOREGROUND`) for the whole run. If a window of the scenario's
app process ever becomes the foreground window, the step fails with
"the app took the foreground during a background run". Foreground changes
between other processes (the person at the machine switching windows) are
only noted in `transcript.log` as `# foreground changed to ...`.

`--foreground` runs every scenario the old way (focused windows, in front),
for watching a run.

### Screenshots and window materials

A `screenshot` takes its pixels from one of two sources (`"source"`):

- **`window`** (the default): the window's own composed frame,
  `PrintWindow(PW_RENDERFULLCONTENT)` on Windows. It works behind other
  windows, so background runs use it. It includes DWM's Mica backdrop (a
  foreground capture matched the screen pixel for pixel) but **not** the
  blur-behind gpui uses for Acrylic (`WindowBackgroundAppearance::Blurred`):
  translucent surfaces over Acrylic come out composited over black, so a dark
  Acrylic panel captures nearly black and a light one muddy grey. Opaque is
  exact.
- **`screen`**: the window's client rect copied from the desktop as shown,
  blur included (Windows only). It needs the window in front, so it is
  refused in background mode, and fails rather than copying another window's
  pixels when the window isn't the foreground window, is covered at any of 9
  sample points, or runs off the screen.

What a background run shows is therefore the window **inactive**: Mica as its
flat inactive fill (light or dark following dv's mode), which is DWM's own
inactive Mica, as it would be on screen (DESIGN §3: the OS manages the
inactive look), and Acrylic over black, which is not how it looks anywhere. Judge layout, type and Opaque from
background screenshots; judge Mica's active tint and Acrylic only from
`shell_materials_foreground`'s screen captures, taken with the window active
(Acrylic's blur samples whatever is behind the window at that moment, so
those captures vary from run to run). The trade-off of a foreground scenario
is the person's machine: it takes the focus for a few seconds, keystrokes
typed meanwhile reach dv, and a click elsewhere fails its screen captures.
That is why it lives in `tests/e2e/foreground/` and runs only on demand.

### dv's scenarios

| Scenario | Covers |
|---|---|
| `shell_layout` | the empty shell's regions (title bar, panel, main area, status bar and their parts) at DESIGN §1.5/§2 sizes at 1440x900, at the 1100x700 minimum after a `resize`, and a smaller `--window-size` raised to the minimum |
| `shell_caption_buttons` | Windows only: the caption buttons' bounds |
| `shell_materials` | Windows only: every material × mode through `set_setting`, the resolved values, a screenshot of each (`NN-shell-<material>-<mode>.png`, window source), and the last choice surviving a restart. Needs a machine that can show all three materials (Windows 11 22H2+, transparency effects on); its first assertion says so when it can't |
| `foreground/shell_materials_foreground` | Windows only, foreground, on demand: the same six combinations as screen captures (`NN-shell-<material>-<mode>-screen.png`) |

### dv's `state`

`state` is the object dv's automation host (`crates/dv/src/automation.rs`,
the `dv` binary's `automation` feature) returns, plus the library's `window`
and `windows` keys (Protocol, below). Its keys are listed here as they land;
assert on them rather than on screenshots, which are for style.

| Key | Contents |
|---|---|
| `app` | `"dv"` |
| `data_dir` | the data dir this run uses (absolute) |
| `settings.material`, `settings.mode` | the settings as set: `"mica"` / `"acrylic"` / `"opaque"`, and `"system"` / `"dark"` / `"light"` |
| `settings.resolved_material`, `settings.resolved_mode` | what is on screen: the material the platform can show (an unsupported one resolves to `"opaque"`, PLAN D8) and `"dark"` / `"light"` (`system` follows the OS) |
| `settings.available_materials` | the materials this machine can show now, e.g. `["mica", "acrylic", "opaque"]`; `["opaque"]` on Windows 10, with transparency effects off, and on Linux |
| `settings.path` | the settings file, `<data_dir>/settings.json` |
| `settings.save_error` | why the last save failed, or `null` |
| `shell.panel_visible` | whether the left panel shows |
| `shell.empty_state` | the main area's empty state (DESIGN §4.4): `"no_review"`, or `null` |
| `shell.window_material` | the material last applied to the window (it follows `resolved_material` before the reply to the command that changed it) |
| `system.requests` | what the app asked the OS to open, oldest first, recorded instead of performed: `{"kind": "open_url", "url"}`, `{"kind": "reveal", "path"}`, `{"kind": "pick_folder", "prompt"}` |
| `system.answers_left` | folder-picker answers still queued |

`settle` waits for the settings to be written as well as for a frame.

dv's own commands:

| Command | Arguments | Result |
|---|---|---|
| `set_setting` | `key`: `"material"` or `"mode"`; `value`: one of that key's values above | `{settings}` (as in `state`); the change goes through the settings service as the UI's would: it is saved to `<data_dir>/settings.json` and the window has repainted in the new theme and material before the reply |

Named targets, at the default 1440x900: `titlebar` (0,0 1440x40), with
`titlebar/mark`, `titlebar/switcher`, `titlebar/command` (300 wide, centred),
`titlebar/min`, `titlebar/max`, `titlebar/close` (46 wide and 40 high less
the title bar's one-device-pixel bottom hairline, Windows only);
`shell/panel` (0,40 280x832) with `shell/panel/header`; `shell/main`
(280,40 1160x832) with `shell/main/empty`; `statusbar` (0,872 1440x28) with
`statusbar/repo` and `statusbar/material`.

## Runner

```
e2e [options] <scenario.jsonl | directory | glob>...
  --app <exe>             default <target>/debug/dv.exe
  --mock-gh <exe>         default <target>/debug/mock_gh.exe
  --filter <text>         only scenarios whose file name contains <text>
  --out <dir>             default <target>/e2e; one run at a time per dir
  --timeout-ms <n>        base reply timeout per command (default 30000)
  --ready-timeout-ms <n>  how long the app may take to start (default 60000)
  --keep-data             keep each scenario's data dir after it passes
  --foreground            focused windows in front instead of background runs
  -v, --verbose           print every request and reply
```

`<target>` is `$CARGO_TARGET_DIR` or the workspace's `target/`; off Windows
the defaults have no `.exe`. Wildcards (`tests/e2e/*.jsonl`) are expanded by
the runner itself, so they work from PowerShell and cmd too; a directory
means every `*.jsonl` in it. The runner never builds anything.

A run holds an OS lock on `<out>/.e2e.lock` for its whole duration. The lock
is released when the process ends, however it ends. A second runner given the
same `--out` fails at once with `another e2e run is using <out> ...` instead
of clearing the first run's scenario dirs, so give concurrent runs different
`--out` dirs inside `target/`. `harness_e2e` does this itself:
`<target>/e2e-harness-<pid>`, removed when it passes and kept when it fails.

Per scenario the runner:

1. Clears `<out>/<scenario>/` and creates its data dir,
   `<out>/<scenario>/data`. Test data never goes to the system temp dir
   (`%TEMP%` is on C:); the next run of the scenario wipes it.
2. If the header has a `gh` file, checks that it and `mock_gh` exist.
3. Starts the app as
   `<app> --automation --automation-background --data-dir <out>/<scenario>/data --window-size WxH [app_args...]`
   (without `--automation-background` under `--foreground`),
   hermetically whatever the runner's own environment holds (PLAN §14.1):
   - `DV_DATA_DIR` is always the same data dir, so even a launch without
     `--data-dir` (a `restart` with `"data_dir": false`) never reaches the
     owner's `%APPDATA%\dv`, which holds the first implementation's files;
   - `DV_GH` is always `mock_gh` (PLAN D7), so no scenario can reach GitHub
     through the real `gh`, whether or not it scripts any answers;
   - `DV_MOCK_GH` is the header's `gh` file as an absolute path, and is
     removed from the environment without one (then `mock_gh` has no entries
     and every call fails with its exit code 64);
   - `DV_MOCK_GH_LOG` is always `<out>/<scenario>/gh_calls.jsonl`, where
     `mock_gh` appends one line per call; every launch of the scenario shares
     it;
   - the working directory is the data dir, not the repo root, so relative
     paths in commands resolve there (use `${workspace}` for repo files),
     except a screenshot's `path`, which the runner resolves first (see
     "Scenario format");
   - `GIT_CEILING_DIRECTORIES` is `<out>/<scenario>`: the data dir sits
     inside this checkout's `target/`, and a repository lookup from the
     working directory must never find dv's own checkout;
   - the header's `env`, plus `E2E_OUT_DIR` / `E2E_DATA_DIR`. A header can't
     set `DV_DATA_DIR`, `DV_GH`, `DV_MOCK_GH`, `DV_MOCK_GH_LOG`,
     `GIT_CEILING_DIRECTORIES`, `E2E_OUT_DIR` or `E2E_DATA_DIR`.
4. Waits for the app's `{"event": "ready"}` line, then streams the steps,
   waiting for each reply before sending the next line.
5. Ends with `quit` (sent for you if the scenario doesn't), requires a
   successful reply and a clean exit (status 0 within 15 s) either way.

On failure it saves `failure.png`, prints the failing line, the reason and
the tail of the app's stderr, kills the app, keeps the data dir and moves on
to the next scenario. The exit code is 0 when everything passed, 1 when a
scenario failed, 2 for usage errors and 130 when interrupted. Ctrl-C stops
the current scenario and kills its processes; on Windows every child also
sits in a kill-on-close job object (which the `mock_gh` processes it starts
inherit), so even a runner that is killed outright takes them with it.

Outputs in `<out>/<scenario>/`:

| File | Contents |
|---|---|
| `NN-<name>.png` | screenshots in scenario order (`01-start.png`, ...), physical pixels |
| `failure.png` | the window at the moment a step failed |
| `transcript.log` | every line sent (`>`) and received (`<`), `gh_log` reads, exits and restarts (`#`), timestamped |
| `app.stderr.log` | the app's log; after a restart it goes to `app.2.stderr.log`, `app.3.stderr.log`, ... |
| `gh_calls.jsonl` | `mock_gh`'s call log (`DV_MOCK_GH_LOG`), once anything has called `gh` |
| `gh_calls.jsonl.lock` | empty; `mock_gh` locks it while it counts and logs a call |
| `data/` | the app's data dir (`--data-dir`, `DV_DATA_DIR`, its working directory): removed once the scenario passes unless `--keep-data`, kept after a failure |

## Scenario format

One JSON object per line. Blank lines and lines starting with `//` or `#`
are ignored. The first line may be a header:

```json
{"scenario": {"window_size": "1440x900", "gh": "../fixtures/gh/pr_482.json", "app_args": [], "env": {"RUST_LOG": "info"}, "platforms": ["windows"], "foreground": false, "description": "..."}}
```

All header fields are optional (`window_size` defaults to `1440x900`, dv's
default window). `gh` names a `mock_gh` scenario file, relative to the
scenario file (below). `foreground: true` starts this scenario's app in the
foreground even in a background run (see "Background runs"). `platforms`
limits the scenario to those operating systems (`windows`, `linux`, `macos`;
empty or absent means all); elsewhere the runner reports it as skipped. Use
it for platform-specific chrome, and keep a sibling scenario for the other
platforms rather than dropping their coverage. Unknown fields are errors.

Every other line is a command, an assertion, or both:

```json
{"cmd": "click", "target": "file_list/src/routes/users.ts"}
{"cmd": "type", "text": "Rename this", "expect": {"pointer": "/composer/text", "equals": "Rename this"}}
{"expect": [{"pointer": "/review/threads", "len": 1}, {"pointer": "/review/status", "equals": "open"}]}
{"cmd": "click", "target": "missing", "expect_error": "no target"}
{"cmd": "query", "prefix": "file_list/", "expect_result": {"pointer": "", "len": 7}}
{"cmd": "screenshot", "name": "empty-shell"}
```

Runner-only keys (never sent to the app):

| Key | Meaning |
|---|---|
| `expect` | expectation(s) checked against a fresh `state` after the line's action (same `window` as the command) |
| `expect_result` | expectation(s) checked against the action's own result: a command's `result`, a `gh_log`'s calls, a restart's `ready` event |
| `expect_error` | the command must fail with an error containing this text |
| `runner_timeout_ms` | reply timeout for this line, replacing the default |
| `save` | `{"name": "<pointer>", ...}`: sets `${name}` from the action's result (a command's `result`, a `gh_log`'s calls, a restart's `ready` event) for this line's `expect` and every later line; a string is used as-is, anything else as compact JSON. E.g. `{"cmd": "state", "save": {"review": "/review/id"}}`, then `{"cmd": "click", "target": "switcher/review/${review}"}` |

Two more kinds of line are handled by the runner itself:

```json
{"gh_log": {}, "expect_result": [{"pointer": "", "len": 1}, {"pointer": "/0/argv/0", "equals": "api"}, {"pointer": "/0/stdin", "contains": "REQUEST_CHANGES"}]}
{"restart": {}}
{"restart": {"app_args": ["--window-size", "1100x700"]}, "expect": {"pointer": "/window/w", "equals": 1100}}
{"restart": {"data_dir": false}, "expect": {"pointer": "/data_dir", "equals": "${data_dir}"}}
```

- **`gh_log`** reads `mock_gh`'s call log (`DV_MOCK_GH_LOG`) and makes its
  calls, in the order they were made, the result for `expect_result` and
  `save`: an array of `{"argv": [...], "stdin": "..." | null, "matched": <entry
  index> | null}`, `[]` when nothing has called `gh` yet. It works with or
  without a `gh` header (without one, every call is unmatched), and after a
  `quit` too, since the log outlives the app. Its options object must be
  empty; it can't take `expect_error` or `runner_timeout_ms`. `mock_gh` logs
  a call before it exits, so once the app has a call's result (after the
  command that made it, or a `settle`) a `gh_log` line sees it.
- **`restart`** sends `quit`, requires a clean exit within 15 s, then starts
  the app again with the same data dir, `mock_gh` scenario and log, window
  size and environment and waits for `ready`. `app_args`, when given,
  replace the header's for this and later launches. `"data_dir": false`
  starts it (and later launches) without `--data-dir`, so it finds its data
  dir as a normal start does, through `DV_DATA_DIR` (which the runner always
  sets to the scenario's); `"data_dir": true` puts the flag back. Screenshot
  numbering and the transcript carry on. `ready` comes with the first frame,
  before anything the app loads in the background: a restart's own `expect`
  sees only what startup restores synchronously, so check the rest on a
  `settle` line after it.
  After an explicit `{"cmd": "quit"}` only a restart (or `gh_log` lines
  without `expect`) may follow.

Don't set `id`: the runner numbers requests by line. A screenshot takes a
`name` and the runner turns it into `<out>/<scenario>/NN-<name>.png`; an
explicit `path` is used instead, and a relative one resolves against the
runner's working directory, not the app's (the data dir, removed once the
scenario passes unless `--keep-data`). The default reply timeout is 30 s
plus whatever the command itself waits (`ms`, `timeout_ms`,
`hold_ms + step_ms * steps`).

Strings in commands, expectations and `app_args` may use `${data_dir}` (this
run's data dir, also the app's working directory), `${out_dir}` (its output
dir), `${workspace}` (the repo root), `${dv_gh}` (the `mock_gh` the app runs
as `gh`), `${dv_mock_gh_log}` (the call log) and, with a `gh` header,
`${dv_mock_gh}` (the absolute path of its file), e.g.
`"path": "${workspace}/tests/fixtures/repos/crlf"`. Variables set by `save`
work in commands and expectations (not in `app_args`).

### `mock_gh` scenario files

A header's `gh` file tells `mock_gh` (`crates/mock_gh`, PLAN §14.2, the
authority on the format) how to answer:

```json
{"calls": [
  {"argv": ["pr", "view", "482", "--json", "*"], "stdout": {"number": 482}, "times": 1},
  {"argv": ["api", "**"], "stdin_contains": "\"event\":\"REQUEST_CHANGES\"", "stdout": {"id": 7}},
  {"argv": ["api", "**"], "stderr": "gh: HTTP 422\n", "exit": 1}
]}
```

`argv` patterns are exact strings, `"*"` for any one argument and `"**"` for
all the rest (possibly none; it must be last). The first entry that matches
(argv, and `stdin_contains` when given) and has calls left (`times`,
unlimited by default) answers with its `stdout` (a string as-is, or any other
JSON value as compact JSON plus a newline, as `gh --json` prints), `stderr`
and `exit` (0 by default). A call no entry answers writes `mock_gh: no
scenario entry for argv [...] in <scenario>` to stderr and exits 64. Unknown
fields are errors. Keep these files in `tests/fixtures/gh/` (`"gh":
"../fixtures/gh/pr_482.json"` from a scenario here), where other tests can
share them.

How `mock_gh` behaves around that:

- **stdin** is read only when an argument asks gh to read it, as the real gh
  does: an argument `-` (`--input -`), or one ending in `=-` or `@-`
  (`--body-file=-`, `-F body=@-`). Otherwise it is left alone (logged as
  `null`), so a call whose stdin is an inherited pipe never drains or waits
  on it, and `stdin_contains` can't match.
- **`times`** counts the calls each entry answered in the call log, across
  every `mock_gh` process and app restart that shares it. A call takes an
  exclusive lock on `<log>.lock` while it counts and appends, so concurrent
  calls never hand out an entry's last answer twice. Without
  `DV_MOCK_GH_LOG` nothing is counted.
- **The log line** (`{"argv", "stdin", "matched"}`, one `write` on an
  append-mode handle) is written before anything is printed, so whoever has
  the call's output can already read it.
- **Exit codes** of its own: 64 for an unmatched call, 65 when the scenario
  file can't be read or is invalid (the call is still logged), 74 when the
  log can't be written (nothing is answered).

### Expectations

`{"pointer": <RFC 6901 pointer>, <check>: <operand>, ...}`; all checks on a
line must hold, and `expect` may be a list.

| Check | Holds when |
|---|---|
| `equals` | deep-equal; numbers compare by value with a 1e-6 relative tolerance |
| `not_equals` | missing, or not deep-equal |
| `contains` | substring of a string; an array element that matches (objects by subset); a subset of an object |
| `exists` | `true`: the pointer resolves (a `null` value counts); `false`: it doesn't |
| `gte`, `lte` | the value is a number `>=` / `<=` the operand |
| `len` | array length, object key count, or string length in characters |

`""` points at the whole document. Unknown keys are errors, so a typo like
`"equal"` fails the scenario instead of asserting nothing.

## Exploratory runs

Looking at the app by hand, or trying a change that no scenario covers yet,
keeps to the same rules as a scenario: a data dir under `target/`, `mock_gh`
in place of `gh`, windows in the background, and a run that ends with `quit`.
A plain `dv` launch without `--automation` is the owner's own run, never an
exploratory one: it uses the real data dir (`%APPDATA%\dv`) and the real
`gh`.

**A scratch scenario** is the simplest way. Keep it out of `tests/e2e`,
where every `*.jsonl` is a test, for example in `target/tmp/explore.jsonl`:

```json
{"scenario": {"window_size": "1440x900"}}
{"cmd": "wait_ready", "timeout_ms": 20000}
{"cmd": "settle"}
{"cmd": "screenshot", "name": "shell"}
{"cmd": "state"}
```

```bash
cargo build -p dv -p mock_gh
cargo run -p gpui_automation --bin e2e -- --out target/tmp/explore-e2e --keep-data target/tmp/explore.jsonl
```

Everything in "Runner" applies: the hermetic environment, background
windows, the closing `quit`, and killing whatever is left. The replies,
`state` included, are in `target/tmp/explore-e2e/explore/transcript.log`
(`-v` prints them as well), next to the screenshots and, with `--keep-data`,
the data dir. Giving the run its own `--out` keeps it clear of a full run's
`target/e2e`.

**Attached over a named pipe**, to send commands one at a time to a running
app. This needs PowerShell 7.4 or later for `Start-Process -Environment`.
Run it as one script, because the process id lives in its variables:

```powershell
cargo build -p dv -p mock_gh
$dir = (New-Item -ItemType Directory -Force 'target/tmp/explore').FullName
$app = $null
try {
    # dv on a scratch data dir, with mock_gh as gh (no scenario file: every gh call fails, exit 64).
    $app = Start-Process target/debug/dv.exe -NoNewWindow -PassThru -RedirectStandardError "$dir/app.log" `
        -ArgumentList "--automation-pipe dv-dev --automation-background --data-dir $dir/data" `
        -Environment @{ DV_DATA_DIR = "$dir/data"; DV_GH = (Resolve-Path target/debug/mock_gh.exe).Path
                        DV_MOCK_GH_LOG = "$dir/gh_calls.jsonl" }
    $pipe = [System.IO.Pipes.NamedPipeClientStream]::new('.', 'dv-dev', 'InOut'); $pipe.Connect(60000)
    $reader = [System.IO.StreamReader]::new($pipe); $writer = [System.IO.StreamWriter]::new($pipe)
    $writer.AutoFlush = $true
    $reader.ReadLine()                                           # {"event":"ready",...}
    $writer.WriteLine('{"id": 1, "cmd": "state"}'); $reader.ReadLine()
    # ...more commands (Protocol, below)...
    $writer.WriteLine('{"id": 2, "cmd": "quit"}'); $reader.ReadLine()
    $pipe.Dispose(); $app.WaitForExit(15000) | Out-Null
} finally {
    if ($app) { Stop-Process -Id $app.Id -ErrorAction Ignore }
}
```

Paths in commands resolve against the app's working directory, which is the
repo root here. The app refuses to start if another process already serves
the pipe name, so give concurrent sessions different names.

## Protocol

The app reads one JSON request per line on stdin and writes exactly one
reply line per request on stdout:

```json
{"id": 7, "cmd": "click", "window": "main", "target": "file_list/src/routes/users.ts"}
{"id": 7, "ok": true, "result": {"clicked": [140.0, 212.0], "count": 1}}
{"id": 8, "ok": false, "error": "no target \"file_list/src/routes/user.ts\" is rendered in window \"main\" (#0); rendered targets include ..."}
```

`id` is echoed (`null` if absent). `window` (a registered name or an index,
see `windows`) addresses one command to a window other than the selected
one. Unknown arguments are errors. Besides replies the app prints one event,
`{"event": "ready", "windows": [...], "pid": N}`, once its window has drawn.
Nothing else goes to stdout; logs go to stderr. Closing stdin quits the app.

With `Options::pipe` (an app's `--automation-pipe <name>` flag, which both
dv and the harness have) the same protocol runs over the Windows named pipe
`\\.\pipe\<name>` instead, so a driver can attach to an app that is already
running. Stdin and stdout are then left alone. One client at a time (another
gets `ERROR_PIPE_BUSY`); each is greeted with the `ready` event once the
window has drawn; disconnecting doesn't quit the app, and the next client can
connect straight away. Replies to a client that has left are dropped, and a
long wait whose client has left gives up early, as it does when stdin closes.
The app refuses to start if another process already serves that name. Any
language can drive it; from PowerShell:

```powershell
$pipe = [System.IO.Pipes.NamedPipeClientStream]::new('.', 'dv', 'InOut')
$pipe.Connect(5000)
$reader = [System.IO.StreamReader]::new($pipe); $writer = [System.IO.StreamWriter]::new($pipe)
$writer.AutoFlush = $true
$reader.ReadLine()                                  # {"event":"ready",...}
$writer.WriteLine('{"id": 1, "cmd": "state"}'); $reader.ReadLine()
$pipe.Dispose()                                     # the app keeps running
```

Every command that injects input replies only after the window has drawn
the result, so the next command never hit-tests a stale frame and `state`
already reflects the change; no `wait` is needed between steps. Commands
that wait on the frame loop fail after 10 s without frames (a minimised
window). Waits are capped at 120 s.

Positions are window coordinates in logical pixels (`x`, `y`), or a
`target`: a name views attach with
`.automation_target("file_list/src/routes/users.ts")`. A target command
captures one fresh frame of the window and uses the centre of the target's
visible (unclipped) bounds, so it keeps working when layout moves; a target
that is rendered but scrolled out of view is an error, not a click on the
wrong thing. Coordinates outside the window are rejected, since a drag
leaving the window could turn into a real OS drag-and-drop.

| Command | Arguments | Result |
|---|---|---|
| `state` | | the host's state object plus `window` and `windows` (below) |
| `windows` | | `[{index, name, id, selected}]`: registered windows in registration order, then unregistered ones |
| `window` | `index` or `name` | selects the default window for later commands; returns its `window` info |
| `actions` | | every registered action name |
| `action` | `name` (full, or a unique suffix such as `"ToggleSplit"`), `data`? | `{action}` with the full name |
| `key` | `keystroke` (`"ctrl-shift-k"`) | `{handled}`; the key goes down and comes back up, so Enter or Space on a focused button presses it |
| `keys` | `keys`: `"ctrl-k down enter"` or a list | `{handled: [..]}`; each key goes down and up, and a frame renders between keystrokes |
| `type` | `text` | `{typed}`; per-character keystrokes into the focused input, `\n` is `enter` |
| `click` | `x`,`y` or `target`; `button` (`left`/`right`/`middle`); `modifiers` (`"ctrl-shift"`); `count` (2 = double click) | `{clicked, count}`; hovers and renders first, like a real pointer. A `target` is pressed once two frames in a row drew it in the same place (up to 1 s), so a button still sliding in (a dialog's entrance) is pressed where it is, not where it was |
| `mouse_move` | `x`,`y` or `target`; `modifiers` | `{moved}`; carries a held button, so it also drags |
| `mouse_down`, `mouse_up` | position optional (defaults to the pointer); `button`, `modifiers` | `{at, down}` |
| `drag` | `from`, `to` (each `[x, y]`, `{"x","y"}`, `{"target"}` with an optional `dx`/`dy` offset from its centre, or a target name); `steps` (10), `step_ms`, `hold_ms`, `release` (true), `button`, `modifiers` | `{from, to, steps, released}` |
| `scroll` | position or `target`; `dx`, `dy`; `unit` (`px` default, or `lines`) | `{at, dx, dy}`; negative `dy` scrolls down, as a wheel notch towards you does |
| `drop_files` | `paths` (must exist); position or `target`; `hold_ms`; `submit` (true); `cancel` | `{paths, at, outcome}` (`dropped`, `hovering`, `cancelled`) |
| `resize` | `w`, `h` (logical), `timeout_ms`? | `{w, h}` once the viewport has that size. On Windows the size is exact even when it is taller or wider than the display (a portrait 1080x1920); the window's minimum size still applies, and a fullscreen or maximised window is an error |
| `fullscreen` | `on`, `timeout_ms`? | the `window` info once the switch has happened |
| `screenshot` | `path`; `source`: `"window"` (default) or `"screen"` (Windows, foreground only; see "Screenshots and window materials") | `{path, source, w, h, scale, logical: {w, h}}`; `w`/`h` are physical pixels |
| `bounds` | `target` | `{target, x, y, w, h, visible, count}`; `visible` is `null` when clipped away |
| `query` | `prefix` (`""` = all) | every rendered target with that prefix, in render order |
| `wait` | `ms` | `{waited_ms}` |
| `wait_frames` | `n` (1) | `{frames, elapsed_ms}` |
| `wait_ready` | `timeout_ms` (15000) | `{ready, elapsed_ms}` once the host reports settled |
| `settle` | `timeout_ms` (10000) | `{frames, elapsed_ms}` once the host is settled and a frame has rendered |
| `wait_for` | an expectation (`pointer` + checks), `timeout_ms` (10000), `interval_ms` (25) | `{elapsed_ms, actual}`; the error carries the last mismatch |
| `assert` | an expectation | `{actual}`, or an error describing the mismatch |
| `quit` | | `{quitting: true}`, then the app exits |

Anything else is offered to the app's `AutomationHost::custom`, which is how
apps add their own commands, then to `AutomationHost::custom_task`, for
commands that finish later (git work, a `gh` call): its reply waits for the
task, within the 120 s cap; unknown commands are errors. A command can't take
an argument called `id` (that is the request id).

`state.window` is
`{index, name, id, w, h, scale, fullscreen, maximized, active, background, bounds: {x, y, w, h}, display: {id, bounds}}`
(logical pixels; `bounds` is the window's position on the desktop) and
`state.windows` repeats the `windows` list.

### Multi-step drags and drops

`drag` moves in `steps` increments and waits for a rendered frame after each
one, so drag-over styling, drag previews and `on_drag_move` handlers see
every position as they would with a real mouse. `hold_ms` hovers at `to`
before releasing (hover-to-expand timers run meanwhile). With
`"release": false` the button stays down: take a screenshot or assert
mid-drag, then continue with `drag` without `from`, or finish with
`mouse_up`.

`drop_files` sends the platform's own `FileDrop` events (entered, then
over, then dropped), so the app's real `on_drop::<ExternalPaths>` code runs.
`"submit": false` leaves the files hovering; a later `drop_files` without
`paths` continues that drag, and `"cancel": true` drags them back out.

## Wiring an app (`gpui_automation`)

```rust
impl gpui_automation::AutomationHost for AppState {
    fn state(&self, window: &Window, cx: &App) -> serde_json::Value { /* the assert surface */ }
    fn settled(&self, cx: &App) -> bool { /* no pending tasks, streams or animations */ }
    fn custom(&mut self, cmd: &str, args: Value, window: &mut Window, cx: &mut Context<Self>)
        -> Option<anyhow::Result<Value>> { None }
    // optional: commands whose reply waits for a task
    fn custom_task(&mut self, cmd: &str, args: Value, window: &mut Window, cx: &mut Context<Self>)
        -> Option<Task<anyhow::Result<Value>>> { None }
}

// before the first window, when --automation-background was passed:
gpui_automation::set_background(true);
// open every window through this (a plain cx.open_window unless in the background):
let window = gpui_automation::open_window(cx, options, |window, cx| /* root view */)?;
// after opening the main window, when --automation was passed
// (`Options { pipe: Some(name), ..Default::default() }` for --automation-pipe <name>):
gpui_automation::start(window.into(), app_state, cx, gpui_automation::Options::default())?;
// when opening another window, also through open_window:
gpui_automation::register_window("second", second.into(), cx);
// never activate in the background:
if !gpui_automation::is_background() { cx.activate(true); }
// in each window's root view, as its first child:
div().child(gpui_automation::pointer_guard()) /* ... */
```

Views name elements with `use gpui_automation::AutomationTargetExt as _;`
and `.automation_target("file_list/src/main.rs")` (or
`.automation_target_with(|| format!(..).into())` for computed names). When
automation is off this is a plain `into_any_element()`; when it is on, bounds
are recorded only during the one frame a target command captures. Name
targets hierarchically so `query` can list a group: `titlebar/switcher`,
`shell/panel`, `file_list/<path>`, `statusbar/material`.

Known limits: `Window::is_window_hovered` follows the real OS cursor, so
hover logic gated on it needs a `gpui_automation::is_active()` bypass;
the named-pipe transport is Windows-only; screenshots work on Windows and on
Linux under X11 (a `GetImage` of the window, so an unscaled display captures
at logical size), and the `screen` source only on Windows; and off Windows
`resize` goes through gpui, where the
window manager may clamp a size larger than the display.

On a GPU-less Linux box (a cloud container), run under Xvfb with Mesa's
software Vulkan (`mesa-vulkan-drivers`, plus `libxkbcommon-x11-dev` to
build): `Xvfb :99 -screen 0 1600x1000x24 & DISPLAY=:99 cargo run -p
gpui_automation --bin e2e -- tests/e2e`.
