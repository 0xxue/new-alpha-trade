-- 种入默认策略（jobs.strategy 是 FK，没行会导致 trade/start 失败）
INSERT INTO strategies (name, version, params_schema) VALUES
    ('adaptive_maker', '0.1-placeholder',
     '{"type":"object","properties":{"slippage":{"type":"string"},"max_hold_ms":{"type":"integer"}}}')
ON CONFLICT(name) DO NOTHING;
