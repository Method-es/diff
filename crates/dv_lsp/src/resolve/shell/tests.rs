//! The shell-`PATH` reader (docs/PLAN.md §18.4, §18.7): the markers found among an interactive
//! shell's banners, the fallbacks (`-l` when `-l -i` fails or hangs, then the backend's own
//! `PATH`), against a scripted backend on every OS and, on Unix, against fake shells run
//! through `LocalBackend`.

use std::collections::VecDeque;
use std::sync::mpsc::{self, Sender};

use dv_core::backend::{
    Backend, BackendError, GitOpts, GitOutput, LocalBackend, LocalBackendOptions, ProcessControl,
    ProcessEvent, ProcessHandle, ProcessSink, ProcessSpec, RepoInfo, WatchHandle, WatchSink,
    WatchSpec,
};
use dv_test_support::FixtureRepo;
use parking_lot::Mutex;
use pretty_assertions::assert_eq;

use super::*;

#[test]
fn the_path_is_what_lies_between_this_calls_markers() {
    let nonce = "n1";
    let out = "Welcome to zsh!\n\u{1b}]0;kyle@box\u{7}__dv_path_begin_n1\n/home/k/.bun/bin:/usr/bin\n\
               /home/k/.bun\n__dv_path_end_n1\nbye\n";
    assert_eq!(
        between_markers(out, nonce),
        Some(Printed {
            path: "/home/k/.bun/bin:/usr/bin".into(),
            bun_install: Some("/home/k/.bun".into()),
        })
    );
    let crlf = "__dv_path_begin_n1\r\n/usr/bin\r\n\r\n__dv_path_end_n1\r\n";
    assert_eq!(
        between_markers(crlf, nonce),
        Some(Printed {
            path: "/usr/bin".into(),
            bun_install: None,
        })
    );
    // Another call's markers, an end that never came, an empty PATH: nothing.
    assert_eq!(between_markers(out, "n2"), None);
    assert_eq!(
        between_markers("__dv_path_begin_n1\n/usr/bin\n", nonce),
        None
    );
    assert_eq!(
        between_markers("__dv_path_begin_n1\n\n\n__dv_path_end_n1\n", nonce),
        None
    );
    // The command's own text never holds a whole marker.
    assert!(!print_script().contains("__dv_path_begin_$"));
    assert!(!print_script().contains("__dv_path_begin_\""));
}

#[test]
fn env_output_is_read_by_name() {
    let env = parse_env("SHELL=/usr/bin/zsh\nPATH=/usr/bin:/bin\nweird line\n=x\nA_B=1=2\n");
    assert_eq!(
        env,
        [
            ("SHELL".to_string(), "/usr/bin/zsh".to_string()),
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("A_B".into(), "1=2".into()),
        ]
    );
    // `env -0`: a value spanning lines stays whole.
    let env = parse_env("MOTD=hello\nworld\0SHELL=/bin/fish\0");
    assert_eq!(
        env,
        [
            ("MOTD".to_string(), "hello\nworld".to_string()),
            ("SHELL".into(), "/bin/fish".into()),
        ]
    );
}

/// What the scripted shell does when started with `-i`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Interactive {
    /// Prints banners around the markers and an interactive `PATH`.
    Prints,
    /// Exits 1 having printed nothing.
    Fails,
    /// Never ends until killed.
    Hangs,
}

/// A backend whose `spawn_stdio` plays `/usr/bin/env` and the user's shell; everything else is
/// a real `LocalBackend`'s.
struct Scripted {
    inner: LocalBackend,
    env: &'static str,
    interactive: Interactive,
    /// The argument lists each spawn ran with.
    spawned: Mutex<Vec<Vec<String>>>,
}

struct Control {
    kill: Sender<()>,
}

impl ProcessControl for Control {
    fn write(&self, _: &[u8]) -> Result<(), BackendError> {
        Ok(())
    }

    fn kill(&self) {
        let _ = self.kill.send(());
    }
}

impl Scripted {
    fn new(repo: &FixtureRepo, env: &'static str, interactive: Interactive) -> Self {
        let inner = LocalBackend::open_with(
            repo.path(),
            LocalBackendOptions {
                env: repo.env_overlay(),
            },
        )
        .unwrap();
        Self {
            inner,
            env,
            interactive,
            spawned: Mutex::new(Vec::new()),
        }
    }
}

