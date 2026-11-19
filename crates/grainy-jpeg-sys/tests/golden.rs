use grainy_jpeg_sys::{decode, encode, EncodeOptions};
use grainy_testkit::Case;
use ndarray::Array3;
use std::fs;

#[test]
fn every_pillow_decode_and_draft_is_exact() {
    let root = grainy_testkit::root().join("golden/codecs");
    let mut count = 0;
    for folder in fs::read_dir(&root).unwrap() {
        let folder = folder.unwrap();
        let path = folder.path();
        if !path.join("input.jpg").exists() {
            continue;
        }
        let name = folder.file_name().to_string_lossy().into_owned();
        let case = Case::open("codecs", &name);
        let input = fs::read(path.join("input.jpg")).unwrap();
        if case.json().get("jpeg_decode_error").is_some() {
            assert!(decode(&input, None).is_err(), "{name}: must reject like Pillow");
            continue;
        }
        for row in case.json()["jpeg_drafts"].as_array().expect("run gen_codecs.py") {
            let max = row["max_size"].as_u64().map(|v| v as usize);
            let actual = decode(&input, max).unwrap_or_else(|e| panic!("{name} {max:?}: {e}"));
            let expected: Array3<u8> = case.array(row["array"].as_str().unwrap());
            assert_eq!(
                (actual.height as usize, actual.width as usize, actual.channels as usize),
                expected.dim(),
                "{name}"
            );
            assert_eq!(actual.full_width as u64, case.json()["jpeg_full_size"][0].as_u64().unwrap());
            assert_eq!(actual.full_height as u64, case.json()["jpeg_full_size"][1].as_u64().unwrap());
            let difference = actual.pixels.iter().zip(&expected).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
            println!("{name} draft {max:?}: max sample error {difference}, tolerance 0");
            assert_eq!(difference, 0, "{name} draft {max:?}");
            count += 1;
        }
    }
    assert!(count > 200, "JPEG matrix unexpectedly small: {count}");
    println!("{count} full/reduced decodes are exact");
}

#[test]
fn pillow_encode_bytes_are_exact() {
    let root = grainy_testkit::root().join("golden/codecs");
    let mut count = 0;
    for folder in fs::read_dir(&root).unwrap() {
        let folder = folder.unwrap();
        let name = folder.file_name().to_string_lossy().into_owned();
        if !name.starts_with("jpeg_encode_") {
            continue;
        }
        let case = Case::open("codecs", &name);
        let pixels: Array3<u8> = case.array("pixels");
        let (h, w, c) = pixels.dim();
        let icc = fs::read(folder.path().join("icc.bin")).unwrap();
        let exif = fs::read(folder.path().join("exif.bin")).unwrap();
        let actual = encode(
            pixels.as_slice().unwrap(),
            &EncodeOptions {
                width: w as u32,
                height: h as u32,
                channels: c as u32,
                quality: case.json()["quality"].as_i64().unwrap() as i32,
                icc: &icc,
                exif: &exif,
            },
        )
        .unwrap();
        let expected = fs::read(folder.path().join("expected.jpg")).unwrap();
        assert_eq!(actual.len(), expected.len(), "{name}: file lengths differ");
        let first = actual.iter().zip(&expected).position(|(a, b)| a != b);
        assert_eq!(first, None, "{name}: first differing byte");
        println!("{name}: {} identical bytes, tolerance 0", actual.len());
        count += 1;
    }
    assert_eq!(count, 45);
}

#[test]
fn invalid_inputs_are_fallible_and_calls_are_independent() {
    for data in [&[][..], b"not JPEG", &[255, 216, 255, 217], &[255, 216, 255, 224, 0, 16]] {
        assert!(decode(data, None).is_err());
    }
    let data = fs::read(grainy_testkit::root().join("golden/codecs/jpeg_baseline_420_q95/input.jpg")).unwrap();
    for cut in [2, 16, data.len() / 2] {
        assert!(decode(&data[..cut], None).is_err());
    }
    assert!(decode(&data, Some(0)).is_err());
    for (width, height, channels) in [(0, 1, 3), (1, 0, 3), (1, 1, 2), (u32::MAX, u32::MAX, 4), (65501, 1, 3)] {
        assert!(encode(&[], &EncodeOptions { width, height, channels, quality: 95, icc: &[], exif: &[] }).is_err());
    }
    let exif = vec![0; 65534];
    assert!(encode(
        &[1, 2, 3],
        &EncodeOptions { width: 1, height: 1, channels: 3, quality: 95, icc: &[], exif: &exif }
    )
    .is_err());
    let expected = decode(&data, None).unwrap().pixels;
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let data = &data;
            let expected = &expected;
            scope.spawn(move || {
                for _ in 0..8 {
                    assert_eq!(&decode(data, None).unwrap().pixels, expected);
                }
            });
        }
    });
}
