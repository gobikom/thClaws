//! Phase 2: codegen. Assembles the prompt in the spec's load-bearing
//! order (§3.6 — subject → action/beats → camera → lighting/style →
//! ambient → dialogue → negatives; first 20–30 words weigh most,
//! Appendix A) and maps the shot onto a Kie createTask payload. Prompt
//! shape was validated live in T0 — golden tests freeze it; change
//! with intent only.
//!
//! Identity is image-led, disambiguation text-led: every `$handle`
//! mention renders as `{descriptor} @ImageN` on first mention and
//! `@ImageN` after, and the dialogue clause re-anchors the speaker with
//! both (the anti-face-blend rule).

use super::ast::ShotLine;
use super::phase1::{PartialShot, Phase1Result, ShotEntity};
use super::resolve::{Mode, ResolvedShot};
use super::{lexer, msg, CompileError, EntityKind};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedAsset {
    pub id: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShotPayload {
    pub shot_id: String,
    /// Earlier shot whose clip this one needs (`@continue_from` or
    /// `@match_cut`) — the harness DAG orders generation with this.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<String>,
    /// `@seed` is harness metadata (Kie's createTask has no seed
    /// field) — recorded per attempt for reproducibility bookkeeping.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    /// The selected backend — the harness dispatches submit/poll on it and
    /// refuses backends whose client isn't wired yet (`harness_wired`).
    #[serde(default)]
    pub backend: super::backend::BackendId,
    pub payload: Value,
}

#[derive(Debug)]
pub struct Phase2Result {
    pub payloads: Vec<ShotPayload>,
    pub errors: Vec<CompileError>,
}

pub fn compile_phase2(phase1: &Phase1Result, assets: &[ResolvedAsset]) -> Phase2Result {
    let thai = phase1.thai;
    if phase1.has_errors() {
        return Phase2Result {
            payloads: Vec::new(),
            errors: vec![CompileError::error(
                "E_PHASE1_ERRORS",
                None,
                msg(
                    thai,
                    "phase 2 ปฏิเสธ: script ยังมี compile error — แก้ให้หมดก่อน (กันยิงงานเสียเงินจาก script พัง)",
                    "phase 2 refused: the script still has compile errors — fix them first (never fire paid calls from a broken script)",
                ),
            )],
        };
    }
    let by_id: HashMap<&str, &ResolvedAsset> = assets.iter().map(|a| (a.id.as_str(), a)).collect();
    let mut errors = Vec::new();
    let mut payloads = Vec::new();

    for shot in &phase1.shots {
        let sid = shot.id();
        let mut missing: Vec<&str> = Vec::new();

        let mut image_urls = Vec::new();
        for e in &shot.entities {
            match by_id.get(e.asset_id.as_str()) {
                Some(a) => image_urls.push(a.url.clone()),
                None => missing.push(&e.asset_id),
            }
        }
        fn url_of<'a>(
            by_id: &HashMap<&str, &ResolvedAsset>,
            id: Option<&'a str>,
            missing: &mut Vec<&'a str>,
        ) -> Option<String> {
            id.and_then(|id| match by_id.get(id) {
                Some(a) => Some(a.url.clone()),
                None => {
                    missing.push(id);
                    None
                }
            })
        }
        let audio_url = url_of(&by_id, shot.tts_asset.as_deref(), &mut missing);
        let video_url = url_of(&by_id, shot.video_asset.as_deref(), &mut missing);
        let frame_url = url_of(&by_id, shot.frame_asset.as_deref(), &mut missing);

        if !missing.is_empty() {
            errors.push(CompileError::error(
                "E_ASSET_UNRESOLVED",
                Some(sid),
                msg(
                    thai,
                    &format!("shot {sid}: asset ยังไม่ resolve: {}", missing.join(", ")),
                    &format!("shot {sid}: unresolved assets: {}", missing.join(", ")),
                ),
            ));
            continue;
        }

        if let Some(tts_id) = shot.tts_asset.as_deref() {
            let dur_ms = by_id.get(tts_id).and_then(|a| a.duration_ms);
            match dur_ms {
                None => {
                    errors.push(CompileError::error(
                        "E_ASSET_UNRESOLVED",
                        Some(sid),
                        msg(
                            thai,
                            &format!("shot {sid}: TTS asset ไม่มี duration_ms — harness ต้องส่งมาด้วย"),
                            &format!("shot {sid}: TTS asset lacks duration_ms — the harness must supply it"),
                        ),
                    ));
                    continue;
                }
                Some(ms) => {
                    let audio_s = ms as f64 / 1000.0;
                    let min = (audio_s + 0.5).ceil() as u32;
                    if (shot.resolved.duration as f64) < audio_s + 0.5 {
                        errors.push(CompileError::error(
                            "E_AUDIO_OVERRUN",
                            Some(sid),
                            msg(
                                thai,
                                &format!(
                                    "shot {sid}: เสียงพูดยาว {audio_s:.1}s แต่ช็อต {}s — เพิ่ม @duration เป็น ≥{min}s หรือตัดบท",
                                    shot.resolved.duration
                                ),
                                &format!(
                                    "shot {sid}: speech runs {audio_s:.1}s but the shot is {}s — raise @duration to ≥{min}s or cut the line",
                                    shot.resolved.duration
                                ),
                            ),
                        ));
                        continue;
                    }
                    // Dead time invites invented speech: if the shot is
                    // much longer than the line, the model fills the gap
                    // with made-up dialogue even with the verbatim prompt
                    // clause (the T4 gen-demo bug). Warn so the author
                    // tightens @duration toward the audio length.
                    if (shot.resolved.duration as f64) > audio_s + 2.5 {
                        errors.push(CompileError::warning(
                            "W_DIALOGUE_DEAD_TIME",
                            Some(sid),
                            msg(
                                thai,
                                &format!(
                                    "shot {sid}: เสียงพูด {audio_s:.1}s แต่ช็อตยาว {}s — ช่องว่างมากทำให้โมเดลแต่งบทเพิ่มเอง ลด @duration ให้ใกล้ {}s",
                                    shot.resolved.duration,
                                    (audio_s + 1.0).ceil() as u32
                                ),
                                &format!(
                                    "shot {sid}: speech is {audio_s:.1}s but the shot is {}s — the gap makes the model invent extra dialogue; trim @duration toward {}s",
                                    shot.resolved.duration,
                                    (audio_s + 1.0).ceil() as u32
                                ),
                            ),
                        ));
                    }
                }
            }
        }

        if audio_url.is_some() && image_urls.is_empty() && video_url.is_none() {
            errors.push(CompileError::error(
                "E_REF_AUDIO_ONLY",
                Some(sid),
                msg(
                    thai,
                    &format!("shot {sid}: มี audio ref แต่ไม่มีรูป/วิดีโอ — Kie ต้องมี ref ภาพอย่างน้อยหนึ่ง"),
                    &format!("shot {sid}: audio ref without any image/video ref — Kie requires at least one visual ref"),
                ),
            ));
            continue;
        }

        let prompt = render_prompt(shot);
        // Codegen dispatches on the shot's selected backend (D4: Grok
        // default + Seedance; D5 adds the rest). Validation already blocked
        // unbuilt backends (E_BACKEND_NOT_BUILT).
        let payload = super::backend::build_payload(
            shot.resolved.backend,
            shot,
            &prompt,
            &image_urls,
            frame_url.as_deref(),
            video_url.as_deref(),
            audio_url.as_deref(),
        );

        payloads.push(ShotPayload {
            shot_id: sid.to_string(),
            depends_on: shot.depends_on().map(str::to_string),
            seed: shot.resolved.seed,
            backend: shot.resolved.backend,
            payload,
        });
    }

    Phase2Result { payloads, errors }
}

