// App 内登录：把 Basic Auth 凭据存本地，给所有 /api 请求自动带 Authorization 头。
// nginx 静态页放开（不再弹原生登录框），/api 仍由 Basic Auth 保护 —— 真正的校验还在 nginx。

const KEY = "nat_auth";

export function getCreds(): string | null {
  return localStorage.getItem(KEY);
}

export function setCreds(user: string, pass: string): void {
  localStorage.setItem(KEY, btoa(`${user}:${pass}`));
}

export function clearCreds(): void {
  localStorage.removeItem(KEY);
}

export function logout(): void {
  clearCreds();
  window.location.href = "/";
}

/// 单点拦截 window.fetch：给同源 /api* 请求加 Authorization: Basic <creds>。
/// 必须在任何 fetch 之前调用（main.tsx 里）。
export function installFetchAuth(): void {
  const orig = window.fetch.bind(window);
  window.fetch = (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const url =
      typeof input === "string"
        ? input
        : input instanceof URL
          ? input.toString()
          : (input as Request).url;
    const creds = getCreds();
    const isApi = url.startsWith("/api") || url.includes("/api/");
    if (isApi) {
      init = { ...init };
      const headers = new Headers(
        init.headers ?? (input instanceof Request ? input.headers : undefined)
      );
      if (creds && !headers.has("Authorization")) headers.set("Authorization", `Basic ${creds}`);
      init.headers = headers;
      // 防止请求永久挂起（网络抽风时"创建中"卡死）→ 25s 自动中止并报错
      if (!init.signal) {
        const ctrl = new AbortController();
        setTimeout(() => ctrl.abort(), 25000);
        init.signal = ctrl.signal;
      }
    }
    return orig(input as RequestInfo, init);
  };
}

/// 用一组用户名/密码打受保护端点验证（200 即正确）。
export async function verifyCreds(user: string, pass: string): Promise<boolean> {
  const creds = btoa(`${user}:${pass}`);
  try {
    const r = await fetch("/api/tokens", { headers: { Authorization: `Basic ${creds}` } });
    return r.ok;
  } catch {
    return false;
  }
}
