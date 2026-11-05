use super::*;
use crate::config::Config;
use crate::router::Router as TaskRouter;
use crate::tools::Toolbox;
use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

fn app(dir: &Path) -> AxRouter {
    let config = Config::default();
    let ctx = Context {
        router: Arc::new(TaskRouter::from_config(&config)),
        tools: Arc::new(Toolbox::detect(&config.tools)),
        config: Arc::new(config),
        client: None,
    };
    build_router(ctx, dir.join("jobs"), Arc::new(RwLock::new(BTreeMap::new())))
}

async fn body_json(resp: Response) -> serde_json::Value {
    let b = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&b).unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

fn multipart(filename: &str, instructions: &str) -> Request<Body> {
    let b = "XBOUNDARYX";
    let body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n%PDF-1.4 tiny\r\n--{b}\r\nContent-Disposition: form-data; name=\"instructions\"\r\n\r\n{instructions}\r\n--{b}--\r\n"
    );
    Request::builder()
        .method("POST")
        .uri("/api/jobs")
        .header("content-type", format!("multipart/form-data; boundary={b}"))
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn serves_index_and_health() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("jobs")).unwrap();
    let app = app(tmp.path());
    let r = app.clone().oneshot(get("/")).await.unwrap();
    assert_eq!(r.status(), 200);
    let b = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
    assert!(String::from_utf8_lossy(&b).contains("notebender"));
    let r = app.clone().oneshot(get("/static/app.js")).await.unwrap();
    assert_eq!(r.status(), 200);
    let r = app.oneshot(get("/api/health")).await.unwrap();
    let v = body_json(r).await;
    assert_eq!(v["ok"], true);
    assert_eq!(v["api_key_configured"], false);
    assert!(v["routing"].as_array().unwrap().len() > 3);
}

#[tokio::test]
async fn job_lifecycle() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("jobs")).unwrap();
    let app = app(tmp.path());

    let r = app.clone().oneshot(multipart("score.txt", "x")).await.unwrap();
    assert_eq!(r.status(), 400);
    let r = app.clone().oneshot(multipart("score.pdf", "  ")).await.unwrap();
    assert_eq!(r.status(), 400);

    let r = app
        .clone()
        .oneshot(multipart("score.pdf", "Transpose to C"))
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let id = body_json(r).await["id"].as_str().unwrap().to_string();
    assert!(tmp.path().join("jobs").join(&id).join("input.pdf").exists());

    let mut state = String::new();
    for _ in 0..50 {
        let r = app.clone().oneshot(get(&format!("/api/jobs/{id}"))).await.unwrap();
        assert_eq!(r.status(), 200);
        let v = body_json(r).await;
        state = v["status"]["state"].as_str().unwrap().to_string();
        if state == "failed" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(state, "failed");
    assert!(tmp.path().join("jobs").join(&id).join("job.json").exists());

    let v = body_json(app.clone().oneshot(get("/api/jobs")).await.unwrap()).await;
    assert_eq!(v.as_array().unwrap().len(), 1);

    for bad in ["../etc", "..%2Fetc", "a%2Fb"] {
        let r = app
            .clone()
            .oneshot(get(&format!("/api/jobs/{id}/files/{bad}")))
            .await
            .unwrap();
        assert!(r.status() == 400 || r.status() == 404, "{bad}: {}", r.status());
    }
    let r = app
        .clone()
        .oneshot(get(&format!("/api/jobs/{id}/files/nope.pdf")))
        .await
        .unwrap();
    assert_eq!(r.status(), 404);

    let del = Request::builder()
        .method("DELETE")
        .uri(format!("/api/jobs/{id}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.clone().oneshot(del).await.unwrap().status(), 204);
    assert!(!tmp.path().join("jobs").join(&id).exists());
}

#[tokio::test]
async fn unknown_job_is_404() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app(tmp.path());
    let id = Uuid::new_v4();
    assert_eq!(
        app.clone()
            .oneshot(get(&format!("/api/jobs/{id}")))
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(app.oneshot(get("/api/jobs/not-a-uuid")).await.unwrap().status(), 404);
}
