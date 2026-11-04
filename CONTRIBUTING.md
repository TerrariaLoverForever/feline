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

- **Do not copy, port, translate or adapt code from Cordial, Sober** or any
  other Roblox runtime, even where its license would allow it.
- **Do not consult their source code** while working on Feline's runtime.
  Public, high-level documentation (READMEs, user docs) may be read, but
  implementations must be your own.
- Pull requests containing code derived from those projects are rejected.
- Third-party code from other projects may only be added after its license
  has been verified file by file and recorded in [docs/LEGAL.md](docs/LEGAL.md).

**Disclosure:** during early planning, Cordial's README and three of its
documentation files (`docs/architecture.md`, `docs/nvidia.md`,
`docs/runtime-spec.md`) were read. None of Cordial's source code was read, and
no Cordial code is in Feline. The optional Cordial adapter
(`crates/feline-core/src/runtime/cordial.rs`) only starts Cordial through its
public command line.

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
