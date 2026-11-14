//! Serviços de IA local: Gerenciador de IA, análise de mídia e ações de
//! edição automática (remover pausas, cenas, legendas da transcrição).

use crate::*;
use avs_ai::analysis::{self, AnalysisOptions, AnalysisResult};
use avs_ai::autoedit::{self, PausePlan, PauseRules};
use avs_ai::ollama::{Ollama, OllamaStatus};
use avs_ai::profile::{self, AiProfile, HardwareInput, ProfileSettings};
use avs_ai::runtime::{self, AiComponent};

const SETTINGS_KEY: &str = "ai.settings";

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AiSettings {
    /// `None` = automático pelo hardware.
    pub profile: Option<AiProfile>,
    /// Idioma da transcrição (`None` = detectar).
    pub language: Option<String>,
    /// Substitui o modelo Whisper do perfil.
    pub whisper_model: Option<String>,
    pub ollama_url: String,
    pub ollama_model: Option<String>,
}

impl Default for AiSettings {
    fn default() -> Self {
        AiSettings { profile: None, language: None, whisper_model: None, ollama_url: avs_ai::ollama::DEFAULT_URL.into(), ollama_model: None }
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct AiStatus {
    pub system_python: Option<String>,
    pub components: Vec<AiComponent>,
    pub ollama: OllamaStatus,
    pub hardware: HardwareInput,
    pub recommended: AiProfile,
    pub settings: AiSettings,
    pub effective: ProfileSettings,
    pub library_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct AssetAnalysis {
    pub asset_id: Id,
    pub result: Option<AnalysisResult>,
}

impl Studio {
    pub fn ai_settings(&self) -> AiSettings {
        self.0.db.lock().unwrap().get_setting::<AiSettings>(SETTINGS_KEY).ok().flatten().unwrap_or_default()
    }

    pub fn set_ai_settings(&self, s: AiSettings) -> AppResult<AiSettings> {
        if !(s.ollama_url.starts_with("http://127.0.0.1") || s.ollama_url.starts_with("http://localhost") || s.ollama_url.starts_with("http://[::1]")) {
            // Mídia e textos não saem da máquina sem uma decisão explícita.
            return Err(AppError::new("ollama_url", "por segurança, o Ollama precisa estar nesta máquina (127.0.0.1/localhost)"));
        }
        self.0.db.lock().unwrap().set_setting(SETTINGS_KEY, &s)?;
        self.0.events.emit(AppEvent::AiChanged);
        Ok(s)
    }

    fn hardware(&self, caps: Option<&serde_json::Value>) -> HardwareInput {
        let si = self.system_info();
        let gpu = si.gpus.first();
        HardwareInput {
            cores: si.cpu_cores,
            ram_mb: si.memory_total / (1024 * 1024),
            cuda_devices: caps.and_then(|c| c["cuda_devices"].as_u64()).unwrap_or(si.gpus.len() as u64) as u32,
            vram_mb: gpu.and_then(|g| g.memory_total_mb),
        }
    }

    fn effective_profile(&self, settings: &AiSettings, hw: &HardwareInput) -> ProfileSettings {
        let p = settings.profile.unwrap_or_else(|| profile::recommend(hw));
        let mut eff = profile::settings(p, hw);
        if let Some(m) = &settings.whisper_model {
            eff.whisper_model = m.clone();
        }
        eff
    }

    pub fn ai_status(&self) -> AiStatus {
        let (components, caps) = runtime::components(&self.0.ai);
        let settings = self.ai_settings();
        let hw = self.hardware(caps.as_ref());
        AiStatus {
            system_python: runtime::find_system_python().map(|(_, v)| v),
            components,
            ollama: Ollama::new(&settings.ollama_url).status(),
            recommended: profile::recommend(&hw),
            effective: self.effective_profile(&settings, &hw),
            hardware: hw,
            settings,
            library_path: self.0.ai.root.clone(),
        }
    }

    /// Instala um componente em segundo plano (job com progresso).
    pub fn ai_install(&self, component: &str) -> AppResult<Id> {
        let (comps, _) = runtime::components(&self.0.ai);
        let c = comps.iter().find(|c| c.id == component).ok_or_else(|| AppError::new("not_found", format!("componente desconhecido: {component}")))?;
        self.enqueue(JobPayload::AiInstall { component: component.into() }, &format!("Instalar {}", c.name))
    }

    pub fn ai_remove(&self, component: &str) -> AppResult<()> {
        runtime::remove(&self.0.ai, component)?;
        self.0.events.emit(AppEvent::AiChanged);
        Ok(())
    }

    pub(crate) fn run_ai_install(&self, component: &str, cancel: &CancelToken, prog: &(dyn Fn(&str, f32) + Sync)) -> AppResult<serde_json::Value> {
        runtime::install(&self.0.ai, component, prog, cancel)?;
        self.0.events.emit(AppEvent::AiChanged);
        Ok(serde_json::json!({ "component": component }))
    }

    pub fn ollama_pull(&self, model: &str) -> AppResult<Id> {
        let m = model.trim();
        if m.is_empty() || m.contains(char::is_whitespace) {
            return Err(AppError::new("invalid", "nome de modelo inválido"));
        }
        self.enqueue(JobPayload::OllamaPull { model: m.into() }, &format!("Baixar {m} (Ollama)"))
    }

    pub(crate) fn run_ollama_pull(&self, model: &str, cancel: &CancelToken, prog: &(dyn Fn(&str, f32) + Sync)) -> AppResult<serde_json::Value> {
        let o = Ollama::new(&self.ai_settings().ollama_url);
        o.pull(model, prog, cancel)?;
        self.0.events.emit(AppEvent::AiChanged);
        Ok(serde_json::json!({ "model": model }))
    }

    pub fn ollama_test(&self, model: &str) -> AppResult<String> {
        Ok(Ollama::new(&self.ai_settings().ollama_url).test_model(model)?)
    }

    pub fn ollama_delete(&self, model: &str) -> AppResult<()> {
        Ollama::new(&self.ai_settings().ollama_url).delete(model)?;
        self.0.events.emit(AppEvent::AiChanged);
        Ok(())
    }

    // ───────────── análise ─────────────

    pub(crate) fn analysis_options(&self, transcript: bool, force: bool) -> AnalysisOptions {
        let settings = self.ai_settings();
        let hw = self.hardware(None);
        let eff = self.effective_profile(&settings, &hw);
        AnalysisOptions {
            transcript,
            language: settings.language.clone(),
            whisper_model: eff.whisper_model,
            visual_interval: eff.visual_interval,
            force,
            ..Default::default()
        }
    }

    /// Enfileira a análise das mídias (todas do projeto, se `asset_ids` vazio).
    pub fn analyze_media(&self, project_id: Id, asset_ids: &[Id], transcript: bool, force: bool) -> AppResult<Vec<Id>> {
        let p = self.session(project_id)?.lock().unwrap().project.clone();
        let opts = self.analysis_options(transcript, force);
        let mut jobs = vec![];
        for a in p.assets.iter().filter(|a| asset_ids.is_empty() || asset_ids.contains(&a.id)) {
            if a.kind == AssetKind::Image || a.status != AssetStatus::Ready {
                continue;
            }
            jobs.push(self.enqueue(JobPayload::Analyze { project_id, asset_id: a.id, options: opts.clone() }, &format!("Analisar {}", a.name))?);
        }
        if jobs.is_empty() {
            return Err(AppError::new("nothing_to_analyze", "nenhuma mídia de vídeo ou áudio disponível para análise"));
        }
        Ok(jobs)
    }

    pub(crate) fn run_analysis(
        &self,
        project_id: Id,
        asset_id: Id,
        opts: &AnalysisOptions,
        cancel: &CancelToken,
        prog: &(dyn Fn(&str, f32) + Sync),
    ) -> AppResult<serde_json::Value> {
        let asset = self
            .session(project_id)?
            .lock()
            .unwrap()
            .project
            .asset(asset_id)
            .cloned()
            .ok_or_else(|| AppError::new("not_found", "mídia removida do projeto"))?;
        let r = analysis::analyze(&self.0.tools, &self.0.ai, &asset, opts, prog, cancel)?;
        self.0.events.emit(AppEvent::AnalysisReady { project_id, asset_id });
        Ok(serde_json::json!({
            "asset_id": asset_id,
            "speech": r.speech.as_ref().map(|s| (s.method.clone(), s.segments.len())),
            "scenes": r.scenes.as_ref().map(|s| s.cuts.len()),
            "transcript": r.transcript.as_ref().map(|t| t.language.clone()),
            "warnings": r.warnings,
        }))
    }

    /// Resultados em cache para as mídias do projeto.
    pub fn analyses(&self, project_id: Id) -> AppResult<Vec<AssetAnalysis>> {
        let p = self.session(project_id)?.lock().unwrap().project.clone();
        Ok(p.assets.iter().map(|a| AssetAnalysis { asset_id: a.id, result: analysis::load_cached(&self.0.ai, a) }).collect())
    }

    pub(crate) fn analyses_map(&self, p: &Project) -> HashMap<Id, AnalysisResult> {
        p.assets.iter().filter_map(|a| analysis::load_cached(&self.0.ai, a).map(|r| (a.id, r))).collect()
    }

    /// Regras de pausa do preset do projeto (ou padrão).
    pub fn pause_rules(&self, project_id: Id) -> AppResult<PauseRules> {
        let p = self.session(project_id)?.lock().unwrap().project.clone();
        let preset = p.preset_id.as_ref().and_then(|id| self.presets().into_iter().find(|x| &x.id == id));
        Ok(match preset {
            Some(pr) => PauseRules {
                min_silence: Time::from_millis(pr.cuts.min_silence_ms as i64),
                padding: Time::from_millis(pr.cuts.keep_padding_ms as i64),
                preserve_action: pr.cuts.preserve_silent_action,
                ..Default::default()
            },
            None => PauseRules::default(),
        })
    }

    /// Plano de remoção de pausas (não altera nada; a UI mostra antes de aplicar).
    pub fn plan_pauses(&self, project_id: Id, clip_ids: Option<Vec<Id>>, rules: PauseRules) -> AppResult<PausePlan> {
        let p = self.session(project_id)?.lock().unwrap().project.clone();
        let map = self.analyses_map(&p);
        if map.is_empty() {
            return Err(AppError::new("not_analyzed", "analise as mídias antes (aba IA → Analisar)"));
        }
        Ok(autoedit::plan_pauses(&p, &map, clip_ids.as_deref(), &rules))
    }

    /// Aplica o plano como uma única operação desfazível. Com `sync_text`, as
    /// trilhas de texto (legendas) acompanham os cortes.
    pub fn apply_pauses(&self, project_id: Id, plan: &PausePlan, sync_text: bool) -> AppResult<ProjectState> {
        if plan.cuts.is_empty() {
            return Err(AppError::new("empty_plan", "nenhuma pausa para remover com essas regras"));
        }
        let sync: Vec<Id> = if sync_text {
            self.session(project_id)?.lock().unwrap().project.timeline.tracks.iter().filter(|t| t.kind == TrackKind::Text && !t.locked).map(|t| t.id).collect()
        } else {
            vec![]
        };
        let cmds = autoedit::pause_commands(plan, &sync);
        self.execute_batch(project_id, &format!("Remover pausas ({} cortes)", plan.cuts.len()), &cmds)
    }

    fn clip_and_analysis(&self, project_id: Id, clip_id: Id) -> AppResult<(Project, Clip, AnalysisResult)> {
        let p = self.session(project_id)?.lock().unwrap().project.clone();
        let c = p.timeline.clip(clip_id).cloned().ok_or_else(|| AppError::new("not_found", "clipe não encontrado"))?;
        let a = c.asset_id().and_then(|id| p.asset(id)).ok_or_else(|| AppError::new("invalid", "o clipe não é de mídia"))?;
        let r = analysis::load_cached(&self.0.ai, a).ok_or_else(|| AppError::new("not_analyzed", format!("'{}' ainda não foi analisado", a.name)))?;
        Ok((p, c, r))
    }

    /// Marcadores (ou divisões) nos cortes de cena do clipe.
    pub fn apply_scenes(&self, project_id: Id, clip_id: Id, split: bool) -> AppResult<ProjectState> {
        let (p, c, r) = self.clip_and_analysis(project_id, clip_id)?;
        let fps = p.settings.fps;
        let cmds = if split { autoedit::scene_split_commands(&c, &r, fps) } else { autoedit::scene_marker_commands(&c, &r, fps) };
        if cmds.is_empty() {
            return Err(AppError::new("no_scenes", "nenhum corte de cena dentro deste clipe"));
        }
        self.execute_batch(project_id, if split { "Dividir nas cenas" } else { "Marcar cenas" }, &cmds)
    }

    /// Legendas a partir da transcrição do clipe (nova trilha, desfazível).
    pub fn captions_from_transcript(&self, project_id: Id, clip_id: Id, max_chars: u32) -> AppResult<ProjectState> {
        let (p, c, r) = self.clip_and_analysis(project_id, clip_id)?;
        let t = r.transcript.as_ref().ok_or_else(|| AppError::new("no_transcript", "este clipe ainda não tem transcrição (Analisar com transcrição)"))?;
        let cues = autoedit::caption_cues(t, &c, max_chars.clamp(16, 80) as usize, Time::from_secs(4));
        if cues.is_empty() {
            return Err(AppError::new("no_speech", "nenhuma fala transcrita dentro deste clipe"));
        }
        let preset = p.preset_id.as_ref().and_then(|id| self.presets().into_iter().find(|x| &x.id == id));
        let mut style = avs_core::subtitles::caption_style();
        if let Some(pr) = preset {
            // Legendas mais altas em formatos verticais (acima dos controles da plataforma).
            if p.settings.height > p.settings.width {
                style.y = (1.0 - pr.safety.safe_area.bottom_pct / 100.0 - 0.04).clamp(0.5, 0.9);
                style.size = 64.0;
            }
        }
        let (out, st) =
            self.execute(project_id, &EditCommand::AddSubtitleTrack { name: Some(format!("Legendas ({})", t.language)), cues, style: Some(style) })?;
        let _ = out;
        Ok(st)
    }
}
