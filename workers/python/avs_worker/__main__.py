"""Ponto de entrada: `python -m avs_worker`."""

from __future__ import annotations

import importlib
import platform
import sys

from . import rpc


def capabilities(params: dict, ctx: rpc.Context) -> dict:
    """O que está realmente instalado e utilizável neste ambiente."""

    def version(mod: str):
        try:
            m = importlib.import_module(mod)
            return getattr(m, "__version__", "ok")
        except Exception:
            return None

    from . import transcribe, vad

    caps = {
        "python": platform.python_version(),
        "numpy": version("numpy"),
        "onnxruntime": version("onnxruntime"),
        "opencv": version("cv2"),
        "scenedetect": version("scenedetect"),
        "faster_whisper": version("faster_whisper"),
        "huggingface_hub": version("huggingface_hub"),
        "silero_vad_model": vad.model_path() is not None,
        "whisper_models": transcribe.installed_models(params["models_dir"]) if params.get("models_dir") else [],
        "cuda_devices": 0,
        "onnx_providers": [],
    }
    try:
        import onnxruntime as ort

        caps["onnx_providers"] = ort.get_available_providers()
    except Exception:
        pass
    try:
        import ctranslate2

        caps["cuda_devices"] = ctranslate2.get_cuda_device_count()
    except Exception:
        pass
    return caps


def main() -> None:
    from . import scenes, transcribe, vad, visual

    rpc.serve(
        {
            "ping": lambda p, c: "pong",
            "capabilities": capabilities,
            "vad": vad.run,
            "scenes": scenes.run,
            "visual": visual.run,
            "transcribe": transcribe.run,
            "download_whisper": transcribe.download,
        }
    )


if __name__ == "__main__":
    sys.exit(main())
