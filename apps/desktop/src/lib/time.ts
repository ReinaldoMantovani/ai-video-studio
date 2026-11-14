// Tempo da timeline em flicks (1/705 600 000 s), igual ao núcleo Rust.
import type { Rational } from "../bindings/Rational";

export const FLICKS = 705_600_000;

export const secs = (t: number) => t / FLICKS;
export const fromSecs = (s: number) => Math.round(s * FLICKS);

export function frameDuration(fps: Rational): number {
  return Math.floor((fps.den * FLICKS) / fps.num);
}

/** Índice do frame que contém `t`. */
export function frameOf(t: number, fps: Rational): number {
  return Math.floor((t * fps.num) / (fps.den * FLICKS) + 1e-9);
}

export function fromFrames(n: number, fps: Rational): number {
  return Math.floor((n * fps.den * FLICKS) / fps.num);
}

export function snapToFrame(t: number, fps: Rational): number {
  return fromFrames(Math.round((t * fps.num) / (fps.den * FLICKS)), fps);
}

/** `HH:MM:SS:FF` (timecode por frame). */
export function timecode(t: number, fps: Rational): string {
  const rate = Math.round(fps.num / fps.den);
  const f = Math.max(0, frameOf(t, fps));
  const total = Math.floor(secs(fromFrames(f, fps)) + 1e-9);
  const ff = f - Math.round((total * fps.num) / fps.den);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(Math.floor(total / 3600))}:${p(Math.floor(total / 60) % 60)}:${p(total % 60)}:${p(Math.max(0, Math.min(rate - 1, ff)))}`;
}

/** `1:05`, `12:30`, `1:02:03` — durações legíveis. */
export function shortDuration(t: number): string {
  const s = Math.round(secs(t));
  const h = Math.floor(s / 3600);
  const m = Math.floor(s / 60) % 60;
  const ss = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

export function fpsLabel(fps: Rational): string {
  const v = fps.num / fps.den;
  return Number.isInteger(v) ? `${v}` : v.toFixed(2);
}

export function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const u = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v >= 100 ? 0 : 1)} ${u[i]}`;
}

/**
 * Snap magnético: aproxima `t` do alvo mais próximo dentro de `tolerance`.
 * Retorna o alvo usado (para desenhar a guia) ou null.
 */
export function snap(t: number, targets: number[], tolerance: number): { t: number; target: number | null } {
  let best: number | null = null;
  let dist = tolerance;
  for (const x of targets) {
    const d = Math.abs(x - t);
    if (d <= dist) {
      dist = d;
      best = x;
    }
  }
  return best === null ? { t, target: null } : { t: best, target: best };
}

/** Interpolação de keyframes idêntica à do núcleo Rust (`interpolate`). */
export function interpolate(kfs: { at: number; value: number; easing: "linear" | "ease_in_out" | "hold" }[], t: number, dflt: number): number {
  if (kfs.length === 0) return dflt;
  if (t <= kfs[0].at) return kfs[0].value;
  for (let i = 0; i < kfs.length - 1; i++) {
    const a = kfs[i];
    const b = kfs[i + 1];
    if (t < b.at) {
      let u = (t - a.at) / Math.max(1, b.at - a.at);
      if (a.easing === "ease_in_out") u = u * u * (3 - 2 * u);
      if (a.easing === "hold") u = 0;
      return a.value + (b.value - a.value) * u;
    }
  }
  return kfs[kfs.length - 1].value;
}
