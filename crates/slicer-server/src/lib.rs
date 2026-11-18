//! HTTP API server for the slicer.
//!
//! Jobs are created with a model and settings, sliced in the background
//! with bounded parallelism, and kept in memory until they expire.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, delete, get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use slicer_core::{Diagnose, Diagnostic, Diagnostics};
use slicer_fdm::Progress;
use slicer_fdm::load;
use slicer_profiles::Settings;
use slicer_send::Target;
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
    /// If set, finished jobs are kept in this directory and survive a
    /// restart (`SPAGHETTI_DATA_DIR`).
    pub data_dir: Option<PathBuf>,
    /// Printers finished jobs may be sent to, by name (`SPAGHETTI_PRINTERS`,
    /// API keys in `SPAGHETTI_PRINTER_KEY_<NAME>`). Callers can only name
    /// one of these, so the server never contacts a host chosen by a caller.
    pub printers: Vec<(String, Target)>,
    /// If set, the browser app built by `tools/build-web.sh` is served from
    /// this directory at `/app/` (`SPAGHETTI_WEB_DIR`).
    pub web_dir: Option<PathBuf>,
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
            data_dir: None,
            printers: Vec::new(),
            web_dir: None,
        }
    }
}

/// Reads `SPAGHETTI_PRINTERS` (`NAME=ADDRESS,NAME=ADDRESS`) and the API key
/// of each printer from `SPAGHETTI_PRINTER_KEY_<NAME>`.
fn printers(lookup: &impl Fn(&str) -> Option<String>) -> Result<Vec<(String, Target)>, String> {
    let mut printers: Vec<(String, Target)> = Vec::new();
    let list = lookup("SPAGHETTI_PRINTERS").unwrap_or_default();
    for entry in list
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        // The entry is not repeated: it might hold credentials.
        let Some((name, address)) = entry.split_once('=') else {
            return Err(
                "SPAGHETTI_PRINTERS expects NAME=ADDRESS entries separated by commas".into(),
            );
        };
        let name = name.trim();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(format!(
                "SPAGHETTI_PRINTERS: the printer name '{name}' may only contain letters, digits and _"
            ));
        }
        if printers
            .iter()
            .any(|(known, _)| known.eq_ignore_ascii_case(name))
        {
            return Err(format!(
                "SPAGHETTI_PRINTERS: the printer name '{name}' is used twice"
            ));
        }
        let key = lookup(&format!(
            "SPAGHETTI_PRINTER_KEY_{}",
            name.to_ascii_uppercase()
        ));
        let target = Target::parse(address)
            .map_err(|error| format!("SPAGHETTI_PRINTERS: {}", error.message))?
            .with_api_key(key);
        printers.push((name.to_string(), target));
    }
    Ok(printers)
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
            data_dir: lookup("SPAGHETTI_DATA_DIR")
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
            printers: printers(&lookup)?,
            web_dir: lookup("SPAGHETTI_WEB_DIR")
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
        };
        if config.workers == 0 {
            return Err("SPAGHETTI_WORKERS must be at least 1".to_string());
        }
        Ok(config)
    }
}

