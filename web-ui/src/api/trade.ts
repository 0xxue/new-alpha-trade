// trading-engine 调用封装

const base = "/api";

async function jsonOrThrow<T>(resp: Response): Promise<T> {
  if (!resp.ok) {
    let detail = `HTTP ${resp.status}`;
    try {
      const body = await resp.json();
      if (body?.error) detail = String(body.error);
    } catch {
      /* ignore */
    }
    throw new Error(detail);
  }
  return resp.json() as Promise<T>;
}

export interface JobRow {
  id: string;
  username: string;
  symbol: string;
  strategy: string;
  params: unknown;
  target_volume: string;
  state: string;
  created_at: string;
  updated_at: string;
}

/// V2 oto_smart 才会填；其他 strategy 这块缺失或全 0
export interface RoundStats {
  total: number;
  filled: number;
  skipped: number;
  failed: number;
  win: number;
  loss: number;
  flat: number;
  win_rate_pct: number | null;
  sum_pnl_usdt: string;
  /// 决策类型 → 出现次数（含 _working_timeout / _pending_timeout 后缀）
  decision_counts: Record<string, number>;
}

export interface JobStats {
  job_id: string;
  username: string;
  symbol: string;
  strategy: string;
  state: string;
  buy_volume_usdt: string;
  target_volume_usdt: string;
  progress_bps: number;
  fill_count: number;
  baseline_spot_usdt: string | null;
  current_spot_usdt: string | null;
  wear_amount_usdt: string | null;
  wear_ratio_bps: number | null;
  base_holding_qty: string | null;
  base_holding_valuation_usdt: string | null;
  /// V2 oto_smart 才有
  rounds?: RoundStats;
}

export async function listJobs(): Promise<JobRow[]> {
  return jsonOrThrow(await fetch(`${base}/trade/jobs`));
}

export async function jobStats(jobId: string): Promise<JobStats> {
  return jsonOrThrow(await fetch(`${base}/trade/stats/${jobId}`));
}

export async function startJob(req: {
  username: string;
  symbol: string;
  target_volume: string;
  single_min_usdt?: string;
  single_max_usdt?: string;
  strategy?: string;
  params?: unknown;
}): Promise<{ job_id: string; state: string }> {
  return jsonOrThrow(
    await fetch(`${base}/trade/start`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(req),
    })
  );
}

export interface TokenInfo {
  symbol: string;
  alpha_id: string;
  pair_symbol: string;
  name: string;
  chain_id: string;
  contract_address: string;
  trade_decimal: number;
  tradable: boolean;
}

export async function listTokens(): Promise<{ count: number; tokens: TokenInfo[] }> {
  return jsonOrThrow(await fetch(`${base}/tokens`));
}

export interface TimePoint {
  ts_ms: number;
  side: string;
  quote_qty: string;
  cum_buy_volume: string;
  cum_sell_value: string;
  cum_pnl_realized: string;
  fill_count: number;
}

export async function jobTimeseries(
  jobId: string
): Promise<{ job_id: string; target_volume: string; points: TimePoint[] }> {
  return jsonOrThrow(await fetch(`${base}/trade/timeseries/${jobId}`));
}

export async function pauseJob(jobId: string) {
  return jsonOrThrow(await fetch(`${base}/trade/pause/${jobId}`, { method: "POST" }));
}
export async function resumeJob(jobId: string) {
  return jsonOrThrow(await fetch(`${base}/trade/resume/${jobId}`, { method: "POST" }));
}
export async function stopJob(jobId: string) {
  return jsonOrThrow(await fetch(`${base}/trade/stop/${jobId}`, { method: "POST" }));
}
