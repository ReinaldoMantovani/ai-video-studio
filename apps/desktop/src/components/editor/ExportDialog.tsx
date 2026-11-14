import { useQuery } from "@tanstack/react-query";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { AlertTriangle, FolderOpen } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import type { ExportRequest } from "../../bindings/ExportRequest";
import { api, errorMessage } from "../../lib/api";
import { secs } from "../../lib/time";
import { useApp } from "../../store/app";
import { Button, Dialog, Field, inputCls, Slider } from "../ui";

const LAST_DIR = "avs.exportDir";

export function ExportDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const ps = useApp((s) => s.ps)!;
  const toast = useApp((s) => s.toast);
  const profiles = useQuery({ queryKey: ["export-profiles"], queryFn: api.exportProfiles });
  const presets = useQuery({ queryKey: ["presets"], queryFn: api.presets });
  const encoders = useQuery({ queryKey: ["encoders"], queryFn: api.encoders, staleTime: Infinity, enabled: open });
  const p = ps.project;
  const preset = presets.data?.find((x) => x.id === p.preset_id);
  const [profileId, setProfileId] = useState("");
  const [name, setName] = useState(p.name);
  const [dir, setDir] = useState<string>(() => localStorage.getItem(LAST_DIR) ?? "");
  const [resMode, setResMode] = useState<"project" | "profile">("project");
  const [encoder, setEncoder] = useState<string>("auto");
  const [quality, setQuality] = useState(19);
  const [loud, setLoud] = useState(true);
  const prof = profiles.data?.find((x) => x.id === profileId);

  useEffect(() => {
    if (!open || !profiles.data) return;
    const id = preset?.export_profile && profiles.data.some((x) => x.id === preset.export_profile) ? preset.export_profile : profiles.data[0]?.id;
    setProfileId(id ?? "");
    setName(p.name);
  }, [open, profiles.data, preset?.export_profile, p.name]);

  // Última pasta usada (pode ter mudado em outro projeto desde que o editor abriu).
  useEffect(() => {
    if (open) setDir((d) => localStorage.getItem(LAST_DIR) ?? d);
  }, [open]);

  useEffect(() => {
    if (!prof) return;
    setQuality(prof.quality);
    setLoud(prof.loudness_lufs != null);
    setEncoder("auto");
    setResMode(prof.resolution && prof.resolution.width * p.settings.height === prof.resolution.height * p.settings.width ? "profile" : "project");
  }, [prof, p.settings.width, p.settings.height]);

  const duration = useMemo(() => p.timeline.tracks.reduce((m, t) => t.clips.reduce((mm, c) => Math.max(mm, c.start + c.duration), m), 0), [p.timeline]);
  const tooLong = prof?.max_duration_s != null && secs(duration) > prof.max_duration_s;
  const aspectMismatch = prof?.resolution && Math.abs(prof.resolution.width / prof.resolution.height - p.settings.width / p.settings.height) > 0.01;
  const working = encoders.data?.filter((e) => e.working && e.codec === prof?.codec) ?? [];

  const start = async () => {
    if (!prof) return;
    if (!dir) {
      toast("error", "Escolha a pasta de destino.");
      return;
    }
    const req: ExportRequest = {
      output_dir: dir,
      file_name: name,
      container: prof.container,
      codec: prof.codec,
      encoder: encoder === "auto" ? null : encoder,
      prefer_hardware: true,
      width: resMode === "profile" && prof.resolution ? prof.resolution.width : null,
      height: resMode === "profile" && prof.resolution ? prof.resolution.height : null,
      fps: prof.fps,
      quality,
      speed: prof.speed,
      bitrate_kbps: prof.bitrate_kbps,
      audio_bitrate_kbps: prof.audio_bitrate_kbps,
      loudness_lufs: loud ? (prof.loudness_lufs ?? -14) : null,
      range: null,
    };
    try {
      await api.exportProject(p.id, req);
      localStorage.setItem(LAST_DIR, dir);
      useApp.getState().setJobsOpen(true);
      onClose();
    } catch (e) {
      toast("error", errorMessage(e));
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Exportar vídeo"
      width={560}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancelar
          </Button>
          <Button variant="primary" onClick={start} disabled={!prof || !dir || !name.trim()}>
            Exportar
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <Field label="Destino">
          <select className={inputCls} value={profileId} onChange={(e) => setProfileId(e.target.value)}>
            {profiles.data?.map((x) => (
              <option key={x.id} value={x.id}>
                {x.name}
              </option>
            ))}
          </select>
        </Field>
        <div className="grid grid-cols-[1fr_auto] items-end gap-2">
          <Field label="Pasta">
            <input className={inputCls} value={dir} readOnly placeholder="Escolha uma pasta" />
          </Field>
          <Button
            onClick={async () => {
              const d = await openDialog({ directory: true, title: "Pasta de destino" });
              if (typeof d === "string") setDir(d);
            }}
          >
            <FolderOpen size={14} /> Escolher
          </Button>
        </div>
        <Field label="Nome do arquivo" hint={prof ? `Extensão .${prof.container} adicionada automaticamente; nomes existentes não são sobrescritos.` : undefined}>
          <input className={inputCls} value={name} onChange={(e) => setName(e.target.value)} />
        </Field>
        <div className="grid grid-cols-2 gap-3">
          <Field label="Resolução">
            <select className={inputCls} value={resMode} onChange={(e) => setResMode(e.target.value as "project" | "profile")}>
              <option value="project">
                Do projeto ({p.settings.width}×{p.settings.height})
              </option>
              {prof?.resolution && (
                <option value="profile">
                  Do destino ({prof.resolution.width}×{prof.resolution.height})
                </option>
              )}
            </select>
          </Field>
          <Field label="Encoder">
            <select className={inputCls} value={encoder} onChange={(e) => setEncoder(e.target.value)}>
              <option value="auto">Automático (melhor disponível)</option>
              {working.map((e) => (
                <option key={e.name} value={e.name}>
                  {e.name} {e.hardware ? `· ${e.vendor}` : "· CPU"}
                </option>
              ))}
            </select>
          </Field>
        </div>
        <Slider label="Qualidade (menor = melhor, arquivo maior)" min={10} max={35} step={1} value={quality} onChange={setQuality} onCommit={setQuality} />
        <label className="flex items-center gap-2 text-xs text-fg-2">
          <input type="checkbox" checked={loud} onChange={(e) => setLoud(e.target.checked)} />
          Normalizar volume para {prof?.loudness_lufs ?? -14} LUFS
        </label>
        {(tooLong || (aspectMismatch && resMode === "profile")) && (
          <div className="flex gap-2 rounded-lg border border-warn/30 bg-warn/10 p-3 text-xs text-warn">
            <AlertTriangle size={15} className="shrink-0" />
            <div className="flex flex-col gap-1">
              {tooLong && <span>A timeline tem {Math.round(secs(duration))} s; o perfil {prof?.name} recomenda até {prof?.max_duration_s} s.</span>}
              {aspectMismatch && resMode === "profile" && <span>A proporção do destino difere do projeto: o vídeo será encaixado com barras.</span>}
            </div>
          </div>
        )}
        <p className="text-[11px] text-fg-3">O arquivo só é marcado como concluído depois de validado: resolução, FPS, codec, duração, áudio, sincronismo e decodificação completa.</p>
      </div>
    </Dialog>
  );
}
