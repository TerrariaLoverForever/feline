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

## Clean-room rule for runtime work

Feline's runtime is developed **independently**.

- **Do not copy, port, translate or adapt code from Cordial, Sober, Stud** or
  any other Roblox runtime, even where its license would allow it.
- **Do not consult their source code** while working on Feline's runtime.
  Public, high-level documentation (READMEs, user docs) may be read, but
  implementations must be your own.
- Pull requests containing code derived from those projects are rejected.
- Third-party code from other projects may only be added after its license
  has been verified file by file and recorded in [docs/LEGAL.md](docs/LEGAL.md).

**Disclosure:** during early planning, Cordial's README and three of its
documentation files (`docs/architecture.md`, `docs/nvidia.md`,
`docs/runtime-spec.md`) were read. No Cordial source code was read during
planning (later, limited reads for the launcher are listed below), and no
Cordial code is in Feline. The optional Cordial adapter
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

## Before sending a change

```sh
cargo fmt --all
cargo clippy --workspace --all-targets   # no warnings
cargo test --workspace
```

New source files start with:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
```
