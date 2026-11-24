use std::fmt;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::{
    self, DeserializeOwned, DeserializeSeed, Error as _, MapAccess, SeqAccess, Visitor,
};
use serde_json::{Map, Number, Value};

use crate::HARD_MAX_FILE_BYTES;
use crate::atomic::atomic_write;
use crate::error::{Result, StorageError};
use crate::lock::WriterLock;
use crate::secure::{FileSnapshot, PrivateDirectory, TargetSnapshot, read_bounded};

const DEFAULT_MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_MAX_DEPTH: usize = 64;
const DEFAULT_MAX_NODES: usize = 100_000;
const HARD_MAX_DEPTH: usize = 128;
/// Sized so the largest store (orchestration, 64 MiB) is bound by its bytes, not
/// by a node count it reaches first.
const HARD_MAX_NODES: usize = 4_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JsonLimits {
    file_bytes: usize,
    depth: usize,
    nodes: usize,
}

impl JsonLimits {
    /// # Errors
    ///
    /// Returns an error when any requested limit is zero or exceeds the hard
    /// ceiling enforced by this crate.
    pub fn new(max_file_bytes: usize, max_depth: usize, max_nodes: usize) -> Result<Self> {
        if max_file_bytes == 0 || max_file_bytes > HARD_MAX_FILE_BYTES {
            return Err(StorageError::InvalidPath {
                path: PathBuf::from("<json-byte-limit>"),
                reason: "JSON byte limit is outside the supported bounded range",
            });
        }
        if max_depth == 0 || max_depth > HARD_MAX_DEPTH {
            return Err(StorageError::InvalidPath {
                path: PathBuf::from("<json-depth-limit>"),
                reason: "JSON depth limit is outside the supported bounded range",
            });
        }
        if max_nodes == 0 || max_nodes > HARD_MAX_NODES {
            return Err(StorageError::InvalidPath {
                path: PathBuf::from("<json-node-limit>"),
                reason: "JSON node limit is outside the supported bounded range",
            });
        }
        Ok(Self {
            file_bytes: max_file_bytes,
            depth: max_depth,
            nodes: max_nodes,
        })
    }

    #[must_use]
    pub const fn max_file_bytes(self) -> usize {
        self.file_bytes
    }
}

