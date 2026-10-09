//! This app's Linux builds and their install inside a WSL distro (docs/PLAN.md §5.2, §13.2).
//!
//! **The builds** ship beside `dv.exe`, not embedded in it: `dv-linux-x86_64` and
//! `dv-linux-aarch64` (`script/build-linux` makes them), in `DV_LINUX_DIR` when that is set,
//! else in the folder of this executable ([`linux_dir`]). A build's hash is the lowercase hex
//! sha256 of its file, computed once per process per file (cached by path, length and
//! modification time, [`LinuxBuild::at`]); `hello` sends it and the server confirms it by
//! hashing its own executable.
//!
//! **The install layout** in the distro's home: `~/.local/share/dv/bin/dv-<hash>`, and
//! `~/.local/bin/dv` a symbolic link to the build installed last, so the dv on the distro's
//! `PATH` is this app's. [`install`] streams a build over `wsl.exe`'s stdin into a temporary
//! file beside its final name, checks its sha256 and renames it into place, then points the
//! link at it (a temporary link renamed over the old one). The link is a convenience: the
//! app's launch and the CLI's run use the build itself, so a link that can't be made (a
//! directory at `~/.local/bin/dv`, a `~/.local/bin` that isn't a writable folder) is a warning,
//! never a failed install. [`prune`] removes the other `dv-*` builds of that folder (a running server keeps
//! its deleted file) and never touches `~/.local/share/dv`'s other folders. [`probe`] reads
//! the state of all this without changing it (`dv setup wsl`).

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use parking_lot::Mutex;

use crate::wsl::{self, Arch, Input, Shell, WslError, select_build};

/// The variable naming the folder of this app's Linux builds (a Windows path).
pub const LINUX_DIR_VAR: &str = "DV_LINUX_DIR";

/// The variable naming the home dv installs into and runs from inside a distro (a POSIX path):
/// the sandbox of tests and agents' runs, never set by the owner's app.
pub const WSL_HOME_VAR: &str = "DV_WSL_HOME";

/// How long an install may take: a cold distro's boot and a few MB through `wsl.exe`.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(120);

/// How long a prune or a probe may take.
const SCRIPT_TIMEOUT: Duration = Duration::from_secs(60);

/// Copies stdin into place as `dv-<$1>` (module docs) and prints `dv-installed <path>`, after
/// `dv-linked <link>` once `~/.local/bin/dv` runs it; exit 3 (`dv-corrupt <its sha256>`) when
/// the copy isn't that build, nothing installed. Lines starting `dv:` on stderr are warnings:
/// a link that couldn't be made, which leaves the install done.
pub const INSTALL: &str = r#"d="$HOME/.local/share/dv/bin"
l="$HOME/.local/bin/dv"
mkdir -p "$d" || exit 1
t="$d/.dv-$1.$$"
trap 'rm -f "$t"' EXIT
cat > "$t" || exit 1
chmod 755 "$t" || exit 1
s=$(sha256sum < "$t") || exit 1
s=${s%% *}
if [ "$s" != "$1" ]; then echo "dv-corrupt $s"; exit 3; fi
mv -f "$t" "$d/dv-$1" || exit 1
if [ -d "$l" ]; then
  echo "dv: $l is a directory, so it doesn't run this dv; it was left alone" >&2
elif e=$({ mkdir -p "$HOME/.local/bin" && ln -sfn "$d/dv-$1" "$l.$$" && mv -f "$l.$$" "$l"; } 2>&1); then
  echo "dv-linked $l"
else
  rm -f "$l.$$" 2> /dev/null
  e=$(printf '%s' "$e" | tr '\n' ' ')
  echo "dv: couldn't point $l at this dv ($e); it was left as it was" >&2
fi
echo "dv-installed $d/dv-$1"
"#;

