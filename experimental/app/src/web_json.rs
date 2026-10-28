//! Strict JSON for the documents the client verifies (bundle manifest, trust
//! statement, health, release feed): what `serde_json` plus `deny_unknown_fields`
//! accepts, minus the shapes serde is lax about and the Linux client's reader
//! (`linux/src/core/json.ts`) refuses. The shared vectors in
//! `test-vectors/json.json` pin both readers to the same answers.
//!
//! - an object key repeated at any depth, also inside a field that is ignored;
//! - a struct written as an array (serde reads a sequence into a struct);
//! - a duplicate key of a map field (serde keeps the last one).
use serde::Deserialize;
use serde::de::{DeserializeOwned, Deserializer, MapAccess, SeqAccess, Visitor};
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::marker::PhantomData;

/// Any JSON value, read only to refuse a repeated object key.
struct Unique;

impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Check;
        impl<'de> Visitor<'de> for Check {
            type Value = Unique;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("JSON")
            }
            fn visit_bool<E>(self, _: bool) -> Result<Unique, E> {
                Ok(Unique)
            }
            fn visit_i64<E>(self, _: i64) -> Result<Unique, E> {
                Ok(Unique)
            }
            fn visit_u64<E>(self, _: u64) -> Result<Unique, E> {
                Ok(Unique)
            }
            fn visit_f64<E>(self, _: f64) -> Result<Unique, E> {
                Ok(Unique)
            }
            fn visit_str<E>(self, _: &str) -> Result<Unique, E> {
                Ok(Unique)
            }
            fn visit_unit<E>(self) -> Result<Unique, E> {
                Ok(Unique)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Unique, A::Error> {
                while seq.next_element::<Unique>()?.is_some() {}
                Ok(Unique)
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Unique, A::Error> {
                let mut seen = HashSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !seen.insert(key) {
                        return Err(serde::de::Error::custom("duplicate key"));
                    }
                    map.next_value::<Unique>()?;
                }
                Ok(Unique)
            }
        }
        deserializer.deserialize_any(Check)
    }
}

/// A struct that must be written as an object.
struct MapOnly<T>(T);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for MapOnly<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Object<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for Object<T> {
            type Value = MapOnly<T>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<MapOnly<T>, A::Error> {
                T::deserialize(serde::de::value::MapAccessDeserializer::new(map)).map(MapOnly)
            }
        }
        deserializer.deserialize_map(Object(PhantomData))
    }
}

/// Parses one document: valid JSON with no repeated key, then `T` from objects only.
pub fn parse<T: DeserializeOwned>(bytes: &[u8]) -> Option<T> {
    serde_json::from_slice::<Unique>(bytes).ok()?;
    serde_json::from_slice::<MapOnly<T>>(bytes)
        .ok()
        .map(|document| document.0)
}

/// For a `Vec` of structs: every element must be an object.
pub fn objects<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Vec<T>, D::Error> {
    Ok(Vec::<MapOnly<T>>::deserialize(deserializer)?
        .into_iter()
        .map(|element| element.0)
        .collect())
}

/// For a string map: a repeated key is an error, not "the last one wins".
pub fn unique_map<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, String>, D::Error> {
    struct Strings;
    impl<'de> Visitor<'de> for Strings {
        type Value = BTreeMap<String, String>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("an object of strings")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut out = BTreeMap::new();
            while let Some((key, value)) = map.next_entry::<String, String>()? {
                if out.insert(key, value).is_some() {
                    return Err(serde::de::Error::custom("duplicate key"));
                }
            }
            Ok(out)
        }
    }
    deserializer.deserialize_map(Strings)
}
