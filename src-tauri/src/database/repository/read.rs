use sqlx::{QueryBuilder, Row, Sqlite};

use super::{IndexedSessionRepository, RepositoryError, source_from_name, source_name};
use crate::indexed_library::{
    AppearancePreferencesV1, ContentAvailability, ContentAvailabilityV1, ContractValidate,
    INDEXED_LIBRARY_SCHEMA_VERSION, IndexedEntryPageV1, IndexedSessionDetailV1,
    IndexedSessionListRequestV1, IndexedSessionPageV1, IndexedSessionSortOrderV1,
    IndexedSessionSourceV1, IndexedSessionSummaryV1, IndexedToolStatus, LibraryFontV1,
    LibraryThemeV1, MAX_INDEXED_PAGE_SIZE, MAX_PRESENTATION_ENTRY_COUNT, PresentationTimingV1,
    ReasoningAvailability, SelectableEntryV1, SessionEntryV1, SessionPreferencesV1,
    SessionRevisionV1, ToolDetailV1, VisibilityPreferencesV1,
};
use crate::model::FontFamily;

const SUMMARY_COLUMNS: &str =
    "s.id, s.source, coalesce(s.custom_title, s.title) AS title, s.created_at_ms, r.indexed_at_ms, s.source_present,
     r.entry_count,
     (SELECT count(*)
        FROM revision_entries selected_entry
        LEFT JOIN entry_selection_overrides selection
          ON selection.session_id = s.id
         AND selection.stable_entry_key = selected_entry.entry_key
       WHERE selected_entry.revision_id = r.id
         AND coalesce(selection.selected, 1) = 1) AS selected_entry_count,
     r.duration_ms, r.diagnostic_count, r.reasoning_availability,
     r.tool_detail_availability";
const SESSION_SORT_TIME: &str = "coalesce(s.created_at_ms, r.indexed_at_ms)";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SessionDateFilter {
    year: Option<u16>,
    month: Option<u8>,
    day: Option<u8>,
}

pub(crate) struct PresentationSource {
    pub(crate) title: String,
    pub(crate) revision_id: String,
    pub(crate) preferences: SessionPreferencesV1,
    pub(crate) entries: Vec<SelectableEntryV1>,
}

