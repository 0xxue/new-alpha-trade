import { useEffect, useRef, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import {
  cancelLogin,
  getLoginStatus,
  qrImageUrl,
  refreshQr,
  startLogin,
  type LoginStatus,
} from "../api/qr";

const STATUS_LABEL: Record<LoginStatus, string> = {
  pending: "正在打开浏览器…",
  qr_ready: "请用币安 App 扫描二维码",
  scanned: "已扫码，正在确认…",
  success: "登录成功",
  expired: "会话超时，请重试",
  failed: "登录失败",
};

/// 整页扫码登录（替代原来的弹窗）。/login?user=xxx 可预填账户名。
export default function Login() {
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const preset = params.get("user") ?? "";

  const [username, setUsername] = useState(preset);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [status, setStatus] = useState<LoginStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [imageTick, setImageTick] = useState(0);
  const pollRef = useRef<number | null>(null);
  const tickRef = useRef<number | null>(null);

  const cleanup = () => {
    if (pollRef.current !== null) window.clearInterval(pollRef.current);
    if (tickRef.current !== null) window.clearInterval(tickRef.current);
    pollRef.current = null;
    tickRef.current = null;
  };

  const goBack = () => {
    cleanup();
    navigate("/accounts");
  };

  useEffect(() => {
    if (!sessionId) return;
    pollRef.current = window.setInterval(async () => {
      try {
        const s = await getLoginStatus(sessionId);
        setStatus(s.status);
        setError(s.error);
        if (s.status === "success") {
          cleanup();
          navigate("/accounts");
        } else if (s.status === "failed" || s.status === "expired") {
          cleanup();
        }
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      }
    }, 1500);
    tickRef.current = window.setInterval(() => setImageTick((n) => n + 1), 3000);
    return cleanup;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

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
    if (sessionId) await cancelLogin(sessionId).catch(() => undefined);
    goBack();
  };

  const handleRefresh = async () => {
    if (!sessionId) return;
    setError(null);
    try {
      await refreshQr(sessionId);
      setImageTick((n) => n + 1);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const imageUrl = sessionId ? `${qrImageUrl(sessionId)}&_=${imageTick}` : null;

  return (
    <div className="min-h-screen w-full bg-neutral-950 flex flex-col items-center justify-center p-4">
      <div className="w-full max-w-sm">
        <div className="text-center mb-6">
          <div className="text-xl font-semibold">new-alpha-trade</div>
          <div className="text-sm text-neutral-500 mt-1">扫码登录账户</div>
        </div>

        <div className="bg-neutral-900 border border-neutral-800 rounded-2xl p-5 sm:p-6 shadow-xl">
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
                  className="mt-1 w-full bg-neutral-950 border border-neutral-800 rounded-lg px-3 py-3 text-base focus:border-neutral-600 outline-none"
                />
              </label>
              {error && <div className="text-sm text-red-400">{error}</div>}
              <button
                onClick={handleStart}
                className="w-full bg-yellow-500 hover:bg-yellow-400 active:bg-yellow-500 text-black font-medium rounded-lg px-3 py-3 text-base"
              >
                开始扫码
              </button>
              <button
                onClick={goBack}
                className="w-full text-neutral-500 hover:text-neutral-300 text-sm py-1"
              >
                返回
              </button>
            </div>
          ) : (
            <div className="space-y-4">
              <div className="text-center text-sm text-neutral-400">
                {status ? STATUS_LABEL[status] : "等待中…"}
              </div>
              <div className="bg-white rounded-xl p-3 flex items-center justify-center aspect-square">
                {imageUrl ? (
                  <img
                    src={imageUrl}
                    alt="QR"
                    className="w-full h-full object-contain"
                    onError={() => {
                      /* QR 还没截好，下一轮 tick 会重试 */
                    }}
                  />
                ) : (
                  <div className="text-neutral-500 text-sm">QR 准备中…</div>
                )}
              </div>
              {error && <div className="text-sm text-red-400 text-center">{error}</div>}
              <div className="flex gap-2">
                <button
                  onClick={handleRefresh}
                  className="flex-1 bg-neutral-800 hover:bg-neutral-700 rounded-lg px-3 py-3 text-sm"
                  title="QR 卡死或过期时点这个让浏览器重 load"
                >
                  刷新二维码
                </button>
                <button
                  onClick={handleCancel}
                  className="flex-1 bg-neutral-800 hover:bg-neutral-700 rounded-lg px-3 py-3 text-sm"
                >
                  取消
                </button>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
