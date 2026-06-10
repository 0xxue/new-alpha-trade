-- 服务器元信息（到期日 / 购买日等小 key-value 配置）
CREATE TABLE IF NOT EXISTS server_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- 种子：5/21 买的，30 天周期 → 6/20 到期
INSERT OR IGNORE INTO server_meta (key, value) VALUES
    ('purchased_at', '2026-05-21'),
    ('expires_at',   '2026-06-20');
