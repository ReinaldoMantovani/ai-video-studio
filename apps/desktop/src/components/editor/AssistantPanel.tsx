import { useQuery } from "@tanstack/react-query";
import clsx from "clsx";
import { Check, Loader2, Lock, MessageSquareText, SendHorizontal, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { AssistantPlan } from "../../bindings/AssistantPlan";
import { api, errorMessage } from "../../lib/api";
import { shortDuration } from "../../lib/time";
import { useApp } from "../../store/app";
import { Button } from "../ui";

const EXAMPLES = [
  "remova as pausas",
  "corte de 0:10 a 0:20",
  "deixe com 30 segundos",
  "aumente a música em 3 dB",
  'adicione o texto "Inscreva-se" em 0:05 por 3 segundos',
  "aplique fade de saída de 2 segundos no final",
  "coloque um zoom de 20% aqui",
  "gere legendas",
];

type Entry = { id: number; text: string; plan?: AssistantPlan; error?: string; status: "pending" | "ready" | "applied" | "discarded" | "error" };

/** Aba "Assistente": pedido em português → prévia das ações → aplicar (um Ctrl+Z). */
export function AssistantPanel() {
  const ps = useApp((s) => s.ps)!;
  const toast = useApp((s) => s.toast);
  const id = ps.project.id;
  const avail = useQuery({ queryKey: ["assistant-availability"], queryFn: api.assistantAvailability, staleTime: 15000 });
  const [useLlm, setUseLlm] = useState(false);
  const [text, setText] = useState("");
  const [log, setLog] = useState<Entry[]>([]);
  const [busy, setBusy] = useState(false);
  const seq = useRef(0);
  const end = useRef<HTMLDivElement>(null);
  useEffect(() => end.current?.scrollIntoView({ block: "end" }), [log]);

  const context = () => {
    const s = useApp.getState();
    return { selected_clip: s.selectedClip, playhead: s.playhead };
  };
  const update = (eid: number, patch: Partial<Entry>) => setLog((l) => l.map((e) => (e.id === eid ? { ...e, ...patch } : e)));

  const send = async (t = text) => {
    const q = t.trim();
    if (!q || busy) return;
    const eid = ++seq.current;
    // Um plano pendente anterior deixa de valer.
    setLog((l) => [...l.map((e) => (e.status === "ready" ? { ...e, status: "discarded" as const } : e)), { id: eid, text: q, status: "pending" }]);
    setText("");
    setBusy(true);
    try {
      const plan = await api.assistantPlan(id, q, context(), useLlm && !!avail.data?.llm);
      update(eid, { plan, status: "ready" });
    } catch (e) {
      update(eid, { error: errorMessage(e), status: "error" });
    } finally {
      setBusy(false);
    }
  };

  const applyPlan = async (e: Entry) => {
    if (!e.plan) return;
    setBusy(true);
    try {
      const st = await api.assistantApply(id, e.plan, context());
      useApp.getState().setState(st);
      update(e.id, { status: "applied" });
    } catch (err) {
      toast("error", errorMessage(err));
      update(e.id, { status: "error", error: errorMessage(err) });
    } finally {
      setBusy(false);
    }
  };

  const llm = avail.data;
  return (
    <div className="flex h-full min-h-[420px] flex-col gap-2">
      <div className="flex items-center gap-2 px-0.5">
        <MessageSquareText size={14} className="text-accent" />
        <h3 className="text-xs font-semibold">Assistente de edição</h3>
      </div>
      <p className="px-0.5 text-[11px] leading-snug text-fg-3">Peça em português. Você vê o plano antes, e nada muda sem confirmar. Cada pedido aplicado é um único Ctrl+Z.</p>

      <div className="rounded-md bg-ink-2/50 px-2 py-1.5 text-[11px]">
        {llm?.llm ? (
          <label className="flex items-center gap-2 text-fg-2">
            <input type="checkbox" checked={useLlm} onChange={(e) => setUseLlm(e.target.checked)} />
            Interpretar com o modelo local ({llm.llm_model})
          </label>
        ) : (
          <p className="text-fg-3">
            Interpretação por regras (sem IA generativa).{" "}
            <button className="text-accent hover:underline" onClick={() => useApp.getState().setAiOpen(true)} title={llm?.llm_reason ?? ""}>
              Usar um modelo local
            </button>
          </p>
        )}
      </div>

      <div className="flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto">
        {log.length === 0 && (
          <div className="flex flex-wrap gap-1">
            {EXAMPLES.map((x) => (
              <button key={x} className="rounded-full border border-line px-2 py-0.5 text-left text-[11px] text-fg-2 hover:border-accent/50 hover:text-fg" onClick={() => setText(x)}>
                {x}
              </button>
            ))}
          </div>
        )}
        {log.map((e) => (
          <div key={e.id} className="flex flex-col gap-1">
            <p className="ml-6 self-end rounded-lg rounded-br-sm bg-accent-soft px-2.5 py-1.5 text-xs text-fg">{e.text}</p>
            {e.status === "pending" && (
              <p className="flex items-center gap-1.5 text-[11px] text-fg-3">
                <Loader2 size={12} className="animate-spin" /> Interpretando…
              </p>
            )}
            {e.error && <p className="rounded-md border border-bad/30 px-2 py-1.5 text-[11px] leading-snug text-fg-2">{e.error}</p>}
            {e.plan && (
              <div className={clsx("rounded-md border px-2 py-1.5 text-xs", e.status === "applied" ? "border-ok/30" : "border-line", e.status === "discarded" && "opacity-50")}>
                <ol className="flex list-decimal flex-col gap-0.5 pl-4 text-fg-2">
                  {e.plan.steps.map((s, i) => (
                    <li key={i}>{s}</li>
                  ))}
                </ol>
                <p className="mt-1 text-[11px] text-fg-3">
                  Duração {shortDuration(e.plan.before)} → {shortDuration(e.plan.after)} · {e.plan.source === "regras" ? "interpretado por regras" : `interpretado por ${e.plan.source.replace("ollama:", "")}`}
                </p>
                {e.plan.note && <p className="mt-0.5 text-[11px] text-fg-3">Modelo: {e.plan.note}</p>}
                {e.plan.data_sent && (
                  <p className="mt-0.5 flex gap-1 text-[10px] text-fg-3">
                    <Lock size={10} className="mt-0.5 shrink-0" /> {e.plan.data_sent}
                  </p>
                )}
                {e.status === "ready" && (
                  <div className="mt-1.5 flex gap-1.5">
                    <Button size="sm" variant="primary" className="flex-1" disabled={busy} onClick={() => applyPlan(e)}>
                      <Check size={13} /> Aplicar
                    </Button>
                    <Button size="sm" variant="ghost" onClick={() => update(e.id, { status: "discarded" })}>
                      <X size={13} /> Descartar
                    </Button>
                  </div>
                )}
                {e.status === "applied" && <p className="mt-1 text-[11px] text-ok">Aplicado. Ctrl+Z desfaz.</p>}
              </div>
            )}
          </div>
        ))}
        <div ref={end} />
      </div>

      <div className="flex items-end gap-1.5">
        <textarea
          rows={2}
          className="min-h-[52px] flex-1 resize-none rounded-md border border-line bg-ink-2 px-2 py-1.5 text-xs text-fg outline-none placeholder:text-fg-3 focus:border-accent/60"
          placeholder='Ex.: remova as pausas e adicione o texto "Oi" em 0:02'
          value={text}
          maxLength={1000}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              send();
            }
          }}
          aria-label="Pedido ao assistente"
        />
        <Button size="sm" variant="primary" disabled={busy || !text.trim()} onClick={() => send()} title="Enviar (Enter)">
          {busy ? <Loader2 size={13} className="animate-spin" /> : <SendHorizontal size={13} />}
        </Button>
      </div>
    </div>
  );
}
