//! Perfis de desempenho da IA a partir do hardware medido.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AiProfile {
    Economy,
    Balanced,
    Max,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HardwareInput {
    pub cores: u32,
    #[ts(type = "number")]
    pub ram_mb: u64,
    pub cuda_devices: u32,
    #[ts(type = "number | null")]
    pub vram_mb: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProfileSettings {
    pub profile: AiProfile,
    pub whisper_model: String,
    /// Amostragem visual (s).
    pub visual_interval: f32,
    /// Sugestão de LLM no Ollama (verificar disponibilidade e licença antes de baixar).
    pub llm_hint: String,
    pub reason: String,
}

/// Escolha automática (o usuário pode trocar manualmente).
pub fn recommend(hw: &HardwareInput) -> AiProfile {
    let vram = hw.vram_mb.unwrap_or(0);
    if hw.cuda_devices > 0 && vram >= 8000 && hw.ram_mb >= 16000 {
        AiProfile::Max
    } else if hw.ram_mb >= 12000 && hw.cores >= 6 || hw.cuda_devices > 0 && vram >= 4000 {
        AiProfile::Balanced
    } else {
        AiProfile::Economy
    }
}

pub fn settings(profile: AiProfile, hw: &HardwareInput) -> ProfileSettings {
    let gpu = hw.cuda_devices > 0;
    let (whisper, interval, llm) = match profile {
        AiProfile::Economy => ("base", 2.0, "modelo de ~4B parâmetros quantizado"),
        AiProfile::Balanced => (if gpu { "medium" } else { "small" }, 1.0, "modelo de 4B–9B parâmetros"),
        AiProfile::Max => (if gpu { "large-v3" } else { "medium" }, 0.5, "o maior modelo que couber na VRAM"),
    };
    let reason = format!(
        "{} núcleos, {:.0} GB de RAM{}",
        hw.cores,
        hw.ram_mb as f64 / 1024.0,
        if gpu { format!(", GPU CUDA com {:.0} GB", hw.vram_mb.unwrap_or(0) as f64 / 1024.0) } else { ", sem GPU CUDA".into() }
    );
    ProfileSettings { profile, whisper_model: whisper.into(), visual_interval: interval, llm_hint: llm.into(), reason }
}

/// Aviso antes de baixar um modelo grande demais para a máquina.
pub fn model_fits(model_ram_mb: u32, hw: &HardwareInput) -> Option<String> {
    let budget = hw.ram_mb / 2;
    (model_ram_mb as u64 > budget)
        .then(|| format!("Este modelo usa cerca de {model_ram_mb} MB de memória; este computador tem {} MB no total. Ele pode ficar muito lento.", hw.ram_mb))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_follow_hardware() {
        let small = HardwareInput { cores: 2, ram_mb: 8000, cuda_devices: 0, vram_mb: None };
        let mid = HardwareInput { cores: 8, ram_mb: 16000, cuda_devices: 0, vram_mb: None };
        let big = HardwareInput { cores: 16, ram_mb: 32000, cuda_devices: 1, vram_mb: Some(12000) };
        assert_eq!(recommend(&small), AiProfile::Economy);
        assert_eq!(recommend(&mid), AiProfile::Balanced);
        assert_eq!(recommend(&big), AiProfile::Max);
        assert_eq!(settings(AiProfile::Balanced, &mid).whisper_model, "small");
        assert_eq!(settings(AiProfile::Max, &big).whisper_model, "large-v3");
        assert!(model_fits(4800, &small).is_some());
        assert!(model_fits(1200, &mid).is_none());
    }
}
