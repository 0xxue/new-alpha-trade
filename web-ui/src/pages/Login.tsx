import { useEffect, useRef, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import {
  cancelLogin,
  getLoginStatus,
  qrImageUrl,
  refreshQr,
  startLogin,
  clickAt,
  typeText,
  pressKey,
  type LoginStatus,
} from "../api/qr";
import AuthImg from "../components/AuthImg";

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
  // 远程操作：币安弹「安全验证要求 0/3」时，截图能点、能输入，直接在服务器
  // 的浏览器里完成验证（否则账号加不进去）。
  const [interactive, setInteractive] = useState(false);
  const [typeBuf, setTypeBuf] = useState("");
  const [acting, setActing] = useState(false);
  const [actErr, setActErr] = useState<string | null>(null);
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

  // 任一远程操作后立刻 bump imageTick 拉最新截图（后端动作完成即已重截）
  const runAction = async (fn: () => Promise<void>) => {
    if (!sessionId || acting) return;
    setActing(true);
    setActErr(null);
    try {
      await fn();
      setImageTick((n) => n + 1);
    } catch (e) {
      setActErr(e instanceof Error ? e.message : String(e));
    } finally {
      setActing(false);
    }
  };

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
                  <AuthImg
                    src={imageUrl}
                    alt="QR"
                    className="w-full h-full object-contain"
                    onError={() => {
                      /* QR 还没截好，下一轮 tick 会重试 */
                    }}
                    onImageClick={
                      interactive
                        ? (x, y) => void runAction(() => clickAt(sessionId!, x, y))
                        : undefined
                    }
                  />
                ) : (
                  <div className="text-neutral-500 text-sm">QR 准备中…</div>
                )}
              </div>

              {/* 远程操作：币安要求「安全验证」时，这张截图就是服务器上的浏览器画面，
                  打开开关后可以直接点它、给它输入，完成验证后登录会继续。 */}
              <div className="rounded-lg border border-neutral-800 bg-neutral-900/60 p-3 space-y-2">
                <label className="flex items-center gap-2 text-sm cursor-pointer select-none">
                  <input
                    type="checkbox"
                    checked={interactive}
                    onChange={(e) => {
                      setInteractive(e.target.checked);
                      setActErr(null);
                    }}
                    className="accent-yellow-500"
                  />
                  <span>
                    手动操作模式
                    <span className="text-neutral-500 text-xs ml-2">
                      出现「安全验证要求」时打开，可直接点击画面完成验证
                    </span>
                  </span>
                </label>

                {interactive && (
                  <>
                    <div className="text-xs text-neutral-500">
                      点击上方画面 = 在服务器浏览器里点同一位置；下方输入框可给焦点处打字。
                      {acting && <span className="text-yellow-400 ml-1">执行中…</span>}
                    </div>
                    <div className="flex gap-2">
                      <input
                        value={typeBuf}
                        onChange={(e) => setTypeBuf(e.target.value)}
                        onKeyDown={(e) => {
                          if (e.key === "Enter" && typeBuf) {
                            void runAction(async () => {
                              await typeText(sessionId!, typeBuf);
                              setTypeBuf("");
                            });
                          }
                        }}
                        placeholder="验证码 / 邮箱等，回车发送"
                        className="flex-1 bg-neutral-950 border border-neutral-800 rounded px-3 py-2 text-sm outline-none focus:border-neutral-600"
                      />
                      <button
                        disabled={acting || !typeBuf}
                        onClick={() =>
                          void runAction(async () => {
                            await typeText(sessionId!, typeBuf);
                            setTypeBuf("");
                          })
                        }
                        className="px-3 py-2 text-sm bg-neutral-800 hover:bg-neutral-700 disabled:opacity-30 rounded"
                      >
                        发送
                      </button>
                    </div>
                    <div className="flex gap-2">
                      {(["Enter", "Backspace", "Tab", "Escape"] as const).map((k) => (
                        <button
                          key={k}
                          disabled={acting}
                          onClick={() => void runAction(() => pressKey(sessionId!, k))}
                          className="flex-1 px-2 py-1.5 text-xs bg-neutral-800 hover:bg-neutral-700 disabled:opacity-30 rounded"
                        >
                          {k === "Enter" ? "回车" : k === "Backspace" ? "退格" : k}
                        </button>
                      ))}
                    </div>
                    {actErr && <div className="text-xs text-red-400">{actErr}</div>}
                  </>
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
