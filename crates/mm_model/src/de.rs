// SPDX-License-Identifier: AGPL-3.0-only

//! Lenient deserializers for the server's JSON.

use std::collections::HashMap;

use serde::de::{DeserializeOwned, Error};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// `null` (a nil Go slice or map) reads as the default value.
pub(crate) fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// A string map whose values may arrive as other scalars (`true`, `5`); they are kept as their
/// JSON text. `null` values are dropped.
pub(crate) fn string_map<'de, D>(deserializer: D) -> Result<HashMap<String, String>, D::Error>
where
    D: Deserializer<'de>,
{
    let map = Option::<HashMap<String, Value>>::deserialize(deserializer)?.unwrap_or_default();
    Ok(map
        .into_iter()
        .filter_map(|(key, value)| match value {
            Value::Null => None,
            Value::String(text) => Some((key, text)),
            other => Some((key, other.to_string())),
        })
        .collect())
}

/// A nested document that the server usually sends JSON-encoded as a string (most WebSocket
/// payloads: `data.post`, `data.channel`, ...) and sometimes as the value itself. `null` and an
/// empty string read as the default value.
pub(crate) fn nested<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    from_nested(Value::deserialize(deserializer)?).map_err(D::Error::custom)
}

pub(crate) fn from_nested<T>(value: Value) -> serde_json::Result<T>
where
    T: DeserializeOwned + Default,
{
    match value {
        Value::Null => Ok(T::default()),
        Value::String(text) if text.is_empty() => Ok(T::default()),
        Value::String(text) => serde_json::from_str(&text),
        other => serde_json::from_value(other),
    }
}
