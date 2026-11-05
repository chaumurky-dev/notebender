//! Local web UI: a small axum server with an on-disk job store.
//!
//! Layout of a job on disk: `data_dir/jobs/<uuid>/{input.<ext>, request.json, job.json, out/}`.

use crate::pipeline::{run_job, Context, JobRequest, JobResult, ProgressEvent, ProgressSink};
use axum::extract::{DefaultBodyLimit, Multipart, Path as AxPath, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router as AxRouter};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock, Semaphore};
use uuid::Uuid;

const INDEX_HTML: &str = include_str!("static/index.html");
const APP_JS: &str = include_str!("static/app.js");
const STYLE_CSS: &str = include_str!("static/style.css");

/// Jobs run one at a time (constant for now).
const MAX_CONCURRENT_JOBS: usize = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Done,
    Failed { error: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub instructions: String,
    pub input_name: String,
    pub status: JobStatus,
    #[serde(default)]
    pub events: Vec<ProgressEvent>,
    #[serde(default)]
    pub result: Option<JobResult>,
}

type Jobs = Arc<RwLock<BTreeMap<Uuid, JobRecord>>>;

#[derive(Clone)]
struct AppState {
    ctx: Context,
    jobs_dir: PathBuf,
    jobs: Jobs,
    sem: Arc<Semaphore>,
}

struct ApiError(StatusCode, String);