impl IndexedSessionRepository {
    /// Reads only the current immutable revision and display-safe session
    /// metadata. Every caller-supplied value remains a SQL bind parameter.
    pub(crate) async fn list_indexed_sessions(
        &self,
        request: &IndexedSessionListRequestV1,
    ) -> Result<IndexedSessionPageV1, RepositoryError> {
        request
            .validate()
            .map_err(|_| RepositoryError::InvalidInput)?;
        let trimmed_query = request.query.trim();
        let search_terms = (!trimmed_query.is_empty()).then(|| literal_search_terms(trimmed_query));
        let date_filter = parse_session_date_filter(trimmed_query);

        let cursor = match request.cursor.as_deref() {
            Some(cursor) => {
                let session_id = request
                    .sort_order
                    .session_id_from_cursor(cursor)
                    .ok_or(RepositoryError::InvalidInput)?;
                let mut cursor_query = QueryBuilder::<Sqlite>::new(format!(
                    "SELECT {SESSION_SORT_TIME} AS sort_at_ms, s.id
                       FROM sessions s
                       JOIN session_revisions r ON r.id = s.current_revision_id
                      WHERE s.id = "
                ));
                cursor_query.push_bind(session_id);
                push_summary_filters(
                    &mut cursor_query,
                    request.source.as_ref(),
                    search_terms.as_deref(),
                    date_filter.as_ref(),
                );
                Some(
                    cursor_query
                        .build()
                        .fetch_optional(&self.pool)
                        .await
                        .map_err(|_| RepositoryError::ReadFailed)?
                        .ok_or(RepositoryError::InvalidInput)
                        .and_then(|row| {
                            Ok((
                                read_u64(&row, "sort_at_ms")?,
                                row.try_get::<String, _>("id")
                                    .map_err(|_| RepositoryError::ReadFailed)?,
                            ))
                        })?,
                )
            }
            None => None,
        };

        let mut query = QueryBuilder::<Sqlite>::new("SELECT ");
        query.push(SUMMARY_COLUMNS).push(
            " FROM sessions s
              JOIN session_revisions r ON r.id = s.current_revision_id
              WHERE 1 = 1",
        );
        push_summary_filters(
            &mut query,
            request.source.as_ref(),
            search_terms.as_deref(),
            date_filter.as_ref(),
        );
        if let Some((sort_at_ms, session_id)) = cursor {
            query
                .push(" AND (")
                .push(SESSION_SORT_TIME)
                .push(match request.sort_order {
                    IndexedSessionSortOrderV1::Newest => " < ",
                    IndexedSessionSortOrderV1::Oldest => " > ",
                })
                .push_bind(to_i64(sort_at_ms)?)
                .push(" OR (")
                .push(SESSION_SORT_TIME)
                .push(" = ")
                .push_bind(to_i64(sort_at_ms)?)
                .push(" AND s.id > ")
                .push_bind(session_id)
                .push("))");
        }
        query
            .push(" ORDER BY ")
            .push(SESSION_SORT_TIME)
            .push(match request.sort_order {
                IndexedSessionSortOrderV1::Newest => " DESC, s.id ASC LIMIT ",
                IndexedSessionSortOrderV1::Oldest => " ASC, s.id ASC LIMIT ",
            })
            .push_bind(
                i64::try_from(request.page_size + 1).map_err(|_| RepositoryError::InvalidInput)?,
            );

        let rows = query
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|_| RepositoryError::ReadFailed)?;
        let has_more = rows.len() > request.page_size;
        let mut items = rows
            .iter()
            .take(request.page_size)
            .map(summary_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = has_more
            .then(|| {
                items
                    .last()
                    .map(|item| request.sort_order.cursor_for(&item.session_id))
            })
            .flatten();
        let page = IndexedSessionPageV1 {
            schema_version: INDEXED_LIBRARY_SCHEMA_VERSION,
            sort_order: request.sort_order,
            items: std::mem::take(&mut items),
            next_cursor,
        };
        page.validate().map_err(|_| RepositoryError::ReadFailed)?;
        Ok(page)
    }

    /// Uses one read transaction so summary, preferences, and entry page all
    /// refer to the same current revision if a refresh commits concurrently.
    pub(crate) async fn get_indexed_session(
        &self,
        session_id: &str,
        entry_cursor: Option<&str>,
    ) -> Result<IndexedSessionDetailV1, RepositoryError> {
        if !valid_opaque_id(session_id)
            || entry_cursor.is_some_and(|cursor| !valid_opaque_id(cursor))
        {
            return Err(RepositoryError::InvalidInput);
        }
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| RepositoryError::ReadFailed)?;

        let aggregate_sql = format!(
            "SELECT {SUMMARY_COLUMNS}, r.id AS revision_id,
                    p.show_tool_calls, p.show_tool_details, p.show_reasoning,
                    p.entry_delay_ms, p.playback_speed, p.background_color,
                    p.surface_color, p.text_color, p.muted_color, p.accent_color,
                    p.success_color, p.error_color, p.font_family, p.font_size_px,
                    p.line_height
               FROM sessions s
               JOIN session_revisions r ON r.id = s.current_revision_id
               JOIN session_preferences p ON p.session_id = s.id
              WHERE s.id = ?"
        );
        let aggregate = sqlx::query(&aggregate_sql)
            .bind(session_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| RepositoryError::ReadFailed)?
            .ok_or(RepositoryError::SessionNotFound)?;
        let summary = summary_from_row(&aggregate)?;
        let revision_id = aggregate
            .try_get::<String, _>("revision_id")
            .map_err(|_| RepositoryError::ReadFailed)?;
        let after_ordinal = match entry_cursor {
            Some(cursor) => Some(
                sqlx::query_scalar::<_, i64>(
                    "SELECT ordinal FROM revision_entries
                     WHERE revision_id = ? AND entry_key = ?",
                )
                .bind(&revision_id)
                .bind(cursor)
                .fetch_optional(&mut *transaction)
                .await
                .map_err(|_| RepositoryError::ReadFailed)?
                .ok_or(RepositoryError::InvalidInput)?,
            ),
            None => None,
        };

