//! Semantic versions and the version-constraint grammar Fabric uses.
//!
//! Supported syntax:
//!
//! * exact SemVer 2.0 versions, with prerelease ordering (`1.2.3-beta.2`);
//! * Maven ranges: `[1.0,2.0)`, `(,2.0]`, `[1.0]`;
//! * wildcards: `*`, `1.x`, `1.2.x`;
//! * comparators: `>=1.2`, `>1.2`, `<=1.2`, `<1.2`, `=1.2`;
//! * tilde and caret: `~1.2.3`, `^1.2.3`;
//! * alternatives with `||`, constraints separated by spaces or commas.
//!
//! The implementation never panics on attacker-controlled input: every parse
//! failure is a [`FabricError`] with [`ErrorCode::VersionInvalid`].

use crate::error::{ErrorCode, FabricError, Result};
use serde::{Deserialize, Serialize};
use std::{cmp::Ordering, fmt, str::FromStr};

/// A semantic version, with SemVer 2.0 precedence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// Dot-separated prerelease identifiers; empty for a release.
    pub pre: Vec<String>,
    /// Build metadata; never part of the comparison.
    pub build: Option<String>,
}

impl Version {
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
            pre: Vec::new(),
            build: None,
        }
    }

    /// Parses a full `major.minor.patch[-pre][+build]` version.
    pub fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        if text.is_empty() {
            return Err(invalid("empty version", text));
        }
        let (without_build, build) = match text.split_once('+') {
            Some((v, b)) => (v, Some(b.to_owned())),
            None => (text, None),
        };
        if let Some(b) = &build {
            if b.is_empty() || !b.split('.').all(|p| valid_identifier(p, false)) {
                return Err(invalid("invalid build metadata", text));
            }
        }
        let (core, pre) = match without_build.split_once('-') {
            Some((c, p)) => {
                if p.is_empty() {
                    return Err(invalid("empty prerelease", text));
                }
                let parts: Vec<String> = p.split('.').map(str::to_owned).collect();
                for part in &parts {
                    if !valid_identifier(part, true) {
                        return Err(invalid("invalid prerelease identifier", text));
                    }
                    if part.len() > 1
                        && part.chars().all(|c| c.is_ascii_digit())
                        && part.starts_with('0')
                    {
                        return Err(invalid("prerelease number has a leading zero", text));
                    }
                }
                (c, parts)
            }
            None => (without_build, Vec::new()),
        };
        let mut numbers = core.split('.');
        let major = parse_number(numbers.next(), text)?;
        let minor = parse_number(numbers.next(), text)?;
        let patch = parse_number(numbers.next(), text)?;
        if numbers.next().is_some() {
            return Err(invalid("too many version components", text));
        }
        Ok(Self {
            major,
            minor,
            patch,
            pre,
            build,
        })
    }

    /// The version with prerelease and build metadata dropped.
    pub fn release(&self) -> Self {
        Self::new(self.major, self.minor, self.patch)
    }

    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

fn parse_number(text: Option<&str>, whole: &str) -> Result<u64> {
    let Some(text) = text else {
        return Err(invalid("a version needs major.minor.patch", whole));
    };
    if text.is_empty() || !text.chars().all(|c| c.is_ascii_digit()) {
        return Err(invalid("a version component is not a number", whole));
    }
    if text.len() > 1 && text.starts_with('0') {
        return Err(invalid("a version number has a leading zero", whole));
    }
    text.parse()
        .map_err(|_| invalid("a version component is too large", whole))
}

fn valid_identifier(part: &str, prerelease: bool) -> bool {
    !part.is_empty()
        && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && (!prerelease || part.chars().any(|c| c.is_ascii_alphanumeric()))
}

fn invalid(message: &str, input: &str) -> FabricError {
    FabricError::with_detail(ErrorCode::VersionInvalid, message, input.to_owned())
}

