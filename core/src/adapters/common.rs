use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::materialize::writer::Manifest;
use crate::model::{AgentId, ContentForm, Frontmatter, Item, ItemKind, Scope};
use crate::security::frontmatter_limits;

use super::{Agent, DiscoveredContent, RenderedFile};

/// Build the single region all items of one kind render into, for a
/// single-file agent (`CLAUDE.md`, `AGENTS.md`, ...). The caller supplies a
/// per-item formatter. Splicing this region between markers in whatever the
/// file already contains happens in `materialize::writer`, which is the only
/// place that reads the current on-disk file — this function stays pure.
pub fn render_as_single_file(
    relative_path: PathBuf,
    scope: Scope,
    items: &[Item],
    format_item: impl Fn(&Item) -> String,
) -> Vec<RenderedFile> {
    let region = items
        .iter()
        .map(&format_item)
        .collect::<Vec<_>>()
        .join("\n\n");
    vec![RenderedFile {
        relative_path,
        contents: region,
        scope,
        form: ContentForm::SingleFile,
    }]
}

/// One file per item, under `dir`.
pub fn render_as_directory(
    dir: PathBuf,
    scope: Scope,
    items: &[Item],
    file_name: impl Fn(&Item) -> String,
    format_item: impl Fn(&Item) -> String,
) -> Vec<RenderedFile> {
    items
        .iter()
        .map(|item| RenderedFile {
            relative_path: dir.join(file_name(item)),
            contents: format_item(item),
            scope,
            form: ContentForm::Directory,
        })
        .collect()
}

pub fn md_file_name(item: &Item) -> String {
    format!("{}.md", item.name())
}

/// The common "a heading, then the body" shape shared by every agent whose
/// rules render as plain Markdown with no frontmatter (`## name` for
/// single-file agents, `# name` for one-file-per-item agents). Body lines
/// that would themselves look like a section heading are escaped so a
/// round-trip through `parse_heading_sections` cannot split the item.
pub fn heading_section(item: &Item, heading: &str) -> String {
    format!(
        "{heading} {}\n\n{}",
        item.name(),
        escape_heading_lines(item.body.trim(), heading)
    )
}

/// How many leading backslashes `line` has, if what follows them is exactly
/// `{heading} ` — i.e. the line either looks like a section heading to
/// `parse_heading_sections`, or looks like an *already-escaped* one.
fn heading_escape_depth(line: &str, heading: &str) -> Option<usize> {
    let backslashes = line.len() - line.trim_start_matches('\\').len();
    line[backslashes..]
        .starts_with(&format!("{heading} "))
        .then_some(backslashes)
}

