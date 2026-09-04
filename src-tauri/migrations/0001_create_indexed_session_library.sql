CREATE TABLE sessions (
    id TEXT PRIMARY KEY
        CHECK(length(id) BETWEEN 1 AND 256 AND instr(id, char(0)) = 0 AND id NOT GLOB '*[^A-Za-z0-9_-]*'),
    source TEXT NOT NULL
        CHECK(source IN ('claude-code', 'codex', 'copilot-cli', 'vscode-copilot')),
    vendor_session_id TEXT NOT NULL
        CHECK(length(vendor_session_id) BETWEEN 1 AND 1024 AND instr(vendor_session_id, char(0)) = 0),
    title TEXT NOT NULL
        CHECK(length(title) BETWEEN 1 AND 512 AND instr(title, char(0)) = 0),
    created_at_ms INTEGER
        CHECK(created_at_ms IS NULL OR created_at_ms BETWEEN 0 AND 9007199254740991),
    source_version TEXT
        CHECK(source_version IS NULL OR (length(source_version) <= 256 AND instr(source_version, char(0)) = 0)),
    collection TEXT
        CHECK(collection IS NULL OR (length(collection) <= 512 AND instr(collection, char(0)) = 0)),
    display_filename TEXT
        CHECK(display_filename IS NULL OR (length(display_filename) <= 512 AND instr(display_filename, char(0)) = 0)),
    current_revision_id TEXT
        CHECK(current_revision_id IS NULL OR (length(current_revision_id) BETWEEN 1 AND 256 AND instr(current_revision_id, char(0)) = 0 AND current_revision_id NOT GLOB '*[^A-Za-z0-9_-]*')),
    source_present INTEGER NOT NULL CHECK(source_present IN (0, 1)),
    first_indexed_at_ms INTEGER NOT NULL CHECK(first_indexed_at_ms BETWEEN 0 AND 9007199254740991),
    last_seen_at_ms INTEGER NOT NULL CHECK(last_seen_at_ms BETWEEN 0 AND 9007199254740991),
    UNIQUE(source, vendor_session_id),
    FOREIGN KEY(id, current_revision_id)
        REFERENCES session_revisions(session_id, id)
        DEFERRABLE INITIALLY DEFERRED
) STRICT;

CREATE TABLE scan_runs (
    generation INTEGER PRIMARY KEY CHECK(generation BETWEEN 1 AND 9007199254740991),
    status TEXT NOT NULL CHECK(status IN ('running', 'completed', 'failed')),
    started_at_ms INTEGER NOT NULL CHECK(started_at_ms BETWEEN 0 AND 9007199254740991),
    completed_at_ms INTEGER
        CHECK(completed_at_ms IS NULL OR completed_at_ms BETWEEN started_at_ms AND 9007199254740991),
    discovered_count INTEGER NOT NULL CHECK(discovered_count BETWEEN 0 AND 100000),
    processed_count INTEGER NOT NULL CHECK(processed_count BETWEEN 0 AND discovered_count),
    indexed_count INTEGER NOT NULL CHECK(indexed_count BETWEEN 0 AND processed_count),
    unchanged_count INTEGER NOT NULL CHECK(unchanged_count BETWEEN 0 AND processed_count),
    failed_count INTEGER NOT NULL CHECK(failed_count BETWEEN 0 AND processed_count),
    error_code TEXT
        CHECK(error_code IS NULL OR (
            length(error_code) BETWEEN 1 AND 128
            AND instr(error_code, char(0)) = 0
            AND substr(error_code, 1, 1) GLOB '[A-Z]'
            AND error_code NOT GLOB '*[^A-Z0-9_]*'
        )),
    CHECK(indexed_count + unchanged_count + failed_count <= processed_count),
    CHECK((status = 'running' AND completed_at_ms IS NULL AND error_code IS NULL)
        OR (status = 'completed' AND completed_at_ms IS NOT NULL AND error_code IS NULL)
        OR (status = 'failed' AND completed_at_ms IS NOT NULL AND error_code IS NOT NULL))
) STRICT;

