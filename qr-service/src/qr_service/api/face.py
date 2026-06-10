"""人脸/手机验证截图 API。

- POST /face/{username}/trigger   触发 Playwright 走完整流程（下单 → 弹窗 → 点手机验证 → 截图）
- GET  /face/{username}/qr        返回最近一次截图 PNG
- GET  /face/{username}/status    返回当前 session 状态 JSON
"""
from __future__ import annotations

import logging
from typing import Annotated

from fastapi import APIRouter, HTTPException, Request
from fastapi.responses import FileResponse, JSONResponse
from pydantic import BaseModel, Field

logger = logging.getLogger("qr_service.api.face")

router = APIRouter()


class TriggerReq(BaseModel):
    symbol: str = Field(..., description="Alpha id (e.g. ALPHA_971) 或 base symbol (e.g. NEX)")
    amount_usdt: float = Field(10.0, ge=0.5, le=100.0, description="触发金额，默认 10 USDT")


@router.post("/{username}/trigger")
async def trigger_face(username: str, body: TriggerReq, request: Request) -> JSONResponse:
    mgr = request.app.state.face_mgr
    sess = await mgr.trigger(username=username, symbol=body.symbol, amount_usdt=body.amount_usdt)
    return JSONResponse(
        {
            "username": sess.username,
            "status": sess.status,
            "message": sess.message,
            "screenshot_available": sess.screenshot_path is not None
            and sess.screenshot_path.exists(),
            "started_at": sess.started_at,
            "finished_at": sess.finished_at,
        }
    )


@router.get("/{username}/status")
async def face_status(username: str, request: Request) -> JSONResponse:
    mgr = request.app.state.face_mgr
    sess = mgr.get_session(username)
    if sess is None:
        return JSONResponse(
            {"username": username, "status": "idle", "message": "never triggered"}
        )
    return JSONResponse(
        {
            "username": sess.username,
            "status": sess.status,
            "message": sess.message,
            "screenshot_available": sess.screenshot_path is not None
            and sess.screenshot_path.exists(),
            "started_at": sess.started_at,
            "finished_at": sess.finished_at,
        }
    )


@router.get("/{username}/qr")
async def face_qr(username: str, request: Request) -> FileResponse:
    mgr = request.app.state.face_mgr
    path = mgr.screenshot_path_for(username)
    if not path.exists():
        raise HTTPException(status_code=404, detail="no screenshot yet — call POST /trigger first")
    return FileResponse(str(path), media_type="image/png")
