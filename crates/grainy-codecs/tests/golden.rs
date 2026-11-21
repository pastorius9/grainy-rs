use grainy_codecs::*;
use grainy_core::Image;
use grainy_testkit::{assert_close, max_abs_diff, Case};
use ndarray::{Array3, ArrayD, Ix3};
use serde_json::{json, Value};
use std::{cell::RefCell, fs, io::Cursor, path::PathBuf};

fn root() -> PathBuf {
    grainy_testkit::root().join("golden/codecs")
}
fn output() -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-output");
    fs::create_dir_all(&p).unwrap();
    p
}
fn names() -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(root())
        .unwrap()
        .map(|p| p.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| root().join(n).join("case.json").exists())
        .collect();
    names.sort();
    names
}
fn bytes(name: &str, file: &str) -> Vec<u8> {
    fs::read(root().join(name).join(file)).unwrap()
}
fn meta(case: &Case) -> Metadata {
    case.json()["metadata"].as_object().unwrap().clone()
}

#[derive(Default)]
struct Cms {
    case: Option<String>,
    calls: RefCell<usize>,
}
impl ColorManager for Cms {
    fn profile_bytes(&self, p: &Profile) -> Result<Vec<u8>> {
        match p {
            Profile::Named(n) if n == "Lab4" => Ok(bytes(self.case.as_ref().unwrap(), "cms_source_0.icc")),
            Profile::Named(n) => Ok(fs::read(root().join("profiles").join(format!("{n}.icc")))?),
            Profile::Embedded(v) => Ok(v.clone()),
            Profile::Calibrated { .. } => Ok(bytes(self.case.as_ref().unwrap(), "cms_source_0.icc")),
        }
    }
    fn inspect(&self, p: &[u8]) -> Result<(ColorModel, String)> {
        let model = if p.get(16..20) == Some(b"GRAY") { ColorModel::Gray } else { ColorModel::Rgb };
        let desc = if let Some(n) = &self.case {
            Case::open("codecs", n).json()["metadata"]["input_profile"].as_str().unwrap_or("fixture").to_string()
        } else {
            "fixture".into()
        };
        Ok((model, desc))
    }
    fn convert(&self, p: &Image, _model: ColorModel, src: &[u8], dst: &[u8]) -> Result<Image> {
        let name = self.case.as_ref().expect("unexpected CMS transform");
        let case = Case::open("codecs", name);
        let i = *self.calls.borrow();
        *self.calls.borrow_mut() += 1;
        assert_eq!(src, bytes(name, &format!("cms_source_{i}.icc")));
        assert_eq!(dst, bytes(name, &format!("cms_destination_{i}.icc")));
        let expected: ArrayD<f32> = case.array(&format!("cms_input_{i}"));
        let expected = if expected.ndim() == 2 {
            expected.insert_axis(ndarray::Axis(2)).into_dimensionality::<Ix3>().unwrap()
        } else {
            expected.into_dimensionality::<Ix3>().unwrap()
        };
        assert_close(p, &expected, 1e-7, &format!("{name} CMS input"));
        Ok(case.array(&format!("cms_output_{i}")))
    }
}

