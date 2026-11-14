// Cliente tipado dos comandos Tauri. Os tipos vêm de `src/bindings`, gerados
// a partir das structs Rust (ts-rs): o contrato é único dos dois lados.
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AppError } from "../bindings/AppError";
import type { AppEvent } from "../bindings/AppEvent";
import type { ApplyOutcome } from "../bindings/ApplyOutcome";
import type { EditCommand } from "../bindings/EditCommand";
import type { EditorialPreset } from "../bindings/EditorialPreset";
import type { EncoderInfo } from "../bindings/EncoderInfo";
import type { ExportProfile } from "../bindings/ExportProfile";
import type { ExportRecord } from "../bindings/ExportRecord";
import type { ExportRequest } from "../bindings/ExportRequest";
import type { ImportResult } from "../bindings/ImportResult";
import type { JobRecord } from "../bindings/JobRecord";
import type { ProjectSettings } from "../bindings/ProjectSettings";
import type { ProjectState } from "../bindings/ProjectState";
import type { ProjectSummary } from "../bindings/ProjectSummary";
import type { SnapshotInfo } from "../bindings/SnapshotInfo";
import type { SubtitleImport } from "../bindings/SubtitleImport";
import type { AiSettings } from "../bindings/AiSettings";
import type { AiStatus } from "../bindings/AiStatus";
import type { AssetAnalysis } from "../bindings/AssetAnalysis";
import type { PausePlan } from "../bindings/PausePlan";
import type { PauseRules } from "../bindings/PauseRules";
import type { SystemInfo } from "../bindings/SystemInfo";
import type { Time } from "../bindings/Time";
import type { AutoEditOptions } from "../bindings/AutoEditOptions";
import type { AutoEditPreview } from "../bindings/AutoEditPreview";
import type { AutoEditResult } from "../bindings/AutoEditResult";
import type { PresetRules } from "../bindings/PresetRules";
import type { QualityReport } from "../bindings/QualityReport";
import type { AssistantAvailability } from "../bindings/AssistantAvailability";
import type { AssistantContext } from "../bindings/AssistantContext";
import type { AssistantPlan } from "../bindings/AssistantPlan";
import type { ShortCandidate } from "../bindings/ShortCandidate";
import type { ShortsDefaults } from "../bindings/ShortsDefaults";
import type { ShortsOptions } from "../bindings/ShortsOptions";
import type { ShortsRequest } from "../bindings/ShortsRequest";

export function isAppError(e: unknown): e is AppError {
  return typeof e === "object" && e !== null && "code" in e && "message" in e;
}

export function errorMessage(e: unknown): string {
  if (isAppError(e)) return e.message;
  if (e instanceof Error) return e.message;
  return String(e);
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return invoke<T>(cmd, args);
}

