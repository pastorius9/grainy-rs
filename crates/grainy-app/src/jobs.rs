//! Background jobs: one service for everything that must not run on the window's thread (imports, exports,
//! listings, automatic buttons, cache writes), with progress, cancellation and the status line. Port of the
//! oracle's `spawn(work, ready, failed, pool)` with `Worker` / `job_finished` (luma/app.py).
//!
//! A job is `work` (runs on its own thread, gets a [`JobHandle`] for progress and cancellation) and `done` (runs
//! on the window's thread with the application context, with the result). The previews and thumbnails of the open
//! list are the render worker's two lanes ([`crate::render::Renderer`]): the newest preview replaces a waiting one
//! and a thumbnail replaces the waiting one of the same photo, which a general queue cannot do; their activity is
//! part of the status line through [`Jobs::set_render_busy`].

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use crate::render::Waker;

/// A job's number.
pub type JobId = u64;

/// How far a job is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    /// What it is doing now (already translated).
    pub text: String,
}

/// What a job's work gets: progress reports and the cancel flag.
#[derive(Clone)]
pub struct JobHandle {
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<Progress>>,
    waker: Option<Waker>,
}

impl JobHandle {
    /// Whether the job was cancelled (work should stop at the next safe point).
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// The cancel flag, for functions that take one.
    pub fn cancel_flag(&self) -> &AtomicBool {
        &self.cancel
    }

    /// Reports progress (the status line shows it).
    pub fn progress(&self, done: usize, total: usize, text: impl Into<String>) {
        if let Ok(mut p) = self.progress.lock() {
            *p = Progress { done, total, text: text.into() };
        }
        if let Some(waker) = &self.waker {
            waker();
        }
    }
}

type Payload = Result<Box<dyn Any + Send>, String>;
type Done<C> = Box<dyn FnOnce(&mut C, Payload)>;

struct Running<C> {
    kind: &'static str,
    label: String,
    handle: JobHandle,
    done: Done<C>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// The job service of a context `C` (the application context).
pub struct Jobs<C> {
    next: JobId,
    running: BTreeMap<JobId, Running<C>>,
    tx: Sender<(JobId, Payload)>,
    rx: Receiver<(JobId, Payload)>,
    deferred: Vec<(JobId, Payload)>,
    waker: Option<Waker>,
    render_busy: bool,
}

impl<C> Default for Jobs<C> {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self { next: 0, running: BTreeMap::new(), deferred: Vec::new(), tx, rx, waker: None, render_busy: false }
    }
}

/// A finished job's `done`, bound to its result: run it with the context.
pub type Finished<C> = Box<dyn FnOnce(&mut C)>;

