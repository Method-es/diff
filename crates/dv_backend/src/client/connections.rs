//! [`Connections`]: the app's registry of connections (docs/PLAN.md §5.2, §11), one per WSL
//! distro and one per named loopback server, each made the first time it is asked for and
//! shared by every repository of that server from then on. The registry builds no WSL
//! launcher itself: the app passes the one its WSL access gives (dv_ui's `WslAccess`, the one
//! place that keeps automation runs and tests out of real distros). The CLI never uses it: a
//! WSL repository's command runs inside its distro instead (PLAN §10.4).

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use parking_lot::Mutex;

use super::Connection;
use crate::launch::Launcher;

/// Which server a connection reaches.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ConnectionKey {
    /// A WSL distro's server. Distro names compare without case, as `wsl.exe` takes them, so
    /// `\\wsl$\ubuntu` and `\\wsl.localhost\Ubuntu` share one: [`ConnectionKey::wsl`] folds it.
    Wsl(String),
    /// A loopback server on this machine, by name (tests and the e2e scenarios).
    Loopback(String),
}

impl ConnectionKey {
    /// The key of `distro`'s server, its name folded to lowercase.
    pub fn wsl(distro: &str) -> Self {
        ConnectionKey::Wsl(distro.to_lowercase())
    }

    /// The key of the loopback server named `name`.
    pub fn loopback(name: impl Into<String>) -> Self {
        ConnectionKey::Loopback(name.into())
    }
}

/// The app's connections by [`ConnectionKey`] (module docs). Cloning shares the registry.
#[derive(Clone, Default)]
pub struct Connections {
    map: Arc<Mutex<BTreeMap<ConnectionKey, Arc<Connection>>>>,
}

impl Connections {
    pub fn new() -> Self {
        Self::default()
    }

    /// The connection of `key`, when one was made.
    pub fn get(&self, key: &ConnectionKey) -> Option<Arc<Connection>> {
        self.map.lock().get(key).cloned()
    }

    /// The connection of `key`, made with `make` the first time; nothing starts until it is
    /// used.
    pub fn get_or_insert_with(
        &self,
        key: ConnectionKey,
        make: impl FnOnce() -> Arc<Connection>,
    ) -> Arc<Connection> {
        Arc::clone(self.map.lock().entry(key).or_insert_with(make))
    }

    /// The loopback connection named `name`, made the first time with the server `launcher`
    /// starts.
    pub fn loopback(&self, name: &str, launcher: impl FnOnce() -> Launcher) -> Arc<Connection> {
        self.get_or_insert_with(ConnectionKey::loopback(name), || {
            Connection::new(name, launcher())
        })
    }

    /// Every connection made, by key: WSL distros first, then loopbacks, each by name.
    pub fn all(&self) -> Vec<Arc<Connection>> {
        self.map.lock().values().cloned().collect()
    }
}

impl fmt::Debug for Connections {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.map.lock().values()).finish()
    }
}
