-- Full-text search over conversations: the text blocks of each message.
ALTER TABLE messages ADD COLUMN tsv tsvector GENERATED ALWAYS AS (
    CASE WHEN kind = 'message' THEN
        to_tsvector('english', jsonb_path_query_array(body::jsonb, '$.content[*] ? (@.type == "text").text')::text)
    END
) STORED;
CREATE INDEX messages_tsv ON messages USING GIN (tsv);
