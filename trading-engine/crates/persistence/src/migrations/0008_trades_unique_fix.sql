-- V2.tune19: trades.UNIQUE(fill_id, symbol) 太严格 — Binance 同一 fill_id 返回多笔 sub-fill
-- (不同 qty/quote_qty)，旧约束把后续 sub-fill 全 IGNORE 掉，DB 漏 fills → wear 算错。
--
-- 实测案例（order 9359557, fill_id=577052）：Binance 返回 3 笔 (8M+220k+220k NEX, $38.49)，
-- DB 只入了第一笔 (8M, $36.50) → 漏 $2 / 单 order。1000+ order 累积漏 $20+。
--
-- 修法：复合 UNIQUE 加上 qty + quote_qty，相同 fill_id 但不同金额视为不同 sub-fill。
-- SQLite 不支持 DROP CONSTRAINT，得 rebuild table.
-- 注：sqlx migrate 已自动包 transaction，本文件不要自己 BEGIN/COMMIT。

CREATE TABLE trades_new (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    fill_id         TEXT    NOT NULL,
    order_id        TEXT    NOT NULL,
    job_id          TEXT,
    username        TEXT    NOT NULL,
    symbol          TEXT    NOT NULL,
    side            TEXT    NOT NULL,
    price           TEXT    NOT NULL,
    qty             TEXT    NOT NULL,
    quote_qty       TEXT    NOT NULL,
    commission      TEXT    NOT NULL,
    commission_asset TEXT   NOT NULL,
    trade_ts_ms     INTEGER NOT NULL,
    raw_json        TEXT,
    created_at      TEXT    NOT NULL DEFAULT (datetime('now')),
    UNIQUE (fill_id, symbol, qty, quote_qty)
);

INSERT INTO trades_new (
    id, fill_id, order_id, job_id, username, symbol, side, price, qty, quote_qty,
    commission, commission_asset, trade_ts_ms, raw_json, created_at
)
SELECT
    id, fill_id, order_id, job_id, username, symbol, side, price, qty, quote_qty,
    commission, commission_asset, trade_ts_ms, raw_json, created_at
FROM trades;

DROP TABLE trades;
ALTER TABLE trades_new RENAME TO trades;

CREATE INDEX idx_trades_username_symbol ON trades(username, symbol);
CREATE INDEX idx_trades_job ON trades(job_id);
CREATE INDEX idx_trades_order ON trades(order_id);
CREATE INDEX idx_trades_side_username ON trades(side, username);