        let mut entries_query = QueryBuilder::<Sqlite>::new(
            "SELECT e.entry_key, e.ordinal, e.at_ms, e.kind, e.body_text,
                    e.tool_name, e.tool_status, e.summary_text,
                    e.tool_detail_availability, e.tool_arguments, e.tool_result,
                    e.display_path, e.source_type, coalesce(o.selected, 1) AS selected
               FROM revision_entries e
               LEFT JOIN entry_selection_overrides o
                 ON o.session_id = ",
        );
        entries_query
            .push_bind(session_id)
            .push(" AND o.stable_entry_key = e.entry_key WHERE e.revision_id = ")
            .push_bind(&revision_id);
        if let Some(ordinal) = after_ordinal {
            entries_query.push(" AND e.ordinal > ").push_bind(ordinal);
        }
        entries_query
            .push(" ORDER BY e.ordinal ASC LIMIT ")
            .push_bind(
                i64::try_from(MAX_INDEXED_PAGE_SIZE + 1)
                    .map_err(|_| RepositoryError::InvalidInput)?,
            );
        let entry_rows = entries_query
            .build()
            .fetch_all(&mut *transaction)
            .await
            .map_err(|_| RepositoryError::ReadFailed)?;
        let has_more = entry_rows.len() > MAX_INDEXED_PAGE_SIZE;
        let entries = entry_rows
            .iter()
            .take(MAX_INDEXED_PAGE_SIZE)
            .map(selectable_entry_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = has_more
            .then(|| entries.last().map(|item| entry_key(&item.entry).to_owned()))
            .flatten();

        let revision = SessionRevisionV1 {
            schema_version: INDEXED_LIBRARY_SCHEMA_VERSION,
            revision_id: revision_id.clone(),
            session_id: summary.session_id.clone(),
            indexed_at_ms: summary.last_indexed_at_ms,
            entry_count: summary.entry_count,
            duration_ms: summary.duration_ms,
            diagnostic_count: summary.diagnostic_count,
        };
        let preferences = preferences_from_row(&aggregate)?;
        let detail = IndexedSessionDetailV1 {
            schema_version: INDEXED_LIBRARY_SCHEMA_VERSION,
            summary,
            revision,
            entry_page: IndexedEntryPageV1 {
                schema_version: INDEXED_LIBRARY_SCHEMA_VERSION,
                session_id: session_id.to_owned(),
                revision_id,
                entries,
                next_cursor,
                total_entry_count: read_u64(&aggregate, "entry_count")?,
            },
            preferences,
        };
        detail.validate().map_err(|_| RepositoryError::ReadFailed)?;
        transaction
            .commit()
            .await
            .map_err(|_| RepositoryError::ReadFailed)?;
        Ok(detail)
    }

