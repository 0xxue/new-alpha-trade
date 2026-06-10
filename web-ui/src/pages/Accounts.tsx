import { useCallback, useEffect, useState } from "react";
import {
  deleteAccount,
  listAccounts,
  refreshAuth,
  type AccountSummary,
} from "../api/qr";
import QrLoginModal from "../components/QrLoginModal";
import TwofaModal from "../components/TwofaModal";

const STATUS_CLASS: Record<string, string> = {
  active: "bg-emerald-500/20 text-emerald-400",
  expired: "bg-red-500/20 text-red-400",
};

function fmtExpiry(ms: number | null): string {
  if (ms == null) return "—";
  const d = new Date(ms);
  return d.toLocaleString();
}

export default function Accounts() {
  const [accounts, setAccounts] = useState<AccountSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showLogin, setShowLogin] = useState(false);
  const [busyUser, setBusyUser] = useState<string | null>(null);
  const [twofaTarget, setTwofaTarget] = useState<AccountSummary | null>(null);
  /// 预填 username 给 QrLoginModal — 点 inline "重新扫码" 时设置
  const [presetUsername, setPresetUsername] = useState<string>("");

  const reload = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const accs = await listAccounts();
      setAccounts(accs);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  const onModalClose = (success: boolean) => {
    setShowLogin(false);
    if (success) void reload();
  };

  const handleDelete = async (username: string) => {
    if (!confirm(`确定删除账户 ${username}？凭据将丢失，需要重新扫码。`)) return;
    setBusyUser(username);
    try {
      await deleteAccount(username);
      await reload();
    } catch (e) {
      alert(e instanceof Error ? e.message : String(e));
    } finally {
      setBusyUser(null);
    }
  };

  const handleRefresh = async (username: string) => {
    setBusyUser(username);
    try {
      const r = await refreshAuth(username);
      // 更精准的文案——区分 ok / expired / error 三种情况
      if (r.result === "ok") {
        alert("✓ 凭据有效，刷新时间已更新");
      } else if (r.result === "expired") {
        alert("✗ 凭据确认已过期（币安返回 401/403），请重新扫码");
      } else if (r.result === "error") {
        alert(
          "⚠ 探测网络/上游异常（无法确定凭据是否有效）\n" +
            "如果你 trade 实际报登录失效，再重新扫码；否则可能只是临时问题"
        );
      } else {
        alert(`刷新结果：${r.result}`);
      }
      await reload();
    } catch (e) {
      alert(e instanceof Error ? e.message : String(e));
    } finally {
      setBusyUser(null);
    }
  };

  const handleRescan = (username: string) => {
    setPresetUsername(username);
    setShowLogin(true);
  };

  return (
    <div>
      <div className="flex items-center justify-between mb-6">
        <h1 className="text-2xl font-semibold">账户</h1>
        <div className="flex gap-2">
          <button
            onClick={reload}
            disabled={loading}
            className="px-3 py-1.5 text-sm bg-neutral-800 hover:bg-neutral-700 rounded disabled:opacity-50"
          >
            {loading ? "刷新中…" : "刷新"}
          </button>
          <button
            onClick={() => setShowLogin(true)}
            className="px-3 py-1.5 text-sm bg-yellow-500 hover:bg-yellow-400 text-black font-medium rounded"
          >
            + 扫码新账户
          </button>
        </div>
      </div>

      {error && (
        <div className="mb-4 p-3 rounded bg-red-500/10 border border-red-500/30 text-sm text-red-300">
          {error}
        </div>
      )}

      <div className="bg-neutral-900 border border-neutral-800 rounded-lg overflow-hidden">
        <table className="w-full text-sm">
          <thead className="bg-neutral-950/50 text-neutral-400 text-xs uppercase tracking-wider">
            <tr>
              <th className="text-left px-4 py-3">账户</th>
              <th className="text-left px-4 py-3">状态</th>
              <th className="text-left px-4 py-3">最后刷新</th>
              <th className="text-left px-4 py-3">过期时间</th>
              <th className="text-left px-4 py-3">2FA</th>
              <th className="text-right px-4 py-3">操作</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-neutral-800">
            {accounts.length === 0 && !loading && (
              <tr>
                <td colSpan={6} className="px-4 py-8 text-center text-neutral-500">
                  还没有账户。点右上"+ 扫码新账户"开始。
                </td>
              </tr>
            )}
            {accounts.map((a) => {
              const busy = busyUser === a.username;
              const cls = STATUS_CLASS[a.status] ?? "bg-neutral-700/30 text-neutral-300";
              return (
                <tr key={a.username} className="hover:bg-neutral-800/30">
                  <td className="px-4 py-3 font-medium">{a.username}</td>
                  <td className="px-4 py-3">
                    <span className={`px-2 py-0.5 rounded text-xs ${cls}`}>{a.status}</span>
                  </td>
                  <td className="px-4 py-3 text-neutral-400">{a.last_refresh ?? "—"}</td>
                  <td className="px-4 py-3 text-neutral-400">{fmtExpiry(a.expires_at_ms)}</td>
                  <td className="px-4 py-3">
                    <button
                      onClick={() => setTwofaTarget(a)}
                      className={`px-2 py-0.5 rounded text-xs ${
                        a.has_2fa
                          ? "bg-emerald-500/20 text-emerald-400 hover:bg-emerald-500/30"
                          : "bg-neutral-700/40 text-neutral-400 hover:bg-neutral-700/60"
                      }`}
                      title={a.has_2fa ? "已配置 2FA — 点击修改" : "未配置 — 点击设置"}
                    >
                      {a.has_2fa ? "✓ 已配" : "未设"}
                    </button>
                  </td>
                  <td className="px-4 py-3 text-right">
                    <button
                      onClick={() => handleRefresh(a.username)}
                      disabled={busy}
                      className="px-2 py-1 text-xs bg-neutral-800 hover:bg-neutral-700 rounded mr-2 disabled:opacity-50"
                      title="后端调一个 alpha 私有端点测试 cookies 有效性"
                    >
                      探测
                    </button>
                    <button
                      onClick={() => handleRescan(a.username)}
                      disabled={busy}
                      className="px-2 py-1 text-xs bg-yellow-600/40 hover:bg-yellow-600/60 text-yellow-200 rounded mr-2 disabled:opacity-50"
                      title="重新扫码登录该账户，cookies/headers 会被覆盖，2FA secret 保留"
                    >
                      重新扫
                    </button>
                    <button
                      onClick={() => handleDelete(a.username)}
                      disabled={busy}
                      className="px-2 py-1 text-xs bg-red-900/40 hover:bg-red-900/60 text-red-300 rounded disabled:opacity-50"
                    >
                      删除
                    </button>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>

      <QrLoginModal
        open={showLogin}
        onClose={(s) => {
          setPresetUsername("");
          onModalClose(s);
        }}
        presetUsername={presetUsername}
      />
      <TwofaModal
        open={twofaTarget !== null}
        username={twofaTarget?.username ?? null}
        hasTwofa={twofaTarget?.has_2fa ?? false}
        onClose={(changed) => {
          setTwofaTarget(null);
          if (changed) void reload();
        }}
      />
    </div>
  );
}
