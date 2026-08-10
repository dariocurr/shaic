use std::sync::OnceLock;

use regex::Regex;

use crate::error::{Error, Result};

struct Pattern {
    name: &'static str,
    regex: &'static str,
}

const PATTERNS: &[Pattern] = &[
    Pattern {
        name: "AWS access key",
        regex: r"AKIA[0-9A-Z]{16}",
    },
    Pattern {
        name: "private key header",
        regex: r"-----BEGIN (RSA|OPENSSH|EC|PGP) PRIVATE KEY-----",
    },
    Pattern {
        name: "GitHub token",
        regex: r"gh[pousr]_[A-Za-z0-9]{36,}",
    },
    Pattern {
        name: "OpenAI-style key",
        regex: r"sk-[A-Za-z0-9]{20,}",
    },
    Pattern {
        name: "Slack token",
        regex: r"xox[baprs]-[A-Za-z0-9-]{10,}",
    },
    Pattern {
        name: "generic secret assignment",
        regex: r#"(?i)(api|secret|token|password)[_-]?key\s*[:=]\s*['"][A-Za-z0-9+/=_-]{16,}['"]"#,
    },
];

fn compiled() -> &'static Vec<(&'static str, Regex)> {
    static CELL: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    CELL.get_or_init(|| {
        PATTERNS
            .iter()
            .map(|p| {
                (
                    p.name,
                    Regex::new(p.regex).expect("static pattern is valid regex"),
                )
            })
            .collect()
    })
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub pattern: &'static str,
    pub line: usize,
    pub excerpt: String,
}

/// Scan content for obvious secret shapes. A best-effort tripwire run before
/// every `shaic push` commit — not a guarantee. False negatives are expected;
/// this only exists to catch the easy, high-confidence mistakes.
pub fn scan(content: &str) -> Vec<Hit> {
    let mut hits = Vec::new();
    for (line_no, line) in content.lines().enumerate() {
        for (name, re) in compiled() {
            if re.is_match(line) {
                hits.push(Hit {
                    pattern: name,
                    line: line_no + 1,
                    excerpt: line.chars().take(80).collect(),
                });
            }
        }
    }
    hits
}

/// Scan and turn any hit into a hard error unless explicitly overridden.
pub fn scan_or_reject(content: &str, override_flag: bool) -> Result<()> {
    let hits = scan(content);
    if hits.is_empty() || override_flag {
        return Ok(());
    }
    let summary = hits
        .iter()
        .map(|h| format!("line {}: {}", h.line, h.pattern))
        .collect::<Vec<_>>()
        .join("; ");
    Err(Error::SecretDetected(summary))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_aws_key() {
        assert!(!scan("AKIAABCDEFGHIJKLMNOP appears here").is_empty());
    }

    #[test]
    fn flags_private_key_header() {
        assert!(!scan(concat!("-----BEGIN ", "RSA PRIVATE KEY-----")).is_empty());
    }

    #[test]
    fn flags_github_token() {
        assert!(!scan("ghp_1234567890abcdefghijklmnopqrstuvwxyz").is_empty());
    }

    #[test]
    fn clean_content_has_no_hits() {
        assert!(scan("# just a normal skill about code review\nno secrets here").is_empty());
    }

    #[test]
    fn tricky_negative_mentions_word_without_value() {
        // Documentation that talks *about* passwords without containing one
        // shouldn't trip the generic heuristic.
        assert!(scan("Never hardcode a password in source.").is_empty());
    }

    #[test]
    fn scan_or_reject_blocks_by_default_and_is_overridable() {
        let content = "AKIAABCDEFGHIJKLMNOP";
        assert!(scan_or_reject(content, false).is_err());
        assert!(scan_or_reject(content, true).is_ok());
    }
}
