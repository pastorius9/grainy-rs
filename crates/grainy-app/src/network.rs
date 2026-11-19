//! The real network of feedback and updates (HANDOFF2 A): the implementations of the traits package 31 left with
//! a "not available in this build" answer, on the HTTP client of `grainy_platform::net` (ureq with the operating
//! system's TLS).
//!
//! * [`HttpTransport`] (`dialogs::feedback::Transport`): the feedback form post, only when 보내기 is pressed.
//! * [`http_opener`] (`commands::settings::UpdateOpener`): one GET of the Rust build's own release feed
//!   (`updater::RS_FEED`, repository `pastorius9/grainy-rs`; the Python Grainy's feed is never read).
//! * [`NetworkBackend`] (`dialogs::update::Backend`): the download of a release, checked against its size and
//!   SHA-256, staged (Windows: the zip unpacked; macOS: the disk image attached read-only and its `Grainy RS.app`
//!   checked and copied, `updater::stage_disk_image`), and the start of the new version's installer step. It
//!   refuses any release that is not this Rust build's file for this system ([`updater::RUST_MARKER`]), so a Python
//!   Grainy installer is never downloaded or started.
//!
//! **The switch.** `--offline` or `GRAINY_OFFLINE=1` (any value except empty and `0`) switches every network use
//! of the program off: [`install`] then installs nothing (the package 31 defaults answer "not available"), and
//! the client itself refuses every request ([`grainy_platform::net::offline`]). Only the program's start
//! ([`crate::GrainyApp::from_arguments`]) calls [`install`]; tests never do, so a test can reach the network only
//! by building one of these types itself, and while the switch is on even that opens nothing.
//!
//! `--finish-update` ([`run_finish`]): the new version, started from its staged folder (on macOS its staged
//! bundle), replaces the old one after it closed (luma/updater.py `run_finish`, without its progress window).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use grainy_platform::feedback::{Request, SendFailure};
use grainy_platform::net::{self, Failure};
use grainy_platform::updater::{self, Layout, Release};
use grainy_platform::BUILD_VERSION;

use crate::commands::settings::{UpdateOpener, UpdateState};
use crate::context::AppContext;
use crate::dialogs::feedback::{FeedbackState, Transport};
use crate::dialogs::update::{Backend, UpdateEnv};
use crate::startup::{tr, unavailable};

/// Whether the network is switched off (`--offline`, `GRAINY_OFFLINE`).
pub fn offline() -> bool {
    net::offline()
}

/// The `User-Agent` of every request: `Grainy/<version>` as the oracle's, with this build's version.
pub fn user_agent() -> String {
    format!("Grainy/{BUILD_VERSION}")
}

fn failure_text(failure: Failure) -> String {
    match failure {
        Failure::Offline => unavailable(),
        other => other.to_string(),
    }
}

fn io_error(failure: Failure) -> std::io::Error {
    std::io::Error::other(failure_text(failure))
}

// ---- feedback ----------------------------------------------------------------------------------------------------

/// The feedback form post over HTTPS (`feedback.send`'s `urlopen`): the request exactly as
/// `grainy_platform::feedback::send` built it.
pub struct HttpTransport;

impl Transport for HttpTransport {
    fn post(&self, request: &Request, timeout: u64) -> Result<u16, SendFailure> {
        match net::post(&request.url, &request.headers, &request.body, timeout) {
            Ok(status) => Ok(status),
            // `urllib.error.HTTPError` → `HTTP <code>`.
            Err(Failure::Status(code, _)) => Err(SendFailure::Http(code)),
            Err(other) => Err(SendFailure::Other(failure_text(other))),
        }
    }
}

// ---- updates -----------------------------------------------------------------------------------------------------

/// The headers of the feed request (`fetch`): `User-Agent` and GitHub's JSON type.
pub fn feed_headers() -> Vec<(String, String)> {
    vec![("User-Agent".into(), user_agent()), ("Accept".into(), "application/vnd.github+json".into())]
}

/// The headers of a download (`download`): `User-Agent` only.
pub fn download_headers() -> Vec<(String, String)> {
    vec![("User-Agent".into(), user_agent())]
}

fn get(url: &str, headers: &[(String, String)]) -> std::io::Result<Box<dyn Read>> {
    net::get(url, headers, updater::TIMEOUT).map(|reader| Box::new(reader) as Box<dyn Read>).map_err(io_error)
}

/// The reader of the release feed over HTTPS (`fetch`); a failure carries the oracle's reason (`Not Found`, the
/// connection's error, `timed out`).
pub fn http_opener() -> UpdateOpener {
    Arc::new(|url: &str| get(url, &feed_headers()))
}

/// Opens a release's address for reading.
pub type Download = Arc<dyn Fn(&str) -> std::io::Result<Box<dyn Read>> + Send + Sync>;
/// Starts the new version's installer step: staged folder, app folder, restart arguments.
pub type Launcher = Arc<dyn Fn(&Path, &Path, &[String]) -> std::io::Result<()> + Send + Sync>;

/// The update download and hand-over: `download` + `stage` + `launch` of luma/updater.py on the platform port,
/// with the address reader, the launcher and the macOS disk image tools given (tests give fakes;
/// [`NetworkBackend::real`] is HTTPS, `hdiutil`/`ditto` and the real launch).
pub struct NetworkBackend {
    /// Opens the release's address.
    pub open: Download,
    /// Starts the staged version.
    pub launcher: Launcher,
    /// The machine.
    pub layout: Layout,
    /// Attaches, copies from and detaches the macOS disk image.
    pub disk_image: Arc<dyn updater::DiskImageTools>,
}

