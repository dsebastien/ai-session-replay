use serde_json::Value;

const MAX_PATH_SEGMENTS: usize = 64;
const MAX_ARRAY_ITEMS: usize = 100_000;

#[derive(Debug, PartialEq)]
pub(crate) struct MutationReplay {
    pub(crate) value: Value,
    pub(crate) reset_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MutationError {
    InvalidJson { record_ordinal: usize },
    InvalidKind { record_ordinal: usize },
    MissingInitial { record_ordinal: usize },
    InvalidPath { record_ordinal: usize },
    InvalidValue { record_ordinal: usize },
}

pub(crate) fn replay_mutations<'a>(
    records: impl IntoIterator<Item = (usize, &'a [u8])>,
) -> Result<MutationReplay, MutationError> {
    let mut state = None;
    let mut reset_count = 0;
    for (record_ordinal, bytes) in records {
        let record: Value = serde_json::from_slice(bytes)
            .map_err(|_| MutationError::InvalidJson { record_ordinal })?;
        let kind = record
            .get("kind")
            .and_then(Value::as_u64)
            .ok_or(MutationError::InvalidKind { record_ordinal })?;
        match kind {
            0 => {
                let value = record
                    .get("v")
                    .filter(|value| value.is_object())
                    .cloned()
                    .ok_or(MutationError::InvalidValue { record_ordinal })?;
                if state.replace(value).is_some() {
                    reset_count += 1;
                }
            }
            1 => {
                let current = state
                    .as_mut()
                    .ok_or(MutationError::MissingInitial { record_ordinal })?;
                let path = mutation_path(&record, record_ordinal)?;
                let value = record
                    .get("v")
                    .cloned()
                    .ok_or(MutationError::InvalidValue { record_ordinal })?;
                set_value(current, &path, value)
                    .ok_or(MutationError::InvalidPath { record_ordinal })?;
            }
            2 => {
                let current = state
                    .as_mut()
                    .ok_or(MutationError::MissingInitial { record_ordinal })?;
                let path = mutation_path(&record, record_ordinal)?;
                let values = match record.get("v") {
                    Some(Value::Array(values)) => values.clone(),
                    Some(_) => return Err(MutationError::InvalidValue { record_ordinal }),
                    None => Vec::new(),
                };
                let truncate_at = match record.get("i") {
                    Some(value) => Some(
                        value
                            .as_u64()
                            .and_then(|value| usize::try_from(value).ok())
                            .filter(|value| *value <= MAX_ARRAY_ITEMS)
                            .ok_or(MutationError::InvalidValue { record_ordinal })?,
                    ),
                    None => None,
                };
                push_values(current, &path, truncate_at, values)
                    .ok_or(MutationError::InvalidPath { record_ordinal })?;
            }
            3 => {
                let current = state
                    .as_mut()
                    .ok_or(MutationError::MissingInitial { record_ordinal })?;
                let path = mutation_path(&record, record_ordinal)?;
                delete_value(current, &path)
                    .ok_or(MutationError::InvalidPath { record_ordinal })?;
            }
            _ => return Err(MutationError::InvalidKind { record_ordinal }),
        }
    }
    Ok(MutationReplay {
        value: state.ok_or(MutationError::MissingInitial { record_ordinal: 0 })?,
        reset_count,
    })
}

#[derive(Debug)]
enum PathSegment {
    Property(String),
    Index(usize),
}

fn mutation_path(record: &Value, record_ordinal: usize) -> Result<Vec<PathSegment>, MutationError> {
    let raw = record
        .get("k")
        .and_then(Value::as_array)
        .filter(|path| !path.is_empty() && path.len() <= MAX_PATH_SEGMENTS)
        .ok_or(MutationError::InvalidPath { record_ordinal })?;
    raw.iter()
        .map(|segment| match segment {
            Value::String(value) if !value.is_empty() && value.chars().count() <= 256 => {
                Ok(PathSegment::Property(value.clone()))
            }
            Value::Number(value) => value
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .filter(|value| *value < MAX_ARRAY_ITEMS)
                .map(PathSegment::Index)
                .ok_or(MutationError::InvalidPath { record_ordinal }),
            _ => Err(MutationError::InvalidPath { record_ordinal }),
        })
        .collect()
}

fn value_at_path_mut<'a>(mut value: &'a mut Value, path: &[PathSegment]) -> Option<&'a mut Value> {
    for segment in path {
        value = match (value, segment) {
            (Value::Object(object), PathSegment::Property(key)) => object.get_mut(key)?,
            (Value::Array(array), PathSegment::Index(index)) => array.get_mut(*index)?,
            _ => return None,
        };
    }
    Some(value)
}