CREATE TABLE source_locations (
    session_id TEXT PRIMARY KEY
        REFERENCES sessions(id) ON DELETE CASCADE,
    canonical_path_utf16le BLOB NOT NULL
        CHECK(length(canonical_path_utf16le) BETWEEN 2 AND 65536 AND length(canonical_path_utf16le) % 2 = 0),
    source_identity_hash BLOB NOT NULL UNIQUE CHECK(length(source_identity_hash) = 32),
    root_identity BLOB NOT NULL CHECK(length(root_identity) BETWEEN 1 AND 512),
    file_identity BLOB NOT NULL CHECK(length(file_identity) BETWEEN 1 AND 512),
    file_size_bytes INTEGER NOT NULL CHECK(file_size_bytes BETWEEN 0 AND 9007199254740991),
    modified_at_ms INTEGER NOT NULL CHECK(modified_at_ms BETWEEN 0 AND 9007199254740991),
    last_seen_generation INTEGER
        REFERENCES scan_runs(generation) ON DELETE SET NULL
) STRICT;

CREATE TABLE session_revisions (
    id TEXT PRIMARY KEY
        CHECK(length(id) BETWEEN 1 AND 256 AND instr(id, char(0)) = 0 AND id NOT GLOB '*[^A-Za-z0-9_-]*'),
    session_id TEXT NOT NULL
        REFERENCES sessions(id) ON DELETE CASCADE,
    content_hash BLOB NOT NULL CHECK(length(content_hash) = 32),
    indexed_at_ms INTEGER NOT NULL CHECK(indexed_at_ms BETWEEN 0 AND 9007199254740991),
    source_created_at_ms INTEGER
        CHECK(source_created_at_ms IS NULL OR source_created_at_ms BETWEEN 0 AND 9007199254740991),
    source_updated_at_ms INTEGER
        CHECK(source_updated_at_ms IS NULL OR source_updated_at_ms BETWEEN 0 AND 9007199254740991),
    entry_count INTEGER NOT NULL CHECK(entry_count BETWEEN 1 AND 100000),
    duration_ms INTEGER NOT NULL CHECK(duration_ms BETWEEN 0 AND 604800000),
    diagnostic_count INTEGER NOT NULL CHECK(diagnostic_count BETWEEN 0 AND 1000),
    reasoning_availability TEXT NOT NULL CHECK(reasoning_availability IN ('available', 'unavailable')),
    tool_detail_availability TEXT NOT NULL CHECK(tool_detail_availability IN ('available', 'partial', 'unavailable')),
    UNIQUE(session_id, content_hash),
    UNIQUE(session_id, id)
) STRICT;

