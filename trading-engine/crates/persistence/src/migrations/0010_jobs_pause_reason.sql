-- 给 jobs 表加「暂停原因」，让前端能区分自动暂停(风控/人脸验证)与手动暂停。
-- NULL = 手动暂停或从未暂停；非空 = 引擎自动暂停时写入的原因文案。
ALTER TABLE jobs ADD COLUMN pause_reason TEXT;
