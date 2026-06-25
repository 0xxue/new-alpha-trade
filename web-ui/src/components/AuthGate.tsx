import { useEffect, useState, type ReactNode } from "react";
import { getCreds, setCreds, verifyCreds } from "../auth";

/// 把整个 app 包起来：未登录显示登录页，登录后才渲染 children。
export default function AuthGate({ children }: { children: ReactNode }) {
  const [phase, setPhase] = useState<"checking" | "login" | "ok">(
    getCreds() ? "checking" : "login"
  );
  const [user, setUser] = useState("admin");
  const [pass, setPass] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // 有存储凭据时启动校验一次（防止密码改了还放行）
  useEffect(() => {
    if (phase !== "checking") return;
    fetch("/api/tokens")
      .then((r) => setPhase(r.ok ? "ok" : "login"))
      .catch(() => setPhase("login"));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const submit = async () => {
    const u = user.trim();
    if (!u || !pass) {
      setErr("请输入用户名和密码");
      return;
    }
    setErr(null);
    setBusy(true);
    const ok = await verifyCreds(u, pass);
    setBusy(false);
    if (ok) {
      setCreds(u, pass);
      setPhase("ok");
    } else {
      setErr("用户名或密码错误");
    }
  };

  if (phase === "ok") return <>{children}</>;

  if (phase === "checking") {
    return (
      <div className="min-h-screen flex items-center justify-center text-neutral-500 text-sm">
        验证登录中…
      </div>
    );
  }

  return (
    <div className="min-h-screen w-full bg-neutral-950 flex flex-col items-center justify-center p-4">
      <div className="w-full max-w-sm">
        <div className="text-center mb-6">
          <div className="text-2xl font-semibold">new-alpha-trade</div>
          <div className="text-sm text-neutral-500 mt-1">请登录</div>
        </div>
        <div className="bg-neutral-900 border border-neutral-800 rounded-2xl p-5 sm:p-6 shadow-xl space-y-4">
          <label className="block text-sm">
            <span className="text-neutral-400">用户名</span>
            <input
              value={user}
              onChange={(e) => setUser(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && submit()}
              autoComplete="username"
              className="mt-1 w-full bg-neutral-950 border border-neutral-800 rounded-lg px-3 py-3 text-base focus:border-neutral-600 outline-none"
            />
          </label>
          <label className="block text-sm">
            <span className="text-neutral-400">密码</span>
            <input
              type="password"
              value={pass}
              onChange={(e) => setPass(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && submit()}
              autoComplete="current-password"
              className="mt-1 w-full bg-neutral-950 border border-neutral-800 rounded-lg px-3 py-3 text-base focus:border-neutral-600 outline-none"
            />
          </label>
          {err && <div className="text-sm text-red-400">{err}</div>}
          <button
            onClick={submit}
            disabled={busy}
            className="w-full bg-yellow-500 hover:bg-yellow-400 disabled:opacity-50 text-black font-medium rounded-lg px-3 py-3 text-base"
          >
            {busy ? "登录中…" : "登录"}
          </button>
        </div>
      </div>
    </div>
  );
}
