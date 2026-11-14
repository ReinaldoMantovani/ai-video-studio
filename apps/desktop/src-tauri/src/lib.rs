//! Ponte Tauri ↔ camada de serviço (`avs-app`).
//!
//! Cada comando roda em `spawn_blocking` (FFmpeg/disco nunca bloqueiam a
//! thread da interface). Erros chegam ao frontend como `{ code, message }`.
//! Eventos do serviço (jobs, mídia, preview) são emitidos em `avs://event`.

use avs_app::presets::{EditorialPreset, ExportProfile};
use avs_app::system::SystemInfo;
use avs_app::*;
use avs_core::*;
use avs_media::hwaccel::EncoderInfo;
use avs_store::{ExportRecord, JobRecord, ProjectSummary, SnapshotInfo};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{Emitter, Manager, State};

struct TauriSink(tauri::AppHandle);
impl EventSink for TauriSink {
    fn emit(&self, e: AppEvent) {
        let _ = self.0.emit("avs://event", e);
    }
}

type Res<T> = Result<T, AppError>;

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Res<T> + Send + 'static) -> Res<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| AppError::new("internal", e.to_string()))?
}

fn studio(s: &State<'_, Studio>) -> Studio {
    s.inner().clone()
}

#[tauri::command]
async fn library_root(s: State<'_, Studio>) -> Res<PathBuf> {
    Ok(s.library_root().to_path_buf())
}

/// Erro de interface capturado pelo ErrorBoundary: vai para `logs/ui-errors.log`
/// da biblioteca, para diagnóstico (o app gráfico não tem console).
#[tauri::command]
async fn report_ui_error(s: State<'_, Studio>, message: String, stack: String) -> Res<String> {
    use std::io::Write;
    let dir = s.library_root().join("logs");
    let path = dir.join("ui-errors.log");
    tracing::error!(target: "avs::ui", %message, "erro de interface");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let when = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let msg: String = message.chars().take(2000).collect();
        let st: String = stack.chars().take(8000).collect();
        let _ = writeln!(f, "--- {when} (unix) v{}\n{msg}\n{st}\n", env!("CARGO_PKG_VERSION"));
    }
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
async fn system_info(s: State<'_, Studio>) -> Res<SystemInfo> {
    let st = studio(&s);
    blocking(move || Ok(st.system_info())).await
}

#[tauri::command]
async fn encoders(s: State<'_, Studio>) -> Res<Vec<EncoderInfo>> {
    let st = studio(&s);
    blocking(move || Ok(st.encoders())).await
}

#[tauri::command]
async fn presets(s: State<'_, Studio>) -> Res<Vec<EditorialPreset>> {
    Ok(s.presets())
}

#[tauri::command]
async fn save_user_preset(s: State<'_, Studio>, preset: EditorialPreset) -> Res<EditorialPreset> {
    let st = studio(&s);
    blocking(move || st.save_user_preset(preset)).await
}

#[tauri::command]
async fn export_profiles(s: State<'_, Studio>) -> Res<Vec<ExportProfile>> {
    Ok(s.export_profiles())
}

#[tauri::command]
async fn list_projects(s: State<'_, Studio>) -> Res<Vec<ProjectSummary>> {
    let st = studio(&s);
    blocking(move || st.list_projects()).await
}

#[tauri::command]
async fn create_project(s: State<'_, Studio>, name: String, preset_id: Option<String>, settings: Option<ProjectSettings>) -> Res<ProjectState> {
    let st = studio(&s);
    blocking(move || st.create_project(&name, preset_id.as_deref(), settings)).await
}

#[tauri::command]
async fn open_project(s: State<'_, Studio>, id: Id) -> Res<ProjectState> {
    let st = studio(&s);
    blocking(move || st.open_project(id)).await
}

#[tauri::command]
async fn save_project(s: State<'_, Studio>, id: Id) -> Res<ProjectState> {
    let st = studio(&s);
    blocking(move || st.save_project(id)).await
}

#[tauri::command]
async fn rename_project(s: State<'_, Studio>, id: Id, name: String) -> Res<ProjectState> {
    let st = studio(&s);
    blocking(move || st.rename_project(id, &name)).await
}

#[tauri::command]
async fn close_project(s: State<'_, Studio>, id: Id) -> Res<()> {
    s.close_project(id);
    Ok(())
}

