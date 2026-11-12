// SPDX-License-Identifier: GPL-3.0-or-later
//! Build script: Cargo runs this before compiling the crate. It turns the
//! `.slint` UI description into Rust code, which `slint::include_modules!()`
//! in `src/gui.rs` then pulls in.

fn main() {
    // "fluent" is the built-in style for standard widgets such as the text
    // field. Its light/dark scheme is switched at run time to follow
    // Feline's own (app.slint sets `Palette.color-scheme`).
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    // A panic here only fails the build, with Slint's error message.
    slint_build::compile_with_config("ui/app.slint", config)
        .expect("the Slint UI failed to compile");
}
