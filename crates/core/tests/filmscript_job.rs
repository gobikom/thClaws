//! FilmScript Tier-3 acceptance (dev-plan/52 M3): a two-shot film —
//! upload, createTask, poll, download, continuation ref-prep — runs
//! end-to-end against a mocked Kie server whose response shapes are
//! copied from the T0 spike's recorded fixtures. Then the resume
//! guarantee: a second run of the same script completes from the
//! shot-result cache with ZERO new createTasks (never double-spend).
//!
//! Needs ffmpeg/ffprobe (ref-video prep + probes); skips politely when
//! absent so bare CI runners stay green.

use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use thclaws_core::filmscript::harness::job;

fn have_av_tools() -> bool {
    ["ffmpeg", "ffprobe"].iter().all(|t| {
        std::process::Command::new(t)
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

fn ffmpeg(args: &[&str]) {
    let ok = std::process::Command::new("ffmpeg")
        .args(["-y", "-v", "quiet"])
        .args(args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(ok, "ffmpeg {args:?}");
}

async fn mock_kie(clip: Vec<u8>, tasks: Arc<AtomicUsize>) -> String {
    use axum::extract::State;
    use axum::routing::{get, post};

    #[derive(Clone)]
    struct S {
        clip: Arc<Vec<u8>>,
        tasks: Arc<AtomicUsize>,
        base: String,
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base = format!("http://{addr}");

    let state = S {
        clip: Arc::new(clip),
        tasks,
        base: base.clone(),
    };
    let app = axum::Router::new()
        .route(
            "/api/file-stream-upload",
            post(|State(s): State<S>, _body: axum::body::Bytes| async move {
                let n = s.tasks.load(Ordering::Relaxed);
                axum::Json(json!({
                    "success": true, "code": 200,
                    "data": { "downloadUrl": format!("mock://upload-{n}") }
                }))
            }),
        )
        .route(
            "/api/v1/jobs/createTask",
            post(
                |State(s): State<S>, axum::Json(_p): axum::Json<Value>| async move {
                    let n = s.tasks.fetch_add(1, Ordering::Relaxed) + 1;
                    axum::Json(json!({
                        "code": 200, "msg": "success",
                        "data": { "taskId": format!("task-{n}") }
                    }))
                },
            ),
        )
        .route(
            "/api/v1/jobs/recordInfo",
            get(|State(s): State<S>| async move {
                // Shape from fixtures/poll_*.json — resultJson is a STRING.
                let result = format!("{{\"resultUrls\":[\"{}/clip.mp4\"]}}", s.base);
                axum::Json(json!({
                    "code": 200, "msg": "success",
                    "data": {
                        "state": "success",
                        "creditsConsumed": 246.0,
                        "resultJson": result
                    }
                }))
            }),
        )
        .route(
            "/clip.mp4",
            get(|State(s): State<S>| async move { s.clip.as_ref().clone() }),
        )
        .with_state(state);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    base
}

#[tokio::test(flavor = "multi_thread")]
async fn two_shot_job_end_to_end_then_resume_without_respend() {
    if !have_av_tools() {
        eprintln!("skipping: ffmpeg/ffprobe not on PATH");
        return;
    }

    let ws = tempfile::tempdir().unwrap();
    std::env::set_current_dir(ws.path()).unwrap();

    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "color=red:size=320x320",
        "-frames:v",
        "1",
        "anny.png",
    ]);
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "color=blue:size=320x320:rate=24",
        "-t",
        "1",
        "clip.mp4",
    ]);
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=3",
        "theme.wav",
    ]);
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=880:duration=1",
        "door.wav",
    ]);
    let clip_bytes = std::fs::read("clip.mp4").unwrap();

    let tasks = Arc::new(AtomicUsize::new(0));
    let base = mock_kie(clip_bytes, tasks.clone()).await;
    std::env::set_var("KIE_BASE_URL", &base);
    std::env::set_var("KIE_UPLOAD_BASE_URL", &base);
    std::env::set_var("KIE_API_KEY", "test-key");

    let script = "char $แอน = @./anny.png desc:\"หญิงสาว\"\n\
                  music $เพลง = @./theme.wav\n\
                  sfx $ประตู = @./door.wav\n\
                  sequence \"สั้น\" {\n\
                  music: $เพลง volume:0.4\n\
                  shot 1 {\n$แอน ยืนมองหน้าต่าง\nsfx: $ประตู at 0.5s\n@duration: 4\n}\n\
                  shot 2 {\n$แอน หันหลังเดินออก\n@continue_from: 1\n@duration: 4\n@transition: to_black 0.5\n}\n\
                  }\n";

    let job_id = job::start(script, 10.0, false).expect("job starts");
    let state = wait_terminal(&job_id, Duration::from_secs(90));
    assert_eq!(state.state, "done", "{state:?}");
    assert!(state.shots.iter().all(|s| s.state == "done"), "{state:?}");
    assert_eq!(
        tasks.load(Ordering::Relaxed),
        2,
        "two createTasks for two shots"
    );
    assert_eq!(state.spent_credits, 492.0);
    for s in &state.shots {
        let clip = s.clip.as_ref().expect("clip path");
        assert!(
            std::path::Path::new(".thclaws/film")
                .join(&job_id)
                .join(clip)
                .exists(),
            "clip file exists"
        );
    }

    // Tier 4: assembly artifacts land next to the clips.
    let arts = state
        .artifacts
        .as_ref()
        .expect("assembly artifacts recorded");
    for key in ["mp4", "srt", "manifest"] {
        let rel = arts[key].as_str().expect(key);
        let p = std::path::Path::new(".thclaws/film")
            .join(&job_id)
            .join(rel);
        assert!(p.exists(), "{key} exists at {}", p.display());
    }
    let final_mp4 = std::path::Path::new(".thclaws/film")
        .join(&job_id)
        .join(arts["mp4"].as_str().unwrap());
    let probe = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
        ])
        .arg(&final_mp4)
        .output()
        .unwrap();
    let dur: f64 = String::from_utf8_lossy(&probe.stdout)
        .trim()
        .parse()
        .unwrap();
    assert!(dur > 1.5, "two 1s clips concatenated, got {dur}s");

    // Resume: same script → same job id, cache satisfies every shot,
    // no new createTask fires.
    let again = job::start(script, 10.0, true).expect("resume starts");
    assert_eq!(again, job_id);
    let state = wait_terminal(&job_id, Duration::from_secs(30));
    assert_eq!(state.state, "done");
    assert_eq!(tasks.load(Ordering::Relaxed), 2, "resume must not re-spend");

    // Budget gate: an estimate over budget refuses before any spend.
    let err = job::start(script, 0.01, true).unwrap_err().to_string();
    assert!(err.contains("budget"), "{err}");
}

fn wait_terminal(job_id: &str, timeout: Duration) -> job::JobState {
    let start = Instant::now();
    loop {
        if let Ok(s) = job::JobState::load(job_id) {
            if s.state != "running" {
                return s;
            }
        }
        assert!(start.elapsed() < timeout, "job did not finish in time");
        std::thread::sleep(Duration::from_millis(300));
    }
}
