"""Detecção de fala com Silero VAD (ONNX), sem PyTorch.

Reproduz o laço de inferência de referência do Silero (janelas de 512
amostras a 16 kHz com 64 de contexto e estado recorrente) e a lógica de
`get_speech_timestamps` (limiar com histerese, duração mínima de fala e de
silêncio, folga).
"""

from __future__ import annotations

import os
from importlib import util as importlib_util

import numpy as np

from .audio import chunks, open_wav
from .rpc import Context, RpcError

WINDOW = 512
CONTEXT = 64
SR = 16000


def model_path() -> str | None:
    spec = importlib_util.find_spec("silero_vad")
    if spec is None or not spec.submodule_search_locations:
        return None
    p = os.path.join(list(spec.submodule_search_locations)[0], "data", "silero_vad.onnx")
    return p if os.path.isfile(p) else None


def _session():
    import onnxruntime as ort

    p = model_path()
    if p is None:
        raise RpcError("component_missing", "modelo Silero VAD não instalado (componente 'Análise de fala')")
    opts = ort.SessionOptions()
    opts.inter_op_num_threads = 1
    opts.intra_op_num_threads = 1
    return ort.InferenceSession(p, sess_options=opts, providers=["CPUExecutionProvider"])


def probabilities(wav: str, ctx: Context) -> tuple[np.ndarray, float]:
    """Probabilidade de fala por janela de 32 ms."""
    w, sr, n = open_wav(wav)
    if sr != SR:
        raise RpcError("invalid_audio", f"VAD requer 16 kHz (recebido {sr})")
    sess = _session()
    state = np.zeros((2, 1, 128), dtype=np.float32)
    context = np.zeros((1, CONTEXT), dtype=np.float32)
    sr_arr = np.array(SR, dtype=np.int64)
    probs: list[float] = []
    total_windows = max(1, n // WINDOW)
    rest = np.zeros(0, dtype=np.float32)
    for block in chunks(w, WINDOW * 256):
        buf = np.concatenate([rest, block])
        k = len(buf) // WINDOW
        for i in range(k):
            x = buf[i * WINDOW : (i + 1) * WINDOW][None, :]
            inp = np.concatenate([context, x], axis=1)
            out, state = sess.run(None, {"input": inp, "state": state, "sr": sr_arr})
            context = inp[:, -CONTEXT:]
            probs.append(float(out[0][0]))
        rest = buf[k * WINDOW :]
        ctx.progress("detectando fala", len(probs) / total_windows)
    w.close()
    return np.array(probs, dtype=np.float32), n / SR


def segments(
    probs: np.ndarray,
    duration: float,
    threshold: float = 0.5,
    min_speech_ms: int = 250,
    min_silence_ms: int = 300,
    pad_ms: int = 60,
) -> list[list[float]]:
    """Trechos de fala [início, fim] em segundos."""
    neg = max(threshold - 0.15, 0.01)
    step = WINDOW / SR
    out: list[list[float]] = []
    in_speech = False
    start = 0.0
    silence_start: float | None = None
    for i, p in enumerate(probs):
        t = i * step
        if p >= threshold:
            silence_start = None
            if not in_speech:
                in_speech = True
                start = t
        elif in_speech and p < neg:
            if silence_start is None:
                silence_start = t
            if t + step - silence_start >= min_silence_ms / 1000:
                out.append([start, silence_start])
                in_speech = False
                silence_start = None
    if in_speech:
        out.append([start, len(probs) * step])
    pad = pad_ms / 1000
    res: list[list[float]] = []
    for s, e in out:
        if e - s < min_speech_ms / 1000:
            continue
        s, e = max(0.0, s - pad), min(duration, e + pad)
        if res and s <= res[-1][1]:
            res[-1][1] = max(res[-1][1], e)
        else:
            res.append([round(s, 3), round(e, 3)])
    return res


def run(params: dict, ctx: Context) -> dict:
    wav = params["wav"]
    probs, duration = probabilities(wav, ctx)
    segs = segments(
        probs,
        duration,
        threshold=float(params.get("threshold", 0.5)),
        min_speech_ms=int(params.get("min_speech_ms", 250)),
        min_silence_ms=int(params.get("min_silence_ms", 300)),
        pad_ms=int(params.get("pad_ms", 60)),
    )
    ctx.progress("detectando fala", 1.0)
    speech = sum(e - s for s, e in segs)
    return {"duration": duration, "speech": segs, "speech_ratio": round(speech / duration, 4) if duration else 0.0, "window_s": WINDOW / SR}