/// Seconds since 1970.
fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// Writes one structured log line (JSON) to standard output.
pub fn log(level: &str, event: &str, fields: Value) {
    let seconds = unix_now();
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

#[derive(Clone)]
struct Job {
    status: Status,
    finished_at: Option<Instant>,
    stats: Option<Value>,
    warnings: Value,
    error: Option<Value>,
    gcode: Option<Arc<String>>,
    /// What the project file for Bambu Lab printers says about the print.
    info: Option<slicer_formats::PrintInfo>,
    /// Set when the job is cancelled, so slicing gives up early.
    stop: Arc<AtomicBool>,
    /// How far slicing has got.
    progress: Arc<Progress>,
}

impl Job {
    fn describe(&self, id: Uuid) -> Value {
        let progress = if self.status == Status::Done {
            1.0
        } else {
            self.progress.fraction()
        };
        let mut out = json!({
            "id": id.to_string(),
            "status": self.status.name(),
            "progress": progress,
        });
        if self.status == Status::Running {
            out["step"] = json!(self.progress.step().label());
        }
        if self.status == Status::Done {
            out["stats"] = self.stats.clone().unwrap_or(Value::Null);
            out["warnings"] = self.warnings.clone();
            out["gcode_url"] = json!(format!("/v1/jobs/{id}/gcode"));
            out["gcode_3mf_url"] = json!(format!("/v1/jobs/{id}/gcode.3mf"));
        }
        if let Some(error) = &self.error {
            out["error"] = error.clone();
        }
        out
    }

    /// Writes a finished job to `dir`: the G-code, then a record of
    /// everything else. The record is renamed into place last, so a record
    /// on disk always has its G-code.
    fn save(&self, dir: &std::path::Path, id: Uuid) -> std::io::Result<()> {
        if let Some(gcode) = &self.gcode {
            std::fs::write(dir.join(format!("{id}.gcode")), gcode.as_bytes())?;
        }
        if let Some(info) = &self.info {
            for (extension, png) in [
                ("png", &info.thumbnail),
                ("small.png", &info.thumbnail_small),
            ] {
                if !png.is_empty() {
                    std::fs::write(dir.join(format!("{id}.{extension}")), png)?;
                }
            }
        }
        let age = self
            .finished_at
            .map_or(0.0, |at| at.elapsed().as_secs_f64());
        let info = self.info.as_ref().map(|info| {
            json!({
                "seconds": info.seconds,
                "weight_g": info.weight_g,
                "filament_m": info.filament_m,
                "layers": info.layers,
                "nozzle_diameter": info.nozzle_diameter,
            })
        });
        let record = json!({
            "status": self.status.name(),
            "finished": unix_now() - age,
            "stats": self.stats,
            "warnings": self.warnings,
            "error": self.error,
            "info": info,
        });
        let temporary = dir.join(format!("{id}.json.tmp"));
        std::fs::write(&temporary, record.to_string())?;
        std::fs::rename(temporary, dir.join(format!("{id}.json")))
    }

    /// Reads a job written by [`Job::save`]. `None` for a record that cannot
    /// be read, and for one older than `ttl`.
    fn load(dir: &std::path::Path, id: Uuid, ttl: Duration) -> Option<Self> {
        let text = std::fs::read_to_string(dir.join(format!("{id}.json"))).ok()?;
        let record: Value = serde_json::from_str(&text).ok()?;
        let status = match record["status"].as_str()? {
            "done" => Status::Done,
            "failed" => Status::Failed,
            _ => return None,
        };
        let age = Duration::try_from_secs_f64((unix_now() - record["finished"].as_f64()?).max(0.0))
            .ok()?;
        if age >= ttl {
            return None;
        }
        let (gcode, info) = if status == Status::Done {
            let gcode = std::fs::read_to_string(dir.join(format!("{id}.gcode"))).ok()?;
            let info = &record["info"];
            let info = slicer_formats::PrintInfo {
                seconds: info["seconds"].as_f64()?,
                weight_g: info["weight_g"].as_f64()?,
                filament_m: info["filament_m"].as_f64()?,
                layers: usize::try_from(info["layers"].as_u64()?).ok()?,
                nozzle_diameter: info["nozzle_diameter"].as_f64()?,
                // Pictures are kept if they can be read; jobs of older
                // versions have none.
                thumbnail: std::fs::read(dir.join(format!("{id}.png"))).unwrap_or_default(),
                thumbnail_small: std::fs::read(dir.join(format!("{id}.small.png")))
                    .unwrap_or_default(),
            };
            (Some(Arc::new(gcode)), Some(info))
        } else {
            (None, None)
        };
        Some(Job {
            status,
            // A machine that has been running for less time than the job is
            // old cannot go back that far; the job then counts as new.
            finished_at: Some(Instant::now().checked_sub(age).unwrap_or_else(Instant::now)),
            stats: Some(record["stats"].clone()).filter(|stats| !stats.is_null()),
            warnings: record["warnings"].clone(),
            error: Some(record["error"].clone()).filter(|error| !error.is_null()),
            gcode,
            info,
            stop: Arc::default(),
            progress: Arc::default(),
        })
    }
}

/// Jobs kept in `dir` by an earlier run. Records that are unreadable or
/// expired are deleted.
fn load_jobs(dir: &std::path::Path, ttl: Duration) -> HashMap<Uuid, Job> {
    let mut jobs = HashMap::new();
    if let Err(error) = std::fs::create_dir_all(dir) {
        log(
            "error",
            "store.unavailable",
            json!({ "dir": dir.display().to_string(), "message": error.to_string() }),
        );
        return jobs;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return jobs;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let Some(id) = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.parse::<Uuid>().ok())
        else {
            continue;
        };
        match Job::load(dir, id, ttl) {
            Some(job) => {
                jobs.insert(id, job);
            }
            None => forget(dir, id),
        }
    }
    log(
        "info",
        "store.loaded",
        json!({ "dir": dir.display().to_string(), "jobs": jobs.len() }),
    );
    jobs
}

