# Product

<!-- impeccable:product-schema 1 -->

## Platform

desktop

Native desktop app (Rust + Slint), not a website. Runs on Linux and Windows today; the goal is every OS where each feature is possible. A feature that can't work on an OS is hidden there (or explained), never faked.

## Users

Mostly casual Roblox players. Feline is just a Roblox launcher, not a product made for kids. They open Feline to get into a game quickly, maybe switch on a mod or a fun extra, and then forget about it while they play. Most of them won't know what an FFlag, a runtime or a process tree is, and shouldn't need to.

Power users (FFlags, runtimes, diagnostics) are welcome, but they are not who the defaults are designed for.

## Product Purpose

Feline is a launcher for Roblox. It gets players into games, keeps their settings and mods in one place, and shows what they're playing on Discord.

- On Linux it starts an external third-party runtime that the player installs themselves (Sober, Cordial, Stud, Tipsy, Mocktail, Nightcap). Feline's own runtime is in development.
- On Windows it bootstraps the official Roblox client.

It works when a player can press Play and be in a game without reading anything. Everything else is optional.

## Positioning

- **Cross-platform:** one launcher on Linux and Windows, with more OSes as a goal.
- **Honest and safe:**
  - It never injects anything into the game client and only writes settings values it has verified.
  - It backs up files before changing them and gives control back when a feature is turned off.
  - It is upfront about account risk, and never claims compatibility it hasn't demonstrated.
- **Cat personality:** cat-themed wording, a desk cat, seasonal touches and achievements. It's a launcher with charm.
- **Light on resources:** nothing animates while idle, so Feline uses no GPU time while a game runs.

## Operating Context

- Players open Feline right before playing and often leave it open, minimised or closed while the game runs. Its window competes with a running game for CPU, GPU and RAM; the dev machine has 8 GB.
- Main flows:
  - one-click Play, or joining from a pasted game link;
  - runtime choice (Linux);
  - Settings: Feline-wide overrides, the runtime's own settings, Roblox's own settings and Privacy;
  - Mods and the Feline Theme;
  - System and Diagnostics pages, including a copyable bug report;
  - About: Make it yours, Desk cat and Achievements.
- Discord Rich Presence runs in its own small process next to the game.

## Capabilities and Constraints

- Unofficial, not affiliated with Roblox Corporation. Roblox does not support third-party clients (see docs/LEGAL.md).
- Ships no Roblox files and no runtime code. It never picks a runtime for the player.
- Mods are Bloxstrap-style folders, applied at launch with backups. Code files are skipped. FFlags are allowlisted only.
- The desk cat is a separate always-on-top window and is never drawn into the game.
- Idle must stay idle: animations are event-driven only.
- Status: "still a kitten". Milestone 1 (the launcher) is done; the runtime is in M4 bring-up.

## Brand Commitments

- Name: **Feline**. Tagline: "Always lands on its feet."
- The name and logo (`icon/`, `assets/icons/`) are excluded from the GPL; all rights reserved (TRADEMARKS.md).
- Voice: warm, playful and cat-themed, but always plain and truthful. Cat wording never hides what a button actually does.
- Humour: Feline isn't written for kids, so edgy humour can be fine. Judge each case on its own. Keep it out of places where clarity matters (errors, warnings, account-risk notes).
- All art is drawn from scratch for Feline (icons, desk-cat sprites, the Feline Theme). The project rejected a theme that brought back old Roblox artwork.

## Evidence on Hand

- Screenshots in `docs/screenshots/` (home, settings, about).
- Roadmap and research docs in `docs/`.
- There are no user counts, testimonials, press or benchmarks. Don't invent any.
- Roblox compatibility of Feline's own runtime has not been demonstrated. Don't claim it.

## Product Principles

1. **Play first.** Getting into a game is one click. Every other feature stays out of the way until someone looks for it.
2. **Never lie, never break things.** Use verified values only, keep backups, be honest about limits and risk, and don't fake features an OS can't support.
3. **Charm without cost.** The personality never spends the player's CPU or GPU while they play.
4. **Plain words for casual players.** Explain things the way you would to a friend. Keep jargon behind "advanced" doors.
5. **Every option, everywhere it can work.** Be feature-rich across OSes, but keep it organised so it never overwhelms.
