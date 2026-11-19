//! Repeated full-buffer decode hashes for the CC0 field corpus; no photo library access.
use grainy_raw::{DecodeMode, RawFile};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{error::Error, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let folder = PathBuf::from(args.next().ok_or("usage: decode_digest <RAW folder> [repeats] [name filter]")?);
    let repeats: usize = args.next().as_deref().unwrap_or("3").parse()?;
    let filter = args.next().unwrap_or_default().to_lowercase();
    let manifest: Vec<Value> = serde_json::from_slice(&std::fs::read(folder.join("MANIFEST.json"))?)?;
    for entry in manifest {
        let name = entry["file"].as_str().ok_or("missing manifest filename")?;
        if !name.to_lowercase().contains(&filter) {
            continue;
        }
        let path = folder.join(name);
        if path.file_name().and_then(|x| x.to_str()) != Some(name) {
            return Err("manifest entry must be a filename".into());
        }
        let source_hash = format!("{:x}", Sha256::digest(std::fs::read(&path)?));
        if Some(source_hash.as_str()) != entry["sha256"].as_str() {
            return Err(format!("manifest hash mismatch: {name}").into());
        }
        for (label, mode) in
            [("camera", DecodeMode::Camera), ("srgb", DecodeMode::Srgb), ("prophoto", DecodeMode::ProPhoto)]
        {
            for half in [false, true] {
                for repeat in 0..repeats {
                    let start = std::time::Instant::now();
                    let mut raw = RawFile::open(&path)?;
                    let pixels = raw.postprocess(mode, half)?;
                    let seconds = start.elapsed().as_secs_f64();
                    let mut hash = Sha256::new();
                    for chunk in pixels.as_slice().ok_or("noncontiguous decode")?.chunks(4096) {
                        let bytes: Vec<_> = chunk.iter().flat_map(|v| v.to_le_bytes()).collect();
                        hash.update(bytes);
                    }
                    println!(
                        "{}",
                        serde_json::json!({"file":name, "mode":label, "half":half,
                        "repeat":repeat, "seconds":seconds, "shape":pixels.shape(), "sha256":format!("{:x}",hash.finalize())})
                    );
                }
            }
        }
    }
    Ok(())
}
