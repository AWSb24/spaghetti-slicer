//! Tests of the HTTP API, driven through the router without a socket.
#![allow(clippy::unwrap_used, missing_docs)]

use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use slicer_core::Mesh;
use slicer_server::{App, Config};
use tower::ServiceExt;

const BOUNDARY: &str = "test-boundary";

fn multipart(fields: &[(&str, &[u8])]) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, content) in fields {
        body.extend(
            format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n")
                .bytes(),
        );
        body.extend(*content);
        body.extend(b"\r\n");
    }
    body.extend(format!("--{BOUNDARY}--\r\n").bytes());
    body
}

async fn send(router: &Router, request: Request<Body>) -> (StatusCode, Vec<u8>) {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, body.to_vec())
}

async fn call(router: &Router, method: &str, path: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let (status, body) = send(router, request).await;
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

async fn create(router: &Router, fields: &[(&str, &[u8])]) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri("/v1/jobs")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(multipart(fields)))
        .unwrap();
    let (status, body) = send(router, request).await;
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// Polls a job until it is no longer queued or running.
async fn finished(router: &Router, id: &str) -> Value {
    for _ in 0..600 {
        let (status, job) = call(router, "GET", &format!("/v1/jobs/{id}")).await;
        assert_eq!(status, StatusCode::OK);
        if job["status"] != "queued" && job["status"] != "running" {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("job did not finish");
}

fn cube() -> Vec<u8> {
    slicer_formats::write_stl_binary(&Mesh::cuboid(10.0, 10.0, 2.0))
}

#[tokio::test]
async fn health_and_unknown_paths() {
    let router = App::new(Config::default()).router();
    let (status, body) = call(&router, "GET", "/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");

    let (status, body) = call(&router, "GET", "/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "request.not_found");
}

#[tokio::test]
async fn browser_app_without_a_build() {
    let router = App::new(Config::default()).router();
    let (status, body) = call(&router, "GET", "/app/").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "web.not_installed");
}

#[tokio::test]
async fn browser_app_from_a_directory() {
    let dir = std::env::temp_dir().join(format!("slicer-web-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("index.html"), "<canvas id=\"slicer\">").unwrap();
    std::fs::write(dir.join("assets/slicer_bg.wasm"), b"\0asm").unwrap();
    std::fs::write(dir.with_extension("secret"), "outside").unwrap();
    let router = App::new(Config {
        web_dir: Some(dir.clone()),
        token: Some("secret".to_string()),
        ..Config::default()
    })
    .router();

    let get = |uri: &str| Request::get(uri).body(Body::empty()).unwrap();
    // No token needed: the page is public, the API it would call is not.
    let response = router.clone().oneshot(get("/app/")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/html; charset=utf-8"
    );
    let response = router.clone().oneshot(get("/app")).await.unwrap();
    assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(response.headers()[header::LOCATION], "/app/");
    let response = router
        .clone()
        .oneshot(get("/app/assets/slicer_bg.wasm"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/wasm");
    let (status, body) = send(&router, get("/app/assets/slicer_bg.wasm")).await;
    assert_eq!((status, body.as_slice()), (StatusCode::OK, &b"\0asm"[..]));

    // Nothing outside the directory, however the path is written.
    for missing in ["/app/nope.js", "/app/../x", "/app/%2e%2e/x"] {
        let (status, _) = send(&router, get(missing)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    }
    let secret = format!(
        "/app/..%2f{}",
        dir.with_extension("secret")
            .file_name()
            .unwrap()
            .to_string_lossy()
    );
    let (status, _) = send(&router, get(&secret)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_file(dir.with_extension("secret")).unwrap();
}

#[tokio::test]
async fn the_api_description_matches_the_router() {
    let router = App::new(Config {
        token: Some("secret".to_string()),
        ..Config::default()
    })
    .router();
    // Served without a token, like the health check.
    let (status, served) = call(&router, "GET", "/openapi.json").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(served, slicer_server::openapi());
    assert_eq!(served["openapi"], "3.1.0");

    // Every operation exists: with a token it is answered by its handler,
    // not by the fallback, and never with "method not allowed".
    let id = "00000000-0000-4000-8000-000000000000";
    let mut described = Vec::new();
    for (path, methods) in served["paths"].as_object().unwrap() {
        for (method, operation) in methods.as_object().unwrap() {
            described.push((method.as_str(), path.as_str()));
            let request = Request::builder()
                .method(method.to_uppercase().as_str())
                .uri(path.replace("{id}", id))
                .header(header::AUTHORIZATION, "Bearer secret")
                .body(Body::empty())
                .unwrap();
            let (status, body) = send(&router, request).await;
            let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            assert_ne!(status, StatusCode::METHOD_NOT_ALLOWED, "{method} {path}");
            assert_ne!(
                body["error"]["code"], "request.not_found",
                "{method} {path}"
            );
            // The answer is one the description announces.
            let announced = operation["responses"].as_object().unwrap();
            assert!(
                announced.contains_key(status.as_str()),
                "{method} {path} answered {status}"
            );
        }
    }
    // The router is built from this list, so a route that is not
    // described (or described but not routed) shows up here.
    let mut routed = slicer_server::operations();
    described.sort_unstable();
    routed.sort_unstable();
    assert_eq!(described, routed);
}

#[tokio::test]
async fn the_status_shows_the_progress() {
    let router = App::new(Config::default()).router();
    let stl = slicer_formats::write_stl_binary(&Mesh::cuboid(30.0, 30.0, 40.0));
    let (status, created) = create(&router, &[("model", &stl), ("set", b"layer_height=0.1")]).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(created["progress"], 0.0);
    let id = created["id"].as_str().unwrap();
    let mut seen = Vec::new();
    let job = loop {
        let (_, job) = call(&router, "GET", &format!("/v1/jobs/{id}")).await;
        let progress = job["progress"].as_f64().unwrap();
        assert!((0.0..=1.0).contains(&progress), "{job}");
        assert_eq!(job["step"].is_string(), job["status"] == "running", "{job}");
        seen.push(progress);
        if job["status"] != "queued" && job["status"] != "running" {
            break job;
        }
        assert!(seen.len() < 6000, "job did not finish");
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    assert_eq!(job["status"], "done", "{job}");
    assert_eq!(job["progress"], 1.0);
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");
    // The description announces both fields.
    let properties = &slicer_server::openapi()["components"]["schemas"]["Job"]["properties"];
    assert!(properties["progress"].is_object() && properties["step"].is_object());
}

#[tokio::test]
async fn a_job_is_sliced_in_the_background() {
    let router = App::new(Config::default()).router();
    let stl = cube();
    let (status, created) = create(
        &router,
        &[
            ("model", &stl),
            ("settings", b"layer_height = 0.3\nwall_loops = 3\n"),
            ("settings", b"layer_height = 0.1\n"),
            ("set", b"infill_density=30"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(created["status"], "queued");
    let id = created["id"].as_str().unwrap();

    let job = finished(&router, id).await;
    assert_eq!(job["status"], "done", "{job}");
    // A 0.2 mm first layer and 18 layers of 0.1 mm.
    assert_eq!(job["stats"]["layers"], 19);
    assert_eq!(job["warnings"].as_array().unwrap().len(), 0);

    let request = Request::builder()
        .uri(job["gcode_url"].as_str().unwrap())
        .body(Body::empty())
        .unwrap();
    let (status, gcode) = send(&router, request).await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8(gcode).unwrap().contains(";LAYER_CHANGE"));

    // The same result as a project file for Bambu Lab printers.
    let request = Request::builder()
        .uri(job["gcode_3mf_url"].as_str().unwrap())
        .body(Body::empty())
        .unwrap();
    let (status, package) = send(&router, request).await;
    assert_eq!(status, StatusCode::OK);
    assert!(slicer_formats::looks_like_3mf(&package));

    // Deleting a finished job removes it.
    let (status, _) = call(&router, "DELETE", &format!("/v1/jobs/{id}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = call(&router, "GET", &format!("/v1/jobs/{id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "job.not_found");
}

/// Whether `bytes` contain `part`.
fn contains(bytes: &[u8], part: &[u8]) -> bool {
    bytes.windows(part.len()).any(|window| window == part)
}

#[tokio::test]
async fn thumbnails_go_into_the_gcode_and_the_project_file() {
    let router = App::new(Config::default()).router();
    let (_, created) = create(&router, &[("model", &cube()), ("set", b"thumbnails=32x24")]).await;
    let job = finished(&router, created["id"].as_str().unwrap()).await;
    assert_eq!(job["status"], "done", "{job}");
    let request = |uri: &str| Request::builder().uri(uri).body(Body::empty()).unwrap();
    let (_, gcode) = send(&router, request(job["gcode_url"].as_str().unwrap())).await;
    let gcode = String::from_utf8(gcode).unwrap();
    assert!(
        gcode.contains("\n; thumbnail begin 32x24 "),
        "{}",
        &gcode[..400]
    );
    assert_eq!(gcode.matches("; thumbnail end\n").count(), 1);
    let (status, package) = send(&router, request(job["gcode_3mf_url"].as_str().unwrap())).await;
    assert_eq!(status, StatusCode::OK);
    assert!(contains(&package, b"Metadata/plate_1.png"));
}

#[tokio::test]
async fn a_3mf_model_is_accepted() {
    let router = App::new(Config::default()).router();
    let model = slicer_formats::write_3mf(&Mesh::cuboid(10.0, 10.0, 2.0)).unwrap();
    let (_, created) = create(&router, &[("model", &model)]).await;
    let job = finished(&router, created["id"].as_str().unwrap()).await;
    assert_eq!(job["status"], "done", "{job}");
}

/// Slices a job to the end and returns its G-code.
async fn sliced(router: &Router, fields: &[(&str, &[u8])]) -> String {
    let (status, created) = create(router, fields).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    let job = finished(router, created["id"].as_str().unwrap()).await;
    assert_eq!(job["status"], "done", "{job}");
    let request = Request::builder()
        .uri(job["gcode_url"].as_str().unwrap())
        .body(Body::empty())
        .unwrap();
    let (status, gcode) = send(router, request).await;
    assert_eq!(status, StatusCode::OK);
    String::from_utf8(gcode).unwrap()
}

#[tokio::test]
async fn an_object_gets_settings_of_its_own() {
    let router = App::new(Config::default()).router();
    let stl = cube();
    let two = [("model", &stl[..]), ("model", &stl[..])];
    let plain = sliced(&router, &two).await;
    let one_wall = sliced(
        &router,
        &[&two[..], &[("object_settings_2", b"wall_loops = 1")]].concat(),
    )
    .await;
    assert_ne!(plain, one_wall);
    // Several fields for one object are layered; the plate's own value
    // prints as without settings of its own.
    let fields = [
        &two[..],
        &[
            ("object_settings_2", b"wall_loops = 1"),
            ("object_settings_2", b"wall_loops = 2"),
        ],
    ]
    .concat();
    assert_eq!(plain, sliced(&router, &fields).await);
}

#[tokio::test]
async fn object_settings_are_checked() {
    let router = App::new(Config::default()).router();
    let stl = cube();
    let (status, body) = create(
        &router,
        &[
            ("model", &stl),
            ("object_settings_1", b"wall_loops = 1\nlayer_height = 0.1"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "settings.not_per_object");
    assert_eq!(body["error"]["context"]["key"], "layer_height");
    assert_eq!(body["error"]["context"]["field"], "object_settings_1");
    assert!(
        body["error"]["context"]["allowed"]
            .as_str()
            .unwrap()
            .contains("wall_loops")
    );

    let (status, body) = create(
        &router,
        &[("model", &stl), ("object_settings_1", b"wall_loops = 0")],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "settings.out_of_range");

    for name in [
        "object_settings_2",
        "object_settings_0",
        "object_settings_x",
    ] {
        let (status, body) = create(&router, &[("model", &stl), (name, b"wall_loops = 1")]).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name}");
        assert_eq!(body["error"]["code"], "request.invalid", "{name}");
    }
}

/// A project file as Orca Slicer and Bambu Studio write it, reduced to
/// what is read: two cubes (objects 1 and 2), the project's settings and,
/// with `object_settings`, settings of object 2 of its own. The same as in
/// the command line tool's tests.
fn orca_project(object_settings: bool) -> Vec<u8> {
    let cube = Mesh::cuboid(20.0, 20.0, 6.0);
    let vertices: String = cube
        .vertices
        .iter()
        .map(|[x, y, z]| format!("<vertex x=\"{x}\" y=\"{y}\" z=\"{z}\"/>"))
        .collect();
    let triangles: String = cube
        .triangles
        .iter()
        .map(|[a, b, c]| format!("<triangle v1=\"{a}\" v2=\"{b}\" v3=\"{c}\"/>"))
        .collect();
    let mesh =
        format!("<mesh><vertices>{vertices}</vertices><triangles>{triangles}</triangles></mesh>");
    let model = format!(
        "<?xml version=\"1.0\"?><model unit=\"millimeter\" xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\"><resources><object id=\"1\" type=\"model\">{mesh}</object><object id=\"2\" type=\"model\">{mesh}</object></resources><build><item objectid=\"1\" transform=\"1 0 0 0 1 0 0 0 1 80 100 0\"/><item objectid=\"2\" transform=\"1 0 0 0 1 0 0 0 1 140 100 0\"/></build></model>"
    );
    let config = r#"{
    "name": "project_settings",
    "from": "project",
    "layer_height": "0.25",
    "initial_layer_print_height": "0.25",
    "wall_loops": "3",
    "sparse_infill_density": ["15%"],
    "brim_type": "no_brim",
    "brim_width": "5",
    "made_up_setting": "7"
}"#;
    let own = if object_settings {
        "<metadata key=\"sparse_infill_density\" value=\"60%\"/><metadata key=\"layer_height\" value=\"0.1\"/><part id=\"1\" subtype=\"normal_part\"><metadata key=\"name\" value=\"Cube\"/></part><part id=\"2\" subtype=\"modifier_part\"><metadata key=\"wall_loops\" value=\"6\"/></part>"
    } else {
        ""
    };
    let settings = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><config><object id=\"1\"><metadata key=\"name\" value=\"Left\"/><metadata key=\"extruder\" value=\"1\"/></object><object id=\"2\"><metadata key=\"name\" value=\"Right\"/><metadata key=\"extruder\" value=\"1\"/>{own}</object><plate><metadata key=\"plater_id\" value=\"1\"/><model_instance><metadata key=\"object_id\" value=\"1\"/></model_instance><model_instance><metadata key=\"object_id\" value=\"2\"/></model_instance></plate></config>"
    );
    slicer_formats::write_package(&[
        ("3D/3dmodel.model", &model),
        ("Metadata/project_settings.config", config),
        ("Metadata/model_settings.config", &settings),
    ])
    .unwrap()
}

/// Slices a job to the end and returns its G-code and the codes of its
/// warnings, with the warnings themselves.
async fn sliced_with_warnings(router: &Router, fields: &[(&str, &[u8])]) -> (String, Vec<Value>) {
    let (status, created) = create(router, fields).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    let job = finished(router, created["id"].as_str().unwrap()).await;
    assert_eq!(job["status"], "done", "{job}");
    let request = Request::builder()
        .uri(job["gcode_url"].as_str().unwrap())
        .body(Body::empty())
        .unwrap();
    let (status, gcode) = send(router, request).await;
    assert_eq!(status, StatusCode::OK);
    let warnings = job["warnings"].as_array().unwrap().clone();
    (String::from_utf8(gcode).unwrap(), warnings)
}

#[tokio::test]
async fn settings_stored_in_a_project_file_are_used_on_request() {
    let router = App::new(Config::default()).router();
    let project = orca_project(true);
    let plain = orca_project(false);
    let codes = |warnings: &[Value]| -> Vec<String> {
        warnings
            .iter()
            .map(|w| w["code"].as_str().unwrap().to_string())
            .collect()
    };
    let on: (&str, &[u8]) = ("project_settings", b"true");

    // Without the field (or with false) the stored settings are ignored.
    let (without, warnings) = sliced_with_warnings(&router, &[("model", &project)]).await;
    assert!(without.contains("; layer_height = 0.2\n"));
    assert!(warnings.is_empty(), "{warnings:?}");
    let off: (&str, &[u8]) = ("project_settings", b"false");
    assert_eq!(without, sliced(&router, &[("model", &project), off]).await);

    // With it the project's settings apply, and what has no counterpart is
    // reported, for the plate and for the object.
    let (with, warnings) = sliced_with_warnings(&router, &[("model", &project), on]).await;
    assert!(with.contains("; layer_height = 0.25\n"), "{with}");
    assert!(with.contains("; wall_loops = 3\n"));
    assert!(with.contains("; brim_width = 0\n"));
    let skipped = warnings
        .iter()
        .find(|w| w["code"] == "project.settings_skipped")
        .unwrap();
    assert_eq!(skipped["context"]["keys"], "made_up_setting");
    let object = warnings
        .iter()
        .find(|w| w["code"] == "project.object_settings_skipped")
        .unwrap();
    assert_eq!(object["context"]["keys"], "layer_height");
    assert!(object["message"].as_str().unwrap().contains("'Right'"));
    assert!(codes(&warnings).contains(&"project.part_settings_skipped".to_string()));

    // The object's own infill makes a difference.
    let plate_only = sliced(&router, &[("model", &plain), on]).await;
    assert_ne!(with, plate_only);
    // object_settings_<n> go over the object's settings from the project:
    // at the project's 15 % object 2 prints as without its own.
    let sparse: (&str, &[u8]) = ("object_settings_1", b"infill_density = 15");
    assert_eq!(
        sliced(&router, &[("model", &project), on, sparse]).await,
        plate_only
    );
    // The settings and set fields go over the project's settings.
    let fields = [
        ("model", &project[..]),
        ("settings", b"wall_loops = 2"),
        ("set", b"layer_height=0.3"),
        on,
    ];
    let set = sliced(&router, &fields).await;
    assert!(set.contains("; layer_height = 0.3\n"));
    assert!(set.contains("; wall_loops = 2\n"));
    assert!(set.contains("; brim_width = 0\n"));

    // With several models the warnings name the model by its number; a
    // model without stored settings is reported.
    let bare = slicer_formats::write_3mf(&Mesh::cuboid(10.0, 10.0, 4.0)).unwrap();
    let (_, warnings) =
        sliced_with_warnings(&router, &[("model", &project), ("model", &bare), on]).await;
    let skipped = warnings
        .iter()
        .find(|w| w["code"] == "project.settings_skipped")
        .unwrap();
    assert_eq!(skipped["context"]["model"], "1");
    assert!(
        skipped["message"]
            .as_str()
            .unwrap()
            .starts_with("model 1: ")
    );
    let none = warnings
        .iter()
        .find(|w| w["code"] == "project.no_settings")
        .unwrap();
    assert_eq!(none["context"]["model"], "2");

    // The field takes true or false only, and is described.
    let (status, body) = create(
        &router,
        &[("model", &project), ("project_settings", b"yes")],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "request.invalid");
    let fields = &slicer_server::openapi()["paths"]["/v1/jobs"]["post"]["requestBody"]["content"]["multipart/form-data"]
        ["schema"]["properties"];
    assert_eq!(fields["project_settings"]["type"], "boolean");
}

#[tokio::test]
async fn errors_have_the_same_shape_as_the_command_line() {
    let router = App::new(Config::default()).router();

    // Invalid requests are refused at once.
    let (status, body) = create(&router, &[("set", b"layer_height=0.1")]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "request.invalid");

    let stl = cube();
    let (status, body) = create(&router, &[("model", &stl), ("set", b"layer_height=9")]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "settings.out_of_range");
    assert_eq!(body["error"]["context"]["key"], "layer_height");

    let (status, body) = create(&router, &[("model", &stl), ("settings", b"nope = 1")]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "settings.parse");

    // A model that cannot be read fails the job, not the request.
    let (status, created) = create(&router, &[("model", b"not a model")]).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let id = created["id"].as_str().unwrap();
    let job = finished(&router, id).await;
    assert_eq!(job["status"], "failed");
    assert_eq!(job["error"]["code"], "model.unrecognized");

    let (status, body) = call(&router, "GET", &format!("/v1/jobs/{id}/gcode")).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "job.no_result");

    let (status, _) = call(&router, "GET", "/v1/jobs/not-an-id").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A plate must be a number, and a plain model has only plate 1.
    let (status, body) = create(&router, &[("model", &stl), ("plate", b"two")]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "request.invalid");
    let (_, created) = create(&router, &[("model", &stl), ("plate", b"2")]).await;
    let job = finished(&router, created["id"].as_str().unwrap()).await;
    assert_eq!(job["error"]["code"], "model.unknown_plate");
    let (_, created) = create(&router, &[("model", &stl), ("plate", b"1")]).await;
    let job = finished(&router, created["id"].as_str().unwrap()).await;
    assert_eq!(job["status"], "done");
}

#[tokio::test]
async fn jobs_wait_for_a_free_worker_and_can_be_cancelled() {
    // No worker at all: every job stays queued.
    let app = App::new(Config {
        workers: 0,
        max_pending: 2,
        ..Config::default()
    });
    let router = app.router();
    let stl = cube();
    let (_, first) = create(&router, &[("model", &stl)]).await;
    let (_, second) = create(&router, &[("model", &stl)]).await;
    let (status, body) = create(&router, &[("model", &stl)]).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "server.busy");

    let id = first["id"].as_str().unwrap();
    let (status, job) = call(&router, "GET", &format!("/v1/jobs/{id}")).await;
    assert_eq!((status, &job["status"]), (StatusCode::OK, &"queued".into()));

    let (status, job) = call(&router, "DELETE", &format!("/v1/jobs/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(job["status"], "cancelled");

    // The cancelled job no longer counts against the limit.
    let (status, _) = create(&router, &[("model", &stl)]).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(second["status"], "queued");
}

#[tokio::test]
async fn cancelling_stops_a_running_slice() {
    let router = App::new(Config {
        workers: 1,
        ..Config::default()
    })
    .router();
    // 2000 layers of a large cube keep the only worker busy for a while.
    let large = slicer_formats::write_stl_binary(&Mesh::cuboid(200.0, 200.0, 200.0));
    let fields: [(&str, &[u8]); 2] = [("model", &large), ("set", b"layer_height=0.05")];

    // How long the slice takes when left alone.
    let started = Instant::now();
    let (_, job) = create(&router, &fields).await;
    let job = finished(&router, job["id"].as_str().unwrap()).await;
    assert_eq!(job["status"], "done");
    let alone = started.elapsed();

    // The same again, cancelled as soon as it runs.
    let (_, job) = create(&router, &fields).await;
    let path = format!("/v1/jobs/{}", job["id"].as_str().unwrap());
    while call(&router, "GET", &path).await.1["status"] != "running" {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let cancelled = Instant::now();
    let (_, job) = call(&router, "DELETE", &path).await;
    assert_eq!(job["status"], "cancelled");

    // The worker is free long before the slice would have ended.
    let (_, small) = create(&router, &[("model", &cube())]).await;
    let small = finished(&router, small["id"].as_str().unwrap()).await;
    assert_eq!(small["status"], "done");
    let waited = cancelled.elapsed();
    // Far less than the whole slice; not half, since a busy machine slows
    // the small job too.
    assert!(waited < alone * 3 / 4, "{waited:?} of {alone:?}");
    println!("alone {alone:?}, after cancelling {waited:?}");
}

#[tokio::test]
async fn finished_jobs_survive_a_restart() {
    let dir = std::env::temp_dir().join(format!("spaghetti-store-{}", uuid::Uuid::new_v4()));
    let config = Config {
        data_dir: Some(dir.clone()),
        ..Config::default()
    };
    let router = App::new(config.clone()).router();
    let (_, done) = create(&router, &[("model", &cube())]).await;
    let done = finished(&router, done["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "done");
    let (_, failed) = create(&router, &[("model", b"not a model")]).await;
    let failed = finished(&router, failed["id"].as_str().unwrap()).await;
    assert_eq!(failed["status"], "failed");
    let gcode_url = done["gcode_url"].as_str().unwrap().to_string();
    let request = |uri: &str| Request::builder().uri(uri).body(Body::empty()).unwrap();
    let (_, gcode) = send(&router, request(&gcode_url)).await;

    // A new server on the same directory knows both jobs.
    let router = App::new(config.clone()).router();
    for job in [&done, &failed] {
        let (status, again) = call(
            &router,
            "GET",
            &format!("/v1/jobs/{}", job["id"].as_str().unwrap()),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(&again, job);
    }
    assert_eq!(
        send(&router, request(&gcode_url)).await,
        (StatusCode::OK, gcode)
    );
    // The pictures of the plate are kept for the project file.
    let (status, package) = send(&router, request(done["gcode_3mf_url"].as_str().unwrap())).await;
    assert_eq!(status, StatusCode::OK);
    assert!(contains(&package, b"Metadata/plate_1.png"));
    assert!(contains(&package, b"Metadata/plate_1_small.png"));

    // Deleting a job deletes its files.
    let path = format!("/v1/jobs/{}", done["id"].as_str().unwrap());
    assert_eq!(
        call(&router, "DELETE", &path).await.0,
        StatusCode::NO_CONTENT
    );
    let router = App::new(config.clone()).router();
    assert_eq!(call(&router, "GET", &path).await.0, StatusCode::NOT_FOUND);

    // Expired records are dropped when the server starts.
    let app = App::new(Config {
        result_ttl: Duration::ZERO,
        ..config
    });
    let path = format!("/v1/jobs/{}", failed["id"].as_str().unwrap());
    assert_eq!(
        call(&app.router(), "GET", &path).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    std::fs::remove_dir(&dir).unwrap();
}

#[tokio::test]
async fn finished_jobs_expire() {
    let app = App::new(Config {
        result_ttl: Duration::ZERO,
        ..Config::default()
    });
    let router = app.router();
    let (_, created) = create(&router, &[("model", &cube())]).await;
    let id = created["id"].as_str().unwrap();
    finished(&router, id).await;
    app.expire();
    let (status, _) = call(&router, "GET", &format!("/v1/jobs/{id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn token_and_upload_limit() {
    let router = App::new(Config {
        token: Some("secret".to_string()),
        max_upload_bytes: 1000,
        ..Config::default()
    })
    .router();

    let (status, body) = call(&router, "GET", "/v1/jobs/x").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "request.unauthorized");
    // The health check stays open for container probes.
    assert_eq!(call(&router, "GET", "/health").await.0, StatusCode::OK);

    let post = |token: &str, model: Vec<u8>| {
        Request::builder()
            .method("POST")
            .uri("/v1/jobs")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={BOUNDARY}"),
            )
            .body(Body::from(multipart(&[("model", &model)])))
            .unwrap()
    };
    assert_eq!(
        send(&router, post("wrong", cube())).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&router, post("secret", cube())).await.0,
        StatusCode::ACCEPTED
    );
    let (status, body) = send(&router, post("secret", vec![0; 5000])).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["error"]["code"], "request.too_large");
}

#[test]
fn configuration_comes_from_the_environment() {
    let config = Config::from_lookup(|name| match name {
        "SPAGHETTI_LISTEN" => Some("127.0.0.1:9000".to_string()),
        "SPAGHETTI_WORKERS" => Some("3".to_string()),
        "SPAGHETTI_RESULT_TTL_SECONDS" => Some("60".to_string()),
        "SPAGHETTI_TOKEN" => Some("abc".to_string()),
        _ => None,
    })
    .unwrap();
    assert_eq!(config.listen.port(), 9000);
    assert_eq!(config.workers, 3);
    assert_eq!(config.result_ttl, Duration::from_secs(60));
    assert_eq!(config.token.as_deref(), Some("abc"));
    assert_eq!(config.max_pending, Config::default().max_pending);

    let error = Config::from_lookup(|name| (name == "SPAGHETTI_WORKERS").then(|| "many".into()));
    assert_eq!(
        error,
        Err("SPAGHETTI_WORKERS has an invalid value 'many'".to_string())
    );
    assert!(Config::from_lookup(|name| (name == "SPAGHETTI_WORKERS").then(|| "0".into())).is_err());
    assert_eq!(Config::from_lookup(|_| None).unwrap(), Config::default());
}

#[tokio::test]
async fn server_stops_when_asked() {
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let config = Config {
        listen: ([127, 0, 0, 1], 0).into(),
        ..Config::default()
    };
    let server = tokio::spawn(slicer_server::serve(config, async {
        let _ = stopped.await;
    }));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!server.is_finished());
    stop.send(()).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), server).await;
    assert!(result.unwrap().unwrap().is_ok());
}

async fn send_to(router: &Router, id: &str, body: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(format!("/v1/jobs/{id}/send"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let (status, body) = send(router, request).await;
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

#[tokio::test]
async fn finished_jobs_are_sent_to_configured_printers_only() {
    // A stand-in for Moonraker on 127.0.0.1; no real printer is contacted.
    let printer = slicer_send::mock::Printer::start(201, r#"{"print_started": true}"#).unwrap();
    let down = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let config = Config::from_lookup(|name| match name {
        "SPAGHETTI_PRINTERS" => Some(format!(
            "shop=moonraker://{}, gone=octoprint://{down}",
            printer.address
        )),
        "SPAGHETTI_PRINTER_KEY_SHOP" => Some("printer-key".to_string()),
        _ => None,
    })
    .unwrap();
    let router = App::new(config).router();
    let (_, created) = create(&router, &[("model", &cube())]).await;
    let id = created["id"].as_str().unwrap();
    let job = finished(&router, id).await;

    let (status, body) = send_to(&router, id, r#"{"printer": "shop", "start": true}"#).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["printer"], "shop");
    assert_eq!(body["file"], format!("{id}.gcode"));
    assert_eq!(body["started"], true);
    let request = printer.request().unwrap();
    assert_eq!(request.path, "/server/files/upload");
    assert_eq!(request.header("x-api-key"), Some("printer-key"));
    assert_eq!(request.field("print").unwrap(), b"true");
    let gcode = Request::builder()
        .uri(job["gcode_url"].as_str().unwrap())
        .body(Body::empty())
        .unwrap();
    assert_eq!(request.field("file").unwrap(), send(&router, gcode).await.1);

    // Callers can only name a configured printer, never an address.
    for body in [
        r#"{"printer": "moonraker://127.0.0.1:1"}"#,
        r#"{"printer": "other"}"#,
    ] {
        let (status, answer) = send_to(&router, id, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(answer["error"]["code"], "send.unknown_printer");
    }
    for body in [
        "",
        "[]",
        r#"{"start": true}"#,
        r#"{"printer": "shop", "start": "yes"}"#,
        r#"{"printer": "shop", "url": "x"}"#,
    ] {
        let (status, answer) = send_to(&router, id, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(answer["error"]["code"], "request.invalid");
    }

    // A printer that cannot be reached is the printer's fault: 502.
    let (status, answer) = send_to(&router, id, r#"{"printer": "gone"}"#).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(answer["error"]["code"], "send.connection");
    // The caller learns the printer's name, not its address.
    let text = answer.to_string();
    assert!(text.contains("printer 'gone'"), "{text}");
    assert!(
        !text.contains("127.0.0.1") && !text.contains("moonraker://"),
        "{text}"
    );

    // Unknown and unfinished jobs have nothing to send.
    let unknown = "00000000-0000-4000-8000-000000000000";
    let (status, _) = send_to(&router, unknown, r#"{"printer": "shop"}"#).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, failed) = create(&router, &[("model", b"not a model")]).await;
    let failed = failed["id"].as_str().unwrap();
    finished(&router, failed).await;
    let (status, answer) = send_to(&router, failed, r#"{"printer": "shop"}"#).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(answer["error"]["code"], "job.no_result");
}

#[test]
fn printers_come_from_the_environment() {
    let config = Config::from_lookup(|name| match name {
        "SPAGHETTI_PRINTERS" => {
            Some("a=moonraker://10.0.0.5:7125,b_2=octoprint+https://host/octo".into())
        }
        "SPAGHETTI_PRINTER_KEY_A" => Some("secret-key".into()),
        _ => None,
    })
    .unwrap();
    let names: Vec<&str> = config
        .printers
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(names, ["a", "b_2"]);
    assert_eq!(
        config.printers[1].1.upload_url(),
        "https://host/octo/api/files/local"
    );
    // The key is never shown.
    assert!(!format!("{config:?}").contains("secret-key"));

    for list in [
        "moonraker://host",
        "a b=moonraker://host",
        "a=moonraker://host,A=octoprint://host",
        "a=http://host",
        "a=moonraker://user:password@host",
    ] {
        let error = Config::from_lookup(|name| (name == "SPAGHETTI_PRINTERS").then(|| list.into()))
            .unwrap_err();
        assert!(error.starts_with("SPAGHETTI_PRINTERS"), "{error}");
        assert!(!error.contains("password"), "{error}");
    }
}
