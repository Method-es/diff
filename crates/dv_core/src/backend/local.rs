//! `LocalBackend`: the in-process [`Backend`] for a repository on this machine
//! (docs/PLAN.md §5.1). It spawns `git` directly, reads and writes files with std, and watches
//! with `notify` (PLAN §5.3). Inside a WSL distro, `dv backend --stdio` serves this same type to
//! the Windows GUI (PLAN D6).

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};

use super::paths::{self, STORE_TEMP_PREFIX};
use super::{
    Backend, BackendError, GitOpts, GitOutput, GitVersion, RepoInfo, TextStats, TextStatsCounter,
    WatchHandle, WatchSink, WatchSpec,
};
use crate::git::{ObjectFormat, command};
use crate::location::Location;

/// How a [`LocalBackend`] runs git.
#[derive(Debug, Clone, Default)]
pub struct LocalBackendOptions {
    /// Environment changes for every git the backend spawns: `(name, Some(value))` sets a
    /// variable, `(name, None)` removes it. Applied before dv's git policy, which wins
    /// ([`command`]).
    ///
    /// The app leaves this empty, so git sees the user's real configuration (autocrlf,
    /// attributes, excludes). Tests pass the fixture's environment (`FixtureRepo::env_overlay`),
    /// because the developer's global and system configuration would make results
    /// machine-dependent.
    pub env: Vec<(OsString, Option<OsString>)>,
}

/// The in-process [`Backend`] for one local worktree.
#[derive(Debug)]
pub struct LocalBackend {
    info: RepoInfo,
    toplevel: PathBuf,
    git_dir: PathBuf,
    common_dir: PathBuf,
    /// `<common_dir>/dv`, the review store's root (PLAN §7).
    store_root: PathBuf,
    options: LocalBackendOptions,
}

impl LocalBackend {
    /// Opens the worktree containing `path` (its root or any directory inside it, a linked
    /// worktree included) with the user's own git configuration.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, BackendError> {
        Self::open_with(path, LocalBackendOptions::default())
    }

    /// [`LocalBackend::open`] with explicit options.
    ///
    /// Refuses a missing path ([`BackendError::PathNotFound`]), a directory outside any
    /// repository ([`BackendError::NotARepository`]), a bare repository
    /// ([`BackendError::BareRepository`]) and a directory inside a git dir
    /// ([`BackendError::InsideGitDir`]).
    pub fn open_with(
        path: impl AsRef<Path>,
        options: LocalBackendOptions,
    ) -> Result<Self, BackendError> {
        let path = path.as_ref();
        let shown = path.display().to_string();
        match fs::metadata(path) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Err(BackendError::NotARepository { path: shown }),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(BackendError::PathNotFound { path: shown });
            }
            Err(err) => return Err(BackendError::io("reading", path, err)),
        }

        // `--path-format=absolute` makes every path absolute (git 2.31+); git prints one line
        // per option, in order.
        let args = [
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-dir",
            "--git-common-dir",
            "--show-object-format",
        ];
        let output = match command::run(path, &options.env, &args, &GitOpts::default()) {
            Ok(output) => output,
            Err(err) => return Err(classify_open_failure(path, &options, err)),
        };
        let unexpected = |detail: String| BackendError::UnexpectedOutput {
            args: args.iter().map(|arg| arg.to_string()).collect(),
            detail,
        };
        let text = output
            .stdout_utf8()
            .ok_or_else(|| unexpected("stdout is not UTF-8".to_string()))?;
        let lines: Vec<&str> = text.lines().collect();
        let [toplevel, git_dir, common_dir, format] = lines[..] else {
            return Err(unexpected(format!("expected 4 lines, got {text:?}")));
        };
        let absolute = |line: &str| {
            let path = native_path(line);
            if path.is_absolute() {
                Ok(path)
            } else {
                Err(unexpected(format!("{line:?} is not an absolute path")))
            }
        };
        let toplevel = absolute(toplevel)?;
        let git_dir = absolute(git_dir)?;
        let common_dir = absolute(common_dir)?;
        let object_format = ObjectFormat::parse(format)
            .ok_or_else(|| unexpected(format!("unknown object format {format:?}")))?;

        let version = command::run(path, &options.env, &["version"], &GitOpts::default())?;
        let git_version = version
            .stdout_utf8()
            .and_then(GitVersion::parse)
            .ok_or_else(|| BackendError::UnexpectedOutput {
                args: vec!["version".to_string()],
                detail: format!("{:?}", String::from_utf8_lossy(&version.stdout)),
            })?;

        let info = RepoInfo {
            location: Location::Local(toplevel.clone()),
            toplevel: toplevel.display().to_string(),
            git_dir: git_dir.display().to_string(),
            common_dir: common_dir.display().to_string(),
            object_format,
            git_version,
        };
        Ok(Self {
            info,
            store_root: common_dir.join("dv"),
            toplevel,
            git_dir,
            common_dir,
            options,
        })
    }

    /// The worktree root.
    pub fn toplevel(&self) -> &Path {
        &self.toplevel
    }

    /// The review store's root, `<git-common-dir>/dv` (PLAN §7).
    pub fn store_root(&self) -> &Path {
        &self.store_root
    }
}

