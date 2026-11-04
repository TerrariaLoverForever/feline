// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! `feline-runtime`: Feline's own runtime (in development).
//!
//! So far `--check` (read the engine library and report what loading it
//! involves) and `--load` (also put it into memory and relocate it). Neither
//! runs any of its code.

use std::process::ExitCode;

const HELP: &str = "\
feline-runtime: Feline's own runtime for Roblox's Android client (in development)

Usage: feline-runtime --check <libroblox.so>
       feline-runtime --load <libroblox.so>
       feline-runtime --imports <libroblox.so>

--check  reads the engine library and reports what loading it would involve.
--load   also puts it into memory: binds every import (forwarded to glibc
         or OpenGL ES, Feline data, or a stub), applies all relocations and
         sets memory permissions. Then it frees the memory and exits.
--imports  like --load, and lists every import by how it was bound.

Neither runs any of the library's code. Roblox's code isn't disassembled;
only its ELF interface is read.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [flag, path] if flag == "--check" => {
            let data = match std::fs::read(path) {
                Ok(data) => data,
                Err(e) => {
                    eprintln!("feline-runtime: couldn't read {path}: {e}");
                    return ExitCode::FAILURE;
                }
            };
            match feline_runtime::check::check(&data) {
                Ok(report) => {
                    print!("{}", report.to_text());
                    if report.is_consistent() {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::FAILURE
                    }
                }
                Err(e) => {
                    eprintln!("feline-runtime: {path}: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        [flag, path] if flag == "--load" || flag == "--imports" => {
            let data = match std::fs::read(path) {
                Ok(data) => data,
                Err(e) => {
                    eprintln!("feline-runtime: couldn't read {path}: {e}");
                    return ExitCode::FAILURE;
                }
            };
            match load_with_imports(&data, flag == "--imports") {
                Ok(text) => {
                    print!("{text}");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("feline-runtime: {path}: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        [flag] if flag == "-h" || flag == "--help" => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        _ => {
            eprint!("{HELP}");
            ExitCode::from(2)
        }
    }
}

/// Binds the imports, loads the library, and describes the result.
fn load_with_imports(data: &[u8], list: bool) -> Result<String, String> {
    use feline_runtime::imports::{self, Kind};
    use std::fmt::Write as _;

    let elf = feline_runtime::elf::Elf::parse(data).map_err(|e| e.to_string())?;
    let symbols = elf.symbols().map_err(|e| e.to_string())?;
    let host = feline_runtime::host::Host::open();
    let imports = imports::bind(&symbols, &host)?;
    let (image, report) =
        feline_runtime::load::load(&elf, Some(&imports)).map_err(|e| e.to_string())?;

    let mut out = report.to_text(&image);
    let kinds = [
        Kind::ForwardLibc,
        Kind::ForwardLibm,
        Kind::ForwardGles,
        Kind::ForwardData,
        Kind::FelineData,
        Kind::Stub,
        Kind::WeakEmpty,
    ];
    let _ = writeln!(out, "Bound           {} imports:", imports.bindings.len());
    for kind in kinds {
        let _ = writeln!(out, "  {:>4}  {}", imports.count(kind), kind.label());
    }
    if !host.has_gles() {
        let _ = writeln!(
            out,
            "Note            no libGLESv2.so.2 found; OpenGL ES functions are stubs"
        );
    }
    if list {
        for kind in kinds {
            let names = imports.names(kind);
            if !names.is_empty() {
                let _ = writeln!(
                    out,
                    "\n{} ({}):\n  {}",
                    kind.label(),
                    names.len(),
                    names.join(" ")
                );
            }
        }
    }
    Ok(out)
}