impl<C: 'static> Jobs<C> {
    /// Sets the function called when a job finishes or reports progress (the window repaints).
    pub fn set_waker(&mut self, waker: Option<Waker>) {
        self.waker = waker;
    }

    /// Starts a job of `kind` (a short name: `import`, `export`, `auto`…) shown as `label`.
    pub fn spawn<T: Send + 'static>(
        &mut self,
        kind: &'static str,
        label: impl Into<String>,
        work: impl FnOnce(&JobHandle) -> Result<T, String> + Send + 'static,
        done: impl FnOnce(&mut C, Result<T, String>) + 'static,
    ) -> JobId {
        self.next += 1;
        let id = self.next;
        let handle = JobHandle {
            cancel: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(Mutex::new(Progress::default())),
            waker: self.waker.clone(),
        };
        let tx = self.tx.clone();
        let worker = handle.clone();
        let waker = self.waker.clone();
        let run = move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&worker)))
                .unwrap_or_else(|_| Err("internal error".to_owned()))
                .map(|value| Box::new(value) as Box<dyn Any + Send>);
            let _ = tx.send((id, result));
            if let Some(waker) = waker {
                waker();
            }
        };
        let done: Done<C> = Box::new(move |cx, payload| {
            let result = payload.and_then(|b| b.downcast::<T>().map(|t| *t).map_err(|_| "job result".to_owned()));
            done(cx, result);
        });
        let thread = match std::thread::Builder::new().name(format!("grainy-{kind}")).spawn(run) {
            Ok(thread) => Some(thread),
            Err(error) => {
                let _ = self.tx.send((id, Err(error.to_string())));
                None
            }
        };
        self.running.insert(id, Running { kind, label: label.into(), handle, done, thread });
        id
    }

    /// The finished jobs, to run with the context (`job_finished`).
    pub fn poll(&mut self) -> Vec<Finished<C>> {
        self.poll_kind(None)
    }

    pub(crate) fn poll_kind(&mut self, kind: Option<&str>) -> Vec<Finished<C>> {
        let mut out: Vec<Finished<C>> = Vec::new();
        let mut ready = std::mem::take(&mut self.deferred);
        ready.extend(self.rx.try_iter());
        for (id, payload) in ready {
            if kind.is_some_and(|k| self.running.get(&id).is_some_and(|j| j.kind != k)) {
                self.deferred.push((id, payload));
                continue;
            }
            if let Some(job) = self.running.remove(&id) {
                let done = job.done;
                out.push(Box::new(move |cx: &mut C| done(cx, payload)));
            }
        }
        out
    }

    /// Waits for this kind of worker to release its files, leaving delivery to the normal poll.
    pub(crate) fn join_kind(&mut self, kind: &str) {
        for job in self.running.values_mut().filter(|j| j.kind == kind) {
            if let Some(thread) = job.thread.take() {
                let _ = thread.join();
            }
        }
    }

    /// Asks a job to stop.
    pub fn cancel(&self, id: JobId) {
        if let Some(job) = self.running.get(&id) {
            job.handle.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Asks every job of `kind` to stop.
    pub fn cancel_kind(&self, kind: &str) {
        for job in self.running.values().filter(|j| j.kind == kind) {
            job.handle.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Whether a job of `kind` runs.
    pub fn running(&self, kind: &str) -> bool {
        self.running.values().any(|j| j.kind == kind)
    }

    /// Whether nothing runs (the render lanes included).
    pub fn idle(&self) -> bool {
        self.running.is_empty() && !self.render_busy
    }

    /// Whether no job runs (the render lanes not counted).
    pub fn jobs_idle(&self) -> bool {
        self.running.is_empty()
    }

    /// The render worker's state, for the status line.
    pub fn set_render_busy(&mut self, busy: bool) {
        self.render_busy = busy;
    }

    /// The status line of the running jobs: the oldest job's label and progress, and how many more run.
    pub fn status(&self) -> Option<String> {
        let (_, first) = self.running.iter().find(|(_, j)| !j.label.is_empty())?;
        let progress = first.handle.progress.lock().map(|p| p.clone()).unwrap_or_default();
        let mut text = if !progress.text.is_empty() {
            progress.text.clone()
        } else if progress.total > 0 {
            format!("{} {}/{}", first.label, progress.done, progress.total)
        } else {
            first.label.clone()
        };
        let others = self.running.values().filter(|j| !j.label.is_empty()).count() - 1;
        if others > 0 {
            text.push_str(&format!("  ·  +{others}"));
        }
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait(jobs: &mut Jobs<Vec<String>>, cx: &mut Vec<String>) {
        let started = Instant::now();
        while !jobs.jobs_idle() {
            for f in jobs.poll() {
                f(cx);
            }
            assert!(started.elapsed() < Duration::from_secs(20));
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn joins_publication_before_library_unlock() {
        let mut jobs = Jobs::<()>::default();
        let published = Arc::new(AtomicBool::new(false));
        let worker = Arc::clone(&published);
        jobs.spawn(
            "files",
            "",
            move |_| {
                worker.store(true, Ordering::SeqCst);
                Ok(())
            },
            |_, _| {},
        );
        jobs.join_kind("files");
        assert!(published.load(Ordering::SeqCst));
        assert_eq!(jobs.poll().len(), 1);
    }

    #[test]
    fn results_progress_cancel_and_status() {
        let mut jobs: Jobs<Vec<String>> = Jobs::default();
        let mut cx = Vec::new();
        jobs.spawn(
            "a",
            "더하기",
            |_| Ok(2 + 2),
            |cx: &mut Vec<String>, r: Result<i32, String>| cx.push(format!("{r:?}")),
        );
        jobs.spawn("b", "", |_| Err::<(), _>("안 됨".into()), |cx: &mut Vec<String>, r| cx.push(format!("{r:?}")));
        jobs.spawn(
            "c",
            "",
            |_| -> Result<(), String> { panic!("boom") },
            |cx: &mut Vec<String>, r| cx.push(format!("{r:?}")),
        );
        wait(&mut jobs, &mut cx);
        cx.sort();
        assert_eq!(cx, ["Err(\"internal error\")", "Err(\"안 됨\")", "Ok(4)"]);

        let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
        let id = jobs.spawn(
            "long",
            "가져오기",
            move |h| {
                h.progress(1, 3, "");
                let _ = go_rx.recv();
                Ok(h.cancelled())
            },
            |cx: &mut Vec<String>, r: Result<bool, String>| cx.push(format!("cancelled {r:?}")),
        );
        let started = Instant::now();
        while jobs.status().as_deref() != Some("가져오기 1/3") {
            assert!(started.elapsed() < Duration::from_secs(20));
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(jobs.running("long") && !jobs.idle());
        jobs.cancel(id);
        go_tx.send(()).unwrap();
        cx.clear();
        wait(&mut jobs, &mut cx);
        assert_eq!(cx, ["cancelled Ok(true)"]);
        assert!(jobs.status().is_none() && jobs.idle());
    }
}
