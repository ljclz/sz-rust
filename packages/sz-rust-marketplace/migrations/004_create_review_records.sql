-- 审核记录表（append-only）
CREATE TABLE IF NOT EXISTS review_records (
    id         BIGSERIAL PRIMARY KEY,
    version_id BIGINT NOT NULL,
    reviewer_id BIGINT NOT NULL,
    decision   VARCHAR(16) NOT NULL CHECK (decision IN ('approve', 'reject')),
    comment    TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    FOREIGN KEY (version_id) REFERENCES plugin_versions(id) ON DELETE CASCADE,
    FOREIGN KEY (reviewer_id) REFERENCES developers(id) ON DELETE RESTRICT
);

CREATE INDEX idx_reviews_version ON review_records(version_id);