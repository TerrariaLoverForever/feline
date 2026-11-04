# Roadmap

Feline's long-term goal is a launcher **and its own, independently developed
Android compatibility runtime** for Roblox on Linux.

**Roblox compatibility of Feline's own runtime has not been demonstrated.** It
will only be claimed once it has been, with reproducible evidence.

| Milestone | Goal | Status |
|---|---|---|
| **M1: Launcher** | GUI with Feline branding, system info, diagnostics, runtime detection, Play button and game-link input, settings, Discord status, basic monitoring. Can start external runtimes the user chooses. | **Done** |
| **M2: Runtime research** | A written survey, no runtime code: which Android components the Roblox client needs; license choice; architecture. See [runtime/M2-RESEARCH.md](runtime/M2-RESEARCH.md). | Done (component license checks happen per component, when one is chosen) |
| **M3: Runtime prototype** | Load the engine library and resolve its imports, without graphics. Success means "loaded and imports resolved", not "a game runs". | In progress (step 1 of 5: reading and checking the engine) |
| **M4: Runtime bring-up** | Windowing, Vulkan/GLES surfaces, input, audio. | Planned |
| **M5: Compatibility** | Sign-in and joining an experience. | Not started |

## M1 stages

| Stage | Content | Status |
|---|---|---|
| 0 | Workspace, docs, icons, desktop entry | Done |
| 1 | System information (`feline --info`) | Done |
| 2 | Runtime layer: `RuntimeBackend`, external Cordial adapter (`--runtimes`, `--doctor N`) | Done |
| 3 | Game links and launching (own process group, log file, exit status); saved runtime choice | Done |
| 4 | App shell: window, navigation, dark theme, branding, Home page, first-run account notice, About page | Done |
| 5 | One-click Play, Join by link, Runtimes page with selection | Done |
| 6 | System page (copyable report), Diagnostics page (quick checks, runtime self-check and info, full bug report, logs) | Done |
| 7 | Sober and Stud backends (detect and launch), shared detection helpers | Done |
| 8 | Settings page: Feline overrides (renderer, GameMode, close on leave), a tab per runtime (Sober, Cordial, Stud) editing its own file, "Managed by Feline" locks | Done |
| 9 | Feline's own Discord Rich Presence (from the Roblox logs; username and Join button off by default; turns off the runtimes' own) | Done |
| 10 | ~~Install and update Stud from Feline~~ | Dropped: users install Stud themselves, like Sober and Cordial |
| 11 | Basic monitoring: time played, CPU and memory (PSS) of the whole game process tree, on the Home page | Done |
| 12 | Polish: page and card animations, button feedback (event-driven only, nothing animates while idle), About credits, README with screenshots | Done |

Out of scope for M1: registering as the `roblox://` link handler, a FastFlag
editor, a global FPS limit (use Roblox's in-game Maximum Frame Rate; Cordial's
page offers its own), Feline self-updates, packaging, benchmarks.
