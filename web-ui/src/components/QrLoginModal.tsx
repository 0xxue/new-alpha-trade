import { useEffect, useRef, useState } from "react";
import {
  cancelLogin,
  getLoginStatus,
  qrImageUrl,
  refreshQr,
  startLogin,
  type LoginStatus,
} from "../api/qr";

interface Props {
  open: boolean;
  onClose: (success: boolean) => void;
  /// 预填用户名（从"重新扫"入口进来时用，避免用户每次手动输入）
  presetUsername?: string;
}

const STATUS_LABEL: Record<LoginStatus, string> = {
  pending: "正在打开浏览器…",
  qr_ready: "请用币安 App 扫描二维码",
  scanned: "已扫码，正在确认…",
  success: "登录成功",
  expired: "会话超时，请重试",
  failed: "登录失败",
};

export default function QrLoginModal({ open, onClose, presetUsername }: Props) {
  const [username, setUsername] = useState(presetUsername ?? "");
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [status, setStatus] = useState<LoginStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [imageTick, setImageTick] = useState(0);
  const pollRef = useRef<number | null>(null);
  const tickRef = useRef<number | null>(null);

  const cleanup = () => {
    if (pollRef.current !== null) {
      window.clearInterval(pollRef.current);
      pollRef.current = null;
    }
    if (tickRef.current !== null) {
      window.clearInterval(tickRef.current);
      tickRef.current = null;
    }
  };

  useEffect(() => {
    if (!open) {
      cleanup();
      setSessionId(null);
      setStatus(null);
      setError(null);
      setUsername(presetUsername ?? "");
    }
  }, [open]);

  useEffect(() => {
    if (!sessionId) return;
    pollRef.current = window.setInterval(async () => {
      try {
        const s = await getLoginStatus(sessionId);
        setStatus(s.status);
        setError(s.error);
        if (s.status === "success") {
          cleanup();
          onClose(true);
        } else if (s.status === "failed" || s.status === "expired") {
          cleanup();
        }
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      }
    }, 1500);
    tickRef.current = window.setInterval(() => setImageTick((n) => n + 1), 3000);
    return cleanup;
  }, [sessionId, onClose]);

  if (!open) return null;

  const handleStart = async () => {
    setError(null);
    const u = username.trim();
    if (!u) {
      setError("用户名必填");
      return;
    }
    try {
      const sess = await startLogin(u);
      setSessionId(sess.session_id);
      setStatus(sess.status);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const handleCancel = async () => {
    if (sessionId) {
      await cancelLogin(sessionId).catch(() => undefined);
    }
    cleanup();
    onClose(false);
  };

  const handleRefresh = async () => {
    if (!sessionId) return;
    setError(null);
    try {
      await refreshQr(sessionId);
      // 让 image tick 立刻 +1 强制重拉一次
      setImageTick((n) => n + 1);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const imageUrl = sessionId ? `${qrImageUrl(sessionId)}&_=${imageTick}` : null;

  return (
    <div className="fixed inset-0 z-50 bg-black/60 backdrop-blur-sm flex items-center justify-center p-4">
      <div className="bg-neutral-900 border border-neutral-800 rounded-lg w-full max-w-md p-6">
        <div className="flex items-center justify-between mb-4">
          <h2 className="text-lg font-semibold">扫码登录</h2>
          <button
            onClick={handleCancel}
            className="text-neutral-500 hover:text-neutral-200 text-sm"
          >
            关闭
          </button>
        </div>

        {sessionId === null ? (
          <div className="space-y-4">
            <label className="block text-sm">
              <span className="text-neutral-400">账户名（任意标识，本地唯一）</span>
              <input
                autoFocus
                value={username}
                onChange={(e) => setUsername(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && handleStart()}
                placeholder="alice / account-1 / acct-1"
                className="mt-1 w-full bg-neutral-950 border border-neutral-800 rounded px-3 py-2 text-sm focus:border-neutral-600 outline-none"
              />
            </label>
            {error && <div className="text-sm text-red-400">{error}</div>}
            <button
              onClick={handleStart}
              className="w-full bg-yellow-500 hover:bg-yellow-400 text-black font-medium rounded px-3 py-2 text-sm"
            >
              开始扫码
            </button>
          </div>
        ) : (
          <div className="space-y-4">
            <div className="text-sm text-neutral-400">
              {status ? STATUS_LABEL[status] : "等待中…"}
            </div>
            <div className="bg-black rounded p-2 flex items-center justify-center min-h-[280px]">
              {imageUrl ? (
                <img
                  src={imageUrl}
                  alt="QR"
                  className="max-h-[260px] object-contain"
                  onError={() => {
                    /* QR 还没截好，下一轮 tick 会重试 */
                  }}
                />
              ) : (
                <div className="text-neutral-500 text-sm">QR 准备中…</div>
              )}
            </div>
            {error && <div className="text-sm text-red-400">{error}</div>}
            <div className="flex gap-2">
              <button
                onClick={handleRefresh}
                className="flex-1 bg-neutral-800 hover:bg-neutral-700 rounded px-3 py-2 text-sm"
                title="QR 卡死或过期时点这个让浏览器重 load"
              >
                刷新二维码
              </button>
              <button
                onClick={handleCancel}
                className="flex-1 bg-neutral-800 hover:bg-neutral-700 rounded px-3 py-2 text-sm"
              >
                取消
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
