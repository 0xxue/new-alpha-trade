"""Playwright 扫码核心。

设计：
- `LoginSessionManager` 持有所有进行中的扫码 session
- 每个 session 一个 `asyncio.Task`，独立跑 Playwright 浏览器
- 状态机：pending -> scanned -> success / expired / failed
- 服务器端无头模式，QR 图定时截图存到 data/qr/{session_id}.png，前端轮询
- 监听 `/bapi/accounts/v1/public/authcenter/callback` 标志登录开始
- 随后任意 `/bapi/.../auth` 或带完整 cookie 的 API 请求 → 抓 headers
- `context.cookies()` 抓 cookies → SQLite

关键点：完全沿用旧项目 `web_qr_server.py` 验证可行的反检测 init_script
和元素选择器。
"""
from __future__ import annotations

import asyncio
import logging
import time
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import Literal

from playwright.async_api import (
    BrowserContext,
    Page,
    Playwright,
    Request,
    async_playwright,
)

from qr_service.storage import Storage

logger = logging.getLogger("qr_service.login")

SessionStatus = Literal["pending", "qr_ready", "scanned", "success", "expired", "failed"]

LOGIN_URL = "https://www.binance.com/zh-CN/login"
CALLBACK_PATH = "/bapi/accounts/v1/public/authcenter/callback"
AUTH_PATH = "/bapi/accounts/v1/public/authcenter/auth"
BAPI_PATTERN = "binance.com/bapi/"

# 反检测脚本（沿用旧项目）
INIT_SCRIPT = """
Object.defineProperty(navigator, 'webdriver', { get: () => undefined });
Object.defineProperty(navigator, 'languages', { get: () => ['zh-CN', 'zh', 'en-US', 'en'] });
Object.defineProperty(navigator, 'platform', { get: () => 'Win32' });
Object.defineProperty(navigator, 'hardwareConcurrency', { get: () => 8 });
Object.defineProperty(navigator, 'deviceMemory', { get: () => 8 });
delete window.playwright;
delete window.__playwright;
delete window.__pw_manual;
delete window.__PW_inspect;
window.chrome = window.chrome || { runtime: {}, loadTimes: function() {}, csi: function() {}, app: {} };
"""

QR_SELECTORS = [
    '[aria-label="QR Code status"]',
    '[aria-label="二维码状态"]',
    'canvas',
]

# Binance 时不时改 HTML，多准备几个变体覆盖中英文 + 新旧 class
QR_LOGIN_BUTTON_SELECTORS = [
    # 2026-06 实测有效（点它会弹出真二维码 canvas）
    '[aria-label="二维码登录"]',
    '[aria-label="QR Code Login"]',
    '[aria-label="QR code login"]',
    '[aria-label="QR Login"]',
    ".qr-login-icon",  # 旧 class，有时能点到但不弹二维码 → 靠下面 canvas 校验兜底
    'button[class*="qr-login"]',
    'svg[class*="qr-login"]',
    # 通用：登录表单旁的"二维码图标"，按位置选（form 内右上角的 button/div）
    'form button:has(svg)',
]

ERROR_DISMISS_SELECTORS = [
    # 中文（旧项目用过）
    'button:has-text("已知晓")',
    'button:has-text("确定")',
    'button:has-text("OK")',
    'button:has-text("知道了")',
    'button:has-text("关闭")',
    # 英文（地区合规弹窗，比如"The products and services... not intended for individuals in Hong Kong"）
    'button:has-text("I Understand")',
    'button:has-text("I understand")',
    'button:has-text("Confirm")',
    'button:has-text("Continue")',
    'button:has-text("Close")',
    # role-based 后备
    '[role="button"]:has-text("I Understand")',
    '[role="button"]:has-text("已知晓")',
]