fn compare_identifier(a: &str, b: &str) -> Ordering {
    let na = a.chars().all(|c| c.is_ascii_digit());
    let nb = b.chars().all(|c| c.is_ascii_digit());
    match (na, nb) {
        (true, true) => {
            // Numeric identifiers are compared as numbers; strip to avoid
            // overflow on hostile input.
            let a = a.trim_start_matches('0');
            let b = b.trim_start_matches('0');
            a.len().cmp(&b.len()).then_with(|| a.cmp(b))
        }
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => a.cmp(b),
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.major
            .cmp(&other.major)
            .then_with(|| self.minor.cmp(&other.minor))
            .then_with(|| self.patch.cmp(&other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => {
                    for (a, b) in self.pre.iter().zip(&other.pre) {
                        let ord = compare_identifier(a, b);
                        if ord != Ordering::Equal {
                            return ord;
                        }
                    }
                    self.pre.len().cmp(&other.pre.len())
                }
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            write!(f, "-{}", self.pre.join("."))?;
        }
        if let Some(build) = &self.build {
            write!(f, "+{build}")?;
        }
        Ok(())
    }
}

impl FromStr for Version {
    type Err = FabricError;
    fn from_str(s: &str) -> Result<Self> {
        Self::parse(s)
    }
}

/// A partial version from a constraint: missing components are `None`.
#[derive(Clone, Debug, Default)]
struct Partial {
    major: Option<u64>,
    minor: Option<u64>,
    patch: Option<u64>,
    pre: Vec<String>,
}

impl Partial {
    fn is_any(&self) -> bool {
        self.major.is_none()
    }

    /// The lowest concrete version this partial names.
    fn lower(&self) -> Version {
        Version {
            major: self.major.unwrap_or(0),
            minor: self.minor.unwrap_or(0),
            patch: self.patch.unwrap_or(0),
            pre: self.pre.clone(),
            build: None,
        }
    }

    /// The exclusive upper bound of the prefix (`1.2` -> `1.3.0`).
    fn next(&self) -> Option<Version> {
        match (self.major, self.minor, self.patch) {
            (Some(major), None, _) => Some(Version::new(major.saturating_add(1), 0, 0)),
            (Some(major), Some(minor), None) => {
                Some(Version::new(major, minor.saturating_add(1), 0))
            }
            (Some(major), Some(minor), Some(_)) if !self.pre.is_empty() => {
                // A prerelease has an exact lower bound; its prefix end is the
                // next patch for `=` on a complete version.
                Some(Version::new(major, minor, 0))
            }
            _ => None,
        }
    }
}

fn parse_partial(text: &str) -> Result<Partial> {
    let text = text.trim();
    if text.is_empty() {
        return Err(invalid("empty version constraint", text));
    }
    if matches!(text, "*" | "x" | "X") {
        return Ok(Partial::default());
    }
    let (core, pre_text) = match text.split_once('-') {
        Some((c, p)) => (c, Some(p.to_owned())),
        None => (text, None),
    };
    let pre = match &pre_text {
        Some(p) => {
            if p.is_empty() || !p.split('.').all(|i| valid_identifier(i, true)) {
                return Err(invalid("invalid prerelease", text));
            }
            p.split('.').map(str::to_owned).collect()
        }
        None => Vec::new(),
    };
    let mut major = None;
    let mut minor = None;
    let mut patch = None;
    for (i, part) in core.split('.').enumerate() {
        if i > 2 {
            return Err(invalid("too many version components", text));
        }
        if matches!(part, "*" | "x" | "X") {
            if i + 1 != core.split('.').count() {
                return Err(invalid("a wildcard must be the last component", text));
            }
            break;
        }
        if part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()) {
            return Err(invalid("a version component is not a number", text));
        }
        if part.len() > 1 && part.starts_with('0') {
            return Err(invalid("a version number has a leading zero", text));
        }
        let n: u64 = part
            .parse()
            .map_err(|_| invalid("a version component is too large", text))?;
        match i {
            0 => major = Some(n),
            1 => minor = Some(n),
            _ => patch = Some(n),
        }
    }
    Ok(Partial {
        major,
        minor,
        patch,
        pre,
    })
}

