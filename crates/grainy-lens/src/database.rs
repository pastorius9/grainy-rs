use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use roxmltree::Node;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{invalid, Result};

/// Port of `optics.cameras`: a lensfun camera's public data.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Camera {
    /// `camera.maker`.
    pub maker: String,
    /// `camera.model`.
    pub model: String,
    /// `camera.variant` (empty if absent).
    pub variant: String,
    /// `camera.mount`.
    pub mount: String,
    /// `camera.crop_factor`, rounded to float32 by the XML reader.
    pub crop_factor: f32,
}

/// Port of the calibration data consumed by `optics.profile_lens`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Calibration {
    /// XML model name.
    pub model: String,
    /// Calibrated focal length.
    pub focal: f32,
    /// Calibrated aperture (vignetting only).
    pub aperture: f32,
    /// Calibrated distance (vignetting only).
    pub distance: f32,
    /// Public lensfunpy term order (unused entries are zero).
    pub terms: Vec<f32>,
}

/// Port of `optics.profile_lens`: lens metadata and calibration entries.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Lens {
    /// `lens.maker`.
    pub maker: String,
    /// `lens.model`.
    pub model: String,
    /// `lens.mounts`.
    pub mounts: Vec<String>,
    /// Calibration crop factor.
    pub crop_factor: f32,
    /// Calibration sensor aspect ratio (long side / short side).
    pub aspect_ratio: f32,
    /// Minimum focal length from XML, name, or calibrations.
    pub min_focal: f32,
    /// Maximum focal length.
    pub max_focal: f32,
    /// Minimum aperture from XML/name.
    pub min_aperture: f32,
    /// Maximum aperture from XML/name.
    pub max_aperture: f32,
    /// Lens projection; correction preserves the source projection.
    pub projection: String,
    /// `lens.calib_distortion`.
    pub distortion: Vec<Calibration>,
    /// `lens.calib_tca`.
    pub tca: Vec<Calibration>,
    /// `lens.calib_vignetting`.
    pub vignetting: Vec<Calibration>,
}

/// Port of each dictionary from `optics.lens_records`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LensRecord {
    /// First 20 hex digits of the saved XML's SHA256.
    pub id: String,
    /// Database maker.
    pub maker: String,
    /// Database model.
    pub model: String,
    /// XML crop factor without float32 rounding (as in the oracle's record).
    pub crop: f64,
    /// Lens mounts.
    pub mounts: Vec<String>,
    /// A standalone, reproducible calibration document.
    pub xml: String,
    /// Original database filename.
    pub file: String,
}

/// Port of `optics.database`, `cameras`, and `lens_records`; the caller supplies the database folder.
#[derive(Clone, Debug, Default)]
pub struct Database {
    cameras: Vec<Camera>,
    records: Vec<LensRecord>,
    mounts: BTreeMap<String, Vec<String>>,
    // The native database replaces near-identical entries, while lens_records keeps every XML record.
    matching_lenses: Vec<Lens>,
}

fn text(node: Node<'_, '_>, name: &str) -> String {
    node.children()
        .find(|n| n.has_tag_name(name) && n.attribute("lang").is_none())
        .and_then(|n| n.text())
        .unwrap_or("")
        .to_owned()
}

fn number(value: Option<&str>, default: f32) -> Result<f32> {
    let value = match value {
        Some(v) => v.parse().map_err(|_| invalid("invalid number in lens XML"))?,
        None => default,
    };
    if !value.is_finite() {
        return Err(invalid("non-finite number in lens XML"));
    }
    Ok(value)
}

fn child_number(node: Node<'_, '_>, name: &str, default: f32) -> Result<f32> {
    number(node.children().find(|n| n.has_tag_name(name)).and_then(|n| n.text()), default)
}

fn escape(s: &str, attribute: bool) -> String {
    let mut s = s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    if attribute {
        s = s.replace('"', "&quot;").replace('\n', "&#10;").replace('\r', "&#13;").replace('\t', "&#09;");
    }
    s
}

