//! Contratos de dados do projeto. Todos os tipos são serializáveis (JSON),
//! possuem IDs estáveis e geram bindings TypeScript via `ts-rs`
//! (`cargo test -p avs-core export_bindings` → `apps/desktop/src/bindings`).

use crate::time::{Rational, Time, TimeRange};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use ts_rs::TS;
use uuid::Uuid;

pub const PROJECT_SCHEMA_VERSION: u32 = 1;

pub type Id = Uuid;

pub fn new_id() -> Id {
    Uuid::new_v4()
}

// ───────────────────────────── Projeto ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Project {
    pub schema_version: u32,
    pub id: Id,
    pub name: String,
    #[ts(type = "string")]
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[ts(type = "string")]
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub settings: ProjectSettings,
    pub assets: Vec<MediaAsset>,
    pub timeline: Timeline,
    /// Preset editorial ativo (ID de arquivo em `presets/`).
    pub preset_id: Option<String>,
}

impl Project {
    pub fn new(name: impl Into<String>, settings: ProjectSettings) -> Self {
        let now = chrono::Utc::now();
        Project {
            schema_version: PROJECT_SCHEMA_VERSION,
            id: new_id(),
            name: name.into(),
            created_at: now,
            updated_at: now,
            settings,
            assets: vec![],
            timeline: Timeline::with_default_tracks(),
            preset_id: None,
        }
    }

