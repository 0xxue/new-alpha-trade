import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  jobStats,
  listJobs,
  listTokens,
  pauseJob,
  resumeJob,
  startJob,
  stopJob,
  type JobRow,
  type JobStats,
  type RoundStats,
  type TokenInfo,
} from "../api/trade";
import {
  faceQrUrl,
  getFaceStatus,
  triggerFace,
  type FaceSession,
} from "../api/face";
import JobChart from "../components/JobChart";
import OrderEvents from "../components/OrderEvents";

const STATE_COLOR: Record<string, string> = {
  pending: "bg-neutral-500/20 text-neutral-300",
  running: "bg-emerald-500/20 text-emerald-400",
  paused: "bg-yellow-500/20 text-yellow-400",
  stopped: "bg-red-500/20 text-red-400",
  done: "bg-blue-500/20 text-blue-400",
  failed: "bg-red-500/20 text-red-400",
};

function fmtUsdt(v: string | null | undefined, digits = 4): string {
  if (v == null) return "—";
  const n = parseFloat(v);
  if (!Number.isFinite(n)) return v;
  return n.toFixed(digits);
}

export default function Trade() {
  const [jobs, setJobs] = useState<JobRow[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [stats, setStats] = useState<JobStats | null>(null);
  const [statsError, setStatsError] = useState<string | null>(null);

  // 新建 job 表单
  const [username, setUsername] = useState("");
  const [symbolQuery, setSymbolQuery] = useState("NEX"); // 用户输入的友好名
  const [target, setTarget] = useState("16400");
  const [singleMin, setSingleMin] = useState("25");
  const [singleMax, setSingleMax] = useState("38");
  const [strategy, setStrategy] = useState<"oto" | "oto_smart" | "simple_round">("oto_smart");
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);

  // tokens registry
  const [tokens, setTokens] = useState<TokenInfo[]>([]);
  const [showSuggest, setShowSuggest] = useState(false);
  const suggestBoxRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    listTokens()
      .then((r) => setTokens(r.tokens.filter((t) => t.tradable)))
      .catch(() => {
        /* registry 没就绪也不致命 */
      });
  }, []);

  const symbolMatches = useMemo(() => {
    const q = symbolQuery.trim().toUpperCase();
    if (!q) return [];
    return tokens
      .filter(
        (t) =>
          t.symbol.toUpperCase().includes(q) ||
          t.name.toUpperCase().includes(q) ||
          t.alpha_id.includes(q)
      )
      .slice(0, 8);
  }, [symbolQuery, tokens]);

  useEffect(() => {
    const onClick = (e: MouseEvent) => {
      if (!suggestBoxRef.current?.contains(e.target as Node)) setShowSuggest(false);
    };
    document.addEventListener("mousedown", onClick);
    return () => document.removeEventListener("mousedown", onClick);
  }, []);

  const reloadJobs = useCallback(async () => {
    try {
      const xs = await listJobs();
      setJobs(xs);
      if (selectedId === null && xs.length > 0) setSelectedId(xs[0].id);
    } catch (e) {
      setCreateError(e instanceof Error ? e.message : String(e));
    }
  }, [selectedId]);

  const reloadStats = useCallback(async () => {
    if (!selectedId) {
      setStats(null);
      return;
    }
    try {
      const s = await jobStats(selectedId);
      setStats(s);
      setStatsError(null);
    } catch (e) {
      setStatsError(e instanceof Error ? e.message : String(e));
    }
  }, [selectedId]);

  // 启动：初次拉 jobs
  useEffect(() => {
    void reloadJobs();
  }, [reloadJobs]);

  // 选中 job 后：2 秒轮询 stats
  useEffect(() => {
    if (!selectedId) return;
    void reloadStats();
    const t = window.setInterval(reloadStats, 2000);
    return () => window.clearInterval(t);
  }, [selectedId, reloadStats]);

  const handleCreate = async () => {
    setCreating(true);
    setCreateError(null);
    try {
      const r = await startJob({
        username: username.trim(),
        symbol: symbolQuery.trim(),
        target_volume: target.trim(),
        single_min_usdt: singleMin.trim() || undefined,
        single_max_usdt: singleMax.trim() || undefined,
        strategy,
      });
      await reloadJobs();
      setSelectedId(r.job_id);
    } catch (e) {
      setCreateError(e instanceof Error ? e.message : String(e));
    } finally {
      setCreating(false);
    }
  };

  const handleAction = async (act: "pause" | "resume" | "stop") => {
    if (!selectedId) return;
    try {
      if (act === "pause") await pauseJob(selectedId);
      else if (act === "resume") await resumeJob(selectedId);
      else await stopJob(selectedId);
      await reloadJobs();
      await reloadStats();
    } catch (e) {
      alert(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <div>
      <h1 className="text-2xl font-semibold mb-6">交易任务</h1>

      {/* 创建表单 */}
      <div className="bg-neutral-900 border border-neutral-800 rounded-lg p-4 mb-6 max-w-5xl">
        <div className="text-sm text-neutral-400 mb-3">
          创建刷量任务 · {tokens.length} 个 Alpha 代币可选
        </div>
        <div className="grid grid-cols-1 md:grid-cols-6 gap-3">
          <input
            value={username}
            onChange={(e) => setUsername(e.target.value)}
            placeholder="账户名"
            className="bg-neutral-950 border border-neutral-800 rounded px-3 py-2 text-sm outline-none focus:border-neutral-600"
          />
          <div className="relative" ref={suggestBoxRef}>
            <input
              value={symbolQuery}
              onChange={(e) => {
                setSymbolQuery(e.target.value);
                setShowSuggest(true);
              }}
              onFocus={() => setShowSuggest(true)}
              placeholder="币种 (NEX, ZEST)"
              className="w-full bg-neutral-950 border border-neutral-800 rounded px-3 py-2 text-sm outline-none focus:border-neutral-600"
            />
            {showSuggest && symbolMatches.length > 0 && (
              <div className="absolute z-10 mt-1 w-full bg-neutral-950 border border-neutral-700 rounded shadow-lg max-h-60 overflow-auto">
                {symbolMatches.map((t) => (
                  <button
                    key={t.alpha_id}
                    type="button"
                    onClick={() => {
                      setSymbolQuery(t.symbol);
                      setShowSuggest(false);
                    }}
                    className="w-full text-left px-3 py-2 text-sm hover:bg-neutral-800 flex items-center justify-between gap-2"
                  >
                    <span className="font-medium">{t.symbol}</span>
                    <span className="text-neutral-500 text-xs truncate">
                      {t.name} · {t.alpha_id}
                    </span>
                  </button>
                ))}
              </div>
            )}
          </div>
          <input
            value={target}
            onChange={(e) => setTarget(e.target.value)}
            placeholder="目标量 USDT"
            className="bg-neutral-950 border border-neutral-800 rounded px-3 py-2 text-sm outline-none focus:border-neutral-600"
          />
          <div className="flex gap-1">
            <input
              value={singleMin}
              onChange={(e) => setSingleMin(e.target.value)}
              placeholder="单笔最低"
              title="单笔最低金额 USDT"
              className="flex-1 bg-neutral-950 border border-neutral-800 rounded px-2 py-2 text-sm outline-none focus:border-neutral-600 min-w-0"
            />
            <span className="text-neutral-600 self-center">~</span>
            <input
              value={singleMax}
              onChange={(e) => setSingleMax(e.target.value)}
              placeholder="单笔最高"
              title="单笔最高金额 USDT"
              className="flex-1 bg-neutral-950 border border-neutral-800 rounded px-2 py-2 text-sm outline-none focus:border-neutral-600 min-w-0"
            />
          </div>
          <select
            value={strategy}
            onChange={(e) =>
              setStrategy(e.target.value as "oto" | "oto_smart" | "simple_round")
            }
            title="刷量方式"
            className="bg-neutral-950 border border-neutral-800 rounded px-3 py-2 text-sm outline-none focus:border-neutral-600 cursor-pointer"
          >
            <option value="oto_smart">oto_smart (v2 决策矩阵, 默认)</option>
            <option value="oto">OTO (v1 快)</option>
            <option value="simple_round">simple_round (稳, 慢)</option>
          </select>
          <button
            onClick={handleCreate}
            disabled={creating || !username.trim() || !symbolQuery.trim()}
            className="bg-yellow-500 hover:bg-yellow-400 disabled:opacity-50 text-black font-medium rounded px-3 py-2 text-sm"
          >
            {creating ? "创建中…" : "开始刷量"}
          </button>
        </div>
        <div className="text-xs text-neutral-600 mt-2">
          单笔金额会在 [min, max] 之间随机；min == max 则固定金额。
          {strategy === "oto" ? (
            <>
              <br />
              OTO：一发买卖两单，每轮 ~3-5 秒；预期磨损 ≈ target × 0.03%。
            </>
          ) : strategy === "oto_smart" ? (
            <>
              <br />
              oto_smart v2：决策矩阵（fast / small_spread_follow / taker_maker_hybrid …），
              maker 优先；实测 wear -4 ~ -7 bps，比 v1 OTO 略好。
            </>
          ) : (
            <>
              <br />
              simple_round：BUY 等成交 SELL 等成交，每轮 ~12-15 秒；预期磨损同上。
            </>
          )}
        </div>
        {createError && (
          <div className="mt-3 text-sm text-red-400">{createError}</div>
        )}
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-3 gap-4">
        {/* job 列表 */}
        <div className="bg-neutral-900 border border-neutral-800 rounded-lg p-3 lg:col-span-1">
          <div className="text-sm text-neutral-400 mb-2 flex items-center justify-between">
            <span>任务列表 ({jobs.length})</span>
            <button
              onClick={reloadJobs}
              className="text-xs text-neutral-500 hover:text-neutral-300"
            >
              刷新
            </button>
          </div>
          {jobs.length === 0 && (
            <div className="text-sm text-neutral-600 p-4 text-center">还没有任务</div>
          )}
          <div className="space-y-1">
            {jobs.map((j) => {
              const cls = STATE_COLOR[j.state] ?? "bg-neutral-700/30 text-neutral-300";
              const selected = j.id === selectedId;
              return (
                <button
                  key={j.id}
                  onClick={() => setSelectedId(j.id)}
                  className={`w-full text-left px-3 py-2 rounded text-sm transition-colors ${
                    selected
                      ? "bg-neutral-800 border border-neutral-700"
                      : "hover:bg-neutral-800/50 border border-transparent"
                  }`}
                >
                  <div className="flex items-center justify-between gap-2">
                    <span className="font-mono text-xs truncate">{j.id.slice(0, 8)}</span>
                    <span className={`px-1.5 py-0.5 rounded text-[10px] ${cls}`}>{j.state}</span>
                  </div>
                  <div className="text-xs text-neutral-500 mt-1 truncate">
                    {j.username} · {j.symbol}
                  </div>
                  <div className="text-xs text-neutral-600">
                    target: {j.target_volume} USDT
                  </div>
                </button>
              );
            })}
          </div>
        </div>

        {/* stats 详情 */}
        <div className="lg:col-span-2 space-y-4">
          {!selectedId && (
            <div className="bg-neutral-900 border border-neutral-800 rounded-lg p-8 text-center text-neutral-500">
              选一个任务看实时统计
            </div>
          )}
          {stats && (
            <>
              {/* 进度 + 操作按钮（紧贴一起 — 暂停/继续/停止 一眼能看到）*/}
              <div className="bg-neutral-900 border border-neutral-800 rounded-lg p-4">
                <div className="flex items-center justify-between mb-2">
                  <span className="text-sm text-neutral-400">
                    Volume / Target ({stats.symbol})
                  </span>
                  <span className="text-sm text-neutral-500">
                    {(stats.progress_bps / 100).toFixed(2)}%
                  </span>
                </div>
                <div className="h-3 bg-neutral-950 rounded overflow-hidden mb-2">
                  <div
                    className="h-full bg-yellow-500 transition-all"
                    style={{
                      width: `${Math.min(100, stats.progress_bps / 100)}%`,
                    }}
                  />
                </div>
                <div className="flex items-center justify-between gap-3">
                  <div className="text-sm font-mono">
                    <span className="text-emerald-400">{fmtUsdt(stats.buy_volume_usdt, 4)}</span>
                    <span className="text-neutral-500"> / </span>
                    <span>{fmtUsdt(stats.target_volume_usdt, 2)}</span>
                    <span className="text-neutral-500"> USDT</span>
                  </div>
                  {/* ⭐ 操作按钮挪到进度条旁边 — 醒目又顺手 */}
                  <div className="flex gap-2">
                    <button
                      onClick={() => handleAction("pause")}
                      disabled={stats.state !== "running"}
                      className="px-3 py-1.5 text-sm bg-yellow-700/40 hover:bg-yellow-700/60 disabled:opacity-30 text-yellow-300 rounded font-medium"
                    >
                      ⏸ 暂停
                    </button>
                    <button
                      onClick={() => handleAction("resume")}
                      disabled={stats.state !== "paused" && stats.state !== "pending"}
                      className="px-3 py-1.5 text-sm bg-emerald-700/40 hover:bg-emerald-700/60 disabled:opacity-30 text-emerald-300 rounded font-medium"
                    >
                      ▶ 启动/继续
                    </button>
                    <button
                      onClick={() => handleAction("stop")}
                      disabled={["stopped", "done", "failed"].includes(stats.state)}
                      className="px-3 py-1.5 text-sm bg-red-700/40 hover:bg-red-700/60 disabled:opacity-30 text-red-300 rounded font-medium"
                    >
                      ⏹ 停止
                    </button>
                  </div>
                </div>
              </div>

              {/* 磨损 */}
              <div className="bg-neutral-900 border border-neutral-800 rounded-lg p-4">
                <div className="text-sm text-neutral-400 mb-3">磨损 (Wear)</div>
                <div className="grid grid-cols-2 gap-4">
                  <Stat label="Baseline SPOT USDT" value={fmtUsdt(stats.baseline_spot_usdt, 8)} />
                  <Stat label="Current SPOT USDT" value={fmtUsdt(stats.current_spot_usdt, 8)} />
                  <Stat
                    label="Wear (绝对值)"
                    value={fmtUsdt(stats.wear_amount_usdt, 8)}
                    color={
                      stats.wear_amount_usdt == null
                        ? undefined
                        : parseFloat(stats.wear_amount_usdt) >= 0
                        ? "text-emerald-400"
                        : "text-red-400"
                    }
                  />
                  <Stat
                    label="Wear Ratio (bps)"
                    value={stats.wear_ratio_bps != null ? `${stats.wear_ratio_bps} bps` : "—"}
                    color={
                      stats.wear_ratio_bps == null
                        ? undefined
                        : stats.wear_ratio_bps >= 0
                        ? "text-emerald-400"
                        : "text-red-400"
                    }
                  />
                </div>
                <div className="text-xs text-neutral-600 mt-3">
                  Wear = current - baseline；负数 = 亏损；ratio 单位 bps (万分之一)
                </div>
                {stats.base_holding_qty && parseFloat(stats.base_holding_qty) > 0.1 && (
                  <div className="mt-2 text-xs text-yellow-400/80 bg-yellow-500/5 border border-yellow-500/20 rounded p-2">
                    ⚠ 当前还持有刷的币: {stats.base_holding_qty}（约{" "}
                    {fmtUsdt(stats.base_holding_valuation_usdt, 4)} USDT 价值）
                    —— 这部分 wear 数字会随币价波动；任务结束时会自动清仓。
                  </div>
                )}
              </div>

              {/* fills 计数 + 持仓 */}
              <div className="bg-neutral-900 border border-neutral-800 rounded-lg p-4">
                <div className="grid grid-cols-4 gap-4">
                  <Stat label="State" value={stats.state} />
                  <Stat label="Strategy" value={stats.strategy} />
                  <Stat label="Fill 数" value={String(stats.fill_count)} />
                  <Stat
                    label="刷的币持仓"
                    value={fmtUsdt(stats.base_holding_qty, 2)}
                    color={
                      stats.base_holding_qty &&
                      parseFloat(stats.base_holding_qty) > 0.1
                        ? "text-yellow-400"
                        : "text-emerald-400"
                    }
                  />
                </div>
              </div>

              {/* V2 oto_smart 决策分布 + 胜率 */}
              {stats.rounds && stats.rounds.total > 0 && (
                <RoundsCard rounds={stats.rounds} />
              )}

              {/* 人脸/手机验证截图（V2.fix3 auto-pause 后扫码续刷用） */}
              <FaceVerifyPanel username={stats.username} defaultSymbol={stats.symbol} />

              {/* 累积曲线 */}
              <JobChart jobId={selectedId} target={stats.target_volume_usdt} />

              {/* 实时订单事件 */}
              <OrderEvents username={stats.username} />
            </>
          )}
          {statsError && (
            <div className="text-sm text-red-400 p-3 bg-red-500/10 border border-red-500/30 rounded">
              {statsError}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function Stat({
  label,
  value,
  color,
}: {
  label: string;
  value: string;
  color?: string;
}) {
  return (
    <div>
      <div className="text-xs text-neutral-500">{label}</div>
      <div className={`font-mono text-sm mt-1 ${color ?? "text-neutral-200"}`}>{value}</div>
    </div>
  );
}

/// 人脸/手机验证截图面板
/// 场景：V2.fix3 auto-pause 后，账户被币安风控要求额外验证，需要扫二维码完成。
/// 流程：填 symbol+amount → 点"触发" → 后端 Playwright 下小单触发风控弹窗 →
/// 点击"手机验证" → 截图二维码 dialog → 这里显示图片让你用手机币安 App 扫
function FaceVerifyPanel({
  username,
  defaultSymbol,
}: {
  username: string;
  defaultSymbol: string;
}) {
  // 把 "ALPHA_971USDT" 退化成 base "NEX" 友好名（实际 trigger 时直接传 alpha_id 即可）
  const [symbol, setSymbol] = useState<string>(() => defaultSymbol.replace(/USDT$/, ""));
  const [amount, setAmount] = useState<string>("10");
  const [session, setSession] = useState<FaceSession | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  // 二维码图片 URL 用 nonce 控制刷新（每次成功后递增，强制重新拉）
  const [imgNonce, setImgNonce] = useState(0);

  // 启动时拉一次当前 status（如果之前已经截过图，立刻显示）
  useEffect(() => {
    let alive = true;
    void getFaceStatus(username).then((s) => {
      if (!alive) return;
      setSession(s);
      if (s.screenshot_available) setImgNonce((n) => n + 1);
    });
    return () => {
      alive = false;
    };
  }, [username]);

  const handleTrigger = async () => {
    setSubmitting(true);
    setErr(null);
    try {
      const amt = parseFloat(amount);
      if (!Number.isFinite(amt) || amt <= 0) throw new Error("amount 必须 > 0");
      const s = await triggerFace(username, symbol.trim(), amt);
      setSession(s);
      if (s.screenshot_available) setImgNonce((n) => n + 1);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setSubmitting(false);
    }
  };

  const statusColor: Record<string, string> = {
    idle: "bg-neutral-700/30 text-neutral-300",
    running: "bg-yellow-500/20 text-yellow-300",
    no_dialog: "bg-blue-500/20 text-blue-300",
    dialog_no_phone: "bg-orange-500/20 text-orange-300",
    captured: "bg-emerald-500/20 text-emerald-300",
    failed: "bg-red-500/20 text-red-300",
  };
  const sc = session
    ? statusColor[session.status] ?? "bg-neutral-700/30"
    : "bg-neutral-700/30";

  return (
    <div className="bg-neutral-900 border border-neutral-800 rounded-lg p-4">
      <div className="text-sm text-neutral-400 mb-3 flex items-center gap-2">
        <span>人脸 / 手机验证</span>
        {session && (
          <span className={`px-1.5 py-0.5 rounded text-[10px] ${sc}`}>
            {session.status}
          </span>
        )}
      </div>

      <div className="grid grid-cols-1 md:grid-cols-4 gap-2 mb-3">
        <input
          value={symbol}
          onChange={(e) => setSymbol(e.target.value)}
          placeholder="symbol (NEX / ALPHA_971)"
          className="bg-neutral-950 border border-neutral-800 rounded px-3 py-2 text-sm outline-none focus:border-neutral-600"
        />
        <input
          value={amount}
          onChange={(e) => setAmount(e.target.value)}
          placeholder="金额 USDT"
          className="bg-neutral-950 border border-neutral-800 rounded px-3 py-2 text-sm outline-none focus:border-neutral-600"
        />
        <button
          onClick={handleTrigger}
          disabled={submitting || !symbol.trim()}
          className="md:col-span-2 bg-yellow-500 hover:bg-yellow-400 disabled:opacity-50 text-black font-medium rounded px-3 py-2 text-sm"
        >
          {submitting ? "正在触发（约 30-60 秒）…" : "触发验证截图"}
        </button>
      </div>

      {err && (
        <div className="text-sm text-red-400 mb-3 p-2 bg-red-500/10 border border-red-500/30 rounded">
          {err}
        </div>
      )}

      {session?.message && (
        <div className="text-xs text-neutral-500 mb-3">→ {session.message}</div>
      )}

      {session?.screenshot_available ? (
        <div className="border border-neutral-800 rounded p-2 bg-neutral-950">
          <div className="text-xs text-neutral-500 mb-2">
            用手机币安 App 扫这个二维码完成验证，完成后回到任务列表点
            <span className="text-emerald-400 mx-1">「启动 / 继续」</span>
            续刷
          </div>
          <img
            src={`${faceQrUrl(username)}&n=${imgNonce}`}
            alt="face verify QR"
            className="max-w-xs rounded"
          />
        </div>
      ) : (
        <div className="text-xs text-neutral-600 italic">
          {session?.status === "no_dialog"
            ? "本次没触发到验证弹窗（账号目前不需要风控）"
            : "尚无截图。账号被 auto-pause 后点上面的按钮触发"}
        </div>
      )}

      <div className="text-xs text-neutral-700 mt-3">
        实现：Playwright 用账号 user_data_dir 打开 alpha 页面 → page.evaluate 调 oto
        /place 下一笔小单 → 触发风控 → 检测 「安全验证」弹窗 → 点击「手机验证」→ 截 dialog。
      </div>
    </div>
  );
}

/// V2 oto_smart 决策分布 + 胜率展示
function RoundsCard({ rounds }: { rounds: RoundStats }) {
  const sumPnl = parseFloat(rounds.sum_pnl_usdt);
  const pnlColor = !Number.isFinite(sumPnl)
    ? "text-neutral-300"
    : sumPnl > 0
    ? "text-emerald-400"
    : sumPnl < 0
    ? "text-red-400"
    : "text-neutral-300";
  // 按出现次数倒序的决策类型表
  const decisionRows = Object.entries(rounds.decision_counts).sort(
    (a, b) => b[1] - a[1]
  );
  return (
    <div className="bg-neutral-900 border border-neutral-800 rounded-lg p-4">
      <div className="text-sm text-neutral-400 mb-3">
        Rounds (V2 决策矩阵) · 共 {rounds.total} 轮
      </div>
      <div className="grid grid-cols-4 gap-4 mb-4">
        <Stat label="Filled" value={String(rounds.filled)} color="text-emerald-400" />
        <Stat label="Skipped" value={String(rounds.skipped)} color="text-yellow-400" />
        <Stat label="Failed" value={String(rounds.failed)} color="text-red-400" />
        <Stat
          label="Win Rate"
          value={
            rounds.win_rate_pct == null
              ? "—"
              : `${rounds.win_rate_pct.toFixed(1)}%`
          }
        />
      </div>
      <div className="grid grid-cols-4 gap-4 mb-4">
        <Stat label="Win" value={String(rounds.win)} color="text-emerald-400" />
        <Stat label="Loss" value={String(rounds.loss)} color="text-red-400" />
        <Stat label="Flat" value={String(rounds.flat)} />
        <Stat
          label="Σ round.pnl"
          value={`${sumPnl.toFixed(4)} USDT`}
          color={pnlColor}
        />
      </div>
      {decisionRows.length > 0 && (
        <div>
          <div className="text-xs text-neutral-500 mb-2">
            决策分布（含 _working_timeout / _pending_timeout 后缀）
          </div>
          <div className="space-y-1">
            {decisionRows.map(([name, cnt]) => {
              const pct = rounds.total > 0 ? (cnt / rounds.total) * 100 : 0;
              const isTimeout = name.endsWith("_timeout");
              return (
                <div key={name} className="flex items-center gap-2 text-xs">
                  <span
                    className={`font-mono w-56 truncate ${
                      isTimeout ? "text-yellow-400" : "text-neutral-300"
                    }`}
                    title={name}
                  >
                    {name}
                  </span>
                  <div className="flex-1 h-2 bg-neutral-950 rounded overflow-hidden">
                    <div
                      className={`h-full ${
                        isTimeout ? "bg-yellow-500/60" : "bg-emerald-500/60"
                      }`}
                      style={{ width: `${pct}%` }}
                    />
                  </div>
                  <span className="font-mono text-neutral-400 w-12 text-right">
                    {cnt}
                  </span>
                  <span className="font-mono text-neutral-500 w-12 text-right">
                    {pct.toFixed(1)}%
                  </span>
                </div>
              );
            })}
          </div>
        </div>
      )}
      <div className="text-xs text-neutral-600 mt-3">
        注意：round.pnl = sell − buy（单轮内），不算 sweep；Σ round.pnl ≠ Wear。
        真实经济效应看上面 Wear 卡片。
      </div>
    </div>
  );
}
