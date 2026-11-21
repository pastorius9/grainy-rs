//! Still-image formats accepted by the browser, the RAW reader and Adobe imports.

macro_rules! formats {
    (raw: [$($raw:literal),+], bitmap: [$($bitmap:literal),+]) => {
        /// Extensions returned by Path::extension, without a leading dot.
        pub const RAW_NAMES: &[&str] = &[$($raw),+];
        /// RAW suffixes including the dot, as stored in catalogs and preset rules.
        pub const RAW_EXTENSIONS: [&str; RAW_NAMES.len()] = [$(concat!(".", $raw)),+];
        /// All supported photo suffixes, for file choosers and directory scanning.
        pub const IMAGE_EXTENSIONS: [&str; RAW_NAMES.len() + [$($bitmap),+].len()] =
            [$(concat!(".", $raw)),+, $(concat!(".", $bitmap)),+];
    };
}

formats!(
    raw: ["arw", "cr2", "cr3", "nef", "nrw", "raf", "rw2", "rwl", "orf", "pef", "dng", "srw", "3fr", "iiq", "kdc", "mos"],
    bitmap: ["jpg", "jpeg", "png", "tif", "tiff", "bmp", "webp", "avif", "jxl"]
);
