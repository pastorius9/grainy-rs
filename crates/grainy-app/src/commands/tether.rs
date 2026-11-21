//! Tethered still capture. A completed transfer is published in the chosen folder before normal catalog import.
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::{import, writable_library, Command, Invocation};
use crate::context::AppContext;
use crate::dialogs::maintenance::{PathWait, Pick};
use crate::text::{tr, tr_with};

pub const COMMANDS: &[Command] = &[
    Command { id: "tether.folder", label: "테더 촬영 폴더 선택…", enabled: writable_library, run: folder },
    Command { id: "tether.capture", label: "카메라 셔터 릴리스", enabled: can_capture, run: capture },
];

#[derive(Default)]
struct Session {
    generation: u64,
    directory: Option<PathBuf>,
}

fn can_capture(cx: &AppContext) -> bool {
    writable_library(cx) && !cx.jobs.running("tether") && !import::importing(cx)
}

fn folder(cx: &mut AppContext, _: &Invocation) {
    choose(cx, false);
}

fn choose(cx: &mut AppContext, shoot: bool) {
    cx.open_dialog(Box::new(PathWait::open(tr("테더 촬영 저장 폴더"), Pick::Folder, None, move |cx, path| {
        if let Err(e) = crate::paths::writable(&path) {
            crate::dialogs::preset_review::show_error(cx, &e.to_string());
            return;
        }
        *cx.state::<Session>() = Session { generation: cx.document_generation, directory: Some(path.clone()) };
        cx.status = tr_with("테더 촬영 저장 폴더: {0}", &[&path.display().to_string()]);
        if shoot {
            start(cx, path);
        }
    })));
}

fn capture(cx: &mut AppContext, _: &Invocation) {
    let generation = cx.document_generation;
    let session = cx.state::<Session>();
    if let Some(path) = session.directory.clone().filter(|_| session.generation == generation) {
        start(cx, path);
    } else {
        choose(cx, true);
    }
}

fn start(cx: &mut AppContext, folder: PathBuf) {
    if !can_capture(cx) {
        return;
    }
    let Some(library) = cx.doc.library_folder() else { return };
    cx.status = tr("카메라에서 촬영하고 파일을 받는 중…");
    cx.spawn_document_job(
        "tether",
        tr("카메라 셔터 릴리스"),
        move |handle| {
            let paths = receive(&folder, camera_command()?, handle.cancel_flag())?;
            // A finished transfer belongs to the library from which the shutter was fired,
            // even if another library has been opened while the camera was downloading.
            let run = import::ImportRun::copies(paths, Default::default());
            run.run(&library, &AtomicBool::new(false), &|done, total, name| handle.progress(done, total, name))
        },
        |cx, result| match result {
            Ok(imported) => {
                crate::folder_watch::imported(cx, &imported, false);
                super::library_column::import_done(cx, Ok(imported.report));
            }
            Err(message) => crate::dialogs::preset_review::show_error(cx, &message),
        },
    );
}

fn camera_command() -> Result<Process, String> {
    let name = if cfg!(windows) { "gphoto2.exe" } else { "gphoto2" };
    let search = std::env::var_os("PATH").unwrap_or_default();
    if let Some(exe) =
        std::env::split_paths(&search).filter(|dir| dir.is_absolute()).map(|p| p.join(name)).find(|p| p.is_file())
    {
        let mut cmd = Process::new(exe);
        cmd.args(["--keep", "--filename", "capture-%n.%C", "--capture-image-and-download"]);
        return Ok(cmd);
    }
    #[cfg(windows)]
    {
        let system =
            PathBuf::from(std::env::var_os("SystemRoot").ok_or_else(|| tr("Windows 시스템 폴더를 찾지 못했습니다."))?);
        let mut cmd = Process::new(system.join("System32/WindowsPowerShell/v1.0/powershell.exe"));
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", include_str!("tether_wia.ps1")]);
        Ok(cmd)
    }
    #[cfg(not(windows))]
    Err(tr("테더 촬영에는 gphoto2와 원격 촬영을 지원하는 카메라가 필요합니다."))
}