export const api = {
  libraryRoot: () => call<string>("library_root"),
  systemInfo: () => call<SystemInfo>("system_info"),
  encoders: () => call<EncoderInfo[]>("encoders"),
  presets: () => call<EditorialPreset[]>("presets"),
  saveUserPreset: (preset: EditorialPreset) => call<EditorialPreset>("save_user_preset", { preset }),
  exportProfiles: () => call<ExportProfile[]>("export_profiles"),

  listProjects: () => call<ProjectSummary[]>("list_projects"),
  createProject: (name: string, presetId: string | null, settings: ProjectSettings | null) =>
    call<ProjectState>("create_project", { name, presetId, settings }),
  openProject: (id: string) => call<ProjectState>("open_project", { id }),
  saveProject: (id: string) => call<ProjectState>("save_project", { id }),
  renameProject: (id: string, name: string) => call<ProjectState>("rename_project", { id, name }),
  closeProject: (id: string) => call<void>("close_project", { id }),
  deleteProject: (id: string) => call<void>("delete_project", { id }),

  importMedia: (id: string, paths: string[]) => call<ImportResult>("import_media", { id, paths }),
  importSubtitles: (id: string, path: string) => call<SubtitleImport>("import_subtitles", { id, path }),
  exportSubtitles: (id: string, trackId: string, path: string) => call<number>("export_subtitles", { id, trackId, path }),
  removeAsset: (id: string, assetId: string) => call<ProjectState>("remove_asset", { id, assetId }),
  relinkAsset: (id: string, assetId: string, path: string, force: boolean) =>
    call<ProjectState>("relink_asset", { id, assetId, path, force }),

  execute: (id: string, command: EditCommand) =>
    call<{ outcome: ApplyOutcome; state: ProjectState }>("execute", { id, command }),
  executeBatch: (id: string, label: string, commands: EditCommand[]) =>
    call<ProjectState>("execute_batch", { id, label, commands }),
  undo: (id: string) => call<ProjectState>("undo", { id }),
  redo: (id: string) => call<ProjectState>("redo", { id }),

  renderFrame: (id: string, at: Time, maxHeight: number) => call<string>("render_frame", { id, at, maxHeight }),
  mediaBaseUrl: () => call<string | null>("media_base_url"),
  requestPreview: (id: string, at: Time | null = null) => call<string>("request_preview", { id, at }),
  waveform: (id: string, assetId: string, from: Time, to: Time, buckets: number) =>
    call<number[]>("waveform", { id, assetId, from, to, buckets }),

  exportProject: (id: string, request: ExportRequest) => call<string>("export_project", { id, request }),
  listExports: (id: string | null) => call<ExportRecord[]>("list_exports", { id }),
  openExport: (path: string, reveal: boolean) => call<void>("open_export", { path, reveal }),

  jobs: () => call<JobRecord[]>("jobs"),
  jobAction: (job: string, action: "cancel" | "pause" | "resume" | "retry" | "remove") =>
    call<void>("job_action", { job, action }),

  aiStatus: () => call<AiStatus>("ai_status"),
  aiInstall: (component: string) => call<string>("ai_install", { component }),
  aiRemove: (component: string) => call<void>("ai_remove", { component }),
  setAiSettings: (settings: AiSettings) => call<AiSettings>("set_ai_settings", { settings }),
  ollamaPull: (model: string) => call<string>("ollama_pull", { model }),
  ollamaTest: (model: string) => call<string>("ollama_test", { model }),
  ollamaDelete: (model: string) => call<void>("ollama_delete", { model }),
  analyzeMedia: (id: string, assetIds: string[], transcript: boolean, force: boolean) => call<string[]>("analyze_media", { id, assetIds, transcript, force }),
  analyses: (id: string) => call<AssetAnalysis[]>("analyses", { id }),
  pauseRules: (id: string) => call<PauseRules>("pause_rules", { id }),
  planPauses: (id: string, clipIds: string[] | null, rules: PauseRules) => call<PausePlan>("plan_pauses", { id, clipIds, rules }),
  applyPauses: (id: string, plan: PausePlan, syncText: boolean) => call<ProjectState>("apply_pauses", { id, plan, syncText }),
  applyScenes: (id: string, clipId: string, split: boolean) => call<ProjectState>("apply_scenes", { id, clipId, split }),
  captionsFromTranscript: (id: string, clipId: string, maxChars: number) => call<ProjectState>("captions_from_transcript", { id, clipId, maxChars }),

  autoEditRules: (id: string) => call<PresetRules>("auto_edit_rules", { id }),
  presetAutoRules: (presetId: string) => call<PresetRules>("preset_auto_rules", { presetId }),
  autoEditPreview: (id: string, options: AutoEditOptions) => call<AutoEditPreview>("auto_edit_preview", { id, options }),
  autoEditApply: (id: string, options: AutoEditOptions) => call<{ result: AutoEditResult; state: ProjectState }>("auto_edit_apply", { id, options }),
  autoEditStart: (id: string, options: AutoEditOptions) => call<string>("auto_edit_start", { id, options }),
  placeAssets: (id: string, assetIds: string[]) => call<ProjectState>("place_assets", { id, assetIds }),
  qualityReview: (id: string) => call<QualityReport>("quality_review", { id }),

  shortsDefaults: (id: string, presetId: string | null) => call<ShortsDefaults>("shorts_defaults", { id, presetId }),
  suggestShorts: (id: string, presetId: string, options: ShortsOptions) => call<ShortCandidate[]>("suggest_shorts", { id, presetId, options }),
  createShorts: (id: string, request: ShortsRequest) => call<string>("create_shorts", { id, request }),

  assistantAvailability: () => call<AssistantAvailability>("assistant_availability"),
  assistantPlan: (id: string, text: string, context: AssistantContext, useLlm: boolean) => call<AssistantPlan>("assistant_plan", { id, text, context, useLlm }),
  assistantApply: (id: string, plan: AssistantPlan, context: AssistantContext) => call<ProjectState>("assistant_apply", { id, plan, context }),

  listSnapshots: (id: string) => call<SnapshotInfo[]>("list_snapshots", { id }),
  createSnapshot: (id: string, label: string) => call<SnapshotInfo>("create_snapshot", { id, label }),
  restoreSnapshot: (id: string, fileName: string) => call<ProjectState>("restore_snapshot", { id, fileName }),
};

export function onAppEvent(cb: (e: AppEvent) => void): Promise<UnlistenFn> {
  return listen<AppEvent>("avs://event", (ev) => cb(ev.payload));
}

/** URL servível de um arquivo dentro da biblioteca (protocolo asset, escopo restrito). */
export function fileUrl(path: string | null | undefined): string | undefined {
  return path ? convertFileSrc(path) : undefined;
}
