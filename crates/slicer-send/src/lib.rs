//! Sends G-code to a printer over the network.
//!
//! Two printer interfaces with a public HTTP API are supported, both with a
//! multipart upload that can start the print right away:
//!
//! - Moonraker (Klipper): `POST /server/files/upload` with the fields
//!   `file`, `root` and `print`
//!   (<https://moonraker.readthedocs.io/en/latest/external_api/file_manager/>);
//!   the answer says `print_started`.
//! - OctoPrint: `POST /api/files/local` with the fields `file` and `print`
//!   (<https://docs.octoprint.org/en/master/api/files.html>); the answer
//!   says `effectivePrint`.
//!
//! Both take an API key in the `X-Api-Key` header
//! (<https://moonraker.readthedocs.io/en/latest/external_api/authorization/>,
//! <https://docs.octoprint.org/en/master/api/general.html>).

use std::fmt;
use std::time::Duration;

use slicer_core::{Diagnostic, Diagnostics};

#[cfg(feature = "mock")]
pub mod mock;

/// The software that answers on the printer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    /// Moonraker, the API server of Klipper.
    Moonraker,
    /// OctoPrint.
    OctoPrint,
}

/// A printer to send G-code to.
///
/// Written as `moonraker://host:port` or `octoprint://host:port`, with an
/// optional path prefix (`octoprint://host/octoprint`); `moonraker+https://`
/// and `octoprint+https://` use TLS. Credentials in the address are refused:
/// the API key is given separately and never shown.
#[derive(Clone, PartialEq, Eq)]
pub struct Target {
    service: Service,
    /// The address as written.
    address: String,
    /// `http(s)://host:port/prefix` without a trailing slash.
    base: String,
    api_key: Option<String>,
}

impl Target {
    /// Reads a printer address; an invalid one is a `send.invalid_target`
    /// error.
    pub fn parse(text: &str) -> Result<Self, Diagnostic> {
        let text = text.trim();
        // An address with credentials is not repeated anywhere.
        let shown = if text.contains('@') { "<hidden>" } else { text };
        let invalid = |reason: &str| {
            Diagnostic::error(
                "send.invalid_target",
                format!("invalid printer address '{shown}': {reason}"),
            )
            .with("target", shown)
        };
        let (scheme, rest) = text
            .split_once("://")
            .ok_or_else(|| invalid("expected moonraker://host:port or octoprint://host:port"))?;
        let (service, tls) = match scheme.to_ascii_lowercase().as_str() {
            "moonraker" | "moonraker+http" => (Service::Moonraker, false),
            "moonraker+https" => (Service::Moonraker, true),
            "octoprint" | "octoprint+http" => (Service::OctoPrint, false),
            "octoprint+https" => (Service::OctoPrint, true),
            _ => {
                return Err(invalid(
                    "the scheme must be moonraker, octoprint, moonraker+https or octoprint+https",
                ));
            }
        };
        let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        if authority.contains('@') {
            return Err(invalid(
                "credentials do not belong in the address; give the API key separately",
            ));
        }
        if authority.is_empty()
            || !authority
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || ".-_:[]".contains(c))
        {
            return Err(invalid("the host is missing or not a valid host name"));
        }
        if !path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._~/".contains(c))
        {
            return Err(invalid(
                "the path may only contain letters, digits and - . _ ~ /",
            ));
        }
        let protocol = if tls { "https" } else { "http" };
        Ok(Self {
            service,
            address: text.to_string(),
            base: format!("{protocol}://{authority}{}", path.trim_end_matches('/')),
            api_key: None,
        })
    }

    /// The same printer with an API key; an empty key counts as none.
    pub fn with_api_key(mut self, key: Option<String>) -> Self {
        self.api_key = key.filter(|key| !key.is_empty());
        self
    }

    /// The software that answers on the printer.
    pub fn service(&self) -> Service {
        self.service
    }

    /// The URL the G-code is posted to.
    pub fn upload_url(&self) -> String {
        match self.service {
            Service::Moonraker => format!("{}/server/files/upload", self.base),
            Service::OctoPrint => format!("{}/api/files/local", self.base),
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.address)
    }
}

/// Shows the address but never the API key.
impl fmt::Debug for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Target")
            .field("address", &self.address)
            .field("api_key", &self.api_key.as_ref().map(|_| "<hidden>"))
            .finish()
    }
}

/// Characters other than these are replaced in the file name sent.
fn safe_file_name(name: &str) -> String {
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-+() ".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.trim().is_empty() {
        "print.gcode".to_string()
    } else {
        name
    }
}

