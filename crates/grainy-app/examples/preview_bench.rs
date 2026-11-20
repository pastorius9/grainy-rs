//! Release-only latency diagnostic. Pass a RAW path, a monitor ICC path, and optionally
//! "worker", "engine", "auto", "buttons", "latency", "sliders", "present" or "verify".
//! Each timing row gives a median in ms and the sample count. Worker timings end at
//! AppContext::show_frame's egui texture update. "present" also fences a headless GPU draw.

use std::path::Path;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use grainy_app::{session::Session, worker::Worker, AppContext, Zoom};
use grainy_cms::Profile;
use grainy_core::{settings::defaults, Settings};
use grainy_engine::files::load_photo;
use grainy_engine::gpu::{develop_gpu_cached, GpuCache};
use grainy_engine::{develop_cached, Crop, DevelopmentCache, Options, Output};
use grainy_gpu::{AdapterKind, Gpu};

fn elapsed(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.
}

fn report(case: &str, mut samples: Vec<f64>) {
    samples.sort_by(f64::total_cmp);
    let n = samples.len();
    println!("{case},{:.2},{n}", (samples[(n - 1) / 2] + samples[n / 2]) / 2.);
}

fn context(path: &Path, gpu: bool, monitor: Option<&Profile>) -> (AppContext, mpsc::Receiver<()>) {
    let mut doc = Session::default();
    doc.add([path.to_owned()]);
    let mut cx = AppContext::new(&Default::default(), Box::new(Worker::with_gpu(gpu)), Box::new(doc));
    cx.renderer.set_monitor(monitor.cloned());
    let (tx, rx) = mpsc::channel();
    let wake: grainy_app::render::Waker = Arc::new(move || {
        let _ = tx.send(());
    });
    cx.renderer.set_waker(wake.clone());
    cx.jobs.set_waker(Some(wake));
    (cx, rx)
}

fn shown(cx: &mut AppContext, wake: &mpsc::Receiver<()>) {
    drop(shown_delta(cx, wake));
}

fn shown_delta(cx: &mut AppContext, wake: &mpsc::Receiver<()>) -> eframe::egui::TexturesDelta {
    let old = cx.frame().map(|f| f.id);
    loop {
        for done in cx.jobs.poll() {
            done(cx);
        }
        cx.refresh_picture();
        if cx.frame().map(|f| f.id) != old {
            // A real backend consumes the texture delta on each frame.
            return cx.egui.tex_manager().write().take_delta();
        }
        wake.recv_timeout(Duration::from_secs(120)).expect("worker timeout");
    }
}

/// Submit the worker's actual egui texture delta, draw the photo and fence the offscreen GPU.
/// This endpoint includes texture upload and sampling, with no monitor refresh or swapchain wait.
fn present(path: &Path, profile: &Profile) {
    use eframe::{egui, egui_wgpu, wgpu};
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .expect("display adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).expect("display device");
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = egui_wgpu::Renderer::new(&device, format, Default::default());
    let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: [1600, 1000], pixels_per_point: 1.0 };
    let surface = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("headless photo view"),
        size: wgpu::Extent3d { width: 1600, height: 1000, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = surface.create_view(&Default::default());
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600., 1000.));
    let (mut cx, wake) = context(path, true, Some(profile));
    let mut ready = vec![];
    let mut painted = vec![];
    for i in 0..=7 {
        let start = Instant::now();
        cx.edit_state_mut().set("exposure", i as f64 / 10.);
        let delta = shown_delta(&mut cx, &wake);
        let queued = elapsed(start);
        let preview = &cx.frame().expect("shown frame").preview;
        let size = [preview.width, preview.height];
        let texture = *delta
            .set
            .iter()
            .find(|(_, updates)| updates.iter().any(|u| u.image.size() == size))
            .expect("fit photo texture")
            .0;
        for update in &delta.set[&texture] {
            renderer.update_texture(&device, &queue, texture, update);
        }
        let mut mesh = egui::Mesh::with_texture(texture);
        mesh.add_rect_with_uv(
            rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
            egui::Color32::WHITE,
        );
        let jobs = [egui::ClippedPrimitive { clip_rect: rect, primitive: egui::epaint::Primitive::Mesh(mesh) }];
        let mut encoder = device.create_command_encoder(&Default::default());
        let extra = renderer.update_buffers(&device, &queue, &mut encoder, &jobs, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            renderer.render(&mut pass.forget_lifetime(), &jobs, &screen);
        }
        let submission = queue.submit(extra.into_iter().chain([encoder.finish()]));
        device
            .poll(wgpu::PollType::Wait { submission_index: Some(submission), timeout: Some(Duration::from_secs(30)) })
            .expect("draw");
        if i > 0 {
            ready.push(queued);
            painted.push(elapsed(start));
        }
        for id in &delta.free {
            renderer.free_texture(id);
        }
    }
    report("slider to egui texture", ready);
    report("slider to completed GPU draw", painted);
}