/// Deletes what [`Job::save`] wrote; missing files are no error.
fn forget(dir: &std::path::Path, id: Uuid) {
    for extension in ["json", "gcode", "json.tmp", "png", "small.png"] {
        let _ = std::fs::remove_file(dir.join(format!("{id}.{extension}")));
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
    /// With a data directory, the jobs kept there are loaded.
    pub fn new(config: Config) -> Self {
        let jobs = match &config.data_dir {
            Some(dir) => load_jobs(dir, config.result_ttl),
            None => HashMap::new(),
        };
        Self {
            slots: Arc::new(Semaphore::new(config.workers)),
            config: Arc::new(config),
            jobs: Arc::new(Mutex::new(jobs)),
        }
    }

    fn jobs(&self) -> MutexGuard<'_, HashMap<Uuid, Job>> {
        self.jobs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Removes finished jobs older than the configured lifetime.
    pub fn expire(&self) {
        let ttl = self.config.result_ttl;
        let mut gone = Vec::new();
        self.jobs().retain(|id, job| {
            let keep = job.finished_at.is_none_or(|at| at.elapsed() < ttl);
            if !keep {
                gone.push(*id);
            }
            keep
        });
        if let Some(dir) = &self.config.data_dir {
            for id in gone {
                forget(dir, id);
            }
        }
    }

    /// Builds the HTTP routes from [`operations`].
    pub fn router(&self) -> Router {
        let mut public = Router::new();
        let mut api = Router::new();
        for (_, path, handler) in OPERATIONS {
            match path.strip_prefix("/v1") {
                Some(path) => api = api.route(path, handler()),
                None => public = public.route(path, handler()),
            }
        }
        let api = api
            .layer(DefaultBodyLimit::max(self.config.max_upload_bytes))
            .layer(middleware::from_fn_with_state(self.clone(), authorize));
        // The browser app: static files, outside the API and its token.
        let web = Router::new()
            .route(
                "/app",
                get(|| async { axum::response::Redirect::permanent("/app/") }),
            )
            .route(
                "/app/",
                get(|State(app): State<App>| async move { web_file(&app, "").await }),
            )
            .route(
                "/app/{*path}",
                get(
                    |State(app): State<App>, Path(path): Path<String>| async move {
                        web_file(&app, &path).await
                    },
                ),
            );
        public
            .merge(web)
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

/// Builds the handler of one operation.
type Handler = fn() -> MethodRouter<App>;

/// Every operation the server answers, as (method, path, handler). The
/// router is built from this list alone; paths under `/v1` need the token.
// ponytail: the method is not checked against the handler here; the API test
// calls each operation with its method and fails on "method not allowed". A
// route added in `App::router` outside this list is not noticed by any test.
const OPERATIONS: [(&str, &str, Handler); 8] = [
    ("get", "/health", || get(health)),
    ("get", "/openapi.json", || get(|| async { Json(openapi()) })),
    ("post", "/v1/jobs", || post(create_job)),
    ("get", "/v1/jobs/{id}", || get(job_status)),
    ("delete", "/v1/jobs/{id}", || delete(cancel_job)),
    ("get", "/v1/jobs/{id}/gcode", || get(job_gcode)),
    ("get", "/v1/jobs/{id}/gcode.3mf", || get(job_gcode_3mf)),
    ("post", "/v1/jobs/{id}/send", || post(send_job)),
];

/// Method (lower case, as in OpenAPI) and path of every operation the router
/// answers. A test checks that [`openapi`] describes exactly these.
pub fn operations() -> Vec<(&'static str, &'static str)> {
    OPERATIONS
        .iter()
        .map(|(method, path, _)| (*method, *path))
        .collect()
}

/// Description of the API in OpenAPI 3.1 form, served at `/openapi.json`.
///
/// It is written by hand next to the router. A test checks that it names
/// exactly the [`operations`] and calls each of them, so a path that is
/// added, renamed or removed in one place but not the other is noticed.
pub fn openapi() -> Value {
    let error = json!({
        "description": "Error",
        "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } }
    });
    let job = json!({
        "description": "State of the job",
        "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Job" } } }
    });
    let id = json!([{ "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }]);
    let object_settings = format!(
        "Settings in TOML form for model N only (object_settings_1 is the first model; every object of a project split by project_settings), layered over the plate's settings and, with project_settings, the values the project stores for the object; may only set {} (otherwise 400 settings.not_per_object); may be repeated, later ones override earlier ones",
        slicer_fdm::OBJECT_SETTINGS.join(", ")
    );
    let range_settings = format!(
        "Settings in TOML form for the layers whose middle lies from Z1 up to (not including) Z2 millimetres above the bed: range_settings_<z1>-<z2> for every model, range_settings_<n>_<z1>-<z2> for model N only (e.g. range_settings_10-20, range_settings_2_0.5-3.5), layered over the settings of the object; may only set {} (otherwise 400 settings.not_per_range); may be repeated, later ones override earlier ones for the same heights",
        slicer_fdm::RANGE_SETTINGS.join(", ")
    );
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Spaghetti Slicer",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Slices 3D models into G-code. Jobs are kept in memory and expire."
        },
        "security": [{ "token": [] }],
        "paths": {
            "/health": { "get": {
                "summary": "Liveness check",
                "security": [],
                "responses": { "200": { "description": "The server is running" } }
            } },
            "/openapi.json": { "get": {
                "summary": "This description",
                "security": [],
                "responses": { "200": { "description": "OpenAPI document" } }
            } },
            "/v1/jobs": { "post": {
                "summary": "Create a slicing job",
                "requestBody": { "required": true, "content": { "multipart/form-data": { "schema": {
                    "type": "object",
                    "required": ["model"],
                    "properties": {
                        "model": { "type": "string", "format": "binary", "description": "STL or 3MF file, one object; may be repeated: several objects are arranged side by side and numbered from 1 in the order sent" },
                        "settings": { "type": "string", "description": "Settings in TOML form; may be repeated, later ones override earlier ones" },
                        "set": { "type": "string", "description": "One override KEY=VALUE; may be repeated" },
                        "plate": { "type": "integer", "description": "Build plate of a project file with several plates" },
                        "project_settings": { "type": "boolean", "default": false, "description": "Use the print settings stored in project files of Orca Slicer or Bambu Studio (3MF): the project's settings below the settings and set fields, the settings of single objects over the plate's and below object_settings_<n>, the settings of height ranges of objects below range_settings_...; a project whose objects have settings or height ranges of their own is sliced as those objects. Settings without a counterpart are reported as warnings (project.settings_skipped, project.object_settings_skipped, project.part_settings_skipped, project.range_settings_skipped, project.range_invalid, project.no_settings)" }
                    },
                    "patternProperties": {
                        "^object_settings_[1-9][0-9]*$": { "type": "string", "description": object_settings },
                        "^range_settings_([1-9][0-9]*_)?[0-9.]+-[0-9.]+$": { "type": "string", "description": range_settings }
                    }
                } } } },
                "responses": {
                    "202": job,
                    "400": error, "401": error, "413": error, "503": error
                }
            } },
            "/v1/jobs/{id}": {
                "get": {
                    "summary": "State, statistics and warnings of a job",
                    "parameters": id,
                    "responses": { "200": job, "401": error, "404": error }
                },
                "delete": {
                    "summary": "Cancel an unfinished job or delete a finished one",
                    "parameters": id,
                    "responses": {
                        "200": job,
                        "204": { "description": "The finished job was deleted" },
                        "401": error, "404": error
                    }
                }
            },
            "/v1/jobs/{id}/gcode": { "get": {
                "summary": "G-code of a finished job",
                "parameters": id,
                "responses": {
                    "200": { "description": "G-code", "content": { "text/plain": { "schema": { "type": "string" } } } },
                    "401": error, "404": error, "409": error
                }
            } },
            "/v1/jobs/{id}/gcode.3mf": { "get": {
                "summary": "G-code of a finished job as a project file for Bambu Lab printers",
                "parameters": id,
                "responses": {
                    "200": { "description": "Project file (.gcode.3mf)", "content": { "model/3mf": { "schema": { "type": "string", "format": "binary" } } } },
                    "401": error, "404": error, "409": error
                }
            } },
            "/v1/jobs/{id}/send": { "post": {
                "summary": "Upload the G-code of a finished job to a printer configured on the server (Moonraker or OctoPrint)",
                "parameters": id,
                "requestBody": { "required": true, "content": { "application/json": { "schema": {
                    "type": "object",
                    "required": ["printer"],
                    "additionalProperties": false,
                    "properties": {
                        "printer": { "type": "string", "description": "Name of a printer in SPAGHETTI_PRINTERS" },
                        "start": { "type": "boolean", "default": false, "description": "Start printing once uploaded" }
                    }
                } } } },
                "responses": {
                    "200": { "description": "Uploaded", "content": { "application/json": { "schema": {
                        "type": "object",
                        "required": ["printer", "file", "started", "warnings"],
                        "properties": {
                            "printer": { "type": "string" },
                            "file": { "type": "string", "description": "File name on the printer" },
                            "started": { "type": "boolean", "description": "Whether the printer reports that the print started" },
                            "warnings": { "type": "array", "items": { "$ref": "#/components/schemas/Diagnostic" } }
                        }
                    } } } },
                    "400": error, "401": error, "404": error, "409": error,
                    "502": { "description": "The printer could not be reached or refused the file (send.connection, send.unauthorized, send.too_large, send.rejected)", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } } }
                }
            } }
        },
        "components": {
            "securitySchemes": { "token": {
                "type": "http", "scheme": "bearer",
                "description": "Only required if the server was started with SPAGHETTI_TOKEN"
            } },
            "schemas": {
                "Diagnostic": {
                    "type": "object",
                    "required": ["severity", "code", "message"],
                    "properties": {
                        "severity": { "type": "string", "enum": ["warning", "error"] },
                        "code": { "type": "string", "description": "Stable identifier, e.g. model.truncated" },
                        "message": { "type": "string" },
                        "context": { "type": "object", "additionalProperties": { "type": "string" } }
                    }
                },
                "Error": {
                    "type": "object",
                    "required": ["error"],
                    "properties": { "error": { "$ref": "#/components/schemas/Diagnostic" } }
                },
                "Job": {
                    "type": "object",
                    "required": ["id", "status"],
                    "properties": {
                        "id": { "type": "string", "format": "uuid" },
                        "status": { "type": "string", "enum": ["queued", "running", "done", "failed", "cancelled"] },
                        "progress": { "type": "number", "minimum": 0, "maximum": 1, "description": "Share of the slicing done; never decreases, 1 when done" },
                        "step": { "type": "string", "description": "What slicing is doing, as text for people; only while running" },
                        "stats": { "type": "object", "properties": {
                            "layers": { "type": "integer" },
                            "filament_length_mm": { "type": "number" },
                            "filament_volume_cm3": { "type": "number" },
                            "filament_weight_g": { "type": "number" },
                            "print_time_s": { "type": "number" }
                        } },
                        "warnings": { "type": "array", "items": { "$ref": "#/components/schemas/Diagnostic" } },
                        "error": { "$ref": "#/components/schemas/Diagnostic" },
                        "gcode_url": { "type": "string" },
                        "gcode_3mf_url": { "type": "string" }
                    }
                }
            }
        }
    })
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