/// Escape body lines that `parse_heading_sections` would otherwise mistake
/// for a new section, by adding *one* backslash — including to a line that
/// already starts with backslashes.
///
/// Adding a backslash unconditionally (rather than only to unescaped
/// headings) is what makes the transform injective, and therefore reversible:
/// escaping only `## Notes` while unescaping any `\## Notes` meant a body the
/// author deliberately wrote as `\## Notes` came back as `## Notes`, silently
/// rewriting content on a round-trip. Now `## x` -> `\## x` -> `## x` and
/// `\## x` -> `\\## x` -> `\## x`, at every depth.
fn escape_heading_lines(body: &str, heading: &str) -> String {
    body.lines()
        .map(|line| match heading_escape_depth(line, heading) {
            Some(_) => format!("\\{line}"),
            None => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Inverse of `escape_heading_lines`: remove exactly one backslash from a
/// line whose backslashes are followed by a heading prefix.
fn unescape_heading_line(line: &str, heading: &str) -> String {
    match heading_escape_depth(line, heading) {
        Some(depth) if depth > 0 => line[1..].to_string(),
        _ => line.to_string(),
    }
}

/// Frontmatter for the "description only" shape shared by several agents'
/// command/prompt/workflow files. A named struct rather than an inline
/// `format!` so the *set* of keys on disk is fixed at compile time — see
/// `frontmatter_document`.
#[derive(Serialize)]
struct DescriptionFrontmatter<'a> {
    description: &'a str,
}

/// The `name`/`description` frontmatter of a canonical `SKILL.md`.
#[derive(Serialize)]
struct SkillFrontmatter<'a> {
    name: &'a str,
    description: &'a str,
}

/// Assemble a `---\n{yaml}---\n\n{body}` document from a `Serialize`
/// frontmatter struct.
///
/// Never build YAML frontmatter by string interpolation. With `format!`, a
/// `description` holding a newline let whoever authored the item inject
/// arbitrary keys into *every* agent's config — `alwaysApply: true` for
/// Cursor, `applyTo: "**"` for Copilot, i.e. "apply my content to every
/// prompt in this repo" — and an ordinary `:`, `#`, leading `*`, quote or
/// `---` line produced YAML that no longer parsed at all. Serializing a
/// struct fixes the key set at compile time (a value can never become a key)
/// and leaves quoting to the emitter, which picks quoted or block style as
/// each value needs.
///
/// Field order follows declaration order (`serde_yaml_ng` preserves it), so
/// output stays byte-stable and `writer::classify` doesn't report a spurious
/// `Update` on every sync.
///
/// `Agent::render` returns `Vec<RenderedFile>` with no `Result` to carry a
/// failure, and serializing a struct of `&str`/`bool` cannot realistically
/// fail. The unreachable error path therefore degrades to a
/// *frontmatter-less* document plus a warning on stderr: a body-only file is
/// still valid Markdown that the reverse direction skips cleanly, whereas
/// emitting a half-written YAML header would either break the agent or, worse,
/// parse as something nobody intended.
///
/// One documented fidelity limit, inherent to the `---`-delimited shape rather
/// than to serde: the newline before the closing `---` belongs both to the last
/// YAML line and to the delimiter, so a value whose final character is a
/// newline comes back one newline shorter. It converges immediately (the
/// shortened value re-renders to itself) and only bites values nobody writes on
/// purpose, which is a better trade than a delimiter the parser can't find.
pub fn frontmatter_document<T: Serialize>(frontmatter: &T, body: &str) -> String {
    let body = body.trim();
    match serde_yaml_ng::to_string(frontmatter) {
        Ok(yaml) => {
            let yaml = if yaml.ends_with('\n') {
                yaml
            } else {
                format!("{yaml}\n")
            };
            format!("---\n{yaml}---\n\n{body}")
        }
        Err(_) => {
            // `serde_yaml_ng` failing on our own structs is a programming
            // bug, not a user input problem. Do not `eprintln!` — the TUI
            // lives on an alternate screen. Empty frontmatter still round-
            // trips as a document the parser can open, unlike a half-written
            // `---` block.
            format!("---\n---\n\n{body}")
        }
    }
}

/// The common "frontmatter with just a description, then the body" shape
/// shared by several agents' command/prompt/workflow files.
pub fn with_description(item: &Item) -> String {
    frontmatter_document(
        &DescriptionFrontmatter {
            description: &item.frontmatter.description,
        },
        &item.body,
    )
}

/// Read discovery: does not write anything, used for `agents discover` /
/// `status` drift detection and for the import path.
pub fn discover_single_file(path: &Path, scope: Scope) -> Vec<DiscoveredContent> {
    match std::fs::read_to_string(path) {
        Ok(raw) => vec![DiscoveredContent {
            source_path: path.to_path_buf(),
            scope,
            raw,
            form: ContentForm::SingleFile,
        }],
        Err(_) => Vec::new(),
    }
}

/// Split a `---\n<yaml>\n---\n\n<body>` document into its raw YAML block and
/// body. Generic (doesn't parse the YAML) so adapters whose on-disk
/// frontmatter fields don't match the canonical `Frontmatter` schema
/// (Cursor's `globs`/`alwaysApply`, Copilot's `applyTo`, ...) can pull out
/// just the field(s) they know how to interpret. `None` if `raw` doesn't
/// start with a frontmatter block at all.
///
/// Accepts LF and CRLF so Windows checkouts with `core.autocrlf=true` still
/// reconcile.
pub fn split_frontmatter_block(raw: &str) -> Option<(&str, &str)> {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let rest = raw
        .strip_prefix("---\r\n")
        .or_else(|| raw.strip_prefix("---\n"))?;
    if let Some(idx) = rest.find("\r\n---\r\n") {
        let (fm, after) = rest.split_at(idx);
        return Some((fm, &after["\r\n---\r\n".len()..]));
    }
    let marker = "\n---\n";
    let idx = rest.find(marker)?;
    let (fm, after) = rest.split_at(idx);
    Some((fm, &after[marker.len()..]))
}

/// Parse an agent file's raw frontmatter block into a YAML document, or
/// `None` if it isn't parseable as a mapping.
///
/// Every adapter reads agent-on-disk frontmatter through here so two
/// properties hold in one place: hostile input is bounded before the parser
/// sees it (`validate_raw` size-caps the block and rejects anchor/alias/merge
/// keys, the "billion laughs" shape — these files are hand-edited and can
/// come from anywhere), and a block that isn't a mapping at all is rejected
/// up front instead of every caller having to reason about indexing into a
/// sequence or scalar.
///
/// Shaic's own output always survives this: `frontmatter_document`'s emitter
/// quotes any value that starts with `&`/`*`, so a description like
/// `*.md files` can never read back as an alias and trip the anchor check.
pub fn parse_frontmatter_value(fm: &str) -> Option<serde_yaml_ng::Value> {
    frontmatter_limits::validate_raw(fm).ok()?;
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(fm).ok()?;
    value.is_mapping().then_some(value)
}

/// One string-valued frontmatter field, or `None` if it's absent or holds
/// something other than a string (a list, a number, `null`, a nested map).
/// Deliberately not "stringify whatever is there": an agent file whose
/// `globs` is a YAML list means something shaic doesn't know how to reverse,
/// and inventing a value for it would write that guess back to disk.
pub fn frontmatter_str<'a>(fm: &'a serde_yaml_ng::Value, key: &str) -> Option<&'a str> {
    fm.get(key)?.as_str()
}