    /// Reads one complete presentation source through a single SQLite snapshot.
    pub(crate) async fn get_presentation_source(
        &self,
        session_id: &str,
        expected_revision_id: &str,
    ) -> Result<PresentationSource, RepositoryError> {
        if !valid_opaque_id(session_id) || !valid_opaque_id(expected_revision_id) {
            return Err(RepositoryError::InvalidInput);
        }
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| RepositoryError::ReadFailed)?;
        let aggregate = sqlx::query(
            "SELECT coalesce(s.custom_title, s.title) AS title, r.id AS revision_id, r.entry_count,
                    p.show_tool_calls, p.show_tool_details, p.show_reasoning,
                    p.entry_delay_ms, p.playback_speed, p.background_color,
                    p.surface_color, p.text_color, p.muted_color, p.accent_color,
                    p.success_color, p.error_color, p.font_family, p.font_size_px,
                    p.line_height
               FROM sessions s
               JOIN session_revisions r ON r.id = s.current_revision_id
               JOIN session_preferences p ON p.session_id = s.id
              WHERE s.id = ?",
        )
        .bind(session_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepositoryError::ReadFailed)?
        .ok_or(RepositoryError::SessionNotFound)?;
        let revision_id = string(&aggregate, "revision_id")?;
        if revision_id != expected_revision_id {
            return Err(RepositoryError::StaleRevision);
        }
        let entry_count = read_u64(&aggregate, "entry_count")?;
        let preferences = preferences_from_row(&aggregate)?;
        let title = string(&aggregate, "title")?;
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT e.entry_key, e.ordinal, e.at_ms, e.kind, e.body_text,
                    e.tool_name, e.tool_status, e.summary_text,
                    e.tool_detail_availability, e.tool_arguments, e.tool_result,
                    e.display_path, e.source_type, coalesce(o.selected, 1) AS selected
               FROM revision_entries e
               LEFT JOIN entry_selection_overrides o
                 ON o.session_id = ",
        );
        query
            .push_bind(session_id)
            .push(" AND o.stable_entry_key = e.entry_key WHERE e.revision_id = ")
            .push_bind(&revision_id)
            .push(" ORDER BY e.ordinal ASC LIMIT ")
            .push_bind(
                i64::try_from(MAX_PRESENTATION_ENTRY_COUNT + 1)
                    .map_err(|_| RepositoryError::InvalidInput)?,
            );
        let entries = query
            .build()
            .fetch_all(&mut *transaction)
            .await
            .map_err(|_| RepositoryError::ReadFailed)?
            .iter()
            .map(selectable_entry_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        if entries.len() > MAX_PRESENTATION_ENTRY_COUNT
            || u64::try_from(entries.len()).ok() != Some(entry_count)
        {
            return Err(RepositoryError::ReadFailed);
        }
        transaction
            .commit()
            .await
            .map_err(|_| RepositoryError::ReadFailed)?;
        Ok(PresentationSource {
            title,
            revision_id,
            preferences,
            entries,
        })
    }
}

fn push_summary_filters(
    query: &mut QueryBuilder<'_, Sqlite>,
    source: Option<&IndexedSessionSourceV1>,
    search_terms: Option<&[String]>,
    date_filter: Option<&SessionDateFilter>,
) {
    if let Some(source) = source {
        query
            .push(" AND s.source = ")
            .push_bind(source_name(source));
    }
    if let Some(search_terms) = search_terms {
        if search_terms.is_empty() {
            query.push(" AND 0 = 1");
            return;
        }
        query.push(" AND (s.id IN (");
        for (index, term) in search_terms.iter().enumerate() {
            if index > 0 {
                query.push(" INTERSECT ");
            }
            query
                .push(
                    "SELECT search_document.session_id
                       FROM session_search
                       JOIN session_search_documents search_document
                         ON search_document.id = session_search.rowid
                      WHERE session_search MATCH ",
                )
                .push_bind(format!("\"{term}\"*"));
        }
        query.push(")");
        if let Some(date_filter) = date_filter {
            query.push(" OR (");
            push_date_filter(query, date_filter);
            query.push(")");
        }
        query.push(")");
    }
}

fn push_date_filter(query: &mut QueryBuilder<'_, Sqlite>, filter: &SessionDateFilter) {
    let mut has_condition = false;
    for (format, value) in [
        ("%Y", filter.year.map(i64::from)),
        ("%m", filter.month.map(i64::from)),
        ("%d", filter.day.map(i64::from)),
    ] {
        let Some(value) = value else { continue };
        if has_condition {
            query.push(" AND ");
        }
        query
            .push("CAST(strftime('")
            .push(format)
            .push("', ")
            .push(SESSION_SORT_TIME)
            .push(" / 1000, 'unixepoch', 'localtime') AS INTEGER) = ")
            .push_bind(value);
        has_condition = true;
    }
}

