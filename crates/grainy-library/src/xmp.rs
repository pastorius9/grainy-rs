//! Port of the sidecar half of `luma/library.py` (`write_sidecar`, `read_sidecar`) with a small ElementTree
//! work-alike, so the bytes written are the bytes Python's `xml.etree.ElementTree` writes.

use crate::pyformat::{parse_int_text, py_str};
use crate::{Error, Result};
use grainy_core::Settings;
use serde_json::{json, Value};
use std::path::Path;

/// `library.NS['x']`.
pub const NS_X: &str = "adobe:ns:meta/";
/// `library.NS['rdf']`.
pub const NS_RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
/// `library.NS['xmp']`.
pub const NS_XMP: &str = "http://ns.adobe.com/xap/1.0/";
/// `library.NS['dc']`.
pub const NS_DC: &str = "http://purl.org/dc/elements/1.1/";
/// `library.NS['lr']`.
pub const NS_LR: &str = "http://ns.adobe.com/lightroom/1.0/";
/// `library.NS['luma']`.
pub const NS_LUMA: &str = "https://luma.local/ns/1.0/";

/// The prefixes `ET.register_namespace` knows after `library.py` registered its own (module import order).
const NAMESPACE_MAP: &[(&str, &str)] = &[
    ("http://www.w3.org/XML/1998/namespace", "xml"),
    ("http://www.w3.org/1999/xhtml", "html"),
    ("http://schemas.xmlsoap.org/wsdl/", "wsdl"),
    ("http://www.w3.org/2001/XMLSchema", "xs"),
    ("http://www.w3.org/2001/XMLSchema-instance", "xsi"),
    (NS_X, "x"),
    (NS_RDF, "rdf"),
    (NS_XMP, "xmp"),
    (NS_DC, "dc"),
    (NS_LR, "lr"),
    (NS_LUMA, "luma"),
];

/// An `xml.etree.ElementTree.Element`: the tag and attribute names are in Clark form (`{uri}local`).
#[derive(Clone, Debug, Default)]
pub struct Element {
    /// Tag in Clark notation.
    pub tag: String,
    /// Attributes in document order.
    pub attrib: Vec<(String, String)>,
    /// Text before the first child.
    pub text: Option<String>,
    /// Text after the element.
    pub tail: Option<String>,
    /// Child elements.
    pub children: Vec<Element>,
}

impl Element {
    /// `Element(tag)`.
    pub fn new(tag: &str) -> Self {
        Element { tag: tag.into(), ..Default::default() }
    }
    /// `element.set(key, value)`.
    pub fn set(&mut self, key: &str, value: impl Into<String>) {
        let value = value.into();
        match self.attrib.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value,
            None => self.attrib.push((key.into(), value)),
        }
    }
    /// `element.get(key)`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.attrib.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
    /// `element.iter(tag)`: itself and all descendants with the tag, in document order.
    pub fn iter<'a>(&'a self, tag: &str, out: &mut Vec<&'a Element>) {
        if self.tag == tag {
            out.push(self);
        }
        for child in &self.children {
            child.iter(tag, out);
        }
    }
}

fn clark(namespace: Option<&str>, local: &str) -> String {
    match namespace {
        Some(ns) => format!("{{{ns}}}{local}"),
        None => local.to_owned(),
    }
}

fn convert(node: roxmltree::Node<'_, '_>) -> Element {
    let name = node.tag_name();
    let mut element = Element::new(&clark(name.namespace(), name.name()));
    for attribute in node.attributes() {
        element.attrib.push((clark(attribute.namespace(), attribute.name()), attribute.value().to_owned()));
    }
    let mut buffer = String::new();
    for child in node.children() {
        if child.is_element() {
            flush(&mut buffer, &mut element);
            element.children.push(convert(child));
        } else if child.is_text() {
            buffer.push_str(child.text().unwrap_or(""));
        }
    }
    flush(&mut buffer, &mut element);
    element
}

fn flush(buffer: &mut String, element: &mut Element) {
    if buffer.is_empty() {
        return;
    }
    let text = std::mem::take(buffer);
    match element.children.last_mut() {
        Some(last) => last.tail = Some(text),
        None => element.text = Some(text),
    }
}

/// `ET.fromstring(text)`: comments and processing instructions are dropped, as the oracle's parser does.
pub fn parse_xml(text: &str) -> Result<Element> {
    let options = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let document = roxmltree::Document::parse_with_options(text, options).map_err(|e| Error::Parse(e.to_string()))?;
    Ok(convert(document.root_element()))
}

