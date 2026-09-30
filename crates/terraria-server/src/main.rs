//! The server binary: a thin shim over the kernel.
//!
//! This file is deliberately almost empty. The server's SHAPE is generated from the
//! sheet book, and the part of it that is hand-written behaviour is the kernel, so
//! there is nothing left for a `main` to do but hand over the command line.

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(terraria_kernel::boot::run(&argv));
}
