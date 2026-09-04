DROP TRIGGER sessions_delete_search;
DROP TABLE session_search;

CREATE TABLE session_search_documents (
    id INTEGER PRIMARY KEY,
    session_id TEXT NOT NULL
        REFERENCES sessions(id) ON DELETE CASCADE,
    search_key TEXT NOT NULL
        CHECK(length(search_key) BETWEEN 1 AND 256 AND instr(search_key, char(0)) = 0 AND search_key NOT GLOB '*[^A-Za-z0-9_-]*'),
    source TEXT
        CHECK(source IS NULL OR source IN ('claude code', 'codex cli', 'copilot cli', 'visual studio code copilot')),
    title TEXT
        CHECK(title IS NULL OR (length(title) BETWEEN 1 AND 512 AND instr(title, char(0)) = 0)),
    body_text TEXT
        CHECK(body_text IS NULL OR (length(body_text) <= 2000000 AND instr(body_text, char(0)) = 0)),
    tool_name TEXT
        CHECK(tool_name IS NULL OR (length(tool_name) BETWEEN 1 AND 256 AND instr(tool_name, char(0)) = 0)),
    tool_status TEXT
        CHECK(tool_status IS NULL OR tool_status IN ('pending', 'running', 'succeeded', 'failed')),
    summary_text TEXT
        CHECK(summary_text IS NULL OR (length(summary_text) <= 2000000 AND instr(summary_text, char(0)) = 0)),
    tool_arguments TEXT
        CHECK(tool_arguments IS NULL OR (length(tool_arguments) <= 2000000 AND instr(tool_arguments, char(0)) = 0)),
    tool_result TEXT
        CHECK(tool_result IS NULL OR (length(tool_result) <= 2000000 AND instr(tool_result, char(0)) = 0)),
    display_path TEXT
        CHECK(display_path IS NULL OR (length(display_path) BETWEEN 1 AND 32768 AND instr(display_path, char(0)) = 0)),
    source_type TEXT
        CHECK(source_type IS NULL OR (length(source_type) BETWEEN 1 AND 256 AND instr(source_type, char(0)) = 0)),
    UNIQUE(session_id, search_key)
) STRICT;

CREATE INDEX session_search_documents_by_session
    ON session_search_documents(session_id);

CREATE VIRTUAL TABLE session_search USING fts5(
    source,
    title,
    body_text,
    tool_name,
    tool_status,
    summary_text,
    tool_arguments,
    tool_result,
    display_path,
    source_type,
    content = 'session_search_documents',
    content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER session_search_documents_insert
AFTER INSERT ON session_search_documents
BEGIN
    INSERT INTO session_search (
        rowid, source, title, body_text, tool_name, tool_status, summary_text,
        tool_arguments, tool_result, display_path, source_type
    ) VALUES (
        NEW.id, NEW.source, NEW.title, NEW.body_text, NEW.tool_name, NEW.tool_status,
        NEW.summary_text, NEW.tool_arguments, NEW.tool_result, NEW.display_path,
        NEW.source_type
    );
END;

CREATE TRIGGER session_search_documents_delete
AFTER DELETE ON session_search_documents
BEGIN
    INSERT INTO session_search (
        session_search, rowid, source, title, body_text, tool_name, tool_status,
        summary_text, tool_arguments, tool_result, display_path, source_type
    ) VALUES (
        'delete', OLD.id, OLD.source, OLD.title, OLD.body_text, OLD.tool_name,
        OLD.tool_status, OLD.summary_text, OLD.tool_arguments, OLD.tool_result,
        OLD.display_path, OLD.source_type
    );
END;

INSERT INTO session_search_documents (
    session_id, search_key, source, title, body_text, tool_name, tool_status,
    summary_text, tool_arguments, tool_result, display_path, source_type
)
SELECT
    id,
    '__session_metadata__',
    CASE source
        WHEN 'claude-code' THEN 'claude code'
        WHEN 'codex' THEN 'codex cli'
        WHEN 'copilot-cli' THEN 'copilot cli'
        WHEN 'vscode-copilot' THEN 'visual studio code copilot'
    END,
    title,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL
FROM sessions;

INSERT INTO session_search_documents (
    session_id, search_key, source, title, body_text, tool_name, tool_status,
    summary_text, tool_arguments, tool_result, display_path, source_type
)
SELECT
    sessions.id,
    entries.entry_key,
    NULL,
    NULL,
    entries.body_text,
    entries.tool_name,
    entries.tool_status,
    entries.summary_text,
    entries.tool_arguments,
    entries.tool_result,
    entries.display_path,
    entries.source_type
FROM sessions
JOIN revision_entries AS entries
  ON entries.revision_id = sessions.current_revision_id;
