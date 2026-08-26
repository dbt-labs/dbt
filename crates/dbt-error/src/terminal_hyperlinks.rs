//! OSC 8 terminal hyperlinks for diagnostic file paths.
//!
//! [`CodeLocationWithFile`] formatting does not know which stream it is writing
//! to, so hyperlink emission is gated by a process-wide flag (with a thread-local
//! override for tests). The CLI enables the flag at startup when stderr is a TTY.
//! Non-TTY sinks must strip OSC 8 sequences; `console::strip_ansi_codes` does not.

use std::borrow::Cow;
use std::cell::Cell;
use std::io::IsTerminal;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// OSC 8 introducer: `ESC ] 8 ; ;`
const OSC8_START: &str = "\x1b]8;;";
/// OSC 8 / ST terminator: `ESC \`
const OSC8_ST: &str = "\x1b\\";
/// OSC 8 closer: `ESC ] 8 ; ; ESC \`
const OSC8_END: &str = "\x1b]8;;\x1b\\";

static ENABLED: AtomicBool = AtomicBool::new(false);

thread_local! {
    static OVERRIDE: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Enables or disables OSC 8 hyperlinks in diagnostic location formatting.
pub fn set_terminal_hyperlinks_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

/// Whether diagnostic locations should be wrapped in OSC 8 hyperlinks.
pub(crate) fn terminal_hyperlinks_enabled() -> bool {
    OVERRIDE.with(|cell| {
        cell.get()
            .unwrap_or_else(|| ENABLED.load(Ordering::Relaxed))
    })
}

/// Runs `f` with a thread-local hyperlink override, restoring the previous value after.
pub fn with_terminal_hyperlinks<F, R>(enabled: bool, f: F) -> R
where
    F: FnOnce() -> R,
{
    OVERRIDE.with(|cell| {
        let prev = cell.replace(Some(enabled));
        struct Reset(Option<bool>);
        impl Drop for Reset {
            fn drop(&mut self) {
                OVERRIDE.with(|cell| cell.set(self.0));
            }
        }
        let _reset = Reset(prev);
        f()
    })
}

/// Enables hyperlinks when stderr is an interactive TTY (and `TERM` is not `dumb`).
pub fn init_terminal_hyperlinks_from_stderr() {
    let dumb_term = std::env::var_os("TERM").is_some_and(|term| term == "dumb");
    set_terminal_hyperlinks_enabled(std::io::stderr().is_terminal() && !dumb_term);
}

/// Wraps `displayed` in an OSC 8 `file://` hyperlink targeting `path`.
///
/// Returns `displayed` unchanged when hyperlinks are disabled, `path` is not a
/// usable filesystem path, or a `file://` URI cannot be built.
pub(crate) fn wrap_file_hyperlink(path: &Path, displayed: &str) -> String {
    if !terminal_hyperlinks_enabled() {
        return displayed.to_string();
    }
    let Some(uri) = path_to_file_uri(path) else {
        return displayed.to_string();
    };
    format!("{OSC8_START}{uri}{OSC8_ST}{displayed}{OSC8_END}")
}

/// Converts `path` to a percent-encoded `file://` URI.
///
/// Relative paths are resolved against the current directory (the file does not
/// need to exist). Returns `None` for empty or placeholder paths.
pub(crate) fn path_to_file_uri(path: &Path) -> Option<String> {
    if !is_hyperlinkable_path(path) {
        return None;
    }
    let absolute = std::path::absolute(path).ok()?;
    #[cfg(windows)]
    let absolute = dunce::simplified(&absolute).to_path_buf();
    if !is_hyperlinkable_path(&absolute) {
        return None;
    }
    Some(encode_file_uri(&absolute))
}

fn is_hyperlinkable_path(path: &Path) -> bool {
    let s = path.to_string_lossy();
    !s.is_empty() && !s.contains('<') && !s.contains('>')
}

fn encode_file_uri(path: &Path) -> String {
    #[cfg(windows)]
    {
        encode_file_uri_windows(path)
    }
    #[cfg(not(windows))]
    {
        encode_file_uri_unix(path)
    }
}

#[cfg(not(windows))]
fn encode_file_uri_unix(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;

    let bytes = path.as_os_str().as_bytes();
    let mut uri = String::from("file://");
    if !bytes.starts_with(b"/") {
        uri.push('/');
    }
    append_encoded_bytes(&mut uri, bytes);
    uri
}

#[cfg(windows)]
fn encode_file_uri_windows(path: &Path) -> String {
    let mut normalized = path.to_string_lossy().replace('\\', "/");
    if let Some(rest) = normalized.strip_prefix("//?/") {
        normalized = rest.to_string();
    }
    if let Some(unc) = normalized.strip_prefix("//") {
        let mut uri = String::from("file://");
        append_encoded_bytes(&mut uri, unc.as_bytes());
        return uri;
    }
    let mut uri = String::from("file:///");
    append_encoded_bytes(&mut uri, normalized.as_bytes());
    uri
}

fn append_encoded_bytes(out: &mut String, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &b in bytes {
        if is_path_uri_byte(b) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0F) as usize] as char);
        }
    }
}

/// Unreserved RFC 3986 bytes, plus `/` and `:` (Windows drive letters).
const fn is_path_uri_byte(b: u8) -> bool {
    matches!(
        b,
        b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~'
            | b'/'
            | b':'
    )
}

