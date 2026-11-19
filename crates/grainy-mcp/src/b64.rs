//! Standard base64 (RFC 4648 with padding), matching Python's `base64.b64encode` and
//! `base64.b64decode(value, validate=True)`.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Port of `base64.b64encode(data).decode('ascii')`.
pub fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[n as usize & 63] as char } else { '=' });
    }
    out
}

fn sextet(byte: u8) -> Option<u32> {
    match byte {
        b'A'..=b'Z' => Some(u32::from(byte - b'A')),
        b'a'..=b'z' => Some(u32::from(byte - b'a') + 26),
        b'0'..=b'9' => Some(u32::from(byte - b'0') + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Port of `base64.b64decode(value, validate=True)`: characters outside the alphabet, misplaced or excess
/// padding and a length that is not a multiple of four are errors (`None`). Non-zero trailing bits are accepted,
/// as Python accepts them.
pub fn decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    let body_len = bytes.iter().position(|&b| b == b'=').unwrap_or(bytes.len());
    let (body, padding) = bytes.split_at(body_len);
    if padding.len() > 2 || padding.iter().any(|&b| b != b'=') {
        return None;
    }
    if body.iter().any(|&b| sextet(b).is_none()) {
        return None;
    }
    if (body.len() + padding.len()) % 4 != 0 {
        return None;
    }
    // Padding must complete exactly the last group: "ab=" with one pad is wrong for two data characters.
    let leftover = body.len() % 4;
    let expected = match leftover {
        0 => 0,
        2 => 2,
        3 => 1,
        _ => return None,
    };
    if padding.len() != expected {
        return None;
    }
    let mut out = Vec::with_capacity(body.len() / 4 * 3 + 2);
    for group in body.chunks(4) {
        let mut n = 0u32;
        for (i, &b) in group.iter().enumerate() {
            n |= sextet(b)? << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if group.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if group.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}
