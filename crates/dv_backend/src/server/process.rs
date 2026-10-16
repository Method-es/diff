//! The server's processes (docs/PLAN.md §18.3): `spawn`, `proc_write`, `proc_kill`, and their
//! end with the input.
//!
//! - **Output, bounded.** A process's lines wait for the writer thread against a budget of
//!   [`BUDGET`] bytes of its own ([`Budget`]): the writer gives a line's bytes back once it has
//!   written the line ([`Credit`]). With the budget spent the process's sink waits, the local
//!   backend's bounded queue fills, and the process waits on its own pipe, so a process that
//!   prints faster than the client reads costs the server a bounded amount of memory, and every
//!   other answer waits behind at most that much of it.
//! - **The end of the input** ([`Procs::close`]). The server stops taking processes, waits for
//!   every `spawn` still starting one, and kills every process it runs, also one a `proc_write`
//!   still holds (blocked on a full stdin pipe: the kill ends that write too). A `spawn` that
//!   finishes after the end kills what it started instead of keeping it, so nothing outlives
//!   the client.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use dv_core::backend::{Backend as _, ProcessEvent, ProcessHandle, ProcessSpec, process};
use parking_lot::{Condvar, Mutex};

use super::{Gate, State};
use crate::proto::{self, SpawnReply, WireError};

/// How many bytes of one process's encoded lines may wait for the writer thread.
pub(super) const BUDGET: usize = 1024 * 1024;

/// The processes a server runs, and whether it still takes new ones.
#[derive(Default)]
pub(super) struct Procs {
    state: Mutex<ProcsState>,
    /// Signalled when a `spawn` in progress ends.
    spawned: Condvar,
}

#[derive(Default)]
struct ProcsState {
    running: HashMap<u32, Proc>,
    /// The `spawn`s starting a process right now.
    spawning: usize,
    /// The input ended: no process is kept any more.
    closed: bool,
}

/// A running process: a write holds a reference to its handle while it waits on the pipe.
struct Proc {
    handle: Arc<ProcessHandle>,
    budget: Arc<Budget>,
}

impl Procs {
    /// The handle of process `process`, while it runs.
    fn handle(&self, process: u32) -> Option<Arc<ProcessHandle>> {
        let state = self.state.lock();
        state
            .running
            .get(&process)
            .map(|proc| Arc::clone(&proc.handle))
    }

    /// The input ended (module docs): kills every process and returns once none can start.
    pub(super) fn close(&self) {
        let procs: Vec<Proc> = {
            let mut state = self.state.lock();
            state.closed = true;
            while state.spawning > 0 {
                self.spawned.wait(&mut state);
            }
            state.running.drain().map(|(_, proc)| proc).collect()
        };
        for proc in &procs {
            // Its sink stops waiting for the writer, and a write it is blocked in fails.
            proc.budget.close();
            proc.handle.kill();
        }
        // Outside the lock, which a process's sink takes at its exit; a handle a write still
        // holds goes with that write, its process already dead.
        drop(procs);
    }
}

/// Forgets process `process`, which has exited. Its handle is dropped outside the lock; from
/// the process's own sink that is allowed (`ProcessHandle`'s drop).
fn forget(state: &Weak<State>, process: u32) {
    if let Some(state) = state.upgrade() {
        let proc = state.procs.state.lock().running.remove(&process);
        drop(proc);
    }
}

/// A `spawn` in progress, counted until dropped ([`Procs::close`] waits for it).
struct Spawning<'a>(&'a Procs);

impl Drop for Spawning<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock();
        state.spawning -= 1;
        self.0.spawned.notify_all();
    }
}