/// Removes `~/.local/share/dv/bin/dv-*` but `dv-<$1>`, `dv-<$2>` and the build `~/.local/bin/dv`
/// links to (another app's install may have pointed it there since), and temporary copies an
/// install left more than an hour ago, printing `dv-pruned <name>` for each build removed.
pub const PRUNE: &str = r#"d="$HOME/.local/share/dv/bin"
[ -d "$d" ] || exit 0
k=$(readlink "$HOME/.local/bin/dv" 2> /dev/null)
for f in "$d"/dv-*; do
  [ -e "$f" ] || [ -L "$f" ] || continue
  n=${f##*/}
  if [ "$n" != "dv-$1" ] && [ "$n" != "dv-$2" ] && [ "$f" != "$k" ]; then
    rm -f "$f" && echo "dv-pruned $n"
  fi
done
find "$d" -maxdepth 1 -name '.dv-*' -mmin +60 -exec rm -f {} + 2> /dev/null
exit 0
"#;

/// Reports, without changing anything, the distro's architecture and home, whether this app's
/// build for it is installed and intact, and where `~/.local/bin/dv` points. Args: the x86_64
/// and aarch64 build hashes.
pub const PROBE: &str = concat!(
    "say() { echo \"$*\"; }\n",
    select_build!(),
    r#"say "dv-arch $a"
say "dv-home $HOME"
if [ -f "$b" ] && [ -x "$b" ]; then
  s=$(sha256sum < "$b"); s=${s%% *}
  if [ "$s" = "$h" ]; then say "dv-build ok"; else say "dv-build corrupt"; fi
else
  say "dv-build missing"
fi
l="$HOME/.local/bin/dv"
if [ -L "$l" ] && [ "$(readlink "$l")" = "$b" ]; then say "dv-link ok"
elif [ -d "$l" ]; then say "dv-link directory"
elif [ -e "$l" ] || [ -L "$l" ]; then say "dv-link other"
else say "dv-link none"; fi
"#
);

/// The folder of this app's Linux builds: `dv_linux_dir` (the value of [`LINUX_DIR_VAR`]) when
/// it is set, else this executable's folder.
pub fn linux_dir(dv_linux_dir: Option<String>) -> PathBuf {
    if let Some(dir) = dv_linux_dir.filter(|dir| !dir.trim().is_empty()) {
        return PathBuf::from(dir.trim());
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_default()
}

/// One Linux build of this app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxBuild {
    pub arch: Arch,
    pub path: PathBuf,
    /// The build hash: the file's lowercase hex sha256.
    pub hash: String,
    pub len: u64,
}

impl LinuxBuild {
    /// The build for `arch` in `dir`, `None` when its file isn't there. Hashing reads the file
    /// once per process while its length and modification time stay the same.
    pub fn at(dir: &Path, arch: Arch) -> Result<Option<Self>, WslError> {
        type Hashes = HashMap<PathBuf, (u64, Option<SystemTime>, String)>;
        static HASHES: Mutex<Option<Hashes>> = Mutex::new(None);
        let path = dir.join(arch.file_name());
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => return Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(read_error(&path, &error)),
        };
        let len = metadata.len();
        let modified = metadata.modified().ok();
        if let Some((cached_len, cached_modified, hash)) =
            HASHES.lock().get_or_insert_default().get(&path)
            && *cached_len == len
            && *cached_modified == modified
        {
            return Ok(Some(Self {
                arch,
                path,
                hash: hash.clone(),
                len,
            }));
        }
        let hash = crate::build::sha256_file(&path).map_err(|error| read_error(&path, &error))?;
        HASHES
            .lock()
            .get_or_insert_default()
            .insert(path.clone(), (len, modified, hash.clone()));
        Ok(Some(Self {
            arch,
            path,
            hash,
            len,
        }))
    }
}

fn read_error(path: &Path, error: &std::io::Error) -> WslError {
    WslError::Failed(format!("couldn't read {}: {error}", path.display()))
}

/// This app's Linux builds (module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxBuilds {
    /// Where they are looked for.
    pub dir: PathBuf,
    builds: Vec<LinuxBuild>,
}

