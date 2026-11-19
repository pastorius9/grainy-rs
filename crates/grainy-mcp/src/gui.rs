//! Port of `mcp_service.GuiCall`: requests wait for the GUI thread, which runs them when it gets to them
//! ([`GuiQueue::pump`] from its event loop). The waiting rules are the oracle's: a call not started within
//! `QUEUE_SECONDS` (or the deadline) is cancelled and never runs (Busy); a started call may finish up to
//! `GRACE_SECONDS` after the deadline, else the request gets `Unfinished`; closing the queue answers every waiting
//! call with Busy, and later calls too.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::host::{CallError, Gui, GuiTask, Host};
use crate::service::{CLOSED, GRACE_SECONDS, QUEUE_SECONDS};
use crate::util::lock;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Waiting,
    Running,
    Done,
    Cancelled,
    Closed,
}

struct Pending {
    task: Mutex<Option<GuiTask>>,
    state: Mutex<State>,
    changed: Condvar,
}

impl Pending {
    fn set(&self, state: State) {
        *lock(&self.state) = state;
        self.changed.notify_all();
    }

    /// Wait until `done(state)` or the timeout; the state then.
    fn wait(&self, seconds: f64, done: impl Fn(State) -> bool) -> State {
        let until = Instant::now() + Duration::from_secs_f64(seconds.max(0.0));
        let mut state = lock(&self.state);
        while !done(*state) {
            let now = Instant::now();
            if now >= until {
                break;
            }
            state = self.changed.wait_timeout(state, until - now).map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
        }
        *state
    }
}

/// The GUI thread's queue of calls from request threads (one per server start).
pub struct GuiQueue {
    pending: Mutex<(bool, VecDeque<Arc<Pending>>)>,
    wake: Box<dyn Fn() + Send + Sync>,
    queue_seconds: f64,
    grace_seconds: f64,
}

impl GuiQueue {
    /// A queue that calls `wake` when a request arrives (the application wakes its event loop there).
    pub fn new(wake: Box<dyn Fn() + Send + Sync>) -> Self {
        Self::with_limits(wake, QUEUE_SECONDS, GRACE_SECONDS)
    }

    /// The same with other waiting limits (tests).
    pub fn with_limits(wake: Box<dyn Fn() + Send + Sync>, queue_seconds: f64, grace_seconds: f64) -> Self {
        Self { pending: Mutex::new((false, VecDeque::new())), wake, queue_seconds, grace_seconds }
    }

    /// GUI thread: run the calls that are waiting, in order.
    pub fn pump(&self, host: &mut dyn Host) -> usize {
        let mut count = 0;
        loop {
            let Some(item) = lock(&self.pending).1.pop_front() else { return count };
            {
                let mut state = lock(&item.state);
                if *state != State::Waiting {
                    continue;
                }
                *state = State::Running;
            }
            if let Some(task) = lock(&item.task).take() {
                task(host);
                count += 1;
            }
            item.set(State::Done);
        }
    }

    /// GUI thread: every waiting caller gets Busy, and so does every later call (`GuiCall.close`).
    pub fn close(&self) {
        let mut pending = lock(&self.pending);
        pending.0 = true;
        for item in pending.1.drain(..) {
            let mut state = lock(&item.state);
            if *state == State::Waiting {
                *state = State::Closed;
                item.changed.notify_all();
            }
        }
    }

    /// Whether the queue was closed.
    pub fn closed(&self) -> bool {
        lock(&self.pending).0
    }
}

impl Gui for GuiQueue {
    fn call(&self, task: GuiTask, deadline: Instant) -> Result<(), CallError> {
        let item = Arc::new(Pending {
            task: Mutex::new(Some(task)),
            state: Mutex::new(State::Waiting),
            changed: Condvar::new(),
        });
        {
            let mut pending = lock(&self.pending);
            if pending.0 {
                return Err(CallError::Busy(CLOSED.into()));
            }
            pending.1.push_back(item.clone());
        }
        (self.wake)();
        let left = deadline.saturating_duration_since(Instant::now()).as_secs_f64();
        let state = item.wait(left.min(self.queue_seconds), |s| matches!(s, State::Done | State::Closed));
        match state {
            State::Done => return Ok(()),
            State::Closed => return Err(CallError::Busy(CLOSED.into())),
            State::Waiting => {
                let mut s = lock(&item.state);
                if *s == State::Waiting {
                    *s = State::Cancelled;
                    return Err(CallError::Busy("Grainy did not get to this call in time (it may be busy)".into()));
                }
            }
            _ => {}
        }
        let grace = (deadline.saturating_duration_since(Instant::now()).as_secs_f64() + self.grace_seconds).max(0.1);
        match item.wait(grace, |s| matches!(s, State::Done | State::Closed)) {
            State::Done => Ok(()),
            State::Closed => Err(CallError::Busy(CLOSED.into())),
            _ => Err(CallError::Unfinished),
        }
    }
}