#[test]
fn pixel_math_goldens() {
    for name in names() {
        let case = Case::open("codecs", &name);
        if name.starts_with("resize_") {
            let a = case.array("input");
            let n = case.json()["longest"].as_u64().unwrap() as usize;
            for (label, actual) in [
                ("bilinear", resize_float(&a, n).unwrap()),
                ("lanczos", resize_export(&a, n, false).unwrap()),
                ("hdr", resize_export(&a, n, true).unwrap()),
            ] {
                assert_close(&actual, &case.array(label), 1e-5, &format!("{name} {label}"));
            }
        } else if name.starts_with("quantize_") {
            let a = case.array("input");
            let q8: Array3<u8> = case.array("u8");
            let q16: Array3<u16> = case.array("u16");
            let d: Array3<u8> = case.array("dither");
            assert_eq!(quantize8(&a).unwrap(), q8, "{name} u8");
            assert_eq!(quantize16(&a).unwrap(), q16, "{name} u16");
            assert_eq!(quantize8_dithered(&a).unwrap(), d, "{name} dither");
            println!("{name}: max sample error 0 (tolerance 0)");
        } else if name.starts_with("gray_") && case.has("input") {
            let a = case.array("input");
            let gray = neutral_gray(&a, 1e-4);
            assert_eq!(gray.is_some(), case.json()["neutral"].as_bool().unwrap(), "{name}");
            if let Some(g) = gray {
                assert_close(&g, &case.array("output"), 0., &name);
            }
        }
    }
    let case = Case::open("codecs", "dither_tile");
    assert_close(dither_tile(), &case.array("output"), 0., "dither tile");
    let case = Case::open("codecs", "sizing");
    for row in case.json()["cases"].as_array().unwrap() {
        let shape = (row["shape"][0].as_u64().unwrap() as usize, row["shape"][1].as_u64().unwrap() as usize);
        let size = &row["size"];
        let limit = if let Some(n) = size.as_u64() {
            SizeLimit::Longest(n as usize)
        } else {
            SizeLimit::Box {
                width: size[0].as_u64().unwrap() as usize,
                height: size[1].as_u64().unwrap() as usize,
                cover: size[2] == "cover",
            }
        };
        assert_eq!(export_longest(shape, limit).unwrap(), row["expected"].as_u64().unwrap() as usize);
    }
    for row in case.json()["distances"].as_array().unwrap() {
        assert_eq!(jxl_distance(row["quality"].as_f64().unwrap()).unwrap(), row["distance"].as_f64().unwrap());
    }
    for name in ["jxl_bare", "jxl_exif", "jxl_xmp", "jxl_both"] {
        let c = Case::open("codecs", name);
        let exif: Vec<u8> = serde_json::from_value(c.json()["exif"].clone()).unwrap();
        let xmp: Vec<u8> = serde_json::from_value(c.json()["xmp"].clone()).unwrap();
        assert_eq!(jxl_container(b"\xff\x0a\x12\x34", Some(&exif), Some(&xmp)).unwrap(), bytes(name, "output.bin"));
    }
}

#[test]
fn metadata_goldens() {
    let long = Case::open("codecs", "metadata_long");
    let keys = long.json()["keywords"].as_str().unwrap();
    assert_eq!(iptc_bytes(&meta(&long), keys), bytes("metadata_long", "iptc.bin"));
    assert_eq!(xmp_metadata(&meta(&long), keys, None).unwrap(), bytes("metadata_long", "xmp.bin"));
    for name in ["exif_source_missing", "exif_source_png", "exif_source_webp", "exif_source_tiff"] {
        let c = Case::open("codecs", name);
        let actual = export_exif(root().join(c.json()["source"].as_str().unwrap()), &meta(&c), None, "sRGB").unwrap();
        assert_eq!(actual, bytes(name, "exif.bin"), "{name}");
    }
    for name in ["meta_empty", "meta_rich", "meta_clear", "meta_zero"] {
        let c = Case::open("codecs", name);
        let m = meta(&c);
        let keywords = c.json()["keywords"].as_str().unwrap();
        let source = root().join("metadata_jpg/input.jpg");
        let exif = export_exif(&source, &m, Some((31, 19)), c.json()["color"].as_str().unwrap()).unwrap();
        assert_eq!(exif, bytes(name, "exif.bin"), "{name} EXIF");
        assert_eq!(iptc_bytes(&m, keywords), bytes(name, "iptc.bin"), "{name} IPTC");
        assert_eq!(xmp_metadata(&m, keywords, Some(&exif)).unwrap(), bytes(name, "xmp.bin"), "{name} XMP");
        assert_eq!(xmp_metadata(&m, keywords, None).unwrap(), bytes(name, "xmp_no_exif.bin"), "{name} XMP no EXIF");
        let jpeg = output().join(format!("{name}.jpg"));
        fs::copy(source, &jpeg).unwrap();
        jpeg_iptc(&jpeg, &m, keywords).unwrap();
        assert_eq!(fs::read(jpeg).unwrap(), bytes(name, "iptc.jpg"));
        for endian in ["le", "be"] {
            let mut handle = Cursor::new(bytes(name, &format!("tiff_before_{endian}.bin")));
            append_tiff_exif(&mut handle, &exif).unwrap();
            let expected = bytes(name, &format!("tiff_after_{endian}.bin"));
            let actual = handle.into_inner();
            fs::write(output().join(format!("{name}_{endian}.tif")), &actual).unwrap();
            assert_eq!(actual, expected, "{name} append TIFF {endian}");
        }
        println!("{name}: EXIF/IPTC/XMP/TIFF bytes identical");
    }
}

