import { useEffect, useState } from "react";
import { jobTimeseries, type TimePoint } from "../api/trade";

interface Props {
  jobId: string | null;
  /** target_volume from stats，画参考线 */
  target?: string;
  /** 自动刷新间隔（ms），默认 3000 */
  refreshMs?: number;
}

/**
 * 纯 SVG 累积曲线图：
 * - 黄色实线：cum_buy_volume（已刷量）
 * - 黄色虚线：target 参考线
 * - 紫色实线：cum_pnl_realized（卖出收入 - 买入花费，正=赚）
 */
export default function JobChart({ jobId, target, refreshMs = 3000 }: Props) {
  const [points, setPoints] = useState<TimePoint[]>([]);
  const [hover, setHover] = useState<TimePoint | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!jobId) {
      setPoints([]);
      return;
    }
    let cancelled = false;
    const load = async () => {
      try {
        const r = await jobTimeseries(jobId);
        if (!cancelled) {
          setPoints(r.points);
          setError(null);
        }
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      }
    };
    void load();
    const t = window.setInterval(load, refreshMs);
    return () => {
      cancelled = true;
      window.clearInterval(t);
    };
  }, [jobId, refreshMs]);

  const W = 720;
  const H = 220;
  const padL = 56;
  const padR = 12;
  const padT = 10;
  const padB = 28;
  const innerW = W - padL - padR;
  const innerH = H - padT - padB;

  if (!jobId) return null;

  if (points.length === 0) {
    return (
      <div className="bg-neutral-900 border border-neutral-800 rounded-lg p-4">
        <div className="text-sm text-neutral-400 mb-2">累积曲线</div>
        {error ? (
          <div className="text-sm text-red-400">{error}</div>
        ) : (
          <div className="text-sm text-neutral-600 py-12 text-center">等待第一笔成交…</div>
        )}
      </div>
    );
  }

  // 横轴：从第一个点 ts 到当前 (now)
  const t0 = points[0].ts_ms;
  const t1 = Math.max(points[points.length - 1].ts_ms, Date.now());
  const tSpan = Math.max(1, t1 - t0);

  // 纵轴：取 cum_buy / target 中较大者 + cum_sell + |pnl| 范围
  const targetN = target ? parseFloat(target) : 0;
  const cumBuyMax = parseFloat(points[points.length - 1].cum_buy_volume) || 0;
  const cumSellMax = parseFloat(points[points.length - 1].cum_sell_value) || 0;
  const pnlAbsMax = Math.max(
    ...points.map((p) => Math.abs(parseFloat(p.cum_pnl_realized) || 0))
  );
  const ymax = Math.max(cumBuyMax, cumSellMax, targetN, pnlAbsMax * 2) || 1;
  // 为 PnL 留对称空间：上下各取 ymax/2 的位置作中线
  // 但 cum_buy/sell 是 ≥ 0 的累积，PnL 才会负
  // 简化：左 Y 轴 = 累积金额 (0..ymax)；PnL 用另一种缩放叠加同坐标
  const yScale = (v: number) => padT + innerH - (v / ymax) * innerH;
  const xScale = (t: number) => padL + ((t - t0) / tSpan) * innerW;

  // 构造 path（cum_buy）
  const buyPath = points
    .map((p, i) => {
      const x = xScale(p.ts_ms);
      const y = yScale(parseFloat(p.cum_buy_volume) || 0);
      return `${i === 0 ? "M" : "L"} ${x.toFixed(1)} ${y.toFixed(1)}`;
    })
    .join(" ");
  const sellPath = points
    .map((p, i) => {
      const x = xScale(p.ts_ms);
      const y = yScale(parseFloat(p.cum_sell_value) || 0);
      return `${i === 0 ? "M" : "L"} ${x.toFixed(1)} ${y.toFixed(1)}`;
    })
    .join(" ");
  const pnlPath = points
    .map((p, i) => {
      const x = xScale(p.ts_ms);
      // PnL 在 [-pnlAbsMax, pnlAbsMax] 之间，映射到 y 轴整个范围（中心 = ymax/2）
      const v = parseFloat(p.cum_pnl_realized) || 0;
      const norm = pnlAbsMax > 0 ? v / pnlAbsMax : 0; // -1..1
      const y = padT + innerH / 2 - (norm * innerH) / 2;
      return `${i === 0 ? "M" : "L"} ${x.toFixed(1)} ${y.toFixed(1)}`;
    })
    .join(" ");

  // target 参考横线
  const targetY = targetN > 0 ? yScale(targetN) : null;

  const handleMove = (e: React.MouseEvent<SVGSVGElement>) => {
    const svg = e.currentTarget;
    const rect = svg.getBoundingClientRect();
    const px = e.clientX - rect.left;
    const t = t0 + ((px - padL) / innerW) * tSpan;
    let best = points[0];
    let bestD = Infinity;
    for (const p of points) {
      const d = Math.abs(p.ts_ms - t);
      if (d < bestD) {
        bestD = d;
        best = p;
      }
    }
    setHover(best);
  };

  return (
    <div className="bg-neutral-900 border border-neutral-800 rounded-lg p-4">
      <div className="flex items-center justify-between mb-2">
        <div className="text-sm text-neutral-400">累积曲线 · {points.length} 个 fill</div>
        <div className="flex items-center gap-3 text-xs">
          <span className="flex items-center gap-1">
            <span className="inline-block w-3 h-0.5 bg-yellow-400" />
            <span className="text-neutral-400">买入累积</span>
          </span>
          <span className="flex items-center gap-1">
            <span className="inline-block w-3 h-0.5 bg-sky-400" />
            <span className="text-neutral-400">卖出累积</span>
          </span>
          <span className="flex items-center gap-1">
            <span className="inline-block w-3 h-0.5 bg-fuchsia-400" />
            <span className="text-neutral-400">已实现 P&L</span>
          </span>
        </div>
      </div>

      <svg
        viewBox={`0 0 ${W} ${H}`}
        className="w-full"
        onMouseMove={handleMove}
        onMouseLeave={() => setHover(null)}
      >
        {/* 网格 + 中线 */}
        <line x1={padL} y1={padT + innerH / 2} x2={W - padR} y2={padT + innerH / 2} stroke="#262626" strokeDasharray="3 3" />
        <line x1={padL} y1={padT} x2={padL} y2={padT + innerH} stroke="#404040" />
        <line x1={padL} y1={padT + innerH} x2={W - padR} y2={padT + innerH} stroke="#404040" />

        {/* 0 / max 标签 */}
        <text x={padL - 6} y={padT + innerH + 4} fill="#737373" fontSize="10" textAnchor="end">0</text>
        <text x={padL - 6} y={padT + 4} fill="#737373" fontSize="10" textAnchor="end">{ymax.toFixed(2)}</text>

        {/* target 参考线 */}
        {targetY !== null && (
          <>
            <line x1={padL} y1={targetY} x2={W - padR} y2={targetY} stroke="#facc15" strokeDasharray="4 4" strokeOpacity="0.5" />
            <text x={W - padR - 4} y={targetY - 4} fill="#facc15" fontSize="10" textAnchor="end" opacity="0.8">
              target {targetN}
            </text>
          </>
        )}

        {/* 三条曲线 */}
        <path d={buyPath} fill="none" stroke="#facc15" strokeWidth="1.5" />
        <path d={sellPath} fill="none" stroke="#38bdf8" strokeWidth="1.5" />
        <path d={pnlPath} fill="none" stroke="#e879f9" strokeWidth="1.5" />

        {/* hover 竖线 */}
        {hover && (
          <line
            x1={xScale(hover.ts_ms)}
            y1={padT}
            x2={xScale(hover.ts_ms)}
            y2={padT + innerH}
            stroke="#525252"
            strokeDasharray="2 2"
          />
        )}
      </svg>

      {hover && (
        <div className="mt-2 text-xs font-mono text-neutral-400 border-t border-neutral-800 pt-2">
          <span className="text-neutral-500">{new Date(hover.ts_ms).toLocaleTimeString()}</span>
          {"  "}
          <span className={hover.side === "BUY" ? "text-yellow-400" : "text-sky-400"}>
            {hover.side}
          </span>{" "}
          {parseFloat(hover.quote_qty).toFixed(5)} USDT
          {"  ·  "}
          cum buy <span className="text-yellow-400">{parseFloat(hover.cum_buy_volume).toFixed(4)}</span>
          {"  cum sell "}<span className="text-sky-400">{parseFloat(hover.cum_sell_value).toFixed(4)}</span>
          {"  pnl "}
          <span
            className={
              parseFloat(hover.cum_pnl_realized) >= 0 ? "text-emerald-400" : "text-red-400"
            }
          >
            {parseFloat(hover.cum_pnl_realized).toFixed(5)}
          </span>
        </div>
      )}
    </div>
  );
}
