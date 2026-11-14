"""Leitura incremental de WAV PCM 16 kHz mono (gerado pelo app com FFmpeg)."""

from __future__ import annotations

import wave
from typing import Iterator

import numpy as np

from .rpc import RpcError


def open_wav(path: str) -> tuple[wave.Wave_read, int, int]:
    try:
        w = wave.open(path, "rb")
    except (FileNotFoundError, wave.Error) as e:
        raise RpcError("invalid_audio", f"não foi possível ler {path}: {e}") from e
    if w.getnchannels() != 1 or w.getsampwidth() != 2:
        raise RpcError("invalid_audio", "esperado WAV PCM 16 bits mono")
    return w, w.getframerate(), w.getnframes()


def chunks(w: wave.Wave_read, size: int) -> Iterator[np.ndarray]:
    """Blocos float32 em [-1, 1]; memória constante para arquivos de horas."""
    while True:
        b = w.readframes(size)
        if not b:
            return
        yield np.frombuffer(b, dtype="<i2").astype(np.float32) / 32768.0
