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

--check  reads the engine library and reports what loading it would involve.
--load   also puts it into memory: copies its segments, applies the
         relocations that don't need imports and sets memory permissions.
         Then it frees the memory and exits.

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
        [flag, path] if flag == "--load" => {
            let data = match std::fs::read(path) {
                Ok(data) => data,
                Err(e) => {
                    eprintln!("feline-runtime: couldn't read {path}: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let result = feline_runtime::elf::Elf::parse(&data)
                .map_err(feline_runtime::load::LoadError::from)
                .and_then(|elf| feline_runtime::load::load(&elf));
            match result {
                Ok((image, report)) => {
                    print!("{}", report.to_text(&image));
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
