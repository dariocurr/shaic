//! Path-authority checks that must hold on every OS, plus Windows-only
//! absolute-path cases. The `validate_name` backslash rejection is what keeps
//! `a\b` from becoming two components on Windows (on Unix it would be one
//! valid filename — hence the explicit reject).

use shaic_core::model::validate_name;
use shaic_core::security::path_guard::ensure_within;
use std::path::Path;

#[test]
fn validate_name_rejects_backslash_on_every_os() {
    for bad in ["a\\b", "a/b", "..", "", "a/../b"] {
        assert!(
            validate_name(bad).is_err(),
            "expected {bad:?} to be rejected"
        );
    }
    assert!(validate_name("code-review-checklist").is_ok());
}

#[test]
fn ensure_within_rejects_dotdot_without_creating_root() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("not-yet-created");
    let err = ensure_within(&missing, Path::new("../../etc/passwd"));
    assert!(err.is_err(), "expected escape to be rejected");
    assert!(
        !missing.exists(),
        "pure check must not create speculative directories"
    );
}

#[test]
#[cfg(windows)]
fn windows_absolute_override_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    // Drive-relative and drive-absolute overrides must not escape the root.
    for candidate in [
        "C:\\Windows\\System32\\evil.md",
        "C:/Windows/System32/evil.md",
        "\\\\server\\share\\evil.md",
    ] {
        let err = ensure_within(tmp.path(), Path::new(candidate));
        assert!(
            err.is_err(),
            "expected {candidate:?} to be rejected under {}",
            tmp.path().display()
        );
    }
}
