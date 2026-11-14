//! Fase 5 — cortes verticais. Cada corte vira um projeto próprio (cópia do
//! projeto de origem limitada ao trecho), reenquadrado para o formato do
//! preset, limpo pelo planejador automático e, se pedido, exportado em lote.
//! O projeto de origem nunca é alterado.

use crate::auto::rules_from_preset;
use crate::*;
use avs_ai::autoplan::{self, AutoEditOptions, CutIntensity};
use avs_ai::shorts::{self, FramingMode, ShortCandidate, ShortsOptions};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ShortsDefaults {
    pub preset_id: String,
    pub options: ShortsOptions,
    pub framing: FramingMode,
    pub export_profile: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ShortsExport {
    pub output_dir: PathBuf,
    pub profile_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ShortsRequest {
    pub preset_id: String,
    pub framing: FramingMode,
    pub candidates: Vec<ShortCandidate>,
    pub captions: bool,
    pub remove_pauses: bool,
    pub export: Option<ShortsExport>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ShortCreated {
    pub project_id: Id,
    pub name: String,
    pub duration: Time,
    /// Planos de enquadramento aplicados (0 = sem reenquadramento).
    pub framing_segments: u32,
    /// Planos que seguem um rosto detectado.
    pub face_segments: u32,
    pub steps: Vec<String>,
    pub warnings: Vec<String>,
    pub export_job: Option<Id>,
}

fn vertical_presets(all: &[EditorialPreset]) -> impl Iterator<Item = &EditorialPreset> {
    all.iter().filter(|p| p.resolution.height > p.resolution.width)
}

/// Copia (hard link quando possível) os arquivos derivados de uma mídia para
/// outro projeto e reaponta os caminhos.
fn link_derived(src: &ProjectPaths, dst: &ProjectPaths, a: &mut MediaAsset) {
    let (sd, dd) = (src.asset_cache(a.id), dst.asset_cache(a.id));
    if std::fs::create_dir_all(&dd).is_err() {
        return;
    }
    if let Ok(rd) = std::fs::read_dir(&sd) {
        for e in rd.flatten() {
            let to = dd.join(e.file_name());
            if std::fs::hard_link(e.path(), &to).is_err() {
                let _ = std::fs::copy(e.path(), &to);
            }
        }
    }
    let remap = |p: &Option<PathBuf>| -> Option<PathBuf> { p.as_ref().and_then(|p| p.strip_prefix(&sd).ok().map(|r| dd.join(r))).filter(|p| p.exists()) };
    if let Some(t) = a.thumbs.as_mut() {
        if let Some(np) = remap(&Some(t.path.clone())) {
            t.path = np;
        }
    }
    a.poster_path = remap(&a.poster_path).or(a.poster_path.take());
    a.waveform_path = remap(&a.waveform_path).or(a.waveform_path.take());
    if let Some(px) = &a.proxy_path {
        let to = dst.proxies().join(px.file_name().unwrap_or_default());
        let _ = std::fs::create_dir_all(dst.proxies());
        a.proxy_path = (std::fs::hard_link(px, &to).is_ok() || to.exists()).then_some(to);
    }
}

fn source_dims(a: &MediaAsset) -> Option<(u32, u32)> {
    let v = a.video.as_ref()?;
    Some(if v.rotation.abs() == 90 || v.rotation.abs() == 270 { (v.height, v.width) } else { (v.width, v.height) })
}

impl Studio {
    /// Valores iniciais para um projeto: preset vertical (o do projeto, se for
    /// vertical; senão YouTube Shorts), durações e enquadramento do preset.
    pub fn shorts_defaults(&self, project_id: Id, preset_id: Option<&str>) -> AppResult<ShortsDefaults> {
        let p = self.session(project_id)?.lock().unwrap().project.clone();
        let all = self.presets();
        let pick = preset_id
            .and_then(|id| vertical_presets(&all).find(|x| x.id == id))
            .or_else(|| p.preset_id.as_ref().and_then(|id| vertical_presets(&all).find(|x| &x.id == id)))
            .or_else(|| vertical_presets(&all).find(|x| x.id == "youtube_shorts"))
            .or_else(|| vertical_presets(&all).next())
            .ok_or_else(|| AppError::new("not_found", "nenhum preset vertical disponível"))?
            .clone();
        let target = pick.duration.target_s.unwrap_or(35.0).clamp(10.0, 170.0);
        let max = (target * 1.5).min(pick.duration.max_s.unwrap_or(180.0)).max(target);
        let min = (target * 0.45).max(8.0).min(target);
        let framing = if pick.reframe.mode == "none" { FramingMode::Center } else { FramingMode::Face };
        let export_profile = self.export_profiles().iter().find(|x| x.id == pick.export_profile).map(|x| x.id.clone());
        Ok(ShortsDefaults {
            preset_id: pick.id.clone(),
            options: ShortsOptions { count: 5, min_s: min, max_s: max, target_s: target },
            framing,
            export_profile,
        })
    }

    /// Sugestões de corte (precisa das mídias analisadas).
    pub fn suggest_shorts(&self, project_id: Id, preset_id: &str, opts: &ShortsOptions) -> AppResult<Vec<ShortCandidate>> {
        let p = self.session(project_id)?.lock().unwrap().project.clone();
        let pr = self.presets().into_iter().find(|x| x.id == preset_id).ok_or_else(|| AppError::new("not_found", "preset não encontrado"))?;
        let main = p
            .timeline
            .tracks
            .iter()
            .find(|t| t.kind == TrackKind::Video && t.clips.iter().any(|c| c.asset_id().is_some()))
            .or_else(|| p.timeline.tracks.iter().find(|t| t.kind == TrackKind::Audio && !t.clips.is_empty()))
            .map(|t| t.id)
            .ok_or_else(|| AppError::new("empty_timeline", "coloque vídeos na timeline antes de gerar cortes"))?;
        let map = self.analyses_map(&p);
        let used = p.timeline.track(main).unwrap().clips.iter().filter_map(|c| c.asset_id()).collect::<Vec<_>>();
        if !used.iter().any(|id| map.contains_key(id)) {
            return Err(AppError::new("not_analyzed", "analise as mídias da timeline (aba IA → Analisar mídias) antes de gerar cortes"));
        }
        let weights = if pr.scoring_weights.is_empty() { avs_ai::scoring::default_weights() } else { pr.scoring_weights.clone() };
        Ok(shorts::suggest(&p, main, &map, &weights, opts))
    }

    pub fn create_shorts_start(&self, project_id: Id, req: ShortsRequest) -> AppResult<Id> {
        if req.candidates.is_empty() {
            return Err(AppError::new("invalid", "escolha pelo menos um corte"));
        }
        if let Some(e) = &req.export {
            if !e.output_dir.is_absolute() {
                return Err(AppError::new("invalid", "escolha uma pasta de destino para a exportação"));
            }
        }
        let name = self.session(project_id)?.lock().unwrap().project.name.clone();
        self.enqueue(JobPayload::Shorts { project_id, request: req.clone() }, &format!("Cortes de {name} ({})", req.candidates.len()))
    }

    pub(crate) fn run_shorts(
        &self,
        project_id: Id,
        req: &ShortsRequest,
        cancel: &CancelToken,
        prog: &(dyn Fn(&str, f32) + Sync),
    ) -> AppResult<serde_json::Value> {
        let n = req.candidates.len() as f32;
        let mut out = vec![];
        let mut errors: Vec<String> = vec![];
        for (i, c) in req.candidates.iter().enumerate() {
            if cancel.is_cancelled() {
                return Err(MediaError::Cancelled.into());
            }
            prog(&format!("Criando corte {} de {}: {}", i + 1, req.candidates.len(), c.title), i as f32 / n);
            // Um corte com problema não impede os outros.
            match self.create_short(project_id, c, req) {
                Ok(x) => out.push(x),
                Err(e) => errors.push(format!("Corte {} ({}): {}", c.rank, c.title, e.message)),
            }
        }
        if out.is_empty() {
            return Err(AppError::new("shorts", errors.join("; ")));
        }
        prog("Cortes criados", 1.0);
        Ok(serde_json::json!({ "shorts": out, "errors": errors }))
    }

    /// Cria um corte como projeto novo. Público para a CLI e os testes.
    pub fn create_short(&self, src_id: Id, cand: &ShortCandidate, req: &ShortsRequest) -> AppResult<ShortCreated> {
        let src = self.session(src_id)?.lock().unwrap().project.clone();
        let pr = self.presets().into_iter().find(|x| x.id == req.preset_id).ok_or_else(|| AppError::new("not_found", "preset não encontrado"))?;
        let (w, h) = (pr.resolution.width, pr.resolution.height);
        let mut settings = src.settings.clone();
        settings.width = w;
        settings.height = h;
        let name = format!("{} — Corte {}: {}", src.name, cand.rank, cand.title);
        let mut p = Project::new(name.chars().take(120).collect::<String>(), settings);
        p.preset_id = Some(pr.id.clone());
        p.timeline = src.timeline.clone();
        let (sp, dp) = (ProjectPaths::new(&self.0.cfg.library_root, src_id), ProjectPaths::new(&self.0.cfg.library_root, p.id));
        dp.create_all()?;
        p.assets = src.assets.clone();
        for a in &mut p.assets {
            link_derived(&sp, &dp, a);
        }
        let map = self.analyses_map(&p);
        let fps = p.settings.fps;
        let range = TimeRange::new(cand.range.start.snap_to_frame(fps), cand.range.end.snap_to_frame(fps));
        let mut steps: Vec<String> = vec![];
        let mut warnings: Vec<String> = vec![];
        let (mut nseg, mut nface) = (0u32, 0u32);

        let mut s = EditSession::new(p);
        s.transaction("Criar corte", |p| {
            // 1. Só o trecho escolhido, em todas as trilhas.
            let end = p.timeline.duration().snap_to_frame(fps);
            for t in p.timeline.tracks.iter_mut() {
                t.locked = false;
            }
            if range.end < end {
                apply(p, &EditCommand::DeleteRange { start: range.end, end, track_ids: None, ripple: true })?;
            }
            if range.start.is_positive() {
                apply(p, &EditCommand::DeleteRange { start: Time::ZERO, end: range.start, track_ids: None, ripple: true })?;
            }
            let main = p.timeline.tracks.iter().find(|t| t.kind == TrackKind::Video && t.clips.iter().any(|c| c.asset_id().is_some())).map(|t| t.id);

            // 2. Enquadramento.
            if let Some(main) = main {
                let clips: Vec<Clip> = p.timeline.track(main).unwrap().clips.clone();
                for c in clips {
                    let Some(asset) = c.asset_id().and_then(|id| p.asset(id)).cloned() else { continue };
                    let Some(dims) = source_dims(&asset) else { continue };
                    let Some(m) = avs_ai::autoedit::ClipMap::of(&c) else { continue };
                    let segs = match req.framing {
                        FramingMode::Face => shorts::plan_reframe(map.get(&asset.id), m.window(), dims, (w, h)),
                        FramingMode::Center => vec![shorts::ReframeSegment { start: m.window().start, end: m.window().end, x: 0.0, face: false }],
                        FramingMode::BlurBackground => vec![],
                    };
                    if segs.is_empty() {
                        let mut tf = c.transform.clone();
                        tf.fit = FitMode::Fit;
                        apply(p, &EditCommand::UpdateClip { clip_id: c.id, patch: ClipPatch { transform: Some(tf), ..Default::default() } })?;
                        continue;
                    }
                    let mut cur = c.id;
                    for (k, sg) in segs.iter().enumerate() {
                        if k > 0 {
                            let at = m.to_timeline(sg.start).snap_to_frame(fps);
                            let cc = p.timeline.clip(cur).unwrap();
                            if at > cc.start && at < cc.end() {
                                cur = apply(p, &EditCommand::SplitClip { clip_id: cur, at })?.created[0];
                            }
                        }
                        let mut tf = p.timeline.clip(cur).unwrap().transform.clone();
                        tf.fit = FitMode::Fill;
                        tf.x = sg.x;
                        tf.y = 0.0;
                        apply(p, &EditCommand::UpdateClip { clip_id: cur, patch: ClipPatch { transform: Some(tf), ..Default::default() } })?;
                        nseg += 1;
                        nface += sg.face as u32;
                    }
                }
            }
            match req.framing {
                FramingMode::Face if nface > 0 => steps.push(format!("Enquadramento: {nseg} plano(s), {nface} seguindo o rosto")),
                FramingMode::Face => {
                    steps.push("Enquadramento: corte central".into());
                    let detector = map.values().any(|a| a.visual.as_ref().map(|v| v.face_detector.is_some()).unwrap_or(false));
                    warnings.push(if detector {
                        "Nenhum rosto detectado no trecho: usado o corte central.".into()
                    } else {
                        "Detecção de rostos indisponível (instale \"Detecção de rostos (YuNet)\" no Gerenciador de IA e reanalise as mídias): usado o corte central.".into()
                    });
                }
                FramingMode::Center => steps.push("Enquadramento: corte central".into()),
                FramingMode::BlurBackground => steps.push("Enquadramento: vídeo inteiro com fundo desfocado".into()),
            }

            // 3. Limpeza automática com as regras do preset vertical (sem nova seleção).
            let mut rules = rules_from_preset(&pr, true);
            rules.narrative = "preserve".into();
            rules.target_s = None;
            rules.transition_frequency = "rare".into();
            let opts = AutoEditOptions {
                cuts: CutIntensity::Balanced,
                effects: None,
                narrative: Some("preserve".into()),
                target_duration: None,
                remove_pauses: Some(req.remove_pauses),
                remove_fillers: Some(req.captions && rules.remove_fillers),
                captions: Some(req.captions),
                transitions: false,
                zoom: req.framing != FramingMode::BlurBackground,
                audio_treatment: true,
                ducking: true,
                color_correction: true,
            };
            match autoplan::run(p, &rules, &map, &opts) {
                Ok(rep) => {
                    steps.extend(rep.steps.iter().map(|s| format!("{}: {}", s.title, s.detail)));
                    warnings.extend(rep.warnings.into_iter().filter(|w| !w.starts_with("Nenhuma alteração")));
                }
                Err(e) => warnings.push(format!("Limpeza automática não aplicada: {e}")),
            }

            // 4. Fundo desfocado: a mesma imagem preenchendo o quadro, abaixo.
            if req.framing == FramingMode::BlurBackground {
                if let Some(main) = main {
                    let bg = apply(p, &EditCommand::AddTrack { kind: TrackKind::Video, name: Some("Fundo desfocado".into()) })?.created[0];
                    apply(p, &EditCommand::MoveTrack { track_id: bg, index: 0 })?;
                    let clips: Vec<Clip> = p.timeline.track(main).unwrap().clips.clone();
                    let mut skipped = 0;
                    for c in clips {
                        let Some(aid) = c.asset_id() else { continue };
                        if (c.speed - 1.0).abs() > 1e-4 || c.reverse || c.freeze {
                            skipped += 1;
                            continue;
                        }
                        let id = apply(p, &EditCommand::InsertClip { track_id: bg, asset_id: aid, start: c.start, source_in: Some(c.source_in()), duration: Some(c.duration), ripple: false })?.created[0];
                        let fx = VideoEffects { blur: 40.0, ..Default::default() };
                        let tf = Transform { fit: FitMode::Fill, ..Default::default() };
                        let color = ColorAdjust { brightness: -0.08, contrast: 1.0, saturation: 0.9 };
                        apply(p, &EditCommand::UpdateClip { clip_id: id, patch: ClipPatch { transform: Some(tf), effects: Some(fx), color: Some(color), audio_muted: Some(true), ..Default::default() } })?;
                    }
                    if skipped > 0 {
                        warnings.push(format!("{skipped} clipe(s) com velocidade, reverso ou congelamento ficaram sem fundo desfocado."));
                    }
                }
            }
            Ok(())
        })
        .map_err(|e| {
            // Nada fica pela metade: o projeto ainda não foi registrado.
            let _ = std::fs::remove_dir_all(&dp.dir);
            AppError::new("shorts", e.to_string())
        })?;
        s.mark_saved();

        // Registra o projeto novo (arquivo, índice, sessão) e salva a versão inicial.
        let p = s.project.clone();
        save_project_file(&dp.file(), &p)?;
        let thumb = p.assets.iter().find_map(|a| a.poster_path.clone());
        self.0.db.lock().unwrap().upsert_project(&p, &dp.dir, thumb.as_deref(), "saved")?;
        avs_store::create_snapshot(&dp, &p, "Corte gerado")?;
        let duration = p.timeline.duration();
        let pid = p.id;
        self.0.sessions.lock().unwrap().insert(pid, Arc::new(Mutex::new(s)));

        let export_job = match &req.export {
            Some(e) => {
                let prof = self
                    .export_profiles()
                    .into_iter()
                    .find(|x| x.id == e.profile_id)
                    .ok_or_else(|| AppError::new("not_found", "perfil de exportação não encontrado"))?;
                let file = sanitize_file_stem(&format!("{:02} {}", cand.rank, cand.title), 60);
                Some(self.export(pid, ExportRequest::from_profile(&prof, e.output_dir.clone(), file))?)
            }
            None => None,
        };
        Ok(ShortCreated { project_id: pid, name: p.name, duration, framing_segments: nseg, face_segments: nface, steps, warnings, export_job })
    }
}
