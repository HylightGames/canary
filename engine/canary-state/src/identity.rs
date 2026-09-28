//! Identity domains: projects, snapshot-local IDs, and live handles.
//!
//! Three identity domains stay apart by construction:
//!
//! - [`ProjectId`] — a stable GUID minted once per authored project and
//!   stored in the project file. Uses the UUID v4 bit layout (random
//!   122 bits) without depending on the `uuid` crate: 16 bytes come from
//!   the OS via `getrandom` and the version/variant bits are set here.
//! - Canonical snapshot-local IDs — small deterministic `u32`s assigned per
//!   encode (see [`snapshot`](crate::snapshot)). Never random, never global.
//! - Live runtime handles — entity indices, component pointers. These never
//!   enter a serialized payload; the snapshot remap table translates them
//!   at the encode boundary.
//!
//! [`ProjectRegistry`] guards the per-session invariant: one process holds
//! at most one live handle per project file, so two sessions cannot edit
//! the same project and diverge silently.

use std::collections::HashSet;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Stable identity of one authored project, minted once and stored in the
/// project file. UUID v4 bit layout over OS randomness.
///
/// Serializes as the hyphenated text form (`8-4-4-4-12`), so project files
/// stay human-readable; snapshots carry the same text inside their canonical
/// payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProjectId([u8; 16]);

impl Serialize for ProjectId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.to_hyphenated())
    }
}

impl<'de> Deserialize<'de> for ProjectId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Hyphenated;
        impl<'de> serde::de::Visitor<'de> for Hyphenated {
            type Value = ProjectId;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a hyphenated UUID v4 like 8-4-4-4-12")
            }
            fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<ProjectId, E> {
                let mut digits = Vec::with_capacity(32);
                for b in text.bytes().filter(|b| *b != b'-') {
                    let d = (b as char).to_digit(16).ok_or_else(|| {
                        E::invalid_value(serde::de::Unexpected::Char(b as char), &self)
                    })?;
                    // to_digit(16) yields 0-15: always fits u8; narrow
                    // explicitly so a widening assumption never hides here.
                    // (`d as u64` below is a lossless widening for the error.)
                    digits.push(u8::try_from(d).map_err(|_| {
                        E::invalid_value(serde::de::Unexpected::Unsigned(d as u64), &self)
                    })?);
                }
                if digits.len() != 32 {
                    return Err(E::invalid_length(digits.len(), &self));
                }
                if digits.len() != 32 {
                    return Err(E::invalid_length(digits.len(), &self));
                }
                let mut bytes = [0u8; 16];
                for (i, pair) in digits.chunks_exact(2).enumerate() {
                    bytes[i] = (pair[0] << 4) | pair[1];
                }
                Ok(ProjectId(bytes))
            }
        }
        deserializer.deserialize_str(Hyphenated)
    }
}

impl ProjectId {
    /// Mints a fresh project identity from OS randomness.
    pub fn generate() -> Result<Self, crate::StateError> {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|e| crate::StateError::Randomness(e.to_string()))?;
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Ok(Self(bytes))
    }

    /// Raw 16 bytes, for storage into envelopes that need them.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Canonical hyphenated lowercase text form, `8-4-4-4-12`.
    #[must_use]
    pub fn to_hyphenated(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(36);
        for (i, byte) in self.0.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                out.push('-');
            }
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
        out
    }
}

/// Tracks which projects this session has open. Opening the same project
/// file twice is refused, so divergent in-memory copies cannot form.
#[derive(Debug, Default)]
pub struct ProjectRegistry {
    open: HashSet<ProjectId>,
}

/// Why an open or close was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryVerdict {
    /// The project was not open; it is now recorded as open.
    Opened,
    /// The project was already open; nothing changed.
    AlreadyOpen,
    /// The project was open; it is now released.
    Closed,
    /// The project was not open; nothing changed.
    NotOpen,
}

impl ProjectRegistry {
    /// Records a project as open, or reports it already is.
    pub fn open(&mut self, id: ProjectId) -> RegistryVerdict {
        if self.open.insert(id) {
            RegistryVerdict::Opened
        } else {
            RegistryVerdict::AlreadyOpen
        }
    }

    /// Releases a project, or reports it was never open.
    pub fn close(&mut self, id: &ProjectId) -> RegistryVerdict {
        if self.open.remove(id) {
            RegistryVerdict::Closed
        } else {
            RegistryVerdict::NotOpen
        }
    }

    /// Whether the project is currently recorded as open.
    #[must_use]
    pub fn is_open(&self, id: &ProjectId) -> bool {
        self.open.contains(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_ids_look_like_uuid_v4() {
        let id = ProjectId::generate().expect("os randomness");
        let text = id.to_hyphenated();
        assert_eq!(text.len(), 36);
        assert_eq!(&text[14..15], "4");
        assert!(matches!(&text[19..20], "8" | "9" | "a" | "b"));
    }

    #[test]
    fn registry_refuses_double_open() {
        let id = ProjectId::generate().expect("os randomness");
        let mut registry = ProjectRegistry::default();
        assert_eq!(registry.open(id), RegistryVerdict::Opened);
        assert_eq!(registry.open(id), RegistryVerdict::AlreadyOpen);
        assert!(registry.is_open(&id));
        assert_eq!(registry.close(&id), RegistryVerdict::Closed);
        assert_eq!(registry.close(&id), RegistryVerdict::NotOpen);
    }
}
