import { useState } from "react";
import { logout } from "../auth";

export default function Settings() {
  const [oldPw, setOldPw] = useState("");
  const [newPw, setNewPw] = useState("");
  const [confirmPw, setConfirmPw] = useState("");
  const [msg, setMsg] = useState<{ type: "ok" | "err"; text: string } | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setMsg(null);
    if (newPw.length < 6) {
      setMsg({ type: "err", text: "新密码至少 6 位" });
      return;
    }
    if (newPw !== confirmPw) {
      setMsg({ type: "err", text: "两次输入的新密码不一致" });
      return;
    }
    setBusy(true);
    try {
      const r = await fetch("/api/change-password", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ old_password: oldPw, new_password: newPw }),
      });
      if (r.ok) {
        setMsg({ type: "ok", text: "✅ 密码已修改,即将自动登出,请用新密码重新登录..." });
        setTimeout(() => logout(), 2500);
      } else {
        const j = await r.json().catch(() => ({} as { error?: string }));
        setMsg({ type: "err", text: j.error || `修改失败(HTTP ${r.status})` });
      }
    } catch (err) {
      setMsg({ type: "err", text: "请求失败:" + (err instanceof Error ? err.message : String(err)) });
    } finally {
      setBusy(false);
    }
  }

  const inputCls =
    "w-full bg-neutral-800 border border-neutral-700 rounded px-3 py-2 text-sm focus:outline-none focus:border-neutral-500";

  return (
    <div className="max-w-md">
      <h1 className="text-xl font-semibold mb-1">设置</h1>
      <p className="text-sm text-neutral-400 mb-4">修改网站后台登录密码</p>
      <form onSubmit={submit} className="space-y-3 bg-neutral-900 border border-neutral-800 rounded p-4">
        <input
          type="password"
          placeholder="旧密码"
          value={oldPw}
          onChange={(e) => setOldPw(e.target.value)}
          autoComplete="current-password"
          className={inputCls}
          required
        />
        <input
          type="password"
          placeholder="新密码(至少 6 位)"
          value={newPw}
          onChange={(e) => setNewPw(e.target.value)}
          autoComplete="new-password"
          className={inputCls}
          required
        />
        <input
          type="password"
          placeholder="确认新密码"
          value={confirmPw}
          onChange={(e) => setConfirmPw(e.target.value)}
          autoComplete="new-password"
          className={inputCls}
          required
        />
        <button
          type="submit"
          disabled={busy}
          className="w-full bg-blue-600 hover:bg-blue-500 disabled:opacity-50 rounded px-3 py-2 text-sm font-medium"
        >
          {busy ? "提交中..." : "修改密码"}
        </button>
        {msg && (
          <div className={`text-sm ${msg.type === "ok" ? "text-green-400" : "text-red-400"}`}>{msg.text}</div>
        )}
      </form>
      <p className="text-xs text-neutral-500 mt-3">
        改的是这个网站后台的登录密码(用户名 admin)。改成功后会自动登出,用新密码重新登录即可。
      </p>
    </div>
  );
}
