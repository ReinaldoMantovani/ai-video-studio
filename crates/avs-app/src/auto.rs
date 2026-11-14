//! "Editar automaticamente" (Fase 4): regras do preset → plano → pré-visualização
//! → aplicação como uma única operação desfazível, com versões salvas antes e
//! depois; revisão de qualidade; fluxo completo a partir do dashboard.

use crate::*;
use avs_ai::analysis::{self, AnalysisResult};
use avs_ai::autoplan::{self, AutoEditOptions, AutoEditReport, PresetRules};
use avs_ai::review::{self, QualityReport, ReviewTargets};
use avs_ai::scoring::default_weights;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AutoEditPreview {
    pub rules: PresetRules,
    pub report: AutoEditReport,
    /// Mídias da timeline sem análise (o plano as ignora; o job as analisa).
    pub missing_analysis: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AutoEditResult {
    pub report: AutoEditReport,
    pub snapshot_before: String,
    pub snapshot_after: String,
}

/// Regras padrão quando o projeto não tem preset.
fn default_rules() -> PresetRules {
    PresetRules {
        name: "Sem preset".into(),
        remove_silence: true,
        min_silence_ms: 700,
        padding_ms: 150,
        preserve_silent_action: true,
        remove_fillers: false,
        captions: false,
        caption_y: 0.85,
        caption_size: 52.0,
        transitions: vec![],
        transition_ms: 300,
        transition_frequency: "rare".into(),
        effects: vec![],
        effect_intensity: "off".into(),
        narrative: "preserve".into(),
        target_s: None,
        max_s: None,
        music_ducking_db: -12.0,
        noise_reduction: false,
        voice_priority: false,
        scoring_weights: default_weights(),
        safe_bottom_pct: 0.05,
    }
}

pub fn rules_from_preset(pr: &EditorialPreset, vertical: bool) -> PresetRules {
    let bottom = pr.safety.safe_area.bottom_pct / 100.0;
    let caption_y = match pr.captions.position.as_str() {
        "center" => 0.5,
        "center_low" => 0.72,
        _ => (1.0 - bottom - 0.05).clamp(0.5, 0.92),
    };
    PresetRules {
        name: pr.name.clone(),
        remove_silence: pr.cuts.remove_silence,
        min_silence_ms: pr.cuts.min_silence_ms,
        padding_ms: pr.cuts.keep_padding_ms,
        preserve_silent_action: pr.cuts.preserve_silent_action,
        remove_fillers: pr.cuts.remove_fillers,
        captions: pr.captions.enabled,
        caption_y,
        caption_size: if vertical { 64.0 } else { 52.0 },
        transitions: pr.transitions.allowed.clone(),
        transition_ms: pr.transitions.default_duration_ms,
        transition_frequency: pr.transitions.frequency.clone(),
        effects: pr.effects.allowed.clone(),
        effect_intensity: pr.effects.intensity.clone(),
        narrative: pr.narrative.mode.clone(),
        target_s: pr.duration.target_s,
        max_s: pr.duration.max_s,
        music_ducking_db: pr.audio.music_ducking_db,
        noise_reduction: pr.audio.noise_reduction,
        voice_priority: pr.audio.voice_priority,
        scoring_weights: if pr.scoring_weights.is_empty() { default_weights() } else { pr.scoring_weights.clone() },
        safe_bottom_pct: bottom,
    }
}

impl Studio {
    fn project_preset(&self, p: &Project) -> Option<EditorialPreset> {
        p.preset_id.as_ref().and_then(|id| self.presets().into_iter().find(|x| &x.id == id))
    }

    /// Regras de um preset (antes de existir o projeto: fluxo do dashboard).
    pub fn preset_auto_rules(&self, preset_id: &str) -> AppResult<PresetRules> {
        let pr = self.presets().into_iter().find(|x| x.id == preset_id).ok_or_else(|| AppError::new("not_found", "preset não encontrado"))?;
        Ok(rules_from_preset(&pr, pr.resolution.height > pr.resolution.width))
    }

    pub fn auto_edit_rules(&self, project_id: Id) -> AppResult<PresetRules> {
        let p = self.session(project_id)?.lock().unwrap().project.clone();
        Ok(self.project_preset(&p).map(|pr| rules_from_preset(&pr, p.settings.height > p.settings.width)).unwrap_or_else(default_rules))
    }

    fn timeline_assets(p: &Project) -> Vec<&MediaAsset> {
        let mut ids: Vec<Id> = vec![];
        for c in p.timeline.tracks.iter().flat_map(|t| t.clips.iter()) {
            if let Some(a) = c.asset_id() {
                if !ids.contains(&a) {
                    ids.push(a);
                }
            }
        }
        ids.iter().filter_map(|id| p.asset(*id)).filter(|a| a.kind != AssetKind::Image).collect()
    }

    /// Plano sem alterar o projeto (executado numa cópia).
    pub fn auto_edit_preview(&self, project_id: Id, opts: &AutoEditOptions) -> AppResult<AutoEditPreview> {
        let mut p = self.session(project_id)?.lock().unwrap().project.clone();
        let rules = self.auto_edit_rules(project_id)?;
        let map = self.analyses_map(&p);
        let assets = Self::timeline_assets(&p);
        if assets.is_empty() {
            return Err(AppError::new("empty_timeline", "coloque vídeos ou áudios na timeline antes da edição automática"));
        }
        let missing: Vec<String> = assets.iter().filter(|a| !map.contains_key(&a.id)).map(|a| a.name.clone()).collect();
        if missing.len() == assets.len() {
            // Nada analisado ainda: não há plano a mostrar, mas a interface oferece
            // "Analisar e aplicar" (o job analisa e edita de uma vez).
            let d = p.timeline.duration();
            let report = autoplan::AutoEditReport {
                preset: rules.name.clone(),
                before: d,
                after: d,
                steps: vec![],
                removed: vec![],
                moments: vec![],
                warnings: vec!["As mídias ainda não foram analisadas: o plano detalhado aparece depois da análise.".into()],
            };
            return Ok(AutoEditPreview { rules, report, missing_analysis: missing });
        }
        let report = autoplan::run(&mut p, &rules, &map, opts).map_err(|e| AppError::new("auto_edit", e.to_string()))?;
        Ok(AutoEditPreview { rules, report, missing_analysis: missing })
    }

    /// Aplica: versão "antes", transação única no histórico, versão "depois".
    pub fn auto_edit_apply(&self, project_id: Id, opts: &AutoEditOptions) -> AppResult<(AutoEditResult, ProjectState)> {
        let rules = self.auto_edit_rules(project_id)?;
        let s = self.session(project_id)?;
        let mut s = s.lock().unwrap();
        let map = self.analyses_map(&s.project);
        let paths = ProjectPaths::new(&self.0.cfg.library_root, project_id);
        let before = avs_store::create_snapshot(&paths, &s.project, "Original (antes da edição automática)")?;
        let report = s
            .transaction(&format!("Edição automática ({})", rules.name), |p| autoplan::run(p, &rules, &map, opts))
            .map_err(|e| AppError::new("auto_edit", e.to_string()))?;
        let n = avs_store::list_snapshots(&paths).map(|v| v.iter().filter(|x| x.label.starts_with("Edição IA")).count()).unwrap_or(0) + 1;
        let after = avs_store::create_snapshot(&paths, &s.project, &format!("Edição IA v{n} ({})", rules.name))?;
        s.touch();
        self.autosave(&s)?;
        Ok((AutoEditResult { report, snapshot_before: before.label, snapshot_after: after.label }, state_of(&s, false)))
    }

    /// Revisão de qualidade da timeline atual.
    pub fn quality_review(&self, project_id: Id) -> AppResult<QualityReport> {
        let p = self.session(project_id)?.lock().unwrap().project.clone();
        let map = self.analyses_map(&p);
        let rules = self.auto_edit_rules(project_id)?;
        Ok(review::review(&p, &map, &ReviewTargets { target_s: rules.target_s, max_s: rules.max_s, safe_bottom_pct: rules.safe_bottom_pct }))
    }

    /// Coloca as mídias na timeline, em ordem: vídeos/imagens em sequência na
    /// primeira trilha de vídeo; áudios na primeira trilha de áudio (trilha
    /// sonora, a partir do início). Em projetos cuja proporção difere da
    /// mídia e cujo preset pede reenquadramento, usa "Preencher" (corte
    /// central; o rastreamento de rosto/ação é etapa futura).
    pub fn place_assets(&self, project_id: Id, asset_ids: &[Id]) -> AppResult<ProjectState> {
        let p = self.session(project_id)?.lock().unwrap().project.clone();
        let vt = p.timeline.tracks.iter().find(|t| t.kind == TrackKind::Video).map(|t| t.id);
        let at = p.timeline.tracks.iter().find(|t| t.kind == TrackKind::Audio).map(|t| t.id);
        let reframe = self.project_preset(&p).map(|pr| pr.reframe.mode != "none").unwrap_or(false);
        let pa = p.settings.width as f32 / p.settings.height.max(1) as f32;
        let mut vcur = vt.and_then(|t| p.timeline.track(t)).map(|t| t.clips.iter().map(|c| c.end()).max().unwrap_or(Time::ZERO)).unwrap_or(Time::ZERO);
        let mut acur = at.and_then(|t| p.timeline.track(t)).map(|t| t.clips.iter().map(|c| c.end()).max().unwrap_or(Time::ZERO)).unwrap_or(Time::ZERO);
        let mut cmds = vec![];
        let mut fills = vec![];
        for id in asset_ids {
            let Some(a) = p.asset(*id) else { continue };
            if a.status != AssetStatus::Ready {
                continue;
            }
            // Duração em quadros inteiros sem passar do fim da mídia.
            let fps = p.settings.fps;
            let dur = if a.kind == AssetKind::Image {
                Time::from_secs(5)
            } else {
                let d = a.duration.snap_to_frame(fps);
                if d > a.duration {
                    d - fps.frame_duration()
                } else {
                    d
                }
            };
            if a.has_video() {
                let Some(t) = vt else { continue };
                cmds.push(EditCommand::InsertClip { track_id: t, asset_id: a.id, start: vcur, source_in: None, duration: Some(dur), ripple: false });
                if let Some(v) = &a.video {
                    let (mut w, mut h) = (v.width as f32, v.height as f32);
                    if v.rotation.abs() == 90 || v.rotation.abs() == 270 {
                        std::mem::swap(&mut w, &mut h);
                    }
                    if reframe && ((w / h.max(1.0)) / pa - 1.0).abs() > 0.15 {
                        fills.push(cmds.len() - 1);
                    }
                }
                vcur += dur;
            } else if a.has_audio() {
                let Some(t) = at else { continue };
                cmds.push(EditCommand::InsertClip { track_id: t, asset_id: a.id, start: acur, source_in: None, duration: Some(dur), ripple: false });
                acur += dur;
            }
        }
        if cmds.is_empty() {
            return Err(AppError::new("nothing_to_place", "nenhuma mídia pronta para colocar na timeline"));
        }
        let s = self.session(project_id)?;
        let mut s = s.lock().unwrap();
        s.transaction("Colocar mídias na timeline", |pr| {
            for (i, c) in cmds.iter().enumerate() {
                let out = apply(pr, c)?;
                if fills.contains(&i) {
                    let cid = out.created[0];
                    let mut tf = pr.timeline.clip(cid).unwrap().transform.clone();
                    tf.fit = FitMode::Fill;
                    apply(pr, &EditCommand::UpdateClip { clip_id: cid, patch: ClipPatch { transform: Some(tf), ..Default::default() } })?;
                }
            }
            Ok(())
        })?;
        self.autosave(&s)?;
        Ok(state_of(&s, false))
    }

    /// Job: analisa o que falta e aplica a edição automática.
    pub fn auto_edit_start(&self, project_id: Id, opts: AutoEditOptions) -> AppResult<Id> {
        let p = self.session(project_id)?.lock().unwrap().project.clone();
        if Self::timeline_assets(&p).is_empty() {
            return Err(AppError::new("empty_timeline", "coloque vídeos ou áudios na timeline antes da edição automática"));
        }
        self.enqueue(JobPayload::AutoEdit { project_id, options: opts }, &format!("Edição automática: {}", p.name))
    }

    pub(crate) fn run_auto_edit(
        &self,
        project_id: Id,
        opts: &AutoEditOptions,
        cancel: &CancelToken,
        prog: &(dyn Fn(&str, f32) + Sync),
    ) -> AppResult<serde_json::Value> {
        let p = self.session(project_id)?.lock().unwrap().project.clone();
        let rules = self.auto_edit_rules(project_id)?;
        // Transcrição só quando o preset precisa dela (legendas / hesitações) e está instalada.
        let wants_text = opts.captions.unwrap_or(rules.captions) || opts.remove_fillers.unwrap_or(rules.remove_fillers);
        let status = self.ai_status();
        let can_transcribe = status.components.iter().any(|c| c.id == "transcription" && c.installed)
            && status.components.iter().any(|c| c.id == format!("whisper:{}", status.effective.whisper_model) && c.installed);
        let aopts = self.analysis_options(wants_text && can_transcribe, false);
        let assets: Vec<MediaAsset> = Self::timeline_assets(&p).into_iter().cloned().collect();
        let n = assets.len().max(1) as f32;
        let mut notes: Vec<String> = vec![];
        if wants_text && !can_transcribe {
            notes.push("Transcrição não instalada: legendas e remoção de hesitações foram puladas (Gerenciador de IA → Transcrição).".into());
        }
        for (i, a) in assets.iter().enumerate() {
            let cached: Option<AnalysisResult> = analysis::load_cached(&self.0.ai, a);
            let need = cached.as_ref().map(|c| aopts.transcript && c.transcript.is_none()).unwrap_or(true);
            if need {
                let base = i as f32 / n;
                let sub = |st: &str, f: f32| prog(&format!("Analisando {}: {st}", a.name), (base + f / n) * 0.85);
                let r = analysis::analyze(&self.0.tools, &self.0.ai, a, &aopts, &sub, cancel)?;
                notes.extend(r.warnings.iter().map(|w| format!("{}: {w}", a.name)));
                self.0.events.emit(AppEvent::AnalysisReady { project_id, asset_id: a.id });
            }
            if cancel.is_cancelled() {
                return Err(avs_ai::AiError::Cancelled.into());
            }
        }
        prog("Montando a edição", 0.9);
        let (res, _) = self.auto_edit_apply(project_id, opts)?;
        prog("Revisando a qualidade", 0.97);
        let review = self.quality_review(project_id)?;
        let mut report = res.report;
        report.warnings.extend(notes);
        Ok(serde_json::json!({ "report": report, "review": review, "snapshot_before": res.snapshot_before, "snapshot_after": res.snapshot_after }))
    }
}