fn parse_session_date_filter(value: &str) -> Option<SessionDateFilter> {
    let normalized = value.trim().replace(',', " ");
    let parts = normalized.split_whitespace().collect::<Vec<_>>();
    match parts.as_slice() {
        [value] => parse_iso_date(value)
            .or_else(|| {
                parse_month_name(value).map(|month| SessionDateFilter {
                    year: None,
                    month: Some(month),
                    day: None,
                })
            })
            .or_else(|| {
                parse_year(value).map(|year| SessionDateFilter {
                    year: Some(year),
                    month: None,
                    day: None,
                })
            }),
        [month_name, value] if parse_month_name(month_name).is_some() => {
            let month = parse_month_name(month_name)?;
            if let Some(year) = parse_year(value) {
                Some(SessionDateFilter {
                    year: Some(year),
                    month: Some(month),
                    day: None,
                })
            } else {
                Some(SessionDateFilter {
                    year: None,
                    month: Some(month),
                    day: Some(parse_day(value, month, None)?),
                })
            }
        }
        [day, month_name] => {
            let month = parse_month_name(month_name)?;
            Some(SessionDateFilter {
                year: None,
                month: Some(month),
                day: Some(parse_day(day, month, None)?),
            })
        }
        [month_name, day, year] if parse_month_name(month_name).is_some() => {
            let month = parse_month_name(month_name)?;
            let year = parse_year(year)?;
            Some(SessionDateFilter {
                year: Some(year),
                month: Some(month),
                day: Some(parse_day(day, month, Some(year))?),
            })
        }
        [day, month_name, year] => {
            let month = parse_month_name(month_name)?;
            let year = parse_year(year)?;
            Some(SessionDateFilter {
                year: Some(year),
                month: Some(month),
                day: Some(parse_day(day, month, Some(year))?),
            })
        }
        _ => None,
    }
}

fn parse_iso_date(value: &str) -> Option<SessionDateFilter> {
    let parts = value.split('-').collect::<Vec<_>>();
    let [year, month, remainder @ ..] = parts.as_slice() else {
        return None;
    };
    let year = parse_year(year)?;
    let month = parse_month_number(month)?;
    let day = match remainder {
        [] => None,
        [day] => Some(parse_day(day, month, Some(year))?),
        _ => return None,
    };
    Some(SessionDateFilter {
        year: Some(year),
        month: Some(month),
        day,
    })
}

fn parse_year(value: &str) -> Option<u16> {
    (value.len() == 4 && value.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| value.parse().ok())
        .flatten()
}

fn parse_month_number(value: &str) -> Option<u8> {
    let month = value.parse::<u8>().ok()?;
    (1..=12).contains(&month).then_some(month)
}

