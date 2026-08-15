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

/// Color for a dashboard/status glyph string (`"in-sync"`, `"drift"`, ...).
pub fn glyph_color(glyph: &str) -> Color {
    match glyph {
        "in-sync" => SUCCESS,
        "drift" => WARNING,
        "unconfirmed" | "experimental, read-only" => INFO,
        _ => DANGER,
    }
}

/// Icon paired with `glyph_color` for the same glyph string, so a status
/// scans by shape as well as by color (readable at a glance, and still
/// meaningful for anyone colorblind or piping output through `less`).
pub fn glyph_icon(glyph: &str) -> &'static str {
    match glyph {
        "in-sync" => "●",
        "drift" => "▲",
        "unconfirmed" | "experimental, read-only" => "◐",
        _ => "✕",
    }
}

/// Best-effort color for a free-form status-line message, based on the
/// vocabulary this crate's own `app.message`/error strings actually use.
/// Wrong guesses are harmless (cosmetic only), so a substring heuristic is
/// fine here — no need for every call site to tag its own message kind.
pub fn message_color(message: &str) -> Color {
    let lower = message.to_lowercase();
    if lower.contains("error")
        || lower.contains("failed")
        || lower.contains("could not")
        || lower.contains("not registered")
    {
        DANGER
    } else if lower.starts_with("no ") {
        WARNING
    } else if lower.contains("applied")
        || lower.contains("ready")
        || lower.contains("removed")
        || lower.contains("saved")
        || lower.contains("pushed")
        || lower.contains("pulled")
        || lower.contains("in sync")
    {
        SUCCESS
    } else {
        Color::Reset
    }
}

/// Distinct color per item kind, so a mixed skills/rules/commands list scans
/// at a glance instead of requiring the `[Kind]` prefix to be read.
pub fn kind_color(kind: ItemKind) -> Color {
    match kind {
        ItemKind::Skill => INFO,
        ItemKind::Rule => Color::Rgb(45, 212, 191),
        ItemKind::Command => Color::Rgb(232, 121, 249),
    }
}
