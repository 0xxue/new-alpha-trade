// qr-service 调用封装。Vite 代理把 /api/qr/* → http://127.0.0.1:7001/*

export type LoginStatus =
  | "pending"
  | "qr_ready"
  | "scanned"
  | "success"
  | "expired"
  | "failed";

export interface AccountSummary {
  username: string;
  last_refresh: string | null;
  expires_at_ms: number | null;
  status: string;
  has_2fa: boolean;
}

export interface LoginSession {
  session_id: string;
  status: LoginStatus;
  qr_image_url: string;
}

export interface LoginStatusResp {
  session_id: string;
  username: string;
  status: LoginStatus;
  error: string | null;
  qr_image_url: string | null;
}

const base = "/api/qr";

async function jsonOrThrow<T>(resp: Response): Promise<T> {
  if (!resp.ok) {
    let detail = `HTTP ${resp.status}`;
    try {
      const body = await resp.json();
      if (body?.detail) detail = String(body.detail);
    } catch {
      /* ignore */
    }
    throw new Error(detail);
  }
  return resp.json() as Promise<T>;
}

export async function listAccounts(): Promise<AccountSummary[]> {
  return jsonOrThrow(await fetch(`${base}/auth`));
}

export async function startLogin(username: string): Promise<LoginSession> {
  return jsonOrThrow(
    await fetch(`${base}/qr/login`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username }),
    })
  );
}

export async function getLoginStatus(sessionId: string): Promise<LoginStatusResp> {
  return jsonOrThrow(await fetch(`${base}/qr/status/${sessionId}`));
}

export function qrImageUrl(sessionId: string): string {
  // 后端是覆盖式截图，靠时间戳 query 破缓存
  return `${base}/qr/image/${sessionId}?t=${Date.now()}`;
}

export async function cancelLogin(sessionId: string): Promise<void> {
  const resp = await fetch(`${base}/qr/${sessionId}`, { method: "DELETE" });
  if (!resp.ok && resp.status !== 404) {
    throw new Error(`cancel failed HTTP ${resp.status}`);
  }
}

/** 用户手动刷新 QR：触发后端 reload 页面 + 重点 QR 按钮 + 重截图 */
export async function refreshQr(sessionId: string): Promise<void> {
  const resp = await fetch(`${base}/qr/refresh/${sessionId}`, { method: "POST" });
  if (!resp.ok) {
    throw new Error(`refresh failed HTTP ${resp.status}`);
  }
}

export async function deleteAccount(username: string): Promise<void> {
  const resp = await fetch(`${base}/auth/${encodeURIComponent(username)}`, {
    method: "DELETE",
  });
  if (!resp.ok && resp.status !== 404) {
    throw new Error(`delete failed HTTP ${resp.status}`);
  }
}

export async function refreshAuth(
  username: string
): Promise<{ username: string; result: string }> {
  return jsonOrThrow(
    await fetch(`${base}/auth/${encodeURIComponent(username)}/refresh`, {
      method: "POST",
    })
  );
}

export async function setTwofaSecret(
  username: string,
  secret: string | null
): Promise<{ username: string; has_2fa: boolean }> {
  return jsonOrThrow(
    await fetch(`${base}/auth/${encodeURIComponent(username)}/2fa-secret`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ secret }),
    })
  );
}

export async function getCurrent2faCode(
  username: string
): Promise<{ username: string; code: string; remaining_seconds: number; step_seconds: number }> {
  return jsonOrThrow(
    await fetch(`${base}/auth/${encodeURIComponent(username)}/2fa-code`)
  );
}

// ---------------------------------------------------------------- 远程交互
// 币安扫码后可能弹「安全验证要求 0/3」，面板上的截图原本点不了 → 账号加不进去。
// 下面三个把用户操作转发到服务器上的无头浏览器，动作后后端会立即重截图。

/** 在截图上点一下。x/y 是图片原始尺寸下的坐标（= 页面坐标）。 */
export async function clickAt(sessionId: string, x: number, y: number): Promise<void> {
  const r = await fetch(`${base}/qr/click/${sessionId}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ x, y }),
  });
  if (!r.ok) throw new Error(`点击失败: HTTP ${r.status}`);
}

/** 往当前焦点输入框敲字（验证码等）。 */
export async function typeText(sessionId: string, text: string): Promise<void> {
  const r = await fetch(`${base}/qr/type/${sessionId}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ text }),
  });
  if (!r.ok) throw new Error(`输入失败: HTTP ${r.status}`);
}

/** 按键：Enter / Backspace / Tab / Escape 等。 */
export async function pressKey(sessionId: string, key: string): Promise<void> {
  const r = await fetch(`${base}/qr/key/${sessionId}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ key }),
  });
  if (!r.ok) throw new Error(`按键失败: HTTP ${r.status}`);
}
