-- 0020__category_tag_owner.sql
-- Categories and tags used to be global: any authenticated user could rename or
-- delete them for everybody (and deleting a category cascades to the
-- recurrences and fee rules that use it). They now belong to the user who
-- created them.
--
-- `owner_id` NULL means "shared": the two seeded fallback categories and every
-- row that existed before this migration. Shared rows stay visible to all and
-- can only be changed by an administrator.
--
-- Dialect-neutral (SQLite + MariaDB): only ADD COLUMN, CREATE TABLE, INSERT ..
-- SELECT, DROP TABLE and RENAME are used. The category `key` stays globally
-- unique; owned categories get an owner suffix in it (see the repository).

ALTER TABLE categories ADD COLUMN owner_id VARCHAR(64) NULL;
CREATE INDEX idx_categories_owner ON categories (owner_id);

-- Tags: the label used to be UNIQUE for the whole installation, which would
-- stop two users from having a tag with the same name. A unique index cannot
-- be dropped in a way both engines accept, so the two tag tables are rebuilt
-- (the data is copied unchanged, owner_id NULL).
CREATE TABLE tags_v2 (
    id          VARCHAR(64)  NOT NULL PRIMARY KEY,
    label       VARCHAR(64)  NOT NULL,
    description VARCHAR(500) NOT NULL DEFAULT '',
    color       VARCHAR(16)  NOT NULL DEFAULT '',
    created_at  VARCHAR(64)  NOT NULL,
    owner_id    VARCHAR(64)  NULL
);

INSERT INTO tags_v2 (id, label, description, color, created_at, owner_id)
SELECT id, label, description, color, created_at, NULL FROM tags;

CREATE TABLE transaction_tags_v2 (
    transaction_id VARCHAR(64) NOT NULL,
    tag_id         VARCHAR(64) NOT NULL,
    PRIMARY KEY (transaction_id, tag_id),
    FOREIGN KEY (transaction_id) REFERENCES transactions (id) ON DELETE CASCADE,
    FOREIGN KEY (tag_id) REFERENCES tags_v2 (id) ON DELETE CASCADE
);

INSERT INTO transaction_tags_v2 (transaction_id, tag_id)
SELECT transaction_id, tag_id FROM transaction_tags;

DROP TABLE transaction_tags;
DROP TABLE tags;

ALTER TABLE tags_v2 RENAME TO tags;
ALTER TABLE transaction_tags_v2 RENAME TO transaction_tags;

CREATE UNIQUE INDEX idx_tags_owner_label ON tags (owner_id, label);
CREATE INDEX idx_tags_owner ON tags (owner_id);
CREATE INDEX idx_transaction_tags_tag ON transaction_tags (tag_id);