#[tauri::command]
async fn delete_project(s: State<'_, Studio>, id: Id) -> Res<()> {
    let st = studio(&s);
    blocking(move || st.delete_project(id)).await
}

#[tauri::command]
async fn import_media(s: State<'_, Studio>, id: Id, paths: Vec<PathBuf>) -> Res<ImportResult> {
    let st = studio(&s);
    blocking(move || st.import_media(id, &paths)).await
}

#[tauri::command]
async fn import_subtitles(s: State<'_, Studio>, id: Id, path: PathBuf) -> Res<SubtitleImport> {
    let st = studio(&s);
    blocking(move || st.import_subtitles(id, &path)).await
}

#[tauri::command]
async fn export_subtitles(s: State<'_, Studio>, id: Id, track_id: Id, path: PathBuf) -> Res<u32> {
    let st = studio(&s);
    blocking(move || st.export_subtitles(id, track_id, &path)).await
}

#[tauri::command]
async fn remove_asset(s: State<'_, Studio>, id: Id, asset_id: Id) -> Res<ProjectState> {
    let st = studio(&s);
    blocking(move || st.remove_asset(id, asset_id)).await
}

#[tauri::command]
async fn relink_asset(s: State<'_, Studio>, id: Id, asset_id: Id, path: PathBuf, force: bool) -> Res<ProjectState> {
    let st = studio(&s);
    blocking(move || st.relink_asset(id, asset_id, &path, force)).await
}

#[derive(serde::Serialize)]
struct ExecuteResult {
    outcome: ApplyOutcome,
    state: ProjectState,
}

#[tauri::command]
async fn execute(s: State<'_, Studio>, id: Id, command: EditCommand) -> Res<ExecuteResult> {
    let st = studio(&s);
    blocking(move || st.execute(id, &command).map(|(outcome, state)| ExecuteResult { outcome, state })).await
}

#[tauri::command]
async fn execute_batch(s: State<'_, Studio>, id: Id, label: String, commands: Vec<EditCommand>) -> Res<ProjectState> {
    let st = studio(&s);
    blocking(move || st.execute_batch(id, &label, &commands)).await
}

#[tauri::command]
async fn undo(s: State<'_, Studio>, id: Id) -> Res<ProjectState> {
    let st = studio(&s);
    blocking(move || st.undo(id)).await
}

#[tauri::command]
async fn redo(s: State<'_, Studio>, id: Id) -> Res<ProjectState> {
    let st = studio(&s);
    blocking(move || st.redo(id)).await
}

#[tauri::command]
async fn render_frame(s: State<'_, Studio>, id: Id, at: Time, max_height: u32) -> Res<PathBuf> {
    let st = studio(&s);
    blocking(move || st.render_frame(id, at, max_height)).await
}

/// Prefixo de URL para tocar mídia no `<video>` (Linux); `None` = protocolo `asset`.
struct MediaBase(Option<String>);

#[tauri::command]
fn media_base_url(m: State<'_, MediaBase>) -> Option<String> {
    m.0.clone()
}

#[tauri::command]
async fn request_preview(s: State<'_, Studio>, id: Id, at: Option<Time>) -> Res<Id> {
    let st = studio(&s);
    blocking(move || st.request_preview_at(id, at)).await
}

#[tauri::command]
async fn waveform(s: State<'_, Studio>, id: Id, asset_id: Id, from: Time, to: Time, buckets: usize) -> Res<Vec<u8>> {
    let st = studio(&s);
    blocking(move || st.waveform(id, asset_id, from, to, buckets)).await
}

#[tauri::command]
async fn export_project(s: State<'_, Studio>, id: Id, request: ExportRequest) -> Res<Id> {
    let st = studio(&s);
    blocking(move || st.export(id, request)).await
}

#[tauri::command]
async fn list_exports(s: State<'_, Studio>, id: Option<Id>) -> Res<Vec<ExportRecord>> {
    let st = studio(&s);
    blocking(move || st.list_exports(id)).await
}

#[tauri::command]
async fn jobs(s: State<'_, Studio>) -> Res<Vec<JobRecord>> {
    let st = studio(&s);
    blocking(move || st.jobs()).await
}