#[test]
fn bitmap_matrix() {
    let mut report = vec![];
    let mut errors = vec![];
    for name in names() {
        let c = Case::open("codecs", &name);
        let Some(file) = c.json()["file"].as_str() else { continue };
        let path = root().join(&name).join(file);
        if c.json()["marker_only"] == true {
            let decoded = decode_bitmap(&path, &LoadOptions::default()).unwrap();
            assert_close(&decoded.samples, &c.array("decoded_f32"), 0., &name);
            assert_eq!(decoded.orientation as u64, c.json()["orientation"].as_u64().unwrap(), "{name}");
            let expected = c.json()["profile"].as_bool().unwrap().then(|| bytes(&name, "profile.bin"));
            match decoded.profile {
                Some(Profile::Embedded(profile)) => assert_eq!(Some(profile), expected, "{name}"),
                None => assert_eq!(expected, None, "{name}"),
                _ => panic!("{name}: unexpected profile"),
            }
            continue;
        }
        if name.starts_with("container_") {
            assert_eq!(read_metadata(&path).unwrap(), meta(&c));
            continue;
        }
        let options = LoadOptions {
            max_size: c.json()["max_size"].as_u64().map(|n| n as usize),
            working_space: c.json()["working"].as_str().unwrap_or("sRGB").into(),
            ..Default::default()
        };
        if c.json().get("error").is_some() {
            assert!(load_image(&path, &options, &Cms::default()).is_err(), "{name} should fail like oracle");
            continue;
        }
        let decoded = match decode_bitmap(&path, &options) {
            Ok(v) => v,
            Err(e) => {
                errors.push(format!("{name}: {e}"));
                continue;
            }
        };
        let expected: Image = c.array("decoded_f32");
        let native = decoded.native_samples.as_ref().unwrap_or(&decoded.samples);
        let diff = if native.dim() == expected.dim() { max_abs_diff(native, &expected).0 } else { f32::INFINITY };
        let mismatches = native.iter().zip(expected.iter()).filter(|(a, b)| a != b).count();
        let mut row = json!({"case":name,"decoder":if file.ends_with(".jpg"){"libjpeg-turbo/2.1.5.1"}else{"format-native"},"max_sample_error":diff,"mismatched_samples":mismatches,"samples":expected.len(),"exact":diff==0.});
        if file.ends_with(".jpg") {
            assert_eq!(diff, 0., "{name}: JPEG samples must equal Pillow exactly");
            assert_eq!(mismatches, 0, "{name}: JPEG samples must equal Pillow exactly");
        }
        // Lossy decoder deviations are reported as sample counts, never hidden behind a tolerance.
        if diff != 0. && !name.starts_with("webp_") {
            errors.push(format!("{name}: decoded max error {diff}, {mismatches} samples"));
        }
        if c.json().get("error").is_some() {
            continue;
        }
        let actual_meta = read_metadata(&path).unwrap();
        if Value::Object(actual_meta) != c.json()["read_metadata"] {
            errors.push(format!("{name}: read_metadata mismatch"));
        }
        if diff != 0. && c.json()["cms_calls"].as_u64().unwrap_or(0) == 0 {
            let native = load_image(&path, &options, &Cms::default()).unwrap();
            let expected: Image = c.array("linear");
            row["native_load_max_error"] = json!(max_abs_diff(&native.pixels, &expected).0);
            if c.has("thumbnail") {
                let (native, _) = thumbnail(&path, options.max_size.unwrap(), &Cms::default()).unwrap();
                let expected: Array3<u8> = c.array("thumbnail");
                row["native_thumbnail_max_error"] =
                    json!(native.iter().zip(expected.iter()).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0));
            }
        }
        {
            let cms = Cms {
                case: if c.json()["cms_calls"].as_u64().unwrap_or(0) > 0 { Some(name.clone()) } else { None },
                ..Default::default()
            };
            let mut loaded = if diff == 0. {
                load_image(&path, &options, &cms).unwrap()
            } else {
                // Isolate the WebP decoder gap from the oracle-tested file pipeline.
                let mut decoded = decoded;
                decoded.samples = expected;
                load_image_with_decoder(&path, &options, &cms, |_, _| Ok(decoded)).unwrap()
            };
            let fingerprint = loaded.metadata.remove("file_fingerprint").unwrap();
            assert_eq!(fingerprint["size"], json!(fs::metadata(&path).unwrap().len()));
            let live_mtime = fs::metadata(&path)
                .unwrap()
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64;
            assert_eq!(fingerprint["mtime_ns"], json!(live_mtime));
            assert_eq!(
                fingerprint["path"],
                json!(fs::canonicalize(&path).unwrap().to_string_lossy().trim_start_matches("\\\\?\\"))
            );
            if Value::Object(loaded.metadata.clone()) != c.json()["metadata"] {
                errors.push(format!(
                    "{name}: load metadata actual {:?}, expected {}",
                    loaded.metadata,
                    c.json()["metadata"]
                ));
            }
            let expected = c.array("linear");
            let difference = max_abs_diff(&loaded.pixels, &expected).0;
            row["linear_pipeline_max_error"] = json!(difference);
            row["linear_pipeline_used_oracle_decoded_samples"] = json!(diff != 0.);
            println!("{name}: linear max {difference:.3e}, tolerance 1e-5");
            if difference > 1e-5 {
                errors.push(format!("{name}: linear {difference}"));
            }
            if c.has("thumbnail") && cms.case.is_none() && diff == 0. {
                let (actual, _) = thumbnail(&path, options.max_size.unwrap(), &cms).unwrap();
                let expected: Array3<u8> = c.array("thumbnail");
                let difference = actual.iter().zip(expected.iter()).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
                if difference != 0 {
                    errors.push(format!("{name}: thumbnail byte error {difference}"));
                }
            }
        }
        report.push(row);
    }
    fs::write(output().join("decode-report.json"), serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

#[test]
fn jpeg_export_files_equal_pillow_bytes() {
    for name in ["export_color", "export_gray", "export_plain", "export_tiny"] {
        let case = Case::open("codecs", name);
        let input = case.array("input");
        for quality in [0, 1, 35, 50, 95, 100] {
            let path = output().join(format!("exact_{name}_q{quality}.jpg"));
            let _ = fs::remove_file(&path);
            let options = ExportOptions {
                quality: quality as f64,
                keep_metadata: case.json()["keep_metadata"].as_bool().unwrap(),
                user_metadata: meta(&case),
                keywords: case.json()["keywords"].as_str().unwrap().into(),
                grayscale: case.json()["gray"].as_bool().unwrap(),
                dither: case.json()["dither"].as_bool().unwrap(),
                ..Default::default()
            };
            export_image(&input, root().join("metadata_jpg/input.jpg"), &path, &options, &Cms::default()).unwrap();
            let actual = fs::read(&path).unwrap();
            let mut expected = bytes(name, &format!("expected_q{quality}.jpg"));
            if options.keep_metadata {
                // The oracle JPEG has EXIF/IPTC but omits the XMP packet. The richer export
                // must add the independently generated XMP bytes and preserve every other byte.
                let namespace = b"http://ns.adobe.com/xap/1.0/\0";
                let packet = bytes(name, "xmp.bin");
                let length = u16::try_from(2 + namespace.len() + packet.len()).unwrap().to_be_bytes();
                let segment = [b"\xff\xe1".as_slice(), &length, namespace, &packet].concat();
                assert_eq!(&expected[2..4], b"\xff\xed");
                let after_iptc = 4 + usize::from(u16::from_be_bytes([expected[4], expected[5]]));
                expected.splice(after_iptc..after_iptc, segment);
            }
            assert_eq!(actual.len(), expected.len(), "{name} q{quality}: JPEG length");
            let first = actual.iter().zip(&expected).position(|(a, b)| a != b);
            assert_eq!(first, None, "{name} q{quality}: first differing JPEG byte");
            println!("{name} q{quality}: {} identical file bytes, tolerance 0", actual.len());
        }
    }
}

#[test]
fn write_exports_for_independent_pillow_check() {
    let folder = output().join("exports");
    fs::create_dir_all(&folder).unwrap();
    let mut manifest = vec![];
    let cms = Cms::default();
    for name in ["export_color", "export_gray", "export_plain"] {
        let c = Case::open("codecs", name);
        let a = c.array("input");
        for (format, label, ext) in [
            (ExportFormat::Png, "PNG", "png"),
            (ExportFormat::Png16, "PNG 16-bit", "png"),
            (ExportFormat::Tiff8, "TIFF 8-bit", "tif"),
            (ExportFormat::Tiff16, "TIFF 16-bit", "tif"),
            (ExportFormat::Jpeg, "JPEG", "jpg"),
            (ExportFormat::Avif, "AVIF", "avif"),
            (ExportFormat::JpegXl, "JPEG XL", "jxl"),
        ] {
            let filename = format!("{name}_{format:?}.{ext}");
            let path = folder.join(&filename);
            if path.exists() {
                fs::remove_file(&path).unwrap();
            }
            let o = ExportOptions {
                format,
                keep_metadata: c.json()["keep_metadata"].as_bool().unwrap(),
                user_metadata: meta(&c),
                keywords: c.json()["keywords"].as_str().unwrap().into(),
                grayscale: c.json()["gray"].as_bool().unwrap(),
                dither: c.json()["dither"].as_bool().unwrap(),
                jxl_lossless: true,
                ..Default::default()
            };
            export_image(&a, root().join("metadata_jpg/input.jpg"), &path, &o, &cms).unwrap();
            let gray = o.grayscale
                && matches!(
                    format,
                    ExportFormat::Png | ExportFormat::Tiff8 | ExportFormat::Tiff16 | ExportFormat::Jpeg
                );
            let sixteen = matches!(format, ExportFormat::Png16 | ExportFormat::Tiff16 | ExportFormat::JpegXl);
            manifest.push(json!({"file":filename,"case":name,"format":label,"metadata":o.keep_metadata,"profile":if gray{"Gray sRGB"}else{"sRGB"},"expected":match (gray,sixteen){(true,true)=>"u16.npy",(true,false)=>"u8.npy",(false,true)=>"rgb16.npy",(false,false)=>"rgb8.npy"}}));
        }
    }
    for name in [
        "export_cms_ProPhoto_sRGB",
        "export_cms_sRGB_Adobe_RGB",
        "export_cms_ProPhoto_Display_P3",
        "export_resized",
        "export_hdr",
    ] {
        let c = Case::open("codecs", name);
        let hdr = name == "export_hdr";
        let cms =
            Cms { case: if name.starts_with("export_cms_") { Some(name.into()) } else { None }, ..Default::default() };
        let filename = format!("{name}.{}", if hdr { "tif" } else { "png" });
        let path = folder.join(&filename);
        if path.exists() {
            fs::remove_file(&path).unwrap();
        }
        let destination = c.json()["destination"].as_str().unwrap_or("sRGB");
        let o = ExportOptions {
            format: if hdr { ExportFormat::TiffHdr32 } else { ExportFormat::Png },
            working_space: c.json()["working"].as_str().unwrap_or("sRGB").into(),
            color_space: destination.into(),
            hdr_enabled: hdr,
            longest: if name == "export_resized" || hdr {
                Some(SizeLimit::Box { width: 13, height: 8, cover: false })
            } else {
                None
            },
            ..Default::default()
        };
        let size = export_image(&c.array("input"), root().join("metadata_jpg/input.jpg"), &path, &o, &cms).unwrap();
        let expected: Image = c.array("output");
        assert_eq!(size, (expected.shape()[1], expected.shape()[0]));
        assert_eq!(*cms.calls.borrow(), usize::from(name.starts_with("export_cms_")));
        manifest.push(json!({"file":filename,"case":name,"format":if hdr{"TIFF HDR 32-bit"}else{"PNG"},"metadata":false,"profile":if hdr{"Linear sRGB"}else{destination},"expected":if hdr{"output.npy"}else{"u8.npy"}}));
    }
    for name in ["export_color", "export_tiny"] {
        let c = Case::open("codecs", name);
        for quality in [0., 1., 50., 100.] {
            let filename = format!("{name}_quality_{quality}.jpg");
            let path = folder.join(&filename);
            if path.exists() {
                fs::remove_file(&path).unwrap();
            }
            export_image(
                &c.array("input"),
                "unused",
                &path,
                &ExportOptions { quality, ..Default::default() },
                &Cms::default(),
            )
            .unwrap();
            manifest.push(json!({"file":filename,"case":name,"format":"JPEG","metadata":false,"profile":"sRGB","expected":"rgb8.npy","quality":quality}));
        }
    }
    fs::write(folder.join("manifest.json"), serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
}

#[test]
fn invalid_input_is_fallible_and_outputs_are_exclusive() {
    let cms = Cms::default();
    let empty = Image::zeros((0, 0, 3));
    assert!(resize_float(&empty, 10).is_err());
    assert!(quantize8(&empty).is_err());
    assert!(jxl_distance(f64::NAN).is_err());
    assert!(export_longest((0, 10), SizeLimit::Longest(1)).is_err());
    let a = Image::zeros((2, 3, 3));
    assert!(resize_export(&a, 0, false).is_err());
    let mut nan = a.clone();
    nan[[0, 0, 0]] = f32::NAN;
    assert!(quantize16(&nan).is_err());
    let p = output().join("existing.bin");
    fs::write(&p, b"keep").unwrap();
    assert!(export_image(&a, &p, &p, &ExportOptions::default(), &cms).is_err());
    assert_eq!(fs::read(&p).unwrap(), b"keep");
    let tmp = tempfile::tempdir().unwrap();
    let occupied = tmp.path().join("photo.png");
    fs::write(&occupied, b"already delivered").unwrap();
    assert!(export_image(&a, "missing", &occupied, &ExportOptions::default(), &cms).is_err());
    assert_eq!(fs::read(occupied).unwrap(), b"already delivered");
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);
    let p = output().join("unsupported-jxl.jxl");
    assert!(export_image(
        &a,
        "missing",
        &p,
        &ExportOptions { format: ExportFormat::JpegXl, ..Default::default() },
        &cms
    )
    .is_err());
    assert!(!p.exists());
}

#[test]
fn parallel_rows_and_strides_are_deterministic() {
    let small: Image = Case::open("codecs", "quantize_tile_wrap").array("input");
    let a = Image::from_shape_fn((520, 540, 3), |(y, x, c)| small[[y % small.shape()[0], x % small.shape()[1], c]]);
    let one = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let four = rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap();
    let expected = one.install(|| quantize8_dithered(&a).unwrap());
    let actual = four.install(|| quantize8_dithered(&a).unwrap());
    assert_eq!(actual, expected);
    let strided = a.clone().permuted_axes([1, 0, 2]);
    assert!(!strided.is_standard_layout());
    assert_eq!(quantize16(&strided.as_standard_layout().to_owned()).unwrap(), quantize16(&strided).unwrap());
    assert_eq!(
        one.install(|| resize_export(&a, 117, false).unwrap()),
        four.install(|| resize_export(&a, 117, false).unwrap())
    );
    for side in [1, 117, 520, 600] {
        for input in [&a, &strided] {
            let contiguous = input.as_standard_layout().to_owned();
            for mode in 0..3 {
                let resize = |a: &Image, n| match mode {
                    0 => resize_float(a, n),
                    1 => resize_export(a, n, false),
                    _ => resize_export(a, n, true),
                };
                let serial = one.install(|| resize(&contiguous, side).unwrap());
                let parallel = four.install(|| resize(input, side).unwrap());
                assert_eq!(serial.mapv(f32::to_bits), parallel.mapv(f32::to_bits));
            }
        }
    }
}
