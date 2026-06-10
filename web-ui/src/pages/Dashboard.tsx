import { useEffect, useState } from "react";
import LivePrice from "../components/LivePrice";

type HealthState = "loading" | "ok" | "fail";

export default function Dashboard() {
  const [engineState, setEngineState] = useState<HealthState>("loading");
  const [qrState, setQrState] = useState<HealthState>("loading");

  useEffect(() => {
    const ping = async (url: string, set: (s: HealthState) => void) => {
      try {
        const r = await fetch(url);
        set(r.ok ? "ok" : "fail");
      } catch {
        set("fail");
      }
    };
    void ping("/api/health", setEngineState);
    void ping("/api/qr/health", setQrState);
  }, []);

  return (
    <div>
      <h1 className="text-2xl font-semibold mb-6">Dashboard</h1>

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-4 max-w-4xl">
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          <ServiceCard name="trading-engine (:7002)" state={engineState} />
          <ServiceCard name="qr-service (:7001)" state={qrState} />
        </div>
        <LivePrice symbol="ALPHA_971USDT" />
      </div>

      <p className="mt-8 text-sm text-neutral-500">
        P3 阶段 — 实时行情已接通。下一步 P4 接策略下单。
      </p>
    </div>
  );
}

function ServiceCard({ name, state }: { name: string; state: HealthState }) {
  const color =
    state === "ok"
      ? "text-emerald-400"
      : state === "fail"
      ? "text-red-400"
      : "text-neutral-500";
  const dot =
    state === "ok" ? "bg-emerald-400" : state === "fail" ? "bg-red-400" : "bg-neutral-500";
  return (
    <div className="bg-neutral-900 border border-neutral-800 rounded-lg p-4">
      <div className="text-sm text-neutral-400 mb-2">{name}</div>
      <div className={`flex items-center gap-2 ${color}`}>
        <span className={`inline-block w-2 h-2 rounded-full ${dot}`} />
        <span className="text-sm">{state}</span>
      </div>
    </div>
  );
}