fn worker(path: &Path, gpu: bool, monitor: Option<&Profile>) {
    let (mut cx, wake) = context(path, gpu, monitor);
    let name = format!("worker gpu={gpu} icc={}", monitor.is_some());
    let start = Instant::now();
    shown(&mut cx, &wake);
    report(&format!("{name} open fit"), vec![elapsed(start)]);
    for key in ["exposure", "contrast", "shadows"] {
        let mut times = Vec::new();
        let mut ready = Vec::new();
        for i in 1..=5 {
            let start = Instant::now();
            cx.edit_state_mut().set(key, if key == "exposure" { i as f64 / 10. } else { i as f64 });
            shown(&mut cx, &wake);
            times.push(elapsed(start));
            ready.push(cx.frame().unwrap().millis);
        }
        report(&format!("{name} fit {key} shown"), times);
        report(&format!("{name} fit {key} worker"), ready);
        cx.edit_state_mut().set(key, 0.);
    }
    cx.edit_state_mut().commit("reset");
    cx.set_zoom(Zoom::Actual);
    let start = Instant::now();
    shown(&mut cx, &wake);
    report(&format!("{name} first full"), vec![elapsed(start)]);
    let mut release = Vec::new();
    for i in 1..=3 {
        cx.edit_state_mut().set("exposure", i as f64 / 10.);
        shown(&mut cx, &wake);
        let start = Instant::now();
        cx.edit_state_mut().commit("release");
        shown(&mut cx, &wake);
        release.push(elapsed(start));
    }
    report(&format!("{name} zoom release shown"), release);
}

fn sliders(path: &Path, profile: &Profile) {
    let (mut cx, wake) = context(path, true, Some(profile));
    shown(&mut cx, &wake);
    for key in ["temperature", "tint", "whites", "blacks", "highlights", "saturation", "vibrance"] {
        let mut times = Vec::new();
        for i in 1..=5 {
            let start = Instant::now();
            cx.edit_state_mut().set(key, i as f64 * 3.);
            shown(&mut cx, &wake);
            times.push(elapsed(start));
        }
        report(&format!("fit {key} shown"), times);
        cx.edit_state_mut().set(key, 0.);
    }
}

fn finish_job(cx: &mut AppContext, wake: &mpsc::Receiver<()>, kind: &str) {
    while cx.jobs.running(kind) {
        for done in cx.jobs.poll() {
            done(cx);
        }
        if cx.jobs.running(kind) {
            wake.recv_timeout(Duration::from_secs(120)).expect("job timeout");
        }
    }
}

fn buttons(path: &Path) {
    let (mut cx, wake) = context(path, false, None);
    shown(&mut cx, &wake);
    finish_job(&mut cx, &wake, "prepare-auto");
    for (key, count) in [("develop.auto_color", 3), ("develop.negative", 1), ("develop.negative_again", 3)] {
        let mut times = Vec::new();
        for _ in 0..count {
            if key == "develop.auto_color" {
                cx.edit_state_mut().set_all(defaults());
            }
            let start = Instant::now();
            assert!(cx.run(key));
            finish_job(&mut cx, &wake, "auto");
            times.push(elapsed(start));
        }
        report(key, times);
    }
    assert!(cx.settings().bool("negative"));
}

fn auto(path: &Path) {
    let (mut cx, mut wake) = context(path, false, None);
    shown(&mut cx, &wake);
    for (label, count) in [("first", 1), ("repeat", 5), ("prepared first", 1)] {
        if label == "prepared first" {
            (cx, wake) = context(path, false, None);
            shown(&mut cx, &wake);
            finish_job(&mut cx, &wake, "prepare-auto");
        }
        let mut times = Vec::new();
        for _ in 0..count {
            cx.edit_state_mut().set_all(defaults());
            let start = Instant::now();
            assert!(cx.run("develop.auto_tone"));
            finish_job(&mut cx, &wake, "auto");
            assert!(cx.settings().f64("exposure") != 0. || cx.settings().f64("blacks") != 0.);
            times.push(elapsed(start));
        }
        report(&format!("auto click {label}"), times);
    }
}

fn engine(path: &Path, profile: &Profile) {
    let gpu = Gpu::with_adapter(AdapterKind::Hardware).expect("adapter");
    if let Some(gpu) = &gpu {
        println!("# adapter {}", gpu.adapter_info().name);
    }
    for (size, label) in [(Some(2048), "fit"), (None, "full")] {
        let mut s: Settings = defaults();
        let start = Instant::now();
        let src = load_photo(path, size, &s).expect("load");
        report(&format!("load {label}"), vec![elapsed(start)]);
        let size = |key: &str| src.metadata[key].as_f64().unwrap();
        for (output, icc) in [(Output::Named("sRGB"), false), (Output::Icc(profile), true)] {
            for accelerated in [false, true] {
                let options = Options {
                    output,
                    original_size: Some((size("width"), size("height"))),
                    use_crop: Crop::Cropped,
                    ..Options::default()
                };
                let mut cpu = DevelopmentCache::growing(256 << 20, 4., Some(1536 << 20));
                let mut cache = GpuCache::default();
                let mut times = Vec::new();
                let mut present = Vec::new();
                let mut first = None;
                for i in 0..=5 {
                    s.set("exposure", i as f64 / 10.);
                    let start = Instant::now();
                    let image = if accelerated {
                        let Some(gpu) = &gpu else { break };
                        match develop_gpu_cached(src.photo.source(), &s.0, &options, gpu, &mut cache, 1) {
                            Ok(image) => image,
                            Err(e) => {
                                println!("# {label} GPU {e}");
                                break;
                            }
                        }
                    } else {
                        develop_cached(src.photo.source(), &s.0, &options, &mut cpu, 1).expect("CPU")
                    };
                    let ms = elapsed(start);
                    let start = Instant::now();
                    let c = grainy_app::histogram::convert(&image);
                    std::hint::black_box(grainy_app::histogram::bins(&c.rgba, c.width, c.height));
                    if i > 0 {
                        times.push(ms);
                        present.push(elapsed(start));
                    } else {
                        first = Some(ms);
                    }
                }
                if !times.is_empty() {
                    let name = format!("engine {label} gpu={accelerated} icc={icc}");
                    report(&format!("{name} first"), vec![first.unwrap()]);
                    report(&name, times);
                    report(&format!("{name} bytes+bins"), present);
                }
            }
        }
    }
}