impl Backend for LocalBackend {
    fn repo(&self) -> &RepoInfo {
        &self.info
    }

    fn git(&self, args: &[&str], opts: &GitOpts) -> Result<GitOutput, BackendError> {
        command::run(&self.toplevel, &self.options.env, args, opts)
    }

    fn worktree_read(&self, rel: &str) -> Result<Option<Vec<u8>>, BackendError> {
        let path = paths::worktree_path(&self.toplevel, rel)?;
        if let Some(target) = link_target(&path)? {
            return Ok(Some(target));
        }
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(err) if is_missing(&err) => Ok(None),
            Err(_) if path.is_dir() => Err(is_a_directory(&path)),
            Err(err) => Err(BackendError::io("reading", &path, err)),
        }
    }

    fn worktree_len(&self, rel: &str) -> Result<Option<u64>, BackendError> {
        let path = paths::worktree_path(&self.toplevel, rel)?;
        if let Some(target) = link_target(&path)? {
            return Ok(Some(target.len() as u64));
        }
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_dir() => Err(is_a_directory(&path)),
            Ok(metadata) => Ok(Some(metadata.len())),
            Err(err) if is_missing(&err) => Ok(None),
            Err(err) => Err(BackendError::io("reading", &path, err)),
        }
    }

    fn worktree_text_stats(&self, rel: &str) -> Result<Option<TextStats>, BackendError> {
        let path = paths::worktree_path(&self.toplevel, rel)?;
        if let Some(target) = link_target(&path)? {
            return Ok(Some(TextStats {
                symlink: true,
                ..TextStats::of(&target)
            }));
        }
        let mut file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(err) if is_missing(&err) => return Ok(None),
            Err(_) if path.is_dir() => return Err(is_a_directory(&path)),
            Err(err) => return Err(BackendError::io("reading", &path, err)),
        };
        let metadata = file
            .metadata()
            .map_err(|err| BackendError::io("reading", &path, err))?;
        if metadata.is_dir() {
            return Err(is_a_directory(&path));
        }
        // Chunks keep memory flat however large the file is; a binary file is measured, not
        // read to its end.
        let mut counter = TextStatsCounter::default();
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(BackendError::io("reading", &path, err)),
            };
            counter.feed(&buffer[..read]);
            if counter.is_binary() {
                return Ok(Some(counter.finish_with_len(metadata.len())));
            }
        }
        Ok(Some(counter.finish()))
    }

    fn store_append(&self, rel: &str, line: &[u8]) -> Result<(), BackendError> {
        let path = paths::store_path(&self.store_root, rel, false)?;
        if line.contains(&b'\n') {
            return Err(BackendError::InvalidLine {
                reason: "contains a newline",
            });
        }
        create_parent(&path)?;
        let mut record = Vec::with_capacity(line.len() + 1);
        record.extend_from_slice(line);
        record.push(b'\n');
        // One write of the whole line to a file opened for appending is what makes concurrent
        // writers safe without a lock (PLAN D5): the OS positions each write at the current end
        // of the file atomically (`O_APPEND` on Unix; on Windows, `append(true)` opens with
        // `FILE_APPEND_DATA` only, which does the same), so lines from other threads and
        // processes land before or after this one, never inside it. `write_all` issues a single
        // write for a regular file; it would only split it after a short write (a full disk),
        // and the fold skips a torn line (PLAN §7).
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|err| BackendError::io("opening", &path, err))?;
        file.write_all(&record)
            .map_err(|err| BackendError::io("appending to", &path, err))
    }

    fn store_read(&self, rel: &str) -> Result<Option<Vec<u8>>, BackendError> {
        let path = paths::store_path(&self.store_root, rel, false)?;
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(err) if is_missing(&err) => Ok(None),
            Err(err) => Err(BackendError::io("reading", &path, err)),
        }
    }

    fn store_list(&self, dir: &str) -> Result<Vec<String>, BackendError> {
        let path = paths::store_path(&self.store_root, dir, true)?;
        let entries = match fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(err) if is_missing(&err) => return Ok(Vec::new()),
            Err(err) => return Err(BackendError::io("listing", &path, err)),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|err| BackendError::io("listing", &path, err))?;
            let is_file = entry
                .file_type()
                .map_err(|err| BackendError::io("listing", &path, err))?
                .is_file();
            // Names that aren't UTF-8 or start with `.` (temp files mid-rename) aren't store
            // entries ([`paths::store_path`]).
            match entry.file_name().into_string() {
                Ok(name) if is_file && !name.starts_with('.') => names.push(name),
                _ => {}
            }
        }
        names.sort();
        Ok(names)
    }

    fn store_write_atomic(&self, rel: &str, bytes: &[u8]) -> Result<(), BackendError> {
        let path = paths::store_path(&self.store_root, rel, false)?;
        let dir = create_parent(&path)?;
        // A temp file in the same directory renamed over the target: the rename is atomic on
        // one filesystem, and replaces an existing target on Windows as well (tempfile's
        // `persist` uses `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING` there).
        let mut temp = tempfile::Builder::new()
            .prefix(STORE_TEMP_PREFIX)
            .tempfile_in(&dir)
            .map_err(|err| BackendError::io("creating a temp file in", &dir, err))?;
        temp.write_all(bytes)
            .and_then(|()| temp.as_file().sync_all())
            .map_err(|err| BackendError::io("writing", temp.path(), err))?;
        temp.persist(&path)
            .map_err(|err| BackendError::io("replacing", &path, err.error))?;
        Ok(())
    }

    fn store_remove(&self, rel: &str) -> Result<bool, BackendError> {
        let path = paths::store_path(&self.store_root, rel, false)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(err) if is_missing(&err) => Ok(false),
            Err(err) => Err(BackendError::io("removing", &path, err)),
        }
    }

    fn watch(&self, spec: WatchSpec, sink: WatchSink) -> Result<WatchHandle, BackendError> {
        super::watch::start(
            super::watch::Roots {
                toplevel: self.toplevel.clone(),
                git_dir: self.git_dir.clone(),
                common_dir: self.common_dir.clone(),
                store_root: self.store_root.clone(),
            },
            self.options.env.clone(),
            spec,
            sink,
        )
    }
}