impl ApiError {
    fn new(code: StatusCode, msg: impl Into<String>) -> Self {
        ApiError(code, msg.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

async fn persist(jobs_dir: &Path, rec: &JobRecord) {
    let path = jobs_dir.join(rec.id.to_string()).join("job.json");
    match serde_json::to_vec_pretty(rec) {
        Ok(bytes) => {
            if let Err(e) = tokio::fs::write(&path, bytes).await {
                tracing::warn!("could not persist {}: {e}", path.display());
            }
        }
        Err(e) => tracing::warn!("could not serialise job {}: {e}", rec.id),
    }
}

async fn load_jobs(jobs_dir: &Path) -> BTreeMap<Uuid, JobRecord> {
    let mut map = BTreeMap::new();
    let Ok(mut rd) = tokio::fs::read_dir(jobs_dir).await else {
        return map;
    };
    while let Ok(Some(entry)) = rd.next_entry().await {
        let path = entry.path().join("job.json");
        let Ok(bytes) = tokio::fs::read(&path).await else {
            continue;
        };
        match serde_json::from_slice::<JobRecord>(&bytes) {
            Ok(mut rec) => {
                if matches!(rec.status, JobStatus::Running | JobStatus::Queued) {
                    rec.status = JobStatus::Failed {
                        error: "interrupted by restart".into(),
                    };
                    persist(jobs_dir, &rec).await;
                }
                map.insert(rec.id, rec);
            }
            Err(e) => tracing::warn!("skipping unreadable {}: {e}", path.display()),
        }
    }
    map
}

fn build_router(ctx: Context, jobs_dir: PathBuf, jobs: Jobs) -> AxRouter {
    let limit = (ctx.config.limits.max_upload_mb as usize).saturating_mul(1024 * 1024);
    let state = AppState {
        ctx,
        jobs_dir,
        jobs,
        sem: Arc::new(Semaphore::new(MAX_CONCURRENT_JOBS)),
    };
    AxRouter::new()
        .route("/", get(index))
        .route("/static/app.js", get(app_js))
        .route("/static/style.css", get(style_css))
        .route("/api/health", get(health))
        .route(
            "/api/jobs",
            get(list_jobs).post(create_job).layer(DefaultBodyLimit::max(limit)),
        )
        .route("/api/jobs/{id}", get(get_job).delete(delete_job))
        .route("/api/jobs/{id}/files/{name}", get(get_file))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}

/// Start the web UI and block until ctrl-c.
pub async fn serve(ctx: Context, host: &str, port: u16, data_dir: PathBuf) -> anyhow::Result<()> {
    let jobs_dir = data_dir.join("jobs");
    tokio::fs::create_dir_all(&jobs_dir).await?;
    let jobs = Arc::new(RwLock::new(load_jobs(&jobs_dir).await));
    let app = build_router(ctx, jobs_dir, jobs);
    let listener = tokio::net::TcpListener::bind((host, port)).await?;
    tracing::info!("notebender web UI: http://{host}:{port}");
    println!("notebender web UI: http://{host}:{port}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

fn asset(ct: &'static str, body: &'static str) -> Response {
    ([(header::CONTENT_TYPE, ct)], body).into_response()
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}
async fn app_js() -> Response {
    asset("text/javascript; charset=utf-8", APP_JS)
}
async fn style_css() -> Response {
    asset("text/css; charset=utf-8", STYLE_CSS)
}

async fn health(State(st): State<AppState>) -> Json<serde_json::Value> {
    let ctx = &st.ctx;
    let routing: Vec<_> = ctx
        .router
        .table()
        .into_iter()
        .map(|(task, r)| {
            json!({
                "task": task,
                "tier": r.tier,
                "effort": r.effort,
                "model": ctx.config.model_id(r.tier),
            })
        })
        .collect();
    let tools: Vec<_> = ctx
        .tools
        .all()
        .into_iter()
        .map(|t| {
            let mut v = serde_json::to_value(t).unwrap_or_default();
            v["available"] = json!(t.available());
            v
        })
        .collect();
    Json(json!({
        "ok": true,
        "api_key_configured": ctx.client.is_some(),
        "tools": tools,
        "routing": routing,
    }))
}

async fn list_jobs(State(st): State<AppState>) -> Json<Vec<serde_json::Value>> {
    let jobs = st.jobs.read().await;
    let mut recs: Vec<&JobRecord> = jobs.values().collect();
    recs.sort_by_key(|r| std::cmp::Reverse(r.created_at));
    Json(
        recs.into_iter()
            .map(|r| {
                json!({
                    "id": r.id,
                    "created_at": r.created_at,
                    "status": r.status,
                    "instructions": r.instructions,
                    "input_name": r.input_name,
                    "total_cost_usd": r.result.as_ref().map(|x| x.total_cost_usd),
                })
            })
            .collect(),
    )
}

fn parse_id(s: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(s).map_err(|_| ApiError::new(StatusCode::NOT_FOUND, "no such job"))
}

async fn get_job(State(st): State<AppState>, AxPath(id): AxPath<String>) -> Result<Json<JobRecord>, ApiError> {
    let id = parse_id(&id)?;
    st.jobs
        .read()
        .await
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "no such job"))
}

async fn delete_job(State(st): State<AppState>, AxPath(id): AxPath<String>) -> Result<StatusCode, ApiError> {
    let id = parse_id(&id)?;
    {
        let mut jobs = st.jobs.write().await;
        match jobs.get(&id).map(|r| &r.status) {
            None => return Err(ApiError::new(StatusCode::NOT_FOUND, "no such job")),
            Some(JobStatus::Running) | Some(JobStatus::Queued) => {
                return Err(ApiError::new(StatusCode::CONFLICT, "job is still running"))
            }
            _ => {}
        }
        jobs.remove(&id);
    }
    let _ = tokio::fs::remove_dir_all(st.jobs_dir.join(id.to_string())).await;
    Ok(StatusCode::NO_CONTENT)
}

fn content_type_for(name: &str) -> &'static str {
    match name.rsplit('.').next().map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("pdf") => "application/pdf",
        Some("musicxml") | Some("xml") => "application/vnd.recordare.musicxml+xml",
        Some("ly") | Some("krn") | Some("abc") | Some("md") | Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

async fn get_file(
    State(st): State<AppState>,
    AxPath((id, name)): AxPath<(String, String)>,
) -> Result<Response, ApiError> {
    let id = parse_id(&id)?;
    if name.is_empty() || name.contains(['/', '\\', '\0']) || name == "." || name.contains("..") {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "invalid file name"));
    }
    if !st.jobs.read().await.contains_key(&id) {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such job"));
    }
    let path = st.jobs_dir.join(id.to_string()).join("out").join(&name);
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| ApiError::new(StatusCode::NOT_FOUND, "no such file"))?;
    let safe: String = name.chars().filter(|c| *c != '"' && !c.is_control()).collect();
    let mut resp = bytes.into_response();
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type_for(&name)));
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{safe}\"")) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(resp)
}

