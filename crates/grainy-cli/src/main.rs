//! `grainy-cli`: render a photo from the command line with the same pixels as the Python Grainy's export
//! (`luma/engine.py export_image`), and show a photo's metadata.

use std::path::PathBuf;
use std::process::ExitCode;

use grainy_codecs::{ExportFormat, Metadata, SizeLimit};
use grainy_engine::files::{export_photo, parse_format, photo_info, ExportRequest};
use serde_json::{Map, Value};

const USAGE: &str = "\
usage:
  grainy-cli render <source> -o <output> [options]
  grainy-cli info <source>
  grainy-cli merge hdr|panorama <first> <second> [more photos] -o <new file.tif>

render options:
  -o, --output <file>       the file to create (it must not exist; the source is never written)
  --settings <file.json>    the settings object the catalogue stores (missing keys are defaults)
  --format <name>           JPEG | PNG | \"PNG 16-bit\" | \"TIFF 8-bit\" | \"TIFF 16-bit\" | \"TIFF HDR 32-bit\" | AVIF |
                            \"AVIF HDR 10-bit\" | \"JPEG XL\" (default: from the output extension)
  --quality <0-100>         JPEG and AVIF quality (default 95)
  --longest <N>             shrink so the long side is N pixels (never enlarges)
  --box <W>x<H>             shrink into a box (0 = no limit on that side), with --fit inside|cover
  --fit inside|cover        how the box is used (default inside)
  --color-space <name>      sRGB | \"Display P3\" | \"Adobe RGB\" | \"ProPhoto RGB\" | ... or an RGB ICC file
  --keep-metadata           copy the source's EXIF and write EXIF/XMP/IPTC
  --metadata <file.json>    user metadata (creator, copyright, caption, title, city, ...) with --keep-metadata
  --keywords <a,b,c>        keywords with --keep-metadata
  --grayscale               neutral sRGB results become one-channel gray files
  --dither                  fixed triangular noise before 8-bit rounding
JPEG XL: the pure-Rust encoder writes lossless files only (the quality is not used).";

fn main() -> ExitCode {
    grainy_core::settings::set_hdr_feature(true);
    let args = std::env::args_os()
        .skip(1)
        .map(|arg| arg.into_string().map_err(|_| "argument is not valid Unicode".to_owned()))
        .collect::<Result<Vec<_>, _>>();
    match args.and_then(|args| run(&args)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("grainy-cli: {}", message.lines().next().unwrap_or("failed"));
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("render") => render(&args[1..]),
        Some("merge") => merge(&args[1..]),
        Some("info") => info(&args[1..]),
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(format!("unknown command {other:?} (try: render, info, merge)")),
        None => Err("no command (try: render, info, merge; --help for usage)".into()),
    }
}

fn merge(args: &[String]) -> Result<(), String> {
    use grainy_engine::merge::{merge_to_tiff, Mode};
    let usage = "usage: grainy-cli merge hdr|panorama <first> <second> [more photos] -o <new file.tif>";
    let [kind, sources @ .., option, destination] = args else { return Err(usage.into()) };
    if sources.len() < 2 || !matches!(option.as_str(), "-o" | "--output") {
        return Err(usage.into());
    }
    let mode = match kind.as_str() {
        "hdr" => Mode::Hdr,
        "panorama" => Mode::Panorama,
        _ => return Err(usage.into()),
    };
    let paths = sources.iter().map(PathBuf::from).collect::<Vec<_>>();
    let output = PathBuf::from(destination);
    let (width, height) = merge_to_tiff(&paths, &output, mode, &std::sync::atomic::AtomicBool::new(false), &|_| {})?;
    println!("{} ({} x {})", output.display(), width, height);
    Ok(())
}

fn info(args: &[String]) -> Result<(), String> {
    let [source] = args else { return Err("usage: grainy-cli info <source>".into()) };
    let metadata = photo_info(source).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(&Value::Object(metadata)).map_err(|e| e.to_string())?;
    println!("{text}");
    Ok(())
}

fn read_json_object(path: &str, what: &str) -> Result<Map<String, Value>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {what} {path}: {e}"))?;
    match serde_json::from_str(&text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(format!("{what} {path} must be a JSON object")),
        Err(e) => Err(format!("{what} {path} is not valid JSON: {e}")),
    }
}