#[tauri::command]
async fn job_action(s: State<'_, Studio>, job: Id, action: String) -> Res<()> {
    let st = studio(&s);
    blocking(move || match action.as_str() {
        "cancel" => st.cancel_job(job),
        "pause" => st.pause_job(job),
        "resume" | "retry" => st.resume_job(job),
        "remove" => st.remove_job(job),
        other => Err(AppError::new("invalid_action", format!("ação desconhecida: {other}"))),
    })
    .await
}

#[tauri::command]
async fn list_snapshots(s: State<'_, Studio>, id: Id) -> Res<Vec<SnapshotInfo>> {
    let st = studio(&s);
    blocking(move || st.list_snapshots(id)).await
}

#[tauri::command]
async fn create_snapshot(s: State<'_, Studio>, id: Id, label: String) -> Res<SnapshotInfo> {
    let st = studio(&s);
    blocking(move || st.create_snapshot(id, &label)).await
}

#[tauri::command]
async fn restore_snapshot(s: State<'_, Studio>, id: Id, file_name: String) -> Res<ProjectState> {
    let st = studio(&s);
    blocking(move || st.restore_snapshot(id, &file_name)).await
}

// ───────────── IA local ─────────────

use avs_ai::autoedit::{PausePlan, PauseRules};
use avs_app::ai::{AiSettings, AiStatus, AssetAnalysis};

#[tauri::command]
async fn ai_status(s: State<'_, Studio>) -> Res<AiStatus> {
    let st = studio(&s);
    blocking(move || Ok(st.ai_status())).await
}

#[tauri::command]
async fn ai_install(s: State<'_, Studio>, component: String) -> Res<Id> {
    let st = studio(&s);
    blocking(move || st.ai_install(&component)).await
}

#[tauri::command]
async fn ai_remove(s: State<'_, Studio>, component: String) -> Res<()> {
    let st = studio(&s);
    blocking(move || st.ai_remove(&component)).await
}

#[tauri::command]
async fn set_ai_settings(s: State<'_, Studio>, settings: AiSettings) -> Res<AiSettings> {
    let st = studio(&s);
    blocking(move || st.set_ai_settings(settings)).await
}

#[tauri::command]
async fn ollama_pull(s: State<'_, Studio>, model: String) -> Res<Id> {
    let st = studio(&s);
    blocking(move || st.ollama_pull(&model)).await
}

#[tauri::command]
async fn ollama_test(s: State<'_, Studio>, model: String) -> Res<String> {
    let st = studio(&s);
    blocking(move || st.ollama_test(&model)).await
}

#[tauri::command]
async fn ollama_delete(s: State<'_, Studio>, model: String) -> Res<()> {
    let st = studio(&s);
    blocking(move || st.ollama_delete(&model)).await
}

#[tauri::command]
async fn analyze_media(s: State<'_, Studio>, id: Id, asset_ids: Vec<Id>, transcript: bool, force: bool) -> Res<Vec<Id>> {
    let st = studio(&s);
    blocking(move || st.analyze_media(id, &asset_ids, transcript, force)).await
}

#[tauri::command]
async fn analyses(s: State<'_, Studio>, id: Id) -> Res<Vec<AssetAnalysis>> {
    let st = studio(&s);
    blocking(move || st.analyses(id)).await
}

#[tauri::command]
async fn pause_rules(s: State<'_, Studio>, id: Id) -> Res<PauseRules> {
    let st = studio(&s);
    blocking(move || st.pause_rules(id)).await
}

#[tauri::command]
async fn plan_pauses(s: State<'_, Studio>, id: Id, clip_ids: Option<Vec<Id>>, rules: PauseRules) -> Res<PausePlan> {
    let st = studio(&s);
    blocking(move || st.plan_pauses(id, clip_ids, rules)).await
}

#[tauri::command]
async fn apply_pauses(s: State<'_, Studio>, id: Id, plan: PausePlan, sync_text: bool) -> Res<ProjectState> {
    let st = studio(&s);
    blocking(move || st.apply_pauses(id, &plan, sync_text)).await
}

#[tauri::command]
async fn apply_scenes(s: State<'_, Studio>, id: Id, clip_id: Id, split: bool) -> Res<ProjectState> {
    let st = studio(&s);
    blocking(move || st.apply_scenes(id, clip_id, split)).await
}

#[tauri::command]
async fn captions_from_transcript(s: State<'_, Studio>, id: Id, clip_id: Id, max_chars: u32) -> Res<ProjectState> {
    let st = studio(&s);
    blocking(move || st.captions_from_transcript(id, clip_id, max_chars)).await
}

