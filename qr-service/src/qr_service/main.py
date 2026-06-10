"""FastAPI entry point for qr-service."""
from __future__ import annotations

import logging
import os
from contextlib import asynccontextmanager
from pathlib import Path

from fastapi import FastAPI

from qr_service import __version__
from qr_service.api import auth, face, qr
from qr_service.face_verify import FaceVerifyManager
from qr_service.playwright_login import LoginSessionManager
from qr_service.storage import Storage
from qr_service.token_refresh import TokenRefresher

logger = logging.getLogger("qr_service")


def _resolve_path(env_var: str, default: str) -> Path:
    raw = os.environ.get(env_var, default)
    p = Path(raw)
    if not p.is_absolute():
        p = (Path(__file__).resolve().parents[2] / raw).resolve()
    return p


@asynccontextmanager
async def lifespan(app: FastAPI):
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s | %(message)s",
        datefmt="%H:%M:%S",
    )
    db_path = _resolve_path("DB_PATH", "../data/new-alpha-trade.db")
    qr_dir = _resolve_path("QR_DIR", "../data/qr")
    face_qr_dir = _resolve_path("FACE_QR_DIR", "../data/face_qr")
    pw_state_dir = _resolve_path("PLAYWRIGHT_STATE_DIR", "../data/playwright-state")
    headless = os.environ.get("PLAYWRIGHT_HEADLESS", "true").lower() in ("1", "true", "yes")

    storage = Storage(db_path)
    if not await storage.has_schema():
        logger.warning(
            "schema missing in %s — start trading-engine first to run migrations", db_path
        )
    login_mgr = LoginSessionManager(
        storage=storage,
        qr_dir=qr_dir,
        playwright_state_dir=pw_state_dir,
        headless=headless,
    )
    face_mgr = FaceVerifyManager(
        playwright_state_dir=pw_state_dir,
        face_qr_dir=face_qr_dir,
        headless=headless,
    )
    refresh_interval = float(os.environ.get("REFRESH_INTERVAL_S", "3600"))
    refresher = TokenRefresher(storage=storage, interval_s=refresh_interval)

    app.state.storage = storage
    app.state.login_mgr = login_mgr
    app.state.face_mgr = face_mgr
    app.state.refresher = refresher

    refresher.start()
    logger.info(
        "qr-service ready (db=%s, qr=%s, face_qr=%s, headless=%s, refresh=%.0fs)",
        db_path,
        qr_dir,
        face_qr_dir,
        headless,
        refresh_interval,
    )
    try:
        yield
    finally:
        logger.info("qr-service shutting down")
        await refresher.stop()
        await login_mgr.shutdown()
        await face_mgr.shutdown()


app = FastAPI(
    title="new-alpha-trade QR service",
    version=__version__,
    lifespan=lifespan,
    # 反代后 307 redirect 会丢失 nginx 路径前缀 → 强制不 redirect，前端必须用准确路径
    redirect_slashes=False,
)

app.include_router(qr.router, prefix="/qr", tags=["qr"])
app.include_router(auth.router, prefix="/auth", tags=["auth"])
app.include_router(face.router, prefix="/face", tags=["face"])


@app.get("/health")
async def health() -> dict:
    return {"status": "ok", "version": __version__, "service": "qr-service"}


if __name__ == "__main__":
    import uvicorn

    port = int(os.environ.get("PORT", "7001"))
    uvicorn.run("qr_service.main:app", host="127.0.0.1", port=port, reload=True)
