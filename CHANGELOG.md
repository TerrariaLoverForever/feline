# Changelog

What changed in each Feline release. The section for a version is also its
release notes on GitHub (see `.github/workflows/release.yml`).

## 0.1.1

### Fixed

- **Roblox's new chat.** Chat now has the emoji button, sends big emoji
  (shown above your head without a speech bubble) and has the Friends tab,
  like Roblox's own app. Feline's runtime now starts Roblox's app the way
  Android does, so Roblox's feature rollouts reach it.
- **Roblox's built-in files follow Roblox updates.** The in-game UI (chat
  included), fonts, shaders and scripts were unpacked once and never again,
  so they stayed at the first Roblox version Feline installed. Now changed
  files are refreshed whenever Roblox updates; files Roblox didn't change
  (and mods over them) are left alone.
- **Roblox's settings are current.** Feline's runtime downloads Roblox's
  settings ("FastFlags") each time it starts instead of reusing the copy
  from setup, so features Roblox turns on reach Feline.
- **Memory.** Converting textures no longer keeps hundreds of MB of memory
  for the whole session (over 500 MB in one Fisch session); memory not used
  for a few seconds goes back to the system.
- **Hitches when effects play.** Large textures are converted on several
  CPU cores, without holding up the game's other graphics work.
- **Typing emoji and other languages.** While you type, emoji, Cyrillic,
  Greek, Arabic, Thai, Hindi and other scripts show as themselves instead of
  Roblox's logo. Chinese, Japanese and Korean need a font for them installed
  on your system (e.g. Noto Sans CJK).
- **Play buttons on the Roblox website.** On Linux the website opens
  `robloxmobile://` links, which Feline didn't handle, so another launcher
  could take them. With "Open Roblox links with Feline" on, Feline now
  handles `roblox://`, `roblox-player:` and `robloxmobile://` links, and takes
  them back when it starts if another launcher claimed them.

### Added

- **ARM64 Linux builds** of the launcher and login window
  (`feline-linux-aarch64.tar.gz`). Feline's own runtime runs Roblox's x86-64
  engine, so on ARM PCs you play through another runtime chosen in Feline.
  Updates pick the right build for your PC.

## 0.1.0

The first release.
