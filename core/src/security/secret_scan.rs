use std::sync::OnceLock;

use regex::Regex;

use crate::error::{Error, Result};

/// Any identifier containing one of these words is treated as naming a
/// credential. Deliberately unanchored: real config keys are `GITHUB_TOKEN`,
/// `db.password`, `x-api-key`, `authToken` — the word is almost never the
/// whole identifier, which is exactly why the previous `…[_-]?key` shape
/// missed plain `password = "…"`.
const SECRET_WORDS: &str = r"(?:secret|token|password|passwd|credential|api[_-]?key|auth)";

/// Shortest assigned value worth flagging. Below this, a match is far more
/// likely to be prose, a short enum value, or an env var name than a real
/// credential, and that noise would train people to reach for the override
/// flag — which costs more than the misses.
const MIN_VALUE_LEN: &str = "{12,}";

/// Shortest password worth flagging inside a `scheme://user:pass@host` URL.
/// Lower than `MIN_VALUE_LEN` because the surrounding syntax is already the
/// evidence: nothing but a credential ever sits in that position.
const MIN_URL_PASSWORD_LEN: &str = "{6,}";

/// How much of a matching line `Hit::excerpt` keeps, after redaction.
const EXCERPT_MAX_CHARS: usize = 80;

/// The tripwire shapes, as `(name, regex source)`.
///
/// A pattern may expose two named capture groups:
/// - `value`: the candidate credential itself. When present, it's the only
///   part that suppression and redaction reason about, so an assignment is
///   judged by what was assigned rather than by the whole line.
/// - `key` and `sep`: the identifier the value was assigned to and the
///   separator used, so the store's own TOML fields — which hold a
///   credential's *name* — can be told apart from an assignment of its value.
///
/// Patterns matching a self-identifying credential (`AKIA…`, a PEM header)
/// need neither group — the entire match is the secret.
fn pattern_sources() -> Vec<(&'static str, String)> {
    vec![
        ("AWS access key", r"AKIA[0-9A-Z]{16}".to_string()),
        (
            "private key header",
            r"-----BEGIN (?:RSA |OPENSSH |EC |DSA |PGP |ENCRYPTED )?PRIVATE KEY-----".to_string(),
        ),
        ("GitHub token", r"gh[pousr]_[A-Za-z0-9]{36,}".to_string()),
        (
            "GitHub fine-grained PAT",
            r"github_pat_[A-Za-z0-9_]{20,}".to_string(),
        ),
        // Allow hyphens so Anthropic `sk-ant-…` is caught; bare `sk-` docs
        // without a long token still need 20+ chars after the prefix.
        ("OpenAI-style key", r"sk-[A-Za-z0-9_-]{20,}".to_string()),
        // Underscore, unlike the hyphen in the OpenAI shape above, so the two
        // never claim the same string.
        (
            "Stripe live key",
            r"(?:sk|rk)_live_[A-Za-z0-9]{10,}".to_string(),
        ),
        ("npm token", r"npm_[A-Za-z0-9]{20,}".to_string()),
        ("PyPI token", r"pypi-[A-Za-z0-9_-]{16,}".to_string()),
        (
            "Google OAuth client secret",
            r"GOCSPX-[A-Za-z0-9_-]{10,}".to_string(),
        ),
        ("Slack token", r"xox[baprs]-[A-Za-z0-9-]{10,}".to_string()),
        ("Google API key", r"AIza[0-9A-Za-z_-]{35}".to_string()),
        // `eyJ` is base64 for `{"`, so this matches an actual JOSE header
        // followed by a payload and a signature, not any dotted blob.
        (
            "JWT",
            r"eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}".to_string(),
        ),
        // The two generic assignment shapes are split by quoting rather than
        // merged with an optional quote, so neither can match the other's
        // value: the bare form excludes quote characters outright, which is
        // also what keeps `KEY="…"` from being reported twice.
        (
            "quoted secret assignment",
            format!(
                r#"(?i)(?P<key>[A-Za-z0-9_.-]*{SECRET_WORDS}[A-Za-z0-9_.-]*)\s*(?P<sep>[:=])\s*["'](?P<value>[^"'\n]{MIN_VALUE_LEN})["']"#
            ),
        ),
        (
            "unquoted secret assignment",
            format!(
                r#"(?i)(?P<key>[A-Za-z0-9_.-]*{SECRET_WORDS}[A-Za-z0-9_.-]*)\s*(?P<sep>[:=])\s*(?P<value>[^\s"'\n]{MIN_VALUE_LEN})"#
            ),
        ),
        // A connection string carries its credential in a position no
        // identifier name announces — `DATABASE_URL` says nothing about
        // holding a password, so the shape has to be matched instead.
        (
            "url with credentials",
            format!(r"://[^\s:/@]+:(?P<value>[^\s:/@]{MIN_URL_PASSWORD_LEN})@"),
        ),
    ]
}

