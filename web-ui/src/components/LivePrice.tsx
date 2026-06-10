import { useEffect, useRef, useState } from "react";
import { StreamClient, type AggTradeMsg, type MarketEvent } from "../api/ws";

interface Tick {
  t: number;
  price: string;
  qty: string;
  m: boolean;
}

interface Props {
  symbol: string; // 例: ALPHA_971USDT
  maxTicks?: number;
}

export default function LivePrice({ symbol, maxTicks = 12 }: Props) {
  const [status, setStatus] = useState<"connecting" | "open" | "closed">("connecting");
  const [last, setLast] = useState<AggTradeMsg | null>(null);
  const [ticks, setTicks] = useState<Tick[]>([]);
  const prevPriceRef = useRef<string | null>(null);
  const [dir, setDir] = useState<"up" | "down" | "flat">("flat");

  useEffect(() => {
    const client = new StreamClient({
      onStatusChange: setStatus,
      onEvent: (e: MarketEvent) => {
        if (e.type !== "market") return;
        const d = e.data as AggTradeMsg;
        if (d.e !== "aggTrade") return;
        if (d.s !== symbol) return;
        setLast(d);
        setTicks((xs) => {
          const next: Tick = { t: d.T, price: d.p, qty: d.q, m: d.m };
          return [next, ...xs].slice(0, maxTicks);
        });
        const prev = prevPriceRef.current;
        if (prev !== null) {
          const a = parseFloat(prev);
          const b = parseFloat(d.p);
          setDir(b > a ? "up" : b < a ? "down" : "flat");
        }
        prevPriceRef.current = d.p;
      },
    });
    client.start();
    return () => client.stop();
  }, [symbol, maxTicks]);

  const statusColor =
    status === "open"
      ? "bg-emerald-500"
      : status === "connecting"
      ? "bg-yellow-500"
      : "bg-red-500";

  const priceColor =
    dir === "up" ? "text-emerald-400" : dir === "down" ? "text-red-400" : "text-neutral-200";

  return (
    <div className="bg-neutral-900 border border-neutral-800 rounded-lg p-4">
      <div className="flex items-center justify-between mb-3">
        <div className="flex items-center gap-2">
          <span className={`inline-block w-2 h-2 rounded-full ${statusColor}`} />
          <span className="text-sm text-neutral-400">实时 · {symbol}</span>
        </div>
        <span className="text-xs text-neutral-500">{status}</span>
      </div>

      <div className="mb-4">
        <div className={`text-3xl font-mono tabular-nums ${priceColor}`}>
          {last ? last.p : "—"}
        </div>
        {last && (
          <div className="text-xs text-neutral-500 mt-1">
            qty {last.q} · {last.m ? "卖方主动" : "买方主动"} ·{" "}
            {new Date(last.T).toLocaleTimeString()}
          </div>
        )}
      </div>

      <div className="text-xs text-neutral-500 mb-1">最近成交</div>
      <div className="font-mono text-xs space-y-0.5 max-h-48 overflow-auto">
        {ticks.length === 0 && (
          <div className="text-neutral-600">等待第一条成交…</div>
        )}
        {ticks.map((t) => (
          <div key={`${t.t}-${t.price}-${t.qty}`} className="flex items-center justify-between gap-2">
            <span className={t.m ? "text-red-400" : "text-emerald-400"}>{t.price}</span>
            <span className="text-neutral-500 truncate">{t.qty}</span>
            <span className="text-neutral-600 text-[10px]">
              {new Date(t.t).toLocaleTimeString().slice(-8)}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}
