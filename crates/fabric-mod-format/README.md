# fabric-mod-format

The `.fabricmod` container, its `fabric.mod.json` manifest, the version
constraint engine, Ed25519 signatures and the one-time migration from `.hmod`
and `.hnmod`.

```
my-mod.fabricmod
├── fabric.mod.json
├── icon.png
├── lib/arm64-v8a/lib<id>.so
├── assets/<id>/...
├── data/<id>/...
├── mixins/<id>.json
├── accessWidener
└── META-INF/fabricmod.sig
```

* `formatVersion` is `1`. A newer value fails with `FABRIC_FORMAT_TOO_NEW`
  instead of being misread.
* Unknown top-level manifest keys are preserved verbatim in
  `Manifest::extra`; unknown top-level container entries are preserved in
  `Container::files`.
* The reader caps entries (4096), a single entry (256 MiB) and the whole
  container (512 MiB) before allocating, rejects symlinks, encrypted entries,
  absolute paths, `..`, backslashes and duplicate names.
* The digest is SHA-256 over the manifest and every payload entry, sorted by
  canonical path, with lengths prefixed.
* Signatures are Ed25519 over that digest; trust is pinned per fingerprint with
  an explicit trust-on-first-use callback and a revocation set.

## Error codes

| Code | Meaning |
| --- | --- |
| `FABRIC_MANIFEST_INVALID` | `fabric.mod.json` is missing or invalid JSON. |
| `FABRIC_MANIFEST_TOO_LARGE` | The manifest is larger than 1 MiB. |
| `FABRIC_FORMAT_TOO_NEW` | The container or manifest schema is newer than this build. |
| `FABRIC_FORMAT_INVALID` | The file is not a `.fabricmod` container. |
| `FABRIC_ZIP_INVALID` | The archive is damaged or encrypted. |
| `FABRIC_ZIP_SLIP` | An entry path escapes the container. |
| `FABRIC_ZIP_SYMLINK` | An entry is a symbolic link. |
| `FABRIC_ZIP_DUPLICATE` | Two entries share a path (case-insensitively). |
| `FABRIC_ZIP_LIMIT` | Entry count or uncompressed size exceeds the caps. |
| `FABRIC_UNSUPPORTED_JAVA` | The mod ships Java classes only. |
| `FABRIC_MIXIN_JAVA_UNSUPPORTED` | A Java mixin class was supplied. |
| `FABRIC_ENTRYPOINT_MISSING` | A declared entrypoint has no exported Rust symbol. |
| `FABRIC_VERSION_CONFLICT` | Two ranges cannot both be satisfied. |
| `FABRIC_VERSION_INVALID` | A version or range does not parse. |
| `FABRIC_DEPENDENCY_MISSING` | A required dependency is absent. |
| `FABRIC_DEPENDENCY_BROKEN` | A selected mod is on a `breaks` list. |
| `FABRIC_CAPABILITY_MISSING` | An undeclared capability was used. |
| `FABRIC_SIGNATURE_INVALID` | The signature does not verify. |
| `FABRIC_SIGNATURE_UNTRUSTED` | The key is unknown and was not accepted. |
| `FABRIC_SIGNATURE_REVOKED` | The signing key is revoked. |
| `FABRIC_ABI_UNSUPPORTED` | Native code is not for `arm64-v8a`. |
| `FABRIC_ACCESS_WIDENER_UNRESOLVED` | A widened member does not exist. |
| `FABRIC_ICON_INVALID` | The icon is missing, not PNG, or over 512x512. |
| `FABRIC_LOCKFILE_MISMATCH` | `fabric.lock` does not match the mod set. |
| `FABRIC_DEPENDENCY_CYCLE` | The dependency graph has a cycle. |
| `FABRIC_DUPLICATE_MOD` | Two mods claim the same id or alias. |
| `FABRIC_MIGRATION_FAILED` | A legacy file could not be converted. |
| `OFFLINE_ASSET_MISSING` | A bundled game file is absent. |
| `OFFLINE_ASSET_CORRUPT` | A bundled game file fails its hash. |
| `FABRIC_SNAPSHOT_MISSING` | A mod-set snapshot does not exist. |
| `FABRIC_MODPACK_INVALID` | A `.fabricpack` is malformed. |
| `FABRIC_INVALID_ID` | A mod id is invalid or reserved. |

## Version grammar

`1.2.3`, `1.2.3-beta.1`, `1.2.x`, `*`, `[1.0,2.0)`, `(,2.0]`, `[1.2.3]`,
`>=1.2 <2.0`, `>1.2`, `<=1.2`, `<1.2`, `=1.2`, `~1.2.3`, `^1.2.3`, joined
with `||` for alternatives. SemVer 2.0 prerelease precedence is used; build
metadata is ignored.
