-- 发布版本携带插件包元数据（从包根 index.ts 静态提取）。
-- 存原始 JSON（字符串），便于以后扩展字段而不频繁改表结构；
-- 必填字段（name/description/author/version）在发布时已经校验过。
ALTER TABLE releases ADD COLUMN metadata_json TEXT NOT NULL DEFAULT '{}';
