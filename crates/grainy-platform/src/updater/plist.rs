//! The bundle identifier of an `Info.plist`, read without any macOS tool so the checks of a staged bundle
//! (`updater::check_bundle_folder`) are the same on every system and tested everywhere. A property list is XML
//! (read with `roxmltree`) or binary (`bplist00`, read here); nothing else is needed than the string under one key
//! of the top-level dictionary. Malformed input gives none, never a panic.

/// The top-level `CFBundleIdentifier` of a property list (XML or binary), or none.
pub fn bundle_identifier(bytes: &[u8]) -> Option<String> {
    top_level_string(bytes, "CFBundleIdentifier")
}

/// The string under `key` in the top-level dictionary of a property list, or none (no such key, a value that is
/// not a string, a list that cannot be read).
pub fn top_level_string(bytes: &[u8], key: &str) -> Option<String> {
    if bytes.starts_with(b"bplist00") {
        binary_string(bytes, key)
    } else {
        xml_string(bytes, key)
    }
}

/// All the text directly inside an element (several text nodes when CDATA is mixed in).
fn text_of(node: roxmltree::Node<'_, '_>) -> String {
    node.children().filter(|child| child.is_text()).filter_map(|child| child.text()).collect()
}

fn xml_string(bytes: &[u8], key: &str) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    // Apple's lists carry a DOCTYPE; roxmltree still refuses entity expansion attacks.
    let options = roxmltree::ParsingOptions { allow_dtd: true, nodes_limit: 1_000_000, ..Default::default() };
    let document = roxmltree::Document::parse_with_options(text, options).ok()?;
    let root = document.root_element();
    if root.tag_name().name() != "plist" {
        return None;
    }
    let dict = root.children().find(|node| node.is_element())?;
    if dict.tag_name().name() != "dict" {
        return None;
    }
    let mut children = dict.children().filter(|node| node.is_element());
    while let Some(name) = children.next() {
        let value = children.next()?;
        if name.tag_name().name() != "key" {
            return None;
        }
        if text_of(name) == key {
            return (value.tag_name().name() == "string").then(|| text_of(value));
        }
    }
    None
}

/// A big-endian unsigned number of `size` bytes (1 to 8) at `at`.
fn number(bytes: &[u8], at: usize, size: usize) -> Option<usize> {
    if !(1..=8).contains(&size) {
        return None;
    }
    let slice = bytes.get(at..at.checked_add(size)?)?;
    usize::try_from(slice.iter().fold(0u64, |value, &byte| (value << 8) | u64::from(byte))).ok()
}

/// A binary property list: its bytes and the facts of its trailer.
struct Binary<'a> {
    bytes: &'a [u8],
    offset_size: usize,
    reference_size: usize,
    count: usize,
    table: usize,
}

impl Binary<'_> {
    /// Where object `index` starts.
    fn offset(&self, index: usize) -> Option<usize> {
        if index >= self.count {
            return None;
        }
        number(self.bytes, self.table.checked_add(index.checked_mul(self.offset_size)?)?, self.offset_size)
    }

    /// The object number stored at `at`.
    fn reference(&self, at: usize) -> Option<usize> {
        number(self.bytes, at, self.reference_size)
    }

    /// Object `index`'s type (the marker's high four bits), its length and where its content starts.
    fn header(&self, index: usize) -> Option<(u8, usize, usize)> {
        let at = self.offset(index)?;
        let marker = *self.bytes.get(at)?;
        let (kind, low) = (marker >> 4, marker & 0x0f);
        if low != 0x0f {
            return Some((kind, usize::from(low), at + 1));
        }
        // A length of 15 or more follows as an integer object: 0x1n and 2^n bytes.
        let integer = *self.bytes.get(at + 1)?;
        if integer >> 4 != 0x1 {
            return None;
        }
        let size = 1usize << (integer & 0x0f);
        let length = number(self.bytes, at + 2, size)?;
        Some((kind, length, at + 2 + size))
    }

    /// Object `index` when it is a string (ASCII `0x5n` or UTF-16 `0x6n`).
    fn string(&self, index: usize) -> Option<String> {
        let (kind, length, start) = self.header(index)?;
        match kind {
            0x5 => String::from_utf8(self.bytes.get(start..start.checked_add(length)?)?.to_vec()).ok(),
            0x6 => {
                let raw = self.bytes.get(start..start.checked_add(length.checked_mul(2)?)?)?;
                let units: Vec<u16> = raw.as_chunks::<2>().0.iter().map(|pair| u16::from_be_bytes(*pair)).collect();
                String::from_utf16(&units).ok()
            }
            _ => None,
        }
    }
}

fn binary_string(bytes: &[u8], key: &str) -> Option<String> {
    let trailer = bytes.len().checked_sub(32)?;
    if trailer < 8 {
        return None;
    }
    let list = Binary {
        bytes,
        offset_size: usize::from(bytes[trailer + 6]),
        reference_size: usize::from(bytes[trailer + 7]),
        count: number(bytes, trailer + 8, 8)?,
        table: number(bytes, trailer + 24, 8)?,
    };
    let top = number(bytes, trailer + 16, 8)?;
    let (kind, entries, keys) = list.header(top)?;
    if kind != 0xd {
        return None;
    }
    // A dictionary: `entries` key references, then as many value references.
    let span = entries.checked_mul(list.reference_size)?;
    let values = keys.checked_add(span)?;
    if values.checked_add(span)? > bytes.len() {
        return None;
    }
    for entry in 0..entries {
        let name = list.reference(keys + entry * list.reference_size)?;
        if list.string(name).as_deref() == Some(key) {
            return list.string(list.reference(values + entry * list.reference_size)?);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_or_broken_lists_give_none() {
        for bytes in [
            &b""[..],
            b"bplist00",
            b"bplist00\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
            b"<plist>",
        ] {
            assert_eq!(bundle_identifier(bytes), None);
        }
        // One dictionary object whose length says 2^64 - 1 entries: refused before any entry is read.
        let mut huge = b"bplist00\xdf\x13\xff\xff\xff\xff\xff\xff\xff\xff".to_vec();
        huge.push(8); // the offset table at 18: object 0 starts at 8
        huge.extend([0u8; 6]);
        huge.extend([1, 1]); // offset size, reference size
        huge.extend(1u64.to_be_bytes()); // objects
        huge.extend(0u64.to_be_bytes()); // top object
        huge.extend(18u64.to_be_bytes()); // offset table
        assert_eq!(bundle_identifier(&huge), None, "a dictionary longer than the file");
        // The same list with a length of 1 and a reference past the objects.
        let mut short = b"bplist00\xd1\x05\x06".to_vec();
        short.push(8);
        short.extend([0u8; 6]);
        short.extend([1, 1]);
        short.extend(1u64.to_be_bytes());
        short.extend(0u64.to_be_bytes());
        short.extend(11u64.to_be_bytes());
        assert_eq!(bundle_identifier(&short), None, "a key that is not an object");
    }
}
