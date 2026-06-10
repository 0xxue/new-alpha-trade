import { useCallback, useEffect, useState } from "react";
import { getServerMeta, renewServerMeta, type ServerMeta } from "../api/server";

/**
 * 左侧 nav 底部的小卡片：显示服务器到期天数 + 续费按钮。
 * - days_left > 7  → 绿色
 * - 3 < days_left <= 7 → 黄色
 * - days_left <= 3 → 红色
 * - days_left <= 0 → "已过期"
 */
export default function ServerExpiryCard() {
  const [meta, setMeta] = useState<ServerMeta | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [renewing, setRenewing] = useState(false);

  const reload = useCallback(async () => {
    try {
      const m = await getServerMeta();
      setMeta(m);
      setErr(null);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    void reload();
    // 每小时刷一次（天数变化频率低）
    const t = window.setInterval(reload, 60 * 60 * 1000);
    return () => window.clearInterval(t);
  }, [reload]);

  const handleRenew = async () => {
    if (!confirm("确认续费 +30 天？")) return;
    setRenewing(true);
    try {
      const m = await renewServerMeta();
      setMeta(m);
      setErr(null);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setRenewing(false);
    }
  };

  if (err) {
    return (
      <div className="mt-6 p-2 rounded text-[11px] text-red-400 border border-red-500/30 bg-red-500/5">
        到期信息加载失败：{err}
      </div>
    );
  }
  if (!meta) {
    return <div className="mt-6 p-2 text-[11px] text-neutral-600">加载到期信息…</div>;
  }

  const left = meta.days_left;
  const expired = left <= 0;
  const color = expired
    ? "text-red-400"
    : left <= 3
    ? "text-red-400"
    : left <= 7
    ? "text-yellow-400"
    : "text-emerald-400";

  return (
    <div className="mt-6 p-2 rounded border border-neutral-800 bg-neutral-950">
      <div className="text-[10px] text-neutral-500 mb-1">服务器</div>
      <div className={`text-sm font-mono ${color}`}>
        {expired ? "已过期" : `剩 ${left} 天`}
      </div>
      <div className="text-[10px] text-neutral-600 mt-0.5">至 {meta.expires_at}</div>
      <button
        onClick={handleRenew}
        disabled={renewing}
        className="mt-2 w-full px-2 py-1 text-[11px] bg-neutral-800 hover:bg-neutral-700 disabled:opacity-50 rounded text-neutral-200"
      >
        {renewing ? "续费中…" : "续费 +30 天"}
      </button>
    </div>
  );
}
