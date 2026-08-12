//! AST for the Tier-2 film level: optional `film{}` header,
//! declarations (entities + audio assets + variants/views), and
//! sequences of shots. A file may omit `film{}`/`sequence{}` and
//! contain bare shots — they land in an implicit default sequence, so
//! every Tier-1 flat script stays valid input. Line kinds are
//! syntactic and unambiguous (spec §2.5) — classification never
//! depends on declared names ("prose stays prose").

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub header: FilmHeader,
    pub declarations: Vec<Declaration>,
    pub sequences: Vec<Sequence>,
    /// Source contains Thai → diagnostics come back in Thai.
    pub thai: bool,
}

/// `film "title" { … }` — outermost layer of the style cascade and the
/// film-wide defaults. All fields optional; `Default` is the implicit
/// header for scripts without one.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FilmHeader {
    pub title: Option<String>,
    pub aspect: Option<String>,
    pub resolution: Option<String>,
    pub style: Option<String>,
    pub lighting: Option<String>,
    pub genre: Option<String>,
    pub fps_look: Option<String>,
    pub audio_default: Option<bool>,
    /// `native | post` — `post` parses but is deferred in v1.
    pub dialogue_sync: Option<String>,
    pub subtitle_default: Option<bool>,
    /// `subtitle_burn: on` hardcodes styled captions into the picture;
    /// default (off) soft-muxes the SRT as a toggle-able track instead.
    pub subtitle_burn: Option<bool>,
    /// Film-wide default video backend (`grok | ltx | seedance | veo |
    /// happyhorse`); per-shot `@backend:` overrides. `None` = the compiled
    /// default (Grok).
    pub backend: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntityKind {
    Char,
    Scene,
    Prop,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceSpec {
    Registry(String),
    /// `voice:@./sample.mp3` — valid grammar, deferred capability
    /// (rejected in validation with `E_UNSUPPORTED_V1`).
    CloneSample(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Declaration {
    Entity(EntityDecl),
    /// `variant $base#tag = @./img.png` (char look) or
    /// `view $base#tag = @./img.png` (scene camera angle) — the image
    /// swaps in at the base's slot; identity fields inherit.
    Variant {
        base: String,
        tag: String,
        image_path: String,
        is_view: bool,
        line: usize,
    },
    /// `music $x = @./f.mp3` / `sfx $y = @./f.wav` — assembly-layer
    /// audio, never sent to the video model.
    Audio {
        is_music: bool,
        handle: String,
        path: String,
        line: usize,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntityDecl {
    pub kind: EntityKind,
    /// NFC-normalized handle, without the `$` sigil.
    pub handle: String,
    pub alias: Option<String>,
    pub image_path: String,
    pub voice: Option<VoiceSpec>,
    pub desc: Option<String>,
    /// Scene time-of-day (`dawn|day|golden_hour|dusk|night`), folded
    /// into lighting at codegen when the shot has no `lighting:`.
    pub time: Option<String>,
    pub line: usize,
}

/// `sequence "title" { … }` — narrative unit sharing scene/style/music.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sequence {
    pub title: Option<String>,
    /// Sequence-level backdrop: `$scene` or `$scene#view`.
    pub scene: Option<String>,
    pub style: Option<String>,
    pub lighting: Option<String>,
    pub aspect: Option<String>,
    pub resolution: Option<String>,
    pub audio_default: Option<bool>,
    pub music: Option<MusicCue>,
    pub shots: Vec<Shot>,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MusicCue {
    pub handle: String,
    pub volume: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShotType {
    Dialogue,
    Action,
    Establishing,
    /// Close detail shot: performance slots closed, 4s default.
    Insert,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shot {
    pub id: String,
    /// `None` = untyped; resolves to `Dialogue` when a say-line is
    /// present, else `Action`.
    pub shot_type: Option<ShotType>,
    pub lines: Vec<ShotLine>,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShotLine {
    Action {
        text: String,
        refs: Vec<String>,
        line: usize,
    },
    Property {
        key: String,
        value: String,
        line: usize,
    },
    Directive {
        key: String,
        value: String,
        line: usize,
    },
    Dialogue {
        speaker: String,
        text: String,
        trailing: Option<String>,
        line: usize,
    },
    /// `[t1-t2] <prose or property>` — time-scoped beat.
    Beat {
        t1: u32,
        t2: u32,
        content: Box<ShotLine>,
        line: usize,
    },
}

pub(crate) const PROPERTY_KEYS: &[&str] = &[
    "camera",
    "expression",
    "gesture",
    "voice_tone",
    "mood",
    "lighting",
    "style",
    "scene",
    "ambient",
    "sfx",
];

pub(crate) const DIRECTIVE_KEYS: &[&str] = &[
    "continue_from",
    "match_cut",
    "duration",
    "resolution",
    "aspect",
    "model",
    "audio",
    "seed",
    "takes",
    "hold",
    "transition",
    "subtitle",
    "subtitle_burn",
    "dialogue_sync",
    "backend",
];
