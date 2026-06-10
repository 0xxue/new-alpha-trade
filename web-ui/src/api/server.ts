// 服务器元信息（到期日 / 购买日）

const base = "/api";

export interface ServerMeta {
  purchased_at: string; // YYYY-MM-DD
  expires_at: string;   // YYYY-MM-DD
  days_left: number;
  days_total: number;
  days_used: number;
}

async function jsonOrThrow<T>(resp: Response): Promise<T> {
  if (!resp.ok) {
    let detail = `HTTP ${resp.status}`;
    try {
      const body = await resp.json();
      if (body?.error) detail = String(body.error);
    } catch {
      /* ignore */
    }
    throw new Error(detail);
  }
  return resp.json() as Promise<T>;
}

export async function getServerMeta(): Promise<ServerMeta> {
  return jsonOrThrow(await fetch(`${base}/server-meta`));
}

export async function renewServerMeta(): Promise<ServerMeta> {
  return jsonOrThrow(await fetch(`${base}/server-meta/renew`, { method: "POST" }));
}