/// Compiled once. A pattern that fails to compile is dropped rather than
/// panicking a library call — `patterns_all_compile` keeps that from silently
/// weakening the scanner, since a broken pattern fails CI instead of a user's
/// `shaic push`.
fn compiled() -> &'static Vec<(&'static str, Regex)> {
    static CELL: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    CELL.get_or_init(|| {
        pattern_sources()
            .into_iter()
            .filter_map(|(name, source)| match Regex::new(&source) {
                Ok(re) => Some((name, re)),
                Err(_) => None,
            })
            .collect()
    })
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub pattern: &'static str,
    pub line: usize,
    /// The matching line with every credential-shaped run replaced by `…`.
    /// A hit report is often pasted into a bug report or a terminal others can
    /// see, so it must never be the thing that leaks the secret the scan just
    /// caught; `pattern` already says what kind of credential it was.
    pub excerpt: String,
}

/// Scan content for obvious secret shapes. A best-effort tripwire run before
/// every `shaic push` commit — not a guarantee. False negatives are expected;
/// this only exists to catch the easy, high-confidence mistakes.
pub fn scan(content: &str) -> Vec<Hit> {
    let mut hits = Vec::new();
    for (line_no, line) in content.lines().enumerate() {
        // Redaction has to consult every pattern, so it's computed at most
        // once per line and shared by all of that line's hits.
        let mut line_excerpt: Option<String> = None;
        for (name, re) in compiled() {
            if !flags(re, line) {
                continue;
            }
            hits.push(Hit {
                pattern: name,
                line: line_no + 1,
                excerpt: line_excerpt.get_or_insert_with(|| redacted(line)).clone(),
            });
        }
    }
    hits
}

/// Whether `re` matches something on `line` that survives suppression. Every
/// match is considered, not just the first: one placeholder on a line must not
/// excuse a real credential later on the same line.
fn flags(re: &Regex, line: &str) -> bool {
    re.captures_iter(line).any(|caps| match caps.name("value") {
        None => true,
        Some(value) => {
            let group = |name| caps.name(name).map(|m| m.as_str()).unwrap_or_default();
            !is_placeholder(value.as_str())
                && !names_a_secret(group("key"), group("sep"), value.as_str())
        }
    })
}

/// Documentation is the main thing in this store, and documentation is full of
/// credential-shaped examples. A value that is *entirely* a stand-in can't be
/// a leak, so suppressing it is what keeps real docs pushable — which matters,
/// because a tripwire people habitually override protects nothing.
fn is_placeholder(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return true;
    }
    const LITERALS: &[&str] = &[
        "changeme",
        "change-me",
        "change_me",
        "example",
        "redacted",
        "placeholder",
        "your-token-here",
        "your_token_here",
        "yourtokenhere",
    ];
    let lower = value.to_ascii_lowercase();
    if LITERALS.contains(&lower.as_str()) {
        return true;
    }
    // An interpolation — `${VAR}`, `$VAR`, `{{ handlebars }}`, `<angle>` — is
    // resolved by whatever reads the file, so the file itself holds nothing.
    if value.starts_with("${") && value.ends_with('}') {
        return true;
    }
    if let Some(rest) = value.strip_prefix('$')
        && !rest.is_empty()
        && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return true;
    }
    if value.starts_with("{{") && value.ends_with("}}") {
        return true;
    }
    if value.starts_with('<') && value.ends_with('>') {
        return true;
    }
    // Already masked (`xxxxxxxxxxxx`, `************`, `sk-……`): there is
    // nothing left in it to leak.
    value.chars().all(|c| matches!(c, 'x' | 'X' | '*' | '.'))
}

