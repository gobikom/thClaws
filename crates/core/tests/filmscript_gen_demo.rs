//! One-off live demo: a full FilmGenerate from a single .film script,
//! Gemini TTS, real Kie generation + assembly. #[ignore]d (spends
//! money). Run from the T0 spike dir so @./assets/*.png resolve:
//!
//! ```sh
//! FILM_DEMO_DIR=/abs/dev-plan/52-t0-spike KIE_API_KEY=… GEMINI_API_KEY=… \
//!   cargo test --test filmscript_gen_demo -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};
use thclaws_core::filmscript::harness::job;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "spends real money — run with --ignored + live keys"]
async fn generate_two_shot_scene_gemini_tts() {
    let dir = std::env::var("FILM_DEMO_DIR").expect("set FILM_DEMO_DIR to the spike dir");
    std::env::set_current_dir(&dir).unwrap();
    assert!(
        std::path::Path::new("assets/anny.png").exists()
            && std::path::Path::new("assets/tony.png").exists(),
        "expected assets/anny.png + assets/tony.png in FILM_DEMO_DIR"
    );

    // th-female-warm → Kore, th-male-low → Charon (both gemini in the
    // built-in registry, direct generateContent when GEMINI_API_KEY set).
    let script = "\
film \"เช้าที่เงียบงัน\" {\n\
    aspect: 16:9\n\
    style: naturalistic Thai drama, warm muted palette\n\
}\n\
char  $แอน  = @./assets/anny.png  voice:th-female-warm  desc:\"หญิงสาวผมยาวสวมเสื้อครีม\"\n\
char  $โทนี่ = @./assets/tony.png  voice:th-male-low     desc:\"ชายหนุ่มไว้เคราสวมแจ็คเก็ตน้ำเงิน\"\n\
sequence \"บทสนทนา\" {\n\
    shot 1 (dialogue) {\n\
        $แอน นั่งที่โต๊ะไม้ริมหน้าต่าง มองมาทางกล้อง\n\
        camera: medium two-shot, static\n\
        $แอน say \"คุณหายไปไหนมาตั้งแต่เช้า\"\n\
        @duration: 4\n\
        @model: fast\n\
        @resolution: 480p\n\
    }\n\
    shot 2 (dialogue) {\n\
        close-up $โทนี่\n\
        expression: หลบสายตาเล็กน้อย\n\
        $โทนี่ say \"ไปทำงานมา\"\n\
        camera: static\n\
        @continue_from: 1\n\
        @duration: 4\n\
        @model: fast\n\
        @resolution: 480p\n\
        @transition: to_black 1.0\n\
    }\n\
}\n";

    let job_id = job::start(script, 4.0, false).expect("job starts");
    eprintln!("gen demo job: {job_id}");

    let deadline = Instant::now() + Duration::from_secs(1800);
    let state = loop {
        let s = job::JobState::load(&job_id).expect("job.json");
        if s.state != "running" {
            break s;
        }
        eprintln!(
            "  … {}",
            s.shots
                .iter()
                .map(|sh| format!("{}={}", sh.id, sh.state))
                .collect::<Vec<_>>()
                .join(" ")
        );
        assert!(Instant::now() < deadline, "timed out: {s:?}");
        std::thread::sleep(Duration::from_secs(15));
    };

    eprintln!("final: {}", serde_json::to_string_pretty(&state).unwrap());
    assert_eq!(state.state, "done", "{state:?}");
    let arts = state.artifacts.as_ref().expect("artifacts");
    let mp4 = std::path::Path::new(".thclaws/film")
        .join(&job_id)
        .join(arts["mp4"].as_str().unwrap());
    let keep = std::env::var("THCLAWS_LIVE_KEEP").unwrap_or_else(|_| "out".into());
    std::fs::copy(&mp4, format!("{keep}/gen_demo_fixed.mp4")).unwrap();
    std::fs::copy(
        std::path::Path::new(".thclaws/film")
            .join(&job_id)
            .join(arts["srt"].as_str().unwrap()),
        format!("{keep}/gen_demo_fixed.srt"),
    )
    .unwrap();
    eprintln!(
        "kept: {keep}/gen_demo_fixed.mp4  (spent {} credits)",
        state.spent_credits
    );
}