fn truthy(s: &str) -> bool {
    matches!(s.trim().to_ascii_lowercase().as_str(), "on" | "true" | "1" | "yes")
}

async fn create_job(
    State(st): State<AppState>,
    mut mp: Multipart,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let bad = |m: String| ApiError::new(StatusCode::BAD_REQUEST, m);
    let mut file: Option<(String, bytes::Bytes)> = None;
    let mut instructions = String::new();
    let (mut review, mut dry_run) = (false, false);
    while let Some(field) = mp.next_field().await.map_err(|e| bad(e.to_string()))? {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                let fname = field.file_name().unwrap_or("").to_string();
                let data = field.bytes().await.map_err(|e| bad(e.to_string()))?;
                file = Some((fname, data));
            }
            "instructions" => instructions = field.text().await.map_err(|e| bad(e.to_string()))?,
            "review" => review = truthy(&field.text().await.map_err(|e| bad(e.to_string()))?),
            "dry_run" => dry_run = truthy(&field.text().await.map_err(|e| bad(e.to_string()))?),
            _ => {}
        }
    }
    let (fname, data) = file.ok_or_else(|| bad("missing 'file' field".into()))?;
    if instructions.trim().is_empty() {
        return Err(bad("'instructions' must not be empty".into()));
    }
    let ext = fname
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    if !matches!(ext.as_str(), "pdf" | "musicxml" | "xml") {
        return Err(bad(
            "unsupported file type: upload a .pdf, .musicxml or .xml file".into()
        ));
    }
    if data.is_empty() {
        return Err(bad("uploaded file is empty".into()));
    }
    let id = Uuid::new_v4();
    let dir = st.jobs_dir.join(id.to_string());
    let out_dir = dir.join("out");
    tokio::fs::create_dir_all(&out_dir).await.map_err(internal)?;
    let input = dir.join(format!("input.{ext}"));
    tokio::fs::write(&input, &data).await.map_err(internal)?;
    let req = JobRequest {
        input,
        instructions: instructions.trim().to_string(),
        out_dir,
        plan: None,
        review,
        dry_run,
    };
    tokio::fs::write(
        dir.join("request.json"),
        serde_json::to_vec_pretty(&req).map_err(internal)?,
    )
    .await
    .map_err(internal)?;
    let input_name = Path::new(&fname)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or(fname);
    let rec = JobRecord {
        id,
        created_at: Utc::now(),
        instructions: req.instructions.clone(),
        input_name,
        status: JobStatus::Queued,
        events: vec![],
        result: None,
    };
    persist(&st.jobs_dir, &rec).await;
    st.jobs.write().await.insert(id, rec);
    tokio::spawn(run_one(st, id, req));
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

async fn update<F: FnOnce(&mut JobRecord)>(st: &AppState, id: Uuid, f: F) {
    let snapshot = {
        let mut jobs = st.jobs.write().await;
        let Some(rec) = jobs.get_mut(&id) else { return };
        f(rec);
        rec.clone()
    };
    persist(&st.jobs_dir, &snapshot).await;
}

async fn run_one(st: AppState, id: Uuid, req: JobRequest) {
    let Ok(_permit) = st.sem.clone().acquire_owned().await else {
        return;
    };
    update(&st, id, |r| r.status = JobStatus::Running).await;

    let (tx, mut rx) = mpsc::unbounded_channel::<ProgressEvent>();
    let sink: ProgressSink = Arc::new(move |ev| {
        let _ = tx.send(ev);
    });
    let consumer = {
        let st = st.clone();
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                update(&st, id, |r| r.events.push(ev)).await;
            }
        })
    };
    let outcome = run_job(&st.ctx, &req, sink).await;
    // `sink` was moved into run_job and dropped with it, so the consumer drains and ends.
    let _ = consumer.await;
    match outcome {
        Ok(res) => {
            update(&st, id, |r| {
                r.result = Some(res);
                r.status = JobStatus::Done;
            })
            .await
        }
        Err(e) => {
            tracing::warn!("job {id} failed: {e:#}");
            update(&st, id, |r| {
                r.status = JobStatus::Failed {
                    error: format!("{e:#}"),
                }
            })
            .await
        }
    }
}

#[cfg(test)]
mod tests;
