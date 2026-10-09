---
name: dv-review
description: Address code review comments left in dv, the local diff reviewer, then hand the work back and wait for the verdict. Use when asked to address review comments, to act on a dv review, when the reviewer left comments on your changes, or to wait for the reviewer in a repository reviewed with dv.
---

# Answering a dv review

dv is a desktop app in which a person, the reviewer, reads your changes as a diff and leaves
comment threads on their lines. The threads live in the repository (under `.git/dv/`); you read
and answer them with the `dv` command line, never by editing those files.

A review goes in rounds. The reviewer finishes a round by requesting changes, in comment threads
and in a summary for the round; you fix the code, answer every open thread and hand the review
back with `dv review done`; the reviewer then resolves threads, requests more changes or
approves.

## Rules

- Pass `--json` whenever you read the output: one JSON document on stdout, `"schema": 1`.
- Pass `--author <agent>` on every command that writes, `<agent>` being your own name:
  `claude-code` for Claude Code, `codex` for Codex. Claude Code is recognised without it; say it
  anyway. The commands below write `<agent>` where your name goes.
- Act on `anchor.current` and `current_code`, never on the line numbers a comment was written
  on, nor on line numbers you read before your last edit to that file: the file has moved on.
- Never run `dv comment resolve` or `dv comment reopen`: they are the reviewer's.
- If you disagree with a comment, reply with your reasons and still address the thread, with a
  note saying that you left the code as it is.
- Send text through `--body-file -` (a note through `--note-file -`), which survives any
  quoting: a quoted heredoc in a POSIX shell, a here-string piped in under PowerShell (steps 3
  and 4 show both). A short one-line note may go inline in single quotes; never in double
  quotes, where a POSIX shell runs whatever is in backticks and PowerShell reads a backtick as
  its escape character.
- Don't start a review (`dv review new`) unless the user asks you to.
- Commit only if the user's instructions say so.

## The loop

1. Find the review and where it stands:

```sh
dv status --json
```

`current` is the review the other commands act on: the open review of the work checked out
here. Keep its `id` for steps 4 and 5, and the `n` of the last of its `rounds` (the round the
reviewer finished) for step 4. When it is `null`, `reviews` lists the repository's open reviews,
each with its `worktree`: if one of them is clearly the review you were asked about, add
`--review <review-id>` to every command (or set `DV_REVIEW` to its id). If none is, or you can't
tell which, stop and tell the user; don't guess, and don't start a review.

`target.kind` says what the review compares, and so where your fixes show up for the reviewer:

- `working`: the files as they are on disk in the review's `worktree`. Your edits show as you
  make them. Work in that worktree.
