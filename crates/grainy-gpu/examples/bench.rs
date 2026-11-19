//! Generated 24 MP pixels only; no photo library. GPU synchronization is included in every timing.
use grainy_core::Image;
use grainy_gpu::{Gpu, Resident};
use std::time::Instant;

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Some(gpu) = Gpu::new() else {
        println!("SKIP: no adapter");
        return Ok(());
    };
    println!("Adapter: {:?}", gpu.adapter_info());
    let image = Image::from_shape_fn((4000, 6000, 3), |(y, x, c)| ((x * 7 + y * 13 + c * 19) % 1009) as f32 / 1008.0);
    let mut s = grainy_core::defaults();
    s.set("exposure", 0.5);
    s.set("saturation", 20.0);
    s.set("vibrance", 15.0);
    // Warm pipelines, memory allocator and the first queue submission.
    let mut resident = Resident::new(&gpu, &image)?;
    let frame = gpu.develop_resident(&mut resident, &s, 1.0)?.ok_or("unexpected CPU fallback")?;
    let _ = gpu.readback(&frame)?;
    let mut total = Vec::new();
    let mut upload = Vec::new();
    let mut compute = Vec::new();
    let mut readback = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let frame = gpu.upload(&image)?;
        gpu.wait()?;
        let u = t.elapsed().as_secs_f64() * 1000.0;
        let c = Instant::now();
        let tone = gpu.tone_frame(&frame, &s, &image)?.ok_or("tone fallback")?;
        let color = gpu.color_frame(&tone, &s)?.ok_or("color fallback")?;
        gpu.wait()?;
        let c = c.elapsed().as_secs_f64() * 1000.0;
        let r = Instant::now();
        let result = gpu.readback(&color)?;
        let r = r.elapsed().as_secs_f64() * 1000.0;
        std::hint::black_box(result);
        total.push(t.elapsed().as_secs_f64() * 1000.0);
        upload.push(u);
        compute.push(c);
        readback.push(r);
    }
    println!("24 MP 6000x4000, five samples, median milliseconds:");
    println!(
        "upload {:.3}; tone+color {:.3}; readback {:.3}; total {:.3}",
        median(&mut upload),
        median(&mut compute),
        median(&mut readback),
        median(&mut total)
    );
    for slider in ["exposure", "saturation"] {
        let mut compute = Vec::new();
        let mut visible = Vec::new();
        for i in 0..7 {
            s.set(slider, if slider == "exposure" { 0.6 + 0.05 * i as f64 } else { 21.0 + i as f64 });
            let t = Instant::now();
            let frame = gpu.develop_resident(&mut resident, &s, 1.0)?.ok_or("resident fallback")?;
            gpu.wait()?;
            compute.push(t.elapsed().as_secs_f64() * 1000.0);
            let result = gpu.readback(&frame)?;
            std::hint::black_box(result);
            visible.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        println!("resident {slider}: compute {:.3}; with readback {:.3}", median(&mut compute), median(&mut visible));
    }
    println!(
        "runs: {:?}; Python's ~69 ms exposure figure is on-screen, not a matched 24 MP benchmark",
        resident.runs()
    );
    Ok(())
}