/// One side of an interval.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bound {
    pub version: Version,
    pub inclusive: bool,
}

/// A single interval of versions, possibly unbounded on either side.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Interval {
    pub lower: Option<Bound>,
    pub upper: Option<Bound>,
}

impl Interval {
    pub fn any() -> Self {
        Self::default()
    }

    pub fn exact(version: Version) -> Self {
        Self {
            lower: Some(Bound {
                version: version.clone(),
                inclusive: true,
            }),
            upper: Some(Bound {
                version,
                inclusive: true,
            }),
        }
    }

    fn is_empty(&self) -> bool {
        match (&self.lower, &self.upper) {
            (Some(lo), Some(hi)) => match lo.version.cmp(&hi.version) {
                Ordering::Greater => true,
                Ordering::Equal => !(lo.inclusive && hi.inclusive),
                Ordering::Less => false,
            },
            _ => false,
        }
    }

    pub fn contains(&self, version: &Version) -> bool {
        if let Some(lo) = &self.lower {
            match version.cmp(&lo.version) {
                Ordering::Less => return false,
                Ordering::Equal if !lo.inclusive => return false,
                _ => {}
            }
        }
        if let Some(hi) = &self.upper {
            match version.cmp(&hi.version) {
                Ordering::Greater => return false,
                Ordering::Equal if !hi.inclusive => return false,
                _ => {}
            }
        }
        true
    }

    /// Intersection, or `None` when the two share no version.
    pub fn intersect(&self, other: &Self) -> Option<Self> {
        let lower = match (&self.lower, &other.lower) {
            (None, None) => None,
            (Some(a), None) => Some(a.clone()),
            (None, Some(b)) => Some(b.clone()),
            (Some(a), Some(b)) => Some(match a.version.cmp(&b.version) {
                Ordering::Greater => a.clone(),
                Ordering::Less => b.clone(),
                Ordering::Equal => Bound {
                    version: a.version.clone(),
                    inclusive: a.inclusive && b.inclusive,
                },
            }),
        };
        let upper = match (&self.upper, &other.upper) {
            (None, None) => None,
            (Some(a), None) => Some(a.clone()),
            (None, Some(b)) => Some(b.clone()),
            (Some(a), Some(b)) => Some(match a.version.cmp(&b.version) {
                Ordering::Less => a.clone(),
                Ordering::Greater => b.clone(),
                Ordering::Equal => Bound {
                    version: a.version.clone(),
                    inclusive: a.inclusive && b.inclusive,
                },
            }),
        };
        let merged = Self { lower, upper };
        (!merged.is_empty()).then_some(merged)
    }
}

/// A version range: one or more alternative intervals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionRange {
    raw: String,
    alternatives: Vec<Interval>,
}

impl VersionRange {
    /// Any version, for `*`.
    pub fn any() -> Self {
        Self {
            raw: "*".into(),
            alternatives: vec![Interval::any()],
        }
    }

    /// Parses the Fabric/Maven grammar. Never panics.
    pub fn parse(text: &str) -> Result<Self> {
        let raw = text.trim().to_owned();
        if raw.is_empty() {
            return Err(invalid("empty version range", text));
        }
        let mut alternatives = Vec::new();
        for alt in raw.split("||") {
            let alt = alt.trim();
            if alt.is_empty() {
                return Err(invalid("empty alternative in version range", text));
            }
            let interval = parse_alternative(alt)?;
            if interval.is_empty() {
                // An empty alternative is legal syntax but matches nothing;
                // keep it so `intersects` is false rather than erroring.
                alternatives.push(interval);
            } else {
                alternatives.push(interval);
            }
        }
        if alternatives.is_empty() {
            return Err(invalid("empty version range", text));
        }
        Ok(Self { raw, alternatives })
    }

    /// The text this range was parsed from.
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    pub fn contains(&self, version: &Version) -> bool {
        self.alternatives.iter().any(|a| a.contains(version))
    }

