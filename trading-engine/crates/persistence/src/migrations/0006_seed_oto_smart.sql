-- v2 smart 策略种入 strategies 表（jobs.strategy 是 FK）
INSERT INTO strategies (name, version, params_schema) VALUES
    ('oto_smart', 'v2',
     '{"type":"object","properties":{"single_min_usdt":{"type":"string"},"single_max_usdt":{"type":"string"}}}')
ON CONFLICT(name) DO NOTHING;
