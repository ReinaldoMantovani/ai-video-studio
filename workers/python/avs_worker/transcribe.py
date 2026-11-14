"""Transcrição com faster-whisper (CTranslate2), com timestamps por palavra."""

from __future__ import annotations

import os

from .rpc import Context, RpcError

REPOS = {
    "tiny": "Systran/faster-whisper-tiny",
    "base": "Systran/faster-whisper-base",
    "small": "Systran/faster-whisper-small",
    "medium": "Systran/faster-whisper-medium",
    "large-v3": "Systran/faster-whisper-large-v3",
    "large-v3-turbo": "mobiuslabsgmbh/faster-whisper-large-v3-turbo",
}


def installed_models(models_dir: str) -> list[str]:
    root = os.path.join(models_dir, "whisper")
    if not os.path.isdir(root):
        return []
    return sorted(d for d in os.listdir(root) if os.path.isfile(os.path.join(root, d, "model.bin")))


def _device() -> tuple[str, str]:
    try:
        import ctranslate2

        if ctranslate2.get_cuda_device_count() > 0:
            return "cuda", "float16"
    except Exception:
        pass
    return "cpu", "int8"


def run(params: dict, ctx: Context) -> dict:
    try:
        from faster_whisper import WhisperModel
    except ImportError as e:
        raise RpcError("component_missing", "faster-whisper não instalado (componente 'Transcrição')") from e

    size = params.get("model", "small")
    model_dir = os.path.join(params["models_dir"], "whisper", size)
    if not os.path.isfile(os.path.join(model_dir, "model.bin")):
        raise RpcError("model_missing", f"modelo Whisper '{size}' não instalado; baixe-o no Gerenciador de IA")
    device, compute = _device()
    if params.get("device") == "cpu":
        device, compute = "cpu", "int8"
    ctx.progress("carregando modelo", 0.0)
    model = WhisperModel(model_dir, device=device, compute_type=compute, cpu_threads=int(params.get("threads", 0)) or 0)
    language = params.get("language") or None
    segments, info = model.transcribe(
        params["wav"],
        language=language,
        word_timestamps=True,
        vad_filter=False,
        condition_on_previous_text=False,
        beam_size=int(params.get("beam_size", 5)),
    )
    duration = float(info.duration or 0)
    out = []
    for seg in segments:  # gerador: decodifica incrementalmente
        ctx.check()
        words = [{"start": round(w.start, 3), "end": round(w.end, 3), "word": w.word, "prob": round(w.probability, 3)} for w in (seg.words or [])]
        out.append({"start": round(seg.start, 3), "end": round(seg.end, 3), "text": seg.text.strip(), "words": words, "avg_logprob": round(seg.avg_logprob, 3), "no_speech_prob": round(seg.no_speech_prob, 3)})
        if duration:
            ctx.progress("transcrevendo", seg.end / duration)
    ctx.progress("transcrevendo", 1.0)
    return {"language": info.language, "language_probability": round(info.language_probability, 3), "duration": duration, "model": size, "device": device, "segments": out}


def download(params: dict, ctx: Context) -> dict:
    """Baixa um modelo Whisper para `models_dir/whisper/<size>` com progresso."""
    try:
        from huggingface_hub import HfApi, hf_hub_download
    except ImportError as e:
        raise RpcError("component_missing", "huggingface_hub não instalado (componente 'Transcrição')") from e
    size = params["model"]
    repo = REPOS.get(size)
    if repo is None:
        raise RpcError("invalid_params", f"modelo desconhecido: {size}")
    dest = os.path.join(params["models_dir"], "whisper", size)
    tmp = dest + ".partial"
    os.makedirs(tmp, exist_ok=True)
    try:
        info = HfApi().model_info(repo, files_metadata=True)
    except Exception as e:
        raise RpcError("network", f"não foi possível consultar {repo}: {e}") from e
    files = [(s.rfilename, s.size or 0) for s in info.siblings if s.rfilename in ("model.bin", "config.json", "tokenizer.json", "vocabulary.txt", "vocabulary.json", "preprocessor_config.json")]
    total = sum(sz for _, sz in files) or 1
    done = 0
    for name, sz in files:
        ctx.check()
        hf_hub_download(repo, name, local_dir=tmp)
        done += sz
        ctx.progress("baixando modelo", done / total, file=name)
    if os.path.isdir(dest):
        import shutil

        shutil.rmtree(dest)
    os.replace(tmp, dest)
    return {"model": size, "path": dest, "bytes": total}
