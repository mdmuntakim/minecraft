//! Stable error codes for every failure a mod container or a mod set can produce.
//!
//! A code never changes once released. The user-facing sentence is what a
//! launcher shows; the developer detail is logged, never shown as the only
//! explanation. The table is documented in `README.md`.

use std::fmt;

/// The stable identifier of a failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ErrorCode {
    /// The manifest is missing, not UTF-8 or not valid JSON.
    ManifestInvalid,
    /// The manifest is larger than the 1 MiB limit.
    ManifestTooLarge,
    /// The container declares a format version this build does not know.
    FormatTooNew,
    /// The container's layout is invalid (version 0, missing manifest entry).
    FormatInvalid,
    /// The file is not a readable zip archive.
    ZipInvalid,
    /// An entry name would escape the container (zip slip).
    ZipSlip,
    /// An entry is a symbolic link.
    ZipSymlink,
    /// Two entries normalise to the same path.
    ZipDuplicate,
    /// The entry count or uncompressed size exceeds a cap.
    ZipLimit,
    /// A code mod ships Java `.class` files only.
    UnsupportedJava,
    /// A Java mixin class was supplied instead of a Rust mixin config.
    MixinJavaUnsupported,
    /// A declared entrypoint has no matching Rust symbol in the native library.
    EntrypointMissing,
    /// Two version ranges cannot be satisfied together.
    VersionConflict,
    /// A version range or version literal does not parse.
    VersionInvalid,
    /// A required dependency is absent.
    DependencyMissing,
    /// A mod is on the `breaks` list of a selected mod.
    DependencyBroken,
    /// A mod uses a capability it did not declare.
    CapabilityMissing,
    /// The Ed25519 signature does not verify.
    SignatureInvalid,
    /// The signing key is not pinned and trust-on-first-use was declined.
    SignatureUntrusted,
    /// A key is pinned as revoked.
    SignatureRevoked,
    /// The mod's native library is for another ABI.
    AbiUnsupported,
    /// An access widener names a member this build does not expose.
    AccessWidenerUnresolved,
    /// The icon is not a PNG within the size limits.
    IconInvalid,
    /// The lockfile and the discovered mod set disagree.
    LockfileMismatch,
    /// The dependency graph has a cycle.
    DependencyCycle,
    /// Two mods claim the same id or alias.
    DuplicateMod,
    /// A legacy `.hmod`/`.hnmod` could not be migrated.
    MigrationFailed,
    /// A bundled offline asset is not in the APK.
    OfflineAssetMissing,
    /// A bundled offline asset does not match its manifest hash.
    OfflineAssetCorrupt,
    /// A requested mod set snapshot does not exist.
    SnapshotMissing,
    /// A `.fabricpack` is malformed.
    ModpackInvalid,
    /// A mod id is not `[a-z0-9_-]` or is reserved.
    InvalidId,
}