// ElementTree's default serializer, including comments and the lens element's trailing whitespace.
fn serialize(node: Node<'_, '_>, out: &mut String) {
    if node.is_text() {
        out.push_str(&escape(node.text().unwrap_or(""), false));
    } else if node.is_comment() {
        out.push_str("<!--");
        out.push_str(node.text().unwrap_or(""));
        out.push_str("-->");
    } else if node.is_element() {
        out.push('<');
        out.push_str(node.tag_name().name());
        for a in node.attributes() {
            out.push(' ');
            out.push_str(a.name());
            out.push_str("=\"");
            out.push_str(&escape(a.value(), true));
            out.push('"');
        }
        if node.children().next().is_none() {
            out.push_str(" />");
        } else {
            out.push('>');
            for child in node.children() {
                serialize(child, out);
            }
            out.push_str("</");
            out.push_str(node.tag_name().name());
            out.push('>');
        }
    }
}

fn calibration(n: Node<'_, '_>, kind: &str) -> Result<Calibration> {
    let model = n.attribute("model").unwrap_or("none");
    let names: &[(&str, f32)] = match (kind, model) {
        ("distortion", "poly3") => &[("k1", 0.), ("k2", 0.), ("k3", 0.)],
        ("distortion", "poly5") => &[("k1", 0.), ("k2", 0.), ("k3", 0.)],
        ("distortion", "ptlens") => &[("a", 0.), ("b", 0.), ("c", 0.)],
        ("tca", "linear") => {
            &[("kr", 1.), ("kb", 1.), ("unused1", 0.), ("unused2", 0.), ("unused3", 0.), ("unused4", 0.)]
        }
        ("tca", "poly3") => &[("vr", 1.), ("vb", 1.), ("cr", 0.), ("cb", 0.), ("br", 0.), ("bb", 0.)],
        ("vignetting", "pa") => &[("k1", 0.), ("k2", 0.), ("k3", 0.)],
        (_, "none") => &[],
        _ => return Err(invalid(&format!("unsupported {kind} model {model}"))),
    };
    Ok(Calibration {
        model: model.into(),
        focal: number(n.attribute("focal"), 0.)?,
        aperture: number(n.attribute("aperture"), 0.)?,
        distance: number(n.attribute("distance"), 0.)?,
        terms: names.iter().map(|(name, d)| number(n.attribute(*name), *d)).collect::<Result<_>>()?,
    })
}

fn numeric_run(s: &str) -> Option<(f32, f32)> {
    let s = s.trim();
    let end = s.find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-').unwrap_or(s.len());
    let mut parts = s[..end].split('-');
    let a = parts.next()?.parse().ok()?;
    let b = parts.next().and_then(|v| v.parse().ok()).unwrap_or(a);
    Some((a, b))
}

fn name_bounds(model: &str) -> ((f32, f32), (f32, f32)) {
    let lower = model.to_lowercase();
    let focal = lower
        .find("mm")
        .and_then(|i| {
            let start = lower[..i]
                .char_indices()
                .rev()
                .find(|(_, c)| !c.is_ascii_digit() && *c != '.' && *c != '-')
                .map_or(0, |(p, c)| p + c.len_utf8());
            numeric_run(&lower[start..i])
        })
        .unwrap_or((0., 0.));
    let aperture_text = lower.find("mm").map_or(lower.as_str(), |i| lower[i + 2..].trim_start());
    let aperture = aperture_text
        .find("f/")
        .filter(|i| *i == 0)
        .and_then(|i| numeric_run(&aperture_text[i + 2..]))
        .or_else(|| aperture_text.strip_prefix("1:").and_then(numeric_run))
        .or_else(|| aperture_text.strip_prefix('f').and_then(numeric_run))
        .map(|(min, _)| (min, 0.))
        .unwrap_or((0., 0.));
    (focal, aperture)
}