/// A file of the browser app below `/app/`; the directory itself is
/// `index.html`.
async fn web_file(app: &App, path: &str) -> Response {
    let Some(dir) = &app.config.web_dir else {
        return failure(
            StatusCode::NOT_FOUND,
            "web.not_installed",
            "the browser app is not installed on this server; build it with tools/build-web.sh and set SPAGHETTI_WEB_DIR",
        );
    };
    let path = std::path::Path::new(if path.is_empty() { "index.html" } else { path });
    // Only plain names below the directory: no `..`, no absolute paths.
    let plain = path
        .components()
        .all(|part| matches!(part, std::path::Component::Normal(_)));
    let file = dir.join(path);
    let bytes = if plain {
        tokio::task::spawn_blocking(move || std::fs::read(file))
            .await
            .ok()
            .and_then(Result::ok)
    } else {
        None
    };
    let Some(bytes) = bytes else {
        return failure(
            StatusCode::NOT_FOUND,
            "request.not_found",
            "no such file in the browser app",
        );
    };
    let content_type = match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript",
        // Browsers compile WebAssembly while it streams only with this type.
        Some("wasm") => "application/wasm",
        Some("css") => "text/css",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    };
    ([(header::CONTENT_TYPE, content_type)], bytes).into_response()
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }))
}