impl NetworkBackend {
    /// HTTPS downloads and the real start of the staged program.
    pub fn real() -> Self {
        let layout = Layout::current();
        let launch_layout = layout.clone();
        NetworkBackend {
            open: Arc::new(|url: &str| get(url, &download_headers())),
            launcher: Arc::new(move |staged: &Path, app: &Path, arguments: &[String]| {
                updater::launch_for(staged, app, arguments, &launch_layout, &updater::RS_PACKAGE)
            }),
            layout,
            disk_image: Arc::new(updater::SystemTools),
        }
    }
}

/// `str(getattr(error, 'reason', error)) or type(error).__name__`.
fn reason_of(error: &updater::UpdateError) -> String {
    let text = error.to_string();
    if !text.is_empty() {
        return text;
    }
    match error {
        updater::UpdateError::Io(_) => "OSError".into(),
        updater::UpdateError::Value(_) => "ValueError".into(),
        updater::UpdateError::Zip(_) => "BadZipFile".into(),
    }
}

impl Backend for NetworkBackend {
    fn download_and_stage(
        &self,
        release: &Release,
        work: &Path,
        progress: &mut dyn FnMut(u64, u64),
        cancel: &AtomicBool,
    ) -> Result<Option<PathBuf>, String> {
        // Only this Rust build's own file for this system (the Windows zip, the macOS disk image): never a Python
        // Grainy installer, whatever reached this point.
        if !updater::is_rust_release(release, &self.layout) {
            return Err(tr("업데이트 파일의 구조가 올바르지 않습니다."));
        }
        let open = |url: &str| (self.open)(url);
        let archive = updater::download(release, &work.join("download"), Some(progress), Some(cancel), &open)
            .map_err(|e| reason_of(&e))?;
        match archive {
            None => Ok(None),
            Some(archive) => updater::stage_rust(&archive, work, &self.layout, self.disk_image.as_ref())
                .map(Some)
                .map_err(|e| reason_of(&e)),
        }
    }

    fn launch(&self, staged: &Path, app: &Path, arguments: &[String]) -> Result<(), String> {
        (self.launcher)(staged, app, arguments).map_err(|e| {
            let text = e.to_string();
            if text.is_empty() {
                "OSError".into()
            } else {
                text
            }
        })
    }
}

// ---- installing ----------------------------------------------------------------------------------------------------

/// At the program's start: switches the network off when `offline` (`--offline`) is set, then, unless it is off,
/// gives feedback and updates their real network. Returns whether it did.
pub fn install(cx: &mut AppContext, offline: bool) -> bool {
    if offline {
        net::set_offline();
    }
    if net::offline() {
        return false;
    }
    cx.state::<FeedbackState>().transport = Some(Arc::new(HttpTransport));
    cx.state::<UpdateEnv>().backend = Some(Arc::new(NetworkBackend::real()));
    cx.state::<UpdateState>().opener = Some(http_opener());
    true
}

// ---- --finish-update -----------------------------------------------------------------------------------------------

/// The arguments of `--finish-update APP --wait-pid PID -- ARGS` (main.py): the app folder, the old program's
/// process and the arguments to restart it with; none for any other command line.
pub fn finish_arguments(args: &[String]) -> Option<(PathBuf, i64, Vec<String>)> {
    if args.first().map(String::as_str) != Some("--finish-update") {
        return None;
    }
    let app = PathBuf::from(args.get(1)?);
    let pid = args.get(3)?.parse().ok()?;
    let rest = args.iter().position(|a| a == "--").map_or_else(Vec::new, |at| args[at + 1..].to_vec());
    Some((app, pid, rest))
}

/// Port of `run_finish` (main.py `--finish-update`), run by the new version from its staged folder: replaces the
/// old version once it has closed ([`updater::finish_logged_for`] with [`updater::RS_PACKAGE`]: on Windows only the
/// release's own files of the program folder are swapped, on macOS the whole `Grainy RS.app`; the log `update.log`
/// in the work folder), then starts the program in the app folder again with its arguments (macOS: through
/// `/usr/bin/open -n`, so it starts as an app of its own). Returns the exit code, or none when the
/// command line is not `--finish-update`. The oracle's progress window and failure box are left out: the reason
/// goes to the log, the error stream and `update.log`.
pub fn run_finish(args: &[String]) -> Option<i32> {
    let (app, pid, arguments) = finish_arguments(args)?;
    let layout = Layout::current();
    let executable = std::env::current_exe().ok().and_then(|p| std::fs::canonicalize(p).ok()).unwrap_or_default();
    let staged = if layout.system.is_mac() {
        executable.ancestors().nth(3).map(Path::to_path_buf)
    } else {
        executable.parent().map(Path::to_path_buf)
    }
    .unwrap_or_default();
    let error = updater::finish_logged_for(&app, &staged, pid, &layout, &updater::RS_PACKAGE);
    if !error.is_empty() {
        log::error!("update not finished: {error}");
        eprintln!(
            "{}",
            crate::text::tr_with("업데이트하지 못했습니다. 이전 버전을 그대로 사용합니다.\n{0}", &[&error])
        );
    }
    let started = if layout.system.is_mac() {
        let program = app.join(updater::RS_PACKAGE.mac.program);
        program.is_file().then(|| {
            let mut command = std::process::Command::new("/usr/bin/open");
            command.arg("-n").arg(&app);
            if !arguments.is_empty() {
                command.arg("--args").args(&arguments);
            }
            command.spawn()
        })
    } else {
        let target = app.join(updater::RS_PACKAGE.program);
        target.is_file().then(|| std::process::Command::new(&target).args(&arguments).current_dir(&app).spawn())
    };
    if let Some(Err(e)) = started {
        log::error!("the updated program did not start: {e}");
    }
    Some(if error.is_empty() { 0 } else { 1 })
}
