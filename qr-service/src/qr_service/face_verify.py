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

from playwright.async_api import BrowserContext, Page, Request, async_playwright, Playwright

from qr_service.storage import Storage

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

# 二维码 60s 过期后,弹窗显示「Verification failed」+ 黄色主按钮「重新验证」(不是「刷新二维码」)。
# 点它重新生成一张新码。实测 DOM(2026-07,在 #mfa-shadow-host 的 shadow-root 内)。
REFRESH_QR_BUTTON_SELECTORS = [
    'button.bn-button__primary:has-text("重新验证")',
    'button:has-text("重新验证")',
    "text=重新验证",
    'button:has-text("Verify Again")',
    'button:has-text("Try Again")',
    'button:has-text("Retry")',
    # 老版本兜底
    "text=刷新二维码",
]

# 币安 alpha 下单私有接口(下单会触发会话令牌轮换 → 抓这些请求的最新 headers 回写 DB)
BAPI_ORDER_PATHS = (
    "/bapi/asset/v1/private/alpha-trade/order/place",
    "/bapi/asset/v1/private/alpha-trade/oto-order/place",
)
# 长期会话 cookie(与 playwright_login 同款 sanity 校验,防止把残缺 cookie 写脏 DB)
LONG_TERM_COOKIE_KEYS = ("cr00", "p20t", "r20t", "f30l", "d1og")

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
    "waiting_scan",    # 截图成功，浏览器保活中，等待 App 扫码完成
    "verified",        # getSteps 显示 DONE
    "expired",         # 等待扫码超时
    "failed",
]


@dataclass
class FaceVerifySession:
    username: str
    status: FaceStatus = "idle"
    message: str = ""
    screenshot_path: Path | None = None
    biz_no: str | None = None
    started_at: float = field(default_factory=time.time)
    finished_at: float | None = None
    verified_at: float | None = None
    expires_at: float | None = None
    last_qr_refresh: float | None = None