/// The settings fields of a job for the whole plate, in the order they
/// are layered.
#[derive(Debug, Default)]
struct PlateSettings {
    /// The `settings` fields (TOML), in the order they were sent.
    texts: Vec<String>,
    /// The `set` fields as key and value, in the order they were sent.
    overrides: Vec<(String, String)>,
}

impl PlateSettings {
    /// `base` with the `settings` fields and then the `set` fields layered
    /// over it.
    fn apply(&self, mut base: Settings) -> Result<Settings, Diagnostic> {
        for text in &self.texts {
            base.apply_toml(text).map_err(|e| e.diagnostic())?;
        }
        for (key, value) in &self.overrides {
            base.set(key, value).map_err(|e| e.diagnostic())?;
        }
        Ok(base)
    }
}

/// Everything a job slices.
#[derive(Debug)]
struct JobInput {
    /// The `model` fields, in the order they were sent.
    models: Vec<axum::body::Bytes>,
    /// The settings of the plate, over the defaults or, with
    /// `project_settings`, over the settings the project files store.
    settings: PlateSettings,
    /// The `object_settings_<n>` fields of every model, in the order they
    /// were sent, followed by the `range_settings_...` fields.
    objects: Vec<Vec<load::ObjectLayer>>,
    /// Whether to use the stored settings of project files, and the plate.
    options: load::Options,
}

