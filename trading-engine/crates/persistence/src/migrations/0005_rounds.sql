-- v2 策略：每轮一行，显式记录 working/pending 关系，方便算胜率
-- 字段
--   id              自增
--   job_id          FK 到 jobs.id
--   round_no        本 job 内的轮号 (1..N)
--   decision_type   策略决策类型 (skip_bearish/double_maker/taker_maker_hybrid/fast/...)
--   status          skipped / filled / partial / failed
--   working_order_id  本轮 BUY 单 ID（可空，skip 时为 null）
--   pending_order_id  本轮 SELL 单 ID
--   buy_quote_qty   本轮 BUY 成交额 USDT (Decimal 字符串)
--   sell_quote_qty  本轮 SELL 成交额 USDT
--   pnl_usdt        = sell - buy（不含手续费近似）
--   commission_usdt 估算手续费总值（USDT 等价）
--   started_ms      轮开始时间
--   ended_ms        轮结束时间

CREATE TABLE rounds (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id            TEXT    NOT NULL,
    round_no          INTEGER NOT NULL,
    decision_type     TEXT    NOT NULL,
    status            TEXT    NOT NULL,
    working_order_id  TEXT,
    pending_order_id  TEXT,
    buy_quote_qty     TEXT,
    sell_quote_qty    TEXT,
    pnl_usdt          TEXT,
    commission_usdt   TEXT,
    started_ms        INTEGER NOT NULL,
    ended_ms          INTEGER,
    UNIQUE (job_id, round_no)
);
CREATE INDEX idx_rounds_job ON rounds(job_id);
CREATE INDEX idx_rounds_decision ON rounds(decision_type);