fn set_value(state: &mut Value, path: &[PathSegment], value: Value) -> Option<()> {
    let (last, parent_path) = path.split_last()?;
    let parent = value_at_path_mut(state, parent_path)?;
    match (parent, last) {
        (Value::Object(object), PathSegment::Property(key)) => {
            object.insert(key.clone(), value);
        }
        (Value::Array(array), PathSegment::Index(index)) if *index < array.len() => {
            array[*index] = value;
        }
        _ => return None,
    }
    Some(())
}

fn push_values(
    state: &mut Value,
    path: &[PathSegment],
    truncate_at: Option<usize>,
    values: Vec<Value>,
) -> Option<()> {
    let target = value_at_path_mut(state, path)?;
    let array = target.as_array_mut()?;
    if truncate_at.is_some_and(|index| index > array.len())
        || truncate_at
            .unwrap_or(array.len())
            .saturating_add(values.len())
            > MAX_ARRAY_ITEMS
    {
        return None;
    }
    if let Some(index) = truncate_at {
        array.truncate(index);
    }
    array.extend(values);
    Some(())
}

fn delete_value(state: &mut Value, path: &[PathSegment]) -> Option<()> {
    let (last, parent_path) = path.split_last()?;
    let parent = value_at_path_mut(state, parent_path)?;
    match (parent, last) {
        (Value::Object(object), PathSegment::Property(key)) => {
            object.remove(key)?;
        }
        (Value::Array(array), PathSegment::Index(index)) if *index < array.len() => {
            array[*index] = Value::Null;
        }
        _ => return None,
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{MutationError, replay_mutations};

    fn replay(lines: &[&str]) -> Result<super::MutationReplay, MutationError> {
        replay_mutations(
            lines
                .iter()
                .enumerate()
                .map(|(ordinal, line)| (ordinal, line.as_bytes())),
        )
    }

    #[test]
    fn replays_initial_set_push_truncate_and_delete_operations() {
        let replayed = replay(&[
            r#"{"kind":0,"v":{"title":"Initial","requests":[{"id":"one"}],"metadata":{"temporary":true}}}"#,
            r#"{"kind":1,"k":["title"],"v":"Updated"}"#,
            r#"{"kind":2,"k":["requests"],"v":[{"id":"two"}]}"#,
            r#"{"kind":2,"k":["requests"],"i":1,"v":[{"id":"replacement"}]}"#,
            r#"{"kind":3,"k":["metadata"]}"#,
        ])
        .expect("valid mutation log should replay");

        assert_eq!(
            replayed.value,
            json!({"title":"Updated","requests":[{"id":"one"},{"id":"replacement"}]})
        );
        assert_eq!(replayed.reset_count, 0);
    }

    #[test]
    fn a_later_initial_record_resets_compacted_state() {
        let replayed = replay(&[
            r#"{"kind":0,"v":{"value":"old"}}"#,
            r#"{"kind":1,"k":["value"],"v":"changed"}"#,
            r#"{"kind":0,"v":{"value":"compacted"}}"#,
        ])
        .expect("a replacement initial record should reset state");

        assert_eq!(replayed.value, json!({"value":"compacted"}));
        assert_eq!(replayed.reset_count, 1);
    }

    #[test]
    fn rejects_malformed_or_unsafe_mutations_deterministically() {
        assert_eq!(
            replay(&[r#"{"kind":1,"k":["value"],"v":1}"#]),
            Err(MutationError::MissingInitial { record_ordinal: 0 })
        );
        assert_eq!(
            replay(&[r#"{"kind":9,"v":{}}"#]),
            Err(MutationError::InvalidKind { record_ordinal: 0 })
        );
        assert_eq!(
            replay(&[r#"{"kind":0,"v":{}}"#, r#"{"kind":2,"k":[],"i":100001}"#]),
            Err(MutationError::InvalidPath { record_ordinal: 1 })
        );
        assert_eq!(
            replay(&[r#"{"kind":0,"v":{}}"#, "not json"]),
            Err(MutationError::InvalidJson { record_ordinal: 1 })
        );
    }

    #[test]
    fn supports_numeric_array_paths_and_rejects_invalid_values_and_traversal() {
        assert_eq!(
            replay(&[
                r#"{"kind":0,"v":{"items":[{"name":"one"},{"name":"two"}]}}"#,
                r#"{"kind":1,"k":["items",1,"name"],"v":"updated"}"#,
                r#"{"kind":3,"k":["items",0]}"#,
            ])
            .unwrap()
            .value,
            json!({"items":[null,{"name":"updated"}]})
        );

        for (line, expected) in [
            (
                r#"{"kind":0,"v":[]}"#,
                MutationError::InvalidValue { record_ordinal: 0 },
            ),
            (
                r#"{"kind":0,"v":{}}\n{"kind":1,"k":["missing"]}"#,
                MutationError::InvalidJson { record_ordinal: 0 },
            ),
        ] {
            assert_eq!(replay(&[line]), Err(expected));
        }
        assert_eq!(
            replay(&[
                r#"{"kind":0,"v":{"items":[]}}"#,
                r#"{"kind":2,"k":["items"],"v":{}}"#
            ]),
            Err(MutationError::InvalidValue { record_ordinal: 1 })
        );
        assert_eq!(
            replay(&[
                r#"{"kind":0,"v":{"items":[]}}"#,
                r#"{"kind":2,"k":["items"],"i":1}"#
            ]),
            Err(MutationError::InvalidPath { record_ordinal: 1 })
        );
        assert_eq!(
            replay(&[
                r#"{"kind":0,"v":{"scalar":1}}"#,
                r#"{"kind":1,"k":["scalar","nested"],"v":2}"#
            ]),
            Err(MutationError::InvalidPath { record_ordinal: 1 })
        );
        assert_eq!(
            replay(&[r#"{"kind":0,"v":{}}"#, r#"{"kind":3,"k":["missing"]}"#]),
            Err(MutationError::InvalidPath { record_ordinal: 1 })
        );
    }

    #[test]
    fn requires_a_bounded_nonempty_mutation_path() {
        let oversized = serde_json::to_string(&json!({
            "kind": 1,
            "k": vec!["segment"; 65],
            "v": 1,
        }))
        .unwrap();
        for mutation in [
            r#"{"kind":1,"k":[],"v":1}"#.to_owned(),
            r#"{"kind":1,"k":[""],"v":1}"#.to_owned(),
            format!(r#"{{"kind":1,"k":["{}"],"v":1}}"#, "x".repeat(257)),
            r#"{"kind":1,"k":[-1],"v":1}"#.to_owned(),
            r#"{"kind":1,"k":[100000],"v":1}"#.to_owned(),
            oversized,
        ] {
            assert_eq!(
                replay(&[r#"{"kind":0,"v":{}}"#, mutation.as_str()]),
                Err(MutationError::InvalidPath { record_ordinal: 1 })
            );
        }
    }

    #[test]
    fn rejects_each_operation_when_its_required_state_or_value_is_missing() {
        assert_eq!(
            replay(&[r#"{"kind":2,"k":["items"],"v":[]}"#]),
            Err(MutationError::MissingInitial { record_ordinal: 0 })
        );
        assert_eq!(
            replay(&[r#"{"kind":3,"k":["value"]}"#]),
            Err(MutationError::MissingInitial { record_ordinal: 0 })
        );
        assert_eq!(
            replay(&[r#"{"kind":0,"v":{}}"#, r#"{"kind":1,"k":["value"]}"#]),
            Err(MutationError::InvalidValue { record_ordinal: 1 })
        );
        assert_eq!(
            replay(&[
                r#"{"kind":0,"v":{"items":[]}}"#,
                r#"{"kind":2,"k":["items"]}"#,
            ])
            .unwrap()
            .value,
            json!({"items":[]})
        );
        assert_eq!(
            replay(&[
                r#"{"kind":0,"v":{"items":[1]}}"#,
                r#"{"kind":1,"k":["items",1],"v":2}"#,
            ]),
            Err(MutationError::InvalidPath { record_ordinal: 1 })
        );
        assert_eq!(
            replay(&[
                r#"{"kind":0,"v":{"items":[1]}}"#,
                r#"{"kind":3,"k":["items",1]}"#,
            ]),
            Err(MutationError::InvalidPath { record_ordinal: 1 })
        );
    }
}