/// Reverse of `with_description`: pull `(description, body)` back out of a
/// `---\ndescription: ...\n---\n\n{body}` block. `None` if `raw` isn't in
/// that shape at all — callers skip a file like that rather than guessing.
pub fn parse_with_description(raw: &str) -> Option<(String, String)> {
    let (fm, body) = split_frontmatter_block(raw)?;
    let value = parse_frontmatter_value(fm)?;
    let description = frontmatter_str(&value, "description")?.to_string();
    Some((description, body.trim().to_string()))
}

/// Reverse of `heading_section`, for a *single* combined-content file that
/// may hold several items back to back (`CLAUDE.md`, `AGENTS.md`, a legacy
/// `.windsurfrules`/`.clinerules`, ...): split on every `"<heading> "`-
/// prefixed line, returning one `(name, body)` pair per section in order.
/// A file with no matching heading line at all yields no sections, not one
/// section with an empty name — there's nothing sane to pull out of that.
pub fn parse_heading_sections(raw: &str, heading: &str) -> Vec<(String, String)> {
    let prefix = format!("{heading} ");
    let mut sections: Vec<(String, String)> = Vec::new();
    for line in raw.lines() {
        if let Some(name) = line.strip_prefix(&prefix) {
            let name = name.trim();
            if !name.is_empty() {
                sections.push((name.to_string(), String::new()));
            }
        } else if let Some((_, body)) = sections.last_mut() {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(&unescape_heading_line(line, heading));
        }
    }
    for (_, body) in &mut sections {
        *body = body.trim().to_string();
    }
    sections
}

/// Reverse of directory-form `heading_section` rendering, where each file
/// holds exactly one item and its name is really carried by the filename
/// (`file_name`/`md_file_name` at render time), not the heading text inside
/// it. Reuses `parse_heading_sections` to strip that leading heading line
/// and get the body, but always returns the *filename's* stem as the name —
/// so a hand-edited heading that disagrees with the filename can't produce
/// an item whose name doesn't match the file every other part of this crate
/// keys on. Falls back to the whole trimmed file as the body if it doesn't
/// start with a matching heading line at all (still recoverable, just
/// without a clean split point).
pub fn parse_directory_heading_item(raw: &str, heading: &str) -> String {
    match parse_heading_sections(raw, heading).into_iter().next() {
        Some((_, body)) => body,
        None => raw.trim().to_string(),
    }
}

/// Whether `discovered`'s on-disk content still matches exactly what shaic
/// itself last wrote there, per the shared per-agent+scope provenance
/// manifest (the same one `plan_materialize`'s delete-safety check uses,
/// covering every kind's writes together since it's keyed by relative path,
/// not kind). If so, there's nothing to reconcile from it: treating an
/// agent's own still-fresh output as new incoming content is exactly how
/// one kind's render can get misread as another kind's hand-edit — Cursor,
/// Windsurf, and Cline all render Skill and Rule into the very same files.
fn is_still_shaic_owned(manifest: &Manifest, root: &Path, discovered: &DiscoveredContent) -> bool {
    let Ok(relative) = discovered.source_path.strip_prefix(root) else {
        return false;
    };
    manifest.safe_to_delete(&relative.to_string_lossy(), &discovered.raw)
}

/// `discover_existing`, filtered down to content that isn't just shaic's own
/// still-fresh output (see `is_still_shaic_owned`). Every `reconcile_existing`
/// impl should iterate over this instead of calling `discover_existing`
/// directly. Yields nothing when this agent+scope has no resolvable root on
/// this machine (`Agent::root` returning `None`) — with no root there is no
/// manifest boundary to judge ownership against, and nothing was ever
/// written there to reconcile back.
pub fn discover_unowned(
    agent: &dyn Agent,
    kind: ItemKind,
    scope: Scope,
    project_root: &Path,
) -> Vec<DiscoveredContent> {
    let Some(root) = agent.root(scope, project_root) else {
        return Vec::new();
    };
    let manifest = Manifest::load(&Manifest::path_for(agent.id(), scope));
    agent
        .discover_existing(kind, scope, project_root)
        .into_iter()
        .filter(|d| !is_still_shaic_owned(&manifest, &root, d))
        .collect()
}

