//! Access widener v1 and v2 parsing and validation.
//!
//! A widener is text:
//!
//! ```text
//! accessWidener v2 named
//! accessible method net/minecraft/world/World getBlockState (Lnet/minecraft/util/math/BlockPos;)Lnet/minecraft/block/BlockState;
//! ```
//!
//! The loader applies an entry only when the target surface exists; otherwise
//! it reports [`ErrorCode::AccessWidenerUnresolved`] and names the member.

use crate::error::{ErrorCode, FabricError, Result};
use serde::{Deserialize, Serialize};

/// One widened member.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessEntry {
    pub access: Access,
    pub kind: MemberKind,
    pub owner: String,
    pub name: String,
    /// The JVM descriptor, empty for a class.
    pub descriptor: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    Accessible,
    Extendable,
    Mutable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemberKind {
    Class,
    Method,
    Field,
}

/// A parsed widener.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessWidener {
    pub version: u32,
    pub namespace: String,
    pub entries: Vec<AccessEntry>,
}

impl AccessWidener {
    /// Parses text, rejecting an unknown version and malformed names.
    pub fn parse(text: &str) -> Result<Self> {
        let mut lines = text.lines().map(str::trim);
        let header = loop {
            match lines.next() {
                Some(line) if line.is_empty() || line.starts_with('#') => continue,
                Some(line) => break line,
                None => {
                    return Err(FabricError::new(
                        ErrorCode::VersionInvalid,
                        "the access widener is empty",
                    ));
                }
            }
        };
        let parts: Vec<&str> = header.split_whitespace().collect();
        let [kind, version, namespace] = parts.as_slice() else {
            return Err(FabricError::with_detail(
                ErrorCode::FormatInvalid,
                "the access widener header must be `accessWidener v1|v2 <namespace>`",
                header.to_owned(),
            ));
        };
        if !kind.eq_ignore_ascii_case("accessWidener") {
            return Err(FabricError::with_detail(
                ErrorCode::FormatInvalid,
                "the access widener header is not recognised",
                header.to_owned(),
            ));
        }
        let version = match *version {
            "v1" => 1,
            "v2" => 2,
            other => {
                return Err(FabricError::with_detail(
                    ErrorCode::FormatTooNew,
                    "this build understands access widener v1 and v2",
                    other.to_owned(),
                ));
            }
        };
        if namespace.is_empty() || namespace.contains('/') {
            return Err(FabricError::with_detail(
                ErrorCode::FormatInvalid,
                "the namespace is not a plain name",
                namespace.to_string(),
            ));
        }
        let mut entries = Vec::new();
        for (number, line) in lines.enumerate() {
            let line = match line.split('#').next().unwrap_or_default().trim() {
                "" => continue,
                line => line,
            };
            let fields: Vec<&str> = line.split_whitespace().collect();
            let access = match fields.first().copied() {
                Some("accessible") => Access::Accessible,
                Some("extendable") => Access::Extendable,
                Some("mutable") => Access::Mutable,
                other => {
                    return Err(FabricError::with_detail(
                        ErrorCode::FormatInvalid,
                        "an access widener line must start with accessible, extendable or mutable",
                        format!("line {}: {other:?}", number + 2),
                    ));
                }
            };
            let (kind, owner, name, descriptor) = match fields.as_slice() {
                [_, "class", owner] => (MemberKind::Class, *owner, String::new(), String::new()),
                [_, "method", owner, name, descriptor] => {
                    (MemberKind::Method, *owner, (*name).to_owned(), (*descriptor).to_owned())
                }
                [_, "field", owner, name, descriptor] => {
                    (MemberKind::Field, *owner, (*name).to_owned(), (*descriptor).to_owned())
                }
                _ => {
                    return Err(FabricError::with_detail(
                        ErrorCode::FormatInvalid,
                        "an access widener line has the wrong number of fields",
                        format!("line {}: {line}", number + 2),
                    ));
                }
            };
            check_internal(owner).map_err(|e| {
                FabricError::with_detail(
                    ErrorCode::FormatInvalid,
                    "an access widener class name is invalid",
                    format!("line {}: {}", number + 2, e.message),
                )
            })?;
            if !name.is_empty() && (name.contains('/') || name.contains('.') || name.contains(';')) {
                return Err(FabricError::with_detail(
                    ErrorCode::FormatInvalid,
                    "an access widener member name is invalid",
                    format!("line {}: {name}", number + 2),
                ));
            }
            if version == 1 && !descriptor.is_empty() {
                // v1 accepted a descriptor; it is harmless, keep it.
            }
            entries.push(AccessEntry {
                access,
                kind,
                owner: owner.to_owned(),
                name,
                descriptor,
            });
        }
        Ok(Self {
            version,
            namespace: namespace.to_string(),
            entries,
        })
    }