/// Slices a job's models; the body of every background job. One model is
/// centred on the bed, several are arranged side by side; with
/// `project_settings` a project file whose objects have settings of their
/// own is split into them, as the command line tool does. Returns the
/// G-code, the warnings and what the `.gcode.3mf` container says about the
/// print, with the pictures of the plate. The G-code holds the pictures
/// that the setting `thumbnails` asks for.
fn slice(
    input: &JobInput,
    stop: &AtomicBool,
    progress: &Progress,
) -> Result<(slicer_gcode::Output, Diagnostics, slicer_formats::PrintInfo), Diagnostic> {
    let mut warnings = Diagnostics::new();
    // Messages name the model by its number when there are several.
    let names: Vec<Option<load::Name>> = (1..=input.models.len())
        .map(|number| {
            (input.models.len() > 1)
                .then(|| load::Name::new(format!("model {number}"), "model", number))
        })
        .collect();
    let models: Vec<load::Model> = input
        .models
        .iter()
        .zip(&names)
        .map(|(bytes, name)| load::Model {
            bytes,
            name: name.as_ref(),
        })
        .collect();
    let plate = load::read_plate(
        &models,
        input.options,
        |base| input.settings.apply(base),
        &input.objects,
        &mut warnings,
    )?;
    let settings = &plate.settings;
    let own = slicer_fdm::OwnSettings {
        objects: &plate.objects.settings,
        ranges: &plate.objects.ranges,
    };
    let toolpaths = slicer_fdm::generate_plate_with_own_settings(
        &plate.objects.meshes,
        own,
        plate.placement,
        settings,
        &mut warnings,
        stop,
        progress,
    )
    .map_err(|e| e.diagnostic())?;
    let mut output = slicer_gcode::write_gcode_with_progress(&toolpaths, settings, progress)
        .map_err(|e| e.diagnostic())?;
    let thumbnail_error = |e: std::io::Error| {
        Diagnostic::error(
            "thumbnail.encode",
            format!("the picture of the plate cannot be made: {e}"),
        )
    };
    let meshes = &plate.objects.meshes;
    slicer_gcode::thumbnails::add_thumbnails(&mut output.gcode, meshes, plate.placement, settings)
        .map_err(thumbnail_error)?;
    let [large, small] = [
        slicer_formats::PLATE_THUMBNAIL_SIZE,
        slicer_formats::PLATE_THUMBNAIL_SMALL_SIZE,
    ];
    let mut pictures = slicer_gcode::thumbnails::render_thumbnails(
        meshes,
        plate.placement,
        &[[large, large], [small, small]],
    )
    .map_err(thumbnail_error)?
    .into_iter()
    .map(|picture| picture.png);
    let info = slicer_formats::PrintInfo {
        seconds: output.stats.print_time_s,
        weight_g: output.stats.filament_weight_g,
        filament_m: output.stats.filament_length_mm / 1000.0,
        layers: output.stats.layers,
        nozzle_diameter: settings.nozzle_diameter,
        thumbnail: pictures.next().unwrap_or_default(),
        thumbnail_small: pictures.next().unwrap_or_default(),
    };
    Ok((output, warnings, info))
}

