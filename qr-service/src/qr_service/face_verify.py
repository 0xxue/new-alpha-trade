"""人脸/手机验证触发 + 截图模块。

旧项目 `web_trading_agent.py::_place_web_order` + `_handle_phone_verify_test`
(L1814-1921 / L2166+) 的异步 Playwright 端口。

== 为什么要真·走网页 UI 下单(而不是 fetch) ==
币安「安全验证」弹窗是**币安自己的网页 App(React)在它自己的下单代码收到风控响应
(risk_challenge_biz_no)时才渲染**的。脚本直接 `fetch()` 下单会绕过 App,响应里虽然带
risk_challenge_biz_no,但页面没人渲染弹窗 → 永远 no_dialog。
所以必须**通过 DOM 真点网页买入按钮**,逼币安自己弹窗。这是老项目能用的关键。

完整流程：
1. 前端点「触发验证」，传 (username, symbol, amount)
2. 用 username 的 user_data_dir 起持久化会话(带登录态)
3. 向引擎 /tokens 解析 symbol → chain_id + contract_address
4. 导航到 https://www.binance.com/zh-CN/alpha/{slug}/{contract}(具体代币交易页)
5. 选「买入」tab → 填成交额 → 点「买入」→ 处理「继续」滑点弹窗(= 真下一笔小额单)
6. 等 3s → 检测「安全验证」弹窗 → 点「手机验证」→ 截图二维码 → data/face_qr/{username}.png
7. 前端 GET /face/{user}/qr 拉截图给用户用币安 App 扫

风险提示：
- 会真下一笔小额买单(默认 10 USDT)来触发风控,这是出弹窗的前提(不能再用假价)
- DOM 选择器搬自老项目,币安改版后可能失效,需现场对当前页面调
- 选择器/链 slug 调试:看 nat-qr-service journal 日志(本模块 logger=qr_service.face_verify)
"""
from __future__ import annotations

import asyncio
import json
import logging
import time
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path
from typing import Literal

from playwright.async_api import BrowserContext, Page, async_playwright, Playwright

logger = logging.getLogger("qr_service.face_verify")

ALPHA_PAGE_URL = "https://www.binance.com/zh-CN/alpha"
ALPHA_TOKEN_URL = "https://www.binance.com/zh-CN/alpha/{slug}/{contract}"

# 同机引擎,/tokens 带 symbol/alpha_id/chain_id/contract_address
ENGINE_TOKENS_URL = "http://127.0.0.1:7002/tokens"

# 引擎 chain_id(币安 chainId 编码) → 币安 Alpha 代币页 URL 用的链名 slug
# 数据来自引擎 /tokens 实测分布:56=BSC(多数) / 1=ETH / 8453=Base / CT_501=Solana ...
CHAIN_ID_TO_SLUG = {
    "56": "bsc",
    "1": "eth",
    "8453": "base",
    "146": "sonic",
    "42161": "arbitrum",
    "59144": "linea",
    "CT_501": "sol",
    "CT_784": "sui",
    "CT_195": "tron",
}

# ---- 老项目 _place_web_order 的下单表单选择器(照搬) ----
BUY_TAB_SELECTORS = [
    'div[role="tab"].bn-tab__buySell:has-text("买入")',
    'div[role="tab"].bn-tab__buySell:has-text("Buy")',
    'div[role="tab"]#bn-tab-0',
]
PRICE_INPUT = "input#limitPrice"
AMOUNT_INPUT = "input#limitAmount"   # 数量(base token)
TOTAL_INPUT = "input#limitTotal"     # 成交额(USDT) —— 第一个
BUY_BUTTON_SELECTORS = [
    "button.bn-button__buy",
    "button.bn-button.bn-button__buy",
    'button:has-text("买入")',
    'button:has-text("Buy")',
    "button.trd-orderForm-confirm",
]
CONTINUE_SELECTORS = [
    'button:has-text("继续")',
    'button:has-text("Continue")',
]
COOKIE_SELECTORS = [
    "button#onetrust-accept-btn-handler",
    'button:has-text("接受所有 Cookie")',
    'button:has-text("Accept all cookies")',
    'button:has-text("Accept All Cookies")',
]

# 旧 web_trading_agent.py 的「安全验证」弹窗选择器
SECURITY_DIALOG_SELECTORS = [
    "text=安全验证",
    "text=Security Verification",
    'div:has-text("请选择完成验证的账号")',
]

PHONE_VERIFY_BUTTON_SELECTORS = [
    'div.mfa-option-box:has-text("手机验证")',
    'div.mfa-option-box:has-text("Phone Verification")',
    'div:has-text("手机验证"):has(svg)',
]

