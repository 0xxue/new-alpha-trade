"""人脸/手机验证触发 + 截图模块。

旧项目 `web_trading_agent.py::_handle_phone_verify_test` (L1814-1921) 的 Rust 化端口：

完整流程：
1. 用户在前端点 "触发人脸验证"，传 (username, symbol, amount)
2. 这里用 username 对应的 user_data_dir 起一个 Playwright 持久化会话（带登录态）
3. 打开 https://www.binance.com/zh-CN/alpha → 等加载
4. 通过 evaluate() 调 alpha-trade /place 接口下一笔小额单（触发风控弹窗）
5. 等待 3s → 检测 "安全验证" 弹窗 (DOM)
6. 点击 "手机验证" 选项 (div.mfa-option-box:has-text("手机验证"))
7. 等待 3s → 弹出含二维码的 dialog
8. 截图 dialog → 存 data/face_qr/{username}.png
9. 前端 GET /face/{user}/qr 拉这个截图给用户扫

风险提示：
- DOM 选择器照搬旧代码（一年前的版本），币安改版后可能失效，需现场调
- 下单 = 真实交易；金额小（默认 10 USDT）+ 弹窗出现即停止后续步骤，正常情况下不会真扣
- 如果 5s 内没检测到弹窗，认为下单成功（账号目前不需要风控），fallback 截一张当前页给排查
"""
from __future__ import annotations

import asyncio
import logging
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Literal

from playwright.async_api import BrowserContext, Page, async_playwright, Playwright

logger = logging.getLogger("qr_service.face_verify")

ALPHA_PAGE_URL = "https://www.binance.com/zh-CN/alpha"

# 旧 web_trading_agent.py 的选择器（一年前抓的，可能要调）
SECURITY_DIALOG_SELECTORS = [
    'text=安全验证',
    'text=Security Verification',
    'div:has-text("请选择完成验证的账号")',
]

PHONE_VERIFY_BUTTON_SELECTORS = [
    'div.mfa-option-box:has-text("手机验证")',
    'div.mfa-option-box:has-text("Phone Verification")',
    'div:has-text("手机验证"):has(svg)',
]

DIALOG_SELECTORS = [
    'div[role="dialog"]',
    'div.modal',
    'div.popup',
]

# 反检测 init script（沿用 playwright_login 的）
INIT_SCRIPT = """
Object.defineProperty(navigator, 'webdriver', { get: () => undefined });
Object.defineProperty(navigator, 'languages', { get: () => ['zh-CN', 'zh', 'en-US', 'en'] });
Object.defineProperty(navigator, 'platform', { get: () => 'Win32' });
delete window.playwright;
delete window.__playwright;
"""

FaceStatus = Literal[
    "idle",
    "running",
    "no_dialog",       # 下单后没弹安全验证弹窗，账号目前不需要风控
    "dialog_no_phone", # 弹窗里没找到 "手机验证" 按钮
    "captured",        # 截图成功
    "failed",
]


@dataclass
class FaceVerifySession:
    username: str
    status: FaceStatus = "idle"
    message: str = ""
    screenshot_path: Path | None = None
    started_at: float = field(default_factory=time.time)
    finished_at: float | None = None