@dataclass
class LoginSession:
    session_id: str
    username: str
    status: SessionStatus = "pending"
    qr_image_path: Path | None = None
    error: str | None = None
    started_at: float = field(default_factory=time.time)
    last_qr_refresh: float | None = None
    # 内部
    _task: asyncio.Task | None = None
    _captured_headers: dict[str, str] | None = None
    _login_started: bool = False
    _cancel_event: asyncio.Event = field(default_factory=asyncio.Event)
    _last_logged_cookie_len: int | None = None
    _yes_clicked: bool = False
    # 用户点"刷新二维码"会 set 这个 flag，capture_loop 检测到 → reload + 重截图
    _refresh_requested: bool = False


class LoginSessionManager:
    """所有扫码 session 的集中管理器。"""

    def __init__(
        self,
        storage: Storage,
        qr_dir: Path,
        playwright_state_dir: Path,
        headless: bool = True,
        session_timeout_s: int = 300,
        qr_refresh_interval_s: float = 3.0,
    ) -> None:
        self.storage = storage
        self.qr_dir = qr_dir
        self.playwright_state_dir = playwright_state_dir
        self.headless = headless
        self.session_timeout_s = session_timeout_s
        self.qr_refresh_interval_s = qr_refresh_interval_s
        self.sessions: dict[str, LoginSession] = {}
        self.qr_dir.mkdir(parents=True, exist_ok=True)
        self.playwright_state_dir.mkdir(parents=True, exist_ok=True)
        self._playwright: Playwright | None = None
        self._playwright_lock = asyncio.Lock()

    async def _ensure_playwright(self) -> Playwright:
        async with self._playwright_lock:
            if self._playwright is None:
                self._playwright = await async_playwright().start()
            return self._playwright

    async def shutdown(self) -> None:
        for sess in list(self.sessions.values()):
            sess._cancel_event.set()
            if sess._task and not sess._task.done():
                sess._task.cancel()
        if self._playwright is not None:
            await self._playwright.stop()
            self._playwright = None

    def start_login(self, username: str) -> LoginSession:
        # 关键：同一账号同时只允许一个登录会话。否则反复点扫码/刷新会开多个 chromium，
        # 全挤在同一个 user_data_dir 上互相抢锁 → 浏览器堆积 + 内存耗尽 + 二维码时好时坏。
        for old in list(self.sessions.values()):
            if old.username == username and old.status in ("pending", "qr_ready", "scanned"):
                old._cancel_event.set()
                if old._task and not old._task.done():
                    old._task.cancel()
                logger.info(
                    "[%s] 取消同账号旧会话 %s（避免浏览器堆积）", username, old.session_id[:8]
                )
        sess = LoginSession(session_id=str(uuid.uuid4()), username=username)
        self.sessions[sess.session_id] = sess
        sess._task = asyncio.create_task(self._run(sess), name=f"login-{username}-{sess.session_id[:8]}")
        return sess

    def get(self, session_id: str) -> LoginSession | None:
        return self.sessions.get(session_id)

    def cancel(self, session_id: str) -> bool:
        sess = self.sessions.get(session_id)
        if sess is None:
            return False
        sess._cancel_event.set()
        return True

    def request_refresh(self, session_id: str) -> bool:
        """用户点'刷新二维码' — capture_loop 下次 tick 会 reload + 重截图。"""
        sess = self.sessions.get(session_id)
        if sess is None:
            return False
        sess._refresh_requested = True
        logger.info("[%s] manual QR refresh requested", sess.username)
        return True

    # ------------------------------------------------------------------ 主逻辑
    async def _run(self, sess: LoginSession) -> None:
        try:
            await asyncio.wait_for(self._do_login(sess), timeout=self.session_timeout_s)
        except asyncio.TimeoutError:
            sess.status = "expired"
            sess.error = "session timed out"
            logger.warning("[%s] session expired", sess.username)
        except asyncio.CancelledError:
            sess.status = "failed"
            sess.error = "cancelled"
            raise
        except Exception as e:  # noqa: BLE001
            sess.status = "failed"
            sess.error = f"{type(e).__name__}: {e}"
            logger.exception("[%s] login task failed", sess.username)

    async def _do_login(self, sess: LoginSession) -> None:
        pw = await self._ensure_playwright()
        user_data_dir = self.playwright_state_dir / sess.username

        context: BrowserContext = await pw.chromium.launch_persistent_context(
            user_data_dir=str(user_data_dir),
            headless=self.headless,
            args=[
                "--no-sandbox",
                "--disable-setuid-sandbox",
                "--disable-blink-features=AutomationControlled",
                "--disable-dev-shm-usage",
            ],
            viewport={"width": 1280, "height": 800},
            user_agent=(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) "
                "AppleWebKit/537.36 (KHTML, like Gecko) "
                "Chrome/131.0.0.0 Safari/537.36"
            ),
            locale="zh-CN",
            timezone_id="Asia/Shanghai",
            extra_http_headers={
                "Accept-Language": "zh-CN,zh;q=0.9,en;q=0.8",
            },
        )

        try:
            page = await context.new_page()
            await page.add_init_script(INIT_SCRIPT)

            page.on("request", lambda req: asyncio.create_task(self._on_request(sess, req)))

            logger.info("[%s] navigating to %s", sess.username, LOGIN_URL)
            # 重试 3 次 + 渐进 timeout（30s → 30s → 60s），单次 hang 不致命
            last_exc: Exception | None = None
            for attempt in range(3):
                try:
                    timeout = 60_000 if attempt == 2 else 30_000
                    await page.goto(LOGIN_URL, wait_until="domcontentloaded", timeout=timeout)
                    last_exc = None
                    break
                except Exception as e:  # noqa: BLE001
                    last_exc = e
                    logger.warning(
                        "[%s] page.goto attempt %d/3 timed out (%s), retrying",
                        sess.username, attempt + 1, type(e).__name__,
                    )
                    await asyncio.sleep(2)
            if last_exc is not None:
                raise last_exc
            await asyncio.sleep(3)

            await self._dismiss_errors(page)
            # 不管 QR 找没找到，先截一张当前页面的图，前端可以拿来排查
            try:
                await self._take_qr_screenshot(page, sess)
            except Exception:  # noqa: BLE001
                pass

            await self._click_qr_button(page, sess)
            try:
                await self._wait_for_qr(page, sess)
            except Exception as e:  # noqa: BLE001
                # 即使 QR 元素未找到，把当前截图存上让人工排查（可能页面被风控了）
                try:
                    await self._take_qr_screenshot(page, sess)
                except Exception:  # noqa: BLE001
                    pass
                raise
            await self._take_qr_screenshot(page, sess)
            sess.status = "qr_ready"

            await self._capture_loop(page, context, sess)
        finally:
            try:
                await context.close()
            except Exception:  # noqa: BLE001
                pass

    # ------------------------------------------------------------------ helpers
    async def _dismiss_errors(self, page: Page) -> int:
        """关掉合规/错误弹窗。最多尝试 3 轮（弹窗可能延迟出现），返回点掉的次数。"""
        clicked = 0
        for _round in range(3):
            any_in_round = False
            for selector in ERROR_DISMISS_SELECTORS:
                try:
                    el = page.locator(selector).first
                    if await el.is_visible(timeout=300):
                        await el.click()
                        clicked += 1
                        any_in_round = True
                        logger.info("dismissed popup via %s", selector)
                        await asyncio.sleep(1)
                except Exception:  # noqa: BLE001
                    continue
            if not any_in_round:
                # 这一轮没找到弹窗了，再等一下看是不是延迟弹的
                await asyncio.sleep(1.5)
            else:
                # 刚关掉一个，可能还有下一个，再来一轮
                await asyncio.sleep(0.5)
        return clicked

    async def _click_qr_button(self, page: Page, sess: LoginSession) -> None:
        # 点二维码切换图标。关键：有的 selector（如 .qr-login-icon）能点到但不弹出二维码，
        # 所以点完必须验证真二维码 <canvas> 是否出现，没出现就继续试下一个 selector。
        for _ in range(5):
            for selector in QR_LOGIN_BUTTON_SELECTORS:
                try:
                    btn = page.locator(selector).first
                    if await btn.is_visible(timeout=500):
                        await btn.click()
                        await asyncio.sleep(1.5)
                        try:
                            if await page.locator("canvas").first.is_visible(timeout=1500):
                                logger.info(
                                    "[%s] clicked QR button (%s) → 二维码 canvas 已出现",
                                    sess.username, selector,
                                )
                                return
                        except Exception:  # noqa: BLE001
                            pass
                        logger.info(
                            "[%s] clicked %s 但没弹出二维码，试下一个", sess.username, selector
                        )
                except Exception:  # noqa: BLE001
                    continue
            await asyncio.sleep(1)
        # 所有写死的 selector 都不行 → JS 兜底：扫整个登录 form，找右上角的 svg/icon button
        # Binance 经常改 class/aria-label，但 form 内"登录"标题旁那个图标按钮的位置稳定。
        try:
            clicked = await page.evaluate(
                """() => {
                    // 找登录 form
                    const forms = Array.from(document.querySelectorAll('form'));
                    for (const form of forms) {
                        // form 内找 button/div 含 svg 的（QR 图标通常是 svg）
                        const candidates = form.querySelectorAll('button, div[role="button"], [role="button"]');
                        for (const el of candidates) {
                            if (el.querySelector('svg')) {
                                // 排除"继续"等大按钮（含文本超过 2 个字符）
                                const text = (el.textContent || '').trim();
                                if (text.length <= 2) {
                                    el.click();
                                    return el.outerHTML.slice(0, 200);
                                }
                            }
                        }
                    }
                    // form 外兜底：右上 1/4 区域 + 含 svg 的小元素
                    const all = Array.from(document.querySelectorAll('button, [role="button"]'));
                    const w = window.innerWidth, h = window.innerHeight;
                    for (const el of all) {
                        const r = el.getBoundingClientRect();
                        if (r.width === 0 || r.height === 0) continue;
                        if (r.width > 60 || r.height > 60) continue;  // 必须是小图标
                        if (r.right < w * 0.4 || r.right > w * 0.7) continue;  // 中间偏右
                        if (r.top > h * 0.7) continue;  // 上半部
                        if (el.querySelector('svg')) {
                            el.click();
                            return 'fallback:' + el.outerHTML.slice(0, 200);
                        }
                    }
                    return null;
                }"""
            )
            if clicked:
                await asyncio.sleep(1.5)
                logger.info("[%s] clicked QR via JS DOM scan: %s", sess.username, clicked)
                return
        except Exception as e:  # noqa: BLE001
            logger.warning("[%s] JS QR scan failed: %s", sess.username, e)

        # 还不行 → dump 整个登录表单 HTML 到日志便于排查
        try:
            html_snippet = await page.evaluate(
                """() => {
                    const f = document.querySelector('form');
                    if (f) return f.outerHTML.slice(0, 3000);
                    const main = document.querySelector('main') || document.body;
                    return main.outerHTML.slice(0, 3000);
                }"""
            )
            logger.warning(
                "[%s] QR button not found. Form HTML dump (first 3000 chars):\n%s",
                sess.username, html_snippet,
            )
        except Exception:  # noqa: BLE001
            pass
        logger.info("[%s] QR login button not found (assume default QR mode)", sess.username)

    async def _wait_for_qr(self, page: Page, sess: LoginSession) -> None:
        # 两轮 attempt：第一轮失败 → reload 页面 + 重新点 QR 按钮再试
        for attempt in range(2):
            for _ in range(20):
                for selector in QR_SELECTORS:
                    try:
                        el = page.locator(selector).first
                        if await el.is_visible(timeout=500):
                            await asyncio.sleep(1.5)  # 等渲染完
                            logger.info("[%s] QR ready (%s)", sess.username, selector)
                            return
                    except Exception:  # noqa: BLE001
                        continue
                await asyncio.sleep(1)
            if attempt == 0:
                # 第一轮没找到 → reload + 重试点 QR 按钮
                logger.warning(
                    "[%s] QR not found in 20s, reloading + retry",
                    sess.username,
                )
                try:
                    await page.reload(wait_until="domcontentloaded", timeout=20_000)
                    await asyncio.sleep(2)
                    await self._click_qr_button(page, sess)
                except Exception as e:  # noqa: BLE001
                    logger.warning("[%s] reload retry failed: %s", sess.username, e)
        raise RuntimeError("QR code element not found within 40s (after reload retry)")

    async def _take_qr_screenshot(self, page: Page, sess: LoginSession) -> None:
        path = self.qr_dir / f"{sess.session_id}.png"
        await page.screenshot(path=str(path))
        sess.qr_image_path = path
        sess.last_qr_refresh = time.time()

    async def _capture_loop(
        self, page: Page, context: BrowserContext, sess: LoginSession
    ) -> None:
        """边定时刷新 QR 图，边等待 cookies 捕获。"""
        while not sess._cancel_event.is_set():
            # 关键修复：扫码后币安会弹"保持登录5天"对话框，必须点"是"才会下发 r30t（30天刷新token）。
            # 不点的话只有短期 p20t/r20t（几小时），session 失效快。
            # 仿旧 web_qr_server.py L744-815 的 yes 按钮检测逻辑。
            if sess._login_started and not sess._yes_clicked:
                yes_selectors = [
                    'button[aria-label="是"]',
                    'button.bn-button:has-text("是")',
                    'button:has-text("是")',
                    'div[role="button"]:has-text("是")',
                    '.bn-button__primary:has-text("是")',
                ]
                for selector in yes_selectors:
                    try:
                        btn = page.locator(selector).first
                        if await btn.is_visible(timeout=300):
                            logger.info(
                                "[%s] '保持登录5天' dialog detected, clicking 是 (selector=%s)",
                                sess.username, selector,
                            )
                            await btn.click()
                            sess._yes_clicked = True
                            # 点完之后给币安 1-2 秒下发长期 cookies
                            await asyncio.sleep(1)
                            break
                    except Exception:  # noqa: BLE001
                        continue
            if sess._captured_headers is not None:
                # 修复 bug：之前在 first auth/bapi 请求时立刻 capture cookies，
                # 但币安的 r30t（30天 refresh token）+ currentAccount 等长期 cookie 是
                # 登录完成后几秒才 set 的。如果太早 capture，r30t 只有 1 字符（empty），
                # 导致 session 几小时就过期（短期 p20t/r20t 到期就死，没法续期）。
                #
                # 修法：先访问一个需要完整登录态的页面（/my/dashboard），等服务端 set
                # 所有长期 cookie，再额外 sleep 4s 兜底，最后一次性 capture。
                logger.info("[%s] auth headers detected, settling long-term cookies...", sess.username)
                try:
                    # 触发完整登录态：跳转到 dashboard（需要长期会话才能正常加载）
                    await page.goto(
                        "https://www.binance.com/zh-CN/my/dashboard",
                        wait_until="domcontentloaded",
                        timeout=20_000,
                    )
                    # 等几秒让 Set-Cookie 落地 + 任何 deferred token endpoint 完成
                    await asyncio.sleep(4)
                    # 再访问 alpha trading 页面，确保 alpha 域的 cookies 也到位
                    try:
                        await page.goto(
                            "https://www.binance.com/zh-CN/alpha/spot",
                            wait_until="domcontentloaded",
                            timeout=15_000,
                        )
                        await asyncio.sleep(2)
                    except Exception:  # noqa: BLE001
                        pass
                except Exception as e:  # noqa: BLE001
                    logger.warning("[%s] dashboard navigation failed: %s", sess.username, e)
                # 现在 capture 完整 cookies
                cookies_list = await context.cookies()
                cookies = {c["name"]: c["value"] for c in cookies_list}
                # 过滤 HTTP/2 伪 header（`:authority` 等）和 content-length 等不该透传的
                clean_headers = {
                    k: v
                    for k, v in sess._captured_headers.items()
                    if not k.startswith(":") and k.lower() not in ("content-length",)
                }
                # Sanity check：长期会话 cookies（cr00=session, p20t/r20t=auth, f30l=30天flag）
                # 旧 web_qr_server.py README 提的 r30t 在当前 Binance 已经不存在，改看这几个
                long_keys = ("cr00", "p20t", "r20t", "f30l", "d1og")
                got_long = [k for k in long_keys if len(cookies.get(k, "")) >= 16]
                if len(got_long) < 3:
                    logger.warning(
                        "[%s] long-term cookies incomplete: only got %s (expect ≥3 of %s). "
                        "Did you click '是' / 'Stay signed in 5 days'?",
                        sess.username, got_long, long_keys,
                    )
                else:
                    logger.info(
                        "[%s] long-term cookies ready: %s",
                        sess.username, got_long,
                    )
                await self.storage.upsert_account(
                    username=sess.username,
                    cookies=cookies,
                    headers=clean_headers,
                )
                sess.status = "success"
                logger.info(
                    "[%s] login success, captured %d cookies + %d headers (long=%s)",
                    sess.username,
                    len(cookies),
                    len(sess._captured_headers),
                    got_long,
                )
                return
            # 定期刷新 QR 截图
            # 用户手动点'刷新二维码' → reload 页面 + 重点 QR 按钮 + 重截图
            if sess._refresh_requested:
                sess._refresh_requested = False
                logger.info("[%s] handling manual QR refresh", sess.username)
                try:
                    await page.reload(wait_until="domcontentloaded", timeout=20_000)
                    await asyncio.sleep(2)
                    await self._click_qr_button(page, sess)
                    await asyncio.sleep(1)
                    await self._take_qr_screenshot(page, sess)
                    logger.info("[%s] manual QR refresh done", sess.username)
                except Exception as e:  # noqa: BLE001
                    logger.warning("[%s] manual QR refresh failed: %s", sess.username, e)
                await asyncio.sleep(0.5)
                continue
            if (
                sess.last_qr_refresh is None
                or time.time() - sess.last_qr_refresh >= self.qr_refresh_interval_s
            ):
                try:
                    await self._take_qr_screenshot(page, sess)
                except Exception:  # noqa: BLE001
                    pass
            await asyncio.sleep(0.5)

    async def _on_request(self, sess: LoginSession, req: Request) -> None:
        try:
            url = req.url
            if CALLBACK_PATH in url:
                if not sess._login_started:
                    sess._login_started = True
                    sess.status = "scanned"
                    logger.info("[%s] callback intercepted -> scanned", sess.username)
                return
            if not sess._login_started:
                return
            if AUTH_PATH in url:
                headers = await req.all_headers()
                if headers.get("cookie"):
                    sess._captured_headers = headers
                    logger.info("[%s] auth headers captured (%d fields)", sess.username, len(headers))
                return
            # 关键修复：每个 bapi 请求都覆盖 captured_headers（仿旧 web_qr_server.py 行为）。
            # 之前只捕第一次 → r30t（30天 refresh token）还没被服务端 set，cookie 缺失，
            # 导致 session 几小时就过期。每次覆盖确保拿到最新（最完整）的 cookies。
            if BAPI_PATTERN in url:
                headers = await req.all_headers()
                cookie = headers.get("cookie", "")
                if len(cookie) > 100:
                    sess._captured_headers = headers
                    # 只在第一次或 cookie 长度增长时打日志（避免太吵）
                    prev_len = sess._last_logged_cookie_len
                    if prev_len is None or len(cookie) > prev_len + 50:
                        logger.info(
                            "[%s] bapi headers captured/refreshed (cookie len=%d)",
                            sess.username,
                            len(cookie),
                        )
                        sess._last_logged_cookie_len = len(cookie)
        except Exception:  # noqa: BLE001
            logger.exception("[%s] _on_request error", sess.username)