fn display(e: &ShotEntity) -> &str {
    e.desc
        .as_deref()
        .or(e.alias.as_deref())
        .unwrap_or(&e.handle)
}

fn sentence(out: &mut String, s: &str) {
    let s = s.trim();
    if s.is_empty() {
        return;
    }
    if !out.is_empty() {
        out.push(' ');
    }
    out.push_str(s);
    if !s.ends_with(['.', '!', '?']) {
        out.push('.');
    }
}

fn lang_name(lang: &str) -> &'static str {
    match lang {
        "th" => "Thai",
        "en" => "English",
        _ => "the same language",
    }
}

fn time_lighting(time: &str) -> Option<&'static str> {
    Some(match time {
        "dawn" => "Soft dawn light",
        "day" => "Natural daylight",
        "golden_hour" => "Warm golden-hour light",
        "dusk" => "Dim dusk light",
        "night" => "Low-key night lighting",
        _ => return None,
    })
}

struct Mentions<'a> {
    by_key: HashMap<String, &'a ShotEntity>,
    seen: Vec<String>,
}

impl<'a> Mentions<'a> {
    fn new(entities: &'a [ShotEntity]) -> Self {
        let mut by_key = HashMap::new();
        for e in entities {
            let key = match &e.tag {
                Some(t) => format!("{}#{t}", e.handle),
                None => e.handle.clone(),
            };
            by_key.insert(key, e);
        }
        Self {
            by_key,
            seen: Vec::new(),
        }
    }

