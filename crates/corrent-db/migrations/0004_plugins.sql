-- Plugins: whether each is on, its settings (secrets included; never sent to clients), and
-- each plugin's own key-value storage.
CREATE TABLE plugins (
    id           TEXT PRIMARY KEY,
    enabled      INTEGER NOT NULL DEFAULT 1,
    settings     TEXT NOT NULL DEFAULT '{}',
    installed_at INTEGER NOT NULL
);

CREATE TABLE plugin_kv (
    plugin TEXT NOT NULL,
    key    TEXT NOT NULL,
    value  TEXT NOT NULL,
    PRIMARY KEY (plugin, key)
);
