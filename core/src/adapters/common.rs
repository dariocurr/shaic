use std::path::{Path, PathBuf};

use crate::materialize::writer::Manifest;
use crate::model::{AgentId, ContentForm, Frontmatter, Item, ItemKind, Scope};

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
/// single-file agents, `# name` for one-file-per-item agents).
pub fn heading_section(item: &Item, heading: &str) -> String {
    format!("{heading} {}\n\n{}", item.name(), item.body.trim())
}

/// The common "frontmatter with just a description, then the body" shape
/// shared by several agents' command/prompt/workflow files.
pub fn with_description(item: &Item) -> String {
    format!(
        "---\ndescription: {}\n---\n\n{}",
        item.frontmatter.description,
        item.body.trim()
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
pub fn split_frontmatter_block(raw: &str) -> Option<(&str, &str)> {
    let rest = raw.strip_prefix("---\n")?;
    let marker = "\n---\n";
    let idx = rest.find(marker)?;
    let (fm, after) = rest.split_at(idx);
    Some((fm, &after[marker.len()..]))
}

/// Reverse of `with_description`: pull `(description, body)` back out of a
/// `---\ndescription: ...\n---\n\n{body}` block. `None` if `raw` isn't in
/// that shape at all — callers skip a file like that rather than guessing.
pub fn parse_with_description(raw: &str) -> Option<(String, String)> {
    let (fm, body) = split_frontmatter_block(raw)?;
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(fm).ok()?;
    let description = value["description"].as_str()?.to_string();
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
            body.push_str(line);
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
/// directly.
pub fn discover_unowned(
    agent: &dyn Agent,
    kind: ItemKind,
    scope: Scope,
    project_root: &Path,
) -> Vec<DiscoveredContent> {
    let root = agent.root(scope, project_root);
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
    format!(
        "---\nname: {}\ndescription: {}\n---\n\n{}",
        item.name(),
        item.frontmatter.description,
        item.body.trim()
    )
}

/// Reconcile a location whose on-disk shape already matches the canonical
/// `name`/`description`+body frontmatter `format_skill` writes (currently
/// just `SKILL.md`) — no format translation needed, just re-scope each
/// discovered item to whichever scope is being reconciled right now, same
/// reasoning as every other `reconcile_existing` impl (an agent's file never
/// encodes scope at all).
pub fn reconcile_canonical_files(
    agent: &dyn Agent,
    kind: ItemKind,
    scope: Scope,
    project_root: &Path,
) -> Vec<Item> {
    discover_unowned(agent, kind, scope, project_root)
        .iter()
        .filter_map(|discovered| {
            let mut item = crate::store::parse_item(kind, &discovered.raw).ok()?;
            item.frontmatter.scope = vec![scope];
            Some(item)
        })
        .collect()
}

pub fn file_stem_name(path: &Path) -> Option<String> {
    Some(path.file_stem()?.to_str()?.to_string())
}

/// Reverse of the `applies_to.join(",")` both Cursor's `globs` and Copilot's
/// `applyTo` are rendered with.
pub fn split_globs(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|glob| !glob.is_empty())
        .map(String::from)
        .collect()
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
    let region = crate::materialize::writer::managed_region(raw).unwrap_or(raw);
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

/// Reconcile a location that may hold either one file per item or a single
/// legacy combined file (`.cursorrules`, `.windsurfrules`, `.clinerules`) —
/// `discover_existing` returns both mixed together for these agents.
/// `from_file` recovers one item from one per-item file (that's the part
/// whose on-disk format differs per agent); the combined form is always just
/// split on `heading` lines.
pub fn reconcile_per_file_or_combined(
    agent: &dyn Agent,
    kind: ItemKind,
    scope: Scope,
    project_root: &Path,
    heading: &str,
    from_file: impl Fn(&DiscoveredContent) -> Option<Item>,
) -> Vec<Item> {
    let mut items = Vec::new();
    for discovered in discover_unowned(agent, kind, scope, project_root) {
        match discovered.form {
            ContentForm::Directory => items.extend(from_file(&discovered)),
            ContentForm::SingleFile => items.extend(items_from_heading_sections(
                kind,
                scope,
                &discovered.raw,
                heading,
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

pub fn discover_directory(dir: &Path, scope: Scope, ext: &str) -> Vec<DiscoveredContent> {
    let mut out = Vec::new();
    if !dir.exists() {
        return out;
    }
    for entry in walkdir::WalkDir::new(dir)
        .max_depth(2)
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