impl ErrorCode {
    /// The code as written in logs, manifests and the README table.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ManifestInvalid => "FABRIC_MANIFEST_INVALID",
            Self::ManifestTooLarge => "FABRIC_MANIFEST_TOO_LARGE",
            Self::FormatTooNew => "FABRIC_FORMAT_TOO_NEW",
            Self::FormatInvalid => "FABRIC_FORMAT_INVALID",
            Self::ZipInvalid => "FABRIC_ZIP_INVALID",
            Self::ZipSlip => "FABRIC_ZIP_SLIP",
            Self::ZipSymlink => "FABRIC_ZIP_SYMLINK",
            Self::ZipDuplicate => "FABRIC_ZIP_DUPLICATE",
            Self::ZipLimit => "FABRIC_ZIP_LIMIT",
            Self::UnsupportedJava => "FABRIC_UNSUPPORTED_JAVA",
            Self::MixinJavaUnsupported => "FABRIC_MIXIN_JAVA_UNSUPPORTED",
            Self::EntrypointMissing => "FABRIC_ENTRYPOINT_MISSING",
            Self::VersionConflict => "FABRIC_VERSION_CONFLICT",
            Self::VersionInvalid => "FABRIC_VERSION_INVALID",
            Self::DependencyMissing => "FABRIC_DEPENDENCY_MISSING",
            Self::DependencyBroken => "FABRIC_DEPENDENCY_BROKEN",
            Self::CapabilityMissing => "FABRIC_CAPABILITY_MISSING",
            Self::SignatureInvalid => "FABRIC_SIGNATURE_INVALID",
            Self::SignatureUntrusted => "FABRIC_SIGNATURE_UNTRUSTED",
            Self::SignatureRevoked => "FABRIC_SIGNATURE_REVOKED",
            Self::AbiUnsupported => "FABRIC_ABI_UNSUPPORTED",
            Self::AccessWidenerUnresolved => "FABRIC_ACCESS_WIDENER_UNRESOLVED",
            Self::IconInvalid => "FABRIC_ICON_INVALID",
            Self::LockfileMismatch => "FABRIC_LOCKFILE_MISMATCH",
            Self::DependencyCycle => "FABRIC_DEPENDENCY_CYCLE",
            Self::DuplicateMod => "FABRIC_DUPLICATE_MOD",
            Self::MigrationFailed => "FABRIC_MIGRATION_FAILED",
            Self::OfflineAssetMissing => "OFFLINE_ASSET_MISSING",
            Self::OfflineAssetCorrupt => "OFFLINE_ASSET_CORRUPT",
            Self::SnapshotMissing => "FABRIC_SNAPSHOT_MISSING",
            Self::ModpackInvalid => "FABRIC_MODPACK_INVALID",
            Self::InvalidId => "FABRIC_INVALID_ID",
        }
    }

    /// A sentence for the player, in the imperative.
    pub const fn sentence(self) -> &'static str {
        match self {
            Self::ManifestInvalid => "The mod's fabric.mod.json is missing or is not valid JSON.",
            Self::ManifestTooLarge => "The mod's fabric.mod.json is larger than 1 MiB.",
            Self::FormatTooNew => "This mod needs a newer version of the game to load.",
            Self::FormatInvalid => "This file is not a .fabricmod container.",
            Self::ZipInvalid => "The mod file is damaged and cannot be read.",
            Self::ZipSlip => "The mod tries to write outside its own folder.",
            Self::ZipSymlink => "The mod contains a symbolic link, which is not allowed.",
            Self::ZipDuplicate => "The mod contains two files with the same path.",
            Self::ZipLimit => "The mod contains too many files or is too large.",
            Self::UnsupportedJava => {
                "This Fabric mod ships Java code only, and this build runs Rust mods."
            }
            Self::MixinJavaUnsupported => {
                "This mod ships Java mixin classes, which this build cannot run."
            }
            Self::EntrypointMissing => {
                "This mod declares an entrypoint its library does not export."
            }
            Self::VersionConflict => "Two mods need versions that cannot both be installed.",
            Self::VersionInvalid => "The mod declares a version range that cannot be parsed.",
            Self::DependencyMissing => "A mod needs another mod that is not installed.",
            Self::DependencyBroken => "A mod cannot be installed together with another one.",
            Self::CapabilityMissing => "A mod uses a capability it did not declare.",
            Self::SignatureInvalid => "The mod's signature does not match its contents.",
            Self::SignatureUntrusted => "The mod is signed by an unknown key.",
            Self::SignatureRevoked => "The mod's signing key has been revoked.",
            Self::AbiUnsupported => "The mod's native code is for a different CPU.",
            Self::AccessWidenerUnresolved => {
                "The mod widens a member that does not exist in this version."
            }
            Self::IconInvalid => "The mod's icon is not a valid PNG.",
            Self::LockfileMismatch => "The installed mods do not match fabric.lock.",
            Self::DependencyCycle => "The mods depend on each other in a circle.",
            Self::DuplicateMod => "Two mods have the same id.",
            Self::MigrationFailed => "An old .hmod or .hnmod file could not be converted.",
            Self::OfflineAssetMissing => "A game file is missing from this installation.",
            Self::OfflineAssetCorrupt => "A game file is damaged.",
            Self::SnapshotMissing => "That mod set snapshot does not exist.",
            Self::ModpackInvalid => "The modpack file is damaged.",
            Self::InvalidId => "A mod id contains characters that are not allowed.",
        }
    }
}

