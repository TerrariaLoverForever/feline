# Architecture

## Overview

```
┌──────────────────────┐   Feline runtime protocol   ┌───────────────────────────┐
│ feline (launcher)    │ ──────────────────────────► │ feline-runtime (FUTURE)   │
│ GUI + text commands  │   separate process          │ own Android compatibility │
└──────────┬───────────┘                             │ layer, independently      │
           │ uses                                    │ developed                 │
┌──────────▼───────────┐                             └───────────────────────────┘
│ feline-core          │                             ┌───────────────────────────┐
│ system info          │ ── optional, opt-in ──────► │ external runtimes         │
│ RuntimeBackend trait │    public CLI only          │ (e.g. Cordial): never     │
│ process handling     │                             │ required, never default   │
└──────────────────────┘                             └───────────────────────────┘
```

All Feline code is licensed GPL-3.0-or-later.

## Rules

1. **Feline will have its own runtime, developed independently.** It is not a
   fork, port, translation or adaptation of Cordial, Sober or any other Roblox
   runtime. See [CONTRIBUTING.md](../CONTRIBUTING.md) for the clean-room rule.
2. **The launcher and the runtime are separate programs.** They will talk over
   a protocol Feline defines (versioned, with no dependency on any other
   project's protocol). It is designed in the runtime-research milestone, not
   before. See [ROADMAP.md](ROADMAP.md).
3. **External runtimes are optional adapters.** They are used only through
   their public interfaces, are never required, are never selected
   automatically, and are always labelled "external third-party runtime".
4. **`RuntimeBackend` is written in Feline's own terms** (detect, version,
   doctor, diagnostics, and later launch), so `feline-runtime` will be just
   another backend.
5. **The launcher is never in the game's critical path.** A runtime runs in its
   own process group, so closing or crashing the launcher doesn't end the game.
6. **The GUI never blocks.** Slow work runs on a background thread.
7. **Small dependency set.** Third-party crates: `slint` (GUI) only, with its
   default features off. Every dependency is license-checked first (see [LEGAL.md](LEGAL.md)).

## `feline-core` modules

| Module | Purpose |
|---|---|
| `system` | Host info from `/proc`, `/sys`, `/etc/os-release`, `pci.ids`; quick checks (`SystemInfo::checks`) for the Diagnostics page |
| `link` | Checks pasted game links; converts `roblox.com/games/<id>` to a `roblox://` link |
| `process` | Runs helper commands with captured output, an output cap and a timeout |
| `runtime` | `RuntimeBackend` trait, `Origin` (Feline vs external), `Installation`, `detect_all` |
| `settings` | `~/.config/feline/settings.conf`: chosen runtime, accepted account-notice version |
| `runtime::cordial` | Optional adapter for the external Cordial runtime: finds Flatpak / `PATH` / AppImage installs, uses `--help`, `--doctor`, `--diagnostics` |

Detection never starts processes; only `Installation::version()` (when the
version isn't in a file), `doctor()` and `diagnostics()` do, always with a timeout.

## Toolchain

Feline builds with a distribution-packaged Rust toolchain; rustup is not
required. The minimum supported version is `rust-version` in `Cargo.toml`.