fn receive(folder: &Path, mut command: Process, cancel: &AtomicBool) -> Result<Vec<PathBuf>, String> {
    crate::paths::writable(folder).map_err(|e| e.to_string())?;
    let stage = tempfile::Builder::new().prefix(".grainy-capture-").tempdir_in(folder).map_err(|e| e.to_string())?;
    let mut errors = tempfile::tempfile().map_err(|e| e.to_string())?;
    if cancel.load(Ordering::Relaxed) {
        return Err(tr("촬영을 취소했습니다."));
    }
    command
        .current_dir(stage.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(errors.try_clone().map_err(|e| e.to_string())?);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let started = Instant::now();
    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break Ok(()),
            Ok(Some(status)) => {
                let mut bytes = Vec::new();
                if errors.rewind().is_ok() {
                    let _ = errors.by_ref().take(4096).read_to_end(&mut bytes);
                }
                let message = String::from_utf8_lossy(&bytes);
                break Err(format!("{} ({status}) {}", tr("카메라 촬영에 실패했습니다."), message.trim()));
            }
            Ok(None) if !cancel.load(Ordering::Relaxed) && started.elapsed() < Duration::from_secs(120) => {
                std::thread::sleep(Duration::from_millis(50));
            }
            other => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(match other {
                    Err(e) => e.to_string(),
                    _ if cancel.load(Ordering::Relaxed) => tr("촬영을 취소했습니다."),
                    _ => tr("카메라가 제한 시간 안에 응답하지 않았습니다."),
                });
            }
        }
    };
    // From this point cleanup may only remove an empty directory: a driver can leave
    // a valid RAW behind even when downloading the second file or listing the folder fails.
    let retained = stage.keep();
    if let Err(message) = result {
        return match fs::remove_dir(&retained) {
            Ok(()) => Err(message),
            Err(_) => Err(format!("{message}\n{}", retained.display())),
        };
    }
    let read_error = |error: std::io::Error| format!("{error}\n{}", retained.display());
    let entries = fs::read_dir(&retained).map_err(read_error)?;
    let token =
        retained.file_name().and_then(|p| p.to_str()).unwrap_or("capture").trim_start_matches(".grainy-capture-");
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(read_error)?;
        let path = entry.path();
        if entry.file_type().map_err(read_error)?.is_file()
            && crate::document::has_extension(&path, &grainy_service::IMAGE_EXTENSIONS)
        {
            files.push(path);
        }
    }
    files.sort();
    if files.is_empty() {
        if fs::remove_dir(&retained).is_ok() {
            return Err(tr("카메라에서 받은 사진이 없습니다."));
        }
        return Err(format!("{}\n{}", tr("가져올 수 있는 촬영 파일이 없습니다."), retained.display()));
    }
    let mut published = Vec::new();
    for (i, source) in files.iter().enumerate() {
        let mut name = OsString::from(format!("Capture-{stamp}-{token}-{}.", i + 1));
        name.push(source.extension().unwrap_or_default());
        let target = folder.join(name);
        grainy_platform::rename_new(source, &target).map_err(|e| format!("{e}\n{}", retained.display()))?;
        published.push(target);
    }
    let _ = fs::remove_dir(&retained); // only removes an empty staging directory
    Ok(published)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn camera_fixture() {
        if std::env::var_os("GRAINY_FIXTURE_CAMERA").is_some() {
            fs::write("capture-1.dng", b"raw sample").unwrap();
            if std::env::var_os("GRAINY_FIXTURE_CAMERA_FAIL").is_some() {
                std::process::exit(3);
            }
            fs::write("capture-2.jpg", b"jpeg sample").unwrap();
        }
    }
    #[test]
    fn a_failed_transfer_keeps_the_raw_an_actual_camera_already_sent() {
        let folder = tempfile::tempdir().unwrap();
        let mut camera = Process::new(std::env::current_exe().unwrap());
        camera
            .args(["--exact", "commands::tether::tests::camera_fixture"])
            .env("GRAINY_FIXTURE_CAMERA", "1")
            .env("GRAINY_FIXTURE_CAMERA_FAIL", "1");
        let error = receive(folder.path(), camera, &AtomicBool::new(false)).unwrap_err();
        let retained = fs::read_dir(folder.path()).unwrap().next().unwrap().unwrap().path();
        assert!(error.contains(retained.to_str().unwrap()));
        assert_eq!(fs::read(retained.join("capture-1.dng")).unwrap(), b"raw sample");
    }
    #[test]
    fn two_camera_files_are_published_without_overwriting_previous_shots() {
        let folder = tempfile::tempdir().unwrap();
        for _ in 0..2 {
            let mut camera = Process::new(std::env::current_exe().unwrap());
            camera.args(["--exact", "commands::tether::tests::camera_fixture"]).env("GRAINY_FIXTURE_CAMERA", "1");
            let paths = receive(folder.path(), camera, &AtomicBool::new(false)).unwrap();
            assert_eq!(paths.len(), 2);
            assert_eq!(fs::read(&paths[0]).unwrap(), b"raw sample");
            assert_eq!(fs::read(&paths[1]).unwrap(), b"jpeg sample");
        }
        assert_eq!(fs::read_dir(folder.path()).unwrap().count(), 4);
    }
}
