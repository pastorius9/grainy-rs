//! A test double for [`Tools`], like `FakeTools` in the oracle's `tests/test_mcp_protocol.py`: the real
//! definitions, a fixed result (or a fault) and a record of the calls.

use std::cell::RefCell;
use std::time::Instant;

use serde_json::{json, Value};

use crate::tools::{definitions, ToolFault, ToolResult, Tools};

/// Returns a fixed result for every call and records `(name, arguments, modern)`.
pub struct StubTools {
    result: Value,
    fail: bool,
    /// The calls received, in order.
    pub calls: RefCell<Vec<(String, Value, bool)>>,
}

impl StubTools {
    /// The oracle's default fake: `{'content':[{'type':'text','text':'{}'}],'isError':False}`.
    pub fn new() -> Self {
        Self::with_result(json!({"content": [{"type": "text", "text": "{}"}], "isError": false}))
    }

    /// A stub that answers every call with `result`.
    pub fn with_result(result: Value) -> Self {
        Self { result, fail: false, calls: RefCell::new(Vec::new()) }
    }

    /// A stub whose calls raise (`FakeTools(fail=True)`): the protocol answers -32603.
    pub fn failing() -> Self {
        Self { fail: true, ..Self::new() }
    }
}

impl Default for StubTools {
    fn default() -> Self {
        Self::new()
    }
}

impl Tools for StubTools {
    fn definitions(&self) -> &[Value] {
        definitions()
    }

    fn call(&self, name: &str, arguments: &Value, modern: bool, _deadline: Instant) -> ToolResult {
        self.calls.borrow_mut().push((name.to_string(), arguments.clone(), modern));
        if self.fail {
            return Err(ToolFault("boom".into()));
        }
        Ok(self.result.clone())
    }
}