    /// Whether some version satisfies both ranges.
    pub fn intersects(&self, other: &Self) -> bool {
        self.alternatives
            .iter()
            .any(|a| other.alternatives.iter().any(|b| a.intersect(b).is_some()))
    }

    /// A witness version in the intersection, when one exists. Used by the
    /// conflict diagnostic to show what would have to be installed.
    pub fn intersection_witness(&self, other: &Self) -> Option<Version> {
        for a in &self.alternatives {
            for b in &other.alternatives {
                if let Some(merged) = a.intersect(b) {
                    if let Some(lo) = &merged.lower {
                        return Some(lo.version.clone());
                    }
                    if let Some(hi) = &merged.upper {
                        return Some(hi.version.clone());
                    }
                    return Some(Version::new(0, 0, 0));
                }
            }
        }
        None
    }

    /// The bounds, for a diagnostic. Alternatives are ORed.
    pub fn alternatives(&self) -> &[Interval] {
        &self.alternatives
    }
}

fn parse_alternative(text: &str) -> Result<Interval> {
    let trimmed = text.trim();
    if trimmed.starts_with('[') || trimmed.starts_with('(') {
        return parse_maven(trimmed);
    }
    let mut interval = Some(Interval::any());
    for token in trimmed
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
    {
        let token = token.trim();
        let (op, rest) = if let Some(rest) = token.strip_prefix(">=") {
            (">=", rest)
        } else if let Some(rest) = token.strip_prefix("<=") {
            ("<=", rest)
        } else if let Some(rest) = token.strip_prefix('>') {
            (">", rest)
        } else if let Some(rest) = token.strip_prefix('<') {
            ("<", rest)
        } else if let Some(rest) = token.strip_prefix('~') {
            ("~", rest)
        } else if let Some(rest) = token.strip_prefix('^') {
            ("^", rest)
        } else if let Some(rest) = token.strip_prefix('=') {
            ("=", rest)
        } else {
            ("=", token)
        };
        let partial = parse_partial(rest)?;
        let piece = constraint_interval(op, &partial, token)?;
        interval = interval.and_then(|current| current.intersect(&piece));
        if interval.is_none() {
            return Ok(empty_interval());
        }
    }
    Ok(interval.unwrap_or_else(Interval::any))
}

fn empty_interval() -> Interval {
    // [1.0.0, 1.0.0) is empty by construction.
    Interval {
        lower: Some(Bound {
            version: Version::new(1, 0, 0),
            inclusive: true,
        }),
        upper: Some(Bound {
            version: Version::new(1, 0, 0),
            inclusive: false,
        }),
    }
}

fn parse_maven(text: &str) -> Result<Interval> {
    let lower_inclusive = text.starts_with('[');
    let last = text.chars().last().unwrap_or(')');
    let upper_inclusive = match last {
        ']' => true,
        ')' => false,
        _ => return Err(invalid("a Maven range must end with ] or )", text)),
    };
    let body = &text[1..text.len().saturating_sub(1)];
    let parts: Vec<&str> = body.split(',').map(str::trim).collect();
    match parts.as_slice() {
        [single] if !single.is_empty() => {
            if !lower_inclusive || !upper_inclusive {
                return Err(invalid("a single-version Maven range must be [x]", text));
            }
            Ok(Interval::exact(Version::parse(single)?))
        }
        [lo, hi] => {
            // Maven bounds may be partial (`[1.0,2.0)`); a missing component
            // is zero, the same reading `1.0` has elsewhere.
            let bound = |text: &str| -> Result<Version> {
                let partial = parse_partial(text)?;
                if partial.is_any() {
                    return Err(invalid("a Maven bound cannot be a wildcard", text));
                }
                Ok(partial.lower())
            };
            let lower = if lo.is_empty() {
                None
            } else {
                Some(Bound {
                    version: bound(lo)?,
                    inclusive: lower_inclusive,
                })
            };
            let upper = if hi.is_empty() {
                None
            } else {
                Some(Bound {
                    version: bound(hi)?,
                    inclusive: upper_inclusive,
                })
            };
            Ok(Interval { lower, upper })
        }
        _ => Err(invalid("a Maven range has one or two bounds", text)),
    }
}

