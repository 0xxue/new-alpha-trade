"""扫码登录路由。"""
from __future__ import annotations

from fastapi import APIRouter, HTTPException, Request, Response, status
from fastapi.responses import FileResponse
from pydantic import BaseModel

from qr_service.playwright_login import LoginSessionManager

router = APIRouter()


def _manager(req: Request) -> LoginSessionManager:
    mgr = getattr(req.app.state, "login_mgr", None)
    if mgr is None:
        raise HTTPException(status.HTTP_503_SERVICE_UNAVAILABLE, "login manager not ready")
    return mgr


class LoginRequest(BaseModel):
    username: str


class LoginResponse(BaseModel):
    session_id: str
    status: str
    qr_image_url: str


class StatusResponse(BaseModel):
    session_id: str
    username: str
    status: str
    error: str | None = None
    qr_image_url: str | None = None


@router.post("/login", response_model=LoginResponse)
async def start_login(body: LoginRequest, req: Request) -> LoginResponse:
    if not body.username or not body.username.strip():
        raise HTTPException(status.HTTP_400_BAD_REQUEST, "username required")
    mgr = _manager(req)
    sess = mgr.start_login(body.username.strip())
    return LoginResponse(
        session_id=sess.session_id,
        status=sess.status,
        qr_image_url=f"/qr/image/{sess.session_id}",
    )


@router.get("/status/{session_id}", response_model=StatusResponse)
async def status_endpoint(session_id: str, req: Request) -> StatusResponse:
    mgr = _manager(req)
    sess = mgr.get(session_id)
    if sess is None:
        raise HTTPException(status.HTTP_404_NOT_FOUND, "session not found")
    return StatusResponse(
        session_id=sess.session_id,
        username=sess.username,
        status=sess.status,
        error=sess.error,
        qr_image_url=f"/qr/image/{sess.session_id}" if sess.qr_image_path else None,
    )


@router.get("/image/{session_id}")
async def qr_image(session_id: str, req: Request) -> FileResponse:
    mgr = _manager(req)
    sess = mgr.get(session_id)
    if sess is None or sess.qr_image_path is None:
        raise HTTPException(status.HTTP_404_NOT_FOUND, "QR image not ready")
    return FileResponse(sess.qr_image_path, media_type="image/png")


@router.delete("/{session_id}", response_class=Response)
async def cancel_login(session_id: str, req: Request) -> Response:
    mgr = _manager(req)
    if not mgr.cancel(session_id):
        raise HTTPException(status.HTTP_404_NOT_FOUND, "session not found")
    return Response(status_code=status.HTTP_204_NO_CONTENT)


@router.post("/refresh/{session_id}", response_class=Response)
async def refresh_qr(session_id: str, req: Request) -> Response:
    """手动触发 QR 刷新：playwright 会 reload + 重点 QR 按钮 + 重截图。"""
    mgr = _manager(req)
    if not mgr.request_refresh(session_id):
        raise HTTPException(status.HTTP_404_NOT_FOUND, "session not found")
    return Response(status_code=status.HTTP_204_NO_CONTENT)