class FaceVerifyManager:
    """所有账户的人脸验证截图会话管理。"""

    def __init__(
        self,
        playwright_state_dir: Path,
        face_qr_dir: Path,
        headless: bool = True,
        per_account_timeout_s: int = 90,
        post_capture_keepalive_s: int = 180,
        qr_refresh_interval_s: int = 62,
        storage: Storage | None = None,
    ) -> None:
        self.playwright_state_dir = playwright_state_dir
        self.face_qr_dir = face_qr_dir
        self.headless = headless
        self.timeout_s = per_account_timeout_s
        self.post_capture_keepalive_s = post_capture_keepalive_s
        # 二维码 60s 过期,>60s 点「刷新二维码」按钮 + 重截图(老 daemon 62s 同款)
        self.qr_refresh_interval_s = qr_refresh_interval_s
        self.storage = storage
        self.face_qr_dir.mkdir(parents=True, exist_ok=True)
        self.sessions: dict[str, FaceVerifySession] = {}
        # 每账号最近一次 bapi 下单请求的 headers(含 csrftoken 等),用于把轮换后的会话回写 DB
        self._captured_headers: dict[str, dict[str, str]] = {}
        self._playwright: Playwright | None = None
        self._pw_lock = asyncio.Lock()
        self._user_locks: dict[str, asyncio.Lock] = {}
        self._held_contexts: dict[str, BrowserContext] = {}
        self._hold_tasks: dict[str, asyncio.Task[None]] = {}

    async def _ensure_playwright(self) -> Playwright:
        async with self._pw_lock:
            if self._playwright is None:
                self._playwright = await async_playwright().start()
            return self._playwright

    async def shutdown(self) -> None:
        for task in list(self._hold_tasks.values()):
            task.cancel()
        if self._hold_tasks:
            await asyncio.gather(*self._hold_tasks.values(), return_exceptions=True)
        self._hold_tasks.clear()
        for ctx in list(self._held_contexts.values()):
            try:
                await ctx.close()
            except Exception:  # noqa: BLE001
                pass
        self._held_contexts.clear()
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
            await self._close_held_context(username)
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
            if sess.status != "waiting_scan":
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
        keep_context = False

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
            self._install_biz_no_listener(page, sess)
            self._install_header_listener(page, sess.username)

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
            captured = await self._screenshot_dialog(page, out_path)
            if captured:
                logger.info("[%s] dialog screenshot saved: %s", sess.username, out_path)
            else:
                logger.info("[%s] full-page fallback screenshot saved: %s", sess.username, out_path)

            sess.screenshot_path = out_path
            sess.last_qr_refresh = time.time()
            # 下单已触发会话令牌轮换 → 立刻把浏览器最新 cookies 回写 DB,别让引擎的 DB 会话被孤立
            await self._sync_cookies_to_db(context, sess.username)
            if self.post_capture_keepalive_s > 0:
                keep_context = True
                sess.status = "waiting_scan"
                sess.expires_at = time.time() + self.post_capture_keepalive_s
                suffix = f" (bizNo={sess.biz_no})" if sess.biz_no else ""
                sess.message = (
                    "QR captured — scan with Binance app; waiting for Binance challenge DONE"
                    f"{suffix}"
                )
                self._hold_context_after_capture(sess.username, context, page, sess)
            else:
                sess.status = "captured"
                suffix = f" (bizNo={sess.biz_no})" if sess.biz_no else ""
                sess.message = f"QR captured — scan with Binance app to complete{suffix}"
        finally:
            if not keep_context:
                try:
                    await context.close()
                except Exception:  # noqa: BLE001
                    pass

    def _install_biz_no_listener(self, page: Page, sess: FaceVerifySession) -> None:
        """Capture the challenge bizNo emitted by Binance's own web order request."""

        def handle_response(response) -> None:  # noqa: ANN001
            try:
                url = response.url
                if (
                    "/bapi/asset/v1/private/alpha-trade/order/place" not in url
                    and "/bapi/asset/v1/private/alpha-trade/oto-order/place" not in url
                ):
                    return
                headers = response.headers
                biz_no = headers.get("risk_challenge_biz_no") or headers.get(
                    "risk-challenge-biz-no"
                )
                if biz_no:
                    sess.biz_no = biz_no
                    logger.info("[%s] captured risk_challenge_biz_no=%s", sess.username, biz_no)
                else:
                    logger.info("[%s] web order response has no risk_challenge_biz_no", sess.username)
            except Exception as e:  # noqa: BLE001
                logger.debug("[%s] capture bizNo failed: %s", sess.username, e)

        page.on("response", handle_response)

    def _install_header_listener(self, page: Page, username: str) -> None:
        """抓 bapi 下单请求的最新 headers(含 cookie/csrftoken 等),供回写 DB 用。
        每次覆盖 → 拿到令牌轮换后最新最完整的一份(仿 playwright_login._on_request)。"""

        async def handle_request(req: Request) -> None:
            try:
                if not any(p in req.url for p in BAPI_ORDER_PATHS):
                    return
                headers = await req.all_headers()
                if len(headers.get("cookie", "")) > 100:
                    self._captured_headers[username] = headers
            except Exception as e:  # noqa: BLE001
                logger.debug("[%s] capture headers failed: %s", username, e)

        page.on("request", handle_request)

    async def _screenshot_dialog(self, page: Page, out_path: Path) -> bool:
        """截安全验证弹窗(含二维码);选择器都不中 → full-page 兜底。返回是否命中弹窗。"""
        for selector in DIALOG_SELECTORS:
            try:
                dlg = page.locator(selector).first
                if await dlg.is_visible(timeout=2000):
                    await dlg.screenshot(path=str(out_path))
                    return True
            except Exception:  # noqa: BLE001
                continue
        try:
            await page.screenshot(path=str(out_path), full_page=True)
        except Exception:  # noqa: BLE001
            pass
        return False

    async def _security_dialog_present(self, page: Page) -> bool:
        """「安全验证」弹窗是否还在。消失 = 挑战完成、单已放行 → 可停止刷新。"""
        for selector in SECURITY_DIALOG_SELECTORS:
            try:
                el = page.locator(selector).first
                if await el.is_visible(timeout=800):
                    return True
            except Exception:  # noqa: BLE001
                continue
        return False

    async def _refresh_qr(self, page: Page, sess: FaceVerifySession, out_path: Path) -> bool:
        """二维码过期后弹窗出现「重新验证」按钮 → 点它重新生成新码 + 重截图。
        码还有效时按钮不存在 → 直接返回 False(不动)。每轮 keepalive 调用一次(3s)。"""
        btn = None
        for selector in REFRESH_QR_BUTTON_SELECTORS:
            try:
                el = page.locator(selector).first
                if await el.count() and await el.is_visible(timeout=800):
                    btn = el
                    break
            except Exception:  # noqa: BLE001
                continue
        if btn is None:
            return False  # 码还有效 / 弹窗已关,无需刷新
        try:
            await btn.click(timeout=1500)
        except Exception:  # noqa: BLE001
            return False
        await page.wait_for_timeout(1500)
        # 重新验证后可能回到「安全验证」方式选择页 → 补点一次「手机验证」把新码调出来
        for selector in PHONE_VERIFY_BUTTON_SELECTORS:
            try:
                el = page.locator(selector).first
                if await el.count() and await el.is_visible(timeout=800):
                    await el.click(timeout=1500)
                    await page.wait_for_timeout(1000)
                    break
            except Exception:  # noqa: BLE001
                continue
        await page.wait_for_timeout(2000)  # 等新码渲染
        await self._screenshot_dialog(page, out_path)
        sess.last_qr_refresh = time.time()
        logger.info("[%s] QR re-verified(点重新验证)+ re-screenshot", sess.username)
        return True

    async def _dump_refresh_debug(self, page: Page, username: str) -> None:
        """刷新按钮没找到时,dump 弹窗真实 DOM(含 shadow root)+ 存调试全屏图,便于定位真实按钮。"""
        try:
            dbg = self.screenshot_path_for(username).parent / f"{username}_dbg.png"
            await page.screenshot(path=str(dbg), full_page=True)
        except Exception:  # noqa: BLE001
            pass
        try:
            info = await page.evaluate(
                r"""
                () => {
                  const scan = (root, label) => {
                    const hits = [];
                    root.querySelectorAll('div,button,span,a,[role="button"],svg').forEach(e => {
                      const t = (e.textContent || '').trim();
                      if (t && t.length <= 24 && /刷新|refresh|失效|expired|重新|过期|Refresh|重试/i.test(t)) {
                        const cls = (e.className && e.className.baseVal !== undefined) ? e.className.baseVal : (e.className || '');
                        hits.push({tag: e.tagName, cls: String(cls).slice(0, 60), text: t});
                      }
                    });
                    return {label, text: (root.textContent || '').replace(/\s+/g, ' ').slice(0, 300), hits};
                  };
                  const out = {url: location.href, roots: []};
                  const sh = document.querySelector('#mfa-shadow-host');
                  if (sh && sh.shadowRoot) out.roots.push(scan(sh.shadowRoot, 'mfa-shadow-root'));
                  document.querySelectorAll('div[role="dialog"],div.bn-modal,div.modal').forEach((d, i) => out.roots.push(scan(d, 'dialog' + i)));
                  return out;
                }
                """
            )
            logger.info("[%s] refresh-debug: %s", username, json.dumps(info, ensure_ascii=False)[:1400])
        except Exception as e:  # noqa: BLE001
            logger.info("[%s] refresh-debug dump failed: %s", username, e)

    async def _sync_cookies_to_db(self, context: BrowserContext, username: str) -> bool:
        """把浏览器 context 当前 cookies + 最近 bapi headers 回写 DB。
        原因:人脸助手在浏览器下单会触发币安令牌轮换,引擎读的是 DB 静态 cookies —— 不回写
        引擎会话就被孤立(`100002001 登录状态已失效`)。回写后引擎下一轮自动续,无需重登。"""
        if self.storage is None:
            return False
        try:
            cookies_list = await context.cookies()
        except Exception as e:  # noqa: BLE001
            logger.warning("[%s] sync cookies: read context.cookies failed: %s", username, e)
            return False
        cookies = {c["name"]: c["value"] for c in cookies_list}
        # sanity:必须有足够的长期会话 cookie,否则别拿残缺的去覆盖引擎正在用的那份
        got_long = [k for k in LONG_TERM_COOKIE_KEYS if len(cookies.get(k, "")) >= 16]
        if len(got_long) < 3:
            logger.warning(
                "[%s] sync cookies skipped: long-term cookies incomplete (got %s)", username, got_long
            )
            return False
        headers = self._captured_headers.get(username) or {}
        clean_headers = {
            k: v
            for k, v in headers.items()
            if not k.startswith(":") and k.lower() not in ("content-length",)
        }
        try:
            await self.storage.upsert_account(
                username=username, cookies=cookies, headers=clean_headers
            )
        except Exception as e:  # noqa: BLE001
            logger.warning("[%s] sync cookies: upsert_account failed: %s", username, e)
            return False
        logger.info(
            "[%s] synced rotated session → DB (%d cookies, %d headers, long=%s)",
            username, len(cookies), len(clean_headers), got_long,
        )
        return True

    async def _close_held_context(self, username: str) -> None:
        task = self._hold_tasks.pop(username, None)
        if task is not None and not task.done():
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)
            return
        ctx = self._held_contexts.pop(username, None)
        if ctx is not None:
            try:
                await ctx.close()
            except Exception:  # noqa: BLE001
                pass

    def _hold_context_after_capture(
        self,
        username: str,
        context: BrowserContext,
        page: Page,
        sess: FaceVerifySession,
    ) -> None:
        self._held_contexts[username] = context
        old_task = self._hold_tasks.pop(username, None)
        if old_task is not None and not old_task.done():
            old_task.cancel()
        self._hold_tasks[username] = asyncio.create_task(
            self._keep_alive_until_verified(username, context, page, sess)
        )

    async def _keep_alive_until_verified(
        self,
        username: str,
        context: BrowserContext,
        page: Page,
        sess: FaceVerifySession,
    ) -> None:
        deadline = time.time() + self.post_capture_keepalive_s
        out_path = self.screenshot_path_for(username)
        last_refresh = time.time()  # 距上次出码时间;满 55s(码~60s过期)才允许点刷新
        try:
            while time.time() < deadline:
                # 1) 完成检测:只认 getSteps=DONE(csrftoken 修好后可靠)。
                #    不再用"弹窗消失"兜底 —— 弹窗切换瞬间会误判,会在用户扫完前提前关会话。
                done = False
                if sess.biz_no:
                    csrf = (self._captured_headers.get(username) or {}).get("csrftoken")
                    status = await self._get_challenge_step_status(page, sess.biz_no, csrf)
                    if status:
                        logger.info("[%s] getSteps status=%s", username, status)
                        sess.message = f"QR captured — waiting (bizNo={sess.biz_no}, status={status})"
                    if status in ("DONE", "PASS", "SUCCESS"):
                        done = True
                if done:
                    sess.status = "verified"
                    sess.verified_at = time.time()
                    sess.finished_at = sess.verified_at
                    sess.message = f"verification completed (bizNo={sess.biz_no})"
                    logger.info("[%s] face verification DONE bizNo=%s", username, sess.biz_no)
                    await self._sync_cookies_to_db(context, username)
                    return
                # 2) 只有距上次出码 >=55s(即码已过期)才点「重新验证」刷新 —— 避免在用户扫码/处理中拆台
                if time.time() - last_refresh >= 55:
                    if await self._refresh_qr(page, sess, out_path):
                        await self._sync_cookies_to_db(context, username)
                        last_refresh = time.time()
                await asyncio.sleep(3)

            if sess.status == "waiting_scan":
                sess.status = "expired"
                sess.finished_at = time.time()
                suffix = f" (bizNo={sess.biz_no})" if sess.biz_no else ""
                sess.message = f"QR wait timeout; trigger a fresh QR if needed{suffix}"
                logger.warning("[%s] face verification wait expired%s", username, suffix)
        except asyncio.CancelledError:
            raise
        except Exception as e:  # noqa: BLE001
            logger.warning("[%s] face verification keepalive failed: %s", username, e)
        finally:
            self._hold_tasks.pop(username, None)
            held = self._held_contexts.pop(username, None)
            if held is context:
                try:
                    await context.close()
                except Exception:  # noqa: BLE001
                    pass
            elif held is not None:
                self._held_contexts[username] = held

    async def _get_challenge_step_status(
        self, page: Page, biz_no: str, csrf: str | None = None
    ) -> str | None:
        try:
            result = await page.evaluate(
                """
                async ([bizNo, csrf]) => {
                    const h = { "mfa-flag": "1", "clienttype": "web" };
                    if (csrf) h["csrftoken"] = csrf;
                    const url = `/bapi/accounts/v1/protect/risk/challenge/getSteps?bizNo=${encodeURIComponent(bizNo)}`;
                    const resp = await fetch(url, { credentials: "include", headers: h });
                    const text = await resp.text();
                    let body = null;
                    try { body = JSON.parse(text); } catch (_) {}
                    return { status: resp.status, body, text };
                }
                """,
                [biz_no, csrf],
            )
        except Exception as e:  # noqa: BLE001
            logger.debug("getSteps poll failed bizNo=%s: %s", biz_no, e)
            return None

        if not isinstance(result, dict):
            return None
        body = result.get("body")
        if isinstance(body, dict):
            data = body.get("data")
            if isinstance(data, dict):
                status = data.get("status")
                if status:
                    return str(status)
            code = body.get("code")
            if code:
                return f"code={code}"
        return None

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