class FaceVerifyManager:
    """所有账户的人脸验证截图会话管理。"""

    def __init__(
        self,
        playwright_state_dir: Path,
        face_qr_dir: Path,
        headless: bool = True,
        per_account_timeout_s: int = 90,
    ) -> None:
        self.playwright_state_dir = playwright_state_dir
        self.face_qr_dir = face_qr_dir
        self.headless = headless
        self.timeout_s = per_account_timeout_s
        self.face_qr_dir.mkdir(parents=True, exist_ok=True)
        self.sessions: dict[str, FaceVerifySession] = {}
        self._playwright: Playwright | None = None
        self._pw_lock = asyncio.Lock()
        self._user_locks: dict[str, asyncio.Lock] = {}

    async def _ensure_playwright(self) -> Playwright:
        async with self._pw_lock:
            if self._playwright is None:
                self._playwright = await async_playwright().start()
            return self._playwright

    async def shutdown(self) -> None:
        if self._playwright is not None:
            await self._playwright.stop()
            self._playwright = None

    def get_session(self, username: str) -> FaceVerifySession | None:
        return self.sessions.get(username)

    def screenshot_path_for(self, username: str) -> Path:
        return self.face_qr_dir / f"{username}.png"

    async def trigger(self, username: str, symbol: str, amount_usdt: float) -> FaceVerifySession:
        """触发一次人脸验证截图。同账户串行执行（避免 user_data_dir 冲突）。"""
        lock = self._user_locks.setdefault(username, asyncio.Lock())
        async with lock:
            sess = FaceVerifySession(username=username, status="running",
                                      message=f"triggering for {symbol} amount={amount_usdt}")
            self.sessions[username] = sess
            try:
                await asyncio.wait_for(
                    self._do_trigger(sess, symbol, amount_usdt), timeout=self.timeout_s
                )
            except asyncio.TimeoutError:
                sess.status = "failed"
                sess.message = f"timeout after {self.timeout_s}s"
                logger.warning("[%s] face_verify timeout", username)
            except Exception as e:  # noqa: BLE001
                sess.status = "failed"
                sess.message = f"{type(e).__name__}: {e}"
                logger.exception("[%s] face_verify failed", username)
            sess.finished_at = time.time()
            return sess

    async def _do_trigger(
        self, sess: FaceVerifySession, symbol: str, amount_usdt: float
    ) -> None:
        pw = await self._ensure_playwright()
        user_data_dir = self.playwright_state_dir / sess.username

        if not user_data_dir.exists():
            sess.status = "failed"
            sess.message = (
                f"user_data_dir {user_data_dir} not found — please complete QR login first"
            )
            return

        context: BrowserContext = await pw.chromium.launch_persistent_context(
            user_data_dir=str(user_data_dir),
            headless=self.headless,
            args=[
                "--no-sandbox",
                "--disable-blink-features=AutomationControlled",
                "--disable-dev-shm-usage",
            ],
            viewport={"width": 1280, "height": 800},
            locale="zh-CN",
            timezone_id="Asia/Shanghai",
        )
        try:
            page = await context.new_page()
            await page.add_init_script(INIT_SCRIPT)

            logger.info("[%s] opening %s", sess.username, ALPHA_PAGE_URL)
            await page.goto(ALPHA_PAGE_URL, wait_until="domcontentloaded", timeout=60_000)
            await asyncio.sleep(4)

            # 让浏览器自己跑 fetch 触发一笔 OTO 下单（最简方式 — 不依赖 alpha 页面的 UI 按钮）
            #
            # 关键：用浏览器 fetch 调币安 API，cookies 自动带；如果触发风控，
            # 币安会返回 risk_challenge_biz_no，并且 alpha 页面会自动弹出"安全验证"模态框
            # （因为页面里有全局监听器对 risk_challenge 做 UI 处理）
            #
            # 注意：金额很小（默认 10 USDT），触发风控弹窗即可，无需真的成单。
            buy_qty_quote = amount_usdt
            try:
                await page.evaluate(
                    """
                    async ({symbol, quoteAmt}) => {
                        try {
                            const resp = await fetch(
                              '/bapi/asset/v1/private/alpha-trade/oto-order/place',
                              {
                                method: 'POST',
                                credentials: 'include',
                                headers: { 'content-type': 'application/json' },
                                body: JSON.stringify({
                                    baseAsset: symbol,
                                    quoteAsset: 'USDT',
                                    workingSide: 'BUY',
                                    workingPrice: 0.000001,    // 极低价 — 不会成交
                                    workingQuantity: 1,
                                    paymentDetails: [
                                      { amount: String(quoteAmt), paymentWalletType: 'CARD' }
                                    ],
                                    pendingPrice: 0.000002,
                                    pendingType: 'LIMIT'
                                })
                              }
                            );
                            return { status: resp.status, ok: resp.ok };
                        } catch (e) {
                            return { error: String(e) };
                        }
                    }
                    """,
                    {"symbol": symbol, "quoteAmt": buy_qty_quote},
                )
                logger.info("[%s] trigger fetch sent", sess.username)
            except Exception as e:  # noqa: BLE001
                logger.warning("[%s] trigger fetch failed: %s", sess.username, e)

            await asyncio.sleep(3)

            # 检测"安全验证"弹窗
            dialog_found = False
            hit_selector = None
            for selector in SECURITY_DIALOG_SELECTORS:
                try:
                    elem = page.locator(selector).first
                    if await elem.is_visible(timeout=500):
                        dialog_found = True
                        hit_selector = selector
                        logger.info("[%s] security dialog detected: %s", sess.username, selector)
                        break
                except Exception:  # noqa: BLE001
                    continue

            if not dialog_found:
                # 没触发 → 截当前页面作为排查
                fallback_path = self.screenshot_path_for(sess.username)
                try:
                    await page.screenshot(path=str(fallback_path), full_page=False)
                    sess.screenshot_path = fallback_path
                except Exception:  # noqa: BLE001
                    pass
                sess.status = "no_dialog"
                sess.message = "no security verification dialog appeared — account may not require it now"
                return

            # 点击"手机验证"选项
            phone_clicked = False
            for selector in PHONE_VERIFY_BUTTON_SELECTORS:
                try:
                    btn = page.locator(selector).first
                    if await btn.is_visible(timeout=500):
                        await btn.click()
                        await asyncio.sleep(3)
                        phone_clicked = True
                        logger.info("[%s] clicked phone verify: %s", sess.username, selector)
                        break
                except Exception:  # noqa: BLE001
                    continue

            if not phone_clicked:
                # 弹窗在但没找到手机验证按钮 — 整体截一张
                fallback_path = self.screenshot_path_for(sess.username)
                try:
                    await page.screenshot(path=str(fallback_path), full_page=True)
                    sess.screenshot_path = fallback_path
                except Exception:  # noqa: BLE001
                    pass
                sess.status = "dialog_no_phone"
                sess.message = (
                    f"security dialog seen ({hit_selector}) but '手机验证' option not found"
                )
                return

            # 截图 dialog（含二维码）
            out_path = self.screenshot_path_for(sess.username)
            captured = False
            for selector in DIALOG_SELECTORS:
                try:
                    dlg = page.locator(selector).first
                    if await dlg.is_visible(timeout=2000):
                        await dlg.screenshot(path=str(out_path))
                        captured = True
                        logger.info("[%s] dialog screenshot saved: %s", sess.username, out_path)
                        break
                except Exception:  # noqa: BLE001
                    continue

            if not captured:
                # 选择器都没匹配 → full page 兜底
                await page.screenshot(path=str(out_path), full_page=True)
                logger.info("[%s] full-page fallback screenshot saved: %s", sess.username, out_path)

            sess.screenshot_path = out_path
            sess.status = "captured"
            sess.message = "QR captured — scan with Binance app to complete"
        finally:
            try:
                await context.close()
            except Exception:  # noqa: BLE001
                pass
