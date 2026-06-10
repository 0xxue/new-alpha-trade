"""auth 凭据查询路由（给 trading-engine 用）。"""
from __future__ import annotations

from datetime import datetime, timedelta, timezone

from fastapi import APIRouter, HTTPException, Request, Response, status
from pydantic import BaseModel

from qr_service.storage import Storage

router = APIRouter()

# Binance Alpha token 有效期 5 天（参考旧项目）
TOKEN_TTL_DAYS = 5


class AuthBundle(BaseModel):
    """trading-engine 通过 GET /auth/{username} 拿到的内容。"""

    username: str
    cookies: dict[str, str]
    headers: dict[str, str]
    last_refresh: str | None = None
    expires_at_ms: int | None = None
    status: str
    twofa_secret: str | None = None  # base32，可空


class AccountSummary(BaseModel):
    username: str
    last_refresh: str | None
    expires_at_ms: int | None
    status: str
    has_2fa: bool


class TwofaUpdate(BaseModel):
    secret: str | None  # null 或 "" 清除


def _storage(req: Request) -> Storage:
    storage: Storage | None = getattr(req.app.state, "storage", None)
    if storage is None:
        raise HTTPException(status.HTTP_503_SERVICE_UNAVAILABLE, "storage not ready")
    return storage


def _compute_expiry(last_refresh: str | None) -> int | None:
    if not last_refresh:
        return None
    try:
        dt = datetime.fromisoformat(last_refresh)
        if dt.tzinfo is None:
            dt = dt.replace(tzinfo=timezone.utc)
        return int((dt + timedelta(days=TOKEN_TTL_DAYS)).timestamp() * 1000)
    except ValueError:
        return None


@router.get("/{username}", response_model=AuthBundle)
async def get_auth(username: str, req: Request) -> AuthBundle:
    storage = _storage(req)
    acc = await storage.get_account(username)
    if acc is None:
        raise HTTPException(status.HTTP_404_NOT_FOUND, f"account {username!r} not found")
    return AuthBundle(
        username=acc.username,
        cookies=acc.cookies,
        headers=acc.headers,
        last_refresh=acc.last_refresh,
        expires_at_ms=_compute_expiry(acc.last_refresh),
        status=acc.status,
        twofa_secret=acc.twofa_secret,
    )


@router.get("/{username}/2fa-code")
async def get_current_2fa_code(username: str, req: Request) -> dict:
    """返回当前 TOTP 6 位码 + 剩余秒数。让用户对照手机 Google Authenticator 是否一致。

    secret 在服务端算，不出库。
    """
    import time

    try:
        import pyotp
    except ImportError:
        raise HTTPException(500, "pyotp not installed on server")

    storage = _storage(req)
    acc = await storage.get_account(username)
    if acc is None:
        raise HTTPException(status.HTTP_404_NOT_FOUND, f"account {username!r} not found")
    if not acc.twofa_secret:
        raise HTTPException(status.HTTP_400_BAD_REQUEST, "account has no 2FA secret")

    try:
        totp = pyotp.TOTP(acc.twofa_secret)
        code = totp.now()
    except Exception as e:  # noqa: BLE001
        raise HTTPException(400, f"TOTP generation failed: {e}")

    now = time.time()
    step = 30
    remaining = step - (now % step)
    return {
        "username": username,
        "code": code,
        "remaining_seconds": round(remaining, 1),
        "step_seconds": step,
    }


@router.put("/{username}/2fa-secret")
async def set_twofa_secret(username: str, body: TwofaUpdate, req: Request) -> dict:
    """设置/清除某账户的 2FA TOTP secret（base32）。"""
    storage = _storage(req)
    acc = await storage.get_account(username)
    if acc is None:
        raise HTTPException(status.HTTP_404_NOT_FOUND, f"account {username!r} not found")
    secret = (body.secret or "").strip().upper() or None
    if secret is not None:
        # 简单校验：base32 字符集
        import re
        if not re.fullmatch(r"[A-Z2-7=]+", secret):
            raise HTTPException(status.HTTP_400_BAD_REQUEST, "secret must be base32 (A-Z, 2-7)")
        if len(secret) < 16:
            raise HTTPException(status.HTTP_400_BAD_REQUEST, "secret too short (need ≥16 base32 chars)")
    # upsert（保留 cookies/headers，仅改 twofa_secret）
    async with storage._conn() as db:  # noqa: SLF001
        await db.execute(
            "UPDATE accounts SET twofa_secret = ? WHERE username = ?",
            (secret, username),
        )
    return {"username": username, "has_2fa": secret is not None}


@router.post("/{username}/refresh")
async def refresh_auth(username: str, req: Request) -> dict:
    """手动触发对该账户的一次有效性探测。返回 ok / expired / error。"""
    storage = _storage(req)
    acc = await storage.get_account(username)
    if acc is None:
        raise HTTPException(status.HTTP_404_NOT_FOUND, f"account {username!r} not found")
    refresher = getattr(req.app.state, "refresher", None)
    if refresher is None:
        raise HTTPException(status.HTTP_503_SERVICE_UNAVAILABLE, "refresher not ready")
    import httpx

    async with httpx.AsyncClient(timeout=10.0) as client:
        outcome = await refresher._probe_one(client, acc)  # noqa: SLF001
    return {"username": username, "result": outcome}


@router.get("", response_model=list[AccountSummary])
async def list_accounts(req: Request) -> list[AccountSummary]:
    storage = _storage(req)
    accs = await storage.list_accounts()
    return [
        AccountSummary(
            username=a.username,
            last_refresh=a.last_refresh,
            expires_at_ms=_compute_expiry(a.last_refresh),
            status=a.status,
            has_2fa=bool(a.twofa_secret),
        )
        for a in accs
    ]


@router.delete("/{username}", response_class=Response)
async def delete_account(username: str, req: Request) -> Response:
    storage = _storage(req)
    ok = await storage.delete_account(username)
    if not ok:
        raise HTTPException(status.HTTP_404_NOT_FOUND, f"account {username!r} not found")
    return Response(status_code=status.HTTP_204_NO_CONTENT)
