//! The network of feedback and updates: one switch that turns every network use of the program off, and (feature
//! `http`) the HTTP client behind it, the oracle's `urllib.request.urlopen` with its timeouts and errors.
//!
//! **The switch.** The environment variable [`OFFLINE_VARIABLE`] (`GRAINY_OFFLINE=1`; any value except empty and
//! `0`) or a call of [`set_offline`] (the application's `--offline`) makes every request here fail with
//! [`Failure::Offline`] before anything is opened. Tests and smoke runs set it.
//!
//! **The client** (`http` feature): `ureq` 2 with the operating system's TLS (`native-tls`: SChannel on Windows,
//! Security.framework on macOS), so certificates are checked against the system's own store and no certificate
//! bundle ships with the program. Timeouts are per connect, read and write, as `urlopen(timeout=...)` sets the
//! socket's timeout; redirects are followed (release downloads are redirected to GitHub's file host).

use std::sync::atomic::{AtomicBool, Ordering};

/// The environment variable that switches the network off (`GRAINY_OFFLINE=1`).
pub const OFFLINE_VARIABLE: &str = "GRAINY_OFFLINE";

static OFFLINE: AtomicBool = AtomicBool::new(false);

/// Whether a value of [`OFFLINE_VARIABLE`] switches the network off: anything but empty and `0`.
pub fn switches_off(value: Option<&str>) -> bool {
    value.is_some_and(|v| !v.trim().is_empty() && v.trim() != "0")
}

/// Whether the network is switched off ([`set_offline`] or [`OFFLINE_VARIABLE`]).
pub fn offline() -> bool {
    OFFLINE.load(Ordering::SeqCst) || switches_off(std::env::var(OFFLINE_VARIABLE).ok().as_deref())
}

/// Switches the network off for the rest of this process (`--offline`). It cannot be switched on again.
pub fn set_offline() {
    OFFLINE.store(true, Ordering::SeqCst);
}

/// How a request failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The network is switched off: nothing was opened.
    Offline,
    /// The server answered with an error status (`urllib.error.HTTPError`): the code and its reason phrase.
    Status(u16, String),
    /// The request failed before an answer (`URLError`, `OSError`): the text of the reason.
    Other(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Offline => write!(f, "offline"),
            Failure::Status(code, reason) if reason.is_empty() => write!(f, "HTTP {code}"),
            Failure::Status(_, reason) => write!(f, "{reason}"),
            Failure::Other(text) => write!(f, "{text}"),
        }
    }
}

/// Header name and value pairs.
pub type Headers<'a> = &'a [(String, String)];

#[cfg(feature = "http")]
mod client {
    use std::io::Read;
    use std::sync::Arc;
    use std::time::Duration;

    use super::{offline, Failure, Headers};

    fn agent(timeout: u64, budget: u64) -> Result<ureq::Agent, Failure> {
        let tls = native_tls::TlsConnector::new().map_err(|e| Failure::Other(e.to_string()))?;
        let timeout = Duration::from_secs(timeout);
        Ok(ureq::AgentBuilder::new()
            .https_only(true)
            .tls_connector(Arc::new(tls))
            .timeout(Duration::from_secs(budget))
            .timeout_connect(timeout)
            .timeout_read(timeout)
            .timeout_write(timeout)
            .build())
    }

    /// The text Python gives for a failure before an answer: `timed out` for a timeout, else the reason.
    fn transport_text(error: &ureq::Transport) -> String {
        let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(error);
        while let Some(cause) = source {
            if let Some(io) = cause.downcast_ref::<std::io::Error>() {
                if matches!(io.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) {
                    return "timed out".into();
                }
            }
            source = cause.source();
        }
        let text = error.to_string();
        if text.is_empty() {
            "OSError".into()
        } else {
            text
        }
    }

    fn failure(error: ureq::Error) -> Failure {
        match error {
            ureq::Error::Status(code, response) => Failure::Status(code, response.status_text().to_owned()),
            ureq::Error::Transport(transport) => Failure::Other(transport_text(&transport)),
        }
    }

    /// Posts `body` to `url` with `headers` (the oracle's `urlopen(Request(url, data, headers), timeout)`);
    /// returns the status of the answer (2xx) or the failure.
    pub fn post(url: &str, headers: Headers<'_>, body: &[u8], timeout: u64) -> Result<u16, Failure> {
        if offline() {
            return Err(Failure::Offline);
        }
        let mut request = agent(timeout, timeout)?.post(url);
        for (name, value) in headers {
            request = request.set(name, value);
        }
        request.send_bytes(body).map(|response| response.status()).map_err(failure)
    }

    /// Opens `url` for reading with `headers` (the oracle's `urlopen(Request(url, headers), timeout)`); the body
    /// is read as it arrives.
    pub fn get(url: &str, headers: Headers<'_>, timeout: u64) -> Result<Box<dyn Read + Send + Sync>, Failure> {
        get_with_budget(url, headers, timeout, timeout)
    }

    /// Open a download with a separate upper bound on the entire HTTPS transfer.
    pub fn get_with_budget(
        url: &str,
        headers: Headers<'_>,
        timeout: u64,
        budget: u64,
    ) -> Result<Box<dyn Read + Send + Sync>, Failure> {
        if offline() {
            return Err(Failure::Offline);
        }
        let mut request = agent(timeout, budget)?.get(url);
        for (name, value) in headers {
            request = request.set(name, value);
        }
        request.call().map(|response| response.into_reader()).map_err(failure)
    }

    #[cfg(test)]
    #[test]
    fn plaintext_is_rejected_before_a_connection_is_opened() {
        // ureq applies the same policy to each redirect, before resolving its host.
        let error = agent(1, 1).unwrap().get("http://example.invalid/").call().unwrap_err();
        assert_eq!(error.kind(), ureq::ErrorKind::InsecureRequestHttpsOnly);
    }
}

#[cfg(feature = "http")]
pub use client::{get, get_with_budget, post};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_variable_switches_off_unless_empty_or_zero() {
        assert!(!switches_off(None));
        assert!(!switches_off(Some("")));
        assert!(!switches_off(Some(" 0 ")));
        assert!(switches_off(Some("1")));
        assert!(switches_off(Some("yes")));
    }

    #[cfg(feature = "http")]
    #[test]
    fn nothing_is_opened_while_offline() {
        set_offline();
        assert!(offline());
        // Addresses that would fail at once anyway: nothing reaches the network either way.
        assert_eq!(post("https://example.invalid/", &[], b"", 1), Err(Failure::Offline));
        assert!(matches!(get("https://example.invalid/", &[], 1), Err(Failure::Offline)));
    }
}