CREATE TABLE revision_entries (
    revision_id TEXT NOT NULL
        REFERENCES session_revisions(id) ON DELETE CASCADE,
    entry_key TEXT NOT NULL
        CHECK(length(entry_key) BETWEEN 1 AND 256 AND instr(entry_key, char(0)) = 0 AND entry_key NOT GLOB '*[^A-Za-z0-9_-]*'),
    ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 99999),
    at_ms INTEGER NOT NULL CHECK(at_ms BETWEEN 0 AND 604800000),
    kind TEXT NOT NULL CHECK(kind IN ('user', 'assistant', 'reasoning', 'tool-call', 'file-change', 'unknown')),
    body_text TEXT
        CHECK(body_text IS NULL OR (length(body_text) <= 2000000 AND instr(body_text, char(0)) = 0)),
    tool_name TEXT
        CHECK(tool_name IS NULL OR (length(tool_name) BETWEEN 1 AND 256 AND instr(tool_name, char(0)) = 0)),
    tool_status TEXT
        CHECK(tool_status IS NULL OR tool_status IN ('pending', 'running', 'succeeded', 'failed')),
    summary_text TEXT
        CHECK(summary_text IS NULL OR (length(summary_text) <= 2000000 AND instr(summary_text, char(0)) = 0)),
    tool_detail_availability TEXT
        CHECK(tool_detail_availability IS NULL OR tool_detail_availability IN ('available', 'unavailable')),
    tool_arguments TEXT
        CHECK(tool_arguments IS NULL OR (length(tool_arguments) <= 2000000 AND instr(tool_arguments, char(0)) = 0)),
    tool_result TEXT
        CHECK(tool_result IS NULL OR (length(tool_result) <= 2000000 AND instr(tool_result, char(0)) = 0)),
    display_path TEXT
        CHECK(display_path IS NULL OR (length(display_path) BETWEEN 1 AND 32768 AND instr(display_path, char(0)) = 0)),
    source_type TEXT
        CHECK(source_type IS NULL OR (length(source_type) BETWEEN 1 AND 256 AND instr(source_type, char(0)) = 0)),
    PRIMARY KEY(revision_id, entry_key),
    UNIQUE(revision_id, ordinal),
    CHECK(
        (kind IN ('user', 'assistant', 'reasoning')
            AND body_text IS NOT NULL
            AND tool_name IS NULL AND tool_status IS NULL AND summary_text IS NULL
            AND tool_detail_availability IS NULL AND tool_arguments IS NULL AND tool_result IS NULL
            AND display_path IS NULL AND source_type IS NULL)
        OR
        (kind = 'tool-call'
            AND body_text IS NULL
            AND tool_name IS NOT NULL AND tool_status IS NOT NULL AND summary_text IS NOT NULL
            AND tool_detail_availability IS NOT NULL
            AND (
                (tool_detail_availability = 'available'
                    AND (tool_arguments IS NOT NULL OR tool_result IS NOT NULL))
                OR
                (tool_detail_availability = 'unavailable'
                    AND tool_arguments IS NULL AND tool_result IS NULL)
            )
            AND display_path IS NULL AND source_type IS NULL)
        OR
        (kind = 'file-change'
            AND body_text IS NULL
            AND tool_name IS NULL AND tool_status IS NULL AND summary_text IS NOT NULL
            AND tool_detail_availability IS NULL AND tool_arguments IS NULL AND tool_result IS NULL
            AND display_path IS NOT NULL AND source_type IS NULL)
        OR
        (kind = 'unknown'
            AND body_text IS NULL
            AND tool_name IS NULL AND tool_status IS NULL AND summary_text IS NULL
            AND tool_detail_availability IS NULL AND tool_arguments IS NULL AND tool_result IS NULL
            AND display_path IS NULL AND source_type IS NOT NULL)
    )
) STRICT;

CREATE TABLE entry_selection_overrides (
    session_id TEXT NOT NULL
        REFERENCES sessions(id) ON DELETE CASCADE,
    stable_entry_key TEXT NOT NULL
        CHECK(length(stable_entry_key) BETWEEN 1 AND 256 AND instr(stable_entry_key, char(0)) = 0 AND stable_entry_key NOT GLOB '*[^A-Za-z0-9_-]*'),
    selected INTEGER NOT NULL CHECK(selected IN (0, 1)),
    updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms BETWEEN 0 AND 9007199254740991),
    PRIMARY KEY(session_id, stable_entry_key)
) STRICT;

