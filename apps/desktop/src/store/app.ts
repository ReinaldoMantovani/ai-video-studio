import { create } from "zustand";
import type { EditCommand } from "../bindings/EditCommand";
import type { JobRecord } from "../bindings/JobRecord";
import type { PausePlan } from "../bindings/PausePlan";
import type { ProjectState } from "../bindings/ProjectState";
import type { QualityReport } from "../bindings/QualityReport";
export type PreviewFile = { path: string; revision: number; range: TimeRange | null };
import type { TimeRange } from "../bindings/TimeRange";
import { api, errorMessage } from "../lib/api";

export type Toast = { id: number; kind: "info" | "error" | "success"; text: string };

export type Tool = "select" | "ripple" | "roll" | "slip" | "slide";

type View = { name: "dashboard" } | { name: "editor"; projectId: string };

interface AppStore {
  view: View;
  jobs: Record<string, JobRecord>;
  toasts: Toast[];
  jobsOpen: boolean;

  // editor
  ps: ProjectState | null;
  selectedClip: string | null;
  selectedTransition: string | null;
  playhead: number;
  pxPerSec: number;
  /** Preview em reprodução (`range` = janela de uma timeline longa) e a próxima janela já pronta. */
  preview: PreviewFile | null;
  nextPreview: PreviewFile | null;
  busy: boolean;
  tool: Tool;
  setTool: (t: Tool) => void;
  aiOpen: boolean;
  setAiOpen: (v: boolean) => void;
  /** Plano de remoção de pausas em pré-visualização (desenhado na timeline). */
  pausePlan: PausePlan | null;
  setPausePlan: (p: PausePlan | null) => void;
  /** Trechos que a edição automática removeria (pré-visualização na timeline). */
  autoPlan: TimeRange[] | null;
  setAutoPlan: (r: TimeRange[] | null) => void;
  /** Cortes sugeridos marcados na timeline. */
  shortRanges: { range: TimeRange; rank: number }[] | null;
  setShortRanges: (r: { range: TimeRange; rank: number }[] | null) => void;
  /** Última revisão de qualidade (diálogo aberto quando `reviewOpen`). */
  review: QualityReport | null;
  reviewOpen: boolean;
  showReview: (r: QualityReport | null, open?: boolean) => void;
  closeReview: () => void;

  go: (v: View) => void;
  toast: (kind: Toast["kind"], text: string) => void;
  dismiss: (id: number) => void;
  setJobsOpen: (v: boolean) => void;
  upsertJob: (j: JobRecord) => void;
  setJobs: (j: JobRecord[]) => void;

  openProject: (id: string) => Promise<void>;
  refresh: () => Promise<void>;
  setState: (ps: ProjectState) => void;
  exec: (cmd: EditCommand) => Promise<string[] | null>;
  batch: (label: string, cmds: EditCommand[]) => Promise<boolean>;
  undo: () => Promise<void>;
  redo: () => Promise<void>;
  save: () => Promise<void>;
  select: (clip: string | null) => void;
  selectTransition: (id: string | null) => void;
  setPlayhead: (t: number) => void;
  setZoom: (pxPerSec: number) => void;
  setPreview: (p: PreviewFile | null) => void;
  /** Novo preview pronto: vira o atual, ou a "próxima janela" se o atual ainda cobre o cursor. */
  previewReady: (p: PreviewFile) => void;
  setNextPreview: (p: PreviewFile | null) => void;
}

let toastSeq = 0;

