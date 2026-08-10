use crate::error::{Error, Result};
use crate::model::Frontmatter;

pub const MAX_FRONTMATTER_BYTES: usize = 64 * 1024;

/// Bound the raw YAML frontmatter block before it ever reaches the parser:
/// size-cap it, and reject anchors/aliases/merge keys outright rather than
/// trusting the size cap alone to bound in-memory expansion.
pub fn validate_raw(raw: &str) -> Result<()> {
    if raw.len() > MAX_FRONTMATTER_BYTES {
        return Err(Error::FrontmatterTooLarge {
            size: raw.len(),
            max: MAX_FRONTMATTER_BYTES,
        });
    }
    if contains_anchor_or_merge(raw) {
        return Err(Error::FrontmatterAnchorsRejected);
    }
    Ok(())
}

fn contains_anchor_or_merge(raw: &str) -> bool {
    raw.lines().any(|line| {
        let t = line.trim_start();
        t.contains(" &") || t.contains(" *") || t.starts_with("<<:") || t.contains(": <<")
    })
}

/// Strict parse: used when *writing* frontmatter shaic itself authored — any
/// unknown field is a bug, so it should fail loudly (`Frontmatter` derives
/// `deny_unknown_fields`).
pub fn parse_strict(raw: &str) -> Result<Frontmatter> {
    validate_raw(raw)?;
    serde_yaml_ng::from_str(raw).map_err(|e| Error::FrontmatterParse(e.to_string()))
}

/// Lenient parse: used when *reading* an item pulled from the store, which may
/// have been authored by a newer shaic version with extra fields. Unknown
/// top-level keys are dropped with a warning instead of hard-failing, so an
/// older client isn't bricked by every file the moment a field is added.
pub fn parse_lenient(raw: &str) -> Result<Frontmatter> {
    validate_raw(raw)?;
    let mut value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(raw).map_err(|e| Error::FrontmatterParse(e.to_string()))?;
    const KNOWN: &[&str] = &[
        "name",
        "description",
        "applies_to",
        "tags",
        "scope",
        "agents",
    ];
    if let serde_yaml_ng::Value::Mapping(map) = &mut value {
        map.retain(|k, _| {
            let keep = k.as_str().map(|s| KNOWN.contains(&s)).unwrap_or(false);
            if !keep {
                eprintln!(
                    "warning: ignoring unknown frontmatter field {k:?} (from a newer shaic version?)"
                );
            }
            keep
        });
    }
    serde_yaml_ng::from_value(value).map_err(|e| Error::FrontmatterParse(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_oversized_frontmatter() {
        let raw = "name: a\ndescription: ".to_string() + &"x".repeat(MAX_FRONTMATTER_BYTES);
        assert!(validate_raw(&raw).is_err());
    }

    #[test]
    fn rejects_anchors() {
        let raw = "name: &anchor a\ndescription: d\n";
        assert!(validate_raw(raw).is_err());
    }

    #[test]
    fn lenient_parse_drops_unknown_fields() {
        let raw = "name: a\ndescription: d\nfuture_field: surprise\n";
        let fm = parse_lenient(raw).unwrap();
        assert_eq!(fm.name, "a");
    }

    #[test]
    fn strict_parse_rejects_unknown_fields() {
        let raw = "name: a\ndescription: d\nfuture_field: surprise\n";
        assert!(parse_strict(raw).is_err());
    }
}