DIALOG_SELECTORS = [
    "div#mfa-shadow-host",   # 币安 MFA/安全验证 弹窗容器(老项目 _place_web_order L2183)
    "div.bn-modal-wrap",
    "div.bn-modal",
    'div[role="dialog"]',
    "div.modal",
    "div.popup",
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

    async def _resolve_token(self, symbol: str) -> dict | None:
        """向同机引擎 /tokens 解析 symbol(或 alpha_id) → {chain_id, contract_address}。"""
        def _fetch() -> dict:
            with urllib.request.urlopen(ENGINE_TOKENS_URL, timeout=6) as r:
                return json.loads(r.read().decode("utf-8"))

        try:
            data = await asyncio.to_thread(_fetch)
        except Exception as e:  # noqa: BLE001
            logger.warning("resolve token: engine /tokens failed: %s", e)
            return None
        toks = data.get("tokens", []) if isinstance(data, dict) else []
        q = symbol.strip().upper()
        for t in toks:
            if str(t.get("alpha_id", "")).upper() == q or str(t.get("symbol", "")).upper() == q:
                return t
        logger.warning("resolve token: %s not found in %d tokens", symbol, len(toks))
        return None

    def _token_page_url(self, token: dict | None) -> str:
        """token → 代币交易页 URL；解析不到链 slug 时回退通用 /alpha。"""
        if not token:
            return ALPHA_PAGE_URL
        slug = CHAIN_ID_TO_SLUG.get(str(token.get("chain_id")))
        contract = token.get("contract_address")
        if slug and contract:
            return ALPHA_TOKEN_URL.format(slug=slug, contract=contract)
        logger.warning(
            "no slug for chain_id=%s (contract=%s) → fallback /alpha",
            token.get("chain_id"), contract,
        )
        return ALPHA_PAGE_URL

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

        token = await self._resolve_token(symbol)
        target_url = self._token_page_url(token)
        logger.info("[%s] symbol=%s → %s", sess.username, symbol, target_url)

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

            logger.info("[%s] opening %s", sess.username, target_url)
            await page.goto(target_url, wait_until="domcontentloaded", timeout=60_000)
            await asyncio.sleep(3)
            await self._dismiss_cookie(page)

            # 等下单表单加载(老项目同款等待条件)
            try:
                await page.wait_for_selector(
                    "input#limitPrice, input#limitAmount", timeout=15_000
                )
                logger.info("[%s] order form loaded", sess.username)
            except Exception:  # noqa: BLE001
                logger.warning(
                    "[%s] order form (#limitPrice/#limitAmount) not found — 页面结构可能变了",
                    sess.username,
                )

            # === 关键:真·走网页 UI 下单,逼币安渲染「安全验证」弹窗 ===
            placed = await self._drive_web_buy(page, amount_usdt)
            logger.info("[%s] web buy submitted=%s", sess.username, placed)
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
                sess.message = (
                    "下单已提交但未弹安全验证弹窗 — 账号当前可能不需要验证,"
                    "或下单未成功(看 journal 日志)"
                )
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

    async def _dismiss_cookie(self, page: Page) -> None:
        """关 Cookie 同意弹窗(老项目 _navigate_to_token_page 同款)。"""
        for selector in COOKIE_SELECTORS:
            try:
                btn = page.locator(selector).first
                if await btn.is_visible(timeout=800):
                    await btn.click()
                    logger.info("dismissed cookie banner: %s", selector)
                    await asyncio.sleep(1)
                    return
            except Exception:  # noqa: BLE001
                continue

    async def _drive_web_buy(self, page: Page, amount_usdt: float) -> bool:
        """照搬老项目 _place_web_order 的核心:选买入 tab → 填成交额 → 点买入 → 处理「继续」。

        目的不是成交,而是**通过币安自己的网页下单代码提交订单**,触发风控弹窗。
        """
        # 1. 切到「买入」tab
        for sel in BUY_TAB_SELECTORS:
            try:
                tab = page.locator(sel).first
                if await tab.is_visible(timeout=800):
                    await tab.click()
                    await asyncio.sleep(0.4)
                    logger.info("buy tab: %s", sel)
                    break
            except Exception:  # noqa: BLE001
                continue

        # 2. 填「成交额」(USDT) —— 留价格预填(市价),最直接表达"买 $X"
        filled = False
        try:
            total = page.locator(TOTAL_INPUT).first
            if await total.is_visible(timeout=1500):
                await total.click()
                await total.fill("")
                await asyncio.sleep(0.1)
                await total.type(str(amount_usdt), delay=30)
                filled = True
                logger.info("filled 成交额=%s USDT", amount_usdt)
        except Exception as e:  # noqa: BLE001
            logger.warning("fill 成交额 failed: %s", e)

        # 2b. 兜底:成交额框没有 → 读市价算数量填「数量」框
        if not filled:
            try:
                price_v = await page.locator(PRICE_INPUT).first.input_value()
                price = float(price_v) if price_v else 0.0
                if price > 0:
                    qty = max(1, round(amount_usdt / price))
                    amt = page.locator(AMOUNT_INPUT).first
                    await amt.click()
                    await amt.fill("")
                    await asyncio.sleep(0.1)
                    await amt.type(str(qty), delay=30)
                    filled = True
                    logger.info("filled 数量=%s (price=%s)", qty, price)
            except Exception as e:  # noqa: BLE001
                logger.warning("fill 数量 fallback failed: %s", e)

        await asyncio.sleep(0.5)

        # 3. 点「买入」按钮
        clicked = False
        for sel in BUY_BUTTON_SELECTORS:
            try:
                btn = page.locator(sel).first
                if await btn.is_visible(timeout=800):
                    if (await btn.get_attribute("aria-disabled")) == "true":
                        logger.info("buy button disabled: %s", sel)
                        continue
                    await btn.click()
                    clicked = True
                    logger.info("clicked buy: %s", sel)
                    break
            except Exception:  # noqa: BLE001
                continue
        await asyncio.sleep(1.0)

        # 4. 处理「继续」滑点警告(老项目 L2818) —— 点了订单才真正提交 → 触发风控
        for sel in CONTINUE_SELECTORS:
            try:
                btn = page.locator(sel).first
                if await btn.is_visible(timeout=800):
                    await btn.click()
                    logger.info("clicked 继续 (slippage warn): %s", sel)
                    await asyncio.sleep(1.0)
                    break
            except Exception:  # noqa: BLE001
                continue

        return clicked