fn decode(data: &[u8]) -> Result<String> {
    if data.starts_with(&[0xff, 0xfe]) || data.starts_with(&[0xfe, 0xff]) {
        let little = data[0] == 0xff;
        let units: Vec<u16> = data[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| if little { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
            .collect();
        return String::from_utf16(&units).map_err(|e| Error::value(e.to_string()));
    }
    let body = data.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(data);
    String::from_utf8(body.to_vec()).map_err(|e| Error::value(e.to_string()))
}

fn has_doctype_or_entity(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i + 1 < chars.len() {
        if chars[i] == '<' && chars[i + 1] == '!' {
            let mut j = i + 2;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            for word in ["doctype", "entity"] {
                let n = word.len();
                if j + n <= chars.len() && chars[j..j + n].iter().collect::<String>().to_lowercase() == word {
                    let after = chars.get(j + n);
                    if after.is_none_or(|c| !(c.is_alphanumeric() || *c == '_')) {
                        return true;
                    }
                }
            }
        }
        i += 1;
    }
    false
}

/// Port of `adobe_develop.xmp_root`.
pub fn xmp_root(data: &[u8]) -> Result<Element> {
    if data.len() > 16 * 1024 * 1024 {
        return Err(Error::Data("XMP 파일이 지원 크기를 넘었습니다.".into()));
    }
    let text = decode(data)?;
    if has_doctype_or_entity(&text) {
        return Err(Error::Data("외부 정의가 포함된 XMP는 읽지 않습니다.".into()));
    }
    let options = roxmltree::ParsingOptions { allow_dtd: false, ..Default::default() };
    match roxmltree::Document::parse_with_options(&text, options) {
        Ok(document) => Ok(convert(document.root_element())),
        Err(_) => Err(Error::Data("XMP 문서 구문을 읽지 못했습니다.".into())),
    }
}

fn escape_cdata(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn escape_attrib(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\r', "&#13;")
        .replace('\n', "&#10;")
        .replace('\t', "&#09;")
}

struct Names {
    namespaces: Vec<(String, String)>,
    qnames: Vec<(String, String)>,
}

impl Names {
    fn add(&mut self, qname: &str) {
        if self.qnames.iter().any(|(k, _)| k == qname) {
            return;
        }
        let value = if let Some(rest) = qname.strip_prefix('{') {
            let (uri, tag) = rest.rsplit_once('}').unwrap_or((rest, ""));
            let mut prefix = self.namespaces.iter().find(|(u, _)| u == uri).map(|(_, p)| p.clone());
            if prefix.is_none() {
                prefix = NAMESPACE_MAP.iter().find(|(u, _)| *u == uri).map(|(_, p)| (*p).to_owned());
                if prefix.is_none() {
                    prefix = Some(format!("ns{}", self.namespaces.len()));
                }
                let chosen = prefix.clone().unwrap_or_default();
                if chosen != "xml" {
                    self.namespaces.push((uri.to_owned(), chosen));
                }
            }
            match prefix {
                Some(p) if !p.is_empty() => format!("{p}:{tag}"),
                _ => tag.to_owned(),
            }
        } else {
            qname.to_owned()
        };
        self.qnames.push((qname.to_owned(), value));
    }
    fn get<'a>(&'a self, qname: &'a str) -> &'a str {
        self.qnames.iter().find(|(k, _)| k == qname).map(|(_, v)| v.as_str()).unwrap_or(qname)
    }
}

fn collect_names(element: &Element, names: &mut Names) {
    names.add(&element.tag);
    for (key, _) in &element.attrib {
        names.add(key);
    }
    for child in &element.children {
        collect_names(child, names);
    }
}

fn write_element(out: &mut String, element: &Element, names: &Names, namespaces: Option<&[(String, String)]>) {
    let tag = names.get(&element.tag);
    out.push('<');
    out.push_str(tag);
    if !element.attrib.is_empty() || namespaces.is_some_and(|n| !n.is_empty()) {
        if let Some(list) = namespaces {
            let mut sorted: Vec<&(String, String)> = list.iter().collect();
            sorted.sort_by(|a, b| a.1.cmp(&b.1));
            for (uri, prefix) in sorted {
                let key = if prefix.is_empty() { String::new() } else { format!(":{prefix}") };
                out.push_str(&format!(" xmlns{key}=\"{}\"", escape_attrib(uri)));
            }
        }
        for (key, value) in &element.attrib {
            out.push_str(&format!(" {}=\"{}\"", names.get(key), escape_attrib(value)));
        }
    }
    let text = element.text.as_deref().unwrap_or("");
    if !text.is_empty() || !element.children.is_empty() {
        out.push('>');
        out.push_str(&escape_cdata(text));
        for child in &element.children {
            write_element(out, child, names, None);
        }
        out.push_str("</");
        out.push_str(tag);
        out.push('>');
    } else {
        out.push_str(" />");
    }
    if let Some(tail) = element.tail.as_deref() {
        out.push_str(&escape_cdata(tail));
    }
}

/// `ET.ElementTree(root).write(path, encoding='utf-8', xml_declaration=True)` as bytes.
pub fn serialize(root: &Element) -> Vec<u8> {
    let mut names = Names { namespaces: Vec::new(), qnames: Vec::new() };
    collect_names(root, &mut names);
    let mut out = String::from("<?xml version='1.0' encoding='utf-8'?>\n");
    write_element(&mut out, root, &names, Some(&names.namespaces));
    out.into_bytes()
}

fn q(namespace: &str, local: &str) -> String {
    format!("{{{namespace}}}{local}")
}

/// Port of `library.write_sidecar`: standard rating, label and keywords plus explicit Grainy settings, written
/// next to (and replacing) the previous sidecar atomically; foreign descriptions in an existing file are kept.
/// `photo` is the catalogue row as `Catalog::photo` returns it (`rating`, `label`, `settings`,
/// `user_metadata`, `keywords`).
pub fn write_sidecar(photo: &Value, path: &Path) -> Result<()> {
    let mut root = if path.exists() {
        let data = sidecar_bytes(path)?;
        if data.len() > 16 * 1024 * 1024 {
            return Err(Error::Parse("XMP sidecar exceeds 16 MiB limit".into()));
        }
        let text = decode(&data)?;
        if has_doctype_or_entity(&text) {
            return Err(Error::Parse("DTD or ENTITY declarations are not allowed in XMP".into()));
        }
        let options = roxmltree::ParsingOptions { allow_dtd: false, ..Default::default() };
        let document =
            roxmltree::Document::parse_with_options(&text, options).map_err(|e| Error::Parse(e.to_string()))?;
        convert(document.root_element())
    } else {
        Element::new(&q(NS_X, "xmpmeta"))
    };
    let rdf_tag = q(NS_RDF, "RDF");
    let index = match root.children.iter().position(|c| c.tag == rdf_tag) {
        Some(i) => i,
        None => {
            root.children.push(Element::new(&rdf_tag));
            root.children.len() - 1
        }
    };
    let rdf = &mut root.children[index];
    let settings_key = q(NS_LUMA, "Settings");
    // Remove only previous Grainy descriptions to preserve third-party metadata.
    rdf.children.retain(|old| old.get(&settings_key).is_none());
    let mut desc = Element::new(&q(NS_RDF, "Description"));
    let object =
        |key: &str| -> Result<&Value> { photo.get(key).ok_or_else(|| Error::Type(format!("KeyError: '{key}'"))) };
    desc.set(&q(NS_XMP, "Rating"), py_str(object("rating")?));
    desc.set(&q(NS_XMP, "Label"), photo.get("label").map(py_str).unwrap_or_default());
    desc.set(&settings_key, grainy_catalog::json::dumps(object("settings")?, false, false, false));
    let empty = json!({});
    desc.set(
        &q(NS_LUMA, "Metadata"),
        grainy_catalog::json::dumps(photo.get("user_metadata").unwrap_or(&empty), false, false, false),
    );
    let keywords: Vec<String> = match object("keywords")? {
        Value::String(text) => text.split(',').map(|k| k.trim().to_owned()).filter(|k| !k.is_empty()).collect(),
        _ => return Err(Error::Type("'keywords' must be a string".into())),
    };
    for (namespace, name) in [(NS_DC, "subject"), (NS_LR, "hierarchicalSubject")] {
        let mut bag = Element::new(&q(NS_RDF, "Bag"));
        for keyword in &keywords {
            let mut item = Element::new(&q(NS_RDF, "li"));
            item.text = Some(keyword.clone());
            bag.children.push(item);
        }
        let mut holder = Element::new(&q(namespace, name));
        holder.children.push(bag);
        desc.children.push(holder);
    }
    rdf.children.push(desc);
    let mut bytes = serialize(&root);
    if cfg!(windows) {
        // `ElementTree.write(filename)` opens the file in text mode: every "\n" becomes "\r\n" on Windows.
        let mut translated = Vec::with_capacity(bytes.len() + 16);
        for b in bytes {
            if b == b'\n' {
                translated.push(b'\r');
            }
            translated.push(b);
        }
        bytes = translated;
    }
    use std::io::Write;
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| Error::os(&e, path, true))?;
    temporary.write_all(&bytes).map_err(|e| Error::os(&e, path, true))?;
    temporary.as_file().sync_all().map_err(|e| Error::os(&e, path, true))?;
    temporary.persist(path).map_err(|e| Error::os(&e.error, path, false))?;
    Ok(())
}

/// What `library.read_sidecar` returns; a field is `None` when the key is not in the dict.
#[derive(Debug, Default, Clone)]
pub struct Sidecar {
    /// `rating`, clamped to 0..=5.
    pub rating: Option<i64>,
    /// `label`.
    pub label: Option<String>,
    /// `settings`, normalized.
    pub settings: Option<Settings>,
    /// `user_metadata` (any JSON the file carries).
    pub user_metadata: Option<Value>,
    /// `keywords`, comma separated.
    pub keywords: Option<String>,
}

fn sidecar_bytes(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| Error::os(&e, path, true))?;
    let mut data = Vec::new();
    file.take(16 * 1024 * 1024 + 1).read_to_end(&mut data).map_err(|e| Error::os(&e, path, true))?;
    Ok(data)
}

/// Port of `library.read_sidecar`.
pub fn read_sidecar(path: &Path) -> Result<Sidecar> {
    let root = xmp_root(&sidecar_bytes(path)?)?;
    let mut result = Sidecar::default();
    let mut descriptions = Vec::new();
    root.iter(&q(NS_RDF, "Description"), &mut descriptions);
    for desc in descriptions {
        if let Some(rating) = desc.get(&q(NS_XMP, "Rating")) {
            let n = parse_int_text(rating).ok_or_else(|| {
                Error::value(format!("invalid literal for int() with base 10: {}", crate::pathx::py_repr(rating)))
            })?;
            result.rating = Some(n.clamp(0, 5));
        }
        if let Some(label) = desc.get(&q(NS_XMP, "Label")) {
            result.label = Some(label.to_owned());
        }
        if let Some(edits) = desc.get(&q(NS_LUMA, "Settings")) {
            let value: Value = serde_json::from_str(edits)?;
            if !value.is_object() {
                return Err(Error::Type("'settings' of a sidecar must be an object".into()));
            }
            result.settings = Some(grainy_catalog::settings(&value)?);
        }
        if let Some(meta) = desc.get(&q(NS_LUMA, "Metadata")) {
            result.user_metadata = Some(serde_json::from_str(meta)?);
        }
    }
    let keywords_of = |name: &str| -> Vec<Option<String>> {
        let tag = q(if name == "subject" { NS_DC } else { NS_LR }, name);
        let mut found = Vec::new();
        fn descendants<'a>(element: &'a Element, out: &mut Vec<&'a Element>) {
            for child in &element.children {
                out.push(child);
                descendants(child, out);
            }
        }
        let mut all = Vec::new();
        descendants(&root, &mut all);
        for holder in all.into_iter().filter(|e| e.tag == tag) {
            for bag in holder.children.iter().filter(|e| e.tag == q(NS_RDF, "Bag")) {
                for item in bag.children.iter().filter(|e| e.tag == q(NS_RDF, "li")) {
                    found.push(item.text.clone());
                }
            }
        }
        found
    };
    let mut keywords = keywords_of("hierarchicalSubject");
    if keywords.is_empty() {
        keywords = keywords_of("subject");
    }
    if !keywords.is_empty() {
        let mut unique: Vec<String> = Vec::new();
        for text in keywords.into_iter().flatten().filter(|t| !t.is_empty()) {
            if !unique.contains(&text) {
                unique.push(text);
            }
        }
        result.keywords = Some(unique.join(", "));
    }
    Ok(result)
}

#[test]
fn oversized_existing_sidecar_is_bounded_before_writing() {
    let file = tempfile::NamedTempFile::new().unwrap();
    file.as_file().set_len(32 * 1024 * 1024).unwrap();
    assert_eq!(sidecar_bytes(file.path()).unwrap().len(), 16 * 1024 * 1024 + 1);
    assert!(write_sidecar(&Value::Null, file.path()).is_err());
    assert_eq!(file.as_file().metadata().unwrap().len(), 32 * 1024 * 1024);
}
