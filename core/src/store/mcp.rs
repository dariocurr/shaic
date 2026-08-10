use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::mcp::McpServer;
use crate::model::validate_name;
use crate::security::secret_scan;

use super::Store;

fn mcp_dir(store_root: &std::path::Path) -> PathBuf {
    store_root.join("mcp")
}

fn mcp_path(store_root: &std::path::Path, name: &str) -> PathBuf {
    mcp_dir(store_root).join(format!("{name}.toml"))
}

/// `(name, message)` — `name` is set only when the skipped file's own name
/// was valid (see `Store::list_mcp_servers`).
pub type SkippedMcpServer = (String, String);

impl Store {
    pub fn save_mcp_server(&self, server: &McpServer) -> Result<()> {
        self.check_schema()?;
        let path = mcp_path(&self.root, &server.name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| Error::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let toml_str =
            toml::to_string_pretty(server).map_err(|e| Error::FrontmatterParse(e.to_string()))?;
        // Belt-and-suspenders: even though secrets are supposed to only ever
        // be `{ secret = "NAME" }` references, catch an obvious literal
        // credential accidentally pasted into an env value before it's even
        // written to disk (the real backstop is the push-time scan, but
        // failing here is cheaper and more specific). Deliberately does NOT
        // run `frontmatter_limits::validate_raw` — that rejects lines
        // containing YAML anchor/merge syntax (` &`, ` *`, ...), which is
        // meaningless for TOML and false-positives on ordinary `args` like
        // `["-c", "run *.py"]`.
        secret_scan::scan_or_reject(&toml_str, false)?;
        std::fs::write(&path, toml_str).map_err(|source| Error::Io { path, source })
    }

    pub fn load_mcp_server(&self, name: &str) -> Result<McpServer> {
        self.check_schema()?;
        validate_name(name)?;
        let path = mcp_path(&self.root, name);
        let raw = std::fs::read_to_string(&path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        crate::mcp::parse_mcp_toml(&raw)
    }

    pub fn remove_mcp_server(&self, name: &str) -> Result<()> {
        self.check_schema()?;
        validate_name(name)?;
        let path = mcp_path(&self.root, name);
        std::fs::remove_file(&path).map_err(|source| Error::Io { path, source })
    }

    /// Skips any `mcp/*.toml` file that fails to parse or has an invalid
    /// name, rather than failing the whole call — one malformed file would
    /// otherwise take down every `shaic status`/`sync` call for every agent
    /// and scope, not just MCP. The second return value carries one entry per
    /// skip: `(name, message)`, with `name` set only when the file's own name
    /// was valid (i.e. it *could* be a manifest-tracked server that just
    /// failed to parse this time) — callers that need to keep such a server
    /// from being mistaken for one that left the store use that name; callers
    /// that only want to display the warning can ignore it.
    pub fn list_mcp_servers(&self) -> Result<(Vec<McpServer>, Vec<SkippedMcpServer>)> {
        self.check_schema()?;
        let dir = mcp_dir(&self.root);
        if !dir.exists() {
            return Ok((Vec::new(), Vec::new()));
        }
        let mut servers = Vec::new();
        let mut skipped = Vec::new();
        for entry in walkdir::WalkDir::new(&dir).max_depth(1).follow_links(false) {
            let entry = entry.map_err(|e| Error::Io {
                path: dir.clone(),
                source: std::io::Error::other(e.to_string()),
            })?;
            if !entry.file_type().is_file() || entry.path().extension().is_none_or(|e| e != "toml")
            {
                continue;
            }
            let Some(name) = entry.path().file_stem().and_then(|n| n.to_str()) else {
                skipped.push((
                    String::new(),
                    format!("{} — not a valid UTF-8 filename", entry.path().display()),
                ));
                continue;
            };
            if validate_name(name).is_err() {
                skipped.push((
                    String::new(),
                    format!(
                        "{} — {name:?} isn't a valid MCP server name",
                        entry.path().display()
                    ),
                ));
                continue;
            }
            match self.load_mcp_server(name) {
                Ok(server) => servers.push(server),
                Err(e) => skipped.push((name.to_string(), format!("MCP server {name:?} — {e}"))),
            }
        }
        Ok((servers, skipped))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::model::Scope;

    fn server_with_args(name: &str, args: Vec<&str>) -> McpServer {
        McpServer::new(
            name.to_string(),
            "npx".to_string(),
            args.into_iter().map(str::to_string).collect(),
            BTreeMap::new(),
            vec![Scope::Project],
        )
        .unwrap()
    }

    #[test]
    fn save_accepts_args_containing_yaml_anchor_like_characters() {
        // Regression: `frontmatter_limits::validate_raw` (a YAML-only check)
        // used to run against this TOML content and reject any line
        // containing " *" or " &" — both common in ordinary shell args.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path().join("store"), None).unwrap();
        let server = server_with_args("globby", vec!["-c", "run *.py && echo done"]);
        store.save_mcp_server(&server).unwrap();
        let loaded = store.load_mcp_server("globby").unwrap();
        assert_eq!(loaded.args, vec!["-c", "run *.py && echo done"]);
    }

    #[test]
    fn list_skips_a_malformed_file_instead_of_failing_the_whole_call() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path().join("store"), None).unwrap();
        store
            .save_mcp_server(&server_with_args("good", vec![]))
            .unwrap();
        std::fs::write(mcp_path(&store.root, "broken"), "not valid toml {{{").unwrap();

        let (servers, skipped) = store.list_mcp_servers().unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "good");
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].0, "broken");
    }
}
