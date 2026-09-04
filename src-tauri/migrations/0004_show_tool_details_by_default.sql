UPDATE session_preferences
SET show_tool_details = 1
WHERE show_tool_calls = 1
  AND show_tool_details = 0;
