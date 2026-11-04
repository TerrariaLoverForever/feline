// SPDX-License-Identifier: GPL-3.0-or-later
//! Build script: Cargo runs this before compiling the crate. It turns the
//! `.slint` UI description into Rust code, which `slint::include_modules!()`
//! in `src/gui.rs` then pulls in.

fn main() {
    // "fluent-dark" is the built-in widget style used for standard widgets
    // such as the text field, so they match Feline's dark theme.
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    // A panic here only fails the build, with Slint's error message.
    slint_build::compile_with_config("ui/app.slint", config)
        .expect("the Slint UI failed to compile");
}