    pub fn asset(&self, id: Id) -> Option<&MediaAsset> {
        self.assets.iter().find(|a| a.id == id)
    }
    pub fn asset_mut(&mut self, id: Id) -> Option<&mut MediaAsset> {
        self.assets.iter_mut().find(|a| a.id == id)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectSettings {
    pub width: u32,
    pub height: u32,
    pub fps: Rational,
    pub sample_rate: u32,
    /// Cor de fundo do canvas (hex `#RRGGBB`).
    pub background: String,
}

impl Default for ProjectSettings {
    fn default() -> Self {
        ProjectSettings { width: 1920, height: 1080, fps: Rational::new(30, 1), sample_rate: 48_000, background: "#000000".into() }
    }
}

// ───────────────────────────── Mídia ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AssetKind {
    Video,
    Audio,
    Image,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
#[ts(export)]
pub enum AssetStatus {
    Processing,
    Ready,
    Missing,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct VideoInfo {
    pub codec: String,
    pub width: u32,
    pub height: u32,
    /// Taxa nominal (`r_frame_rate`).
    pub fps: Rational,
    /// Taxa média (`avg_frame_rate`).
    pub avg_fps: Option<Rational>,
    pub time_base: Option<Rational>,
    /// Frame rate variável detectado.
    pub vfr: bool,
    pub pix_fmt: Option<String>,
    pub bit_depth: Option<u32>,
    /// Rotação de exibição em graus (metadado de celulares).
    pub rotation: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AudioInfo {
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u32,
    pub stream_count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ThumbStrip {
    /// Imagem sprite horizontal com `count` miniaturas.
    pub path: PathBuf,
    pub count: u32,
    pub tile_width: u32,
    pub tile_height: u32,
    pub interval: Time,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MediaAsset {
    pub id: Id,
    pub kind: AssetKind,
    pub name: String,
    /// Referência ao arquivo original (nunca copiado nem alterado).
    pub path: PathBuf,
    /// Impressão digital para relocalizar arquivos movidos.
    pub fingerprint: String,
    #[ts(type = "number")]
    pub size_bytes: u64,
    pub duration: Time,
    pub container: String,
    pub video: Option<VideoInfo>,
    pub audio: Option<AudioInfo>,
    pub proxy_path: Option<PathBuf>,
    pub thumbs: Option<ThumbStrip>,
    pub waveform_path: Option<PathBuf>,
    pub poster_path: Option<PathBuf>,
    pub status: AssetStatus,
}

impl MediaAsset {
    pub fn has_video(&self) -> bool {
        self.video.is_some()
    }
    pub fn has_audio(&self) -> bool {
        self.audio.is_some()
    }
    /// Imagens não têm limite de duração na timeline.
    pub fn is_unbounded(&self) -> bool {
        self.kind == AssetKind::Image
    }
}

// ───────────────────────────── Timeline ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Timeline {
    /// Ordem = empilhamento: índice 0 fica no fundo.
    pub tracks: Vec<Track>,
    pub transitions: Vec<Transition>,
    pub markers: Vec<Marker>,
}

impl Timeline {
    pub fn with_default_tracks() -> Self {
        Timeline {
            tracks: vec![Track::new(TrackKind::Video, "Vídeo 1"), Track::new(TrackKind::Text, "Texto 1"), Track::new(TrackKind::Audio, "Áudio 1")],
            transitions: vec![],
            markers: vec![],
        }
    }

    /// Fim do último clipe de qualquer trilha.
    pub fn duration(&self) -> Time {
        self.tracks.iter().flat_map(|t| t.clips.iter()).map(|c| c.end()).max().unwrap_or(Time::ZERO)
    }

    pub fn track(&self, id: Id) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }
    pub fn track_mut(&mut self, id: Id) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }
    /// Localiza (índice da trilha, índice do clipe).
    pub fn locate_clip(&self, clip: Id) -> Option<(usize, usize)> {
        self.tracks.iter().enumerate().find_map(|(ti, t)| t.clips.iter().position(|c| c.id == clip).map(|ci| (ti, ci)))
    }
    pub fn clip(&self, clip: Id) -> Option<&Clip> {
        self.locate_clip(clip).map(|(t, c)| &self.tracks[t].clips[c])
    }
    pub fn transition_between(&self, a: Id, b: Id) -> Option<&Transition> {
        self.transitions.iter().find(|t| t.from_clip == a && t.to_clip == b)
    }
    pub fn sort_clips(&mut self) {
        for t in &mut self.tracks {
            t.clips.sort_by_key(|c| (c.start, c.id));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TrackKind {
    Video,
    Audio,
    Text,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Track {
    pub id: Id,
    pub kind: TrackKind,
    pub name: String,
    pub muted: bool,
    pub hidden: bool,
    pub locked: bool,
    /// Ganho da trilha (linear).
    pub volume: f32,
    /// Ordenados por `start`.
    pub clips: Vec<Clip>,
}

impl Track {
    pub fn new(kind: TrackKind, name: impl Into<String>) -> Self {
        Track { id: new_id(), kind, name: name.into(), muted: false, hidden: false, locked: false, volume: 1.0, clips: vec![] }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Clip {
    pub id: Id,
    pub name: String,
    /// Posição na timeline.
    pub start: Time,
    pub duration: Time,
    pub content: ClipContent,
    /// Ganho de áudio linear (1.0 = original).
    pub volume: f32,
    pub audio_muted: bool,
    pub fade_in: Time,
    pub fade_out: Time,
    pub opacity: f32,
    pub color: ColorAdjust,
    pub transform: Transform,
    /// Velocidade de reprodução (1 = normal). A mídia consumida é `duration × speed`.
    #[serde(default = "one")]
    pub speed: f32,
    /// Reproduz o trecho de trás para frente.
    #[serde(default)]
    pub reverse: bool,
    /// Congela o frame em `source_in` durante toda a duração (sem áudio).
    #[serde(default)]
    pub freeze: bool,
    #[serde(default)]
    pub effects: VideoEffects,
    #[serde(default)]
    pub audio_fx: AudioEffects,
    #[serde(default)]
    pub keyframes: Keyframes,
}

fn one() -> f32 {
    1.0
}

/// Efeitos de imagem por clipe (valores neutros = efeito desligado).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct VideoEffects {
    /// Desfoque gaussiano (sigma em px a 1080p; 0 = desligado).
    pub blur: f32,
    /// Nitidez (0..2; 0 = desligado).
    pub sharpen: f32,
    /// Vinheta (0..1; 0 = desligada).
    pub vignette: f32,
    /// Temperatura de cor em Kelvin (6500 = neutro).
    pub temperature: f32,
    /// LUT 3D (.cube/.3dl) aplicada após a correção de cor.
    pub lut: Option<PathBuf>,
}

impl Default for VideoEffects {
    fn default() -> Self {
        VideoEffects { blur: 0.0, sharpen: 0.0, vignette: 0.0, temperature: 6500.0, lut: None }
    }
}

impl VideoEffects {
    pub fn is_neutral(&self) -> bool {
        *self == VideoEffects::default()
    }
}

/// Tratamento de áudio por clipe.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AudioEffects {
    /// Filtro passa-altas (remove ronco/vento abaixo de ~80 Hz).
    pub highpass: bool,
    /// Redução de ruído de fundo (afftdn).
    pub denoise: bool,
    /// Compressor de voz.
    pub compressor: bool,
    /// Equalizador de 3 bandas em dB (graves 100 Hz, médios 1 kHz, agudos 8 kHz).
    pub eq_low_db: f32,
    pub eq_mid_db: f32,
    pub eq_high_db: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Easing {
    Linear,
    EaseInOut,
    Hold,
}

/// Keyframe de uma propriedade. `at` é relativo ao início do clipe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Keyframe {
    pub at: Time,
    pub value: f32,
    pub easing: Easing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum KeyframeProperty {
    Scale,
    PositionX,
    PositionY,
    Volume,
}

/// Animações por propriedade (vazio = valor estático do clipe).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Keyframes {
    pub scale: Vec<Keyframe>,
    pub position_x: Vec<Keyframe>,
    pub position_y: Vec<Keyframe>,
    pub volume: Vec<Keyframe>,
}

impl Keyframes {
    pub fn get(&self, p: KeyframeProperty) -> &Vec<Keyframe> {
        match p {
            KeyframeProperty::Scale => &self.scale,
            KeyframeProperty::PositionX => &self.position_x,
            KeyframeProperty::PositionY => &self.position_y,
            KeyframeProperty::Volume => &self.volume,
        }
    }
    pub fn get_mut(&mut self, p: KeyframeProperty) -> &mut Vec<Keyframe> {
        match p {
            KeyframeProperty::Scale => &mut self.scale,
            KeyframeProperty::PositionX => &mut self.position_x,
            KeyframeProperty::PositionY => &mut self.position_y,
            KeyframeProperty::Volume => &mut self.volume,
        }
    }
    pub fn has_transform(&self) -> bool {
        !self.scale.is_empty() || !self.position_x.is_empty() || !self.position_y.is_empty()
    }
    pub fn all_mut(&mut self) -> [&mut Vec<Keyframe>; 4] {
        [&mut self.scale, &mut self.position_x, &mut self.position_y, &mut self.volume]
    }
}

/// Valor interpolado de uma curva de keyframes em `t` (relativo ao clipe).
pub fn interpolate(kfs: &[Keyframe], t: Time, default: f32) -> f32 {
    let Some(first) = kfs.first() else { return default };
    if t <= first.at {
        return first.value;
    }
    for w in kfs.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        if t < b.at {
            let u = ((t - a.at).flicks() as f64 / (b.at - a.at).flicks().max(1) as f64) as f32;
            let u = match a.easing {
                Easing::Linear => u,
                Easing::EaseInOut => u * u * (3.0 - 2.0 * u),
                Easing::Hold => 0.0,
            };
            return a.value + (b.value - a.value) * u;
        }
    }
    kfs.last().unwrap().value
}

/// Multiplica um tempo por um fator (velocidade), arredondando ao flick.
pub fn scale_time(t: Time, k: f32) -> Time {
    Time::from_flicks((t.flicks() as f64 * k as f64).round() as i64)
}

impl Clip {
    pub fn new(start: Time, duration: Time, content: ClipContent, name: impl Into<String>) -> Self {
        Clip {
            id: new_id(),
            name: name.into(),
            start,
            duration,
            content,
            volume: 1.0,
            audio_muted: false,
            fade_in: Time::ZERO,
            fade_out: Time::ZERO,
            opacity: 1.0,
            color: ColorAdjust::default(),
            transform: Transform::default(),
            speed: 1.0,
            reverse: false,
            freeze: false,
            effects: VideoEffects::default(),
            audio_fx: AudioEffects::default(),
            keyframes: Keyframes::default(),
        }
    }
    /// Quanto da mídia de origem o clipe consome (congelado: um instante).
    pub fn source_span(&self) -> Time {
        if self.freeze {
            Time::ZERO
        } else {
            scale_time(self.duration, self.speed)
        }
    }
    pub fn end(&self) -> Time {
        self.start + self.duration
    }
    pub fn range(&self) -> TimeRange {
        TimeRange::new(self.start, self.end())
    }
    pub fn asset_id(&self) -> Option<Id> {
        match &self.content {
            ClipContent::Media { asset_id, .. } => Some(*asset_id),
            _ => None,
        }
    }
    pub fn source_in(&self) -> Time {
        match &self.content {
            ClipContent::Media { source_in, .. } => *source_in,
            _ => Time::ZERO,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum ClipContent {
    /// Trecho de um asset, iniciando em `source_in` no arquivo de origem.
    Media {
        asset_id: Id,
        source_in: Time,
    },
    Text {
        text: String,
        style: TextStyle,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TextStyle {
    /// Nome de família; resolvido para arquivo de fonte pelo renderizador.
    pub font: String,
    /// Tamanho em pixels relativo à altura de 1080 (escala com a resolução).
    pub size: f32,
    pub color: String,
    pub outline_color: String,
    pub outline_width: f32,
    pub background: Option<String>,
    /// Centro do texto em coordenadas normalizadas 0..1.
    pub x: f32,
    pub y: f32,
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle {
            font: "Inter".into(),
            size: 72.0,
            color: "#FFFFFF".into(),
            outline_color: "#000000".into(),
            outline_width: 3.0,
            background: None,
            x: 0.5,
            y: 0.85,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ColorAdjust {
    /// -1..1 (0 = neutro)
    pub brightness: f32,
    /// 0..3 (1 = neutro)
    pub contrast: f32,
    /// 0..3 (1 = neutro)
    pub saturation: f32,
}

impl Default for ColorAdjust {
    fn default() -> Self {
        ColorAdjust { brightness: 0.0, contrast: 1.0, saturation: 1.0 }
    }
}

impl ColorAdjust {
    pub fn is_neutral(&self) -> bool {
        self.brightness == 0.0 && self.contrast == 1.0 && self.saturation == 1.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum FitMode {
    /// Cabe inteiro, com barras (letterbox).
    Fit,
    /// Preenche o quadro, recortando o excesso.
    Fill,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Transform {
    pub fit: FitMode,
    /// Escala adicional (1 = sem zoom).
    pub scale: f32,
    /// Deslocamento normalizado do centro (-1..1 = meia largura/altura).
    pub x: f32,
    pub y: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Transform { fit: FitMode::Fit, scale: 1.0, x: 0.0, y: 0.0 }
    }
}

impl Transform {
    pub fn is_identity(&self) -> bool {
        self.scale == 1.0 && self.x == 0.0 && self.y == 0.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TransitionKind {
    Crossfade,
    FadeBlack,
    WipeLeft,
    SlideLeft,
    Dissolve,
}

impl TransitionKind {
    /// Nome da transição no filtro `xfade` do FFmpeg.
    pub fn xfade_name(self) -> &'static str {
        match self {
            TransitionKind::Crossfade => "fade",
            TransitionKind::FadeBlack => "fadeblack",
            TransitionKind::WipeLeft => "wipeleft",
            TransitionKind::SlideLeft => "slideleft",
            TransitionKind::Dissolve => "dissolve",
        }
    }
}

/// Transição entre dois clipes consecutivos da mesma trilha.
/// Os clipes se sobrepõem exatamente por `duration`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Transition {
    pub id: Id,
    pub track_id: Id,
    pub from_clip: Id,
    pub to_clip: Id,
    pub kind: TransitionKind,
    pub duration: Time,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Marker {
    pub id: Id,
    pub at: Time,
    pub label: String,
    pub color: String,
}