fn verify(path: &Path, profile: &Profile) {
    for native in [false, true] {
        let mut s = defaults();
        s.set("negative", native);
        let (_, seed) = grainy_engine::files::load_preview(path, 2048, 1800, &s).expect("fit");
        let direct = load_photo(path, Some(1800), &s).expect("analysis");
        let derived = seed.expect("24 MP half decode").finish().expect("resize");
        assert_eq!(derived.pixels(), direct.photo.pixels());
        let measure = |src: &grainy_engine::files::Photo| {
            grainy_engine::auto::auto_tone_settings(src.source(), &s, &Default::default()).unwrap()
        };
        let (a, ar) = measure(&derived);
        let (b, br) = measure(&direct.photo);
        assert_eq!((a.0, ar), (b.0, br));
        println!("# analysis native={native} exact pixels, settings and report");
    }
    let (mut cx, wake) = context(path, true, Some(profile));
    let s = defaults();
    let src = load_photo(path, Some(2048), &s).expect("load");
    let options = Options {
        output: Output::Working,
        original_size: Some((src.metadata["width"].as_f64().unwrap(), src.metadata["height"].as_f64().unwrap())),
        ..Options::default()
    };
    for (key, v) in [("exposure", 0.3), ("temperature", -23.), ("blacks", 17.), ("shadows", 10.)] {
        cx.edit_state_mut().set(key, v);
        shown(&mut cx, &wake);
        let working = grainy_engine::develop(src.photo.source(), &cx.settings().0, &options).unwrap();
        let stats = grainy_app::histogram::convert(&working);
        let shown = grainy_cms::output_rgb(&working, "sRGB", grainy_cms::Destination::Icc(profile)).unwrap();
        let expected = grainy_app::histogram::convert(&shown);
        let frame = cx.frame().unwrap();
        let diff = expected.rgba.iter().zip(&frame.preview.rgba).map(|(a, b)| a.abs_diff(*b)).max();
        assert_eq!(diff, Some(0), "{key}");
        assert_eq!(frame.clipping, stats.mask);
        assert_eq!(frame.channels, stats.channels);
        assert_eq!(frame.bins, grainy_app::histogram::bins(&stats.rgba, stats.width, stats.height));
        println!("# {key} ICC bytes, histogram and clipping exact");
    }
}

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let path = Path::new(args.first().expect("RAW file"));
    let profile = Profile::from_bytes(std::fs::read(args.get(1).expect("monitor ICC")).expect("read ICC"));
    let group = args.get(2).and_then(|s| s.to_str()).unwrap_or("all");
    if group == "worker-case" {
        let gpu = args.get(3).is_some_and(|v| v == "on");
        let icc = args.get(4).is_some_and(|v| v == "icc");
        worker(path, gpu, icc.then_some(&profile));
        return;
    }
    if group == "buttons" {
        buttons(path);
        return;
    }
    if group == "sliders" {
        sliders(path, &profile);
        return;
    }
    if group == "present" {
        present(path, &profile);
        return;
    }
    if group == "verify" {
        verify(path, &profile);
        return;
    }
    println!("# {:?} — median_ms,count", path.file_name().unwrap());
    if matches!(group, "all" | "engine") {
        engine(path, &profile);
    }
    if matches!(group, "all" | "auto") {
        auto(path);
    }
    if matches!(group, "all" | "worker") {
        // A dropped worker and graphics device may still be retiring on detached threads.
        // A fresh process gives each preference the same cold engine/ICC caches.
        let exe = std::env::current_exe().expect("benchmark executable");
        for gpu in ["off", "on"] {
            for output in ["srgb", "icc"] {
                let status = std::process::Command::new(&exe)
                    .arg(path)
                    .arg(&args[1])
                    .args(["worker-case", gpu, output])
                    .status()
                    .expect("worker benchmark");
                assert!(status.success(), "worker {gpu} {output}");
            }
        }
    }
    if group == "latency" {
        worker(path, true, Some(&profile));
    }
}
