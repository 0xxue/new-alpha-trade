import { useEffect, useState } from "react";
import { getCurrent2faCode, setTwofaSecret } from "../api/qr";

interface Props {
  open: boolean;
  username: string | null;
  hasTwofa: boolean;
  onClose: (changed: boolean) => void;
}

export default function TwofaModal({ open, username, hasTwofa, onClose }: Props) {
  const [secret, setSecret] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** 设置/已配置成功后展示当前 TOTP 对照 */
  const [verify, setVerify] = useState<{ code: string; remaining: number } | null>(null);

  useEffect(() => {
    if (!open) {
      setSecret("");
      setError(null);
      setVerify(null);
      return;
    }
    // 弹出时如果已配置，自动开启对照
    if (hasTwofa && username) {
      void refreshCode(username, true);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  // 每秒刷新一次（实际只在 remaining 到 0 时切到新码，但每秒查最准）
  useEffect(() => {
    if (!verify || !username) return;
    const t = window.setInterval(() => void refreshCode(username, false), 1000);
    return () => window.clearInterval(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [verify === null, username]);

  const refreshCode = async (u: string, surfaceErr: boolean) => {
    try {
      const r = await getCurrent2faCode(u);
      setVerify({ code: r.code, remaining: r.remaining_seconds });
    } catch (e) {
      if (surfaceErr) {
        // 已配置但拉不到说明 secret 异常，提示一下
        const msg = e instanceof Error ? e.message : String(e);
        if (!msg.includes("no 2FA secret")) setError(msg);
      }
    }
  };

  if (!open || !username) return null;

  const handleSet = async () => {
    const s = secret.trim().toUpperCase().replace(/\s+/g, "");
    if (!s) {
      setError("secret 不能为空（如果想清除请用'清除'按钮）");
      return;
    }
    if (!/^[A-Z2-7=]+$/.test(s)) {
      setError("secret 必须是 base32（A-Z, 2-7）");
      return;
    }
    if (s.length < 16) {
      setError(`secret 太短（当前 ${s.length} 位，至少 16 位 base32）`);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await setTwofaSecret(username, s);
      // 不直接关，先拉一次当前码让用户对照
      await refreshCode(username, true);
      setSecret("");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleClear = async () => {
    if (!confirm(`清除 ${username} 的 2FA secret？后续触发 2FA 时下单会失败。`)) return;
    setBusy(true);
    setError(null);
    try {
      await setTwofaSecret(username, null);
      onClose(true);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  // 进度条比例
  const progressPct = verify ? Math.max(0, Math.min(100, (verify.remaining / 30) * 100)) : 0;

  return (
    <div className="fixed inset-0 z-50 bg-black/60 backdrop-blur-sm flex items-center justify-center p-4">
      <div className="bg-neutral-900 border border-neutral-800 rounded-lg w-full max-w-md p-6">
        <div className="flex items-center justify-between mb-4">
          <h2 className="text-lg font-semibold">
            2FA 设置 — <span className="text-neutral-400 font-mono">{username}</span>
          </h2>
          <button
            onClick={() => onClose(verify !== null /* 设过了就让 reload */)}
            className="text-neutral-500 hover:text-neutral-200 text-sm"
          >
            关闭
          </button>
        </div>

        {/* TOTP 当前码对照（已配置时显示） */}
        {verify && (
          <div className="mb-4 p-3 rounded bg-emerald-500/5 border border-emerald-500/30">
            <div className="text-xs text-emerald-400 mb-1">
              当前 TOTP 码（对照你手机 App 上的码是否一致）
            </div>
            <div className="text-3xl font-mono tracking-widest text-emerald-300 text-center my-2">
              {verify.code.slice(0, 3)} {verify.code.slice(3)}
            </div>
            <div className="h-1 bg-neutral-950 rounded overflow-hidden">
              <div
                className="h-full bg-emerald-400 transition-all"
                style={{ width: `${progressPct}%` }}
              />
            </div>
            <div className="text-[10px] text-neutral-500 mt-1 text-right">
              {verify.remaining.toFixed(1)} 秒后刷新
            </div>
            <div className="text-xs text-neutral-500 mt-2">
              ⚠ 如果跟手机不一致，说明 secret 错了 — 重新输入。
            </div>
          </div>
        )}

        <div className="text-xs text-neutral-500 mb-3">
          当前状态：
          {hasTwofa ? (
            <span className="text-emerald-400 ml-1">✓ 已配置</span>
          ) : (
            <span className="text-neutral-400 ml-1">未配置</span>
          )}
        </div>

        <p className="text-xs text-neutral-500 mb-3 leading-relaxed">
          币安 Google Authenticator 的 base32 secret。下单触发 2FA 时引擎会自动用这个算
          TOTP 码验证。secret 保存在服务器 SQLite，永远不传到浏览器后续请求里。
        </p>

        <label className="block text-sm mb-3">
          <span className="text-neutral-400">2FA secret (base32, ≥16 位)</span>
          <input
            autoFocus
            type="password"
            value={secret}
            onChange={(e) => setSecret(e.target.value)}
            placeholder="JBSWY3DPEHPK3PXP..."
            className="mt-1 w-full bg-neutral-950 border border-neutral-800 rounded px-3 py-2 text-sm font-mono outline-none focus:border-neutral-600"
          />
        </label>

        {error && <div className="text-sm text-red-400 mb-3">{error}</div>}

        <div className="flex gap-2">
          <button
            onClick={handleSet}
            disabled={busy}
            className="flex-1 bg-yellow-500 hover:bg-yellow-400 disabled:opacity-50 text-black font-medium rounded px-3 py-2 text-sm"
          >
            {busy ? "处理中…" : hasTwofa ? "覆盖更新" : "设置"}
          </button>
          {hasTwofa && (
            <button
              onClick={handleClear}
              disabled={busy}
              className="bg-red-900/40 hover:bg-red-900/60 disabled:opacity-50 text-red-300 rounded px-3 py-2 text-sm"
            >
              清除
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
