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
       feline-runtime --init <libroblox.so>

--check  reads the engine library and reports what loading it would involve.
--load   also puts it into memory: binds every import (forwarded to glibc
         or OpenGL ES, Feline data, or a stub), applies all relocations and
         sets memory permissions. Then it frees the memory and exits.
--imports  like --load, and lists every import by how it was bound.
--init   like --load, then RUNS the engine's constructors (its start-up code),
         the first of its code to run. Stops at the first function Feline
         doesn't provide yet, and names it. Run it without network access,
         e.g. `unshare -rn feline-runtime --init …`.

Only --init runs any of the library's code. Roblox's code isn't disassembled;
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
        [flag, path] if flag == "--init" => {
            let data = match std::fs::read(path) {
                Ok(data) => data,
                Err(e) => {
                    eprintln!("feline-runtime: couldn't read {path}: {e}");
                    return ExitCode::FAILURE;
                }
            };
            // The image and imports must stay alive while the engine's code
            // may run, so this never returns normally.
            run_init(&data)
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
        Kind::Feline,
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

/// `--init`: load, bind, run the constructors, report, and exit without
/// running the engine's exit handlers.
fn run_init(data: &[u8]) -> ExitCode {
    let result = (|| -> Result<String, String> {
        let elf = feline_runtime::elf::Elf::parse(data).map_err(|e| e.to_string())?;
        let symbols = elf.symbols().map_err(|e| e.to_string())?;
        let host = feline_runtime::host::Host::open();
        // Leaked on purpose: the engine's code and its pointers into Feline's
        // data must stay valid until the process ends.
        let imports = Box::leak(Box::new(feline_runtime::imports::bind(&symbols, &host)?));
        let (image, _) =
            feline_runtime::load::load(&elf, Some(imports)).map_err(|e| e.to_string())?;
        let image = Box::leak(Box::new(image));
        eprintln!(
            "feline-runtime: running {} constructors…",
            elf.init_array_len()
        );
        let ran = feline_runtime::init::run_constructors(&elf, image)?;
        eprintln!("feline-runtime: calling JNI_OnLoad…");
        let version = feline_runtime::init::call_jni_onload(&elf, image)?;
        let world = feline_runtime::jni::summary();
        let mut per_class: std::collections::BTreeMap<&str, usize> = Default::default();
        for (class, _, _) in &world.natives {
            *per_class.entry(class.as_str()).or_insert(0) += 1;
        }
        let mut text = format!(
            "Constructors    all {ran} ran and returned\n\
             JNI_OnLoad      returned version {version:#x}\n\
             Java world      {} classes looked up, {} methods/fields, {} natives registered\n",
            world.classes.len(),
            world.members,
            world.natives.len()
        );
        for (class, n) in per_class {
            text.push_str(&format!("  natives {n:>4}  {class}\n"));
        }
        Ok(text)
    })();
    match result {
        Ok(text) => {
            print!("{text}");
            feline_runtime::init::exit_now(0)
        }
        Err(e) => {
            eprintln!("feline-runtime: {e}");
            feline_runtime::init::exit_now(1)
        }
    }
}