impl LinuxBuilds {
    /// The builds in `dir`, hashed (blocks while a file is read the first time).
    pub fn find(dir: &Path) -> Result<Self, WslError> {
        let mut builds = Vec::new();
        for arch in Arch::ALL {
            if let Some(build) = LinuxBuild::at(dir, arch)? {
                builds.push(build);
            }
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            builds,
        })
    }

    pub fn get(&self, arch: Arch) -> Option<&LinuxBuild> {
        self.builds.iter().find(|build| build.arch == arch)
    }

    pub fn is_empty(&self) -> bool {
        self.builds.is_empty()
    }

    /// The x86_64 and aarch64 hashes as the scripts take them, `-` for a build missing.
    pub fn hash_args(&self) -> [String; 2] {
        Arch::ALL.map(|arch| {
            self.get(arch)
                .map_or_else(|| "-".to_string(), |build| build.hash.clone())
        })
    }

    /// The error for `arch`'s build missing: which file, and what makes it.
    pub fn missing(&self, arch: Arch) -> WslError {
        WslError::NoBuild {
            arch,
            path: self.dir.join(arch.file_name()),
        }
    }
}

/// What an install did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// The build's path inside the distro.
    pub path: String,
    /// Whether `~/.local/bin/dv` links to it now (module docs: a link that couldn't be made
    /// is a warning).
    pub linked: bool,
    /// What the install warned about: `~/.local/bin/dv` being a directory, or a link that
    /// couldn't be made.
    pub warnings: Vec<String>,
}

/// Installs `build` where `shell` runs (module docs), `progress` hearing the bytes copied.
/// Blocks; never call it on a UI thread.
pub fn install(
    shell: &Shell,
    build: &LinuxBuild,
    progress: &dyn Fn(u64, u64),
) -> Result<Installed, WslError> {
    let mut file = File::open(&build.path).map_err(|error| read_error(&build.path, &error))?;
    let output = wsl::run(
        shell.command(INSTALL, &[&build.hash]),
        Some(Input {
            reader: &mut file,
            len: build.len,
            progress,
        }),
        INSTALL_TIMEOUT,
    )?;
    let stdout = wsl::decode(&output.stdout);
    let warnings: Vec<String> = wsl::decode(&output.stderr)
        .lines()
        .filter_map(|line| line.trim().strip_prefix("dv: "))
        .map(str::to_string)
        .collect();
    if output.status.success()
        && let Some(path) = stdout
            .lines()
            .find_map(|line| line.trim().strip_prefix("dv-installed "))
    {
        return Ok(Installed {
            path: path.to_string(),
            linked: stdout
                .lines()
                .any(|line| line.trim().starts_with("dv-linked ")),
            warnings,
        });
    }
    if output.status.code() == Some(3)
        && let Some(actual) = stdout
            .lines()
            .find_map(|line| line.trim().strip_prefix("dv-corrupt "))
    {
        return Err(WslError::Failed(format!(
            "the copy of {} in {} hashed to {actual}, not {}: nothing was installed",
            build.path.display(),
            shell.name(),
            build.hash
        )));
    }
    Err(WslError::Failed(format!(
        "installing dv in {} failed: {}",
        shell.name(),
        wsl::words(&output)
    )))
}

/// Removes the builds in the distro's `~/.local/share/dv/bin` that aren't `keep`'s (module
/// docs); the names removed.
pub fn prune(shell: &Shell, keep: &LinuxBuilds) -> Result<Vec<String>, WslError> {
    let hashes = keep.hash_args();
    let output = shell.run(PRUNE, &[&hashes[0], &hashes[1]], SCRIPT_TIMEOUT)?;
    if !output.status.success() {
        return Err(WslError::Failed(format!(
            "pruning old builds of dv in {} failed: {}",
            shell.name(),
            wsl::words(&output)
        )));
    }
    Ok(wsl::decode(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().strip_prefix("dv-pruned "))
        .map(str::to_string)
        .collect())
}

