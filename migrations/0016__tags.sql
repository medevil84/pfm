-- 0016__tags.sql
-- Free-form tags (label + description) that can be attached, many per
-- transaction, to identify arbitrary groups of transactions.

CREATE TABLE IF NOT EXISTS tags (
    id          VARCHAR(64)  NOT NULL PRIMARY KEY,
    label       VARCHAR(64)  NOT NULL,
    description VARCHAR(500) NOT NULL DEFAULT '',
    color       VARCHAR(16)  NOT NULL DEFAULT '',
    created_at  VARCHAR(64)  NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_tags_label ON tags (label);

CREATE TABLE IF NOT EXISTS transaction_tags (
    transaction_id VARCHAR(64) NOT NULL,
    tag_id         VARCHAR(64) NOT NULL,
    PRIMARY KEY (transaction_id, tag_id),
    FOREIGN KEY (transaction_id) REFERENCES transactions (id) ON DELETE CASCADE,
    FOREIGN KEY (tag_id) REFERENCES tags (id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_transaction_tags_tag ON transaction_tags (tag_id);
