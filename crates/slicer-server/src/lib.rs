//! HTTP API server for the slicer.
//!
//! Jobs are created with a model and settings, sliced in the background
//! with bounded parallelism, and kept in memory until they expire.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use slicer_core::{Diagnose, Diagnostic, Diagnostics};
use slicer_profiles::Settings;
use tokio::sync::Semaphore;
use uuid::Uuid;

/// Server settings, read from environment variables only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Address to listen on (`SPAGHETTI_LISTEN`).
    pub listen: SocketAddr,
    /// Number of jobs sliced at the same time (`SPAGHETTI_WORKERS`).
    pub workers: usize,
    /// Number of unfinished jobs accepted before new ones are refused
    /// (`SPAGHETTI_MAX_PENDING`).
    pub max_pending: usize,
    /// Largest accepted request body in bytes (`SPAGHETTI_MAX_UPLOAD_BYTES`).
    pub max_upload_bytes: usize,
    /// How long finished jobs are kept (`SPAGHETTI_RESULT_TTL_SECONDS`).
    pub result_ttl: Duration,
    /// If set, requests to `/v1/` need this bearer token (`SPAGHETTI_TOKEN`).
    pub token: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([0, 0, 0, 0], 8080)),
            workers: std::thread::available_parallelism().map_or(1, usize::from),
            max_pending: 100,
            max_upload_bytes: 256 << 20,
            result_ttl: Duration::from_secs(3600),
            token: None,
        }
    }
}

impl Config {
    /// Reads the configuration from the process environment.
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Reads the configuration through `lookup`; unset variables keep their
    /// defaults, invalid values are an error naming the variable.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        fn parse<T: std::str::FromStr>(
            lookup: &impl Fn(&str) -> Option<String>,
            name: &str,
            default: T,
        ) -> Result<T, String> {
            match lookup(name) {
                Some(text) => text
                    .trim()
                    .parse()
                    .map_err(|_| format!("{name} has an invalid value '{text}'")),
                None => Ok(default),
            }
        }
        let defaults = Self::default();
        let config = Self {
            listen: parse(&lookup, "SPAGHETTI_LISTEN", defaults.listen)?,
            workers: parse(&lookup, "SPAGHETTI_WORKERS", defaults.workers)?,
            max_pending: parse(&lookup, "SPAGHETTI_MAX_PENDING", defaults.max_pending)?,
            max_upload_bytes: parse(
                &lookup,
                "SPAGHETTI_MAX_UPLOAD_BYTES",
                defaults.max_upload_bytes,
            )?,
            result_ttl: Duration::from_secs(parse(
                &lookup,
                "SPAGHETTI_RESULT_TTL_SECONDS",
                defaults.result_ttl.as_secs(),
            )?),
            token: lookup("SPAGHETTI_TOKEN").filter(|token| !token.is_empty()),
        };
        if config.workers == 0 {
            return Err("SPAGHETTI_WORKERS must be at least 1".to_string());
        }
        Ok(config)
    }
}