async fn create_job(State(app): State<App>, mut multipart: Multipart) -> Response {
    let invalid = |message: String| {
        error_response(
            StatusCode::BAD_REQUEST,
            &Diagnostic::error("request.invalid", message),
        )
    };
    let mut models = Vec::new();
    // The plate's settings over the defaults, to check them at once; with
    // project_settings the job layers them again over the project's.
    let mut settings = Settings::default();
    let mut plate_settings = PlateSettings::default();
    let mut overrides = Vec::new();
    let mut object_texts = Vec::new();
    let mut range_texts = Vec::new();
    let mut options = load::Options::default();
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
        let object = name.strip_prefix("object_settings_");
        let range = name.strip_prefix("range_settings_").map(str::to_string);
        match name.as_str() {
            "model" => models.push(bytes),
            "settings" | "set" | "plate" | "project_settings" => {
                let Ok(text) = std::str::from_utf8(&bytes) else {
                    return invalid(format!("field '{name}' is not valid UTF-8"));
                };
                if name == "settings" {
                    // Several files are layered in the order they are sent.
                    if let Err(e) = settings.apply_toml(text) {
                        return error_response(StatusCode::BAD_REQUEST, &e.diagnostic());
                    }
                    plate_settings.texts.push(text.to_string());
                } else if name == "plate" {
                    let Ok(number) = text.trim().parse::<u32>() else {
                        return invalid(format!("field 'plate' expects a number, got '{text}'"));
                    };
                    options.plate = Some(number);
                } else if name == "project_settings" {
                    options.project_settings = match text.trim() {
                        "true" => true,
                        "false" => false,
                        _ => {
                            return invalid(format!(
                                "field 'project_settings' expects true or false, got '{text}'"
                            ));
                        }
                    };
                } else {
                    overrides.push(text.to_string());
                }
            }
            _ if object.is_some() => {
                let Ok(text) = String::from_utf8(bytes.to_vec()) else {
                    return invalid(format!("field '{name}' is not valid UTF-8"));
                };
                let number = object.and_then(|n| n.parse::<usize>().ok());
                object_texts.push((name, number, text));
            }
            _ if range.is_some() => {
                let Ok(text) = String::from_utf8(bytes.to_vec()) else {
                    return invalid(format!("field '{name}' is not valid UTF-8"));
                };
                range_texts.push((name, range.unwrap_or_default(), text));
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
        plate_settings
            .overrides
            .push((key.trim().to_string(), value.trim().to_string()));
    }
    if models.is_empty() {
        return invalid("field 'model' is missing".to_string());
    }
    // Settings of single objects start from those of the plate, so they are
    // checked once the plate's are complete; several fields for one object
    // are layered in the order they are sent. The job layers them again
    // over the settings of every object of their model.
    let mut own: Vec<Option<Settings>> = vec![None; models.len()];
    let mut objects: Vec<Vec<load::ObjectLayer>> = vec![Vec::new(); models.len()];
    for (name, number, text) in object_texts {
        let index = number.and_then(|n| n.checked_sub(1));
        let Some((object, layers)) =
            index.and_then(|i| Some((own.get_mut(i)?, objects.get_mut(i)?)))
        else {
            return invalid(format!(
                "field '{name}' names no object; object_settings_1 to object_settings_{} name the models in the order they are sent",
                models.len()
            ));
        };
        let object = object.get_or_insert_with(|| settings.clone());
        let name = load::Name::new(name.clone(), "field", name);
        if let Err(e) = slicer_fdm::apply_object_toml(object, &text) {
            return error_response(StatusCode::BAD_REQUEST, &name.apply(e));
        }
        layers.push(load::ObjectLayer {
            text,
            name,
            range: None,
        });
    }
    // Height ranges of every model (range_settings_<z1>-<z2>) or of one
    // (range_settings_<n>_<z1>-<z2>), checked over the plate's settings.
    if range_texts.len() > load::MAX_RANGES {
        return invalid(format!(
            "{} range_settings fields; at most {} are allowed",
            range_texts.len(),
            load::MAX_RANGES
        ));
    }
    for (name, suffix, text) in range_texts {
        let (model, heights) = match suffix.split_once('_') {
            Some((number, heights)) => (Some(number), heights),
            None => (None, suffix.as_str()),
        };
        let model = match model.map(|n| n.parse::<usize>().ok().and_then(|n| n.checked_sub(1))) {
            Some(Some(index)) if index < models.len() => Some(index),
            None => None,
            Some(_) => {
                return invalid(format!(
                    "field '{name}' names no object; range_settings_1_<z1>-<z2> to range_settings_{}_<z1>-<z2> name the models in the order they are sent",
                    models.len()
                ));
            }
        };
        let Some((from, to)) = load::parse_height_range(heights) else {
            return invalid(format!(
                "field '{name}' expects heights <z1>-<z2> in millimetres with 0 <= z1 < z2"
            ));
        };
        let name = load::Name::new(name.clone(), "field", name);
        let mut range = slicer_fdm::HeightRange::new(from, to);
        if let Err(e) = slicer_fdm::apply_range_toml(&mut range, &settings, &text) {
            return error_response(StatusCode::BAD_REQUEST, &name.apply(e));
        }
        for (index, layers) in objects.iter_mut().enumerate() {
            if model.is_none_or(|model| model == index) {
                layers.push(load::ObjectLayer {
                    text: text.clone(),
                    name: name.clone(),
                    range: Some((from, to)),
                });
            }
        }
    }

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
                warnings: json!([]),
                error: None,
                gcode: None,
                info: None,
                stop: Arc::new(AtomicBool::new(false)),
                progress: Arc::default(),
            },
        );
    }
    log(
        "info",
        "job.created",
        json!({
            "job": id.to_string(),
            "bytes": models.iter().map(|model| model.len()).sum::<usize>(),
        }),
    );
    let input = JobInput {
        models,
        settings: plate_settings,
        objects,
        options,
    };
    tokio::spawn(run_job(app.clone(), id, input));

    let mut response = (
        StatusCode::ACCEPTED,
        Json(json!({ "id": id.to_string(), "status": Status::Queued.name(), "progress": 0.0 })),
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

async fn run_job(app: App, id: Uuid, input: JobInput) {
    let Ok(_slot) = app.slots.clone().acquire_owned().await else {
        return;
    };
    let (stop, progress) = {
        let mut jobs = app.jobs();
        match jobs.get_mut(&id) {
            Some(job) if job.status == Status::Queued => {
                job.status = Status::Running;
                (job.stop.clone(), job.progress.clone())
            }
            // Cancelled or expired while waiting.
            _ => return,
        }
    };
    let started = Instant::now();
    let outcome = tokio::task::spawn_blocking(move || slice(&input, &stop, &progress))
        .await
        .unwrap_or_else(|_| {
            Err(Diagnostic::error(
                "server.internal",
                "slicing stopped unexpectedly",
            ))
        });

    let mut job = {
        let jobs = app.jobs();
        match jobs.get(&id) {
            Some(job) if job.status == Status::Running => job.clone(),
            // Cancelled while slicing: the result is discarded.
            _ => return,
        }
    };
    job.finished_at = Some(Instant::now());
    match outcome {
        Ok((output, warnings, info)) => {
            job.status = Status::Done;
            job.stats = Some(json!(output.stats));
            job.info = Some(info);
            job.warnings = json!(warnings.as_slice());
            job.gcode = Some(Arc::new(output.gcode));
        }
        Err(error) => {
            job.status = Status::Failed;
            job.error = Some(json!(error));
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
            "error": job.error.as_ref().map(|e| e["code"].clone()),
        }),
    );
    // Written before the result is shown, so a client that sees the job
    // finished can rely on it surviving a restart.
    let dir = app.config.data_dir.clone();
    if let Some(dir) = dir.clone() {
        let record = job.clone();
        let saved = tokio::task::spawn_blocking(move || record.save(&dir, id)).await;
        if let Ok(Err(error)) = saved {
            log(
                "error",
                "store.write",
                json!({ "job": id.to_string(), "message": error.to_string() }),
            );
        }
    }
    let mut jobs = app.jobs();
    match jobs.get_mut(&id) {
        Some(current) if current.status == Status::Running => *current = job,
        // Cancelled while the result was written.
        _ => {
            if let Some(dir) = dir {
                forget(&dir, id);
            }
        }
    }
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

/// The G-code of a finished job and what is known about the print, or the
/// answer for a job that is unknown or has no result.
fn result_of(
    app: &App,
    id: &str,
) -> Result<(Arc<String>, slicer_formats::PrintInfo), Box<Response>> {
    let jobs = app.jobs();
    let Some(job) = id.parse().ok().and_then(|id: Uuid| jobs.get(&id)) else {
        return Err(Box::new(not_found(id)));
    };
    match (&job.gcode, &job.info) {
        (Some(gcode), Some(info)) => Ok((gcode.clone(), info.clone())),
        _ => Err(Box::new(error_response(
            StatusCode::CONFLICT,
            &Diagnostic::error(
                "job.no_result",
                format!("job is {} and has no G-code", job.status.name()),
            )
            .with("status", job.status.name()),
        ))),
    }
}

async fn job_gcode(State(app): State<App>, Path(id): Path<String>) -> Response {
    match result_of(&app, &id) {
        Ok((gcode, _)) => (
            [(header::CONTENT_TYPE, "text/x.gcode; charset=utf-8")],
            Body::from(gcode.as_ref().clone()),
        )
            .into_response(),
        Err(response) => *response,
    }
}

/// The same G-code packed into the project file of Bambu Lab printers.
async fn job_gcode_3mf(State(app): State<App>, Path(id): Path<String>) -> Response {
    let (gcode, info) = match result_of(&app, &id) {
        Ok(result) => result,
        Err(response) => return *response,
    };
    match slicer_formats::write_gcode_3mf(&gcode, &info) {
        Ok(package) => (
            [
                (header::CONTENT_TYPE, "model/3mf"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"plate.gcode.3mf\"",
                ),
            ],
            Body::from(package),
        )
            .into_response(),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.diagnostic()),
    }
}