impl ErrorCode {
    /// Parses a code from its written form.
    pub fn parse(text: &str) -> Option<Self> {
        ALL_CODES.iter().copied().find(|code| code.as_str() == text)
    }
}

impl serde::Serialize for ErrorCode {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for ErrorCode {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let text = <std::string::String as serde::Deserialize>::deserialize(deserializer)?;
        Self::parse(&text)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown error code {text}")))
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A failure with a stable code, a player-facing sentence and a detail line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FabricError {
    pub code: ErrorCode,
    pub message: String,
    pub detail: Option<String>,
}

impl FabricError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: None,
        }
    }

    pub fn with_detail(
        code: ErrorCode,
        message: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            detail: Some(detail.into()),
        }
    }

    /// The line a log file carries: `CODE: message (detail)`.
    pub fn log_line(&self) -> String {
        match &self.detail {
            Some(detail) => format!("{}: {} ({detail})", self.code, self.message),
            None => format!("{}: {}", self.code, self.message),
        }
    }
}

impl fmt::Display for FabricError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)?;
        if let Some(detail) = &self.detail {
            write!(f, " ({detail})")?;
        }
        Ok(())
    }
}

impl std::error::Error for FabricError {}

/// Shorthand for results carrying a [`FabricError`].
pub type Result<T> = std::result::Result<T, FabricError>;

/// The error codes in the order the README lists them.
pub const ALL_CODES: &[ErrorCode] = &[
    ErrorCode::ManifestInvalid,
    ErrorCode::ManifestTooLarge,
    ErrorCode::FormatTooNew,
    ErrorCode::FormatInvalid,
    ErrorCode::ZipInvalid,
    ErrorCode::ZipSlip,
    ErrorCode::ZipSymlink,
    ErrorCode::ZipDuplicate,
    ErrorCode::ZipLimit,
    ErrorCode::UnsupportedJava,
    ErrorCode::MixinJavaUnsupported,
    ErrorCode::EntrypointMissing,
    ErrorCode::VersionConflict,
    ErrorCode::VersionInvalid,
    ErrorCode::DependencyMissing,
    ErrorCode::DependencyBroken,
    ErrorCode::CapabilityMissing,
    ErrorCode::SignatureInvalid,
    ErrorCode::SignatureUntrusted,
    ErrorCode::SignatureRevoked,
    ErrorCode::AbiUnsupported,
    ErrorCode::AccessWidenerUnresolved,
    ErrorCode::IconInvalid,
    ErrorCode::LockfileMismatch,
    ErrorCode::DependencyCycle,
    ErrorCode::DuplicateMod,
    ErrorCode::MigrationFailed,
    ErrorCode::OfflineAssetMissing,
    ErrorCode::OfflineAssetCorrupt,
    ErrorCode::SnapshotMissing,
    ErrorCode::ModpackInvalid,
    ErrorCode::InvalidId,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_and_spelled() {
        let mut seen = std::collections::BTreeSet::new();
        for code in ALL_CODES {
            assert!(seen.insert(code.as_str()), "duplicate {code}");
            assert!(
                code.as_str()
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c == '_'),
                "{code}"
            );
            assert!(!code.sentence().is_empty());
        }
    }

    #[test]
    fn display_includes_detail() {
        let e = FabricError::with_detail(ErrorCode::ZipSlip, "bad path", "../x");
        assert_eq!(e.to_string(), "FABRIC_ZIP_SLIP: bad path (../x)");
        assert!(e.log_line().contains("FABRIC_ZIP_SLIP"));
    }
}
