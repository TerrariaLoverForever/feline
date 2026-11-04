# Feline

A polished launcher for playing Roblox on Linux, with its own runtime under
development.

> **Status: early development (Milestone 1, the launcher).** Not ready for
> general use. Feline's own runtime doesn't exist yet, and its Roblox
> compatibility has not been demonstrated. See the [roadmap](docs/ROADMAP.md).

Until Feline's runtime is ready, Feline can optionally start an **external
third-party runtime** that you install yourself (currently supported:
[Sober](https://sober.vinegarhq.org), [Cordial](https://github.com/luohoa97/cordial)
and [Stud](https://github.com/CatPieLeaf/Stud)). Feline never picks one for
you, contains none of their code, and ships no Roblox files.

**Unofficial project, not affiliated with Roblox Corporation.** Roblox does not
support third-party clients; read [docs/LEGAL.md](docs/LEGAL.md) for the
account risks before using it.

## Building

Requires Rust 1.92 or newer (a distribution-packaged toolchain is fine; rustup
is not needed).

```sh
cargo build --release
./target/release/feline --help
```

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [Roadmap](docs/ROADMAP.md)
- [Legal notes and account risk](docs/LEGAL.md)
- [Contributing](CONTRIBUTING.md) (DCO sign-off, clean-room rule)
- [Name and logo policy](TRADEMARKS.md)

## License

Copyright (C) 2026 The Feline contributors

Feline is free software: you can redistribute it and/or modify it under the
terms of the GNU General Public License as published by the Free Software
Foundation, either version 3 of the License, or (at your option) any later
version.

Feline is distributed in the hope that it will be useful, but WITHOUT ANY
WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A
PARTICULAR PURPOSE. See the GNU General Public License for more details.

You should have received a copy of the GNU General Public License along with
Feline (see [COPYING](COPYING)). If not, see <https://www.gnu.org/licenses/>.

The Feline name and logo are not covered by the GPL; see
[TRADEMARKS.md](TRADEMARKS.md).