/// Removes OSC 8 wrappers, keeping the visible hyperlink text.
///
/// `console::strip_ansi_codes` only matches CSI sequences (`ESC [`), not OSC 8
/// (`ESC ] 8 ; ; …`).
pub fn strip_osc8_hyperlinks(input: &str) -> Cow<'_, str> {
    if !input.contains(OSC8_START) {
        return Cow::Borrowed(input);
    }

    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if let Some(uri_end) = osc8_open_end(bytes, i) {
            i = uri_end;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    Cow::Owned(String::from_utf8(out).expect("OSC 8 wrappers are ASCII, so UTF-8 is preserved"))
}

/// If `bytes[i..]` starts an OSC 8 sequence, returns the index after its terminator.
fn osc8_open_end(bytes: &[u8], i: usize) -> Option<usize> {
    const PREFIX: &[u8] = b"\x1b]8;";
    if !bytes[i..].starts_with(PREFIX) {
        return None;
    }
    // Skip params (`id=…`) until the URI-separating `;`, then skip the URI.
    let after_params = find_byte(bytes, i + PREFIX.len(), b';')? + 1;
    (after_params..bytes.len()).find_map(|j| osc_terminator_end(bytes, j))
}

fn find_byte(bytes: &[u8], start: usize, needle: u8) -> Option<usize> {
    bytes[start..]
        .iter()
        .position(|&b| b == needle)
        .map(|offset| start + offset)
}

/// BEL (`0x07`) or ST (`ESC \`) that ends an OSC 8 introducer.
fn osc_terminator_end(bytes: &[u8], j: usize) -> Option<usize> {
    match bytes.get(j..) {
        Some([0x07, ..]) => Some(j + 1),
        Some([0x1b, b'\\', ..]) => Some(j + 2),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn disabled_by_default() {
        with_terminal_hyperlinks(false, || {
            assert!(!terminal_hyperlinks_enabled());
            let wrapped = wrap_file_hyperlink(Path::new("models/foo.sql"), "models/foo.sql:1:1");
            assert_eq!(wrapped, "models/foo.sql:1:1");
        });
    }

    #[test]
    fn wrap_emits_osc8_and_file_uri() {
        with_terminal_hyperlinks(true, || {
            let displayed = "models/foo.sql:43:9";
            let wrapped = wrap_file_hyperlink(Path::new("models/foo.sql"), displayed);
            assert!(
                wrapped.starts_with(OSC8_START),
                "expected OSC 8 start, got {wrapped:?}"
            );
            assert!(
                wrapped.ends_with(OSC8_END),
                "expected OSC 8 end, got {wrapped:?}"
            );
            assert!(wrapped.contains(displayed));
            assert!(wrapped.contains("file://"));
            assert!(
                wrapped.contains("/models/foo.sql"),
                "URI should include absolute path, got {wrapped:?}"
            );
        });
    }

    #[test]
    fn spaces_are_percent_encoded() {
        with_terminal_hyperlinks(true, || {
            let wrapped =
                wrap_file_hyperlink(Path::new("models/my model.sql"), "models/my model.sql:1:1");
            assert!(
                wrapped.contains("my%20model.sql"),
                "expected percent-encoded space, got {wrapped:?}"
            );
            let uri_end = wrapped.find(OSC8_ST).expect("OSC 8 terminator");
            assert!(
                !wrapped[..uri_end].contains("my model.sql"),
                "URI must not contain a raw space, got {wrapped:?}"
            );
        });
    }

    #[test]
    fn placeholder_paths_are_not_hyperlinked() {
        with_terminal_hyperlinks(true, || {
            let wrapped = wrap_file_hyperlink(Path::new("<unknown>"), "<unknown>");
            assert_eq!(wrapped, "<unknown>");
        });
    }

    #[test]
    fn strip_removes_wrappers_keeps_text() {
        let wrapped = with_terminal_hyperlinks(true, || {
            wrap_file_hyperlink(Path::new("models/foo.sql"), "models/foo.sql:43:9")
        });
        let stripped = strip_osc8_hyperlinks(&wrapped);
        assert_eq!(stripped.as_ref(), "models/foo.sql:43:9");
        assert!(!stripped.contains('\x1b'));
    }

    #[test]
    fn strip_handles_bel_terminator() {
        let input = "\x1b]8;;file:///tmp/a.sql\x07a.sql:1:1\x1b]8;;\x07";
        assert_eq!(strip_osc8_hyperlinks(input).as_ref(), "a.sql:1:1");
    }

    #[test]
    fn strip_is_borrowed_when_absent() {
        let s = "models/foo.sql:43:9";
        match strip_osc8_hyperlinks(s) {
            Cow::Borrowed(b) => assert_eq!(b, s),
            Cow::Owned(_) => panic!("expected borrowed"),
        }
    }

    #[test]
    fn file_uri_is_absolute() {
        let uri = path_to_file_uri(Path::new("models/foo.sql")).expect("uri");
        assert!(uri.starts_with("file://"));
        assert!(uri.contains("/models/foo.sql"));
        assert!(!uri.contains(' '));
    }

    #[test]
    fn strip_preserves_utf8_text() {
        let wrapped = with_terminal_hyperlinks(true, || {
            wrap_file_hyperlink(Path::new("models/café.sql"), "models/café.sql:1:1")
        });
        assert_eq!(
            strip_osc8_hyperlinks(&wrapped).as_ref(),
            "models/café.sql:1:1"
        );
    }

    #[test]
    fn thread_local_override_restores() {
        with_terminal_hyperlinks(false, || {
            assert!(!terminal_hyperlinks_enabled());
            with_terminal_hyperlinks(true, || {
                assert!(terminal_hyperlinks_enabled());
            });
            assert!(!terminal_hyperlinks_enabled());
        });
    }
}
