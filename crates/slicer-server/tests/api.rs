//! Tests of the HTTP API, driven through the router without a socket.
#![allow(clippy::unwrap_used, missing_docs)]

use std::time::Duration;

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

    // Deleting a finished job removes it.
    let (status, _) = call(&router, "DELETE", &format!("/v1/jobs/{id}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = call(&router, "GET", &format!("/v1/jobs/{id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "job.not_found");
}

#[tokio::test]
async fn a_3mf_model_is_accepted() {
    let router = App::new(Config::default()).router();
    let model = slicer_formats::write_3mf(&Mesh::cuboid(10.0, 10.0, 2.0)).unwrap();
    let (_, created) = create(&router, &[("model", &model)]).await;
    let job = finished(&router, created["id"].as_str().unwrap()).await;
    assert_eq!(job["status"], "done", "{job}");
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
