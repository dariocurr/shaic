use crate::error::{Error, Result};
use crate::model::Frontmatter;

pub const MAX_FRONTMATTER_BYTES: usize = 64 * 1024;

/// Bound the raw YAML frontmatter block before it ever reaches the parser:
/// size-cap it, and reject anchors/aliases/merge keys outright rather than
/// trusting the size cap alone to bound in-memory expansion (a few hundred
/// bytes of nested aliases expand to gigabytes — the "billion laughs" shape).
/// Both checks have to run *before* parsing, which is what forces the
/// hand-written pre-scan below instead of inspecting a parsed document.
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

/// Anchor/alias/merge-key detection, accurate enough to be usable.
///
/// The predecessor flagged any line containing `" &"` or `" *"`, which made
/// ordinary English (`description: Tools & tips`) and ordinary globs
/// (`description: match *.md files`) unsaveable. `&`/`*` are only YAML syntax
/// in *node position* — where a value or a sequence entry begins — so that's
/// the only place they're treated as syntax here. Everything else (plain
/// scalars, quoted scalars, comments, block-scalar bodies) is text.
fn contains_anchor_or_merge(raw: &str) -> bool {
    let mut state = ScanState::default();
    for line in raw.lines() {
        if state.line_is_syntax(line) {
            return true;
        }
    }
    false
}

#[derive(Default)]
struct ScanState {
    /// Set while a multi-line quoted scalar is still open, so its later lines
    /// are treated as the text they are rather than rescanned as syntax.
    open_quote: Option<u8>,
    /// Indentation of the line that introduced a `|`/`>` block scalar. Its
    /// body is literal text at deeper indentation, and stays literal no matter
    /// what it contains.
    block_scalar_indent: Option<usize>,
}

impl ScanState {
    fn line_is_syntax(&mut self, line: &str) -> bool {
        let bytes = line.as_bytes();
        let indent = bytes.iter().take_while(|b| **b == b' ').count();

        if let Some(parent) = self.block_scalar_indent {
            if line.trim().is_empty() || indent > parent {
                return false;
            }
            self.block_scalar_indent = None;
        }

        // Resume an unterminated quoted scalar: nothing before its closing
        // quote is syntax, and once closed we're mid-line, never at a node.
        let mut cursor = 0usize;
        if let Some(quote) = self.open_quote {
            match end_of_quoted(bytes, 0, quote) {
                Some(end) => {
                    self.open_quote = None;
                    cursor = end;
                }
                None => return false,
            }
        }

        let (found, ends_open_quote) =
            scan_line(bytes, cursor, self.open_quote.is_none() && cursor == 0);
        if found {
            return true;
        }
        self.open_quote = ends_open_quote;
        if self.open_quote.is_none() && opens_block_scalar(line) {
            self.block_scalar_indent = Some(indent);
        }
        false
    }
}

/// Scan one line from `start`, returning `(found_syntax, unterminated_quote)`.
/// `at_node` says whether the first non-space character begins a fresh node —
/// false when resuming after a quoted scalar that opened on an earlier line.
fn scan_line(bytes: &[u8], start: usize, at_node: bool) -> (bool, Option<u8>) {
    let mut i = start;
    let mut at_node = at_node;
    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' => i += 1,
            // A comment starts only at the beginning of a token, which after
            // the whitespace arm above is exactly where we are.
            b'#' => return (false, None),
            quote @ (b'\'' | b'"') => match end_of_quoted(bytes, i, quote) {
                Some(end) => {
                    i = end;
                    at_node = false;
                }
                None => return (false, Some(quote)),
            },
            b'&' | b'*' if at_node => {
                if bytes.get(i + 1).is_some_and(|b| is_anchor_char(*b)) {
                    return (true, None);
                }
                at_node = false;
                i += 1;
            }
            // `<<` is a merge key only in key position — as a value it is the
            // plain scalar "<<".
            b'<' if at_node && bytes.get(i + 1) == Some(&b'<') => {
                if rest_starts_a_key(bytes, i + 2) {
                    return (true, None);
                }
                at_node = false;
                i += 2;
            }
            // A block sequence entry: `- ` opens a fresh node, `-1` doesn't.
            b'-' if at_node && matches!(bytes.get(i + 1), None | Some(b' ') | Some(b'\t')) => {
                i += 1;
            }
            b'[' | b'{' | b',' => {
                at_node = true;
                i += 1;
            }
            b':' => {
                i += 1;
                // Only a `:` followed by whitespace (or end of line) separates
                // a key from its value; `12:30` is one plain scalar.
                at_node = matches!(bytes.get(i), None | Some(b' ') | Some(b'\t'));
            }
            _ => {
                at_node = false;
                i += 1;
            }
        }
    }
    (false, None)
}

/// Index just past the scalar's closing quote, or `None` if it runs off the
/// end of the line (a legal multi-line scalar).
fn end_of_quoted(bytes: &[u8], start: usize, quote: u8) -> Option<usize> {
    let mut i = if bytes.get(start) == Some(&quote) {
        start + 1
    } else {
        start
    };
    while i < bytes.len() {
        if bytes[i] == quote {
            // `''` inside a single-quoted scalar is an escaped quote, not the
            // end of it; `"` uses backslash escapes instead.
            if quote == b'\'' && bytes.get(i + 1) == Some(&b'\'') {
                i += 2;
                continue;
            }
            return Some(i + 1);
        }
        if quote == b'"' && bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        i += 1;
    }
    None
}