/// Writes one structured log line (JSON) to standard output.
pub fn log(level: &str, event: &str, fields: Value) {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64());
    let mut line =
        json!({ "ts": (seconds * 1000.0).round() / 1000.0, "level": level, "event": event });
    if let (Some(line), Value::Object(fields)) = (line.as_object_mut(), fields) {
        line.extend(fields);
    }
    println!("{line}");
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Status {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl Status {
    fn name(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn finished(self) -> bool {
        !matches!(self, Self::Queued | Self::Running)
    }
}

struct Job {
    status: Status,
    finished_at: Option<Instant>,
    stats: Option<Value>,
    warnings: Vec<Diagnostic>,
    error: Option<Diagnostic>,
    gcode: Option<Arc<String>>,
}

impl Job {
    fn describe(&self, id: Uuid) -> Value {
        let mut out = json!({ "id": id.to_string(), "status": self.status.name() });
        if self.status == Status::Done {
            out["stats"] = self.stats.clone().unwrap_or(Value::Null);
            out["warnings"] = json!(self.warnings);
            out["gcode_url"] = json!(format!("/v1/jobs/{id}/gcode"));
        }
        if let Some(error) = &self.error {
            out["error"] = json!(error);
        }
        out
    }
}

/// Shared state of a running server.
#[derive(Clone)]
pub struct App {
    config: Arc<Config>,
    jobs: Arc<Mutex<HashMap<Uuid, Job>>>,
    slots: Arc<Semaphore>,
}

impl App {
    /// Creates the state for a server with the given configuration.
    pub fn new(config: Config) -> Self {
        Self {
            slots: Arc::new(Semaphore::new(config.workers)),
            config: Arc::new(config),
            jobs: Arc::default(),
        }
    }

    fn jobs(&self) -> MutexGuard<'_, HashMap<Uuid, Job>> {
        self.jobs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Removes finished jobs older than the configured lifetime.
    pub fn expire(&self) {
        let ttl = self.config.result_ttl;
        self.jobs()
            .retain(|_, job| job.finished_at.is_none_or(|at| at.elapsed() < ttl));
    }

    /// Builds the HTTP routes.
    pub fn router(&self) -> Router {
        let api = Router::new()
            .route("/jobs", post(create_job))
            .route("/jobs/{id}", get(job_status).delete(cancel_job))
            .route("/jobs/{id}/gcode", get(job_gcode))
            .layer(DefaultBodyLimit::max(self.config.max_upload_bytes))
            .layer(middleware::from_fn_with_state(self.clone(), authorize));
        Router::new()
            .route("/health", get(health))
            .nest("/v1", api)
            .fallback(|| async {
                failure(
                    StatusCode::NOT_FOUND,
                    "request.not_found",
                    "no such endpoint",
                )
            })
            .layer(middleware::from_fn(log_request))
            .with_state(self.clone())
    }
}

/// An error response in the same shape as the command line tool's errors.
fn error_response(status: StatusCode, diagnostic: &Diagnostic) -> Response {
    (status, Json(json!({ "error": diagnostic }))).into_response()
}

fn failure(status: StatusCode, code: &'static str, message: &str) -> Response {
    error_response(status, &Diagnostic::error(code, message))
}

fn not_found(id: &str) -> Response {
    error_response(
        StatusCode::NOT_FOUND,
        &Diagnostic::error("job.not_found", format!("no job with id '{id}'")).with("id", id),
    )
}

async fn log_request(request: Request, next: Next) -> Response {
    let started = Instant::now();
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let response = next.run(request).await;
    log(
        "info",
        "http.request",
        json!({
            "method": method,
            "path": path,
            "status": response.status().as_u16(),
            "ms": (started.elapsed().as_secs_f64() * 1e4).round() / 10.0,
        }),
    );
    response
}

/// Compares two strings without stopping at the first difference.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0, |acc, (x, y)| acc | (x ^ y))
            == 0
}

async fn authorize(State(app): State<App>, request: Request, next: Next) -> Response {
    if let Some(expected) = &app.config.token {
        let given = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "));
        if !given.is_some_and(|given| same(given, expected)) {
            let mut response = failure(
                StatusCode::UNAUTHORIZED,
                "request.unauthorized",
                "a valid bearer token is required",
            );
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            return response;
        }
    }
    next.run(request).await
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }))
}

/// Slices a model; the body of every background job.
fn slice(
    model: &[u8],
    plate: Option<u32>,
    settings: &Settings,
) -> Result<(slicer_gcode::Output, Diagnostics), Diagnostic> {
    let mut warnings = Diagnostics::new();
    let mesh = slicer_formats::parse_model(model, plate, &mut warnings)?;
    let toolpaths = slicer_fdm::generate_reporting(&mesh, settings, &mut warnings)
        .map_err(|e| e.diagnostic())?;
    let output = slicer_gcode::write_gcode(&toolpaths, settings).map_err(|e| e.diagnostic())?;
    Ok((output, warnings))
}

async fn create_job(State(app): State<App>, mut multipart: Multipart) -> Response {
    let invalid = |message: String| {
        error_response(
            StatusCode::BAD_REQUEST,
            &Diagnostic::error("request.invalid", message),
        )
    };
    let mut model = None;
    let mut settings = Settings::default();
    let mut overrides = Vec::new();
    let mut plate = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => return multipart_error(&error),
        };
        let name = field.name().unwrap_or_default().to_string();
        let bytes = match field.bytes().await {
            Ok(bytes) => bytes,
            Err(error) => return multipart_error(&error),
        };
        match name.as_str() {
            "model" => model = Some(bytes),
            "settings" | "set" | "plate" => {
                let Ok(text) = std::str::from_utf8(&bytes) else {
                    return invalid(format!("field '{name}' is not valid UTF-8"));
                };
                if name == "settings" {
                    // Several files are layered in the order they are sent.
                    if let Err(e) = settings.apply_toml(text) {
                        return error_response(StatusCode::BAD_REQUEST, &e.diagnostic());
                    }
                } else if name == "plate" {
                    let Ok(number) = text.trim().parse::<u32>() else {
                        return invalid(format!("field 'plate' expects a number, got '{text}'"));
                    };
                    plate = Some(number);
                } else {
                    overrides.push(text.to_string());
                }
            }
            _ => return invalid(format!("unknown field '{name}'")),
        }
    }
    for item in &overrides {
        let Some((key, value)) = item.split_once('=') else {
            return invalid(format!("field 'set' expects KEY=VALUE, got '{item}'"));
        };
        if let Err(e) = settings.set(key.trim(), value.trim()) {
            return error_response(StatusCode::BAD_REQUEST, &e.diagnostic());
        }
    }
    let Some(model) = model else {
        return invalid("field 'model' is missing".to_string());
    };

    let id = Uuid::new_v4();
    {
        let mut jobs = app.jobs();
        let pending = jobs.values().filter(|job| !job.status.finished()).count();
        if pending >= app.config.max_pending {
            return failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "server.busy",
                "too many unfinished jobs; try again later",
            );
        }
        jobs.insert(
            id,
            Job {
                status: Status::Queued,
                finished_at: None,
                stats: None,
                warnings: Vec::new(),
                error: None,
                gcode: None,
            },
        );
    }
    log(
        "info",
        "job.created",
        json!({ "job": id.to_string(), "bytes": model.len() }),
    );
    tokio::spawn(run_job(app.clone(), id, model, plate, settings));

    let mut response = (
        StatusCode::ACCEPTED,
        Json(json!({ "id": id.to_string(), "status": Status::Queued.name() })),
    )
        .into_response();
    if let Ok(location) = HeaderValue::from_str(&format!("/v1/jobs/{id}")) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    response
}