/// shaic's whole MCP design is that the store holds a credential's *name*
/// (`secret = "GITHUB_TOKEN"`, `bearer_token_env_var = "…"`) while the value
/// lives in the OS keychain. Those keys therefore never hold a credential, and
/// flagging them would make every MCP server that references a secret
/// impossible to push — the exact opposite of what the tripwire is for.
///
/// Kept deliberately narrow, because this is the one place the scanner is
/// taught to look away:
/// - only TOML assignment (`=`), the syntax the store is actually written in.
///   A `secret: "…"` in a rule, a doc, or an agent's JSON is not shaic's
///   reference syntax and stays flagged;
/// - only these name-shaped keys, and only when the value has the *shape of a
///   name* rather than merely being alphanumeric: `SCREAMING_SNAKE` (the env
///   var convention in `mcp::mcp_template`) or a separated lowercase
///   identifier (`github-pat`, `deploy_token`). An undifferentiated run of
///   letters and digits — `secret = "hunter2hunter2hunter2"`, a real
///   credential pasted where a reference belongs — stays flagged.
///
/// Anything that actually looks like a credential (`AKIA…`, `ghp_…`, a JWT)
/// has its own pattern and is caught no matter which key it sits under, so
/// this cannot launder a recognizable secret either.
fn names_a_secret(key: &str, sep: &str, value: &str) -> bool {
    if sep != "=" {
        return false;
    }
    // A diff line's leading `+`/`-` is part of the identifier match; strip any
    // such punctuation so `-secret = "…"` is judged like `secret = "…"`.
    let key = key
        .trim_start_matches(|c: char| !c.is_ascii_alphanumeric())
        .to_ascii_lowercase();
    // An over-long "name" is far likelier to be a pasted credential than a
    // reference, whichever key it sits under.
    if value.is_empty() || value.len() > 48 {
        return false;
    }
    let holds_a_name = key == "secret"
        || key.ends_with("_env_var")
        || key.ends_with("_env")
        || key.ends_with("_var")
        || key.ends_with("_name");
    holds_a_name && looks_like_a_name(value)
}

/// `SCREAMING_SNAKE`, or a lowercase identifier with a `_`/`-` separator —
/// both shapes a human picks for a *reference*. A bare run of mixed letters
/// and digits is the shape of a credential, so it is deliberately excluded.
fn looks_like_a_name(value: &str) -> bool {
    let screaming_snake = value.starts_with(|c: char| c.is_ascii_uppercase())
        && value
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    let separated_lowercase = value.starts_with(|c: char| c.is_ascii_lowercase())
        && value.contains(['_', '-'])
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    screaming_snake || separated_lowercase
}