/// [`prune`] in the background, once per process for each distro and home (PLAN §5.2: after
/// the first successful handshake), best effort.
pub fn prune_once(shell: &Shell, keep: &LinuxBuilds) {
    /// A distro (folded to lowercase) and the home dv runs from there.
    type Key = (String, Option<String>);
    static PRUNED: Mutex<Option<HashSet<Key>>> = Mutex::new(None);
    let key = (
        shell.name().to_lowercase(),
        shell.home().map(str::to_string),
    );
    if !PRUNED.lock().get_or_insert_default().insert(key) {
        return;
    }
    let shell = shell.clone();
    let keep = keep.clone();
    let started = std::thread::Builder::new()
        .name("dv-wsl-prune".into())
        .spawn(move || match prune(&shell, &keep) {
            Ok(pruned) if !pruned.is_empty() => {
                log::info!("removed old builds of dv in {}: {pruned:?}", shell.name());
            }
            Ok(_) => {}
            Err(error) => log::warn!("{error}"),
        });
    if let Err(error) = started {
        log::warn!("couldn't start pruning old builds of dv: {error}");
    }
}

/// The state of this app's install in a distro ([`probe`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub arch: Arch,
    /// The home dv installs into there.
    pub home: String,
    /// Where this app's build for `arch` is, or goes.
    pub path: String,
    pub build: BuildState,
    pub link: LinkState,
}

impl Probe {
    /// Whether this app's build is installed, intact, and `~/.local/bin/dv` runs it (or can't,
    /// being a directory, which an install leaves alone too).
    pub fn up_to_date(&self) -> bool {
        self.build == BuildState::Ok && matches!(self.link, LinkState::Ok | LinkState::Directory)
    }

    /// `~/.local/bin/dv` there.
    pub fn link_path(&self) -> String {
        format!("{}/.local/bin/dv", self.home)
    }
}

/// This app's build in the distro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildState {
    /// Installed and hashing to the build.
    Ok,
    /// There, but not this build's bytes.
    Corrupt,
    Missing,
}

/// What `~/.local/bin/dv` is in the distro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    /// A link to this app's build.
    Ok,
    /// A directory, which an install leaves alone.
    Directory,
    /// Something else: another build, or the first implementation's file.
    Other,
    /// Nothing.
    None,
}

/// Reads the state of this app's install where `shell` runs ([`PROBE`]), changing nothing.
/// Boots a stopped distro.
pub fn probe(shell: &Shell, builds: &LinuxBuilds) -> Result<Probe, WslError> {
    if builds.is_empty() {
        return Err(builds.missing(Arch::host()));
    }
    let hashes = builds.hash_args();
    let output = shell.run(PROBE, &[&hashes[0], &hashes[1]], SCRIPT_TIMEOUT)?;
    let stdout = wsl::decode(&output.stdout);
    let mut said: HashMap<&str, &str> = HashMap::new();
    for line in stdout.lines() {
        if let Some((key, value)) = line.trim().split_once(' ') {
            said.insert(key, value);
        }
    }
    match (
        output.status.code(),
        said.get("dv-arch"),
        said.get("dv-no-build"),
    ) {
        (Some(121), Some(machine), _) => return Err(WslError::UnknownArch(machine.to_string())),
        (Some(122), _, Some(arch)) => {
            if let Some(arch) = Arch::from_name(arch) {
                return Err(builds.missing(arch));
            }
        }
        _ => {}
    }
    let parsed = (|| {
        let arch = Arch::from_name(said.get("dv-arch")?)?;
        let home = said.get("dv-home")?.to_string();
        let build = match *said.get("dv-build")? {
            "ok" => BuildState::Ok,
            "corrupt" => BuildState::Corrupt,
            "missing" => BuildState::Missing,
            _ => return None,
        };
        let link = match *said.get("dv-link")? {
            "ok" => LinkState::Ok,
            "directory" => LinkState::Directory,
            "other" => LinkState::Other,
            "none" => LinkState::None,
            _ => return None,
        };
        let hash = &builds.get(arch)?.hash;
        Some(Probe {
            arch,
            path: format!("{home}/.local/share/dv/bin/dv-{hash}"),
            home,
            build,
            link,
        })
    })();
    match parsed {
        Some(probe) if output.status.success() => Ok(probe),
        _ => Err(WslError::Failed(format!(
            "checking dv in {} failed: {}",
            shell.name(),
            wsl::words(&output)
        ))),
    }
}

#[cfg(test)]
mod tests;
