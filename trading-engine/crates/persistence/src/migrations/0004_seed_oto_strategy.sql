-- 加 oto + simple_round 策略到 strategies 表（jobs.strategy 是 FK）
INSERT INTO strategies (name, version, params_schema) VALUES
    ('oto', 'v1-fast',
     '{"type":"object","properties":{"single_min_usdt":{"type":"string"},"single_max_usdt":{"type":"string"}}}')
ON CONFLICT(name) DO NOTHING;

INSERT INTO strategies (name, version, params_schema) VALUES
    ('simple_round', 'v1',
     '{"type":"object","properties":{"single_min_usdt":{"type":"string"},"single_max_usdt":{"type":"string"}}}')
ON CONFLICT(name) DO NOTHING;