fn constraint_interval(op: &str, partial: &Partial, token: &str) -> Result<Interval> {
    if partial.is_any() {
        return match op {
            // `*`, `=*` and `^*` mean any version; a bound on a wildcard is
            // meaningless and is rejected rather than silently widened.
            "=" | "~" | "^" => Ok(Interval::any()),
            _ => Err(invalid("a wildcard cannot be used with > or <", token)),
        };
    }
    let lower = partial.lower();
    let next = partial.next();
    let exact = matches!(
        (partial.major, partial.minor, partial.patch),
        (Some(_), Some(_), Some(_))
    );

    match op {
        ">=" => Ok(Interval {
            lower: Some(Bound {
                version: lower,
                inclusive: true,
            }),
            upper: None,
        }),
        ">" => {
            let version = if exact {
                lower
            } else {
                next.ok_or_else(|| invalid("bad bound", token))?
            };
            Ok(Interval {
                lower: Some(Bound {
                    version,
                    inclusive: false,
                }),
                upper: None,
            })
        }
        "<=" => {
            let upper = if exact {
                Bound {
                    version: lower,
                    inclusive: true,
                }
            } else {
                Bound {
                    version: next.ok_or_else(|| invalid("bad bound", token))?,
                    inclusive: false,
                }
            };
            Ok(Interval {
                lower: None,
                upper: Some(upper),
            })
        }
        "<" => Ok(Interval {
            lower: None,
            upper: Some(Bound {
                version: lower,
                inclusive: false,
            }),
        }),
        "=" => {
            if exact {
                Ok(Interval::exact(lower))
            } else {
                Ok(Interval {
                    lower: Some(Bound {
                        version: lower,
                        inclusive: true,
                    }),
                    upper: next.map(|v| Bound {
                        version: v,
                        inclusive: false,
                    }),
                })
            }
        }
        "~" => {
            let upper = match (partial.major, partial.minor) {
                (Some(major), None) => Version::new(major.saturating_add(1), 0, 0),
                (Some(major), Some(minor)) => Version::new(major, minor.saturating_add(1), 0),
                _ => return Err(invalid("bad tilde constraint", token)),
            };
            Ok(Interval {
                lower: Some(Bound {
                    version: lower,
                    inclusive: true,
                }),
                upper: Some(Bound {
                    version: upper,
                    inclusive: false,
                }),
            })
        }
        "^" => {
            let upper = match (partial.major, partial.minor, partial.patch) {
                (Some(0), Some(0), Some(patch)) => Version::new(0, 0, patch.saturating_add(1)),
                (Some(0), Some(minor), _) => Version::new(0, minor.saturating_add(1), 0),
                (Some(major), _, _) => Version::new(major.saturating_add(1), 0, 0),
                _ => return Err(invalid("bad caret constraint", token)),
            };
            Ok(Interval {
                lower: Some(Bound {
                    version: lower,
                    inclusive: true,
                }),
                upper: Some(Bound {
                    version: upper,
                    inclusive: false,
                }),
            })
        }
        _ => Err(invalid("unknown comparator", token)),
    }
}

impl fmt::Display for VersionRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