fn is_anchor_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// Whether what follows is `:` (possibly after spaces) — i.e. the token we
/// just read was a key.
fn rest_starts_a_key(bytes: &[u8], from: usize) -> bool {
    let mut i = from;
    while matches!(bytes.get(i), Some(b' ') | Some(b'\t')) {
        i += 1;
    }
    bytes.get(i) == Some(&b':')
}

/// Whether the line ends with a `|`/`>` block-scalar indicator (with optional
/// chomping/indent indicators). The indicator has to be its own token, so
/// `regex: ^a|b$` isn't mistaken for one.
fn opens_block_scalar(line: &str) -> bool {
    let trimmed = line.trim_end();
    let mut rest = trimmed;
    while rest
        .chars()
        .next_back()
        .is_some_and(|c| c == '-' || c == '+' || c.is_ascii_digit())
    {
        rest = &rest[..rest.len() - 1];
    }
    let Some(head) = rest.strip_suffix(['|', '>']) else {
        return false;
    };
    matches!(head.chars().next_back(), Some(' ') | Some('\t') | Some(':'))
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
///
/// Warnings come back as data — this is a library, and the TUI's alternate
/// screen is destroyed by a stray `eprintln!`.
pub fn parse_lenient(raw: &str) -> Result<(Frontmatter, Vec<String>)> {
    validate_raw(raw)?;
    let mut value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(raw).map_err(|e| Error::FrontmatterParse(e.to_string()))?;
    let mut warnings = Vec::new();
    if let serde_yaml_ng::Value::Mapping(map) = &mut value {
        map.retain(|k, _| {
            let keep = k
                .as_str()
                .map(|s| Frontmatter::FIELDS.contains(&s))
                .unwrap_or(false);
            if !keep {
                warnings.push(format!(
                    "ignoring unknown frontmatter field {k:?} (from a newer shaic version?)"
                ));
            }
            keep
        });
    }
    let frontmatter =
        serde_yaml_ng::from_value(value).map_err(|e| Error::FrontmatterParse(e.to_string()))?;
    Ok((frontmatter, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_oversized_frontmatter() {
        let raw = "name: a\ndescription: ".to_string() + &"x".repeat(MAX_FRONTMATTER_BYTES);
        assert!(matches!(
            validate_raw(&raw),
            Err(Error::FrontmatterTooLarge { .. })
        ));
    }

    #[test]
    fn rejects_anchors() {
        for raw in [
            "name: &anchor a\ndescription: d\n",
            "name: &a x\n",
            "name: x\nother: *a\n",
            "<<: *base\nname: x\n",
            "tags:\n  - &first one\n",
            "tags: [*a, b]\n",
            "map: {k: *a}\n",
            "merged:\n  <<: *defaults\n",
        ] {
            assert!(
                matches!(validate_raw(raw), Err(Error::FrontmatterAnchorsRejected)),
                "expected {raw:?} to be rejected"
            );
        }
    }

    #[test]
    fn accepts_ordinary_text_that_merely_contains_ampersand_or_star() {
        // Regression: the old ` &`/` *` substring heuristic made every one of
        // these impossible to save at all.
        for raw in [
            "name: a\ndescription: Tools & tips\n",
            "name: a\ndescription: match *.md files\n",
            "name: a\ndescription: a & b\n",
            "name: a\ndescription: \"quoted &anchor-looking\"\n",
            "name: a\ndescription: 2 * 3\n",
            "name: a\ndescription: 'it''s &fine'\n",
            "name: a\napplies_to: [\"*.md\", \"src/**/*.rs\"]\n",
            "name: a\ndescription: R&D\n",
            "name: a\ndescription: |\n  bullet points:\n  *emphasis* and &more\n",
            "name: a\n# a comment about &anchors and *aliases\n",
            "name: a\ndescription: see docs # &not-an-anchor\n",
        ] {
            assert!(validate_raw(raw).is_ok(), "expected {raw:?} to be accepted");
        }
    }

    #[test]
    fn a_multiline_quoted_scalar_stays_text() {
        let raw = "name: a\ndescription: \"first line\n  &still text\"\n";
        assert!(validate_raw(raw).is_ok());
    }

    #[test]
    fn lenient_parse_drops_unknown_fields() {
        let raw = "name: a\ndescription: d\nfuture_field: surprise\n";
        let (fm, warnings) = parse_lenient(raw).unwrap();
        assert_eq!(fm.name, "a");
        assert!(warnings.iter().any(|w| w.contains("future_field")));
    }

    #[test]
    fn strict_parse_rejects_unknown_fields() {
        let raw = "name: a\ndescription: d\nfuture_field: surprise\n";
        assert!(parse_strict(raw).is_err());
    }

    #[test]
    fn ordinary_text_still_parses_after_the_pre_scan() {
        let raw = "name: a\ndescription: Tools & tips\n";
        let fm = parse_strict(raw).unwrap();
        assert_eq!(fm.description, "Tools & tips");
    }
}
