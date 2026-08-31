-- 任务列表「一键清空」用：隐藏而不是删除。
-- 不删的原因：trades/orders/rounds 都以 job_id 关联（单机就有 90 万+ 条成交），
-- 真删会把历史盈亏一并抹掉。隐藏后列表干净，数据还在。
ALTER TABLE jobs ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;