/// Uploads the G-code of a finished job to a printer from the server's list.
/// The body is `{"printer": NAME, "start": BOOL}`.
async fn send_job(
    State(app): State<App>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let invalid = |message: String| {
        error_response(
            StatusCode::BAD_REQUEST,
            &Diagnostic::error("request.invalid", message),
        )
    };
    let Ok(Value::Object(request)) = serde_json::from_slice::<Value>(&body) else {
        return invalid(
            r#"expected a JSON object such as {"printer": "name", "start": false}"#.into(),
        );
    };
    if let Some(key) = request
        .keys()
        .find(|key| *key != "printer" && *key != "start")
    {
        return invalid(format!("unknown field '{key}'"));
    }
    let Some(name) = request.get("printer").and_then(Value::as_str) else {
        return invalid("field 'printer' is missing or not a string".into());
    };
    let start = match request.get("start") {
        None => false,
        Some(Value::Bool(start)) => *start,
        Some(_) => return invalid("field 'start' must be true or false".into()),
    };
    let Some((_, target)) = app.config.printers.iter().find(|(known, _)| known == name) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            &Diagnostic::error(
                "send.unknown_printer",
                format!("no printer named '{name}' is configured on the server"),
            )
            .with("printer", name),
        );
    };
    let (gcode, _) = match result_of(&app, &id) {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let target = target.clone();
    // Callers know the printer by name only; its address stays on the server.
    let address = target.to_string();
    let file = format!("{id}.gcode");
    let started = Instant::now();
    let outcome = tokio::task::spawn_blocking(move || {
        let mut warnings = Diagnostics::new();
        slicer_send::upload(&target, &file, gcode.as_bytes(), start, &mut warnings)
            .map(|sent| (sent, warnings))
    })
    .await
    .unwrap_or_else(|_| {
        Err(Diagnostic::error(
            "server.internal",
            "sending stopped unexpectedly",
        ))
    });
    let ms = started.elapsed().as_millis() as u64;
    match outcome {
        Ok((sent, warnings)) => {
            log(
                "info",
                "job.sent",
                json!({ "job": id, "printer": name, "started": sent.started, "ms": ms }),
            );
            Json(json!({
                "printer": name,
                "file": sent.file,
                "started": sent.started,
                "warnings": warnings.as_slice(),
            }))
            .into_response()
        }
        Err(error) => {
            log(
                "warn",
                "job.send_failed",
                json!({ "job": id, "printer": name, "error": error.code, "ms": ms }),
            );
            let message = error
                .message
                .replace(&address, &format!("printer '{name}'"));
            let error = Diagnostic::error(error.code, message).with("printer", name);
            error_response(StatusCode::BAD_GATEWAY, &error)
        }
    }
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
        if let Some(dir) = &app.config.data_dir {
            forget(dir, id);
        }
        return StatusCode::NO_CONTENT.into_response();
    }
    job.status = Status::Cancelled;
    job.stop.store(true, Ordering::Relaxed);
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
            "printers": app.config.printers.iter().map(|(name, _)| name).collect::<Vec<_>>(),
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
