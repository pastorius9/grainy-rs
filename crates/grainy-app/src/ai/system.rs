//! The one door to the computer for the AI features: the environment, the home folder, process launches and web
//! pages. The oracle reads `os.environ`, `Path.home()` and starts `subprocess.Popen` / `ProcessTree` directly; here
//! every such use goes through a [`System`], so that tests give a temporary home, fake programs and scripted
//! processes, and never read the person's AI configuration or start a real AI program, installer or sign-in.
//!
//! Files are read and written with `std::fs` on the paths a [`System`] gives (the tests' paths are temporary).

use std::collections::BTreeMap;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// `sys.platform` as the AI code branches on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// `win32`.
    Windows,
    /// `darwin`.
    MacOs,
    /// Anything else (`linux`…): no sign-in or install terminal.
    Other,
}

impl Os {
    /// This computer.
    pub fn current() -> Self {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Other
        }
    }

    /// The `platform` argument of the oracle (`'win32'`, `'darwin'`, `'linux'`).
    pub fn parse(name: &str) -> Self {
        match name {
            "win32" => Os::Windows,
            "darwin" => Os::MacOs,
            _ => Os::Other,
        }
    }
}

/// A process to start.
#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    /// The program and its arguments.
    pub argv: Vec<String>,
    /// The working folder.
    pub cwd: PathBuf,
    /// The whole environment of the child, or `None` to inherit this process's.
    pub env: Option<Vec<(String, String)>>,
}

/// A started program whose output Grainy reads (`ai_process.ProcessTree`): stdin closed, stdout and stderr piped, no
/// window; [`Piped::stop`] ends it with every process it started.
pub trait Piped: Send {
    /// The standard output, once (the runner reads it line by line).
    fn stdout(&mut self) -> Option<Box<dyn BufRead + Send>>;
    /// The standard error, once.
    fn stderr(&mut self) -> Option<Box<dyn Read + Send>>;
    /// Waits for the exit code (`-1` when it was killed without one).
    fn wait(&mut self) -> std::io::Result<i32>;
    /// What ends the program and its descendants (`ProcessTree.stop`); callable from any thread, more than once.
    fn stopper(&self) -> Arc<dyn Fn() + Send + Sync>;
}

/// A program running in a console window the person owns (`SignInSession`): sign-in and installers.
pub trait Console: Send {
    /// Whether it ended (`SignInSession.poll`).
    fn finished(&mut self) -> bool;
    /// Removes what it left behind (`SignInSession.cleanup`).
    fn cleanup(&mut self) {}
}

/// The computer as the AI features see it.
pub trait System: Send + Sync {
    /// `sys.platform`.
    fn os(&self) -> Os;
    /// `os.environ.get(name)`.
    fn var(&self, name: &str) -> Option<String>;
    /// `dict(os.environ)`, in a stable order.
    fn vars(&self) -> Vec<(String, String)>;
    /// `Path.home()`.
    fn home(&self) -> PathBuf;
    /// `os.curdir` for `shutil.which` on Windows.
    fn current_dir(&self) -> PathBuf {
        PathBuf::from(".")
    }
    /// `ProcessTree(argv, cwd, env)`.
    fn piped(&self, launch: &Launch) -> std::io::Result<Box<dyn Piped>>;
    /// `subprocess.Popen(argv, cwd, env, creationflags=CREATE_NEW_CONSOLE)` on Windows, `open -a Terminal …` on
    /// macOS (`SignInSession`): a window the person sees and owns. `done` is the file the macOS script touches when
    /// it ends (the session is finished when it exists or `open` failed).
    fn console(&self, launch: &Launch, done: Option<PathBuf>) -> std::io::Result<Box<dyn Console>>;
    /// `QDesktopServices.openUrl`: true when a browser took it.
    fn open_url(&self, url: &str) -> bool;
}

/// This computer: the real environment (or a given one), real programs.
#[derive(Debug, Clone, Default)]
pub struct HostSystem {
    /// Replaces the whole environment when set (as the goldens' `isolated_env`).
    pub env: Option<BTreeMap<String, String>>,
}

impl HostSystem {
    /// The real computer and environment.
    pub fn new() -> Self {
        Self { env: None }
    }
}

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

fn command(launch: &Launch) -> std::io::Result<std::process::Command> {
    let program = launch.argv.first().ok_or_else(|| std::io::Error::other("nothing to start"))?;
    let mut command = std::process::Command::new(program);
    command.args(&launch.argv[1..]).current_dir(&launch.cwd);
    if let Some(env) = &launch.env {
        command.env_clear();
        command.envs(env.iter().map(|(k, v)| (k, v)));
    }
    Ok(command)
}