- `branch`: the branch's commits, plus the files on disk in whichever worktree has that branch
  checked out (the review is `current` only in that worktree; checked out nowhere, it is the
  branch's commits alone). Your edits show there.
- `staged`: what is staged (`git add`ed), not the files on disk. Your edits show only once they
  are staged: ask the user before you run `git add`, and say so when you hand back.
- `commit`, `range`: fixed history. Edits can't change what this review shows: tell the user
  instead of handing back.

2. Read what the reviewer asked for. The summary of the round they finished is the `body` of the
   last of `current.rounds` (`round.body` in step 5's output): it speaks for the whole round
   and may ask for more than the threads do. Then the threads waiting for you:

```sh
dv comment list --status open --json
```

Each thread has `id`, `body` (the comment that opened it, Markdown), its `author` and
`replies`. `anchor.current` is where the commented code is now (`path`, `side`, `start_line`,
`end_line`) and `current_code` its text. `side` `new` is the review's new side: the file on disk for a `working`
review, what is staged for a `staged` one (see `target.kind` above); `old` is the file as it
was before the change. `code` is what the comment was written on. When `anchor.current` is
`null` the code is gone (`outdated`): judge from `code` and the comment.

A thread whose `author.kind` is `agent` is a question an agent asked the reviewer (`dv comment
add`, below), not a request. While the reviewer hasn't replied to it, it waits for them: leave
it open, don't address it. Once they have replied, act on the reply as on any comment.

A thread you addressed in an earlier round stays `addressed` when the reviewer only replies to
it; read the `replies` of those as well:

```sh
dv comment list --status addressed --json
```

3. For each thread that asks something of you, fix the code, then say what you did and mark the
   thread addressed. Once you have edited a file, the line numbers you listed for it are stale:
   before each fix, read the thread again for its `anchor.current` (or find the code by its
   `current_code`):

```sh
dv comment show <thread-id> --json
dv comment reply <thread-id> --author <agent> --body-file - <<'EOF'
Moved the cache lookup into `getUser` and covered the miss path with a test.
EOF
dv comment address <thread-id> --author <agent> --note 'Cache lookup moved into getUser'
```

In PowerShell, pipe a here-string instead of the heredoc (in Windows PowerShell 5.1, set
`$OutputEncoding = [System.Text.UTF8Encoding]::new()` first, or text that isn't ASCII arrives
mangled):

```powershell
@'
Renamed the helper as asked.
'@ | dv comment reply <thread-id> --author <agent> --body-file -
```

4. When every open thread is answered and addressed (save your own questions still waiting for
   the reviewer), and what the round's summary asks is done, hand the review back by its id,
   naming the round you worked on, with a note on what you did, Markdown the reviewer reads in
   dv. `<n>` is that round's number: `round.n` from the `dv review wait` that gave you the
   round (step 5), or, when you started without waiting, the `n` you kept in step 1 (with no
   round finished yet, leave `--round` out):

```sh
dv review done <review-id> --round <n> --author <agent> --note-file - <<'EOF'
- Moved the cache lookup into `getUser`
- Covered the miss path with a test
EOF
```

In PowerShell, pipe the note in as in step 3, with `--note-file -`.

If it exits 1 with the code `unseen_round`, the reviewer finished another round while you were
working, round `<n>` in the message, and nothing was handed back. Read that round as in step
2: its summary (the `body` of the last of `rounds` in `dv review show <review-id> --json`) and
its open threads. Act on them, then run the command above again with that round's number.

5. Wait for the reviewer, naming the review by its id (once approved or closed it is no longer
   open, and only its id finds it):

```sh
dv review wait <review-id> --timeout 600 --json
```

It returns once the reviewer finishes the round, or at once if they already have; `outcome`
says how:

- `changes_requested`: go back to step 2, starting with `round.body`; `round.n` is the round
  you now work on (step 4). `threads.open` counts the
  threads waiting, but at 0 the reviewer still wants something: it is in the summary, or in a
  reply on an addressed thread.
- `approved`: you are done. Commit only if the user's instructions say so.
- `closed`: the reviewer closed the review; stop.

Exit code 3 means the timeout passed first: run it again, or tell the user you are waiting.

## More commands

```sh
dv review show <review-id> --json
dv comment add --file src/app.ts --lines 10:12 --author <agent> --body-file - <<'EOF'
Is this branch still reachable?
EOF
```

`dv comment add` asks the reviewer something on lines of a changed file (`--side old` for the
lines as they were).

## Exit codes

- `0`: done.
- `1`: the operation failed; the JSON error's `code` says why (`not_found`, `not_allowed`,
  `rejected`, …). `finished`: the review is approved or closed and takes no more changes.
  `unseen_round` (from `dv review done`): the reviewer finished a round you haven't read; read
  it, then run step 4 again with that round's number.
- `2`: the command line is wrong, or it doesn't say which review. With `ambiguous_review`,
  `error.candidates` lists the open reviews (each with its `worktree`): pick the one you were
  asked about with `--review <review-id>`. With `no_review` and no candidates there is no review
  to answer: tell the user.
- `3`: `dv review wait` timed out.
- `4`: the repository is inside WSL and dv runs on Windows: run dv inside the distro instead
  (`wsl -d <distro> --cd <path> dv …`).

Errors go to stderr; with `--json`, stdout also carries `{"schema": 1, "error": {"code",
"message"}}`.