    /// The entries whose target does not exist in this build. `exists` is
    /// given the owner and, for members, the name and descriptor.
    pub fn unresolved<'a>(
        &'a self,
        mut exists: impl FnMut(&str, MemberKind, &str, &str) -> bool,
    ) -> Vec<&'a AccessEntry> {
        self.entries
            .iter()
            .filter(|entry| !exists(&entry.owner, entry.kind, &entry.name, &entry.descriptor))
            .collect()
    }

    /// Fails with the first unresolved member named.
    pub fn apply(&self, exists: impl FnMut(&str, MemberKind, &str, &str) -> bool) -> Result<()> {
        if let Some(entry) = self.unresolved(exists).first() {
            let member = if entry.name.is_empty() {
                entry.owner.clone()
            } else {
                format!("{} {}{}", entry.owner, entry.name, entry.descriptor)
            };
            return Err(FabricError::with_detail(
                ErrorCode::AccessWidenerUnresolved,
                "the access widener names a member this version does not have",
                member,
            ));
        }
        Ok(())
    }
}

fn check_internal(name: &str) -> Result<()> {
    if name.is_empty()
        || name.starts_with('/')
        || name.ends_with('/')
        || name.contains("..")
        || name.contains('\\')
        || name.contains('.')
        || name.contains(';')
        || name.len() > 512
    {
        return Err(FabricError::new(ErrorCode::FormatInvalid, "invalid class name"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDENER: &str = r#"
# a comment
accessWidener v2 named
accessible class net/minecraft/world/World
accessible method net/minecraft/world/World getBlockState (Lnet/minecraft/util/math/BlockPos;)Lnet/minecraft/block/BlockState;
mutable field net/minecraft/world/World ticks I
extendable method net/minecraft/entity/Entity tick ()V
"#;

    #[test]
    fn parses_v2_and_v1() {
        let w = AccessWidener::parse(WIDENER).unwrap();
        assert_eq!(w.version, 2);
        assert_eq!(w.entries.len(), 4);
        assert_eq!(w.entries[1].kind, MemberKind::Method);
        let v1 = AccessWidener::parse("accessWidener v1 named\naccessible class a/B\n").unwrap();
        assert_eq!(v1.version, 1);
    }

    #[test]
    fn unresolved_members_are_named() {
        let w = AccessWidener::parse(WIDENER).unwrap();
        let missing = w
            .unresolved(|owner, kind, name, _| owner == "net/minecraft/world/World" && kind == MemberKind::Class || name == "getBlockState")
            ;
        assert_eq!(missing.len(), 2, "{missing:?}");
        let err = w.apply(|owner, _, name, _| owner == "net/minecraft/world/World" && name != "getBlockState").unwrap_err();
        assert_eq!(err.code, ErrorCode::AccessWidenerUnresolved);
        assert!(err.detail.unwrap().contains("getBlockState"));
    }

    #[test]
    fn hostile_input_never_panics() {
        for text in [
            "",
            "accessWidener",
            "accessWidener v3 named",
            "accessWidener v2",
            "accessWidener v2 named\nopen class a/B",
            "accessWidener v2 named\naccessible class ../../etc/passwd",
            "accessWidener v2 named\naccessible method a/B",
            "accessWidener v2 named\naccessible method a/B m",
        ] {
            let _ = AccessWidener::parse(text);
        }
    }
}