impl FromStr for VersionRange {
    type Err = FabricError;
    fn from_str(s: &str) -> Result<Self> {
        Self::parse(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn semver_ordering() {
        let mut versions = [
            "1.0.0",
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-beta",
            "1.0.0-rc.1",
            "1.0.0+build",
            "0.9.9",
        ]
        .map(v)
        .to_vec();
        versions.sort();
        let names: Vec<String> = versions.iter().map(ToString::to_string).collect();
        assert_eq!(names[0], "0.9.9");
        assert!(
            names.iter().position(|s| s == "1.0.0-alpha").unwrap()
                < names.iter().position(|s| s == "1.0.0-alpha.1").unwrap()
        );
        assert_eq!(v("1.0.0+build").cmp(&v("1.0.0")), Ordering::Equal);
    }

    #[test]
    fn numeric_prerelease_ordering_has_no_overflow() {
        assert!(v("1.0.0-2").cmp(&v("1.0.0-1000")) == Ordering::Less);
        assert!(Version::parse("1.0.0-00000000000000000000000002").is_err());
    }

    #[test]
    fn rejects_hostile_versions_without_panicking() {
        for bad in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "1.2.3-",
            "1.2.3+",
            "01.2.3",
            "1.2.3-a..b",
            "1.-2.3",
        ] {
            assert!(Version::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn maven_ranges() {
        let r = VersionRange::parse("[1.0,2.0)").unwrap();
        assert!(r.contains(&v("1.0.0")));
        assert!(r.contains(&v("1.9.9")));
        assert!(!r.contains(&v("2.0.0")));
        let r = VersionRange::parse("(,2.0]").unwrap();
        assert!(r.contains(&v("0.0.1")));
        assert!(r.contains(&v("2.0.0")));
        assert!(!r.contains(&v("2.0.1")));
        let r = VersionRange::parse("[1.2.3]").unwrap();
        assert!(r.contains(&v("1.2.3")));
        assert!(!r.contains(&v("1.2.4")));
    }

    #[test]
    fn wildcards_comparators_tilde_caret() {
        let r = VersionRange::parse("*").unwrap();
        assert!(r.contains(&v("0.0.1")));
        let r = VersionRange::parse("1.2.x").unwrap();
        assert!(r.contains(&v("1.2.9")));
        assert!(!r.contains(&v("1.3.0")));
        let r = VersionRange::parse(">=1.2 <2.0").unwrap();
        assert!(r.contains(&v("1.5.0")));
        assert!(!r.contains(&v("2.0.0")));
        let r = VersionRange::parse("~1.2.3").unwrap();
        assert!(r.contains(&v("1.2.9")));
        assert!(!r.contains(&v("1.3.0")));
        let r = VersionRange::parse("^1.2.3").unwrap();
        assert!(r.contains(&v("1.9.0")));
        assert!(!r.contains(&v("2.0.0")));
        let r = VersionRange::parse("^0.2.3").unwrap();
        assert!(r.contains(&v("0.2.9")));
        assert!(!r.contains(&v("0.3.0")));
        let r = VersionRange::parse("^0.0.3").unwrap();
        assert!(!r.contains(&v("0.0.4")));
        let r = VersionRange::parse("=1.4.2").unwrap();
        assert!(r.contains(&v("1.4.2")));
        assert!(!r.contains(&v("1.4.3")));
    }

    #[test]
    fn alternatives_and_intersection() {
        let a = VersionRange::parse(">=1.0 <2.0").unwrap();
        let b = VersionRange::parse("[1.5,3.0)").unwrap();
        assert!(a.intersects(&b));
        assert_eq!(a.intersection_witness(&b), Some(v("1.5.0")));
        let c = VersionRange::parse(">=3.0").unwrap();
        assert!(!a.intersects(&c));
        assert_eq!(a.intersection_witness(&c), None);
        let d = VersionRange::parse("<=0.5 || >=1.2 <1.3").unwrap();
        assert!(d.contains(&v("0.4.0")));
        assert!(d.contains(&v("1.2.5")));
        assert!(!d.contains(&v("1.4.0")));
        assert!(!d.intersects(&b));
    }

    #[test]
    fn rejects_bad_ranges() {
        for bad in [
            "",
            "[1.0",
            "[1.0,2.0,3.0)",
            ">=x",
            "1.2.3.4",
            ">~",
            "[a,b]",
            "||",
        ] {
            assert!(VersionRange::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn prerelease_boxes() {
        let r = VersionRange::parse(">=1.0.0-alpha <1.0.0").unwrap();
        assert!(r.contains(&v("1.0.0-beta")));
        assert!(!r.contains(&v("1.0.0")));
    }
}
