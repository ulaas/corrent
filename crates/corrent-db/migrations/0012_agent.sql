-- Changes AI agents (MCP clients) propose; a person approves or rejects each one.
CREATE TABLE agent_proposals (
    id         INTEGER PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    tool       TEXT NOT NULL,
    input      TEXT NOT NULL,               -- the tool's arguments (JSON)
    summary    TEXT NOT NULL,
    status     TEXT NOT NULL DEFAULT 'pending',  -- pending | applied | failed | rejected
    decided_at INTEGER,
    result     TEXT
);
CREATE INDEX agent_proposals_status ON agent_proposals(status, id);
