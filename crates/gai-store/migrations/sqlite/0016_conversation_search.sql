-- Full-text search over conversations: the text blocks of each message,
-- kept in step with `messages` by triggers (deletes include cascades from
-- deleted sessions).
CREATE VIRTUAL TABLE message_search USING fts5(
    text,
    session_id UNINDEXED,
    seq UNINDEXED,
    tokenize = 'porter unicode61'
);

CREATE TRIGGER messages_search_insert AFTER INSERT ON messages WHEN new.kind = 'message' BEGIN
    INSERT INTO message_search (text, session_id, seq)
    SELECT group_concat(json_extract(j.value, '$.text'), ' '), new.session_id, new.seq
    FROM json_each(new.body, '$.content') j
    WHERE json_extract(j.value, '$.type') = 'text'
    HAVING COUNT(*) > 0;
END;

CREATE TRIGGER messages_search_delete AFTER DELETE ON messages WHEN old.kind = 'message' BEGIN
    DELETE FROM message_search WHERE session_id = old.session_id AND seq = old.seq;
END;

INSERT INTO message_search (text, session_id, seq)
SELECT group_concat(json_extract(j.value, '$.text'), ' '), m.session_id, m.seq
FROM messages m, json_each(m.body, '$.content') j
WHERE m.kind = 'message' AND json_extract(j.value, '$.type') = 'text'
GROUP BY m.session_id, m.seq;
