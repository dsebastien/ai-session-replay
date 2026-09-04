CREATE VIRTUAL TABLE session_search USING fts5(
    session_id UNINDEXED,
    search_key UNINDEXED,
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
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER sessions_delete_search
AFTER DELETE ON sessions
BEGIN
    DELETE FROM session_search WHERE session_id = OLD.id;
END;

INSERT INTO session_search (
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

INSERT INTO session_search (
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
