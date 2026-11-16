//! Uploads against a stand-in printer API on 127.0.0.1; nothing here talks
//! to a real printer.
#![allow(clippy::unwrap_used, missing_docs)]

use slicer_core::Diagnostics;
use slicer_send::mock::{Field, Printer};
use slicer_send::{Service, Target, upload};

const GCODE: &[u8] = b"; test\nG28\nG1 X10 Y10 E1\n";

fn file(name: &str) -> Field {
    Field {
        name: "file".to_string(),
        file_name: Some(name.to_string()),
        content: GCODE.to_vec(),
    }
}

fn text(name: &str, value: &str) -> Field {
    Field {
        name: name.to_string(),
        file_name: None,
        content: value.as_bytes().to_vec(),
    }
}

#[test]
fn moonraker_gets_the_file_and_starts_it() {
    let printer =
        Printer::start(201, r#"{"print_started": true, "action": "create_file"}"#).unwrap();
    let target = Target::parse(&format!("moonraker://{}", printer.address))
        .unwrap()
        .with_api_key(Some("key-123".to_string()));
    assert_eq!(target.service(), Service::Moonraker);
    let mut warnings = Diagnostics::new();
    let sent = upload(&target, "cube.gcode", GCODE, true, &mut warnings).unwrap();
    assert!(sent.started);
    assert_eq!(sent.file, "cube.gcode");
    assert!(warnings.is_empty());

    let request = printer.request().unwrap();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/server/files/upload");
    assert_eq!(request.header("x-api-key"), Some("key-123"));
    assert!(
        request
            .header("content-type")
            .unwrap()
            .starts_with("multipart/form-data; boundary=")
    );
    assert_eq!(
        request.fields(),
        [
            text("root", "gcodes"),
            text("print", "true"),
            file("cube.gcode")
        ]
    );
}

#[test]
fn octoprint_gets_the_file_under_a_path_prefix() {
    let printer = Printer::start(201, r#"{"done": true, "effectivePrint": false}"#).unwrap();
    let target = Target::parse(&format!("octoprint://{}/octo/", printer.address))
        .unwrap()
        .with_api_key(Some("abc".to_string()));
    let mut warnings = Diagnostics::new();
    // Asked to start, but the printer did not: uploaded with a warning.
    let sent = upload(&target, "a\"b.gcode", GCODE, true, &mut warnings).unwrap();
    assert!(!sent.started);
    assert_eq!(sent.file, "a_b.gcode");
    assert_eq!(warnings.codes(), ["send.not_started"]);

    let request = printer.request().unwrap();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/octo/api/files/local");
    assert_eq!(request.header("x-api-key"), Some("abc"));
    assert_eq!(request.fields(), [text("print", "true"), file("a_b.gcode")]);
}

#[test]
fn without_key_and_start_neither_is_sent() {
    let printer = Printer::start(201, "{}").unwrap();
    let target = Target::parse(&format!("octoprint://{}", printer.address))
        .unwrap()
        .with_api_key(Some(String::new()));
    let mut warnings = Diagnostics::new();
    assert!(
        !upload(&target, "x.gcode", GCODE, false, &mut warnings)
            .unwrap()
            .started
    );
    assert!(warnings.is_empty());
    let request = printer.request().unwrap();
    assert_eq!(request.header("x-api-key"), None);
    assert_eq!(request.field("print").unwrap(), b"false");
}

#[test]
fn printer_errors_have_their_own_codes() {
    for (status, code) in [
        (401, "send.unauthorized"),
        (403, "send.unauthorized"),
        (413, "send.too_large"),
        (409, "send.rejected"),
        (500, "send.rejected"),
        (302, "send.rejected"),
    ] {
        let printer = Printer::start(status, r#"{"error": "nope"}"#).unwrap();
        let target = Target::parse(&format!("moonraker://{}", printer.address))
            .unwrap()
            .with_api_key(Some("secret-key".to_string()));
        let error = upload(&target, "x.gcode", GCODE, true, &mut Diagnostics::new()).unwrap_err();
        assert_eq!(error.code, code, "{status}: {error}");
        assert_eq!(error.context["status"], status.to_string());
        assert!(!error.message.contains("secret-key"));
        printer.request().unwrap();
    }

    // Nobody listens on a port that was just freed.
    let address = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let target = Target::parse(&format!("moonraker://{address}")).unwrap();
    let error = upload(&target, "x.gcode", GCODE, false, &mut Diagnostics::new()).unwrap_err();
    assert_eq!(error.code, "send.connection", "{error}");
    assert_eq!(error.context["target"], format!("moonraker://{address}"));
}

#[test]
fn addresses_are_checked() {
    let target = Target::parse(" moonraker+https://printer.local:7125/ ").unwrap();
    assert_eq!(
        target.upload_url(),
        "https://printer.local:7125/server/files/upload"
    );
    assert_eq!(target.to_string(), "moonraker+https://printer.local:7125/");
    assert_eq!(
        Target::parse("OctoPrint://[::1]:5000")
            .unwrap()
            .upload_url(),
        "http://[::1]:5000/api/files/local"
    );
    for bad in [
        "printer.local",
        "http://printer.local",
        "moonraker://",
        "moonraker:///path",
        "moonraker://host?x=1",
        "moonraker://host/a b",
        "octoprint://user:password@host",
    ] {
        let error = Target::parse(bad).unwrap_err();
        assert_eq!(error.code, "send.invalid_target", "{bad}");
        assert!(!error.message.contains("password"), "{error}");
    }
    let target = Target::parse("moonraker://host")
        .unwrap()
        .with_api_key(Some("secret-key".to_string()));
    assert!(!format!("{target:?}").contains("secret-key"));
}
