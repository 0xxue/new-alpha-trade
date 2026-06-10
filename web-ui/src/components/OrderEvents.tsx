import { useEffect, useState } from "react";
import { StreamClient, type MarketEvent } from "../api/ws";

interface UserEvent {
  ts: number;
  user: string;
  inner_stream: string;
  data: unknown;
}

interface Props {
  /** 只关心某个用户的事件；为空 = 全部 */
  username?: string | null;
  maxRows?: number;
}

export default function OrderEvents({ username, maxRows = 20 }: Props) {
  const [events, setEvents] = useState<UserEvent[]>([]);
  const [status, setStatus] = useState<"connecting" | "open" | "closed">("connecting");

  useEffect(() => {
    const client = new StreamClient({
      onStatusChange: setStatus,
      onEvent: (e: MarketEvent) => {
        if (e.type !== "market") return;
        // user 流名格式：`user:<username>|<inner stream>`
        if (!e.stream.startsWith("user:")) return;
        const rest = e.stream.slice(5);
        const sep = rest.indexOf("|");
        const u = sep > 0 ? rest.slice(0, sep) : rest;
        const inner = sep > 0 ? rest.slice(sep + 1) : "";
        if (username && u !== username) return;
        setEvents((xs) => {
          const next: UserEvent = {
            ts: Date.now(),
            user: u,
            inner_stream: inner,
            data: e.data,
          };
          return [next, ...xs].slice(0, maxRows);
        });
      },
    });
    client.start();
    return () => client.stop();
  }, [username, maxRows]);

  const statusDot =
    status === "open"
      ? "bg-emerald-500"
      : status === "connecting"
      ? "bg-yellow-500"
      : "bg-red-500";

  return (
    <div className="bg-neutral-900 border border-neutral-800 rounded-lg p-4">
      <div className="flex items-center justify-between mb-3">
        <div className="flex items-center gap-2">
          <span className={`inline-block w-2 h-2 rounded-full ${statusDot}`} />
          <span className="text-sm text-neutral-400">
            实时订单事件{username ? ` · ${username}` : ""}
          </span>
        </div>
        <span className="text-xs text-neutral-500">{events.length} 条</span>
      </div>

      {events.length === 0 && (
        <div className="text-sm text-neutral-600 py-6 text-center">
          等待事件…（下单/成交/撤单时这里会冒出来）
        </div>
      )}

      <div className="font-mono text-xs space-y-1 max-h-64 overflow-auto">
        {events.map((e, i) => {
          const d = e.data as Record<string, unknown>;
          const evtType = typeof d?.e === "string" ? d.e : e.inner_stream;
          return (
            <div
              key={`${e.ts}-${i}`}
              className="border border-neutral-800 rounded px-2 py-1.5 bg-neutral-950/40"
            >
              <div className="flex items-center justify-between text-[10px] text-neutral-500">
                <span>{e.user}</span>
                <span>{new Date(e.ts).toLocaleTimeString()}</span>
              </div>
              <div className="text-neutral-300 truncate" title={JSON.stringify(d)}>
                <span className="text-yellow-400">{evtType}</span>{" "}
                {JSON.stringify(d).slice(0, 200)}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
