//! The stdio server loop: one JSON-RPC message per line in, one reply per line out, as the stdio bridges
//! (`assets/mcp/grainy-mcp-bridge.*`) relay them to `POST /mcp/stdio`: the stdio flavour of the protocol (every
//! reply status 200), a parse error answered with `-32700`, notifications answered with nothing. Used by a
//! windowless `grainy-cli mcp` and by the protocol-level tests.

use std::io::{BufRead, Write};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::protocol::{Flavour, Protocol, DEADLINE_SECONDS};
use crate::tools::Tools;

/// `mcp_http.PARSE_ERROR`.
pub fn parse_error() -> Value {
    json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "Parse error"}})
}

/// One line in, the reply line out (`None` for a notification or a client's response).
pub fn handle_line(line: &str, protocol: &Protocol, tools: &dyn Tools) -> Option<Vec<u8>> {
    let body = match serde_json::from_str::<Value>(line) {
        Err(_) => Some(parse_error()),
        Ok(message) => {
            let deadline = Instant::now() + Duration::from_secs(DEADLINE_SECONDS);
            protocol.handle(&message, &[], Flavour::Stdio, tools, deadline).body
        }
    };
    body.map(|b| crate::pyjson::dumps(&b))
}

/// Serve until the input ends: each non-empty line is one message.
pub fn serve(
    input: impl BufRead,
    mut output: impl Write,
    protocol: &Protocol,
    tools: &dyn Tools,
) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle_line(&line, protocol, tools) {
            output.write_all(&reply)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
    Ok(())
}
