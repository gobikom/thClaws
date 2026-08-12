//! FilmScript Tier-1 acceptance (dev-plan/52 M1): the flat two-shot
//! derivative of spec §4 compiles to byte-stable asset requests and
//! Kie payloads. The golden file freezes prompt assembly that was
//! validated live against Seedance in the T0 spike — a diff here is a
//! semantic change to what we send the model, never noise.
//!
//! Regenerate intentionally with:
//! `UPDATE_GOLDEN=1 cargo test --test filmscript_golden`

use serde_json::{json, Value};
use std::path::PathBuf;
use thclaws_core::filmscript::{compile_phase1, compile_phase2, AssetRequest, ResolvedAsset};

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/filmscript/golden")
}

/// Deterministic mock fulfilment: url derives from the asset id; TTS
/// durations are the T0-measured real ones per voice.
fn mock_assets(requests: &[AssetRequest]) -> Vec<ResolvedAsset> {
    requests
        .iter()
        .map(|r| ResolvedAsset {
            id: r.id().to_string(),
            url: format!("mock://{}", r.id()),
            duration_ms: match r {
                AssetRequest::Tts { voice, .. } if voice == "th-female-warm" => Some(4920),
                AssetRequest::Tts { .. } => Some(2256),
                _ => None,
            },
        })
        .collect()
}

#[test]
fn two_shot_golden() {
    let dir = golden_dir();
    let source = std::fs::read_to_string(dir.join("two_shot.film")).unwrap();

    let p1 = compile_phase1(&source);
    assert!(!p1.has_errors(), "phase1 errors: {:#?}", p1.errors);
    let p2 = compile_phase2(&p1, &mock_assets(&p1.asset_requests));
    assert!(
        !p2.errors
            .iter()
            .any(|e| matches!(e.severity, thclaws_core::filmscript::Severity::Error)),
        "phase2 errors: {:#?}",
        p2.errors
    );

    let actual = json!({
        "asset_requests": p1.asset_requests,
        "payloads": p2.payloads,
    });
    let actual_pretty = serde_json::to_string_pretty(&actual).unwrap() + "\n";

    let golden_path = dir.join("two_shot.expected.json");
    if std::env::var("UPDATE_GOLDEN").is_ok() {
        std::fs::write(&golden_path, &actual_pretty).unwrap();
        return;
    }
    let expected: Value =
        serde_json::from_str(&std::fs::read_to_string(&golden_path).unwrap_or_else(|_| {
            panic!("missing golden file {golden_path:?} — run UPDATE_GOLDEN=1 once")
        }))
        .unwrap();
    assert_eq!(
        actual, expected,
        "golden mismatch — if the prompt/payload change is intentional, \
         rerun with UPDATE_GOLDEN=1 and review the diff"
    );
}

#[test]
fn thai_errors_for_thai_scripts() {
    let p1 = compile_phase1("char $แอน = @./a.png desc:\"x\"\nshot 1 {\n$แอน say \"สวัสดี\"\n}\n");
    let err = p1
        .errors
        .iter()
        .find(|e| e.code == "E_TTS_NO_VOICE")
        .expect("voice error");
    assert!(err.message.contains("ผูก voice"), "{}", err.message);

    let p1 = compile_phase1("char $a = @./a.png desc:\"x\"\nshot 1 {\n$a say \"hello\"\n}\n");
    let err = p1
        .errors
        .iter()
        .find(|e| e.code == "E_TTS_NO_VOICE")
        .expect("voice error");
    assert!(err.message.contains("voice binding"), "{}", err.message);
}

/// Tier-2 acceptance (dev-plan/52 M2): the full spec §4 three-shot film
/// — header, sequence + music, scene views, continuation, transition —
/// compiles to byte-stable requests, payloads, assembly plan and cost.
#[test]
fn full_film_golden() {
    let dir = golden_dir();
    let source = std::fs::read_to_string(dir.join("full_film.film")).unwrap();

    let p1 = compile_phase1(&source);
    assert!(!p1.has_errors(), "phase1 errors: {:#?}", p1.errors);
    let p2 = compile_phase2(&p1, &mock_assets(&p1.asset_requests));
    assert!(
        !p2.errors
            .iter()
            .any(|e| matches!(e.severity, thclaws_core::filmscript::Severity::Error)),
        "phase2 errors: {:#?}",
        p2.errors
    );

    let actual = json!({
        "asset_requests": p1.asset_requests,
        "assembly_plan": p1.assembly_plan,
        "cost": p1.cost,
        "payloads": p2.payloads,
    });
    let actual_pretty = serde_json::to_string_pretty(&actual).unwrap() + "\n";

    let golden_path = dir.join("full_film.expected.json");
    if std::env::var("UPDATE_GOLDEN").is_ok() {
        std::fs::write(&golden_path, &actual_pretty).unwrap();
        return;
    }
    let expected: Value =
        serde_json::from_str(&std::fs::read_to_string(&golden_path).unwrap_or_else(|_| {
            panic!("missing golden file {golden_path:?} — run UPDATE_GOLDEN=1 once")
        }))
        .unwrap();
    assert_eq!(
        actual, expected,
        "golden mismatch — if the change is intentional, rerun with \
         UPDATE_GOLDEN=1 and review the diff"
    );
}