impl State {
    /// Starts a process and answers it: its output lines wait at the gate until the answer is
    /// queued. Once it has exited the server forgets it (module docs).
    pub(super) fn spawn(
        self: &Arc<Self>,
        id: u64,
        repo: u32,
        spec: &ProcessSpec,
    ) -> Result<(), WireError> {
        let backend = self.repo(repo)?;
        let _spawning = {
            let mut state = self.procs.state.lock();
            if state.closed {
                return Err(WireError::protocol("the backend is shutting down"));
            }
            state.spawning += 1;
            Spawning(&self.procs)
        };
        let process = self.next_proc.fetch_add(1, Ordering::Relaxed);
        let gate = Arc::new(Gate::default());
        let budget = Arc::new(Budget::default());
        // Set by the sink at the exit: a process that exits before it is kept is forgotten
        // here instead.
        let ended = Arc::new(AtomicBool::new(false));
        let sink = {
            let (gate, out, budget) = (Arc::clone(&gate), self.out.clone(), Arc::clone(&budget));
            let (ended, state) = (Arc::clone(&ended), Arc::downgrade(self));
            Box::new(move |event: ProcessEvent| {
                let line = match &event {
                    ProcessEvent::Stdout(bytes) => proto::proc_out_line(process, bytes),
                    ProcessEvent::Stderr(line) => proto::proc_err_line(process, line),
                    ProcessEvent::Exit(code) => proto::proc_exit_line(process, *code),
                };
                // `None` once the server is ending: nobody reads the line any more.
                if let Some(credit) = budget.take(line.len()) {
                    gate.deliver(line, Some(credit), &out);
                }
                if let ProcessEvent::Exit(_) = event {
                    ended.store(true, Ordering::SeqCst);
                    forget(&state, process);
                }
            })
        };
        let handle = Arc::new(
            backend
                .spawn_stdio(spec, sink)
                .map_err(|error| WireError::from(&error))?,
        );
        let reply = SpawnReply {
            process,
            pid: handle.pid(),
        };
        {
            let mut state = self.procs.state.lock();
            if state.closed {
                drop(state);
                // The input ended while it started: nobody will hear of it (module docs).
                budget.close();
                handle.kill();
                return Ok(());
            }
            state.running.insert(
                process,
                Proc {
                    handle: Arc::clone(&handle),
                    budget,
                },
            );
        }
        if ended.load(Ordering::SeqCst) {
            forget(&Arc::downgrade(self), process);
        }
        self.send(proto::ok_line(id, &reply));
        gate.open(&self.out);
        Ok(())
    }

    /// `proc_write`: once written, or an error once the process has ended.
    pub(super) fn write(&self, process: u32, bytes: &[u8]) -> Result<(), WireError> {
        // Cloned out, so a write waiting on a full pipe holds no lock.
        let handle = self
            .procs
            .handle(process)
            .ok_or_else(|| WireError::from(&process::ended(&format!("process {process}"))))?;
        handle.write(bytes).map_err(|error| WireError::from(&error))
    }

    /// `proc_kill`: also for a process that has ended.
    pub(super) fn kill(&self, process: u32) {
        if let Some(handle) = self.procs.handle(process) {
            handle.kill();
        }
    }
}

/// One process's budget of bytes waiting for the writer thread (module docs).
#[derive(Default)]
pub(super) struct Budget {
    state: Mutex<BudgetState>,
    freed: Condvar,
}

#[derive(Default)]
struct BudgetState {
    queued: usize,
    /// The server is ending: nothing waits any more, and nothing more is queued.
    closed: bool,
}

impl Budget {
    /// Waits until `bytes` more fit the budget (a line always fits when nothing waits), and
    /// counts them until the [`Credit`] is dropped; `None` once closed.
    pub(super) fn take(self: &Arc<Self>, bytes: usize) -> Option<Credit> {
        let mut state = self.state.lock();
        while !state.closed && state.queued > 0 && state.queued + bytes > BUDGET {
            self.freed.wait(&mut state);
        }
        if state.closed {
            return None;
        }
        state.queued += bytes;
        Some(Credit {
            budget: Arc::clone(self),
            bytes,
        })
    }

    fn close(&self) {
        self.state.lock().closed = true;
        self.freed.notify_all();
    }

    /// The bytes waiting now.
    #[cfg(test)]
    pub(super) fn queued(&self) -> usize {
        self.state.lock().queued
    }
}

/// Bytes counted against a [`Budget`] until dropped: the writer drops it once the line is
/// written, and a line nobody writes (the output broke) is dropped with it.
pub(super) struct Credit {
    budget: Arc<Budget>,
    bytes: usize,
}

impl Drop for Credit {
    fn drop(&mut self) {
        let mut state = self.budget.state.lock();
        state.queued -= self.bytes;
        self.budget.freed.notify_all();
    }
}