pub(crate) fn parse_lens(node: Node<'_, '_>) -> Result<Lens> {
    let model = text(node, "model");
    let (mut focal, mut aperture) = name_bounds(&model);
    for (name, bounds) in [("focal", &mut focal), ("aperture", &mut aperture)] {
        if let Some(n) = node.children().find(|n| n.has_tag_name(name)) {
            if let Some(v) = n.attribute("value") {
                let v = number(Some(v), 0.)?;
                *bounds = (v, v);
            } else {
                *bounds = (number(n.attribute("min"), bounds.0)?, number(n.attribute("max"), bounds.1)?);
            }
        }
    }
    let mut distortion = Vec::new();
    let mut tca = Vec::new();
    let mut vignetting = Vec::new();
    for c in node.children().filter(|n| n.has_tag_name("calibration")) {
        for n in c.children().filter(Node::is_element) {
            let kind = n.tag_name().name();
            let dst = match kind {
                "distortion" => &mut distortion,
                "tca" => &mut tca,
                "vignetting" => &mut vignetting,
                _ => continue,
            };
            let entry = calibration(n, kind)?;
            if entry.model != "none" {
                dst.push(entry);
            }
        }
    }
    if focal.0 == 0. {
        focal.0 = distortion.iter().chain(&tca).chain(&vignetting).map(|c| c.focal).reduce(f32::min).unwrap_or(0.);
    }
    if focal.1 == 0. {
        focal.1 = distortion.iter().chain(&tca).chain(&vignetting).map(|c| c.focal).reduce(f32::max).unwrap_or(focal.0);
    }
    if aperture.0 == 0. {
        aperture.0 = vignetting.iter().map(|c| c.aperture).reduce(f32::min).unwrap_or(0.);
    }
    if aperture.1 == 0. {
        aperture.1 = vignetting.iter().map(|c| c.aperture).reduce(f32::max).unwrap_or(0.);
    }
    let ratio = text(node, "aspect-ratio");
    let aspect = if let Some((a, b)) = ratio.split_once(':') {
        number(Some(a), 0.)? / number(Some(b), 1.)?
    } else {
        number((!ratio.is_empty()).then_some(ratio.as_str()), 1.5)?
    };
    let crop = child_number(node, "cropfactor", 1.)?;
    if crop <= 0. || aspect <= 0. || !aspect.is_finite() || !(1. / aspect).is_finite() {
        return Err(invalid("non-positive calibration crop/aspect"));
    }
    Ok(Lens {
        maker: text(node, "maker"),
        model,
        mounts: node
            .children()
            .filter(|n| n.has_tag_name("mount"))
            .filter_map(|n| n.text().map(str::to_owned))
            .collect(),
        crop_factor: crop,
        aspect_ratio: aspect.max(1. / aspect),
        min_focal: focal.0,
        max_focal: focal.1,
        min_aperture: aperture.0,
        max_aperture: aperture.1,
        projection: {
            let t = text(node, "type");
            if t.is_empty() {
                "rectilinear".into()
            } else {
                t
            }
        },
        distortion,
        tca,
        vignetting,
    })
}

pub(crate) fn document(xml: &str) -> Result<roxmltree::Document<'_>> {
    let upper = xml.to_ascii_uppercase();
    if upper.contains("<!DOCTYPE") || upper.contains("<!ENTITY") {
        return Err(invalid("렌즈 보정 데이터가 올바르지 않습니다."));
    }
    let doc = roxmltree::Document::parse(xml)?;
    if !doc.root_element().has_tag_name("lensdatabase") {
        return Err(invalid("렌즈 프로파일 하나가 필요합니다."));
    }
    Ok(doc)
}

impl Database {
    /// Port of `optics.database`/`lens_records`: load only `*.xml` from an explicit folder.
    pub fn from_folder(folder: impl AsRef<Path>) -> Result<Self> {
        let mut files = std::fs::read_dir(folder)?.map(|e| e.map(|e| e.path())).collect::<std::io::Result<Vec<_>>>()?;
        files.retain(|p| p.extension().is_some_and(|x| x == "xml"));
        files.sort();
        let mut db = Self::default();
        for path in files {
            db.add_xml(&std::fs::read_to_string(&path)?, &path.file_name().unwrap_or_default().to_string_lossy())?;
        }
        db.sort();
        Ok(db)
    }

    /// The database that is embedded in the program (the unmodified Lensfun XML files of `data/lensfun/`): what
    /// `lf.Database(load_common=False)` loads from lensfunpy's bundled `db_files` in the oracle.
    pub fn builtin() -> Result<Self> {
        Self::from_sources(crate::builtin::FILES.iter().copied())
    }

    /// Number of XML files embedded in the program.
    pub fn builtin_file_count() -> usize {
        crate::builtin::file_count()
    }

    /// A database from `(file name, XML text)` pairs, in the order given (`from_folder` passes them sorted).
    pub fn from_sources<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> Result<Self> {
        let mut db = Self::default();
        for (name, xml) in files {
            db.add_xml(xml, name)?;
        }
        db.sort();
        Ok(db)
    }

