//! What a plugin reports about itself: a small `status.json` in its state directory.
//!
//! The plugin writes it whenever its state changes (is it running, which network mode, ...) and
//! moo only reads it, when a client lists plugins, so a plugin's settings can show their current
//! value without moo running anything. Values are kept as short strings; anything else in the
//! file is ignored.

use std::collections::BTreeMap;
use std::io::Read;

const MAX_STATUS_BYTES: u64 = 16 * 1024;
pub(crate) const MAX_KEY_CHARS: usize = 64;
pub(crate) const MAX_VALUE_CHARS: usize = 256;

/// The reported status in `bytes`: the top-level string, number and boolean entries of a JSON
/// object. A document that is not an object reports nothing.
pub(crate) fn parse_status(bytes: &[u8]) -> BTreeMap<String, String> {
    let Ok(serde_json::Value::Object(object)) = serde_json::from_slice(bytes) else {
        return BTreeMap::new();
    };
    object
        .into_iter()
        .filter(|(key, _)| key.chars().count() <= MAX_KEY_CHARS)
        .filter_map(|(key, value)| {
            let text = match value {
                serde_json::Value::String(text) => text,
                serde_json::Value::Bool(flag) => flag.to_string(),
                serde_json::Value::Number(number) => number.to_string(),
                _ => return None,
            };
            (text.chars().count() <= MAX_VALUE_CHARS).then_some((key, text))
        })
        .collect()
}

/// What `plugin_id` last reported; empty when it has reported nothing or the file is unreadable.
pub(crate) fn read_plugin_status(plugin_id: &str) -> BTreeMap<String, String> {
    let path = crate::plugin_paths::plugin_state_dir(plugin_id).join("status.json");
    match std::fs::metadata(&path) {
        Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_STATUS_BYTES => {}
        _ => return BTreeMap::new(),
    }
    std::fs::File::open(&path)
        .map(read_status)
        .unwrap_or_default()
}

fn read_status(reader: impl Read) -> BTreeMap<String, String> {
    let mut bytes = Vec::new();
    if reader
        .take(MAX_STATUS_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() as u64 > MAX_STATUS_BYTES
    {
        return BTreeMap::new();
    }
    parse_status(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_status_values_are_ignored_instead_of_becoming_another_option() {
        let value = "界".repeat(256);
        let bytes = serde_json::to_vec(&serde_json::json!({
            "mode": value,
            "oversized": format!("{value}y"),
        }))
        .unwrap();
        assert_eq!(
            parse_status(&bytes),
            BTreeMap::from([("mode".to_string(), value)])
        );
    }

    #[test]
    fn status_reader_rejects_oversized_documents_and_accepts_the_limit() {
        let mut bytes = br#"{"network":"lan"}"#.to_vec();
        bytes.resize(MAX_STATUS_BYTES as usize, b' ');
        assert_eq!(
            read_status(bytes.as_slice()),
            BTreeMap::from([("network".to_string(), "lan".to_string())])
        );
        bytes.push(b' ');
        assert!(read_status(bytes.as_slice()).is_empty());
        let mut reader = std::io::Cursor::new(bytes);
        reader.get_mut().resize(MAX_STATUS_BYTES as usize * 2, b' ');
        assert!(read_status(&mut reader).is_empty());
        assert_eq!(reader.position(), MAX_STATUS_BYTES + 1);
    }

    #[test]
    fn keeps_strings_numbers_and_booleans_as_text() {
        let status = parse_status(
            br#"{"network":"lan","running":true,"pid":412,"nested":{"a":1},"list":[1]}"#,
        );
        assert_eq!(status.get("network").map(String::as_str), Some("lan"));
        assert_eq!(status.get("running").map(String::as_str), Some("true"));
        assert_eq!(status.get("pid").map(String::as_str), Some("412"));
        assert!(!status.contains_key("nested") && !status.contains_key("list"));
    }

    #[test]
    fn anything_but_an_object_reports_nothing() {
        for bad in [&b"not json"[..], b"[1,2]", b"\"lan\"", b"", b"null"] {
            assert!(parse_status(bad).is_empty());
        }
    }

    #[test]
    fn preserves_all_valid_entries_and_ignores_oversized_keys_and_values() {
        let long_value = "x".repeat(1000);
        let mut object = serde_json::Map::new();
        object.insert("k".repeat(MAX_KEY_CHARS + 1), "dropped".into());
        object.insert("long".into(), long_value.into());
        for index in 0..100 {
            object.insert(format!("entry{index:03}"), "v".into());
        }
        let status = read_status(serde_json::Value::Object(object).to_string().as_bytes());
        assert_eq!(status.len(), 100);
        assert_eq!(status.get("entry099").map(String::as_str), Some("v"));
        assert!(!status.contains_key("long"));
        assert!(status
            .keys()
            .all(|key| key.chars().count() <= MAX_KEY_CHARS));
        assert!(status
            .values()
            .all(|value| value.chars().count() <= MAX_VALUE_CHARS));
    }
}
