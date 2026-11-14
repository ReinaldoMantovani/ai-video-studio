//! Presets editoriais e perfis de exportação: arquivos JSON versionados em
//! `presets/`, nunca condicionais espalhados pelo código.

use avs_core::{ProjectSettings, Rational};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use ts_rs::TS;

pub const PRESET_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DurationTarget {
    pub min_s: Option<f32>,
    pub max_s: Option<f32>,
    /// Duração-alvo editorial (não confundir com o limite da plataforma).
    pub target_s: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CutRules {
    pub remove_silence: bool,
    pub min_silence_ms: u32,
    pub keep_padding_ms: u32,
    /// Não remover trechos silenciosos com ação visual (gameplay).
    pub preserve_silent_action: bool,
    pub remove_fillers: bool,
    pub max_shot_s: Option<f32>,
    pub rules: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AudioRules {
    pub loudness_lufs: f32,
    pub true_peak_db: f32,
    pub music_ducking_db: f32,
    pub noise_reduction: bool,
    pub voice_priority: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CaptionRules {
    pub enabled: bool,
    pub style: String,
    pub position: String,
    pub max_chars_per_line: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EffectRules {
    pub allowed: Vec<String>,
    pub intensity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TransitionRules {
    pub allowed: Vec<String>,
    pub default_duration_ms: u32,
    pub frequency: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReframeRules {
    pub mode: String,
    pub keep_hud: bool,
    pub layouts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NarrativeRules {
    pub mode: String,
    pub hook_required: bool,
    pub rules: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SafeArea {
    pub top_pct: f32,
    pub bottom_pct: f32,
    pub left_pct: f32,
    pub right_pct: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SafetyRules {
    pub safe_area: SafeArea,
    pub rules: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EditorialPreset {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub platforms: Vec<String>,
    pub aspect: String,
    pub resolution: Resolution,
    /// `None` = preservar o FPS da origem.
    pub fps: Option<Rational>,
    pub duration: DurationTarget,
    pub pacing: String,
    pub cuts: CutRules,
    pub audio: AudioRules,
    pub captions: CaptionRules,
    pub effects: EffectRules,
    pub transitions: TransitionRules,
    pub reframe: ReframeRules,
    pub narrative: NarrativeRules,
    pub safety: SafetyRules,
    pub export_profile: String,
    /// Pesos iniciais da pontuação editorial (heurística calibrável).
    pub scoring_weights: BTreeMap<String, f32>,
    #[serde(default)]
    pub builtin: bool,
}

impl EditorialPreset {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version > PRESET_SCHEMA_VERSION {
            return Err(format!("{}: schema_version {} não suportado", self.id, self.schema_version));
        }
        if self.id.is_empty() || !self.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return Err(format!("id inválido: '{}'", self.id));
        }
        if !self.resolution.width.is_multiple_of(2) || !self.resolution.height.is_multiple_of(2) || self.resolution.width < 16 {
            return Err(format!("{}: resolução inválida", self.id));
        }
        if let Some(m) = self.duration.max_s {
            if m <= 0.0 {
                return Err(format!("{}: duração máxima inválida", self.id));
            }
        }
        let sum: f32 = self.scoring_weights.values().sum();
        if !self.scoring_weights.is_empty() && (sum - 1.0).abs() > 0.01 {
            return Err(format!("{}: pesos de pontuação somam {sum:.2} (devem somar 1.0)", self.id));
        }
        Ok(())
    }

    /// Configurações de projeto derivadas do preset (FPS da origem quando `None`).
    pub fn project_settings(&self, source_fps: Option<Rational>) -> ProjectSettings {
        ProjectSettings {
            width: self.resolution.width,
            height: self.resolution.height,
            fps: self.fps.or(source_fps).unwrap_or(Rational::new(30, 1)),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExportProfile {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub container: avs_media::render::Container,
    /// Família do codec: h264 | hevc | vp9
    pub codec: String,
    /// `None` = resolução do projeto.
    pub resolution: Option<Resolution>,
    pub fps: Option<Rational>,
    pub quality: u8,
    pub speed: String,
    pub bitrate_kbps: Option<u32>,
    pub audio_bitrate_kbps: u32,
    pub loudness_lufs: Option<f32>,
    pub max_duration_s: Option<f32>,
    pub notes: String,
}

#[derive(Debug, Deserialize)]
struct ProfilesFile {
    schema_version: u32,
    profiles: Vec<ExportProfile>,
}

/// Carrega `*.json` de `dir/editorial`. Arquivos inválidos são reportados, não ignorados em silêncio.
pub fn load_editorial(dir: &Path) -> (Vec<EditorialPreset>, Vec<String>) {
    let mut out = vec![];
    let mut errors = vec![];
    let mut files: Vec<_> = std::fs::read_dir(dir.join("editorial")).map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    files.sort();
    for f in files {
        if f.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match std::fs::read(&f).map_err(|e| e.to_string()).and_then(|b| serde_json::from_slice::<EditorialPreset>(&b).map_err(|e| e.to_string())) {
            Ok(p) => match p.validate() {
                Ok(()) => out.push(p),
                Err(e) => errors.push(format!("{}: {e}", f.display())),
            },
            Err(e) => errors.push(format!("{}: {e}", f.display())),
        }
    }
    (out, errors)
}

pub fn load_export_profiles(dir: &Path) -> Result<Vec<ExportProfile>, String> {
    let b = std::fs::read(dir.join("export_profiles.json")).map_err(|e| format!("export_profiles.json: {e}"))?;
    let f: ProfilesFile = serde_json::from_slice(&b).map_err(|e| format!("export_profiles.json: {e}"))?;
    if f.schema_version > 1 {
        return Err("export_profiles.json: versão não suportada".into());
    }
    Ok(f.profiles)
}