/// Blank out every credential-shaped run on `line`, from *all* patterns rather
/// than only the one being reported — a line with two secrets must not leak
/// the second one while reporting the first.
fn redacted(line: &str) -> String {
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for (_, re) in compiled() {
        for caps in re.captures_iter(line) {
            if let Some(m) = caps.name("value").or_else(|| caps.get(0)) {
                spans.push((m.start(), m.end()));
            }
        }
    }
    spans.sort_unstable();

    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in spans {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }

    let mut out = String::new();
    let mut cursor = 0usize;
    for (start, end) in merged {
        out.push_str(&line[cursor..start]);
        out.push('…');
        cursor = end;
    }
    out.push_str(&line[cursor..]);
    out.chars().take(EXCERPT_MAX_CHARS).collect()
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

    fn hit_names(content: &str) -> Vec<&'static str> {
        scan(content).into_iter().map(|h| h.pattern).collect()
    }

    #[test]
    fn patterns_all_compile() {
        assert_eq!(
            compiled().len(),
            pattern_sources().len(),
            "a secret-scan pattern failed to compile and was silently dropped"
        );
    }

    #[test]
    fn flags_aws_key() {
        assert!(!scan("AKIAABCDEFGHIJKLMNOP appears here").is_empty());
    }

    #[test]
    fn flags_private_key_header() {
        assert!(!scan(concat!("-----BEGIN ", "RSA PRIVATE KEY-----")).is_empty());
        assert!(!scan(concat!("-----BEGIN ", "PRIVATE KEY-----")).is_empty());
        assert!(!scan(concat!("-----BEGIN ", "ENCRYPTED PRIVATE KEY-----")).is_empty());
    }

    #[test]
    fn flags_github_token() {
        assert!(!scan("ghp_1234567890abcdefghijklmnopqrstuvwxyz").is_empty());
        assert!(!scan("github_pat_11AAAAAAA0123456789012_abcdefghijklmnopqrstuv").is_empty());
    }

    #[test]
    fn flags_anthropic_style_key() {
        assert!(!scan("sk-ant-api03-abcdefghijklmnopqrstuvwxyz012345").is_empty());
    }

    #[test]
    fn flags_google_api_key() {
        assert!(!scan("AIzaSyA-abcdefghijklmnopqrstuvwxyz0123456").is_empty());
    }

    #[test]
    fn flags_stripe_live_keys() {
        assert!(hit_names("sk_live_4eC39HqLyjWDarjtT1zdp7dc").contains(&"Stripe live key"));
        assert!(hit_names("rk_live_4eC39HqLyjWDarjtT1zdp7dc").contains(&"Stripe live key"));
        assert!(
            hit_names("sk_test_4eC39HqLyjWDarjtT1zdp7dc").is_empty(),
            "test-mode keys aren't credentials worth blocking a push over"
        );
    }

    #[test]
    fn flags_npm_and_pypi_tokens() {
        assert!(
            hit_names("npm_abcdefghijklmnopqrstuvwxyz0123456789").contains(&"npm token"),
            "npm token shape must be flagged"
        );
        assert!(
            hit_names("pypi-AgEIcHlwaS5vcmcCJDExMjIzMzQ0").contains(&"PyPI token"),
            "PyPI token shape must be flagged"
        );
    }

    #[test]
    fn flags_google_oauth_client_secret() {
        assert!(
            hit_names("GOCSPX-abcdefghijklmnopqrstuvwx").contains(&"Google OAuth client secret")
        );
    }

    #[test]
    fn flags_jwt() {
        let jwt = concat!(
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.",
            "eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvZSJ9.",
            "dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXk"
        );
        assert!(hit_names(jwt).contains(&"JWT"));
    }

    #[test]
    fn flags_url_with_embedded_credentials() {
        assert!(
            hit_names(r#"DATABASE_URL = "postgres://user:sup3rs3cretpassword@host/db""#)
                .contains(&"url with credentials"),
            "a connection string names nothing secret-sounding, so the shape has to catch it"
        );
        assert!(
            hit_names("see https://example.com:8080/docs for the port").is_empty(),
            "a port number is not a password"
        );
        assert!(
            hit_names("clone from https://github.com/x/y.git").is_empty(),
            "a plain url has no credential to flag"
        );
    }

    #[test]
    fn flags_assignments_the_old_key_only_pattern_missed() {
        // Regression: the previous generic pattern required the literal word
        // `key` after the secret word, so every one of these got through.
        for line in [
            r#"password = "hunter2hunter2hunter2""#,
            r#"secret: "abcdefghijklmnopqrstuvwxyz""#,
            r#"api_token = "abcdefghijklmnopqrstuvwxyz""#,
            r#"DATABASE_URL = "postgres://user:sup3rs3cretpassword@host/db""#,
        ] {
            assert!(!scan(line).is_empty(), "expected a hit for {line:?}");
        }
    }

    #[test]
    fn flags_unquoted_env_file_style_assignment() {
        assert!(
            hit_names("API_TOKEN=abcdefghijklmnopqrstuvwxyz")
                .contains(&"unquoted secret assignment"),
            "a `.env` line has no quotes to key off"
        );
        assert!(
            hit_names("DB_PASSWORD=hunter2hunter2hunter2").contains(&"unquoted secret assignment")
        );
    }

    #[test]
    fn quoted_and_unquoted_patterns_do_not_double_report() {
        let names = hit_names(r#"password = "hunter2hunter2hunter2""#);
        assert_eq!(names, vec!["quoted secret assignment"]);
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
        assert!(scan("Rotate the api key every 90 days.").is_empty());
    }

    #[test]
    fn suppresses_placeholder_values() {
        for line in [
            r#"password = "changeme""#,
            r#"api_key: "your-token-here""#,
            r#"token = "REDACTED""#,
            r#"password = "${DB_PASSWORD}""#,
            r#"password = "$DB_PASSWORD""#,
            r#"api_key = "{{ vault.api_key }}""#,
            r#"token = "<your token here>""#,
            r#"password = "xxxxxxxxxxxxxxxx""#,
            r#"token = "****************""#,
            r#"DATABASE_URL = "postgres://user:${PGPASSWORD}@host/db""#,
        ] {
            assert!(scan(line).is_empty(), "expected {line:?} to be suppressed");
        }
    }

    #[test]
    fn a_placeholder_does_not_excuse_a_real_secret_on_the_same_line() {
        let line = r#"a_token = "${FROM_ENV}" and b_token = "abcdefghijklmnopqrst""#;
        assert!(!scan(line).is_empty());
    }

    #[test]
    fn suppresses_shaic_secret_name_references() {
        // These are the store's canonical way to *reference* a credential.
        // Flagging them would make every MCP server holding a secret
        // unpushable.
        for line in [
            r#"secret = "shaic-test-secret-never-set-9f2c""#,
            r#"bearer_token_env_var = "GITHUB_PERSONAL_ACCESS_TOKEN""#,
            r#"-secret = "shaic-test-secret-never-set-9f2c""#,
        ] {
            assert!(scan(line).is_empty(), "expected {line:?} to be suppressed");
        }
        assert!(
            !scan(r#"secret = "AKIAABCDEFGHIJKLMNOP""#).is_empty(),
            "a recognizable credential must still be caught under a name-shaped key"
        );
        assert!(
            !scan(r#"secret: "abcdefghijklmnopqrstuvwxyz""#).is_empty(),
            "only the store's own TOML reference syntax is exempt, not a colon assignment"
        );
        assert!(
            !scan(r#"secret = "hunter2hunter2hunter2""#).is_empty(),
            "a credential pasted where a reference belongs has no name shape and must be caught"
        );
    }

    #[test]
    fn excerpt_never_echoes_the_secret() {
        let hits = scan(r#"password = "hunter2hunter2hunter2""#);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].excerpt, "password = \"…\"");

        let hits = scan("AKIAABCDEFGHIJKLMNOP appears here");
        assert_eq!(hits[0].excerpt, "… appears here");

        let hits = scan(r#"DATABASE_URL = "postgres://user:sup3rs3cretpassword@host/db""#);
        assert!(
            hits.iter().all(|h| !h.excerpt.contains("sup3rs3cret")),
            "excerpt leaked the password: {:?}",
            hits[0].excerpt
        );
    }

    #[test]
    fn scan_or_reject_blocks_by_default_and_is_overridable() {
        let content = "AKIAABCDEFGHIJKLMNOP";
        assert!(scan_or_reject(content, false).is_err());
        assert!(scan_or_reject(content, true).is_ok());
    }
}