impl Default for JsonLimits {
    fn default() -> Self {
        Self {
            file_bytes: DEFAULT_MAX_FILE_BYTES,
            depth: DEFAULT_MAX_DEPTH,
            nodes: DEFAULT_MAX_NODES,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedJson<T> {
    value: T,
    snapshot: FileSnapshot,
}

impl<T> LoadedJson<T> {
    #[must_use]
    pub const fn value(&self) -> &T {
        &self.value
    }

    #[must_use]
    pub const fn snapshot(&self) -> &FileSnapshot {
        &self.snapshot
    }

    #[must_use]
    pub fn into_parts(self) -> (T, FileSnapshot) {
        (self.value, self.snapshot)
    }
}

/// Reads and strictly decodes a private JSON file without changing any bytes
/// or filesystem metadata intentionally.
///
/// # Errors
///
/// Returns an error for unsafe storage, exceeded bounds, duplicate members,
/// invalid JSON, or a value that fails `T`'s schema.
pub fn read_json<T: DeserializeOwned>(
    directory: &PrivateDirectory,
    name: &str,
    limits: JsonLimits,
) -> Result<Option<LoadedJson<T>>> {
    let Some(file) = read_bounded(directory, name, limits.file_bytes)? else {
        return Ok(None);
    };
    let path = directory.path().join(name);
    let value = decode_json_strict(file.bytes(), limits, &path)?;
    Ok(Some(LoadedJson {
        value,
        snapshot: file.snapshot().clone(),
    }))
}

/// Strictly decodes bounded JSON and rejects duplicate decoded object keys.
///
/// # Errors
///
/// Returns an error for byte/depth/node overflow, invalid JSON, duplicate
/// members, trailing data, or a value that fails `T`'s schema.
pub fn decode_json_strict<T: DeserializeOwned>(
    bytes: &[u8],
    limits: JsonLimits,
    path: impl AsRef<Path>,
) -> Result<T> {
    let path = path.as_ref();
    let value = decode_json_value_strict(bytes, limits, path)?;
    serde_json::from_value(value).map_err(|error| StorageError::Corrupt {
        path: path.to_path_buf(),
        reason: format!("JSON failed its typed schema: {error}"),
    })
}

/// The untyped half of [`decode_json_strict`]: the same bounds, duplicate-key
/// and trailing-data checks, returning the decoded tree itself. A caller that
/// only validates bytes or walks the tree uses this rather than
/// `decode_json_strict::<Value>`, which would rebuild an identical tree.
///
/// # Errors
///
/// Returns an error for byte/depth/node overflow, invalid JSON, duplicate
/// members, or trailing data.
pub fn decode_json_value_strict(
    bytes: &[u8],
    limits: JsonLimits,
    path: impl AsRef<Path>,
) -> Result<Value> {
    let path = path.as_ref();
    if bytes.len() > limits.file_bytes {
        return Err(StorageError::BoundsExceeded {
            path: path.to_path_buf(),
            max_bytes: limits.file_bytes,
            actual_bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        });
    }
    let mut budget = JsonBudget { nodes: 0, limits };
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = StrictValueSeed {
        budget: &mut budget,
        depth: 0,
    }
    .deserialize(&mut deserializer)
    .map_err(|error| StorageError::Corrupt {
        path: path.to_path_buf(),
        reason: format!("invalid, duplicate, or over-budget JSON: {error}"),
    })?;
    deserializer.end().map_err(|error| StorageError::Corrupt {
        path: path.to_path_buf(),
        reason: format!("trailing or invalid JSON data: {error}"),
    })?;
    Ok(value)
}

/// Serializes and atomically installs typed JSON with a trailing newline.
///
/// # Errors
///
/// Returns an error when serialization or bounds fail, the expected snapshot
/// drifted, the lease was lost, or the durable atomic write cannot be proven.
pub fn atomic_write_json<T: Serialize + DeserializeOwned>(
    directory: &PrivateDirectory,
    lock: &WriterLock,
    name: &str,
    value: &T,
    expected: &TargetSnapshot,
    limits: JsonLimits,
) -> Result<LoadedJson<T>> {
    prepare_json(value, limits, directory.path().join(name))?.write(directory, lock, name, expected)
}

/// Strictly validated JSON bytes, prepared once before any filesystem mutation.
/// The fields are private so callers cannot substitute unvalidated bytes.
pub struct PreparedJson<T> {
    bytes: Vec<u8>,
    value: T,
    file_bytes: usize,
}

impl<T> PreparedJson<T> {
    /// The encoded length, trailing newline included: the size of the file a
    /// write installs.
    #[must_use]
    pub const fn byte_len(&self) -> usize {
        self.bytes.len()
    }

    /// Installs the prepared representation using the ordinary lease, CAS and
    /// durability checks. Preparation does not establish filesystem authority.
    ///
    /// # Errors
    /// Returns the same drift, lease and uncertain-write errors as `atomic_write`.
    pub fn write(
        self,
        directory: &PrivateDirectory,
        lock: &WriterLock,
        name: &str,
        expected: &TargetSnapshot,
    ) -> Result<LoadedJson<T>> {
        let snapshot = atomic_write(
            directory,
            lock,
            name,
            &self.bytes,
            expected,
            self.file_bytes,
        )?;
        Ok(LoadedJson {
            value: self.value,
            snapshot,
        })
    }
}

/// Encodes and strictly validates typed JSON without performing any I/O.
/// `path` supplies diagnostic context only; writing rechecks authority.
///
/// # Errors
/// Returns a pre-write error for serialization, bounds or typed-schema failures.
pub fn prepare_json<T: Serialize + DeserializeOwned>(
    value: &T,
    limits: JsonLimits,
    path: impl AsRef<Path>,
) -> Result<PreparedJson<T>> {
    let path = path.as_ref().to_path_buf();
    let mut bytes =
        serde_json::to_vec_pretty(value).map_err(|error| StorageError::InvalidData {
            path: path.clone(),
            reason: format!("value cannot be represented as JSON: {error}"),
        })?;
    bytes.push(b'\n');
    if bytes.len() > limits.file_bytes {
        return Err(StorageError::BoundsExceeded {
            path: path.clone(),
            max_bytes: limits.file_bytes,
            actual_bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        });
    }
    // Prove the authored representation round-trips through the strict reader
    // before it can become authoritative. A custom Serialize implementation
    // must not commit bytes that its corresponding Deserialize rejects.
    let decoded =
        decode_json_strict(&bytes, limits, &path).map_err(|error| StorageError::InvalidData {
            path: path.clone(),
            reason: error.to_string(),
        })?;
    Ok(PreparedJson {
        bytes,
        value: decoded,
        file_bytes: limits.file_bytes,
    })
}

struct JsonBudget {
    nodes: usize,
    limits: JsonLimits,
}

impl JsonBudget {
    fn visit<E: de::Error>(&mut self, depth: usize) -> std::result::Result<(), E> {
        if depth > self.limits.depth {
            return Err(E::custom("maximum JSON depth exceeded"));
        }
        self.nodes = self.nodes.saturating_add(1);
        if self.nodes > self.limits.nodes {
            return Err(E::custom("maximum JSON node count exceeded"));
        }
        Ok(())
    }
}

struct StrictValueSeed<'a> {
    budget: &'a mut JsonBudget,
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for StrictValueSeed<'_> {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        self.budget.visit::<D::Error>(self.depth)?;
        deserializer.deserialize_any(StrictValueVisitor {
            budget: self.budget,
            depth: self.depth,
        })
    }
}

struct StrictValueVisitor<'a> {
    budget: &'a mut JsonBudget,
    depth: usize,
}

impl<'de> Visitor<'de> for StrictValueVisitor<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded JSON value without duplicate object members")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> std::result::Result<Self::Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> std::result::Result<Self::Value, E> {
        Ok(Value::Number(Number::from(value)))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> std::result::Result<Self::Value, E> {
        Ok(Value::Number(Number::from(value)))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> std::result::Result<Self::Value, E> {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<Self::Value, E> {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> std::result::Result<Self::Value, E> {
        Ok(Value::String(value))
    }

    fn visit_none<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_unit<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_some<D>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        StrictValueSeed {
            budget: self.budget,
            depth: self.depth,
        }
        .deserialize(deserializer)
    }

    fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(StrictValueSeed {
            budget: &mut *self.budget,
            depth: self.depth + 1,
        })? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut object: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            self.budget.visit::<A::Error>(self.depth + 1)?;
            if values.contains_key(&key) {
                return Err(A::Error::custom(format!(
                    "duplicate decoded object member {key:?}"
                )));
            }
            let value = object.next_value_seed(StrictValueSeed {
                budget: &mut *self.budget,
                depth: self.depth + 1,
            })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}