    fn mention(&mut self, raw: &str) -> Option<String> {
        let e = self.by_key.get(raw)?;
        Some(if self.seen.iter().any(|s| s == &e.handle) {
            format!("@Image{}", e.image_n)
        } else {
            self.seen.push(e.handle.clone());
            format!("{} @Image{}", display(e), e.image_n)
        })
    }

    fn expand(&mut self, text: &str) -> String {
        let mut rendered = String::new();
        let mut rest = text;
        while let Some(pos) = rest.find('$') {
            rendered.push_str(&rest[..pos]);
            let (h, tail) = lexer::scan_handle(&rest[pos + 1..]);
            match self.mention(&h) {
                Some(m) => rendered.push_str(&m),
                None => {
                    rendered.push('$');
                    rendered.push_str(&h);
                }
            }
            rest = tail;
        }
        rendered.push_str(rest);
        rendered
    }
}

fn render_prompt(shot: &PartialShot) -> String {
    let r: &ResolvedShot = &shot.resolved;
    let mut m = Mentions::new(&shot.entities);

    let mut action_text = String::new();
    for line in &r.action {
        if let ShotLine::Action { text, .. } = line {
            let rendered = m.expand(text);
            sentence(&mut action_text, &rendered);
        }
    }

    let mut out = String::new();
    for e in &shot.entities {
        if e.kind != EntityKind::Scene && !m.seen.iter().any(|s| s == &e.handle) {
            m.seen.push(e.handle.clone());
            sentence(&mut out, &format!("{} @Image{}", display(e), e.image_n));
        }
    }
    if !action_text.is_empty() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&action_text);
    }
    for e in &shot.entities {
        if e.kind == EntityKind::Scene && !m.seen.iter().any(|s| s == &e.handle) {
            m.seen.push(e.handle.clone());
            sentence(
                &mut out,
                &format!("Inside {} @Image{}", display(e), e.image_n),
            );
        }
    }

    // Timeline beats, in authored order — Seedance respects ordered,
    // time-marked segments (validated in T0 shot 3).
    for beat in &r.beats {
        let content = match &beat.content {
            ShotLine::Action { text, .. } => m.expand(text),
            ShotLine::Property { key, value, .. } if key == "camera" => m.expand(value),
            _ => continue,
        };
        sentence(
            &mut out,
            &format!(
                "[{:02}:{:02}-{:02}:{:02}] {content}",
                beat.t1 / 60,
                beat.t1 % 60,
                beat.t2 / 60,
                beat.t2 % 60
            ),
        );
    }

    let props: Vec<(String, String)> = ["expression", "gesture", "camera", "mood"]
        .iter()
        .filter_map(|k| {
            r.properties
                .get(*k)
                .map(|(v, _)| (k.to_string(), m.expand(v)))
        })
        .collect();
    let prop = |k: &str| {
        props
            .iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.as_str())
    };
    if let Some(v) = prop("expression") {
        sentence(&mut out, &format!("Expression: {v}"));
    }
    if let Some(v) = prop("gesture") {
        sentence(&mut out, &format!("Gesture: {v}"));
    }
    if let Some(v) = prop("camera") {
        sentence(&mut out, v);
    }
    match &r.lighting {
        Some(v) => {
            let v = m.expand(v);
            sentence(&mut out, &v);
        }
        None => {
            if let Some(t) = shot
                .entities
                .iter()
                .find(|e| e.kind == EntityKind::Scene)
                .and_then(|e| e.time.as_deref())
                .and_then(time_lighting)
            {
                sentence(&mut out, t);
            }
        }
    }
    if let Some(v) = &r.style {
        let v = m.expand(v);
        sentence(&mut out, &v);
    }
    if let Some(v) = &r.genre {
        sentence(&mut out, &format!("Genre: {v}"));
    }
    if let Some(v) = &r.fps_look {
        sentence(&mut out, v);
    }
    if let Some(v) = prop("mood") {
        sentence(&mut out, &format!("Mood: {v}"));
    }
    if let Some(v) = &r.ambient {
        if r.audio_on {
            let v = m.expand(v);
            sentence(&mut out, &format!("Ambient sound: {v}"));
        }
    }

    if let Some(d) = &r.dialogue {
        let key = &d.speaker;
        if let Some(e) = m
            .by_key
            .get(key.as_str())
            .copied()
            .or_else(|| m.by_key.values().find(|e| &e.handle == key).copied())
        {
            // Dialogue clause is backend-shaped. Both give the verbatim line
            // in quotes + a hard "no other words" rule (else the model pads a
            // long shot with invented speech — the T4 gen-demo bug). The
            // audio-ref path (Seedance / LTX-a2v) also points at @Audio1 (the
            // uploaded voice); native-audio backends (Grok/Veo) generate the
            // voice from the line itself and must NOT reference @Audio1.
            let clause = if r.dialogue_overlay && r.backend.caps().audio_ref {
                format!(
                    "{} @Image{} says exactly: \"{}\". Use @Audio1 as the exact voice performance; lip-sync must follow this line verbatim and speak no other words; locked framing, minimal head movement",
                    display(e),
                    e.image_n,
                    d.text.trim(),
                )
            } else {
                format!(
                    "{} @Image{} says in {} exactly: \"{}\", speaking this line verbatim and no other words, lips moving clearly, locked framing, minimal head movement",
                    display(e),
                    e.image_n,
                    lang_name(d.lang),
                    d.text.trim(),
                )
            };
            sentence(&mut out, &clause);
        }
        if let Some(t) = &d.trailing {
            sentence(&mut out, t);
        }
    }

    if super::resolve::mode_of(r) == Mode::FirstFrame {
        sentence(&mut out, "Start exactly on the given first frame");
    }
    if r.dialogue.is_some() {
        sentence(
            &mut out,
            "No other people, no text overlays, no extra or invented dialogue",
        );
    } else {
        sentence(&mut out, "No text overlays");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::compile_phase1;
    use super::super::phase1::AssetRequest;
    use super::*;

    // Pinned to Seedance + overlay (the audio-ref / TTS path these tests
    // exercise); Grok native is the default and has its own tests below.
    const SRC: &str = "film \"t\" {\nbackend: seedance\ndialogue_sync: overlay\n}\n\
                       char $แอน = @./anny.png voice:th-female-warm desc:\"หญิงสาวผมยาว\"\n\
                       char $โทนี่ = @./tony.png voice:th-male-low desc:\"ชายหนุ่มไว้เครา\"\n\
                       scene $ห้อง = @./room.png time:day\n\
                       shot 1 (dialogue) {\n\
                       $แอน นั่งที่โต๊ะใน $ห้อง, $โทนี่ นั่งตรงข้าม\n\
                       camera: two-shot, static\n\
                       $แอน say \"คุณหายไปไหนมาตั้งแต่เช้า\"\n\
                       @duration: 6\n\
                       }\n";

    fn assets_for(p1: &Phase1Result, tts_ms: u64) -> Vec<ResolvedAsset> {
        p1.asset_requests
            .iter()
            .map(|r| ResolvedAsset {
                id: r.id().to_string(),
                url: format!("mock://{}", r.id()),
                duration_ms: matches!(r, AssetRequest::Tts { .. }).then_some(tts_ms),
            })
            .collect()
    }

    #[test]
    fn dialogue_payload_shape() {
        let p1 = compile_phase1(SRC);
        assert!(!p1.has_errors(), "{:?}", p1.errors);
        let r = compile_phase2(&p1, &assets_for(&p1, 4920));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let p = &r.payloads[0].payload;
        assert_eq!(p["model"], "bytedance/seedance-2");
        let input = &p["input"];
        assert_eq!(input["nsfw_checker"], true);
        assert_eq!(input["generate_audio"], true);
        assert_eq!(input["duration"], 6);
        assert_eq!(input["reference_image_urls"].as_array().unwrap().len(), 3);
        assert_eq!(input["reference_audio_urls"].as_array().unwrap().len(), 1);
        let prompt = input["prompt"].as_str().unwrap();
        assert!(prompt.contains("หญิงสาวผมยาว @Image1"), "{prompt}");
        assert!(prompt.contains("ชายหนุ่มไว้เครา @Image2"), "{prompt}");
        assert!(prompt.contains("ห้อง @Image3"), "{prompt}");
        assert!(
            prompt.contains("says exactly: \"คุณหายไปไหนมาตั้งแต่เช้า\""),
            "{prompt}"
        );
        assert!(
            prompt.contains("verbatim and speak no other words"),
            "{prompt}"
        );
        assert!(prompt.contains("Natural daylight"), "{prompt}");
        let subj = prompt.find("@Image1").unwrap();
        let cam = prompt.find("two-shot").unwrap();
        let neg = prompt.find("No other people").unwrap();
        assert!(subj < cam && cam < neg, "section order broken: {prompt}");
    }

    #[test]
    fn audio_overrun_names_exact_minimum() {
        let p1 = compile_phase1(SRC);
        let r = compile_phase2(&p1, &assets_for(&p1, 7900));
        let e = r
            .errors
            .iter()
            .find(|e| e.code == "E_AUDIO_OVERRUN")
            .expect("overrun");
        assert!(e.message.contains("≥9s"), "{}", e.message);
        assert!(r.payloads.is_empty());
    }

    #[test]
    fn match_cut_uses_first_frame_and_note() {
        let src =
            "film \"t\" {\nbackend: seedance\n}\nshot 1 {\nwide hallway\n}\nshot 2 {\n@match_cut: 1\nthe same hallway, now empty\n}\n";
        let p1 = compile_phase1(src);
        assert!(!p1.has_errors(), "{:?}", p1.errors);
        let r = compile_phase2(&p1, &assets_for(&p1, 0));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let input = &r.payloads[1].payload["input"];
        assert!(input["first_frame_url"]
            .as_str()
            .unwrap()
            .starts_with("mock://frame-"));
        assert!(input.get("reference_image_urls").is_none());
        let prompt = input["prompt"].as_str().unwrap();
        assert!(
            prompt.contains("Start exactly on the given first frame"),
            "{prompt}"
        );
        assert_eq!(r.payloads[1].depends_on.as_deref(), Some("1"));
    }

    #[test]
    fn beats_and_ambient_render_in_prompt() {
        let src = "film \"t\" {\nbackend: seedance\n}\nchar $a = @./a.png desc:\"woman\"\n\
                   shot 1 {\n$a ลุกจากเก้าอี้\n[0-3] wide static shot\n[3-6] camera: push-in to medium\n\
                   ambient: rain against the windows\n@duration: 6\n}\n";
        let p1 = compile_phase1(src);
        assert!(!p1.has_errors(), "{:?}", p1.errors);
        let r = compile_phase2(&p1, &assets_for(&p1, 0));
        let prompt = r.payloads[0].payload["input"]["prompt"].as_str().unwrap();
        assert!(
            prompt.contains("[00:00-00:03] wide static shot"),
            "{prompt}"
        );
        assert!(
            prompt.contains("[00:03-00:06] push-in to medium"),
            "{prompt}"
        );
        assert!(prompt.contains("Ambient sound: rain"), "{prompt}");
        assert_eq!(r.payloads[0].payload["input"]["generate_audio"], true);
    }

    #[test]
    fn variant_mention_uses_base_descriptor() {
        let src = "char $แอน = @./a.png desc:\"หญิงสาวผมยาว\"\nvariant $แอน#เปียก = @./a_wet.png\n\
                   shot 1 {\n$แอน#เปียก ยืนกลางฝน\n}\n";
        let p1 = compile_phase1(src);
        assert!(!p1.has_errors(), "{:?}", p1.errors);
        let r = compile_phase2(&p1, &assets_for(&p1, 0));
        let prompt = r.payloads[0].payload["input"]["prompt"].as_str().unwrap();
        assert!(prompt.contains("หญิงสาวผมยาว @Image1"), "{prompt}");
        assert!(!prompt.contains('#'), "{prompt}");
    }

    #[test]
    fn seed_travels_as_metadata_not_payload() {
        let p1 = compile_phase1("shot 1 {\n@seed: 42\nempty beach\n}\n");
        let r = compile_phase2(&p1, &[]);
        assert_eq!(r.payloads[0].seed, Some(42));
        assert!(r.payloads[0].payload["input"].get("seed").is_none());
    }

    #[test]
    fn phase2_refuses_on_phase1_errors() {
        // audit: never hand the harness payloads for a broken film
        let p1 = compile_phase1("shot 1 {\n@duration: 99\nx\n}\n");
        assert!(p1.has_errors());
        let r = compile_phase2(&p1, &[]);
        assert!(r.payloads.is_empty());
        assert_eq!(r.errors[0].code, "E_PHASE1_ERRORS");
    }

    #[test]
    fn grok_is_default_and_routes_i2v_vs_t2v() {
        // text-only → t2v
        let p1 = compile_phase1("shot 1 {\nwaves on an empty beach\n}\n");
        let r = compile_phase2(&p1, &assets_for(&p1, 0));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.payloads[0].payload["model"], "grok-imagine/text-to-video");
        assert!(r.payloads[0].payload["input"].get("image_urls").is_none());

        // with a character reference → i2v carrying image_urls
        let p1 = compile_phase1("char $a = @./a.png desc:\"a woman\"\nshot 1 {\n$a walks in\n}\n");
        let r = compile_phase2(&p1, &assets_for(&p1, 0));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(
            r.payloads[0].payload["model"],
            "grok-imagine/image-to-video"
        );
        assert_eq!(
            r.payloads[0].payload["input"]["image_urls"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn dialogue_is_native_by_default_even_on_seedance() {
        // Default (no dialogue_sync): NO TTS on any backend — the backend's
        // own audio is used. Overlay is opt-in.
        let src = "film \"t\" {\nbackend: seedance\n}\n\
                   char $a = @./a.png voice:th-female-warm desc:\"หญิงสาว\"\n\
                   shot 1 (dialogue) {\n$a นั่ง\n$a say \"สวัสดีค่ะ\"\n@duration: 4\n}\n";
        let p1 = compile_phase1(src);
        assert!(!p1.has_errors(), "{:?}", p1.errors);
        assert!(
            !p1.asset_requests
                .iter()
                .any(|r| matches!(r, AssetRequest::Tts { .. })),
            "native default must not synthesize TTS"
        );
        let r = compile_phase2(&p1, &assets_for(&p1, 0));
        let input = &r.payloads[0].payload["input"];
        assert!(input.get("reference_audio_urls").is_none());
        let prompt = input["prompt"].as_str().unwrap();
        assert!(!prompt.contains("@Audio1"), "{prompt}");
        assert!(prompt.contains("says in Thai exactly"), "{prompt}");

        // Opt into overlay → TTS asset + @Audio1 reference clause.
        let overlay = src.replace(
            "backend: seedance",
            "backend: seedance\ndialogue_sync: overlay",
        );
        let p1o = compile_phase1(&overlay);
        assert!(
            p1o.asset_requests
                .iter()
                .any(|r| matches!(r, AssetRequest::Tts { .. })),
            "overlay must synthesize TTS"
        );
        let ro = compile_phase2(&p1o, &assets_for(&p1o, 2000));
        let po = ro.payloads[0].payload["input"]["prompt"].as_str().unwrap();
        assert!(po.contains("Use @Audio1"), "{po}");
    }

    #[test]
    fn lipsync_mode_emits_tts_but_keeps_native_prompt() {
        // lipsync: TTS synthesized (for the post lip-sync pass), but the video
        // prompt stays the native clause (mouth moves) — NOT the @Audio1
        // audio-ref clause (that's overlay+audio-ref only).
        let src = "film \"t\" {\ndialogue_sync: lipsync\n}\n\
                   char $a = @./a.png voice:th-female-warm desc:\"หญิงสาว\"\n\
                   shot 1 (dialogue) {\n$a นั่ง\n$a say \"สวัสดีค่ะ\"\n@duration: 4\n}\n";
        let p1 = compile_phase1(src);
        assert!(!p1.has_errors(), "{:?}", p1.errors);
        assert!(
            p1.asset_requests
                .iter()
                .any(|r| matches!(r, AssetRequest::Tts { .. })),
            "lipsync must synthesize TTS"
        );
        let r = compile_phase2(&p1, &assets_for(&p1, 2000));
        let prompt = r.payloads[0].payload["input"]["prompt"].as_str().unwrap();
        assert!(prompt.contains("says in Thai exactly"), "{prompt}");
        assert!(!prompt.contains("@Audio1"), "{prompt}");
    }

    #[test]
    fn grok_dialogue_is_native_no_tts_no_audio_ref() {
        let src = "char $a = @./a.png voice:th-female-warm desc:\"หญิงสาว\"\n\
                   shot 1 (dialogue) {\n$a นั่งที่โต๊ะ\n$a say \"สวัสดีค่ะ\"\n@duration: 4\n}\n";
        let p1 = compile_phase1(src);
        assert!(!p1.has_errors(), "{:?}", p1.errors);
        // native backend emits no TTS asset
        assert!(
            !p1.asset_requests
                .iter()
                .any(|r| matches!(r, AssetRequest::Tts { .. })),
            "grok must not emit TTS"
        );
        let r = compile_phase2(&p1, &assets_for(&p1, 0));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let input = &r.payloads[0].payload["input"];
        assert!(input.get("reference_audio_urls").is_none());
        let prompt = input["prompt"].as_str().unwrap();
        // dialogue in the prompt, native clause (no @Audio1)
        assert!(
            prompt.contains("says in Thai exactly: \"สวัสดีค่ะ\""),
            "{prompt}"
        );
        assert!(!prompt.contains("@Audio1"), "{prompt}");
    }

    #[test]
    fn all_backends_compile_and_carry_backend_tag() {
        // Every backend now has codegen; the payload carries its backend so
        // the harness can gate/dispatch. Veo → REFERENCE_2_VIDEO with refs.
        let p1 = compile_phase1(
            "char $a = @./a.png desc:\"a hero\"\nshot 1 {\n@backend: veo\n$a stands in a forest\n}\n",
        );
        assert!(!p1.has_errors(), "{:?}", p1.errors);
        let r = compile_phase2(&p1, &assets_for(&p1, 0));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.payloads[0].backend, super::super::backend::BackendId::Veo);
        assert_eq!(r.payloads[0].payload["model"], "veo3_fast");
        assert_eq!(r.payloads[0].payload["generationType"], "REFERENCE_2_VIDEO");
        assert_eq!(
            r.payloads[0].payload["imageUrls"].as_array().unwrap().len(),
            1
        );
    }

    #[test]
    fn ltx_and_happyhorse_payload_shapes() {
        // LTX i2v: native pixel resolution + fps + generate_audio + image_uri.
        let p1 = compile_phase1("char $a = @./a.png desc:\"a woman\"\nshot 1 {\n@backend: ltx\n@resolution: 1080p\n$a by a window\n}\n");
        let r = compile_phase2(&p1, &assets_for(&p1, 0));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let i = &r.payloads[0].payload;
        assert_eq!(i["model"], "ltx-2-3-fast");
        assert_eq!(i["resolution"], "1920x1080");
        assert_eq!(i["fps"], 25);
        assert!(i["image_uri"].is_string());

        // Happy Horse i2v: DashScope shape (ratio, uppercase P, media).
        let p1 = compile_phase1(
            "char $a = @./a.png desc:\"a woman\"\nshot 1 {\n@backend: happyhorse\n$a smiles\n}\n",
        );
        let r = compile_phase2(&p1, &assets_for(&p1, 0));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let p = &r.payloads[0].payload;
        assert_eq!(p["model"], "happyhorse-1.0-i2v");
        assert_eq!(p["parameters"]["resolution"], "720P");
        assert_eq!(p["input"]["media"][0]["type"], "first_frame");
    }

    #[test]
    fn property_values_expand_refs() {
        let src = "char $แอน = @./a.png desc:\"หญิงสาว\"\nshot 1 {\n$แอน ยืนนิ่ง\ncamera: slow push-in on $แอน\n}\n";
        let p1 = compile_phase1(src);
        assert!(!p1.has_errors(), "{:?}", p1.errors);
        let r = compile_phase2(&p1, &assets_for(&p1, 0));
        let prompt = r.payloads[0].payload["input"]["prompt"].as_str().unwrap();
        assert!(prompt.contains("push-in on @Image1"), "{prompt}");
        assert!(!prompt.contains('$'), "{prompt}");
    }

    #[test]
    fn text_only_shot_has_no_ref_arrays() {
        let p1 = compile_phase1(
            "film \"t\" {\nbackend: seedance\n}\nshot 1 {\nwaves on an empty beach\n@audio: off\n}\n",
        );
        let r = compile_phase2(&p1, &[]);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let input = &r.payloads[0].payload["input"];
        assert!(input.get("reference_image_urls").is_none());
        assert_eq!(input["generate_audio"], false);
    }
}
