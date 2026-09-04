ALTER TABLE sessions
ADD COLUMN custom_title TEXT
    CHECK(custom_title IS NULL OR (
        length(custom_title) BETWEEN 1 AND 512
        AND instr(custom_title, char(0)) = 0
        AND trim(custom_title) = custom_title
    ));

CREATE TRIGGER session_search_documents_update
AFTER UPDATE OF source, title, body_text, tool_name, tool_status, summary_text,
    tool_arguments, tool_result, display_path, source_type
ON session_search_documents
BEGIN
    INSERT INTO session_search (
        session_search, rowid, source, title, body_text, tool_name, tool_status,
        summary_text, tool_arguments, tool_result, display_path, source_type
    ) VALUES (
        'delete', OLD.id, OLD.source, OLD.title, OLD.body_text, OLD.tool_name,
        OLD.tool_status, OLD.summary_text, OLD.tool_arguments, OLD.tool_result,
        OLD.display_path, OLD.source_type
    );
    INSERT INTO session_search (
        rowid, source, title, body_text, tool_name, tool_status, summary_text,
        tool_arguments, tool_result, display_path, source_type
    ) VALUES (
        NEW.id, NEW.source, NEW.title, NEW.body_text, NEW.tool_name,
        NEW.tool_status, NEW.summary_text, NEW.tool_arguments, NEW.tool_result,
        NEW.display_path, NEW.source_type
    );
END;
