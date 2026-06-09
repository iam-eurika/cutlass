//! Serialize [`Map`] as a stable sorted `Vec` of pairs for JSON project files.

use std::hash::Hash;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::Map;

pub fn serialize<K, V, S>(map: &Map<K, V>, serializer: S) -> Result<S::Ok, S::Error>
where
    K: Serialize + Ord,
    V: Serialize,
    S: Serializer,
{
    let mut pairs: Vec<(&K, &V)> = map.iter().collect();
    pairs.sort_by_key(|(k, _)| *k);
    pairs.serialize(serializer)
}

pub fn deserialize<'de, K, V, D>(deserializer: D) -> Result<Map<K, V>, D::Error>
where
    K: Deserialize<'de> + Eq + Hash,
    V: Deserialize<'de>,
    D: Deserializer<'de>,
{
    let pairs: Vec<(K, V)> = Vec::deserialize(deserializer)?;
    Ok(pairs.into_iter().collect())
}