fn multipart_error(error: &axum::extract::multipart::MultipartError) -> Response {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        failure(
            StatusCode::PAYLOAD_TOO_LARGE,
            "request.too_large",
            "the upload exceeds the size limit",
        )
    } else {
        error_response(
            StatusCode::BAD_REQUEST,
            &Diagnostic::error("request.invalid", error.body_text()),
        )
    }
}

async fn run_job(
    app: App,
    id: Uuid,
    model: axum::body::Bytes,
    plate: Option<u32>,
    settings: Settings,
) {
    let Ok(_slot) = app.slots.clone().acquire_owned().await else {
        return;
    };
    {
        let mut jobs = app.jobs();
        match jobs.get_mut(&id) {
            Some(job) if job.status == Status::Queued => job.status = Status::Running,
            // Cancelled or expired while waiting.
            _ => return,
        }
    }
    let started = Instant::now();
    let outcome = tokio::task::spawn_blocking(move || slice(&model, plate, &settings))
        .await
        .unwrap_or_else(|_| {
            Err(Diagnostic::error(
                "server.internal",
                "slicing stopped unexpectedly",
            ))
        });

    let mut jobs = app.jobs();
    let Some(job) = jobs.get_mut(&id) else { return };
    if job.status != Status::Running {
        // Cancelled while slicing: the result is discarded.
        return;
    }
    job.finished_at = Some(Instant::now());
    match outcome {
        Ok((output, warnings)) => {
            job.status = Status::Done;
            job.stats = Some(json!(output.stats));
            job.warnings = warnings.as_slice().to_vec();
            job.gcode = Some(Arc::new(output.gcode));
        }
        Err(error) => {
            job.status = Status::Failed;
            job.error = Some(error);
        }
    }
    log(
        if job.status == Status::Done {
            "info"
        } else {
            "warn"
        },
        "job.finished",
        json!({
            "job": id.to_string(),
            "status": job.status.name(),
            "ms": started.elapsed().as_millis() as u64,
            "error": job.error.as_ref().map(|e| e.code),
        }),
    );
}

async fn job_status(State(app): State<App>, Path(id): Path<String>) -> Response {
    let jobs = app.jobs();
    match id
        .parse()
        .ok()
        .and_then(|id: Uuid| Some((id, jobs.get(&id)?)))
    {
        Some((id, job)) => Json(job.describe(id)).into_response(),
        None => not_found(&id),
    }
}

async fn job_gcode(State(app): State<App>, Path(id): Path<String>) -> Response {
    let gcode = {
        let jobs = app.jobs();
        let Some(job) = id.parse().ok().and_then(|id: Uuid| jobs.get(&id)) else {
            return not_found(&id);
        };
        match &job.gcode {
            Some(gcode) => gcode.clone(),
            None => {
                return error_response(
                    StatusCode::CONFLICT,
                    &Diagnostic::error(
                        "job.no_result",
                        format!("job is {} and has no G-code", job.status.name()),
                    )
                    .with("status", job.status.name()),
                );
            }
        }
    };
    (
        [(header::CONTENT_TYPE, "text/x.gcode; charset=utf-8")],
        Body::from(gcode.as_ref().clone()),
    )
        .into_response()
}

/// Cancels an unfinished job, or deletes the result of a finished one.
async fn cancel_job(State(app): State<App>, Path(id): Path<String>) -> Response {
    let mut jobs = app.jobs();
    let Some((id, job)) = id
        .parse()
        .ok()
        .and_then(|id: Uuid| Some((id, jobs.get_mut(&id)?)))
    else {
        return not_found(&id);
    };
    if job.status.finished() {
        jobs.remove(&id);
        return StatusCode::NO_CONTENT.into_response();
    }
    job.status = Status::Cancelled;
    job.finished_at = Some(Instant::now());
    log("info", "job.cancelled", json!({ "job": id.to_string() }));
    Json(job.describe(id)).into_response()
}

/// Runs the server until `shutdown` resolves.
pub async fn serve(
    config: Config,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let app = App::new(config);
    let listener = tokio::net::TcpListener::bind(app.config.listen).await?;
    log(
        "info",
        "server.start",
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "listen": listener.local_addr()?.to_string(),
            "workers": app.config.workers,
            "auth": app.config.token.is_some(),
        }),
    );
    let cleaner = app.clone();
    let cleanup = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        loop {
            tick.tick().await;
            cleaner.expire();
        }
    });
    let result = axum::serve(listener, app.router())
        .with_graceful_shutdown(shutdown)
        .await;
    cleanup.abort();
    log("info", "server.stop", json!({}));
    result
}
