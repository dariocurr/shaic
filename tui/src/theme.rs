use ratatui::style::Color;

use shaic_core::model::ItemKind;

/// shaic's signature color — used for the wordmark, focused panel borders,
/// and any other "this is shaic" accent. Everything else in the palette is
/// semantic (status/kind); this is the one purely aesthetic choice, so it's
/// spent in exactly one place per screen rather than smeared everywhere.
pub const ACCENT: Color = Color::Rgb(124, 108, 255);

pub const SUCCESS: Color = Color::Rgb(61, 214, 140);
pub const WARNING: Color = Color::Rgb(245, 194, 66);
pub const DANGER: Color = Color::Rgb(255, 92, 122);
pub const INFO: Color = Color::Rgb(122, 162, 247);

/// Background tint for the selected row of a list/table. A solid tint reads
/// consistently across terminal themes; `Modifier::REVERSED` doesn't — it
/// swaps whatever fg/bg a cell already carries, so an already-colored glyph
/// cell looks different when selected than an uncolored one next to it.
pub const SELECTION_BG: Color = Color::Rgb(38, 33, 64);

pub const WORDMARK: &str = "⟡ shaic";

/// Machine-readable sync status. Replaces the old `&'static str` glyphs
/// (`"in-sync"`, `"drift"`, ...) so a typo becomes a compile error instead of
/// a silent red `✕`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    InSync,
    Drift,
    Unconfirmed,
    Error,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::InSync => "in-sync",
            Status::Drift => "drift",
            Status::Unconfirmed => "unconfirmed",
            Status::Error => "error",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Status::InSync => "●",
            Status::Drift => "▲",
            Status::Unconfirmed => "◐",
            Status::Error => "✕",
        }
    }

    pub fn color(self) -> Color {
        match self {
            Status::InSync => SUCCESS,
            Status::Drift => WARNING,
            Status::Unconfirmed => INFO,
            Status::Error => DANGER,
        }
    }

    fn rank(self) -> u8 {
        match self {
            Status::Error => 0,
            Status::Drift => 1,
            Status::Unconfirmed => 2,
            Status::InSync => 3,
        }
    }

    /// Worst-of across sub-rows: a single scope problem is visible at top level.
    pub fn worst(statuses: impl Iterator<Item = Status>) -> Status {
        statuses.min_by_key(|s| s.rank()).unwrap_or(Status::InSync)
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<&str> for Status {
    fn from(s: &str) -> Self {
        match s {
            "in-sync" => Status::InSync,
            "drift" => Status::Drift,
            "unconfirmed" | "experimental, read-only" => Status::Unconfirmed,
            _ => Status::Error,
        }
    }
}

/// Explicit message severity. Call sites tag their own message instead of
/// relying on substring sniffing — wrong guesses were harmless cosmetically
/// but hid real errors behind `Reset` coloring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MessageKind {
    Success,
    Warning,
    #[default]
    Info,
    Error,
}

impl MessageKind {
    pub fn color(self) -> Color {
        match self {
            MessageKind::Success => SUCCESS,
            MessageKind::Warning => WARNING,
            MessageKind::Error => DANGER,
            MessageKind::Info => Color::Reset,
        }
    }
}

/// Explicit color for a tagged status-line message.
pub fn message_color_for_kind(kind: MessageKind) -> Color {
    kind.color()
}

/// Legacy fallback for free-form strings (wizard status) that don't carry a
/// `MessageKind` yet. Kept substring-based; prefer tagging new messages.
pub fn message_color(message: &str) -> Color {
    message_kind_for_legacy(message).color()
}

/// Best-effort kind for a free-form message, based on the vocabulary this
/// crate's own messages actually use.
pub fn message_kind_for_legacy(message: &str) -> MessageKind {
    let lower = message.to_lowercase();
    if lower.contains("error")
        || lower.contains("failed")
        || lower.contains("could not")
        || lower.contains("not registered")
    {
        MessageKind::Error
    } else if lower.starts_with("no ") {
        MessageKind::Warning
    } else if lower.contains("applied")
        || lower.contains("ready")
        || lower.contains("removed")
        || lower.contains("saved")
        || lower.contains("pushed")
        || lower.contains("pulled")
        || lower.contains("in sync")
    {
        MessageKind::Success
    } else {
        MessageKind::Info
    }
}

/// Distinct color per item kind, so a mixed skills/rules/commands/subagents
/// list scans at a glance instead of requiring the `[Kind]` prefix to be read.
pub fn kind_color(kind: ItemKind) -> Color {
    match kind {
        ItemKind::Skill => INFO,
        ItemKind::Rule => Color::Rgb(45, 212, 191),
        ItemKind::Command => Color::Rgb(232, 121, 249),
        ItemKind::Subagent => Color::Rgb(251, 146, 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_worst_prefers_error_over_drift_over_unconfirmed() {
        assert_eq!(
            Status::worst([Status::InSync, Status::Drift].into_iter()),
            Status::Drift
        );
        assert_eq!(
            Status::worst([Status::InSync, Status::Unconfirmed].into_iter()),
            Status::Unconfirmed
        );
        assert_eq!(
            Status::worst([Status::Drift, Status::Error].into_iter()),
            Status::Error
        );
        assert_eq!(Status::worst([].into_iter()), Status::InSync);
    }

    #[test]
    fn status_from_str_maps_legacy_readonly_to_unconfirmed() {
        assert_eq!(Status::from("in-sync"), Status::InSync);
        assert_eq!(Status::from("experimental, read-only"), Status::Unconfirmed);
        assert_eq!(Status::from("bogus"), Status::Error);
    }

    #[test]
    fn legacy_message_heuristic_tags_errors_and_success() {
        assert_eq!(
            message_kind_for_legacy("push failed: boom"),
            MessageKind::Error
        );
        assert_eq!(
            message_kind_for_legacy("pushed 3 items"),
            MessageKind::Success
        );
        assert_eq!(message_kind_for_legacy("q=quit"), MessageKind::Info);
    }
}
