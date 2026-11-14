//! # avs-app
//!
//! Camada de serviço compartilhada pela aplicação desktop (Tauri) e pela CLI.
//! Coordena projetos abertos, importação, a fila persistente de jobs
//! (prioridades, pausa, cancelamento, recuperação), preview, exportação
//! validada, presets e informações do sistema. Não conhece a interface: emite
//! eventos tipados por um `EventSink`.

pub mod ai;
pub mod assistant;
pub mod auto;
pub mod media_server;
pub mod presets;
pub mod shorts;
pub mod system;

use avs_core::*;
use avs_media::compile::{audio_coverage, video_coverage, CompileOptions};
use avs_media::hwaccel::{best_encoder, detect_encoders, EncoderInfo};
use avs_media::render::{render, render_frame, AudioEncode, Container, RenderProgress, RenderRequest, VideoEncode};
use avs_media::validate::{codec_family, validate_output, Expectation, ValidationReport};
use avs_media::{derive, probe, CancelToken, FfmpegTools, FontResolver, MediaError};
use avs_store::{
    load_project_file, load_with_fallback, now_rfc3339, safe_join, sanitize_file_stem, save_project_file, write_atomic, Db, ExportRecord, JobRecord,
    ProjectPaths, ProjectSummary, SnapshotInfo, StoreError,
};
use presets::{EditorialPreset, ExportProfile};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use ts_rs::TS;

pub use avs_ai;
pub use avs_core;
pub use avs_media;
pub use avs_store;

// ───────────────────────────── Erros ─────────────────────────────

/// Erro serializável exibido na interface (código estável + mensagem real).
#[derive(Debug, Clone, Serialize, TS, thiserror::Error)]
#[ts(export)]
#[error("{message}")]
pub struct AppError {
    pub code: String,
    pub message: String,
}

impl AppError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        AppError { code: code.into(), message: message.into() }
    }
}
impl From<MediaError> for AppError {
    fn from(e: MediaError) -> Self {
        let code = match &e {
            MediaError::ToolNotFound(_) => "ffmpeg_missing",
            MediaError::Cancelled => "cancelled",
            MediaError::InvalidMedia(_) => "invalid_media",
            MediaError::InvalidTimeline(_) => "invalid_timeline",
            MediaError::ValidationFailed(_) => "validation_failed",
            _ => "media",
        };
        AppError::new(code, e.to_string())
    }
}
impl From<StoreError> for AppError {
    fn from(e: StoreError) -> Self {
        AppError::new("store", e.to_string())
    }
}
impl From<CommandError> for AppError {
    fn from(e: CommandError) -> Self {
        AppError::new("edit", e.to_string())
    }
}
impl From<avs_ai::AiError> for AppError {
    fn from(e: avs_ai::AiError) -> Self {
        AppError::new(e.code(), e.to_string())
    }
}
impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::new("io", e.to_string())
    }
}

pub type AppResult<T> = std::result::Result<T, AppError>;

// ───────────────────────────── Eventos ─────────────────────────────

#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum AppEvent {
    Job {
        job: JobRecord,
    },
    Asset {
        project_id: Id,
        asset: MediaAsset,
    },
    ProjectChanged {
        project_id: Id,
        #[ts(type = "number")]
        revision: u64,
    },
    PreviewReady {
        project_id: Id,
        path: PathBuf,
        #[ts(type = "number")]
        revision: u64,
        /// Janela da timeline coberta (`None` = timeline inteira).
        range: Option<TimeRange>,
    },
    AnalysisReady {
        project_id: Id,
        asset_id: Id,
    },
    AiChanged,
}

pub trait EventSink: Send + Sync {
    fn emit(&self, e: AppEvent);
}

pub struct NullSink;
impl EventSink for NullSink {
    fn emit(&self, _e: AppEvent) {}
}

// ───────────────────────────── Configuração ─────────────────────────────

#[derive(Debug, Clone)]
pub struct StudioConfig {
    pub library_root: PathBuf,
    pub presets_dir: PathBuf,
    pub ffmpeg_dirs: Vec<PathBuf>,
    pub font_dirs: Vec<PathBuf>,
    pub workers: usize,
    pub generate_proxies: bool,
    pub deep_validation: bool,
    /// Pasta com o pacote Python `avs_worker` (empacotada como recurso).
    pub worker_dir: PathBuf,
}

impl StudioConfig {
    pub fn new(library_root: PathBuf, presets_dir: PathBuf) -> Self {
        let worker_dir =
            std::env::var_os("AVS_WORKER_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../workers/python"));
        StudioConfig {
            library_root,
            presets_dir,
            ffmpeg_dirs: vec![],
            font_dirs: vec![],
            workers: 2,
            generate_proxies: true,
            deep_validation: true,
            worker_dir,
        }
    }
}

