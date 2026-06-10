-- 改造 trades 表：从"按 cycle 一行"改成"按 fill 一行"
-- 旧设计（0001）是给磨损 cycle 统计用的，但实际更细的 fill 数据才是权威来源
-- 一个 order_id 可能有多个 fills（部分成交）
--
-- 顺便：原 jobs.username 是 NOT NULL + FK；
-- 手动下单时塞一个占位 username 没问题，但 jobs 这边是 P4 才严格用
-- 暂不改 jobs schema（FK 静默失败的 chip 后面单独处理）

-- SQLite 不支持 DROP COLUMN，索性新建 trades_v2 + 旧 trades 留作历史
DROP TABLE IF EXISTS trades;

CREATE TABLE trades (
    -- 主键：复合 fill 标识。币安给的 tradeId 在不同 symbol 上可能重复，加 symbol 一起
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    fill_id         TEXT    NOT NULL,            -- 币安 user-trades 里的 id 字段
    order_id        TEXT    NOT NULL,
    job_id          TEXT,                        -- 可空（手动下单时为 null）
    username        TEXT    NOT NULL,
    symbol          TEXT    NOT NULL,
    side            TEXT    NOT NULL,            -- BUY / SELL
    price           TEXT    NOT NULL,            -- Decimal 字符串
    qty             TEXT    NOT NULL,
    quote_qty       TEXT    NOT NULL,            -- 币安给的成交额（USDT），volume 累加用这个
    commission      TEXT    NOT NULL,
    commission_asset TEXT   NOT NULL,            -- ALPHA_xxx / USDT / BNB
    trade_ts_ms     INTEGER NOT NULL,            -- 币安的 time 字段（毫秒）
    raw_json        TEXT,                        -- 完整 fill JSON 留底
    created_at      TEXT    NOT NULL DEFAULT (datetime('now')),
    UNIQUE (fill_id, symbol)                     -- 重复抓取时幂等
);
CREATE INDEX idx_trades_username_symbol ON trades(username, symbol);
CREATE INDEX idx_trades_job ON trades(job_id);
CREATE INDEX idx_trades_order ON trades(order_id);
CREATE INDEX idx_trades_side_username ON trades(side, username);
