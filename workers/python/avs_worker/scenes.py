"""Detecção de cortes de cena com PySceneDetect (decodificação via OpenCV)."""

from __future__ import annotations

from .rpc import Context, RpcError


def run(params: dict, ctx: Context) -> dict:
    try:
        from scenedetect import AdaptiveDetector, ContentDetector, SceneManager, open_video
    except ImportError as e:
        raise RpcError("component_missing", "PySceneDetect não instalado (componente 'Análise visual')") from e

    path = params["video"]
    try:
        video = open_video(path, backend="opencv")
    except Exception as e:
        raise RpcError("invalid_media", f"não foi possível abrir {path}: {e}") from e
    sm = SceneManager()
    if params.get("detector", "adaptive") == "content":
        sm.add_detector(ContentDetector(threshold=float(params.get("threshold", 27.0)), min_scene_len=int(params.get("min_scene_frames", 15))))
    else:
        sm.add_detector(AdaptiveDetector(min_scene_len=int(params.get("min_scene_frames", 15))))
    sm.auto_downscale = True

    duration = video.duration.get_seconds() if video.duration is not None else 0.0
    step = 20.0
    pos = 0.0
    # Processa em fatias para reportar progresso real e permitir cancelamento.
    while True:
        ctx.check()
        end = pos + step
        frames = sm.detect_scenes(video=video, end_time=end, show_progress=False)
        pos = video.position.get_seconds()
        if duration:
            ctx.progress("detectando cenas", pos / duration)
        if frames == 0 or (duration and pos >= duration - 1e-3):
            break
    scenes = sm.get_scene_list(start_in_scene=True)
    shots = [[round(s.get_seconds(), 3), round(e.get_seconds(), 3)] for s, e in scenes]
    cuts = [s for s, _ in shots[1:]]
    ctx.progress("detectando cenas", 1.0)
    return {"duration": duration, "cuts": cuts, "shots": shots, "fps": float(video.frame_rate)}
