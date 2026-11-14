"""Análise visual amostrada: movimento, brilho e rostos.

Não analisa todos os frames: amostra a cada `interval` segundos e, em cada
ponto, compara dois frames consecutivos para medir movimento real.
Rostos: YuNet (ONNX, se o modelo estiver instalado) ou Haar cascade
(empacotado no OpenCV) como alternativa.
"""

from __future__ import annotations

import os

from .rpc import Context, RpcError


def _face_detector(model_dir: str | None):
    import cv2

    yunet = os.path.join(model_dir, "face_detection_yunet_2023mar.onnx") if model_dir else None
    if yunet and os.path.isfile(yunet) and hasattr(cv2, "FaceDetectorYN"):
        det = cv2.FaceDetectorYN.create(yunet, "", (320, 320), 0.7, 0.3, 50)

        def run_yunet(img):
            h, w = img.shape[:2]
            det.setInputSize((w, h))
            _, faces = det.detect(img)
            out = []
            if faces is not None:
                for f in faces:
                    x, y, fw, fh, score = float(f[0]), float(f[1]), float(f[2]), float(f[3]), float(f[-1])
                    out.append({"x": round(x / w, 4), "y": round(y / h, 4), "w": round(fw / w, 4), "h": round(fh / h, 4), "score": round(score, 3)})
            return out

        return "yunet", run_yunet
    if not hasattr(cv2, "CascadeClassifier"):
        # OpenCV 5 removeu as cascatas Haar: sem o modelo YuNet não há detector.
        return None, None
    casc = cv2.CascadeClassifier(os.path.join(cv2.data.haarcascades, "haarcascade_frontalface_default.xml"))

    def run_haar(img):
        h, w = img.shape[:2]
        gray = cv2.cvtColor(img, cv2.COLOR_BGR2GRAY)
        rects = casc.detectMultiScale(gray, scaleFactor=1.15, minNeighbors=6, minSize=(max(24, w // 30), max(24, h // 30)))
        return [{"x": round(x / w, 4), "y": round(y / h, 4), "w": round(fw / w, 4), "h": round(fh / h, 4), "score": None} for (x, y, fw, fh) in rects]

    return "haar", run_haar


def run(params: dict, ctx: Context) -> dict:
    try:
        import cv2
        import numpy as np
    except ImportError as e:
        raise RpcError("component_missing", "OpenCV não instalado (componente 'Análise visual')") from e

    path = params["video"]
    interval = max(0.1, float(params.get("interval", 1.0)))
    detect_faces = bool(params.get("faces", True))
    cap = cv2.VideoCapture(path)
    if not cap.isOpened():
        raise RpcError("invalid_media", f"não foi possível abrir {path}")
    fps = cap.get(cv2.CAP_PROP_FPS) or 30.0
    nframes = cap.get(cv2.CAP_PROP_FRAME_COUNT) or 0
    duration = float(params.get("duration") or (nframes / fps if nframes else 0))
    kind, faces_fn = _face_detector(params.get("model_dir")) if detect_faces else (None, None)

    samples = []
    t = 0.0
    width = 320
    while duration == 0 or t < duration:
        ctx.check()
        cap.set(cv2.CAP_PROP_POS_MSEC, t * 1000.0)
        ok1, f1 = cap.read()
        ok2, f2 = cap.read()
        if not ok1:
            break
        h0, w0 = f1.shape[:2]
        scale = width / w0
        small1 = cv2.resize(f1, (width, max(2, int(h0 * scale))), interpolation=cv2.INTER_AREA)
        g1 = cv2.cvtColor(small1, cv2.COLOR_BGR2GRAY)
        motion = 0.0
        if ok2:
            small2 = cv2.resize(f2, (small1.shape[1], small1.shape[0]), interpolation=cv2.INTER_AREA)
            g2 = cv2.cvtColor(small2, cv2.COLOR_BGR2GRAY)
            motion = float(np.mean(cv2.absdiff(g1, g2))) / 255.0
        s = {"t": round(t, 3), "motion": round(motion, 5), "luma": round(float(np.mean(g1)) / 255.0, 4)}
        if faces_fn is not None:
            # Rostos numa versão de 640 px (equilíbrio entre custo e alcance).
            fw = 640
            fs = fw / w0
            img = cv2.resize(f1, (fw, max(2, int(h0 * fs))), interpolation=cv2.INTER_AREA)
            s["faces"] = faces_fn(img)
        samples.append(s)
        if duration:
            ctx.progress("analisando imagem", t / duration)
        t += interval
    cap.release()
    ctx.progress("analisando imagem", 1.0)
    return {"duration": duration, "interval": interval, "face_detector": kind, "samples": samples}