CREATE TABLE session_preferences (
    session_id TEXT PRIMARY KEY
        REFERENCES sessions(id) ON DELETE CASCADE,
    show_tool_calls INTEGER NOT NULL CHECK(show_tool_calls IN (0, 1)),
    show_tool_details INTEGER NOT NULL CHECK(show_tool_details IN (0, 1)),
    show_reasoning INTEGER NOT NULL CHECK(show_reasoning IN (0, 1)),
    entry_delay_ms INTEGER NOT NULL CHECK(entry_delay_ms BETWEEN 250 AND 10000),
    playback_speed REAL NOT NULL CHECK(playback_speed BETWEEN 0.25 AND 4.0),
    background_color TEXT NOT NULL CHECK(length(background_color) = 7 AND instr(background_color, char(0)) = 0 AND substr(background_color, 1, 1) = '#' AND substr(background_color, 2) NOT GLOB '*[^0-9A-Fa-f]*'),
    surface_color TEXT NOT NULL CHECK(length(surface_color) = 7 AND instr(surface_color, char(0)) = 0 AND substr(surface_color, 1, 1) = '#' AND substr(surface_color, 2) NOT GLOB '*[^0-9A-Fa-f]*'),
    text_color TEXT NOT NULL CHECK(length(text_color) = 7 AND instr(text_color, char(0)) = 0 AND substr(text_color, 1, 1) = '#' AND substr(text_color, 2) NOT GLOB '*[^0-9A-Fa-f]*'),
    muted_color TEXT NOT NULL CHECK(length(muted_color) = 7 AND instr(muted_color, char(0)) = 0 AND substr(muted_color, 1, 1) = '#' AND substr(muted_color, 2) NOT GLOB '*[^0-9A-Fa-f]*'),
    accent_color TEXT NOT NULL CHECK(length(accent_color) = 7 AND instr(accent_color, char(0)) = 0 AND substr(accent_color, 1, 1) = '#' AND substr(accent_color, 2) NOT GLOB '*[^0-9A-Fa-f]*'),
    success_color TEXT NOT NULL CHECK(length(success_color) = 7 AND instr(success_color, char(0)) = 0 AND substr(success_color, 1, 1) = '#' AND substr(success_color, 2) NOT GLOB '*[^0-9A-Fa-f]*'),
    error_color TEXT NOT NULL CHECK(length(error_color) = 7 AND instr(error_color, char(0)) = 0 AND substr(error_color, 1, 1) = '#' AND substr(error_color, 2) NOT GLOB '*[^0-9A-Fa-f]*'),
    font_family TEXT NOT NULL CHECK(font_family = 'JetBrains Mono'),
    font_size_px INTEGER NOT NULL CHECK(font_size_px BETWEEN 12 AND 96),
    line_height REAL NOT NULL CHECK(line_height BETWEEN 1.0 AND 2.5),
    updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms BETWEEN 0 AND 9007199254740991),
    CHECK(show_tool_details <= show_tool_calls)
) STRICT;

CREATE TABLE source_diagnostics (
    id INTEGER PRIMARY KEY,
    scan_generation INTEGER NOT NULL
        REFERENCES scan_runs(generation) ON DELETE CASCADE,
    session_id TEXT
        REFERENCES sessions(id) ON DELETE SET NULL,
    source TEXT NOT NULL CHECK(source IN ('claude-code', 'codex', 'copilot-cli', 'vscode-copilot')),
    code TEXT NOT NULL CHECK(
        length(code) BETWEEN 1 AND 128
        AND instr(code, char(0)) = 0
        AND substr(code, 1, 1) GLOB '[A-Z]'
        AND code NOT GLOB '*[^A-Z0-9_]*'
    ),
    occurrence_count INTEGER NOT NULL CHECK(occurrence_count BETWEEN 1 AND 1000),
    recorded_at_ms INTEGER NOT NULL CHECK(recorded_at_ms BETWEEN 0 AND 9007199254740991)
) STRICT;

CREATE TABLE suppressed_sources (
    id TEXT PRIMARY KEY
        CHECK(length(id) BETWEEN 1 AND 256 AND instr(id, char(0)) = 0 AND id NOT GLOB '*[^A-Za-z0-9_-]*'),
    source TEXT NOT NULL CHECK(source IN ('claude-code', 'codex', 'copilot-cli', 'vscode-copilot')),
    source_identity_hash BLOB NOT NULL UNIQUE CHECK(length(source_identity_hash) = 32),
    vendor_session_id TEXT NOT NULL
        CHECK(length(vendor_session_id) BETWEEN 1 AND 1024 AND instr(vendor_session_id, char(0)) = 0),
    source_deleted INTEGER NOT NULL CHECK(source_deleted IN (0, 1)),
    suppressed_at_ms INTEGER NOT NULL CHECK(suppressed_at_ms BETWEEN 0 AND 9007199254740991)
) STRICT;

CREATE INDEX session_revisions_by_session_and_time
    ON session_revisions(session_id, indexed_at_ms DESC);
CREATE INDEX revision_entries_by_revision_and_ordinal
    ON revision_entries(revision_id, ordinal);
CREATE INDEX sessions_by_last_seen
    ON sessions(last_seen_at_ms DESC, id);
CREATE INDEX source_diagnostics_by_scan
    ON source_diagnostics(scan_generation, id);