    /// Port of `lf.Database(xml=..., load_common=False, load_bundled=False)` in `optics._saved_lens`.
    pub fn from_xml(xml: &str, filename: &str) -> Result<Self> {
        let mut db = Self::default();
        db.add_xml(xml, filename)?;
        db.sort();
        Ok(db)
    }

    fn add_xml(&mut self, xml: &str, filename: &str) -> Result<()> {
        // XML 1.0 line ends: CRLF or a lone CR is a LF (expat does this in the oracle; roxmltree keeps them in comments).
        let normalized;
        let xml = if xml.contains('\r') {
            normalized = xml.replace("\r\n", "\n").replace('\r', "\n");
            normalized.as_str()
        } else {
            xml
        };
        let doc = document(xml)?;
        for n in doc.root_element().children().filter(Node::is_element) {
            match n.tag_name().name() {
                "mount" => {
                    self.mounts.insert(
                        text(n, "name"),
                        n.children()
                            .filter(|n| n.has_tag_name("compat"))
                            .filter_map(|n| n.text().map(str::to_owned))
                            .collect(),
                    );
                }
                "camera" => self.cameras.push(Camera {
                    maker: text(n, "maker"),
                    model: text(n, "model"),
                    variant: text(n, "variant"),
                    mount: text(n, "mount"),
                    crop_factor: child_number(n, "cropfactor", 1.)?,
                }),
                "lens" => {
                    let lens = parse_lens(n)?;
                    if let Some(i) = self.matching_lenses.iter().position(|existing| {
                        existing.maker.to_lowercase() == lens.maker.to_lowercase()
                            && existing.model.to_lowercase() == lens.model.to_lowercase()
                            && (existing.crop_factor - lens.crop_factor).abs() < 0.01
                    }) {
                        self.matching_lenses[i] = lens.clone();
                    } else {
                        self.matching_lenses.push(lens.clone());
                    }
                    let mut xml = String::from("<lensdatabase version=\"1\">");
                    serialize(n, &mut xml);
                    if let Some(t) = n.next_sibling().filter(Node::is_text).and_then(|n| n.text()) {
                        xml.push_str(&escape(t, false));
                    }
                    xml.push_str("</lensdatabase>");
                    let digest = format!("{:x}", Sha256::digest(xml.as_bytes()));
                    let crop = text(n, "cropfactor").parse().unwrap_or(1.);
                    self.records.push(LensRecord {
                        id: digest[..20].into(),
                        maker: lens.maker,
                        model: lens.model,
                        crop,
                        mounts: lens.mounts,
                        xml,
                        file: filename.into(),
                    });
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn sort(&mut self) {
        self.cameras.sort_by(|a, b| (&a.maker, &a.model, &a.variant).cmp(&(&b.maker, &b.model, &b.variant)));
        self.records.sort_by(|a, b| (&a.maker, &a.model).cmp(&(&b.maker, &b.model)).then(a.crop.total_cmp(&b.crop)));
    }

    /// Port of `optics.cameras`, in its deterministic order.
    pub fn cameras(&self) -> &[Camera] {
        &self.cameras
    }
    /// Port of `optics.lens_records`, in its deterministic order.
    pub fn lens_records(&self) -> &[LensRecord] {
        &self.records
    }
    /// Port of `optics.compatible_lenses` (mount compatibility is directional).
    pub fn compatible_lenses(&self, camera: Option<&Camera>) -> Vec<&LensRecord> {
        let Some(c) = camera else { return self.records.iter().collect() };
        let valid: BTreeSet<_> = self
            .matching_lenses
            .iter()
            .filter(|lens| {
                let mount = lens
                    .mounts
                    .iter()
                    .any(|m| m == &c.mount || self.mounts.get(&c.mount).is_some_and(|v| v.contains(m)));
                mount && f64::from(lens.crop_factor) * 0.96 <= f64::from(c.crop_factor)
            })
            .map(|lens| (lens.maker.as_str(), lens.model.as_str(), format!("{:.3}", lens.crop_factor)))
            .collect();
        self.records
            .iter()
            .filter(|r| valid.contains(&(r.maker.as_str(), r.model.as_str(), format!("{:.3}", r.crop))))
            .collect()
    }
}
