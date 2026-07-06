-- v2 maker 策略(NEX 等窄深盘口省磨损:卖腿挂单赚点差+等45s)种入 strategies 表
-- (jobs.strategy 是 FK,不种入建 job 会报 FOREIGN KEY 787)
INSERT INTO strategies (name, version, params_schema) VALUES
    ('oto_smart_maker', 'v2-maker',
     '{"type":"object","properties":{"single_min_usdt":{"type":"string"},"single_max_usdt":{"type":"string"}}}')
ON CONFLICT(name) DO NOTHING;
