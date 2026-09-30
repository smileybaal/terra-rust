//! The shim must survive a command line that is not valid Unicode.
//!
//! `std::env::args()` panics on an argument it cannot turn into a `String` (a
//! non-UTF-8 byte on Unix, a lone surrogate on Windows), which made the server
//! die with `exit 101` before it could report anything. This spawns the real
//! binary with such an argument and requires it to reach its own argument handling.
//!
//! The unusable `-port` is deliberate and must not be removed. The server SERVES now
//! (`kernel/net.rs`), so a run with a usable port never returns and this test would
//! hang forever; an unusable one makes it exit at once. The exit code is the check:
//! argument handling refuses the port with 1, while a panicking argv collection is
//! 101, so the difference is exactly what the test is looking for.

use std::ffi::OsString;
use std::process::Command;

/// An argument with no valid UTF-8 form.
fn non_unicode_arg() -> OsString {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        // A lone high surrogate: a UTF-16 code unit Windows can carry on a
        // command line, but not valid Unicode.
        OsString::from_wide(&[0xD800, b'x' as u16])
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        // A lone continuation byte is not valid UTF-8.
        OsString::from_vec(vec![0x80, b'x'])
    }
}

#[test]
fn a_non_utf8_argument_does_not_panic() {
    let exe = env!("CARGO_BIN_EXE_terraria-server");
    let out = Command::new(exe)
        .arg(non_unicode_arg())
        .args(["-port", "not-a-port"])
        .output()
        .expect("failed to spawn terraria-server");

    assert_eq!(
        out.status.code(),
        Some(1),
        "the argument handling should refuse the port and exit 1; a Rust panic exits 101. stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