/// Turns a failed `rev-parse` at open into the typed error for what `path` is.
fn classify_open_failure(
    path: &Path,
    options: &LocalBackendOptions,
    err: BackendError,
) -> BackendError {
    let shown = path.display().to_string();
    let BackendError::Git { stderr, .. } = &err else {
        return err;
    };
    // git's messages are stable under the policy's `LC_ALL=C`.
    if stderr.contains("not a git repository") {
        return BackendError::NotARepository { path: shown };
    }
    if stderr.contains("dubious ownership") {
        return BackendError::UnsafeRepository { path: shown };
    }
    if stderr.contains("must be run in a work tree") {
        let bare = command::run(
            path,
            &options.env,
            &["rev-parse", "--is-bare-repository"],
            &GitOpts::default(),
        )
        .ok()
        .and_then(|output| output.stdout_utf8().map(|text| text.trim() == "true"));
        return if bare == Some(true) {
            BackendError::BareRepository { path: shown }
        } else {
            BackendError::InsideGitDir { path: shown }
        };
    }
    err
}

/// A path git printed, in this OS's spelling: git for Windows prints `C:/x/y`, which works but
/// reads oddly next to every other Windows path and differs from the paths file watchers report.
fn native_path(text: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(text.replace('/', "\\"))
    } else {
        PathBuf::from(text)
    }
}

/// The target of the symbolic link at `path`, as the bytes git stores for a link (`/`-separated);
/// `None` when `path` isn't a link (or doesn't exist). git records a link as its target path and
/// never follows one, so the worktree methods read a link the same way: its "content" is the
/// target, whether that exists, is a file or is a directory.
fn link_target(path: &Path) -> Result<Option<Vec<u8>>, BackendError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {}
        Ok(_) => return Ok(None),
        Err(err) if is_missing(&err) => return Ok(None),
        Err(err) => return Err(BackendError::io("reading", path, err)),
    }
    let target = fs::read_link(path).map_err(|err| BackendError::io("reading", path, err))?;
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStringExt as _;
        target.into_os_string().into_vec()
    };
    // git for Windows writes link targets with `/`, as on every other OS.
    #[cfg(not(unix))]
    let bytes = target.to_string_lossy().replace('\\', "/").into_bytes();
    Ok(Some(bytes))
}

/// Whether a read failed because the path doesn't exist (also when a parent is a file, which
/// Unix reports as "not a directory").
fn is_missing(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

fn is_a_directory(path: &Path) -> BackendError {
    BackendError::io(
        "reading",
        path,
        io::Error::new(io::ErrorKind::IsADirectory, "is a directory"),
    )
}

/// Creates `path`'s parent directory and returns it.
fn create_parent(path: &Path) -> Result<PathBuf, BackendError> {
    let dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    fs::create_dir_all(&dir).map_err(|err| BackendError::io("creating", &dir, err))?;
    Ok(dir)
}
