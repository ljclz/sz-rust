-- 安装记录表
CREATE TABLE IF NOT EXISTS install_records (
    id           BIGSERIAL PRIMARY KEY,
    plugin_id    BIGINT NOT NULL,
    version_id   BIGINT NOT NULL,
    installer_id BIGINT NOT NULL,
    installed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    FOREIGN KEY (plugin_id) REFERENCES plugins(id) ON DELETE CASCADE,
    FOREIGN KEY (version_id) REFERENCES plugin_versions(id) ON DELETE CASCADE,
    FOREIGN KEY (installer_id) REFERENCES developers(id) ON DELETE RESTRICT
);

CREATE INDEX idx_installs_installer ON install_records(installer_id);