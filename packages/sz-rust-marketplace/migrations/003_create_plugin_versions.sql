-- 插件版本表
CREATE TABLE IF NOT EXISTS plugin_versions (
    id            BIGSERIAL PRIMARY KEY,
    plugin_id     BIGINT NOT NULL,
    version       VARCHAR(64) NOT NULL,
    archive_key   VARCHAR(512) NOT NULL,
    sha256        CHAR(64) NOT NULL,
    signature     TEXT NOT NULL DEFAULT '',
    review_status VARCHAR(16) NOT NULL DEFAULT 'pending'
                    CHECK (review_status IN ('pending', 'approved', 'rejected')),
    changelog     TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    FOREIGN KEY (plugin_id) REFERENCES plugins(id) ON DELETE CASCADE,
    UNIQUE (plugin_id, version)
);

CREATE INDEX idx_versions_plugin ON plugin_versions(plugin_id);
CREATE INDEX idx_versions_status ON plugin_versions(review_status);