/// Frontmatter for an item recovered from an agent's own on-disk file. Only
/// `name` is ever carried by the plainest on-disk shapes, so everything else
/// starts empty and the caller fills in whatever its format actually encodes
/// (`Frontmatter { description, ..reconciled_frontmatter(name, scope) }`).
/// `scope` is always just the single scope being reconciled — an agent's file
/// never encodes scope at all; `reconcile_items` is what merges this back
/// together with the scopes the store already had.
pub fn reconciled_frontmatter(name: impl Into<String>, scope: Scope) -> Frontmatter {
    Frontmatter {
        name: name.into(),
        description: String::new(),
        applies_to: Vec::new(),
        tags: Vec::new(),
        scope: vec![scope],
        // Unlike `scope`, deliberately every agent, not just the one being
        // reconciled from — narrowing this by default would mean importing
        // a hand-written skill via any one agent silently opts every other
        // agent out of it, with no signal to the user that happened. Restrict
        // explicitly (`shaic item edit`) once it's in the store.
        agents: AgentId::ALL.to_vec(),
    }
}

/// The `name`/`description` YAML frontmatter + body shape of a `SKILL.md`
/// file — a cross-agent standard (Claude Code and Codex both read this exact
/// format verbatim, no per-agent translation), unlike `with_description`'s
/// description-only frontmatter used for commands/prompts.
pub fn format_skill(item: &Item) -> String {
    frontmatter_document(
        &SkillFrontmatter {
            name: item.name(),
            description: &item.frontmatter.description,
        },
        &item.body,
    )
}

/// The filename that carries a skill's own frontmatter+body, by cross-agent
/// convention: `<skills dir>/<name>/SKILL.md`.
pub const SKILL_FILE_NAME: &str = "SKILL.md";

/// The canonical item name for a file discovered by `discover_skill_files`,
/// taken from the **path**: `<skills>/<name>/SKILL.md` -> `<name>`, and a
/// flat `<skills>/<name>.md` -> `<name>`.
///
/// Deliberately not the file's own `name:` frontmatter field. The path is
/// what every other part of this crate keys on — render writes
/// `skills/<item name>/SKILL.md`, and the delete-safety manifest tracks that
/// relative path — so a hand-edited `skills/foo/SKILL.md` claiming
/// `name: bar` used to import as item `bar`, which then rendered a *second*
/// directory `skills/bar/` while `skills/foo/` stayed behind forever, with
/// the store and disk permanently disagreeing. When the two disagree the
/// path wins and the frontmatter `name` is ignored.
pub fn canonical_item_name(path: &Path) -> Option<String> {
    if path.file_name()?.to_str()? == SKILL_FILE_NAME {
        return path.parent()?.file_name()?.to_str().map(String::from);
    }
    file_stem_name(path)
}

/// Reverse of `format_skill`, tolerant on purpose: real `SKILL.md` files in
/// the wild carry extra keys (`allowed-tools`, `license`, ...) that shaic has
/// no canonical field for, and a missing or non-string `description` is
/// recovered as empty, which `reconcile_items` reads as "inherit whatever the
/// store already had" rather than "clear it". `None` only when there's no
/// frontmatter block to read at all.
fn parse_skill_document(raw: &str) -> Option<(String, String)> {
    let (fm, body) = split_frontmatter_block(raw)?;
    let value = parse_frontmatter_value(fm)?;
    let description = frontmatter_str(&value, "description")
        .unwrap_or_default()
        .to_string();
    Some((description, body.trim().to_string()))
}

/// Reconcile a location whose on-disk shape already matches the canonical
/// `name`/`description`+body frontmatter `format_skill` writes (currently
/// just `SKILL.md`) — no format translation needed, just the name taken from
/// the path (see `canonical_item_name`) and each discovered item re-scoped to
/// whichever scope is being reconciled right now, same reasoning as every
/// other `reconcile_existing` impl (an agent's file never encodes scope at
/// all).
pub fn reconcile_canonical_files(
    agent: &dyn Agent,
    kind: ItemKind,
    scope: Scope,
    project_root: &Path,
) -> Vec<Item> {
    discover_unowned(agent, kind, scope, project_root)
        .iter()
        .filter_map(|discovered| {
            let name = canonical_item_name(&discovered.source_path)?;
            let (description, body) = parse_skill_document(&discovered.raw)?;
            Item::new(
                kind,
                Frontmatter {
                    description,
                    ..reconciled_frontmatter(name, scope)
                },
                body,
            )
            .ok()
        })
        .collect()
}

pub fn file_stem_name(path: &Path) -> Option<String> {
    Some(path.file_stem()?.to_str()?.to_string())
}

