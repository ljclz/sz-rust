-- 插件表
CREATE TABLE IF NOT EXISTS plugins (
    id           BIGSERIAL PRIMARY KEY,
    name         VARCHAR(255) NOT NULL UNIQUE,
    identifier   VARCHAR(255) NOT NULL UNIQUE,
    title        VARCHAR(255) NOT NULL,
    author       VARCHAR(255) NOT NULL,
    homepage     TEXT,
    license      VARCHAR(64) NOT NULL DEFAULT 'Apache-2.0',
    description  TEXT,
    tags         TEXT[] NOT NULL DEFAULT '{}',
    price        DOUBLE PRECISION NOT NULL DEFAULT 0.0 CHECK (price >= 0),
    developer_id BIGINT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    FOREIGN KEY (developer_id) REFERENCES developers(id) ON DELETE RESTRICT
);

CREATE INDEX idx_plugins_tags ON plugins USING GIN (tags);
CREATE INDEX idx_plugins_developer ON plugins(developer_id);