#[tauri::command]
async fn auto_edit_rules(s: State<'_, Studio>, id: Id) -> Res<avs_ai::autoplan::PresetRules> {
    let st = studio(&s);
    blocking(move || st.auto_edit_rules(id)).await
}

#[tauri::command]
async fn preset_auto_rules(s: State<'_, Studio>, preset_id: String) -> Res<avs_ai::autoplan::PresetRules> {
    let st = studio(&s);
    blocking(move || st.preset_auto_rules(&preset_id)).await
}

#[tauri::command]
async fn auto_edit_preview(s: State<'_, Studio>, id: Id, options: avs_ai::autoplan::AutoEditOptions) -> Res<avs_app::auto::AutoEditPreview> {
    let st = studio(&s);
    blocking(move || st.auto_edit_preview(id, &options)).await
}

#[derive(serde::Serialize)]
struct AutoEditApplied {
    result: avs_app::auto::AutoEditResult,
    state: ProjectState,
}

#[tauri::command]
async fn auto_edit_apply(s: State<'_, Studio>, id: Id, options: avs_ai::autoplan::AutoEditOptions) -> Res<AutoEditApplied> {
    let st = studio(&s);
    blocking(move || st.auto_edit_apply(id, &options).map(|(result, state)| AutoEditApplied { result, state })).await
}

#[tauri::command]
async fn auto_edit_start(s: State<'_, Studio>, id: Id, options: avs_ai::autoplan::AutoEditOptions) -> Res<Id> {
    let st = studio(&s);
    blocking(move || st.auto_edit_start(id, options)).await
}

#[tauri::command]
async fn place_assets(s: State<'_, Studio>, id: Id, asset_ids: Vec<Id>) -> Res<ProjectState> {
    let st = studio(&s);
    blocking(move || st.place_assets(id, &asset_ids)).await
}

#[tauri::command]
async fn quality_review(s: State<'_, Studio>, id: Id) -> Res<avs_ai::review::QualityReport> {
    let st = studio(&s);
    blocking(move || st.quality_review(id)).await
}

#[tauri::command]
async fn shorts_defaults(s: State<'_, Studio>, id: Id, preset_id: Option<String>) -> Res<avs_app::shorts::ShortsDefaults> {
    let st = studio(&s);
    blocking(move || st.shorts_defaults(id, preset_id.as_deref())).await
}

#[tauri::command]
async fn suggest_shorts(s: State<'_, Studio>, id: Id, preset_id: String, options: avs_ai::shorts::ShortsOptions) -> Res<Vec<avs_ai::shorts::ShortCandidate>> {
    let st = studio(&s);
    blocking(move || st.suggest_shorts(id, &preset_id, &options)).await
}

#[tauri::command]
async fn create_shorts(s: State<'_, Studio>, id: Id, request: avs_app::shorts::ShortsRequest) -> Res<Id> {
    let st = studio(&s);
    blocking(move || st.create_shorts_start(id, request)).await
}

#[tauri::command]
async fn assistant_availability(s: State<'_, Studio>) -> Res<avs_app::assistant::AssistantAvailability> {
    let st = studio(&s);
    blocking(move || Ok(st.assistant_availability())).await
}

#[tauri::command]
async fn assistant_plan(
    s: State<'_, Studio>,
    id: Id,
    text: String,
    context: avs_ai::assistant::AssistantContext,
    use_llm: bool,
) -> Res<avs_app::assistant::AssistantPlan> {
    let st = studio(&s);
    blocking(move || st.assistant_plan(id, &text, &context, use_llm)).await
}

#[tauri::command]
async fn assistant_apply(
    s: State<'_, Studio>,
    id: Id,
    plan: avs_app::assistant::AssistantPlan,
    context: avs_ai::assistant::AssistantContext,
) -> Res<ProjectState> {
    let st = studio(&s);
    blocking(move || st.assistant_apply(id, &plan, &context)).await
}

