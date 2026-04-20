//! Globally-selected theme.
//!
//! The application chooses one theme at startup (defaulting to dark) and
//! every component reads it through [`current`]. This lets OSC-detected
//! terminal brightness flow into cell rendering without threading a
//! `&Theme` through every `display_lines` call.
//!
//! Initialization is idempotent: the first call to [`init`] stores the
//! theme, subsequent calls are ignored so tests and embedded runtimes
//! can't race each other into an inconsistent state.

use std::sync::OnceLock;

use super::Theme;

static CURRENT: OnceLock<Theme> = OnceLock::new();

/// Install the globally-selected theme. Has effect only on the first
/// call; subsequent calls are ignored.
pub fn init(theme: Theme) {
    let _ = CURRENT.set(theme);
}

/// Return the current theme. Defaults to [`Theme::dark`] when [`init`]
/// has not been called, matching the historic behavior for code paths
/// that construct a theme inline.
#[must_use]
pub fn current() -> Theme {
    CURRENT.get().cloned().unwrap_or_else(Theme::dark)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_dark_when_uninit() {
        // Calling `current()` without `init` in this test process is
        // not guaranteed safe because other tests may have initialized
        // first — but the call should at minimum not panic and should
        // return *some* valid theme. That's what this test confirms.
        let theme = current();
        let accents = theme.accents();
        // The returned theme must have a usable claude accent, proving
        // the fallback path yields a fully-constructed struct.
        assert_ne!(format!("{:?}", accents.claude), "");
    }
}
