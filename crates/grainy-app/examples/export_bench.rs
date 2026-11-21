//! Time the regular JPEG export with or without the original RAW already open at full resolution.
//! export_bench cold|warm source.raw settings.json new-output.jpg
use grainy_core::Settings;
use grainy_engine::files::{export_photo, load_full_preview, ExportRequest};
use std::{env, error::Error, fs, path::Path, time::Instant};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let [mode, raw, settings, dest] = args.as_slice() else {
        return Err("expected cold|warm source.raw settings.json new-output.jpg".into());
    };
    let edits = serde_json::from_slice(&fs::read(settings)?)?;
    let source = Path::new(raw);
    let start = Instant::now();
    let preview = match mode.to_str() {
        Some("warm") => Some(load_full_preview(source, &Settings(grainy_core::normalized(&edits).0))?),
        Some("cold") => None,
        _ => return Err("mode must be cold or warm".into()),
    };
    let prepared = start.elapsed();
    let start = Instant::now();
    export_photo(source, dest, &edits, &ExportRequest::default())?;
    let elapsed = start.elapsed();
    drop(preview);
    println!("{}", serde_json::json!({"prepare_s": prepared.as_secs_f64(), "export_s": elapsed.as_secs_f64()}));
    Ok(())
}