export const useApp = create<AppStore>((set, get) => ({
  view: { name: "dashboard" },
  jobs: {},
  toasts: [],
  jobsOpen: false,
  ps: null,
  selectedClip: null,
  selectedTransition: null,
  playhead: 0,
  pxPerSec: 60,
  preview: null,
  nextPreview: null,
  busy: false,
  tool: "select",
  setTool: (tool) => set({ tool }),
  aiOpen: false,
  setAiOpen: (aiOpen) => set({ aiOpen }),
  pausePlan: null,
  setPausePlan: (pausePlan) => set({ pausePlan }),
  autoPlan: null,
  setAutoPlan: (autoPlan) => set({ autoPlan }),
  shortRanges: null,
  setShortRanges: (shortRanges) => set({ shortRanges }),
  review: null,
  reviewOpen: false,
  showReview: (review, open = true) => set({ review, reviewOpen: open && !!review }),
  closeReview: () => set({ reviewOpen: false }),

  go: (v) => set({ view: v }),
  toast: (kind, text) => {
    const id = ++toastSeq;
    set((s) => ({ toasts: [...s.toasts.slice(-3), { id, kind, text }] }));
    setTimeout(() => get().dismiss(id), kind === "error" ? 7000 : 3500);
  },
  dismiss: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
  setJobsOpen: (v) => set({ jobsOpen: v }),
  upsertJob: (j) => set((s) => ({ jobs: { ...s.jobs, [j.id]: j } })),
  setJobs: (list) => set({ jobs: Object.fromEntries(list.map((j) => [j.id, j])) }),

  openProject: async (id) => {
    try {
      const ps = await api.openProject(id);
      set({ ps, view: { name: "editor", projectId: id }, selectedClip: null, selectedTransition: null, playhead: 0, preview: null, nextPreview: null, autoPlan: null, pausePlan: null, shortRanges: null, review: null, reviewOpen: false });
      if (ps.recovered) get().toast("info", "Alterações não salvas foram recuperadas do salvamento automático.");
    } catch (e) {
      get().toast("error", errorMessage(e));
    }
  },
  refresh: async () => {
    const id = get().ps?.project.id;
    if (!id) return;
    try {
      set({ ps: await api.openProject(id) });
    } catch (e) {
      get().toast("error", errorMessage(e));
    }
  },
  setState: (ps) => {
    const s = get();
    const clipExists = (cid: string | null) => !!cid && ps.project.timeline.tracks.some((t) => t.clips.some((c) => c.id === cid));
    const trExists = (tid: string | null) => !!tid && ps.project.timeline.transitions.some((t) => t.id === tid);
    set({
      ps,
      pausePlan: s.ps && s.ps.history.revision !== ps.history.revision ? null : s.pausePlan,
      autoPlan: s.ps && s.ps.history.revision !== ps.history.revision ? null : s.autoPlan,
      shortRanges: s.ps && s.ps.history.revision !== ps.history.revision ? null : s.shortRanges,
      selectedClip: clipExists(s.selectedClip) ? s.selectedClip : null,
      selectedTransition: trExists(s.selectedTransition) ? s.selectedTransition : null,
    });
  },
  exec: async (cmd) => {
    const id = get().ps?.project.id;
    if (!id) return null;
    try {
      const r = await api.execute(id, cmd);
      get().setState(r.state);
      return r.outcome.created;
    } catch (e) {
      get().toast("error", errorMessage(e));
      return null;
    }
  },
  batch: async (label, cmds) => {
    const id = get().ps?.project.id;
    if (!id) return false;
    try {
      get().setState(await api.executeBatch(id, label, cmds));
      return true;
    } catch (e) {
      get().toast("error", errorMessage(e));
      return false;
    }
  },
  undo: async () => {
    const id = get().ps?.project.id;
    if (!id || !get().ps?.history.can_undo) return;
    get().setState(await api.undo(id));
  },
  redo: async () => {
    const id = get().ps?.project.id;
    if (!id || !get().ps?.history.can_redo) return;
    get().setState(await api.redo(id));
  },
  save: async () => {
    const id = get().ps?.project.id;
    if (!id) return;
    try {
      get().setState(await api.saveProject(id));
      get().toast("success", "Projeto salvo");
    } catch (e) {
      get().toast("error", errorMessage(e));
    }
  },
  select: (clip) => set({ selectedClip: clip, selectedTransition: null }),
  selectTransition: (id) => set({ selectedTransition: id, selectedClip: null }),
  setPlayhead: (t) => set({ playhead: Math.max(0, t) }),
  setZoom: (pxPerSec) => set({ pxPerSec: Math.min(600, Math.max(2, pxPerSec)) }),
  setPreview: (p) => set({ preview: p }),
  setNextPreview: (p) => set({ nextPreview: p }),
  previewReady: (p) => {
    const { preview: cur, playhead } = get();
    const curCovers = !!cur && cur.revision === p.revision && (!cur.range || (playhead >= cur.range.start && playhead < cur.range.end));
    if (curCovers && cur!.range && p.range && p.range.start > cur!.range.start) set({ nextPreview: p });
    else set({ preview: p, nextPreview: null });
  },
}));