fn parse_box(text: &str) -> Result<(usize, usize), String> {
    let bad = || format!("--box wants WIDTHxHEIGHT, for example 2048x1536, not {text:?}");
    let (w, h) = text.split_once(['x', 'X']).ok_or_else(bad)?;
    Ok((w.trim().parse().map_err(|_| bad())?, h.trim().parse().map_err(|_| bad())?))
}

fn format_from_name(output: &std::path::Path) -> Result<ExportFormat, String> {
    let ext = output.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    Ok(match ext.as_str() {
        "jpg" | "jpeg" => ExportFormat::Jpeg,
        "png" => ExportFormat::Png,
        "tif" | "tiff" => ExportFormat::Tiff8,
        "avif" => ExportFormat::Avif,
        "jxl" => ExportFormat::JpegXl,
        _ => return Err("cannot tell the format from the output name: give --format".into()),
    })
}

fn render(args: &[String]) -> Result<(), String> {
    let mut source: Option<String> = None;
    let mut output: Option<PathBuf> = None;
    let mut settings = Map::new();
    let mut request = ExportRequest::default();
    let mut format: Option<ExportFormat> = None;
    let mut longest: Option<usize> = None;
    let mut boxed: Option<(usize, usize)> = None;
    let mut cover = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut value = |name: &str| iter.next().cloned().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "-o" | "--output" => output = Some(PathBuf::from(value(arg)?)),
            "--settings" => settings = read_json_object(&value(arg)?, "settings file")?,
            "--format" => {
                let name = value(arg)?;
                format = Some(parse_format(&name).ok_or_else(|| format!("unknown format {name:?}"))?);
            }
            "--quality" => {
                let text = value(arg)?;
                request.quality = text.parse().map_err(|_| format!("--quality wants a number, not {text:?}"))?;
            }
            "--longest" => {
                let text = value(arg)?;
                longest = Some(text.parse().map_err(|_| format!("--longest wants a whole number, not {text:?}"))?);
            }
            "--box" => boxed = Some(parse_box(&value(arg)?)?),
            "--fit" => {
                cover = match value(arg)?.as_str() {
                    "inside" => false,
                    "cover" => true,
                    other => return Err(format!("--fit is inside or cover, not {other:?}")),
                }
            }
            "--color-space" => request.color_space = value(arg)?,
            "--keep-metadata" => request.keep_metadata = true,
            "--metadata" => request.user_metadata = read_json_object(&value(arg)?, "metadata file")? as Metadata,
            "--keywords" => request.keywords = value(arg)?,
            "--grayscale" => request.grayscale = true,
            "--dither" => request.dither = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            flag if flag.starts_with('-') && flag.len() > 1 => return Err(format!("unknown option {flag}")),
            _ if source.is_none() => source = Some(arg.clone()),
            _ => return Err(format!("unexpected argument {arg:?}")),
        }
    }
    let source = source.ok_or("usage: grainy-cli render <source> -o <output> [options]")?;
    let output = output.ok_or("render needs -o <output>")?;
    if grainy_platform::user_paths::is_oracle_folder(&output, &grainy_platform::Environment::current()) {
        return Err("the Python Grainy folder is read-only".into());
    }
    request.format = match format {
        Some(format) => format,
        None => format_from_name(&output)?,
    };
    if longest.is_some() && boxed.is_some() {
        return Err("--longest and --box cannot be combined".into());
    }
    request.longest = match (longest, boxed) {
        (Some(n), _) => Some(SizeLimit::Longest(n)),
        (_, Some((width, height))) => Some(SizeLimit::Box { width, height, cover }),
        _ => None,
    };
    if request.format == ExportFormat::JpegXl {
        request.jxl_lossless = true;
        eprintln!("grainy-cli: note: JPEG XL is written lossless (the pure-Rust encoder has no distance mode)");
    }
    let (width, height) = export_photo(&source, &output, &settings, &request).map_err(|e| e.to_string())?;
    println!("{} ({width}x{height})", output.display());
    Ok(())
}
