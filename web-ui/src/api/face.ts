// qr-service 人脸/手机验证截图 API（走 nginx /api/qr/face/*）

const base = "/api/qr/face";

export type FaceStatus =
  | "idle"
  | "running"
  | "no_dialog"
  | "dialog_no_phone"
  | "captured"
  | "failed";

export interface FaceSession {
  username: string;
  status: FaceStatus;
  message: string;
  screenshot_available: boolean;
  started_at?: number;
  finished_at?: number | null;
}

async function jsonOrThrow<T>(resp: Response): Promise<T> {
  if (!resp.ok) {
    let detail = `HTTP ${resp.status}`;
    try {
      const body = await resp.json();
      if (body?.detail) detail = String(body.detail);
      else if (body?.error) detail = String(body.error);
    } catch {
      /* ignore */
    }
    throw new Error(detail);
  }
  return resp.json() as Promise<T>;
}

export async function triggerFace(
  username: string,
  symbol: string,
  amount_usdt: number
): Promise<FaceSession> {
  return jsonOrThrow(
    await fetch(`${base}/${encodeURIComponent(username)}/trigger`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ symbol, amount_usdt }),
    })
  );
}

export async function getFaceStatus(username: string): Promise<FaceSession> {
  return jsonOrThrow(await fetch(`${base}/${encodeURIComponent(username)}/status`));
}

/** 二维码图片 URL — 带时间戳防缓存 */
export function faceQrUrl(username: string): string {
  return `${base}/${encodeURIComponent(username)}/qr?t=${Date.now()}`;
}
