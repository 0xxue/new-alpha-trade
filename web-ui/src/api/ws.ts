// trading-engine WebSocket 客户端。
//
// Nginx 反代：ws://host/ws/stream → trading-engine :7002/ws/stream
// Dev server vite 代理同样路径。

export interface AggTradeMsg {
  e: "aggTrade";
  E: number;
  T: number;
  s: string; // symbol
  p: string; // price
  q: string; // qty
  m: boolean; // buyer is maker (=true 卖方主动)
  a?: number;
}

export interface DepthUpdateMsg {
  e: "depthUpdate";
  E: number;
  s: string;
  U: number;
  u: number;
  pu?: number;
  b: [string, string][];
  a: [string, string][];
}

export type MarketEvent =
  | { type: "hello"; service: string; version: string }
  | { type: "market"; stream: string; data: AggTradeMsg | DepthUpdateMsg | unknown };

export interface WsClientOpts {
  onEvent: (e: MarketEvent) => void;
  onStatusChange?: (status: "connecting" | "open" | "closed") => void;
}

export class StreamClient {
  private ws: WebSocket | null = null;
  private closedByUser = false;
  private retry = 0;

  constructor(private readonly opts: WsClientOpts) {}

  start(): void {
    this.closedByUser = false;
    this.connect();
  }

  stop(): void {
    this.closedByUser = true;
    this.ws?.close();
    this.ws = null;
  }

  private connect(): void {
    this.opts.onStatusChange?.("connecting");
    // 当前页协议决定 ws/wss
    const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
    const url = `${proto}//${window.location.host}/ws/stream`;
    const ws = new WebSocket(url);
    this.ws = ws;

    ws.onopen = () => {
      this.retry = 0;
      this.opts.onStatusChange?.("open");
    };

    ws.onmessage = (ev) => {
      try {
        const msg = JSON.parse(ev.data) as MarketEvent;
        this.opts.onEvent(msg);
      } catch {
        /* ignore malformed */
      }
    };

    ws.onclose = () => {
      this.opts.onStatusChange?.("closed");
      if (!this.closedByUser) {
        const delay = Math.min(1000 * 2 ** this.retry, 30000);
        this.retry += 1;
        setTimeout(() => this.connect(), delay);
      }
    };

    ws.onerror = () => {
      // close 会跟着触发，retry 在那里走
    };
  }
}