/// A `multipart/form-data` body with text fields and one file named `file`.
/// Returns the boundary and the body.
fn multipart(fields: &[(&str, &str)], file_name: &str, content: &[u8]) -> (String, Vec<u8>) {
    let mut boundary = String::from("spaghetti-slicer-boundary");
    while content
        .windows(boundary.len())
        .any(|window| window == boundary.as_bytes())
    {
        boundary.push('x');
    }
    let mut body = Vec::with_capacity(content.len() + 512);
    for (name, value) in fields {
        body.extend(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .bytes(),
        );
    }
    body.extend(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n"
        )
        .bytes(),
    );
    body.extend(content);
    body.extend(format!("\r\n--{boundary}--\r\n").bytes());
    (boundary, body)
}

/// What [`upload`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    /// The file name the printer got (characters other than letters, digits
    /// and `._-+() ` are replaced by `_`).
    pub file: String,
    /// Whether the printer reports that the print started.
    pub started: bool,
}

/// Uploads `gcode` to the printer as `file_name` and, with `start`, asks
/// it to print the file. A print that was asked for but not started adds
/// the warning `send.not_started`.
///
/// Errors: `send.connection` (the printer cannot be reached),
/// `send.unauthorized` (HTTP 401 or 403), `send.too_large` (HTTP 413) and
/// `send.rejected` (any other answer than success).
// ponytail: plain HTTP or TLS with the built-in web root certificates; a
// printer with a self-signed certificate cannot be reached over https.
pub fn upload(
    target: &Target,
    file_name: &str,
    gcode: &[u8],
    start: bool,
    warnings: &mut Diagnostics,
) -> Result<Sent, Diagnostic> {
    let print = if start { "true" } else { "false" };
    let fields: &[(&str, &str)] = match target.service {
        Service::Moonraker => &[("root", "gcodes"), ("print", print)],
        Service::OctoPrint => &[("print", print)],
    };
    let file = safe_file_name(file_name);
    let (boundary, body) = multipart(fields, &file, gcode);
    // A printer is on the local network: no proxy from the environment, and
    // no redirect that could lead the request (and the key) elsewhere.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .max_redirects_will_error(false)
        .proxy(None)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(Duration::from_secs(600)))
        .build()
        .into();
    let mut request = agent.post(target.upload_url()).header(
        "Content-Type",
        format!("multipart/form-data; boundary={boundary}"),
    );
    if let Some(key) = &target.api_key {
        request = request.header("X-Api-Key", key);
    }
    let fail = |code: &'static str, message: String| {
        Diagnostic::error(code, format!("{target}: {message}")).with("target", target)
    };
    let mut response = request.send(&body[..]).map_err(|error| {
        fail(
            "send.connection",
            format!("cannot reach the printer: {error}"),
        )
    })?;
    let status = response.status().as_u16();
    let answer = response
        .body_mut()
        .with_config()
        .limit(1 << 20)
        .read_to_string()
        .unwrap_or_default();
    match status {
        200..=299 => {}
        401 | 403 => {
            return Err(fail(
                "send.unauthorized",
                format!("the printer refused access (HTTP {status}); check the API key"),
            )
            .with("status", status));
        }
        413 => {
            return Err(fail(
                "send.too_large",
                "the printer (or a proxy in front of it) refused the file as too large (HTTP 413)"
                    .to_string(),
            )
            .with("status", status));
        }
        _ => {
            let excerpt: String = answer
                .chars()
                .filter(|c| !c.is_control())
                .take(200)
                .collect();
            return Err(fail(
                "send.rejected",
                format!("the printer refused the upload (HTTP {status}): {excerpt}"),
            )
            .with("status", status));
        }
    }
    let answer: serde_json::Value = serde_json::from_str(&answer).unwrap_or_default();
    let started = match target.service {
        Service::Moonraker => answer["print_started"] == true,
        Service::OctoPrint => answer["effectivePrint"] == true,
    };
    if start && !started {
        warnings.push(
            Diagnostic::warning(
                "send.not_started",
                format!(
                    "{target}: the file was uploaded, but the printer did not start it \
                     (it may be busy or not ready)"
                ),
            )
            .with("target", target),
        );
    }
    Ok(Sent { file, started })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_made_safe() {
        assert_eq!(safe_file_name("cube 2 (1).gcode"), "cube 2 (1).gcode");
        assert_eq!(safe_file_name("a\"b/c\\d\r\nä.gcode"), "a_b_c_d___.gcode");
        assert_eq!(safe_file_name(""), "print.gcode");
    }

    #[test]
    fn the_boundary_does_not_occur_in_the_file() {
        let (boundary, _) = multipart(&[], "x", b"--spaghetti-slicer-boundary");
        assert_eq!(boundary, "spaghetti-slicer-boundaryx");
    }
}
