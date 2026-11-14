"""JSON-RPC 2.0 por stdin/stdout (uma mensagem por linha).

- Uma requisição por vez é executada numa thread; o laço principal continua
  lendo stdin para receber `$/cancel` a qualquer momento.
- Progresso real é enviado como notificação `$/progress`.
- Logs vão para stderr; stdout é exclusivo do protocolo.
"""

from __future__ import annotations

import json
import sys
import threading
import traceback
from typing import Any, Callable

_out_lock = threading.Lock()


class Cancelled(Exception):
    pass


class RpcError(Exception):
    def __init__(self, code: str, message: str):
        super().__init__(message)
        self.code = code
        self.message = message


def send(msg: dict) -> None:
    line = json.dumps(msg, ensure_ascii=False, separators=(",", ":"))
    with _out_lock:
        sys.stdout.write(line + "\n")
        sys.stdout.flush()


class Context:
    """Passado a cada método: progresso e verificação de cancelamento."""

    def __init__(self, req_id: Any):
        self.req_id = req_id
        self._cancel = threading.Event()
        self._last = -1.0

    def cancel(self) -> None:
        self._cancel.set()

    def check(self) -> None:
        if self._cancel.is_set():
            raise Cancelled()

    def progress(self, stage: str, fraction: float, **extra: Any) -> None:
        self.check()
        f = max(0.0, min(1.0, float(fraction)))
        # Limita a frequência: só envia mudanças de ≥0,5 % ou fim.
        if f < 1.0 and abs(f - self._last) < 0.005:
            return
        self._last = f
        send({"jsonrpc": "2.0", "method": "$/progress", "params": {"id": self.req_id, "stage": stage, "fraction": f, **extra}})


Handler = Callable[[dict, Context], Any]


def serve(methods: dict[str, Handler]) -> None:
    current: dict[str, Any] = {"ctx": None, "thread": None}

    def run(req: dict) -> None:
        rid = req.get("id")
        ctx = Context(rid)
        current["ctx"] = ctx
        try:
            fn = methods.get(req.get("method", ""))
            if fn is None:
                raise RpcError("method_not_found", f"método desconhecido: {req.get('method')}")
            result = fn(req.get("params") or {}, ctx)
            send({"jsonrpc": "2.0", "id": rid, "result": result})
        except Cancelled:
            send({"jsonrpc": "2.0", "id": rid, "error": {"code": "cancelled", "message": "operação cancelada"}})
        except RpcError as e:
            send({"jsonrpc": "2.0", "id": rid, "error": {"code": e.code, "message": e.message}})
        except Exception as e:  # erro real, com detalhe para o log
            traceback.print_exc(file=sys.stderr)
            send({"jsonrpc": "2.0", "id": rid, "error": {"code": "internal", "message": f"{type(e).__name__}: {e}"}})
        finally:
            current["ctx"] = None

    for raw in sys.stdin:
        raw = raw.strip()
        if not raw:
            continue
        try:
            req = json.loads(raw)
        except json.JSONDecodeError as e:
            send({"jsonrpc": "2.0", "id": None, "error": {"code": "parse_error", "message": str(e)}})
            continue
        if req.get("method") == "$/cancel":
            ctx = current["ctx"]
            if ctx is not None:
                ctx.cancel()
            continue
        if req.get("method") == "shutdown":
            send({"jsonrpc": "2.0", "id": req.get("id"), "result": None})
            break
        t = current["thread"]
        if t is not None and t.is_alive():
            send({"jsonrpc": "2.0", "id": req.get("id"), "error": {"code": "busy", "message": "worker ocupado"}})
            continue
        t = threading.Thread(target=run, args=(req,), daemon=True)
        current["thread"] = t
        t.start()
    t = current["thread"]
    if t is not None:
        if current["ctx"] is not None:
            current["ctx"].cancel()
        t.join(timeout=5)