/// Abre (ou revela na pasta) um arquivo exportado. Só aceita caminhos
/// registrados no histórico de exportações — o frontend não abre caminhos arbitrários.
#[tauri::command]
async fn open_export(app: tauri::AppHandle, s: State<'_, Studio>, path: PathBuf, reveal: bool) -> Res<()> {
    let st = studio(&s);
    let known = blocking(move || st.list_exports(None)).await?.into_iter().any(|e| e.path == path);
    if !known || !path.is_file() {
        return Err(AppError::new("not_allowed", "arquivo não encontrado no histórico de exportações"));
    }
    use tauri_plugin_opener::OpenerExt;
    let r = if reveal { app.opener().reveal_item_in_dir(&path) } else { app.opener().open_path(path.to_string_lossy(), None::<&str>) };
    r.map_err(|e| AppError::new("open", e.to_string()))
}

fn presets_dir(app: &tauri::App) -> PathBuf {
    if let Ok(d) = std::env::var("AVS_PRESETS_DIR") {
        return PathBuf::from(d);
    }
    if let Ok(r) = app.path().resource_dir() {
        let p = r.join("presets");
        if p.join("export_profiles.json").is_file() {
            return p;
        }
    }
    // Desenvolvimento: presets do repositório.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../presets")
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())).init();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // AVS_LIBRARY permite outra biblioteca (testes, uso portátil); padrão: dados do app.
            let library = match std::env::var_os("AVS_LIBRARY") {
                Some(p) if !p.is_empty() => std::path::PathBuf::from(p),
                _ => app.path().app_data_dir()?,
            };
            // O protocolo `asset` só serve arquivos dos projetos da biblioteca em uso.
            let _ = app.asset_protocol_scope().allow_directory(library.join("projects"), true);
            // Linux: o player do WebKitGTK não toca vídeo do protocolo `asset`;
            // o preview usa um servidor local restrito (ver media_server.rs).
            let media = if cfg!(target_os = "linux") {
                match avs_app::media_server::MediaServer::start(vec![library.join("projects")]) {
                    Ok(m) => Some(m.base_url()),
                    Err(e) => {
                        tracing::warn!("servidor de mídia local indisponível: {e}");
                        None
                    }
                }
            } else {
                None
            };
            app.manage(MediaBase(media));
            let mut cfg = StudioConfig::new(library, presets_dir(app));
            if let Ok(r) = app.path().resource_dir() {
                let w = r.join("workers").join("python");
                if w.join("avs_worker").is_dir() {
                    cfg.worker_dir = w;
                }
                cfg.ffmpeg_dirs.push(r.join("resources").join("ffmpeg"));
                cfg.font_dirs.push(r.join("resources").join("fonts"));
            }
            let studio = Studio::start(cfg, Arc::new(TauriSink(app.handle().clone()))).map_err(|e| {
                // Mensagem real (ex.: FFmpeg ausente) em vez de falha silenciosa.
                let _ = tauri_plugin_dialog::DialogExt::dialog(app.handle())
                    .message(format!("Não foi possível iniciar o AI Video Studio:\n\n{}", e.message))
                    .title("AI Video Studio")
                    .blocking_show();
                Box::new(e) as Box<dyn std::error::Error>
            })?;
            app.manage(studio);
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                if let Some(s) = window.try_state::<Studio>() {
                    s.shutdown();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            library_root,
            report_ui_error,
            system_info,
            encoders,
            presets,
            save_user_preset,
            export_profiles,
            list_projects,
            create_project,
            open_project,
            save_project,
            rename_project,
            close_project,
            delete_project,
            import_media,
            import_subtitles,
            export_subtitles,
            remove_asset,
            relink_asset,
            execute,
            execute_batch,
            undo,
            redo,
            render_frame,
            request_preview,
            media_base_url,
            waveform,
            export_project,
            list_exports,
            jobs,
            job_action,
            list_snapshots,
            create_snapshot,
            restore_snapshot,
            open_export,
            ai_status,
            ai_install,
            ai_remove,
            set_ai_settings,
            ollama_pull,
            ollama_test,
            ollama_delete,
            analyze_media,
            analyses,
            pause_rules,
            plan_pauses,
            apply_pauses,
            apply_scenes,
            captions_from_transcript,
            auto_edit_rules,
            preset_auto_rules,
            auto_edit_preview,
            auto_edit_apply,
            auto_edit_start,
            place_assets,
            quality_review,
            shorts_defaults,
            suggest_shorts,
            create_shorts,
            assistant_availability,
            assistant_plan,
            assistant_apply,
        ])
        .run(tauri::generate_context!())
        .expect("erro ao iniciar a aplicação");
}
