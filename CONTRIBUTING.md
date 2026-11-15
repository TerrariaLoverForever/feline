# Contributing to Feline

Thanks for your interest! Please read this before sending changes.

## License of contributions

Feline is licensed under **GPL-3.0-or-later**. By contributing, you agree that
your contribution is licensed under the same terms (inbound = outbound).
There is no CLA: you keep your copyright.

## Developer Certificate of Origin (DCO)

Every commit must be signed off, certifying the
[Developer Certificate of Origin 1.1](https://developercertificate.org/):
that you wrote the change, or otherwise have the right to submit it under
Feline's license. Add the sign-off with:

```sh
git commit -s
```

which appends a line like:

```
Signed-off-by: Your Name <you@example.com>
```

Commits without a sign-off can't be merged.

## Rules for runtime work

Feline's runtime is the project's **own code**.

- **Do not copy, port, translate or adapt code from another Roblox runtime**,
  even where its license would allow it. Implementations must be your own.
- **Cordial, Sober and Stud: do not consult their source code** while working
  on Feline's runtime. Their public, high-level documentation (READMEs, user
  docs) may be read.
- **Permissively licensed runtimes (today: Mocktail and Nightcap, both
  Apache-2.0) may be read as a reference** for facts about how Roblox's app
  expects to be driven: key and button numbers, which of Roblox's functions
  to call and in what order, setting names. When you do:
  - write the code yourself;
  - say so in a comment at that place, naming the project, its license and
    the file you followed;
  - make sure the project is listed under "Runtime component policy" in
    [docs/LEGAL.md](docs/LEGAL.md).
- Pull requests containing code copied or adapted from another runtime are
  rejected.
- Third-party code from other projects may only be added after its license
  has been verified file by file and recorded in [docs/LEGAL.md](docs/LEGAL.md).

**Disclosure:** during early planning, Cordial's README and three of its
documentation files (`docs/architecture.md`, `docs/nvidia.md`,
`docs/runtime-spec.md`) were read. No Cordial source code was read during
planning (later, limited reads for the launcher are listed below), and no
Cordial code is in Feline. Mocktail's and Nightcap's sources were read as a
reference for the runtime's input, text, voice, message-bus, desktop-layout
and frame-rate work; the files that follow them are listed in
[docs/LEGAL.md](docs/LEGAL.md). An earlier version of this file forbade
consulting any runtime's source; the rule above replaced it so that it
matches how the runtime was actually written. The optional Cordial adapter
(`crates/feline-core/src/runtime/cordial.rs`) only starts Cordial through its
public command line.

For the launcher's Stud adapter and settings work (stage 7 onward, not runtime
work), Stud's README, release metadata and packaging files
(`packaging/stud.desktop.in`, `packaging/flatpak/io.github.catpieleaf.Stud.yml`,
`packaging/aur/PKGBUILD.bin`, `CMakeLists.txt`) were read, plus AI-made summaries
of two source files: `stud-config/src/settings.cpp` (the keys and types of
Stud's `config.json`, so Feline can edit that file without breaking it) and
`render-host/src/discord_rpc.cpp` (which confirmed that Discord presence needs an
application ID), plus a summary of `stud-config/include/stud/settings.h` (the
defaults of those keys). No Stud code is in Feline.

For the Cordial settings tab, Cordial's `crates/cordial-protocol/src/settings.rs`
(its MIT/Apache protocol crate, meant for other programs) and an AI-made summary
of `crates/cordial-shell/src/shell_config.rs` (only the accepted values of a few
`shell.json` keys) were read. No Cordial code is in Feline. Sober's public configuration
documentation was read; Sober's code is not available.

### Working with Roblox's app

The runtime has to talk to Roblox's engine, so it reads what the engine
**declares**, never how Roblox's code **works**:

- **Native library (`libroblox.so`):** its import and export tables (symbol
  names), and Feline's own records of what the engine calls at run time.
  Disassembling or decompiling the engine is not part of Feline's development.
- **Java part (`classes*.dex` in the APK):** only the declarations of
  `native` methods, which are the Java side of the engine's `Java_*` exports:
  class name, method name, type signature and access flags
  (`crates/feline-runtime/src/dex.rs`, `feline-runtime --natives`). Method
  bodies are never read, decompiled or reproduced. Where the engine needs an
  answer from Java code, Feline writes its own answer from what the engine
  asks for and from public Android documentation.
- **The engine's own log** (`appData/logs/` in the runtime's data folder) may
  be read: it's what the program reports about itself.
- **Assets** in the APK are unpacked for the engine at run time on the user's
  machine; they're never committed or redistributed.

#### Exceptions

One piece of Feline came from looking at the engine's machine code, against
the rule above. It was kept by the maintainer's decision, and is written
down here so nobody has to find it by surprise:

- **`crates/feline-runtime/src/alloc.rs`:** the engine crashed when it freed
  buffers that came from the system's allocator. The fix calls the engine's
  own internal `malloc`, which the engine doesn't export, so its address
  (for Roblox 2.742.0.1059) and its first bytes were read from a disassembly
  of that function and of the place that calls it. Nothing else was read,
  and none of Roblox's code is reproduced beyond the 20 bytes Feline
  compares before calling. On any other engine version the check fails and
  Feline doesn't call it.
- The finding that the engine reads `osVersion` as an Android API level
  (`"33"`) came from the same work.

This is not a precedent. New work follows the rule; an exception needs the
maintainer's decision and an entry in this list.

## Before sending a change

```sh
cargo clippy --workspace --all-targets   # no warnings
cargo test --workspace
```

Don't run `cargo fmt` on the whole project: Feline's code isn't in rustfmt's
default style (its lines run to about 120 columns), so that would rewrite
most files. Write new code the way the code around it looks.

New source files start with:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
```
