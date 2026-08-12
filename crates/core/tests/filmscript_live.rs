//! Live smoke against the real Kie + TTS APIs (dev-plan/52 M3) —
//! `#[ignore]`d so CI never spends money. Run deliberately:
//!
//! ```sh
//! KIE_API_KEY=… GEMINI_API_KEY=… \
//!   cargo test --test filmscript_live -- --ignored --nocapture
//! ```
//!
//! One 4s/720p dialogue shot (~$0.82 + TTS cents): synthetic character
//! image (plumbing smoke — generation *quality* was validated by human
//! eyes in T0), real Thai TTS, real audio-ref lip-sync payload, real
//! poll + download.

use std::time::{Duration, Instant};
use thclaws_core::filmscript::harness::job;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "spends real money — run with --ignored and live keys"]
async fn live_smoke_one_dialogue_shot() {
    for key in ["KIE_API_KEY"] {
        assert!(
            std::env::var(key)
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false),
            "{key} must be set for the live smoke"
        );
    }

    let ws = tempfile::tempdir().unwrap();
    std::env::set_current_dir(ws.path()).unwrap();
    let ok = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-v",
            "quiet",
            "-f",
            "lavfi",
            "-i",
            "gradients=size=640x800:c0=peachpuff:c1=saddlebrown",
            "-frames:v",
            "1",
            "anny.png",
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(ok, "ffmpeg generates the placeholder character image");

    let script = "char $แอน = @./anny.png voice:th-female-warm desc:\"หญิงสาวในภาพวาดสีอบอุ่น\"\n\
                  shot 1 (dialogue) {\n\
                  $แอน มองมาที่กล้องอย่างสงบ\n\
                  camera: static medium shot\n\
                  $แอน say \"สวัสดีค่ะ\"\n\
                  @duration: 4\n\
                  }\n";

    let job_id = job::start(script, 2.0, false).expect("live job starts");
    eprintln!("live job: {job_id}");

    let deadline = Instant::now() + Duration::from_secs(600);
    let state = loop {
        let s = job::JobState::load(&job_id).expect("job.json");
        if s.state != "running" {
            break s;
        }
        assert!(Instant::now() < deadline, "live job timed out: {s:?}");
        std::thread::sleep(Duration::from_secs(5));
    };

    eprintln!(
        "final state: {}",
        serde_json::to_string_pretty(&state).unwrap()
    );
    assert_eq!(state.state, "done", "{state:?}");
    let shot = &state.shots[0];
    assert_eq!(shot.state, "done");
    let clip_rel = shot.clip.as_ref().expect("clip recorded");
    let clip = std::path::Path::new(".thclaws/film")
        .join(&job_id)
        .join(clip_rel);
    assert!(clip.exists(), "clip downloaded to {}", clip.display());
    assert!(shot.credits.unwrap_or(0.0) > 0.0, "credits recorded");
    // Keep the artifact somewhere inspectable before the tempdir drops.
    let keep = std::env::var("THCLAWS_LIVE_KEEP").unwrap_or_default();
    if !keep.is_empty() {
        std::fs::copy(&clip, format!("{keep}/live_smoke.mp4")).unwrap();
        eprintln!("clip kept at {keep}/live_smoke.mp4");
    }
}