impl Backend for Scripted {
    fn repo(&self) -> &RepoInfo {
        self.inner.repo()
    }
    fn git(&self, args: &[&str], opts: &GitOpts) -> Result<GitOutput, BackendError> {
        self.inner.git(args, opts)
    }
    fn worktree_read(&self, rel: &str) -> Result<Option<Vec<u8>>, BackendError> {
        self.inner.worktree_read(rel)
    }
    fn worktree_len(&self, rel: &str) -> Result<Option<u64>, BackendError> {
        self.inner.worktree_len(rel)
    }
    fn store_append(&self, rel: &str, line: &[u8]) -> Result<(), BackendError> {
        self.inner.store_append(rel, line)
    }
    fn store_read(&self, rel: &str) -> Result<Option<Vec<u8>>, BackendError> {
        self.inner.store_read(rel)
    }
    fn store_list(&self, dir: &str) -> Result<Vec<String>, BackendError> {
        self.inner.store_list(dir)
    }
    fn store_write_atomic(&self, rel: &str, bytes: &[u8]) -> Result<(), BackendError> {
        self.inner.store_write_atomic(rel, bytes)
    }
    fn store_remove(&self, rel: &str) -> Result<bool, BackendError> {
        self.inner.store_remove(rel)
    }
    fn copy_index_to_store(&self, rel: &str) -> Result<bool, BackendError> {
        self.inner.copy_index_to_store(rel)
    }
    fn watch(&self, spec: WatchSpec, sink: WatchSink) -> Result<WatchHandle, BackendError> {
        self.inner.watch(spec, sink)
    }

    fn spawn_stdio(
        &self,
        spec: &ProcessSpec,
        mut sink: ProcessSink,
    ) -> Result<ProcessHandle, BackendError> {
        let mut argv = vec![spec.program.clone()];
        argv.extend(spec.args.iter().cloned());
        self.spawned.lock().push(argv.clone());
        let nonce = spec
            .env
            .iter()
            .find(|(name, _)| name == "DV_PATH_NONCE")
            .and_then(|(_, value)| value.clone())
            .unwrap_or_default();
        let marked = |path: &str| {
            format!("__dv_path_begin_{nonce}\n{path}\n\n__dv_path_end_{nonce}\n").into_bytes()
        };
        let mut script: VecDeque<ProcessEvent> = VecDeque::new();
        let mut hang = false;
        match (spec.program.as_str(), argv.contains(&"-i".to_string())) {
            ("/usr/bin/env", _) => {
                script.push_back(ProcessEvent::Stdout(self.env.as_bytes().to_vec()));
                script.push_back(ProcessEvent::Exit(Some(0)));
            }
            ("/bin/sh", true) => match self.interactive {
                Interactive::Prints => {
                    script.push_back(ProcessEvent::Stdout(b"Last login: today\n".to_vec()));
                    script.push_back(ProcessEvent::Stdout(marked(
                        "/home/u/.bun/bin:/usr/bin:/mnt/c/Windows",
                    )));
                    script.push_back(ProcessEvent::Stdout(b"\x1b]0;u@box\x07".to_vec()));
                    script.push_back(ProcessEvent::Exit(Some(0)));
                }
                Interactive::Fails => script.push_back(ProcessEvent::Exit(Some(1))),
                Interactive::Hangs => hang = true,
            },
            ("/bin/sh", false) => {
                script.push_back(ProcessEvent::Stdout(marked("/usr/local/bin:/usr/bin")));
                script.push_back(ProcessEvent::Exit(Some(0)));
            }
            _ => {
                return Err(BackendError::InvalidProcess {
                    program: spec.program.clone(),
                    reason: "not scripted".into(),
                });
            }
        }
        let (kill, killed) = mpsc::channel();
        std::thread::spawn(move || {
            for event in script {
                sink(event);
            }
            if hang {
                let _ = killed.recv();
                sink(ProcessEvent::Exit(None));
            }
        });
        Ok(ProcessHandle::new(1, Control { kill }))
    }
}

const ENV: &str = "HOME=/home/u\nSHELL=/usr/bin/zsh\nUSER=u\nPATH=/usr/bin:/bin:/mnt/d/tools\n";

#[test]
fn the_interactive_login_shell_comes_first() {
    let repo = FixtureRepo::new();
    let backend = Scripted::new(&repo, ENV, Interactive::Prints);
    let host = read_with(&backend, "/usr/bin/env", Duration::from_secs(5));
    assert_eq!(host.path(), ["/home/u/.bun/bin", "/usr/bin"]);
    assert_eq!(host.home(), Some("/home/u"));
    assert_eq!(
        host.from(),
        &PathFrom::Shell {
            shell: "/usr/bin/zsh".into(),
            flags: "-l -i".into()
        }
    );
    let spawned = backend.spawned.lock().clone();
    assert_eq!(spawned.len(), 2, "{spawned:?}");
    assert_eq!(spawned[0], ["/usr/bin/env", "-0"]);
    // The user's shell, its stdin from /dev/null, through /bin/sh.
    assert_eq!(
        spawned[1][..7],
        [
            "/bin/sh",
            "-c",
            "exec \"$0\" \"$@\" </dev/null",
            "/usr/bin/zsh",
            "-l",
            "-i",
            "-c"
        ]
    );
}