// ───────────────────────────── Contratos de API ─────────────────────────────

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ProjectState {
    pub project: Project,
    pub history: HistoryInfo,
    pub dirty: bool,
    /// O projeto foi recuperado do autosave (fechamento inesperado).
    pub recovered: bool,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ImportResult {
    pub assets: Vec<MediaAsset>,
    /// Trilhas de legenda criadas a partir de arquivos .srt/.vtt importados.
    pub subtitle_tracks: Vec<Id>,
    pub skipped: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct SubtitleImport {
    pub track_id: Id,
    pub cues: u32,
    pub skipped: u32,
    pub state: ProjectState,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExportRequest {
    pub output_dir: PathBuf,
    pub file_name: String,
    pub container: Container,
    /// h264 | hevc | vp9
    pub codec: String,
    /// Encoder específico; `None` = melhor encoder funcional detectado.
    pub encoder: Option<String>,
    pub prefer_hardware: bool,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<Rational>,
    pub quality: u8,
    pub speed: String,
    pub bitrate_kbps: Option<u32>,
    pub audio_bitrate_kbps: u32,
    pub loudness_lufs: Option<f32>,
    pub range: Option<TimeRange>,
}

impl ExportRequest {
    pub fn from_profile(p: &ExportProfile, output_dir: PathBuf, file_name: String) -> Self {
        ExportRequest {
            output_dir,
            file_name,
            container: p.container,
            codec: p.codec.clone(),
            encoder: None,
            prefer_hardware: true,
            width: p.resolution.as_ref().map(|r| r.width),
            height: p.resolution.as_ref().map(|r| r.height),
            fps: p.fps,
            quality: p.quality,
            speed: p.speed.clone(),
            bitrate_kbps: p.bitrate_kbps,
            audio_bitrate_kbps: p.audio_bitrate_kbps,
            loudness_lufs: p.loudness_lufs,
            range: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ExportResult {
    pub path: PathBuf,
    pub encoder: String,
    pub report: ValidationReport,
    pub duration: Time,
    pub chunks_total: u32,
    pub chunks_reused: u32,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum JobPayload {
    Import {
        project_id: Id,
        asset_id: Id,
    },
    Preview {
        project_id: Id,
        revision: u64,
        /// Janela (timelines longas); `None` = timeline inteira.
        #[serde(default)]
        range: Option<TimeRange>,
    },
    Export {
        project_id: Id,
        request: ExportRequest,
        project: Box<Project>,
    },
    Analyze {
        project_id: Id,
        asset_id: Id,
        options: avs_ai::analysis::AnalysisOptions,
    },
    AiInstall {
        component: String,
    },
    OllamaPull {
        model: String,
    },
    AutoEdit {
        project_id: Id,
        options: avs_ai::autoplan::AutoEditOptions,
    },
    Shorts {
        project_id: Id,
        request: crate::shorts::ShortsRequest,
    },
}

impl JobPayload {
    fn kind(&self) -> &'static str {
        match self {
            JobPayload::Import { .. } => "import",
            JobPayload::Preview { .. } => "preview",
            JobPayload::Export { .. } => "export",
            JobPayload::Analyze { .. } => "analysis",
            JobPayload::AiInstall { .. } => "ai_install",
            JobPayload::OllamaPull { .. } => "ollama_pull",
            JobPayload::AutoEdit { .. } => "auto_edit",
            JobPayload::Shorts { .. } => "shorts",
        }
    }
    /// Menor = mais prioritário (interatividade > preview > análise > exportação).
    fn priority(&self) -> i32 {
        match self {
            JobPayload::Preview { .. } => 2,
            JobPayload::Import { .. } => 4,
            JobPayload::Export { .. } => 5,
            JobPayload::AutoEdit { .. } => 5,
            JobPayload::Shorts { .. } => 5,
            JobPayload::Analyze { .. } => 4,
            JobPayload::AiInstall { .. } | JobPayload::OllamaPull { .. } => 6,
        }
    }
    fn project_id(&self) -> Option<Id> {
        match self {
            JobPayload::Import { project_id, .. }
            | JobPayload::Preview { project_id, .. }
            | JobPayload::Export { project_id, .. }
            | JobPayload::Analyze { project_id, .. }
            | JobPayload::AutoEdit { project_id, .. }
            | JobPayload::Shorts { project_id, .. } => Some(*project_id),
            JobPayload::AiInstall { .. } | JobPayload::OllamaPull { .. } => None,
        }
    }
}

pub mod job_state {
    pub const QUEUED: &str = "queued";
    pub const RUNNING: &str = "running";
    pub const PAUSED: &str = "paused";
    pub const COMPLETED: &str = "completed";
    pub const CANCELLED: &str = "cancelled";
    pub const FAILED: &str = "failed";
    pub const FAILED_RECOVERABLE: &str = "failed_recoverable";
}
use job_state::*;

struct RunningCtl {
    cancel: CancelToken,
    pause: Arc<AtomicBool>,
}

#[derive(Default)]
struct Queue {
    pending: Vec<(i32, u64, Id)>,
    records: HashMap<Id, JobRecord>,
    running: HashMap<Id, RunningCtl>,
    seq: u64,
}

struct Inner {
    cfg: StudioConfig,
    db: Mutex<Db>,
    tools: FfmpegTools,
    fonts: FontResolver,
    monitor: system::SystemMonitor,
    encoders: Mutex<Option<Vec<EncoderInfo>>>,
    sessions: Mutex<HashMap<Id, Arc<Mutex<EditSession>>>>,
    queue: Mutex<Queue>,
    cv: Condvar,
    events: Arc<dyn EventSink>,
    presets: Mutex<Vec<EditorialPreset>>,
    ai: avs_ai::runtime::AiPaths,
    preset_errors: Vec<String>,
    profiles: Vec<ExportProfile>,
    shutting_down: AtomicBool,
}

/// Serviço principal. Clonável e seguro entre threads.
#[derive(Clone)]
pub struct Studio(Arc<Inner>);

pub const MEDIA_EXTENSIONS: &[&str] = &[
    "mp4", "mov", "mkv", "webm", "avi", "m4v", "mts", "m2ts", "ts", "flv", "wmv", "mpg", "mpeg", "3gp", "mxf", // vídeo
    "mp3", "wav", "aac", "m4a", "flac", "ogg", "opus", "wma", "aiff", "aif", // áudio
    "png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "gif", // imagem
    "srt", "vtt", // legendas
];

fn even(x: f64) -> u32 {
    (((x / 2.0).round() as u32) * 2).max(2)
}

impl Studio {
    pub fn start(cfg: StudioConfig, events: Arc<dyn EventSink>) -> AppResult<Studio> {
        std::fs::create_dir_all(&cfg.library_root)?;
        let tools = FfmpegTools::locate(&cfg.ffmpeg_dirs)?;
        let fonts = FontResolver::new(&cfg.font_dirs);
        let db = Db::open(&cfg.library_root.join("library.db"))?;
        let (mut presets, preset_errors) = presets::load_editorial(&cfg.presets_dir);
        for p in &mut presets {
            p.builtin = true;
        }
        let (user, user_err) = presets::load_editorial(&cfg.library_root.join("presets"));
        presets.extend(user);
        let mut preset_errors = preset_errors;
        preset_errors.extend(user_err);
        for e in &preset_errors {
            tracing::warn!(target: "avs::presets", "{e}");
        }
        let profiles = presets::load_export_profiles(&cfg.presets_dir).map_err(|e| AppError::new("presets", e))?;
        let workers = cfg.workers.max(1);
        let ai_paths = avs_ai::runtime::AiPaths::new(&cfg.library_root, cfg.worker_dir.clone());
        let inner = Arc::new(Inner {
            cfg,
            db: Mutex::new(db),
            tools,
            fonts,
            monitor: system::SystemMonitor::default(),
            encoders: Mutex::new(None),
            sessions: Mutex::new(HashMap::new()),
            queue: Mutex::new(Queue::default()),
            cv: Condvar::new(),
            events,
            presets: Mutex::new(presets),
            ai: ai_paths,
            preset_errors,
            profiles,
            shutting_down: AtomicBool::new(false),
        });
        let studio = Studio(inner);
        studio.recover_jobs()?;
        for i in 0..workers {
            let s = studio.clone();
            std::thread::Builder::new().name(format!("avs-worker-{i}")).spawn(move || s.worker_loop())?;
        }
        Ok(studio)
    }

    pub fn ffmpeg(&self) -> &FfmpegTools {
        &self.0.tools
    }
    pub fn library_root(&self) -> &Path {
        &self.0.cfg.library_root
    }

    /// Para os workers. Jobs interrompidos voltam para a fila e serão
    /// retomados na próxima execução (reaproveitando blocos já renderizados).
    pub fn shutdown(&self) {
        self.0.shutting_down.store(true, Ordering::SeqCst);
        let q = self.0.queue.lock().unwrap();
        for ctl in q.running.values() {
            ctl.cancel.cancel();
        }
        drop(q);
        self.0.cv.notify_all();
    }

    // ───────────── sistema / presets ─────────────

    pub fn system_info(&self) -> system::SystemInfo {
        self.0.monitor.snapshot(&self.0.cfg.library_root, &self.0.tools.version)
    }

    /// Encoders funcionais (testados com codificação real; resultado em cache).
    pub fn encoders(&self) -> Vec<EncoderInfo> {
        let mut g = self.0.encoders.lock().unwrap();
        if g.is_none() {
            *g = Some(detect_encoders(&self.0.tools));
        }
        g.clone().unwrap()
    }

    pub fn presets(&self) -> Vec<EditorialPreset> {
        self.0.presets.lock().unwrap().clone()
    }
    pub fn preset_errors(&self) -> Vec<String> {
        self.0.preset_errors.clone()
    }
    pub fn export_profiles(&self) -> Vec<ExportProfile> {
        self.0.profiles.clone()
    }

    /// Salva/atualiza um preset do usuário (duplicar, editar, importar).
    pub fn save_user_preset(&self, mut p: EditorialPreset) -> AppResult<EditorialPreset> {
        p.builtin = false;
        p.validate().map_err(|e| AppError::new("preset_invalid", e))?;
        let dir = self.0.cfg.library_root.join("presets").join("editorial");
        let path = safe_join(&dir, &format!("{}.json", p.id))?;
        std::fs::create_dir_all(&dir)?;
        write_atomic(&path, &serde_json::to_vec_pretty(&p).map_err(|e| AppError::new("json", e.to_string()))?)?;
        let mut all = self.0.presets.lock().unwrap();
        all.retain(|x| !(x.id == p.id && !x.builtin));
        if all.iter().any(|x| x.id == p.id && x.builtin) {
            return Err(AppError::new("preset_conflict", format!("'{}' é um preset embutido; use outro id", p.id)));
        }
        all.push(p.clone());
        Ok(p)
    }

    // ───────────── projetos ─────────────

    pub fn list_projects(&self) -> AppResult<Vec<ProjectSummary>> {
        Ok(self.0.db.lock().unwrap().list_projects()?)
    }

    pub fn create_project(&self, name: &str, preset_id: Option<&str>, settings: Option<ProjectSettings>) -> AppResult<ProjectState> {
        let preset = preset_id.and_then(|id| self.presets().into_iter().find(|p| p.id == id));
        let settings = settings.or_else(|| preset.as_ref().map(|p| p.project_settings(None))).unwrap_or_default();
        let name = if name.trim().is_empty() { "Projeto sem título" } else { name.trim() };
        let mut p = Project::new(name, settings);
        p.preset_id = preset.map(|p| p.id);
        let issues = validate_project(&p);
        if !issues.is_empty() {
            return Err(AppError::new("invalid_settings", issues[0].message.clone()));
        }
        let paths = ProjectPaths::new(&self.0.cfg.library_root, p.id);
        paths.create_all()?;
        save_project_file(&paths.file(), &p)?;
        self.0.db.lock().unwrap().upsert_project(&p, &paths.dir, None, "idle")?;
        let session = EditSession::new(p);
        let st = state_of(&session, false);
        self.0.sessions.lock().unwrap().insert(st.project.id, Arc::new(Mutex::new(session)));
        Ok(st)
    }

    pub(crate) fn session(&self, id: Id) -> AppResult<Arc<Mutex<EditSession>>> {
        if let Some(s) = self.0.sessions.lock().unwrap().get(&id) {
            return Ok(s.clone());
        }
        self.open_project(id)?;
        self.0.sessions.lock().unwrap().get(&id).cloned().ok_or_else(|| AppError::new("not_found", "projeto não encontrado"))
    }

    pub fn open_project(&self, id: Id) -> AppResult<ProjectState> {
        if let Some(s) = self.0.sessions.lock().unwrap().get(&id) {
            return Ok(state_of(&s.lock().unwrap(), false));
        }
        let paths = ProjectPaths::new(&self.0.cfg.library_root, id);
        let (mut project, from_bak) = load_with_fallback(&paths)?;
        let mut recovered = from_bak;
        // Autosave mais recente que o arquivo salvo → recuperação após falha.
        if let (Ok(a), Ok(f)) = (std::fs::metadata(paths.autosave()), std::fs::metadata(paths.file())) {
            if a.modified()? > f.modified()? {
                if let Ok(auto) = load_project_file(&paths.autosave()) {
                    if auto != project {
                        project = auto;
                        recovered = true;
                    }
                }
            }
        }
        self.relocate_missing(&mut project);
        let mut session = EditSession::new(project);
        if recovered {
            session.touch();
        }
        let st = state_of(&session, recovered);
        self.0.sessions.lock().unwrap().insert(id, Arc::new(Mutex::new(session)));
        Ok(st)
    }

    /// Marca mídias ausentes e tenta relocalizá-las pela impressão digital
    /// (mesma pasta pai, até 2 níveis).
    fn relocate_missing(&self, p: &mut Project) {
        for a in &mut p.assets {
            if a.path.is_file() {
                if a.status == AssetStatus::Missing {
                    a.status = AssetStatus::Ready;
                }
                continue;
            }
            let found = a.path.parent().and_then(|d| d.parent().or(Some(d))).and_then(|root| probe::find_by_fingerprint(root, &a.fingerprint, 2));
            match found {
                Some(np) => {
                    tracing::info!(target: "avs::relink", "{} relocalizado em {}", a.name, np.display());
                    a.path = np;
                    a.status = AssetStatus::Ready;
                }
                None => a.status = AssetStatus::Missing,
            }
        }
    }

    /// Relocaliza manualmente uma mídia. Exige a mesma impressão digital,
    /// salvo `force` (arquivo re-exportado, por exemplo).
    pub fn relink_asset(&self, project_id: Id, asset_id: Id, new_path: &Path, force: bool) -> AppResult<ProjectState> {
        let fp = probe::fingerprint(new_path)?;
        let s = self.session(project_id)?;
        let mut s = s.lock().unwrap();
        let a = s.project.asset_mut(asset_id).ok_or_else(|| AppError::new("not_found", "mídia não encontrada"))?;
        if a.fingerprint != fp && !force {
            return Err(AppError::new("fingerprint_mismatch", "o arquivo selecionado não é o mesmo original (conteúdo diferente)"));
        }
        if a.fingerprint != fp {
            let info = probe::probe(&self.0.tools, new_path)?;
            if info.duration < a.duration && a.kind != AssetKind::Image {
                return Err(AppError::new("relink_shorter", "o novo arquivo é mais curto que o original; clipes ficariam inválidos"));
            }
            a.fingerprint = fp;
        }
        a.path = new_path.to_path_buf();
        a.status = AssetStatus::Ready;
        s.touch();
        self.autosave(&s)?;
        Ok(state_of(&s, false))
    }

    pub fn project_state(&self, id: Id) -> AppResult<ProjectState> {
        let s = self.session(id)?;
        let s = s.lock().unwrap();
        Ok(state_of(&s, false))
    }

    fn autosave(&self, s: &EditSession) -> AppResult<()> {
        let paths = ProjectPaths::new(&self.0.cfg.library_root, s.project.id);
        write_atomic(&paths.autosave(), &serde_json::to_vec(&s.project).map_err(|e| AppError::new("json", e.to_string()))?)?;
        self.0.events.emit(AppEvent::ProjectChanged { project_id: s.project.id, revision: s.revision() });
        Ok(())
    }

    pub fn save_project(&self, id: Id) -> AppResult<ProjectState> {
        let s = self.session(id)?;
        let mut s = s.lock().unwrap();
        let paths = ProjectPaths::new(&self.0.cfg.library_root, id);
        save_project_file(&paths.file(), &s.project)?;
        let _ = std::fs::remove_file(paths.autosave());
        let thumb = s.project.assets.iter().find_map(|a| a.poster_path.clone());
        self.0.db.lock().unwrap().upsert_project(&s.project, &paths.dir, thumb.as_deref(), "saved")?;
        s.mark_saved();
        Ok(state_of(&s, false))
    }

    pub fn rename_project(&self, id: Id, name: &str) -> AppResult<ProjectState> {
        let s = self.session(id)?;
        let mut s = s.lock().unwrap();
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::new("invalid_name", "o nome não pode ficar vazio"));
        }
        s.project.name = name.into();
        s.touch();
        self.autosave(&s)?;
        Ok(state_of(&s, false))
    }

    pub fn close_project(&self, id: Id) {
        self.0.sessions.lock().unwrap().remove(&id);
    }

    pub fn delete_project(&self, id: Id) -> AppResult<()> {
        self.close_project(id);
        let paths = ProjectPaths::new(&self.0.cfg.library_root, id);
        if paths.dir.starts_with(self.0.cfg.library_root.join("projects")) && paths.dir.is_dir() {
            std::fs::remove_dir_all(&paths.dir)?;
        }
        self.0.db.lock().unwrap().delete_project(id)?;
        Ok(())
    }

    pub fn create_snapshot(&self, id: Id, label: &str) -> AppResult<SnapshotInfo> {
        let s = self.session(id)?;
        let s = s.lock().unwrap();
        Ok(avs_store::create_snapshot(&ProjectPaths::new(&self.0.cfg.library_root, id), &s.project, label)?)
    }

    pub fn list_snapshots(&self, id: Id) -> AppResult<Vec<SnapshotInfo>> {
        Ok(avs_store::list_snapshots(&ProjectPaths::new(&self.0.cfg.library_root, id))?)
    }

    /// Restaura um snapshot como uma única operação desfazível.
    pub fn restore_snapshot(&self, id: Id, file_name: &str) -> AppResult<ProjectState> {
        let snap = avs_store::load_snapshot(&ProjectPaths::new(&self.0.cfg.library_root, id), file_name)?;
        let s = self.session(id)?;
        let mut s = s.lock().unwrap();
        // Mídias do snapshot que não estão mais no projeto são re-adicionadas.
        for a in &snap.assets {
            if s.project.asset(a.id).is_none() {
                s.project.assets.push(a.clone());
            }
        }
        s.replace("Restaurar versão", snap.timeline, snap.settings)?;
        s.touch();
        self.autosave(&s)?;
        Ok(state_of(&s, false))
    }

    // ───────────── edição ─────────────

    pub fn execute(&self, id: Id, cmd: &EditCommand) -> AppResult<(ApplyOutcome, ProjectState)> {
        let s = self.session(id)?;
        let mut s = s.lock().unwrap();
        let out = s.execute(cmd)?;
        self.autosave(&s)?;
        Ok((out, state_of(&s, false)))
    }

    pub fn execute_batch(&self, id: Id, label: &str, cmds: &[EditCommand]) -> AppResult<ProjectState> {
        let s = self.session(id)?;
        let mut s = s.lock().unwrap();
        s.execute_batch(label, cmds)?;
        self.autosave(&s)?;
        Ok(state_of(&s, false))
    }

    pub fn undo(&self, id: Id) -> AppResult<ProjectState> {
        let s = self.session(id)?;
        let mut s = s.lock().unwrap();
        s.undo();
        self.autosave(&s)?;
        Ok(state_of(&s, false))
    }

    pub fn redo(&self, id: Id) -> AppResult<ProjectState> {
        let s = self.session(id)?;
        let mut s = s.lock().unwrap();
        s.redo();
        self.autosave(&s)?;
        Ok(state_of(&s, false))
    }

    // ───────────── importação ─────────────

    /// Importa arquivos e/ou pastas. Metadados são lidos imediatamente (erros
    /// aparecem na hora); miniaturas, waveform e proxy rodam em job de fundo.
    pub fn import_media(&self, id: Id, paths: &[PathBuf]) -> AppResult<ImportResult> {
        let mut files = vec![];
        for p in paths {
            collect_media(p, 0, &mut files);
        }
        let mut res = ImportResult { assets: vec![], subtitle_tracks: vec![], skipped: vec![], errors: vec![] };
        let s = self.session(id)?;
        for f in files {
            if SubtitleFormat::from_path(&f).is_some() {
                match self.import_subtitles(id, &f) {
                    Ok(r) => res.subtitle_tracks.push(r.track_id),
                    Err(e) => res.errors.push(format!("{}: {}", f.display(), e.message)),
                }
                continue;
            }
            let fp = match probe::fingerprint(&f) {
                Ok(fp) => fp,
                Err(e) => {
                    res.errors.push(format!("{}: {e}", f.display()));
                    continue;
                }
            };
            if s.lock().unwrap().project.assets.iter().any(|a| a.fingerprint == fp) {
                res.skipped.push(format!("{} (já importado)", f.display()));
                continue;
            }
            match probe::probe(&self.0.tools, &f) {
                Ok(info) => {
                    let mut asset = probe::build_asset(&info, &f, fp);
                    asset.status = AssetStatus::Ready;
                    {
                        let mut g = s.lock().unwrap();
                        g.project.assets.push(asset.clone());
                        g.touch();
                        self.autosave(&g)?;
                    }
                    self.enqueue(JobPayload::Import { project_id: id, asset_id: asset.id }, &format!("Analisar {}", asset.name))?;
                    res.assets.push(asset);
                }
                Err(e) => res.errors.push(e.to_string()),
            }
        }
        Ok(res)
    }

    /// Importa um arquivo SRT/VTT como nova trilha de legendas (desfazível).
    pub fn import_subtitles(&self, id: Id, path: &Path) -> AppResult<SubtitleImport> {
        if SubtitleFormat::from_path(path).is_none() {
            return Err(AppError::new("subtitle_format", "use um arquivo .srt ou .vtt"));
        }
        let meta = std::fs::metadata(path)?;
        if meta.len() > 10 * 1024 * 1024 {
            return Err(AppError::new("subtitle_size", "arquivo de legenda maior que 10 MB"));
        }
        let bytes = std::fs::read(path)?;
        // UTF-8 com fallback para Latin-1 (comum em SRT antigos).
        let text = match String::from_utf8(bytes) {
            Ok(t) => t,
            Err(e) => e.into_bytes().iter().map(|&b| b as char).collect(),
        };
        let (cues, skipped) = avs_core::subtitles::parse(&text);
        if cues.is_empty() {
            return Err(AppError::new("subtitle_empty", "nenhuma legenda válida encontrada no arquivo"));
        }
        let name = path.file_stem().map(|n| n.to_string_lossy().into_owned());
        let count = cues.len() as u32;
        let (out, state) = self.execute(id, &EditCommand::AddSubtitleTrack { name, cues, style: None })?;
        Ok(SubtitleImport { track_id: out.created[0], cues: count, skipped: skipped as u32, state })
    }

    /// Exporta os textos de uma trilha como SRT ou VTT (formato pela extensão).
    pub fn export_subtitles(&self, id: Id, track_id: Id, path: &Path) -> AppResult<u32> {
        let fmt = SubtitleFormat::from_path(path).ok_or_else(|| AppError::new("subtitle_format", "o arquivo precisa terminar em .srt ou .vtt"))?;
        if !path.is_absolute() {
            return Err(AppError::new("invalid_path", "caminho precisa ser absoluto"));
        }
        let s = self.session(id)?;
        let s = s.lock().unwrap();
        let track = s.project.timeline.track(track_id).ok_or_else(|| AppError::new("not_found", "trilha não encontrada"))?;
        let cues = avs_core::subtitles::cues_from_track(track, Time::ZERO);
        if cues.is_empty() {
            return Err(AppError::new("subtitle_empty", "a trilha não tem textos para exportar"));
        }
        write_atomic(path, avs_core::subtitles::write(&cues, fmt).as_bytes())?;
        Ok(cues.len() as u32)
    }

    /// Remove uma mídia que não está em uso na timeline.
    pub fn remove_asset(&self, id: Id, asset_id: Id) -> AppResult<ProjectState> {
        let s = self.session(id)?;
        let mut s = s.lock().unwrap();
        if s.project.timeline.tracks.iter().flat_map(|t| &t.clips).any(|c| c.asset_id() == Some(asset_id)) {
            return Err(AppError::new("asset_in_use", "a mídia está em uso na timeline; remova os clipes antes"));
        }
        s.project.assets.retain(|a| a.id != asset_id);
        s.touch();
        self.autosave(&s)?;
        Ok(state_of(&s, false))
    }

    pub fn waveform(&self, id: Id, asset_id: Id, from: Time, to: Time, buckets: usize) -> AppResult<Vec<u8>> {
        let s = self.session(id)?;
        let path = s.lock().unwrap().project.asset(asset_id).and_then(|a| a.waveform_path.clone());
        let Some(path) = path else { return Ok(vec![]) };
        let peaks = std::fs::read(path)?;
        Ok(derive::waveform_slice(&peaks, from, to, buckets.min(8192)))
    }

    // ───────────── preview ─────────────

    /// Renderiza um frame da timeline pelo mesmo compilador da exportação.
    pub fn render_frame(&self, id: Id, at: Time, max_height: u32) -> AppResult<PathBuf> {
        let (project, rev) = {
            let s = self.session(id)?;
            let s = s.lock().unwrap();
            (s.project.clone(), s.revision())
        };
        let opts = preview_opts(&project, max_height);
        let fi = at.frame_floor(project.settings.fps);
        let paths = ProjectPaths::new(&self.0.cfg.library_root, id);
        let dir = paths.work().join("frames");
        std::fs::create_dir_all(&dir)?;
        let out = dir.join(format!("r{rev}-f{fi}-h{}.jpg", opts.height));
        if !out.is_file() {
            render_frame(&self.0.tools, &self.0.fonts, &project, at, &opts, &paths.work(), &out)?;
            prune_dir(&dir, 400);
        }
        Ok(out)
    }

    /// Enfileira o preview completo (cancela previews anteriores do projeto).
    pub fn request_preview(&self, id: Id) -> AppResult<Id> {
        self.request_preview_at(id, None)
    }

    /// Preview para reprodução. Timelines curtas: inteira. Longas (> 4 min):
    /// uma janela de ~2 min a partir de `at`, alinhada à grade de blocos de
    /// 5 s (reaproveita o cache entre janelas e com o preview completo).
    pub fn request_preview_at(&self, id: Id, at: Option<Time>) -> AppResult<Id> {
        let (rev, total) = {
            let s = self.session(id)?;
            let s = s.lock().unwrap();
            (s.revision(), s.project.timeline.duration())
        };
        let range = preview_window(total, at);
        {
            let mut q = self.0.queue.lock().unwrap();
            let stale: Vec<Id> = q
                .records
                .values()
                .filter(|r| r.kind == "preview" && r.project_id == Some(id) && (r.state == QUEUED || r.state == RUNNING))
                .map(|r| r.id)
                .collect();
            for j in stale {
                if let Some(ctl) = q.running.get(&j) {
                    ctl.cancel.cancel();
                } else {
                    q.pending.retain(|(_, _, x)| *x != j);
                    if let Some(r) = q.records.get_mut(&j) {
                        r.state = CANCELLED.into();
                        let r = r.clone();
                        let _ = self.0.db.lock().unwrap().update_job(&r);
                        self.0.events.emit(AppEvent::Job { job: r });
                    }
                }
            }
        }
        let title = match range {
            Some(r) => format!("Gerar preview ({}–{})", mmss(r.start), mmss(r.end)),
            None => "Gerar preview".into(),
        };
        self.enqueue(JobPayload::Preview { project_id: id, revision: rev, range }, &title)
    }

    // ───────────── exportação ─────────────

    pub fn export(&self, id: Id, req: ExportRequest) -> AppResult<Id> {
        let project = self.session(id)?.lock().unwrap().project.clone();
        if !project.timeline.duration().is_positive() {
            return Err(AppError::new("empty_timeline", "a timeline está vazia"));
        }
        if req.file_name.trim().is_empty() {
            return Err(AppError::new("invalid_name", "informe um nome de arquivo"));
        }
        if !req.output_dir.is_absolute() {
            return Err(AppError::new("invalid_path", "a pasta de destino precisa ser um caminho absoluto"));
        }
        let title = format!("Exportar {}", req.file_name);
        self.enqueue(JobPayload::Export { project_id: id, request: req, project: Box::new(project) }, &title)
    }

    pub fn list_exports(&self, project: Option<Id>) -> AppResult<Vec<ExportRecord>> {
        Ok(self.0.db.lock().unwrap().list_exports(project)?)
    }

    // ───────────── jobs ─────────────

    pub fn jobs(&self) -> AppResult<Vec<JobRecord>> {
        let q = self.0.queue.lock().unwrap();
        let mut v: Vec<JobRecord> = q.records.values().cloned().collect();
        drop(q);
        let known: std::collections::HashSet<Id> = v.iter().map(|j| j.id).collect();
        v.extend(self.0.db.lock().unwrap().list_jobs(100)?.into_iter().filter(|j| !known.contains(&j.id)));
        v.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(v)
    }

    pub fn cancel_job(&self, job: Id) -> AppResult<()> {
        let mut q = self.0.queue.lock().unwrap();
        if let Some(ctl) = q.running.get(&job) {
            ctl.pause.store(false, Ordering::SeqCst);
            ctl.cancel.cancel();
            return Ok(());
        }
        q.pending.retain(|(_, _, j)| *j != job);
        if let Some(r) = q.records.get_mut(&job) {
            if r.state == QUEUED || r.state == PAUSED {
                r.state = CANCELLED.into();
                r.finished_at = Some(now_rfc3339());
                let r = r.clone();
                drop(q);
                self.0.db.lock().unwrap().update_job(&r)?;
                self.0.events.emit(AppEvent::Job { job: r });
            }
        }
        Ok(())
    }

    /// Pausa: o FFmpeg em execução é interrompido; blocos concluídos ficam no
    /// cache e são reaproveitados ao continuar.
    pub fn pause_job(&self, job: Id) -> AppResult<()> {
        let mut q = self.0.queue.lock().unwrap();
        if let Some(ctl) = q.running.get(&job) {
            ctl.pause.store(true, Ordering::SeqCst);
            ctl.cancel.cancel();
            return Ok(());
        }
        let was_pending = q.pending.iter().any(|(_, _, j)| *j == job);
        q.pending.retain(|(_, _, j)| *j != job);
        if was_pending {
            if let Some(r) = q.records.get_mut(&job) {
                r.state = PAUSED.into();
                let r = r.clone();
                drop(q);
                self.0.db.lock().unwrap().update_job(&r)?;
                self.0.events.emit(AppEvent::Job { job: r });
            }
        }
        Ok(())
    }

    /// Continua um job pausado ou repete um job com erro/cancelado.
    pub fn resume_job(&self, job: Id) -> AppResult<()> {
        let mut q = self.0.queue.lock().unwrap();
        let rec = match q.records.get(&job) {
            Some(r) => r.clone(),
            None => {
                drop(q);
                let r = self
                    .0
                    .db
                    .lock()
                    .unwrap()
                    .list_jobs(500)?
                    .into_iter()
                    .find(|r| r.id == job)
                    .ok_or_else(|| AppError::new("not_found", "job não encontrado"))?;
                q = self.0.queue.lock().unwrap();
                r
            }
        };
        if ![PAUSED, CANCELLED, FAILED, FAILED_RECOVERABLE].contains(&rec.state.as_str()) {
            return Err(AppError::new("job_state", format!("não é possível retomar um job em estado '{}'", rec.state)));
        }
        let mut r = rec;
        r.state = QUEUED.into();
        r.error = None;
        r.finished_at = None;
        q.seq += 1;
        let seq = q.seq;
        q.pending.push((r.priority, seq, r.id));
        q.records.insert(r.id, r.clone());
        drop(q);
        self.0.db.lock().unwrap().update_job(&r)?;
        self.0.events.emit(AppEvent::Job { job: r });
        self.0.cv.notify_all();
        Ok(())
    }

    pub fn remove_job(&self, job: Id) -> AppResult<()> {
        let mut q = self.0.queue.lock().unwrap();
        if q.running.contains_key(&job) {
            return Err(AppError::new("job_running", "cancele o job antes de removê-lo"));
        }
        q.pending.retain(|(_, _, j)| *j != job);
        q.records.remove(&job);
        drop(q);
        self.0.db.lock().unwrap().delete_job(job)?;
        Ok(())
    }

    /// Bloqueia até o job terminar (CLI e testes).
    pub fn wait_job(&self, job: Id, timeout: Duration) -> AppResult<JobRecord> {
        let start = Instant::now();
        let mut q = self.0.queue.lock().unwrap();
        loop {
            if let Some(r) = q.records.get(&job) {
                if [COMPLETED, CANCELLED, FAILED, FAILED_RECOVERABLE, PAUSED].contains(&r.state.as_str()) {
                    return Ok(r.clone());
                }
            } else {
                return Err(AppError::new("not_found", "job não encontrado"));
            }
            let left = timeout.checked_sub(start.elapsed()).ok_or_else(|| AppError::new("timeout", "tempo de espera esgotado"))?;
            q = self.0.cv.wait_timeout(q, left.min(Duration::from_millis(200))).unwrap().0;
        }
    }

    pub fn wait_idle(&self, timeout: Duration) -> AppResult<()> {
        let start = Instant::now();
        loop {
            {
                let q = self.0.queue.lock().unwrap();
                if q.pending.is_empty() && q.running.is_empty() {
                    return Ok(());
                }
            }
            if start.elapsed() > timeout {
                return Err(AppError::new("timeout", "fila não esvaziou a tempo"));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    pub(crate) fn enqueue(&self, payload: JobPayload, title: &str) -> AppResult<Id> {
        let rec = JobRecord {
            id: new_id(),
            project_id: payload.project_id(),
            kind: payload.kind().into(),
            title: title.into(),
            state: QUEUED.into(),
            stage: None,
            progress: 0.0,
            priority: payload.priority(),
            created_at: now_rfc3339(),
            started_at: None,
            finished_at: None,
            error: None,
            payload: serde_json::to_value(&payload).map_err(|e| AppError::new("json", e.to_string()))?,
            result: None,
        };
        self.0.db.lock().unwrap().insert_job(&rec)?;
        let mut q = self.0.queue.lock().unwrap();
        q.seq += 1;
        let seq = q.seq;
        q.pending.push((rec.priority, seq, rec.id));
        q.records.insert(rec.id, rec.clone());
        drop(q);
        self.0.events.emit(AppEvent::Job { job: rec.clone() });
        self.0.cv.notify_all();
        Ok(rec.id)
    }

    fn recover_jobs(&self) -> AppResult<()> {
        let unfinished = self.0.db.lock().unwrap().unfinished_jobs()?;
        let mut q = self.0.queue.lock().unwrap();
        for mut r in unfinished {
            // Preview antigo não interessa; exportação e análise são retomadas.
            if r.kind == "preview" {
                r.state = CANCELLED.into();
                self.0.db.lock().unwrap().update_job(&r)?;
                continue;
            }
            if r.state != PAUSED {
                r.state = QUEUED.into();
                r.stage = Some("retomado após reinício".into());
                q.seq += 1;
                let seq = q.seq;
                q.pending.push((r.priority, seq, r.id));
            }
            self.0.db.lock().unwrap().update_job(&r)?;
            q.records.insert(r.id, r);
        }
        Ok(())
    }

    fn worker_loop(&self) {
        loop {
            let (rec, ctl) = {
                let mut q = self.0.queue.lock().unwrap();
                loop {
                    if self.0.shutting_down.load(Ordering::SeqCst) {
                        return;
                    }
                    // Limite de concorrência: no máximo uma exportação por vez (RAM/VRAM).
                    let exports_running = q.running.keys().filter(|id| q.records.get(id).map(|r| r.kind == "export").unwrap_or(false)).count();
                    let mut cands: Vec<(i32, u64, Id)> = q.pending.clone();
                    cands.sort();
                    let pick = cands.into_iter().find(|(_, _, id)| {
                        let kind = q.records.get(id).map(|r| r.kind.as_str()).unwrap_or("");
                        kind != "export" || exports_running == 0
                    });
                    if let Some((_, _, id)) = pick {
                        q.pending.retain(|(_, _, x)| *x != id);
                        let ctl = RunningCtl { cancel: CancelToken::new(), pause: Arc::new(AtomicBool::new(false)) };
                        let handle = (ctl.cancel.clone(), ctl.pause.clone());
                        q.running.insert(id, ctl);
                        let r = q.records.get_mut(&id).unwrap();
                        r.state = RUNNING.into();
                        r.started_at = Some(now_rfc3339());
                        r.error = None;
                        break (r.clone(), handle);
                    }
                    q = self.0.cv.wait_timeout(q, Duration::from_millis(500)).unwrap().0;
                }
            };
            let _ = self.0.db.lock().unwrap().update_job(&rec);
            self.0.events.emit(AppEvent::Job { job: rec.clone() });

            let result = self.run_job(&rec, &ctl.0);

            let mut q = self.0.queue.lock().unwrap();
            q.running.remove(&rec.id);
            let shutting = self.0.shutting_down.load(Ordering::SeqCst);
            let r = q.records.get_mut(&rec.id).unwrap();
            match result {
                Ok(v) => {
                    r.state = COMPLETED.into();
                    r.progress = 1.0;
                    r.stage = Some("finalizado".into());
                    r.result = Some(v);
                    r.finished_at = Some(now_rfc3339());
                }
                Err(e) if e.code == "cancelled" && shutting => {
                    r.state = QUEUED.into(); // retomado no próximo início
                }
                Err(e) if e.code == "cancelled" && ctl.1.load(Ordering::SeqCst) => {
                    r.state = PAUSED.into();
                    r.stage = Some("pausado".into());
                }
                Err(e) if e.code == "cancelled" => {
                    r.state = CANCELLED.into();
                    r.finished_at = Some(now_rfc3339());
                }
                Err(e) => {
                    let recoverable = matches!(e.code.as_str(), "media" | "io" | "timeout" | "store" | "network" | "ai_worker");
                    r.state = if recoverable { FAILED_RECOVERABLE } else { FAILED }.into();
                    r.error = Some(e.message);
                    r.finished_at = Some(now_rfc3339());
                }
            }
            let r = r.clone();
            drop(q);
            let _ = self.0.db.lock().unwrap().update_job(&r);
            self.0.events.emit(AppEvent::Job { job: r });
            self.0.cv.notify_all();
        }
    }

    /// Atualiza estágio/progresso (eventos limitados a ~10/s por job).
    fn progress(&self, job: Id, stage: &str, fraction: f32, last: &Mutex<(Instant, String)>) {
        let mut l = last.lock().unwrap();
        let now = Instant::now();
        let stage_changed = l.1 != stage;
        if !stage_changed && now.duration_since(l.0) < Duration::from_millis(100) && fraction < 1.0 {
            return;
        }
        let persist = stage_changed || now.duration_since(l.0) > Duration::from_secs(2);
        *l = (now, stage.to_string());
        drop(l);
        let mut q = self.0.queue.lock().unwrap();
        let Some(r) = q.records.get_mut(&job) else { return };
        r.stage = Some(stage.into());
        r.progress = fraction.clamp(0.0, 1.0);
        let r = r.clone();
        drop(q);
        if persist {
            let _ = self.0.db.lock().unwrap().update_job(&r);
        }
        self.0.events.emit(AppEvent::Job { job: r });
    }

    fn run_job(&self, rec: &JobRecord, cancel: &CancelToken) -> AppResult<serde_json::Value> {
        let payload: JobPayload = serde_json::from_value(rec.payload.clone()).map_err(|e| AppError::new("job_payload", e.to_string()))?;
        let last = Mutex::new((Instant::now() - Duration::from_secs(1), String::new()));
        let prog = |stage: &str, f: f32| self.progress(rec.id, stage, f, &last);
        match payload {
            JobPayload::Import { project_id, asset_id } => self.run_import(project_id, asset_id, cancel, &prog),
            JobPayload::Preview { project_id, revision, range } => self.run_preview(project_id, revision, range, cancel, &prog),
            JobPayload::Export { project_id, request, project } => self.run_export(project_id, &request, &project, cancel, &prog),
            JobPayload::Analyze { project_id, asset_id, options } => self.run_analysis(project_id, asset_id, &options, cancel, &prog),
            JobPayload::AiInstall { component } => self.run_ai_install(&component, cancel, &prog),
            JobPayload::OllamaPull { model } => self.run_ollama_pull(&model, cancel, &prog),
            JobPayload::AutoEdit { project_id, options } => self.run_auto_edit(project_id, &options, cancel, &prog),
            JobPayload::Shorts { project_id, request } => self.run_shorts(project_id, &request, cancel, &prog),
        }
    }

    fn update_asset(&self, project_id: Id, asset: &MediaAsset) -> AppResult<()> {
        let s = self.session(project_id)?;
        let mut s = s.lock().unwrap();
        if let Some(a) = s.project.asset_mut(asset.id) {
            // Preserva o caminho/estado atuais (podem ter sido relocalizados).
            let (path, status) = (a.path.clone(), a.status.clone());
            *a = asset.clone();
            a.path = path;
            a.status = status;
        }
        s.touch();
        self.autosave(&s)?;
        drop(s);
        self.0.events.emit(AppEvent::Asset { project_id, asset: asset.clone() });
        Ok(())
    }

    fn run_import(&self, project_id: Id, asset_id: Id, cancel: &CancelToken, prog: &(dyn Fn(&str, f32) + Sync)) -> AppResult<serde_json::Value> {
        let mut asset = self
            .session(project_id)?
            .lock()
            .unwrap()
            .project
            .asset(asset_id)
            .cloned()
            .ok_or_else(|| AppError::new("not_found", "mídia removida do projeto"))?;
        let paths = ProjectPaths::new(&self.0.cfg.library_root, project_id);
        let dir = paths.asset_cache(asset_id);
        std::fs::create_dir_all(&dir)?;
        let par = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).min(4);
        let mut warnings: Vec<String> = vec![];

        if asset.has_video() {
            prog("analisando: miniaturas", 0.05);
            let poster = dir.join("poster.jpg");
            match derive::make_poster(&self.0.tools, &asset, &poster) {
                Ok(()) => asset.poster_path = Some(poster),
                Err(e) => warnings.push(format!("poster: {e}")),
            }
            if asset.kind == AssetKind::Video {
                match derive::make_thumb_strip(&self.0.tools, &asset, &dir, 120, par) {
                    Ok(t) => asset.thumbs = Some(t),
                    Err(e) => warnings.push(format!("miniaturas: {e}")),
                }
            }
            self.update_asset(project_id, &asset)?;
        }
        if cancel.is_cancelled() {
            return Err(MediaError::Cancelled.into());
        }
        if asset.has_audio() {
            prog("analisando: waveform", 0.35);
            let wf = dir.join("waveform.u8");
            derive::make_waveform(&self.0.tools, &asset, &wf, cancel)?;
            asset.waveform_path = Some(wf);
            self.update_asset(project_id, &asset)?;
        }
        if self.0.cfg.generate_proxies && derive::needs_proxy(&asset) {
            prog("gerando proxy", 0.5);
            let out = paths.proxies().join(format!("{asset_id}.mp4"));
            derive::make_proxy(&self.0.tools, &asset, &out, cancel, |f| prog("gerando proxy", 0.5 + 0.5 * f))?;
            asset.proxy_path = Some(out);
            self.update_asset(project_id, &asset)?;
        }
        // Thumbnail do projeto no dashboard.
        if let Some(p) = &asset.poster_path {
            let s = self.session(project_id)?;
            let s = s.lock().unwrap();
            let _ = self.0.db.lock().unwrap().upsert_project(&s.project, &paths.dir, Some(p), "editing");
        }
        Ok(serde_json::json!({ "asset_id": asset_id, "warnings": warnings }))
    }

    fn run_preview(
        &self,
        project_id: Id,
        revision: u64,
        range: Option<TimeRange>,
        cancel: &CancelToken,
        prog: &(dyn Fn(&str, f32) + Sync),
    ) -> AppResult<serde_json::Value> {
        let project = self.session(project_id)?.lock().unwrap().project.clone();
        let paths = ProjectPaths::new(&self.0.cfg.library_root, project_id);
        let out = match range {
            Some(r) => paths.renders().join(format!("preview-r{revision}-{}.mp4", (r.start.as_secs_f64() * 1000.0).round() as i64)),
            None => paths.renders().join(format!("preview-r{revision}.mp4")),
        };
        let mut opts = preview_opts(&project, 540);
        opts.use_proxies = true;
        let req = RenderRequest {
            project,
            range,
            compile: opts,
            video: VideoEncode { encoder: "libx264".into(), quality: 26, speed: "ultrafast".into(), bitrate_kbps: None },
            audio: AudioEncode { bitrate_kbps: 128, loudness_lufs: None },
            container: Container::Mp4,
            chunk_len: Time::from_secs(5),
            parallel: 2,
            chunk_dir: paths.chunks("preview"),
            work_dir: paths.work().join(format!("preview-{}", new_id().simple())),
            output: out.clone(),
        };
        let res = render(&self.0.tools, &self.0.fonts, &req, cancel, &|p: RenderProgress| {
            prog(if p.fraction < 0.92 { "gerando preview" } else { "finalizando preview" }, p.fraction)
        });
        let _ = std::fs::remove_dir_all(&req.work_dir);
        let res = res?;
        // Remove previews de outras revisões e janelas antigas (mantém as 3 mais
        // recentes desta revisão: a que está tocando pode ser a anterior).
        if let Ok(rd) = std::fs::read_dir(paths.renders()) {
            let cur = format!("preview-r{revision}");
            let mut same: Vec<(std::time::SystemTime, PathBuf)> = vec![];
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if !n.starts_with("preview-r") || e.path() == out {
                    continue;
                }
                let is_cur = n == format!("{cur}.mp4") || n.starts_with(&format!("{cur}-"));
                if is_cur {
                    same.push((e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), e.path()));
                } else {
                    let _ = std::fs::remove_file(e.path());
                }
            }
            same.sort_by_key(|x| std::cmp::Reverse(x.0));
            for (_, p) in same.into_iter().skip(2) {
                let _ = std::fs::remove_file(p);
            }
        }
        self.0.events.emit(AppEvent::PreviewReady { project_id, path: out.clone(), revision, range });
        Ok(
            serde_json::json!({ "path": out, "revision": revision, "range": range, "chunks_total": res.chunks_total, "chunks_reused": res.chunks_reused, "elapsed_ms": res.elapsed_ms }),
        )
    }

    fn run_export(
        &self,
        project_id: Id,
        rq: &ExportRequest,
        project: &Project,
        cancel: &CancelToken,
        prog: &(dyn Fn(&str, f32) + Sync),
    ) -> AppResult<serde_json::Value> {
        prog("preparando", 0.0);
        let encoders = self.encoders();
        let encoder = match &rq.encoder {
            Some(e) => e.clone(),
            None => best_encoder(&encoders, &rq.codec, rq.prefer_hardware)
                .map(|e| e.name.clone())
                .ok_or_else(|| AppError::new("no_encoder", format!("nenhum encoder funcional para {}", rq.codec)))?,
        };
        let stem = sanitize_file_stem(rq.file_name.trim_end_matches(&format!(".{}", rq.container.ext())), 120);
        std::fs::create_dir_all(&rq.output_dir)?;
        let mut output = rq.output_dir.join(format!("{stem}.{}", rq.container.ext()));
        let mut n = 2;
        while output.exists() {
            output = rq.output_dir.join(format!("{stem} ({n}).{}", rq.container.ext()));
            n += 1;
        }
        let (w, h) = match (rq.width, rq.height) {
            (Some(w), Some(h)) => (w & !1, h & !1),
            _ => (project.settings.width, project.settings.height),
        };
        let fps = rq.fps.unwrap_or(project.settings.fps);
        let paths = ProjectPaths::new(&self.0.cfg.library_root, project_id);
        let make_req = |enc: &str| RenderRequest {
            project: project.clone(),
            range: rq.range,
            compile: CompileOptions { width: w, height: h, fps, sample_rate: project.settings.sample_rate, use_proxies: false, include_audio: true },
            video: VideoEncode { encoder: enc.into(), quality: rq.quality, speed: rq.speed.clone(), bitrate_kbps: rq.bitrate_kbps },
            audio: AudioEncode { bitrate_kbps: rq.audio_bitrate_kbps, loudness_lufs: rq.loudness_lufs },
            container: rq.container,
            chunk_len: Time::from_secs(30),
            parallel: {
                let mut sys = sysinfo::System::new();
                sys.refresh_memory();
                let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
                avs_media::render::plan_parallelism(w, h, enc, &rq.speed, cores, sys.available_memory() / 1_048_576).0
            },
            chunk_dir: paths.chunks("export"),
            work_dir: paths.work().join(format!("export-{}", new_id().simple())),
            output: output.clone(),
        };
        let on = |p: RenderProgress| prog(if p.fraction < 0.92 { "renderizando" } else { "finalizando arquivo" }, p.fraction * 0.9);
        let mut used = encoder.clone();
        let req = make_req(&encoder);
        let mut res = render(&self.0.tools, &self.0.fonts, &req, cancel, &on);
        let _ = std::fs::remove_dir_all(&req.work_dir);
        // Encoder de hardware falhou em tempo de execução → repete em software.
        if let Err(MediaError::Failed { .. }) = &res {
            if !encoder.starts_with("lib") {
                let sw = best_encoder(&encoders, &rq.codec, false).map(|e| e.name.clone());
                if let Some(sw) = sw {
                    tracing::warn!(target: "avs::export", "encoder {encoder} falhou; repetindo com {sw}");
                    used = sw.clone();
                    let req = make_req(&sw);
                    res = render(&self.0.tools, &self.0.fonts, &req, cancel, &on);
                    let _ = std::fs::remove_dir_all(&req.work_dir);
                }
            }
        }
        let res = res?;

        prog("validando", 0.9);
        let range_start = rq.range.map(|r| r.start).unwrap_or(Time::ZERO);
        let shift = |v: Vec<TimeRange>| -> Vec<TimeRange> { v.into_iter().map(|r| TimeRange::new(r.start - range_start, r.end - range_start)).collect() };
        let exp = Expectation {
            duration: res.duration,
            width: w,
            height: h,
            fps,
            video_codec: codec_family(&used),
            audio: true,
            covered_video: shift(video_coverage(project)),
            covered_audio: shift(audio_coverage(project)),
        };
        let report = validate_output(&self.0.tools, &output, &exp, self.0.cfg.deep_validation, cancel)?;
        if !report.ok {
            let failed: Vec<String> =
                report.checks.iter().filter(|c| c.status == avs_media::validate::CheckStatus::Fail).map(|c| format!("{}: {}", c.label, c.detail)).collect();
            let bad = output.with_extension(format!("invalido.{}", rq.container.ext()));
            let _ = std::fs::rename(&output, &bad);
            return Err(AppError::new(
                "validation_failed",
                format!("o arquivo exportado não passou na validação ({}); mantido como {}", failed.join("; "), bad.display()),
            ));
        }
        let size = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);
        let rec = ExportRecord {
            id: new_id(),
            project_id,
            path: output.clone(),
            created_at: now_rfc3339(),
            duration: res.duration,
            width: w,
            height: h,
            codec: used.clone(),
            size_bytes: size,
            report: serde_json::to_value(&report).unwrap_or_default(),
        };
        self.0.db.lock().unwrap().insert_export(&rec)?;
        prog("concluído", 1.0);
        let out = ExportResult {
            path: output,
            encoder: used,
            report,
            duration: res.duration,
            chunks_total: res.chunks_total,
            chunks_reused: res.chunks_reused,
            elapsed_ms: res.elapsed_ms,
        };
        Ok(serde_json::to_value(out).unwrap_or_default())
    }
}

/// Duração acima da qual o preview é feito por janelas.
pub const PREVIEW_WINDOW_ABOVE: Time = Time::from_secs(240);
/// Tamanho da janela de preview e grade de alinhamento (= bloco do preview).
pub const PREVIEW_WINDOW: Time = Time::from_secs(120);
const PREVIEW_GRID: Time = Time::from_secs(5);

/// Janela de preview para uma timeline de duração `total` a partir de `at`.
pub fn preview_window(total: Time, at: Option<Time>) -> Option<TimeRange> {
    if total <= PREVIEW_WINDOW_ABOVE {
        return None;
    }
    let at = at.unwrap_or(Time::ZERO).clamp(Time::ZERO, total);
    let g = PREVIEW_GRID.flicks();
    // Começa um pouco antes do cursor, na grade de 5 s.
    let start = Time::from_flicks(((at - Time::from_secs(5)).max(Time::ZERO).flicks() / g) * g);
    let start = start.min(Time::from_flicks(((total - PREVIEW_WINDOW).max(Time::ZERO).flicks() / g) * g));
    Some(TimeRange::new(start, (start + PREVIEW_WINDOW).min(total)))
}

fn mmss(t: Time) -> String {
    let s = t.as_secs_f64().max(0.0) as i64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn state_of(s: &EditSession, recovered: bool) -> ProjectState {
    ProjectState { project: s.project.clone(), history: s.info(), dirty: s.is_dirty(), recovered }
}

fn preview_opts(p: &Project, max_h: u32) -> CompileOptions {
    let mut o = CompileOptions::from_project(p);
    let max_h = max_h.max(90);
    if o.height > max_h {
        let w = even(o.width as f64 * max_h as f64 / o.height as f64);
        o.width = w;
        o.height = max_h & !1;
    }
    o
}

fn collect_media(p: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if p.is_dir() {
        if depth > 3 {
            return;
        }
        let mut entries: Vec<PathBuf> = std::fs::read_dir(p).map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default();
        entries.sort();
        for e in entries {
            collect_media(&e, depth + 1, out);
        }
    } else if p.is_file() {
        let ok = p.extension().and_then(|e| e.to_str()).map(|e| MEDIA_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str())).unwrap_or(false);
        // Arquivos passados explicitamente são tentados mesmo com extensão incomum.
        if ok || depth == 0 {
            out.push(p.to_path_buf());
        }
    }
}

fn prune_dir(dir: &Path, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd.flatten().filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path()))).collect();
    if files.len() <= keep {
        return;
    }
    files.sort();
    for (_, p) in files.iter().take(files.len() - keep) {
        let _ = std::fs::remove_file(p);
    }
}

/// Diretório de dados padrão da biblioteca (CLI). O app desktop usa o
/// diretório de dados do Tauri.
pub fn default_library_root() -> PathBuf {
    if let Some(p) = std::env::var_os("AVS_LIBRARY") {
        return PathBuf::from(p);
    }
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"));
    #[cfg(all(unix, not(target_os = "macos")))]
    let base = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
    base.unwrap_or_else(|| PathBuf::from(".")).join("ai-video-studio")
}