/// Reverse of the `applies_to.join(",")` both Cursor's `globs` and Copilot's
/// `applyTo` are rendered with.
///
/// Brace-aware: a comma inside `{...}` is part of one glob's alternation, not
/// a separator. Splitting on every comma tore `{src,dist}/**/*.ts` into the
/// two nonsense patterns `{src` and `dist}/**/*.ts`, so a single sync could
/// silently change which files a rule applied to.
pub fn split_globs(raw: &str) -> Vec<String> {
    let mut globs = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    for ch in raw.chars() {
        match ch {
            '{' => {
                depth += 1;
                current.push(ch);
            }
            // Saturating: an unbalanced `}` (a malformed hand-edit) must not
            // wrap the depth counter and start splitting inside later braces.
            '}' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            ',' if depth == 0 => {
                push_glob(&mut globs, &current);
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    push_glob(&mut globs, &current);
    globs
}

fn push_glob(globs: &mut Vec<String>, candidate: &str) {
    let glob = candidate.trim();
    if !glob.is_empty() {
        globs.push(glob.to_string());
    }
}

fn items_from_heading_sections(
    kind: ItemKind,
    scope: Scope,
    raw: &str,
    heading: &str,
) -> Vec<Item> {
    // Only the shaic-managed block, never any hand-written text (or the
    // markers themselves) sitting outside it — otherwise a note left below
    // the block gets pulled into the last item's body and re-emitted inside
    // the block on the very next sync, growing without bound.
    let region_owned = crate::materialize::writer::managed_region(raw);
    let region = region_owned.as_deref().unwrap_or(raw);
    parse_heading_sections(region, heading)
        .into_iter()
        .filter_map(|(name, body)| Item::new(kind, reconciled_frontmatter(name, scope), body).ok())
        .collect()
}

/// Reconcile an agent whose whole `kind`+`scope` content lives in one
/// combined file (`CLAUDE.md`, `AGENTS.md`, `GEMINI.md`,
/// `copilot-instructions.md`): one item per `<heading> name` section. Nothing
/// but the name and body is recoverable, since `heading_section` never wrote
/// anything else.
pub fn reconcile_heading_sections(
    agent: &dyn Agent,
    kind: ItemKind,
    scope: Scope,
    project_root: &Path,
    heading: &str,
) -> Vec<Item> {
    discover_unowned(agent, kind, scope, project_root)
        .iter()
        .flat_map(|discovered| items_from_heading_sections(kind, scope, &discovered.raw, heading))
        .collect()
}

/// Everything that differs between the three agents (Cursor, Windsurf, Cline)
/// that support both a modern one-file-per-item directory *and* a legacy
/// single combined file. Extracted because all three otherwise duplicated the
/// same render branch, the same dual discovery, and the same "only Rule is
/// reversible" guard — three copies of logic whose bugs had to be found and
/// fixed three times. Their real differences (which paths, which extension,
/// which heading level, `.mdc` frontmatter vs a plain heading) stay explicit
/// at each adapter's call site.
pub struct DualForm {
    /// Directory holding one file per item, relative to the agent's `root()`.
    pub directory: PathBuf,
    /// Extension of the per-item files inside `directory`.
    pub extension: &'static str,
    /// The legacy single combined file, relative to the agent's `root()`.
    pub legacy_file: PathBuf,
    /// Heading level for sections in the legacy combined file.
    pub heading: &'static str,
}

/// Discover only the **active** form's content: the per-item directory if it
/// holds anything at all, otherwise the legacy combined file.
///
/// One decision point for both directions, on purpose. Both forms used to be
/// reported together, which meant `plan_materialize` rendered in directory
/// form (directory wins) while reconcile still imported the legacy file's
/// sections — and since single-file content was processed last, a stale
/// `.cursorrules` section named `foo` overwrote the item just reconciled from
/// `foo.mdc`, quietly reverting the user's edit. Now render and reconcile see
/// the same one form.
///
/// The legacy file is only *ignored*, never deleted: it's the user's file,
/// possibly still read by an older version of their editor, and shaic has no
/// business removing content it didn't write. It simply stops being treated
/// as a source of truth once the directory form exists.
pub fn discover_dual_form(form: &DualForm, root: &Path, scope: Scope) -> Vec<DiscoveredContent> {
    let per_item = discover_directory(&root.join(&form.directory), scope, form.extension);
    if !per_item.is_empty() {
        return per_item;
    }
    discover_single_file(&root.join(&form.legacy_file), scope)
}

/// Render for a `DualForm` agent: keep whichever form is already on disk
/// (`existing_form`, derived from `discover_dual_form` by
/// `plan::determine_existing_form`), defaulting to the modern directory form
/// for a project that has neither yet.
pub fn render_dual_form(
    form: &DualForm,
    items: &[Item],
    scope: Scope,
    existing_form: Option<ContentForm>,
    file_name: impl Fn(&Item) -> String,
    format_item: impl Fn(&Item) -> String,
) -> Vec<RenderedFile> {
    match existing_form {
        Some(ContentForm::SingleFile) => {
            render_as_single_file(form.legacy_file.clone(), scope, items, |item| {
                heading_section(item, form.heading)
            })
        }
        _ => render_as_directory(form.directory.clone(), scope, items, file_name, format_item),
    }
}

/// Reconcile a `DualForm` agent — the inverse of `render_dual_form`, reading
/// whichever single form `discover_dual_form` reports as active.
/// `from_file` recovers one item from one per-item file (the part whose
/// on-disk format genuinely differs per agent: Cursor's `.mdc` frontmatter vs
/// Windsurf's/Cline's plain leading heading); the legacy combined file is
/// always just split on `heading` lines.
///
/// Only `ItemKind::Rule` is reversible for all three: they render Skill and
/// Rule into the very same files with the very same format, so there is no
/// way to tell which kind a file on disk was meant as. Reversing for Skill
/// too would import every rule a second time under a second kind, and then
/// two store items would fight over one path forever.
pub fn reconcile_dual_form(
    agent: &dyn Agent,
    kind: ItemKind,
    scope: Scope,
    project_root: &Path,
    form: &DualForm,
    from_file: impl Fn(&DiscoveredContent) -> Option<Item>,
) -> Vec<Item> {
    if kind != ItemKind::Rule {
        return Vec::new();
    }
    let mut items = Vec::new();
    for discovered in discover_unowned(agent, kind, scope, project_root) {
        match discovered.form {
            ContentForm::Directory => items.extend(from_file(&discovered)),
            ContentForm::SingleFile => items.extend(items_from_heading_sections(
                kind,
                scope,
                &discovered.raw,
                form.heading,
            )),
        }
    }
    items
}

/// Reverse of directory-form `heading_section` rendering: the name comes from
/// the filename, the body from whatever follows the leading heading line.
pub fn item_from_heading_file(
    kind: ItemKind,
    scope: Scope,
    heading: &str,
    discovered: &DiscoveredContent,
) -> Option<Item> {
    let name = file_stem_name(&discovered.source_path)?;
    let body = parse_directory_heading_item(&discovered.raw, heading);
    Item::new(kind, reconciled_frontmatter(name, scope), body).ok()
}

/// Reconcile one-file-per-item content rendered by `with_description`.
/// `name_from` recovers the item name from the filename — not always the plain
/// stem, since Copilot's filenames carry a second, meaningful extension
/// segment.
pub fn reconcile_described_files(
    agent: &dyn Agent,
    kind: ItemKind,
    scope: Scope,
    project_root: &Path,
    name_from: impl Fn(&Path) -> Option<String>,
) -> Vec<Item> {
    discover_unowned(agent, kind, scope, project_root)
        .iter()
        .filter_map(|discovered| {
            let name = name_from(&discovered.source_path)?;
            let (description, body) = parse_with_description(&discovered.raw)?;
            Item::new(
                kind,
                Frontmatter {
                    description,
                    ..reconciled_frontmatter(name, scope)
                },
                body,
            )
            .ok()
        })
        .collect()
}

/// How deep a discovery walk descends below an agent's content directory.
///
/// Bounded rather than unlimited: these directories are user-controlled, and
/// an accidental symlink-free-but-deep tree (a checked-in `node_modules`
/// under `.claude/skills/`, say) shouldn't turn `shaic status` into a full
/// filesystem crawl. Deep enough for the nesting agents actually use, which
/// is more than the two levels this used to allow: Claude Code groups skills
/// (`skills/<group>/<name>/SKILL.md` is already three) and namespaces
/// commands by subdirectory, and neither was discoverable at all before.
const MAX_DISCOVERY_DEPTH: usize = 6;

pub fn discover_directory(dir: &Path, scope: Scope, ext: &str) -> Vec<DiscoveredContent> {
    let mut out = Vec::new();
    if !dir.exists() {
        return out;
    }
    for entry in walkdir::WalkDir::new(dir)
        .max_depth(MAX_DISCOVERY_DEPTH)
        // Never follow symlinks: discovery reads whatever it finds, and a
        // link out of the agent's directory would read from anywhere.
        .follow_links(false)
        .into_iter()
        .flatten()
    {
        if entry.file_type().is_file()
            && entry.path().extension().is_some_and(|e| e == ext)
            && let Ok(raw) = std::fs::read_to_string(entry.path())
        {
            out.push(DiscoveredContent {
                source_path: entry.path().to_path_buf(),
                scope,
                raw,
                form: ContentForm::Directory,
            });
        }
    }
    out
}

/// Discover a `SKILL.md`-per-directory tree: every `SKILL.md` at any depth
/// (so grouped skills like `skills/<group>/<name>/SKILL.md` are found), plus
/// flat `<name>.md` files sitting directly in `dir`.
///
/// Deliberately *not* every `*.md` in the tree, which is what a plain
/// `discover_directory` would return. A skill directory routinely holds
/// supporting Markdown that is part of the skill's payload
/// (`<name>/reference/api.md`, ...); importing those as items of their own
/// would invent one phantom item per support file, each of which shaic would
/// then render back out somewhere else.
pub fn discover_skill_files(dir: &Path, scope: Scope) -> Vec<DiscoveredContent> {
    let mut out = Vec::new();
    if !dir.exists() {
        return out;
    }
    for entry in walkdir::WalkDir::new(dir)
        .max_depth(MAX_DISCOVERY_DEPTH)
        .follow_links(false)
        .into_iter()
        .flatten()
    {
        if !entry.file_type().is_file() {
            continue;
        }
        // `depth >= 2` for `SKILL.md`: it takes its name from its parent
        // directory, so a stray `<dir>/SKILL.md` would import as an item
        // named after the skills directory itself.
        let is_skill_file =
            entry.depth() >= 2 && entry.file_name().to_str() == Some(SKILL_FILE_NAME);
        let is_flat_item =
            entry.depth() == 1 && entry.path().extension().is_some_and(|e| e == "md");
        if !(is_skill_file || is_flat_item) {
            continue;
        }
        if let Ok(raw) = std::fs::read_to_string(entry.path()) {
            out.push(DiscoveredContent {
                source_path: entry.path().to_path_buf(),
                scope,
                raw,
                form: ContentForm::Directory,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AgentId, Frontmatter, ItemKind};

    /// Descriptions that used to either inject frontmatter keys or produce
    /// unparseable YAML when the block was built with `format!`.
    const HOSTILE_DESCRIPTIONS: &[&str] = &[
        "legit\nalwaysApply: true\nbogus: ",
        "has: a colon",
        "*leading star",
        "#leading hash",
        "a \"double quote\" inside",
        "before\n---\nafter",
        "trailing space ",
        "",
    ];

    fn item(kind: ItemKind, name: &str, description: &str, body: &str) -> Item {
        Item::new(
            kind,
            Frontmatter {
                name: name.to_string(),
                description: description.to_string(),
                applies_to: vec![],
                tags: vec![],
                scope: vec![Scope::Project],
                agents: AgentId::ALL.to_vec(),
            },
            body.to_string(),
        )
        .expect("test item name is valid")
    }

    /// The keys actually present in a rendered document's frontmatter.
    fn frontmatter_keys(rendered: &str) -> Vec<String> {
        let (fm, _) = split_frontmatter_block(rendered).expect("rendered a frontmatter block");
        let value = parse_frontmatter_value(fm).expect("frontmatter parses as a mapping");
        value
            .as_mapping()
            .expect("checked mapping")
            .keys()
            .filter_map(|k| k.as_str().map(String::from))
            .collect()
    }

    #[test]
    fn heading_round_trip_preserves_body_lines_that_look_like_headings() {
        let item = item(ItemKind::Rule, "outer", "", "intro\n## Notes\nmore");
        let rendered = heading_section(&item, "##");
        let sections = parse_heading_sections(&rendered, "##");
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].0, "outer");
        assert_eq!(sections[0].1, "intro\n## Notes\nmore");
    }

    #[test]
    fn heading_escape_is_injective_across_backslash_depths() {
        // An author who deliberately wrote `\## Notes` must get exactly that
        // back: escaping only unescaped headings while unescaping any depth
        // silently rewrote their content.
        let body = "intro\n## Notes\n\\## Notes\n\\\\## Notes\ntail";
        let item = item(ItemKind::Rule, "outer", "", body);
        let rendered = heading_section(&item, "##");
        let sections = parse_heading_sections(&rendered, "##");
        assert_eq!(
            sections.len(),
            1,
            "escaped headings must not split the item"
        );
        assert_eq!(sections[0].1, body);
    }

    #[test]
    fn with_description_round_trips_hostile_descriptions_without_extra_keys() {
        for description in HOSTILE_DESCRIPTIONS {
            let item = item(ItemKind::Command, "cmd", description, "The body.");
            let rendered = with_description(&item);
            assert_eq!(
                frontmatter_keys(&rendered),
                vec!["description".to_string()],
                "rendered {rendered:?} must expose exactly one key"
            );
            let (parsed_description, body) =
                parse_with_description(&rendered).expect("round-trips");
            assert_eq!(&parsed_description, description);
            assert_eq!(body, "The body.");
        }
    }

    #[test]
    fn format_skill_round_trips_hostile_descriptions_without_extra_keys() {
        for description in HOSTILE_DESCRIPTIONS {
            let item = item(ItemKind::Skill, "skill", description, "The body.");
            let rendered = format_skill(&item);
            assert_eq!(
                frontmatter_keys(&rendered),
                vec!["name".to_string(), "description".to_string()],
                "rendered {rendered:?} must expose exactly name+description"
            );
            let (parsed_description, body) = parse_skill_document(&rendered).expect("round-trips");
            assert_eq!(&parsed_description, description);
            assert_eq!(body, "The body.");
        }
    }

    /// `split_frontmatter_block` finds the end of the block by looking for a
    /// `---` line at column 0, so nothing the emitter produces for a value may
    /// ever start one. It doesn't: multi-line values become indented block
    /// scalars and long ones fold with indentation, so a `---` from a
    /// description always lands indented. This pins that invariant, since the
    /// whole document shape depends on it.
    #[test]
    fn a_serialized_value_never_starts_a_column_zero_end_marker() {
        let long = format!("{} --- {}", "word ".repeat(60), "tail ".repeat(60));
        for description in
            HOSTILE_DESCRIPTIONS
                .iter()
                .copied()
                .chain(["---", "\n---\n", "a\n---", long.as_str()])
        {
            let rendered = with_description(&item(
                ItemKind::Command,
                "cmd",
                description,
                "The body.\n---\nstill body",
            ));
            let (fm, body) = split_frontmatter_block(&rendered).expect("frontmatter block");
            assert!(
                !fm.lines().any(|line| line == "---"),
                "frontmatter for {description:?} would close its own block: {fm}"
            );
            assert_eq!(body.trim(), "The body.\n---\nstill body");
            let (parsed, _) = parse_with_description(&rendered).expect("round-trips");
            // A value ending in a newline comes back one newline shorter,
            // because that newline is also the closing delimiter's — see
            // `frontmatter_document`. Everything else is exact.
            assert_eq!(
                parsed,
                description.strip_suffix('\n').unwrap_or(description)
            );
        }
    }

    #[test]
    fn parse_with_description_skips_non_string_and_absent_descriptions() {
        for fm in [
            "description:\n  - a list\n",
            "description: {nested: map}\n",
            "description:\n",
            "other: value\n",
            "- not a mapping\n",
            "just a scalar\n",
        ] {
            let raw = format!("---\n{fm}---\n\nbody\n");
            assert!(
                parse_with_description(&raw).is_none(),
                "expected {fm:?} to be skipped, not guessed at"
            );
        }
    }

    #[test]
    fn split_globs_keeps_brace_alternations_intact() {
        assert_eq!(
            split_globs("{src,dist}/**/*.ts"),
            vec!["{src,dist}/**/*.ts".to_string()]
        );
        assert_eq!(
            split_globs("{src,{a,b}}/**/*.ts, docs/*.md"),
            vec!["{src,{a,b}}/**/*.ts".to_string(), "docs/*.md".to_string()]
        );
        assert_eq!(
            split_globs("a,b"),
            vec!["a".to_string(), "b".to_string()],
            "a plain comma list must still split"
        );
        assert_eq!(split_globs(""), Vec::<String>::new());
        assert_eq!(
            split_globs("a, ,b"),
            vec!["a".to_string(), "b".to_string()],
            "empty entries are dropped, not kept as empty globs"
        );
    }

    #[test]
    fn brace_globs_survive_a_join_then_split_round_trip() {
        let applies_to = vec!["{src,dist}/**/*.ts".to_string(), "docs/*.md".to_string()];
        assert_eq!(split_globs(&applies_to.join(",")), applies_to);
    }

    #[test]
    fn canonical_item_name_comes_from_the_path() {
        assert_eq!(
            canonical_item_name(Path::new("/p/.claude/skills/foo/SKILL.md")).as_deref(),
            Some("foo")
        );
        assert_eq!(
            canonical_item_name(Path::new("/p/.claude/skills/group/foo/SKILL.md")).as_deref(),
            Some("foo"),
            "a grouped skill takes the immediate parent directory's name"
        );
        assert_eq!(
            canonical_item_name(Path::new("/p/.claude/skills/flat.md")).as_deref(),
            Some("flat")
        );
    }

    #[test]
    fn discover_skill_files_finds_nested_skills_but_not_support_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let skills = dir.path().join("skills");
        let nested = skills.join("group").join("my-skill");
        std::fs::create_dir_all(nested.join("reference")).expect("create dirs");
        std::fs::write(
            nested.join(SKILL_FILE_NAME),
            "---\nname: my-skill\n---\n\nx",
        )
        .expect("write skill");
        std::fs::write(nested.join("reference").join("api.md"), "support doc")
            .expect("write support doc");
        std::fs::write(skills.join("flat.md"), "---\nname: flat\n---\n\ny").expect("write flat");

        let found = discover_skill_files(&skills, Scope::Project);
        let mut names: Vec<String> = found
            .iter()
            .filter_map(|d| canonical_item_name(&d.source_path))
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec!["flat".to_string(), "my-skill".to_string()],
            "nested SKILL.md must be discovered; support files must not become items"
        );
    }
}