#[test]
fn a_failing_or_hanging_interactive_shell_falls_back_to_the_login_shell() {
    let repo = FixtureRepo::new();
    for interactive in [Interactive::Fails, Interactive::Hangs] {
        let backend = Scripted::new(&repo, ENV, interactive);
        let started = Instant::now();
        let host = read_with(&backend, "/usr/bin/env", Duration::from_millis(300));
        assert_eq!(host.path(), ["/usr/local/bin", "/usr/bin"]);
        assert_eq!(
            host.from(),
            &PathFrom::Shell {
                shell: "/usr/bin/zsh".into(),
                flags: "-l".into()
            }
        );
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}

#[test]
fn without_a_shell_the_backends_own_path_is_used() {
    let repo = FixtureRepo::new();
    // No SHELL and no passwd entry for the user: the backend's PATH, /mnt left out.
    let backend = Scripted::new(
        &repo,
        "HOME=/home/u\nUSER=dv-no-such-user\nPATH=/usr/bin:/mnt/c/x\n",
        Interactive::Prints,
    );
    let host = read_with(&backend, "/usr/bin/env", Duration::from_secs(5));
    assert_eq!(host.path(), ["/usr/bin"]);
    assert_eq!(host.from(), &PathFrom::Backend);
    // No env at all: nothing known.
    let host = read_with(&backend, "/dv/no/env", Duration::from_secs(5));
    assert_eq!(host.path(), [] as [&str; 0]);
    assert_eq!(host.from(), &PathFrom::Unknown);
}

/// Real fake shells through `LocalBackend` (POSIX only: the reader runs in a distro).
#[cfg(unix)]
mod real {
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Path;

    use pretty_assertions::assert_eq;

    use super::*;

    fn script(dir: &Path, name: &str, text: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
    }

    /// A fake shell: banners on stdout, reads its stdin to the end (which hangs unless it is
    /// closed), sleeps when `-i` and `hang`, then runs its `-c` command with `path` as PATH.
    fn shell(dir: &Path, name: &str, interactive_path: &str, hang: bool) -> String {
        let hang = if hang { "sleep 30" } else { "true" };
        script(
            dir,
            name,
            &format!(
                "#!/bin/sh\necho 'Welcome to the fake shell'\nprintf '\\033]0;title\\007'\n\
                 cat >/dev/null\ninteractive=no\nfor arg in \"$@\"; do [ \"$arg\" = -i ] && \
                 interactive=yes; done\nnewpath=/usr/bin:/bin\nif [ $interactive = yes ]; then \
                 {hang}; newpath='{interactive_path}'; fi\nwhile [ $# -gt 0 ] && [ \"$1\" != -c \
                 ]; do shift; done\nshift\nPATH=$newpath\neval \"$1\"\necho bye\n"
            ),
        )
    }

    fn backend(repo: &FixtureRepo) -> LocalBackend {
        LocalBackend::open_with(
            repo.path(),
            LocalBackendOptions {
                env: repo.env_overlay(),
            },
        )
        .unwrap()
    }

    #[test]
    fn a_fake_interactive_shell_prints_its_path_among_banners() {
        let repo = FixtureRepo::new();
        let dir = repo.outside_dir("shells");
        let zsh = shell(
            &dir,
            "zsh",
            "/home/u/.bun/bin:/usr/bin:/mnt/c/Windows",
            false,
        );
        let env = script(
            &dir,
            "env",
            &format!("#!/bin/sh\necho HOME=/home/u\necho SHELL={zsh}\necho PATH=/usr/bin\n"),
        );
        let host = read_with(&backend(&repo), &env, Duration::from_secs(5));
        assert_eq!(host.path(), ["/home/u/.bun/bin", "/usr/bin"]);
        assert_eq!(
            host.from(),
            &PathFrom::Shell {
                shell: zsh,
                flags: "-l -i".into()
            }
        );
    }

    #[test]
    fn a_hanging_fake_shell_is_killed_and_the_login_shell_used() {
        let repo = FixtureRepo::new();
        let dir = repo.outside_dir("shells");
        let zsh = shell(&dir, "zsh", "/never", true);
        let env = script(
            &dir,
            "env",
            &format!("#!/bin/sh\necho HOME=/home/u\necho SHELL={zsh}\necho PATH=/sbin\n"),
        );
        let started = Instant::now();
        let host = read_with(&backend(&repo), &env, Duration::from_millis(500));
        assert_eq!(host.path(), ["/usr/bin", "/bin"]);
        assert_eq!(
            host.from(),
            &PathFrom::Shell {
                shell: zsh,
                flags: "-l".into()
            }
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
