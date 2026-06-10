"""Token 续期/有效性巡检后台任务。

思路（简化版）：
- 每 N 分钟扫一遍所有 active 账户
- 用账户的 cookies + headers 调一个**轻量私有端点**（cloud-wallet/alpha）
- 200 + success=true → 更新 last_refresh
- 4xx → mark_expired，前端会提示重新扫码
- 真正"自动续期"币安并不支持，过期就只能重新扫码
"""
from __future__ import annotations

import asyncio
import logging
from datetime import datetime, timezone

import httpx

from qr_service.storage import Account, Storage

logger = logging.getLogger("qr_service.refresh")

# 用一个轻量的私有端点做有效性探测（不会下单不会改状态）。
# 改用旧 web_qr_server.py 验证可行的 URL（get-user-base-info）— alpha endpoint 对 cookie
# 严格度更高（require 长期 r30t），实测会假报 expired。base-info 只要 csrftoken+cookie 够。
PROBE_URL = "https://www.binance.com/bapi/accounts/v1/private/account/get-user-base-info"
PROBE_TIMEOUT_S = 10.0
# 旧 web_qr_server.py L181-190 验证过的最小 header 集，多余 headers（content-length / origin
# / :authority 等）会让 binance 拒绝。
PROBE_HEADER_KEYS = {
    "bnc-uuid",
    "csrftoken",
    "device-info",
    "fvideo-id",
    "fvideo-token",
    "user-agent",
}


class TokenRefresher:
    def __init__(
        self,
        storage: Storage,
        interval_s: float = 3600.0,
        initial_delay_s: float = 30.0,
    ) -> None:
        self.storage = storage
        self.interval_s = interval_s
        self.initial_delay_s = initial_delay_s
        self._task: asyncio.Task | None = None
        self._stop = asyncio.Event()

    def start(self) -> None:
        if self._task is None or self._task.done():
            self._task = asyncio.create_task(self._loop(), name="token-refresher")
            logger.info("token refresher started (interval=%.0fs)", self.interval_s)

    async def stop(self) -> None:
        self._stop.set()
        if self._task is not None and not self._task.done():
            self._task.cancel()
            try:
                await self._task
            except (asyncio.CancelledError, Exception):  # noqa: BLE001
                pass

    async def _loop(self) -> None:
        try:
            await asyncio.sleep(self.initial_delay_s)
        except asyncio.CancelledError:
            return
        while not self._stop.is_set():
            try:
                await self.run_once()
            except Exception:  # noqa: BLE001
                logger.exception("refresh round failed")
            try:
                await asyncio.wait_for(self._stop.wait(), timeout=self.interval_s)
            except asyncio.TimeoutError:
                pass

    async def run_once(self) -> dict[str, str]:
        """跑一轮全量探测，返回 {username: 'ok'|'expired'|'error'} 的快照。"""
        accs = await self.storage.list_accounts()
        result: dict[str, str] = {}
        async with httpx.AsyncClient(timeout=PROBE_TIMEOUT_S, follow_redirects=False) as client:
            for acc in accs:
                if acc.status != "active":
                    result[acc.username] = "skipped"
                    continue
                outcome = await self._probe_one(client, acc)
                result[acc.username] = outcome
        return result

    async def _probe_one(self, client: httpx.AsyncClient, acc: Account) -> str:
        cookies = acc.cookies or {}
        # 只发关键 headers + 必要硬编码常量（仿旧 verify_token L181-190 ）
        src = acc.headers or {}
        src_lower = {k.lower(): v for k, v in src.items()}
        headers: dict[str, str] = {
            "accept": "*/*",
            "clienttype": "web",
            "content-type": "application/json",
            "lang": "zh-CN",
        }
        for key in PROBE_HEADER_KEYS:
            if key in src_lower and src_lower[key]:
                headers[key] = src_lower[key]
        try:
            resp = await client.get(PROBE_URL, cookies=cookies, headers=headers)
        except httpx.HTTPError as e:
            logger.warning("[%s] probe network error: %s", acc.username, e)
            return "error"
        if 200 <= resp.status_code < 300:
            try:
                body = resp.json()
                ok = bool(body.get("success", True))
            except ValueError:
                ok = True
            if ok:
                await self._touch_refresh(acc.username)
                logger.info("[%s] token healthy", acc.username)
                return "ok"
            # 200 但 success=false 也算过期（比如 csrf 失效）
            await self.storage.mark_expired(acc.username)
            logger.warning("[%s] token rejected by server (body.success=false)", acc.username)
            return "expired"
        if resp.status_code in (401, 403):
            await self.storage.mark_expired(acc.username)
            logger.warning("[%s] token expired (HTTP %s)", acc.username, resp.status_code)
            return "expired"
        logger.warning("[%s] probe unexpected HTTP %s", acc.username, resp.status_code)
        return "error"

    async def _touch_refresh(self, username: str) -> None:
        now = datetime.now(timezone.utc).isoformat(timespec="seconds")
        # 用 SQL 直接更新 last_refresh，不走 upsert（保留 cookies/headers 原样）
        async with self.storage._conn() as db:  # noqa: SLF001
            await db.execute(
                "UPDATE accounts SET last_refresh = ?, status = 'active' WHERE username = ?",
                (now, username),
            )