/// Ends a process and the processes it started.
fn kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &format!("-{pid}")])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

struct HostPiped {
    child: std::process::Child,
    stopper: Arc<dyn Fn() + Send + Sync>,
}

impl Piped for HostPiped {
    fn stdout(&mut self) -> Option<Box<dyn BufRead + Send>> {
        self.child.stdout.take().map(|s| Box::new(std::io::BufReader::new(s)) as Box<dyn BufRead + Send>)
    }
    fn stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        self.child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>)
    }
    fn wait(&mut self) -> std::io::Result<i32> {
        Ok(self.child.wait()?.code().unwrap_or(-1))
    }
    fn stopper(&self) -> Arc<dyn Fn() + Send + Sync> {
        self.stopper.clone()
    }
}

struct HostConsole {
    child: std::process::Child,
    done: Option<PathBuf>,
    folder: Option<PathBuf>,
}

impl Console for HostConsole {
    fn finished(&mut self) -> bool {
        let code = self.child.try_wait().ok().flatten().map(|s| s.code().unwrap_or(-1));
        match &self.done {
            // `open` returns as soon as Terminal starts, not when the program ends.
            Some(done) => done.is_file() || code.is_some_and(|c| c != 0),
            None => code.is_some(),
        }
    }
    fn cleanup(&mut self) {
        if let Some(folder) = self.folder.take() {
            let _ = std::fs::remove_dir_all(folder);
        }
    }
}

impl System for HostSystem {
    fn os(&self) -> Os {
        Os::current()
    }

    fn var(&self, name: &str) -> Option<String> {
        match &self.env {
            Some(env) => {
                if cfg!(windows) {
                    env.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone())
                } else {
                    env.get(name).cloned()
                }
            }
            None => std::env::var(name).ok(),
        }
    }

    fn vars(&self) -> Vec<(String, String)> {
        match &self.env {
            Some(env) => env.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            None => {
                let mut all: Vec<(String, String)> = std::env::vars().collect();
                all.sort();
                all
            }
        }
    }

    fn home(&self) -> PathBuf {
        let home = if cfg!(windows) { self.var("USERPROFILE") } else { self.var("HOME") };
        home.filter(|h| !h.is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
    }

    fn current_dir(&self) -> PathBuf {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }

    fn piped(&self, launch: &Launch) -> std::io::Result<Box<dyn Piped>> {
        require_online()?;
        let mut command = command(launch)?;
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let child = command.spawn()?;
        let pid = child.id();
        let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopper: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            if !stopped.swap(true, std::sync::atomic::Ordering::SeqCst) {
                kill_tree(pid);
            }
        });
        Ok(Box::new(HostPiped { child, stopper }))
    }

    fn console(&self, launch: &Launch, done: Option<PathBuf>) -> std::io::Result<Box<dyn Console>> {
        require_online()?;
        let mut command = command(launch)?;
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NEW_CONSOLE);
        }
        if done.is_some() {
            command
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
        }
        let folder = done.as_ref().and_then(|d| d.parent().map(Path::to_path_buf));
        let child = command.spawn()?;
        Ok(Box::new(HostConsole { child, done, folder }))
    }

    fn open_url(&self, url: &str) -> bool {
        if require_online().is_err() {
            return false;
        }
        let (program, args): (&str, Vec<&str>) = match self.os() {
            Os::Windows => ("rundll32", vec!["url.dll,FileProtocolHandler", url]),
            Os::MacOs => ("open", vec![url]),
            Os::Other => ("xdg-open", vec![url]),
        };
        std::process::Command::new(program).args(args).spawn().is_ok()
    }
}

/// Refuse external AI processes, installers and login flows while the app is offline.
pub(crate) fn require_online() -> std::io::Result<()> {
    if grainy_platform::net::offline() {
        Err(std::io::Error::other("offline"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_blocks_external_programs_before_launch() {
        grainy_platform::net::set_offline();
        let system = HostSystem::new();
        let launch = Launch { argv: Vec::new(), cwd: PathBuf::new(), env: None };
        assert!(matches!(system.piped(&launch), Err(e) if e.to_string() == "offline"));
        assert!(matches!(system.console(&launch, None), Err(e) if e.to_string() == "offline"));
        assert!(!system.open_url("https://example.invalid/"));
    }
}
