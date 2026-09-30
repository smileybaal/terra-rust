//! The server binary: a thin shim over the kernel.
//!
//! This file is deliberately almost empty. The server's SHAPE is generated from the
//! sheet book, and the part of it that is hand-written behaviour is the kernel, so
//! there is nothing left for a `main` to do but hand over the command line.

use std::ffi::OsString;

/// Collect the arguments after `argv[0]` as Rust strings.
///
/// `std::env::args()` PANICS on an argument that is not valid Unicode: a non-UTF-8
/// byte on Unix, or a lone surrogate on Windows. A server launched with such an
/// argument would die with a Rust panic (`exit 101`) before it could report
/// anything, instead of starting. `args_os()` never panics, so it is the way to
/// read the command line; a value that has no UTF-8 form is converted lossily,
/// which is the closest a Rust `String` can come to the UTF-16 argument the C#
/// server would have received.
fn collect_argv<I: IntoIterator<Item = OsString>>(args: I) -> Vec<String> {
    args.into_iter().skip(1).map(|a| a.to_string_lossy().into_owned()).collect()
}

fn main() {
    let argv = collect_argv(std::env::args_os());
    std::process::exit(terraria_kernel::boot::run(&argv));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv0_is_dropped_and_the_rest_kept() {
        let argv = collect_argv(["prog".into(), "-port".into(), "7777".into()]);
        assert_eq!(argv, ["-port", "7777"]);
    }

    #[test]
    fn a_non_unicode_argument_does_not_panic() {
        let bad = non_unicode_os_string();
        // Before the fix this went through `env::args()` and the conversion to
        // String panicked; now the invalid code unit is replaced, not fatal.
        let argv = collect_argv([OsString::from("prog"), bad]);
        assert_eq!(argv.len(), 1);
        assert_eq!(argv[0], "\u{FFFD}x");
    }

    /// An `OsString` that has no valid UTF-8 form: a lone surrogate on Windows, a
    /// lone continuation byte on Unix.
    fn non_unicode_os_string() -> OsString {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            OsString::from_wide(&[0xD800, b'x' as u16])
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            OsString::from_vec(vec![0x80, b'x'])
        }
    }
}
