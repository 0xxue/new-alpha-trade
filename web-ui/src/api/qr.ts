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
