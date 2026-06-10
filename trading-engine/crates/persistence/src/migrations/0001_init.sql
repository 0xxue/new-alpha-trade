-- new-alpha-trade 初始 schema（设计文档 §4.4）
-- 所有时间戳用 UTC ISO8601；金额/价格用 TEXT 存（Decimal 字符串），不要 REAL。

PRAGMA foreign_keys = ON;

CREATE TABLE accounts (
    username       TEXT PRIMARY KEY,
    cookies_json   TEXT    NOT NULL,
    headers_json   TEXT    NOT NULL,
    twofa_secret   TEXT,
    last_refresh   TEXT,
    status         TEXT    NOT NULL DEFAULT 'active'
);

CREATE TABLE strategies (
    name           TEXT PRIMARY KEY,
    version        TEXT    NOT NULL,
    params_schema  TEXT    NOT NULL
);

CREATE TABLE jobs (
    id             TEXT PRIMARY KEY,
    username       TEXT    NOT NULL REFERENCES accounts(username),
    symbol         TEXT    NOT NULL,
    strategy       TEXT    NOT NULL REFERENCES strategies(name),
    params_json    TEXT    NOT NULL,
    target_volume  TEXT    NOT NULL,
    state          TEXT    NOT NULL DEFAULT 'pending',
    created_at     TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at     TEXT    NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX idx_jobs_username_state ON jobs(username, state);

CREATE TABLE trades (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id         TEXT    NOT NULL REFERENCES jobs(id),
    cycle_no       INTEGER NOT NULL,
    buy_order_id   TEXT,
    sell_order_id  TEXT,
    buy_price      TEXT,
    sell_price     TEXT,
    quantity       TEXT,
    pnl            TEXT,
    wear_ratio     TEXT,
    ts_buy         TEXT,
    ts_sell        TEXT
);
CREATE INDEX idx_trades_job ON trades(job_id);

CREATE TABLE orders (
    order_id       TEXT PRIMARY KEY,
    job_id         TEXT    NOT NULL REFERENCES jobs(id),
    side           TEXT    NOT NULL,
    price          TEXT    NOT NULL,
    qty            TEXT    NOT NULL,
    status         TEXT    NOT NULL,
    raw_response   TEXT,
    ts             TEXT    NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX idx_orders_job_status ON orders(job_id, status);