fn parse_day(value: &str, month: u8, year: Option<u16>) -> Option<u8> {
    let day = value.parse::<u8>().ok()?;
    let maximum = match month {
        2 if year.is_none_or(is_leap_year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=maximum).contains(&day).then_some(day)
}

fn parse_month_name(value: &str) -> Option<u8> {
    match value.to_ascii_lowercase().as_str() {
        "jan" | "january" => Some(1),
        "feb" | "february" => Some(2),
        "mar" | "march" => Some(3),
        "apr" | "april" => Some(4),
        "may" => Some(5),
        "jun" | "june" => Some(6),
        "jul" | "july" => Some(7),
        "aug" | "august" => Some(8),
        "sep" | "sept" | "september" => Some(9),
        "oct" | "october" => Some(10),
        "nov" | "november" => Some(11),
        "dec" | "december" => Some(12),
        _ => None,
    }
}

fn is_leap_year(year: u16) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

/// Converts the bounded user query into literal Unicode word terms. FTS
/// operators and punctuation never enter the MATCH expression, and separate
/// terms may match separate normalized entries in the same session.
/// Source: https://www.sqlite.org/fts5.html#full_text_query_syntax
/// Source: https://www.sqlite.org/fts5.html#fts5_prefix_queries
fn literal_search_terms(value: &str) -> Vec<String> {
    let mut terms = Vec::new();
    for term in value
        .split(|character: char| !character.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(str::to_lowercase)
    {
        if !terms.contains(&term) {
            terms.push(term);
        }
    }
    terms
}

fn summary_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<IndexedSessionSummaryV1, RepositoryError> {
    Ok(IndexedSessionSummaryV1 {
        schema_version: INDEXED_LIBRARY_SCHEMA_VERSION,
        session_id: string(row, "id")?,
        source: source_from_name(&string(row, "source")?)?,
        title: string(row, "title")?,
        created_at_ms: optional_u64(row, "created_at_ms")?,
        last_indexed_at_ms: read_u64(row, "indexed_at_ms")?,
        source_present: boolean(row, "source_present")?,
        entry_count: read_u64(row, "entry_count")?,
        selected_entry_count: read_u64(row, "selected_entry_count")?,
        duration_ms: read_u64(row, "duration_ms")?,
        diagnostic_count: read_u64(row, "diagnostic_count")?,
        content_availability: ContentAvailabilityV1 {
            reasoning: parse_reasoning(&string(row, "reasoning_availability")?)?,
            tool_details: parse_availability(&string(row, "tool_detail_availability")?)?,
        },
    })
}

fn selectable_entry_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<SelectableEntryV1, RepositoryError> {
    let entry_key = string(row, "entry_key")?;
    let ordinal = read_u64(row, "ordinal")?;
    let at_ms = read_u64(row, "at_ms")?;
    let entry = match string(row, "kind")?.as_str() {
        "user" => SessionEntryV1::User {
            entry_key,
            ordinal,
            at_ms,
            text: required_optional_string(row, "body_text")?,
        },
        "assistant" => SessionEntryV1::Assistant {
            entry_key,
            ordinal,
            at_ms,
            markdown: required_optional_string(row, "body_text")?,
        },
        "reasoning" => SessionEntryV1::Reasoning {
            entry_key,
            ordinal,
            at_ms,
            text: required_optional_string(row, "body_text")?,
        },
        "tool-call" => SessionEntryV1::ToolCall {
            entry_key,
            ordinal,
            at_ms,
            name: required_optional_string(row, "tool_name")?,
            status: parse_tool_status(&required_optional_string(row, "tool_status")?)?,
            summary: required_optional_string(row, "summary_text")?,
            detail: match required_optional_string(row, "tool_detail_availability")?.as_str() {
                "available" => ToolDetailV1::Available {
                    arguments: optional_string(row, "tool_arguments")?,
                    result: optional_string(row, "tool_result")?,
                },
                "unavailable" => ToolDetailV1::Unavailable,
                _ => return Err(RepositoryError::ReadFailed),
            },
        },
        "file-change" => SessionEntryV1::FileChange {
            entry_key,
            ordinal,
            at_ms,
            display_path: required_optional_string(row, "display_path")?,
            summary: required_optional_string(row, "summary_text")?,
        },
        "unknown" => SessionEntryV1::Unknown {
            entry_key,
            ordinal,
            at_ms,
            source_type: required_optional_string(row, "source_type")?,
        },
        _ => return Err(RepositoryError::ReadFailed),
    };
    Ok(SelectableEntryV1 {
        entry,
        selected: boolean(row, "selected")?,
    })
}

fn preferences_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<SessionPreferencesV1, RepositoryError> {
    Ok(SessionPreferencesV1 {
        schema_version: INDEXED_LIBRARY_SCHEMA_VERSION,
        visibility: VisibilityPreferencesV1 {
            show_tool_calls: boolean(row, "show_tool_calls")?,
            show_tool_details: boolean(row, "show_tool_details")?,
            show_reasoning: boolean(row, "show_reasoning")?,
        },
        timing: PresentationTimingV1 {
            entry_delay_ms: read_u64(row, "entry_delay_ms")?,
            playback_speed: row
                .try_get("playback_speed")
                .map_err(|_| RepositoryError::ReadFailed)?,
        },
        appearance: AppearancePreferencesV1 {
            theme: LibraryThemeV1 {
                background: string(row, "background_color")?,
                surface: string(row, "surface_color")?,
                text: string(row, "text_color")?,
                muted: string(row, "muted_color")?,
                accent: string(row, "accent_color")?,
                success: string(row, "success_color")?,
                error: string(row, "error_color")?,
            },
            font: LibraryFontV1 {
                family: match string(row, "font_family")?.as_str() {
                    "JetBrains Mono" => FontFamily::JetBrainsMono,
                    _ => return Err(RepositoryError::ReadFailed),
                },
                size_px: u32::try_from(read_u64(row, "font_size_px")?)
                    .map_err(|_| RepositoryError::ReadFailed)?,
                line_height: row
                    .try_get("line_height")
                    .map_err(|_| RepositoryError::ReadFailed)?,
            },
        },
    })
}

fn parse_reasoning(value: &str) -> Result<ReasoningAvailability, RepositoryError> {
    match value {
        "available" => Ok(ReasoningAvailability::Available),
        "unavailable" => Ok(ReasoningAvailability::Unavailable),
        _ => Err(RepositoryError::ReadFailed),
    }
}

fn parse_availability(value: &str) -> Result<ContentAvailability, RepositoryError> {
    match value {
        "available" => Ok(ContentAvailability::Available),
        "partial" => Ok(ContentAvailability::Partial),
        "unavailable" => Ok(ContentAvailability::Unavailable),
        _ => Err(RepositoryError::ReadFailed),
    }
}

fn parse_tool_status(value: &str) -> Result<IndexedToolStatus, RepositoryError> {
    match value {
        "pending" => Ok(IndexedToolStatus::Pending),
        "running" => Ok(IndexedToolStatus::Running),
        "succeeded" => Ok(IndexedToolStatus::Succeeded),
        "failed" => Ok(IndexedToolStatus::Failed),
        _ => Err(RepositoryError::ReadFailed),
    }
}

fn entry_key(entry: &SessionEntryV1) -> &str {
    match entry {
        SessionEntryV1::User { entry_key, .. }
        | SessionEntryV1::Assistant { entry_key, .. }
        | SessionEntryV1::Reasoning { entry_key, .. }
        | SessionEntryV1::ToolCall { entry_key, .. }
        | SessionEntryV1::FileChange { entry_key, .. }
        | SessionEntryV1::Unknown { entry_key, .. } => entry_key,
    }
}

fn string(row: &sqlx::sqlite::SqliteRow, column: &str) -> Result<String, RepositoryError> {
    row.try_get(column).map_err(|_| RepositoryError::ReadFailed)
}

fn optional_string(
    row: &sqlx::sqlite::SqliteRow,
    column: &str,
) -> Result<Option<String>, RepositoryError> {
    row.try_get(column).map_err(|_| RepositoryError::ReadFailed)
}

fn required_optional_string(
    row: &sqlx::sqlite::SqliteRow,
    column: &str,
) -> Result<String, RepositoryError> {
    optional_string(row, column)?.ok_or(RepositoryError::ReadFailed)
}

fn read_u64(row: &sqlx::sqlite::SqliteRow, column: &str) -> Result<u64, RepositoryError> {
    let value: i64 = row
        .try_get(column)
        .map_err(|_| RepositoryError::ReadFailed)?;
    u64::try_from(value).map_err(|_| RepositoryError::ReadFailed)
}

fn optional_u64(
    row: &sqlx::sqlite::SqliteRow,
    column: &str,
) -> Result<Option<u64>, RepositoryError> {
    row.try_get::<Option<i64>, _>(column)
        .map_err(|_| RepositoryError::ReadFailed)?
        .map(u64::try_from)
        .transpose()
        .map_err(|_| RepositoryError::ReadFailed)
}

fn boolean(row: &sqlx::sqlite::SqliteRow, column: &str) -> Result<bool, RepositoryError> {
    match row.try_get::<i64, _>(column) {
        Ok(0) => Ok(false),
        Ok(1) => Ok(true),
        _ => Err(RepositoryError::ReadFailed),
    }
}

fn to_i64(value: u64) -> Result<i64, RepositoryError> {
    i64::try_from(value).map_err(|_| RepositoryError::InvalidInput)
}

fn valid_opaque_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}
