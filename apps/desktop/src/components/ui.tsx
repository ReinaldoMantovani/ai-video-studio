// Primitivos de interface (equivalentes aos do shadcn/ui, sem dependências extras).
import clsx from "clsx";
import { X } from "lucide-react";
import { useEffect, type ButtonHTMLAttributes, type ReactNode } from "react";

type Variant = "primary" | "ghost" | "subtle" | "danger";

export function Button({
  variant = "subtle",
  size = "md",
  className,
  ...p
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant; size?: "sm" | "md" }) {
  return (
    <button
      {...p}
      className={clsx(
        "inline-flex items-center justify-center gap-1.5 rounded-md font-medium transition-colors disabled:opacity-40 disabled:pointer-events-none whitespace-nowrap",
        size === "sm" ? "h-7 px-2.5 text-xs" : "h-8 px-3 text-[13px]",
        variant === "primary" && "bg-accent-strong text-white hover:bg-accent",
        variant === "subtle" && "bg-ink-2 text-fg hover:bg-ink-3 border border-line",
        variant === "ghost" && "text-fg-2 hover:text-fg hover:bg-ink-2",
        variant === "danger" && "bg-bad/15 text-bad hover:bg-bad/25",
        className,
      )}
    />
  );
}

export function IconButton({
  label,
  active,
  className,
  ...p
}: ButtonHTMLAttributes<HTMLButtonElement> & { label: string; active?: boolean }) {
  return (
    <button
      aria-label={label}
      title={label}
      {...p}
      className={clsx(
        "inline-flex size-7 items-center justify-center rounded-md transition-colors disabled:opacity-35 disabled:pointer-events-none",
        active ? "bg-accent-soft text-accent" : "text-fg-2 hover:text-fg hover:bg-ink-3",
        className,
      )}
    />
  );
}

export function Field({ label, children, hint }: { label: string; children: ReactNode; hint?: string }) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-xs text-fg-2">{label}</span>
      {children}
      {hint && <span className="text-[11px] text-fg-3">{hint}</span>}
    </label>
  );
}

export const inputCls =
  "h-8 rounded-md bg-ink-2 border border-line px-2.5 text-[13px] text-fg placeholder:text-fg-3 focus:border-accent focus:outline-none";

export function Slider({
  value,
  min,
  max,
  step,
  onChange,
  onCommit,
  format,
  label,
}: {
  value: number;
  min: number;
  max: number;
  step: number;
  label: string;
  onChange: (v: number) => void;
  onCommit: (v: number) => void;
  format?: (v: number) => string;
}) {
  return (
    <div className="flex flex-col gap-1">
      <div className="flex justify-between text-xs">
        <span className="text-fg-2">{label}</span>
        <span className="tabular text-fg">{format ? format(value) : value}</span>
      </div>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        aria-label={label}
        onChange={(e) => onChange(Number(e.target.value))}
        onPointerUp={(e) => onCommit(Number((e.target as HTMLInputElement).value))}
        onKeyUp={(e) => onCommit(Number((e.target as HTMLInputElement).value))}
        className="w-full"
      />
    </div>
  );
}

export function Progress({ value, tone = "accent" }: { value: number; tone?: "accent" | "ok" | "bad" }) {
  return (
    <div className="h-1.5 w-full overflow-hidden rounded-full bg-ink-3">
      <div
        className={clsx("h-full rounded-full transition-[width] duration-300", tone === "accent" && "bg-accent", tone === "ok" && "bg-ok", tone === "bad" && "bg-bad")}
        style={{ width: `${Math.round(Math.max(0, Math.min(1, value)) * 100)}%` }}
      />
    </div>
  );
}

export function Dialog({
  open,
  onClose,
  title,
  children,
  footer,
  width = 520,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  children: ReactNode;
  footer?: ReactNode;
  width?: number;
}) {
  useEffect(() => {
    if (!open) return;
    const k = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [open, onClose]);
  if (!open) return null;
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/55 backdrop-blur-[2px]" onMouseDown={onClose}>
      <div
        role="dialog"
        aria-modal
        aria-label={title}
        className="avs-in flex max-h-[86vh] flex-col rounded-[14px] border border-line bg-ink-1 shadow-2xl shadow-black/50"
        style={{ width }}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between border-b border-line px-5 py-3.5">
          <h2 className="text-[15px] font-semibold">{title}</h2>
          <IconButton label="Fechar" onClick={onClose}>
            <X size={16} />
          </IconButton>
        </div>
        <div className="overflow-y-auto px-5 py-4">{children}</div>
        {footer && <div className="flex justify-end gap-2 border-t border-line px-5 py-3">{footer}</div>}
      </div>
    </div>
  );
}

export function Empty({ icon, title, children }: { icon: ReactNode; title: string; children?: ReactNode }) {
  return (
    <div className="flex flex-col items-center justify-center gap-2 px-6 py-10 text-center">
      <div className="text-fg-3">{icon}</div>
      <p className="font-medium text-fg">{title}</p>
      {children && <div className="max-w-xs text-xs text-fg-2">{children}</div>}
    </div>
  );
}

export function Tabs<T extends string>({
  tabs,
  value,
  onChange,
}: {
  tabs: { id: T; label: string; icon?: ReactNode }[];
  value: T;
  onChange: (v: T) => void;
}) {
  return (
    <div role="tablist" className="flex gap-0.5 rounded-lg bg-ink-0 p-0.5">
      {tabs.map((t) => (
        <button
          key={t.id}
          role="tab"
          aria-selected={value === t.id}
          onClick={() => onChange(t.id)}
          title={t.label}
          aria-label={t.label}
          className={clsx(
            "flex min-w-0 items-center justify-center gap-1.5 rounded-md px-2 py-1.5 text-xs transition-colors",
            value === t.id ? "flex-[2_1_auto] bg-ink-3 text-fg" : "flex-1 text-fg-3 hover:text-fg-2",
            !t.icon && "flex-1",
          )}
        >
          {t.icon}
          {/* Abas inativas mostram só o ícone quando há ícone (o nome fica no tooltip). */}
          {(value === t.id || !t.icon) && <span className="truncate">{t.label}</span>}
        </button>
      ))}
    </div>
  );
}
