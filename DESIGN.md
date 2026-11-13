---
name: Feline
description: A cat-shaped Roblox launcher. Always lands on its feet.
colors:
  feline-violet: "#8f7ef7"
  feline-violet-strong: "#7a67f2"
  feline-violet-light: "#9988fa"
  feline-violet-hover: "#a596ff"
  feline-violet-soft: "#8f7ef71f"
  lavender: "#cdc5ff"
  violet-den: "#231f3d"
  violet-den-deeper: "#2a2148"
  on-accent: "#ffffff"
  den-floor: "#111218"
  den-shadow: "#0d0e13"
  slate-surface: "#171820"
  slate-raised: "#1d1f29"
  slate-hover: "#242633"
  slate-border: "#252735"
  slate-border-strong: "#343749"
  inset-well: "#0b0c10"
  hero-mid: "#1a1a28"
  hero-end: "#15161e"
  wash: "#ffffff08"
  wash-hover: "#ffffff12"
  track: "#ffffff0c"
  scrim: "#05030acc"
  knob: "#ffffff"
  moon-text: "#ebebf3"
  dusk-text: "#a5a7ba"
  faint-text: "#6d7086"
  purr-green: "#5fcf98"
  amber-warning: "#e9b860"
  hiss-red: "#ef6f80"
  sky-info: "#67b4f0"
typography:
  display:
    fontFamily: "system-ui, Segoe UI, Noto Sans, sans-serif"
    fontSize: "28px"
    fontWeight: 800
  headline:
    fontFamily: "system-ui, Segoe UI, Noto Sans, sans-serif"
    fontSize: "16px"
    fontWeight: 700
  title:
    fontFamily: "system-ui, Segoe UI, Noto Sans, sans-serif"
    fontSize: "14px"
    fontWeight: 600
  body:
    fontFamily: "system-ui, Segoe UI, Noto Sans, sans-serif"
    fontSize: "13.5px"
    fontWeight: 400
  label:
    fontFamily: "system-ui, Segoe UI, Noto Sans, sans-serif"
    fontSize: "11px"
    fontWeight: 700
    letterSpacing: "1.1px"
  mono:
    fontFamily: "monospace"
    fontSize: "12px"
    fontWeight: 400
rounded:
  sm: "10px"
  md: "14px"
  lg: "20px"
  pill: "999px"
spacing:
  xxs: "2px"
  xs: "4px"
  sm: "8px"
  md: "12px"
  lg: "16px"
  xl: "20px"
  page: "32px"
components:
  button-play:
    backgroundColor: "{colors.feline-violet}"
    textColor: "{colors.on-accent}"
    typography: "{typography.headline}"
    rounded: "{rounded.pill}"
    padding: "0 30px 0 26px"
    height: "52px"
  button-play-hover:
    backgroundColor: "{colors.feline-violet-hover}"
  button-secondary:
    backgroundColor: "{colors.slate-raised}"
    textColor: "{colors.moon-text}"
    rounded: "{rounded.sm}"
    padding: "0 15px 0 14px"
    height: "36px"
  button-secondary-hover:
    backgroundColor: "{colors.feline-violet-soft}"
  button-primary:
    backgroundColor: "{colors.feline-violet}"
    textColor: "{colors.on-accent}"
    rounded: "{rounded.sm}"
    padding: "0 15px 0 14px"
    height: "36px"
  button-primary-hover:
    backgroundColor: "{colors.feline-violet-light}"
  button-danger-hover:
    textColor: "{colors.hiss-red}"
  card:
    backgroundColor: "{colors.slate-surface}"
    rounded: "{rounded.md}"
    padding: "20px"
  nav-item:
    textColor: "{colors.dusk-text}"
    rounded: "{rounded.sm}"
    height: "40px"
    padding: "0 8px 0 12px"
  nav-item-active:
    backgroundColor: "{colors.feline-violet-soft}"
    textColor: "{colors.moon-text}"
  tab-bar:
    backgroundColor: "{colors.slate-surface}"
    rounded: "{rounded.md}"
    height: "40px"
  toggle-on:
    backgroundColor: "{colors.feline-violet}"
    rounded: "{rounded.pill}"
    width: "42px"
    height: "24px"
  pill:
    backgroundColor: "{colors.feline-violet-soft}"
    textColor: "{colors.feline-violet}"
    rounded: "{rounded.pill}"
    height: "22px"
---

# Design System: Feline

## Overview

**Creative North Star: "The Night-Lit Den"**

Feline looks like a quiet, dim room where a cat has curled up for the evening. The walls are calm, cool slate, a few shades apart, so the eye rests easily in a dark room. The only warmth is a soft violet light that falls on whatever matters right now: the Play button, the page you're on, a switch that's on. Everything else stays in shadow.

The den is still until you touch it. Things react to hover and press with a small, springy give, and pages and cards settle in as they appear. Once you stop interacting, nothing moves, glows or loops, because a running game needs the GPU more than the launcher does. Personality lives in the details instead: paw-print tips, a cat face on empty lists, a logo that tilts its head when you hover over it, seasonal sprinkles, and an accent the player can recolour.

The design must not look like Roblox's own interface (no Roblox layouts, artwork or icons). It must not look like a stock Fluent or Material settings app either: standard widgets are allowed, but they are dressed in Feline's tokens and shapes, never left looking like the default.

**Key Characteristics:**
- Dark slate tonal layers, separated by small steps in lightness and thin borders.
- One accent colour, the player's choice (default Feline Violet), used sparingly and only where it means something.
- Rounded, soft shapes: 10–20px corners, pill-shaped main actions and toggles.
- Event-driven motion only: springy reactions to hover and press, a settle-in when things appear, and stillness at rest.
- Cat touches (paws, the cat face, the desk cat) as quiet decoration, never as chrome that gets in the way.

## Colors

A cool, near-neutral slate night with a single soft violet light. Status colours are kept separate from the accent.

### Primary
- **Feline Violet** (feline-violet): the one accent, taken from the Feline icon. Marks the active nav item, toggles that are on, the Play button, section-icon tiles and focus. The player can change it (About → Make it yours, and seasonal themes). The whole accent family is then recomputed from that one colour, so components must use the accent tokens, never hard-coded violet.
- **Feline Violet Strong / Light / Hover** (feline-violet-strong, -light, -hover): pressed, gradient and hover steps of the accent. The Play button's gradient runs from Light to Strong.
- **Feline Violet Soft** (feline-violet-soft): the accent at about 12% opacity. Backgrounds for selected nav items, section icons and pills.
- **Lavender** (lavender): accent-coloured icons and small text on surfaces. A pale tint in the dark den; in the light den it is darkened step by step until it reaches 4.5:1 on white, for any accent.
- **Accent Ink** (accent-ink, Theme only): accent-coloured text that must stay readable, such as pills and the active nav icon. The accent itself in the dark den, Lavender in the light one.
- **Violet Den / Deeper** (violet-den, violet-den-deeper): very dark surfaces with a hint of the accent, for hero areas that need a little colour without getting bright.

### Neutral
- **Den Floor** (den-floor): the window background.
- **Den Shadow** (den-shadow): the sidebar, a step darker than the floor.
- **Slate Surface / Raised / Hover** (slate-surface, slate-raised, slate-hover): cards, then raised controls inside cards, then hover states. Each step is about 3–4% lighter.
- **Slate Border / Strong** (slate-border, slate-border-strong): 1px borders on cards and controls. Use the strong one for the selected tab and for hovered borders.
- **Inset Well** (inset-well): below the floor, for command output boxes.
- **Hero Mid / End** (hero-mid, hero-end): where a hero panel's accent gradient fades back into the den (Home hero, Mods heroes).
- **Wash / Wash Hover / Track** (wash, wash-hover, track): faint white washes for chips, thumbnails and bar tracks sitting on a surface.
- **Scrim** (scrim): dims the window behind a dialog.
- **Moon Text** (moon-text): primary text.
- **Dusk Text** (dusk-text): secondary and help text.
- **Faint Text** (faint-text): captions and disabled items.

### The Light Den
The same den with the lights on (About → Make it yours → Light or dark: Dark, Light, or match the system). Every neutral is a Theme token set from Rust (`look.rs`, `Neutrals::DARK` / `Neutrals::LIGHT`), so no page knows which den it is in.
- Floor #f3f3f7, sidebar #e9e9f0, surfaces paper-white #ffffff stepping down to #f7f7fa and #eeeef4, borders #e1e1ea / #cbcbd8.
- Ink-dark text #1c1b26, secondary #52526a, captions #727289 (at least 7:1, 4.5:1 and 3:1 on white; a unit test keeps it so).
- Status colours deepen to read on white: green #1d8a56, amber #9a6200, red #c9354b, blue #1f72b8.
- Washes and tracks become ink at 4–8% instead of white; shadows become ink at 11% (lift) and 20% (pop), so the den stays lifted without going grey.
- The accent family keeps its hue. Its tinted surfaces (violet-den, violet-den-deeper) become pale washes of the accent, and accent ink darkens (see Lavender).
- Slint's standard widgets switch between their light and dark Fluent schemes along with Feline (`Palette.color-scheme`, set in app.slint).

### Status
- **Purr Green** (purr-green): OK and running. **Amber** (amber-warning): needs attention. **Hiss Red** (hiss-red): problems and destructive actions. **Sky** (sky-info): neutral information. Show them as dots, pills and values, never as large fills.

### Named Rules
**The One Lamp Rule.** Only the accent glows, and only on what matters right now: the main action, the current place, an "on" state. If two things on a screen compete for the violet, one of them is wrong.

**The Recolourable Rule.** Never hard-code accent shades. Every violet in the UI comes from the Theme accent tokens, because players and seasons change them.

## Typography

**Body Font:** the system UI sans (Segoe UI on Windows, the desktop's sans on Linux), with no bundled font.
**Mono Font:** the system monospace, used only for command output and reports.

**Character:** one plain, native family. Hierarchy comes from weight rather than size. Headings are heavy (700–800), labels semi-bold (600) and body text regular, so the type feels sturdy and friendly at the small sizes a desktop window uses.

### Hierarchy
- **Display** (800, 28px): page titles only.
- **Headline** (700, 16px): card titles and the Play button label. Section titles inside cards use 700 at 15px.
- **Title** (600, 14px): setting labels, nav items (500 at rest, 600 when active) and the window default size.
- **Body** (400, 13.5px): help text and descriptions, in Dusk Text, with word-wrap. Buttons and tabs use 600 at 13px.
- **Group heading** (700, 13px, sentence case, Dusk Text): names a group of rows ("Recently played", "Quick checks", the sidebar's "Help"), with a little more room above it than below. There are no ALL-CAPS labels. Pills use 600 at 11px.

### Named Rules
**The Weight-Not-Size Rule.** Make hierarchy with weight and colour (Moon → Dusk → Faint), not with new sizes. The scale stays at 11 / 12 / 13.5 / 14 / 16 / 28.

## Layout

- A fixed 232px sidebar (Den Shadow) sits on the left, and the current page fills the rest.
- The window opens at 1160×760 and can shrink to 960×660. Pages scroll vertically and never scroll sideways.
- Pages are padded 30–32px and stack: a header (title, description, actions on the right), then cards.
- Cards use 20px of inner padding and a 12px gap between their children.
- Setting rows put the label and help on the left (stretching) and the control on the right (natural size), 24px apart, with 10px of vertical padding.
- The spacing rhythm is 2 / 4 / 8 / 12 / 16 / 20 / 32. 10px and 14px appear as fine-tuning inside components.
- Information is dense but calm: one idea per card, grouped by tabs (TabBar) rather than by long scrolling pages.

**The Pinned Row Rule.** Every row made from a list (`for … in …`) that can hold wrapped text sits in a Rectangle whose `height` is bound to its content's `preferred-height`. Without it, Slint sizes the card as if each wrapped paragraph were one line, and the rows spill out of the bottom of the card (the old Crash detective bug). Inside such a row, don't wrap the row's parts in `if`: an `if` around a part that itself has an `if` (like `SettingRow`) is measured as one line again, and the rows overlap in a narrow window (the old runtime-settings bug). Give hidden parts height 0 instead.

## Elevation & Depth

Lifted tonal layers. Surfaces step up from Den Shadow (sidebar) through Den Floor, Slate Surface, Slate Raised and Slate Hover, with 1px borders, and every panel that sits on the floor casts a soft, dark, offset shadow. Shadows are never coloured. Coloured glow is reserved for the main action (Play), the logo and live status dots. All shadow values are tokens in `theme.slint` (`lift-*`, `nudge-*`, `pop-*`), so pages never write raw shadow numbers.

### Shadow Vocabulary
- **Accent glow, Play** (blur 16px at rest and 26px on hover, offset-y 4px, accent at 38%): the Play button only.
- **Logo glow** (blur 0.32× size, 0.5× on hover, accent at 45%): the Logo with `glow`.
- **Status glow** (blur 7px, tone at 60%): StatusDot.
- **Lift** (`lift-blur` 18px, `lift-offset` 6px, `lift-color` #00000059): everything resting on the floor. Card, StatTile, the Home hero, the launch-status panel, Most played, Mods heroes and ModCards.
- **Lift, hover** (`lift-hover-blur` 24px, `lift-hover-offset` 8px): clickable rows rise while pointed at (GameRow, runtime rows, server rows), animated over 240ms with ease-out-quint.
- **Nudge** (`nudge-blur` 6px, `nudge-offset` 2px, lift colour): small things resting on a surface. The selected tab's highlight, the toggle knob (half offset) and the chosen accent swatch (doubled).
- **Pop** (`pop-blur` 28px, `pop-offset` 10px, `pop-color` #00000073): dialogs, above everything (the account notice).
- Things *inside* a card (rows, tips, previews) stay flat: one level of lift per stack, never a shadow inside a shadow.

### Named Rules
**The One Shadow Deep Rule.** A lifted panel's contents are flat. If something inside a card seems to need its own shadow, it should be a separate card or a tonal step instead.

**The Still Den Rule.** No idle animation, ever. Motion, glow pulses and shadow changes happen only in response to input or a state change (a game starting). The only repeating animation is the "starting" pulse of a status dot, and it stops when the game is up.

## Shapes

- Soft, rounded rectangles everywhere: 10px for controls (buttons, nav items, banners), 14px for cards and tiles, 20px for large hero surfaces.
- The main actions are fully rounded pills: the Play button (52px tall, 26px radius), toggles and status pills.
- Icon tiles are small rounded squares: 30px with a 9px radius for section icons, 36px with an 11px radius in stat tiles.
- Borders are always 1px. Icons are line icons on a 24-unit grid, drawn for Feline.

## Components

### Buttons
Soft and responsive: quiet at rest, with a springy give when touched.
- **Play:** a pill, 52px tall and at least 196px wide, with a Light → Strong accent gradient at 100°, white 700 text and a play icon. On hover it lifts to scale 1.025 with a brighter gradient and a wider glow; on press it drops to 0.96. It uses ease-out-back easing.
- **Secondary:** 36px tall, 10px radius, Slate Raised with a 1px border. On hover it gets an accent tint at 14% and an accent border at 45%; on press it scales to 0.96. It takes an optional 16px icon in Lavender.
- **Primary:** the same shape filled with accent and white text. Use it for the main action of a section.
- **Danger:** tinted with Hiss Red. Actions that can't be undone use **ConfirmButton**: the first click arms it ("Sure? Click again" for 3 seconds), and the second click acts.
- **Disabled:** 60% opacity, no hover, no pointer cursor.
- **Keyboard focus:** every Feline control (Play, buttons, switches, nav items, tab bars) takes focus with Tab, not with a click, and shows a 2px accent ring 3px outside its shape (`FocusRing`). Space or Enter activates it; tab bars also move with the arrow keys.

### Pills
- 22px tall pills with the tone at 13% behind 600 11px text in the tone colour. Used for "Soon", "Set by Feline", "Third-party" and similar.

### Cards / Containers
- **Card:** Slate Surface, 14px radius, 1px Slate Border and 20px padding. It fades and settles in (scale 0.985 → 1) when it appears.
- **StatTile:** a lifted card-styled tile with a tinted icon square, a Faint label and a bold value. It has no hover state, because it can't be clicked.
- **Banner / PawTip:** message boxes with the tone at 7–8% and a border at 25–30%. Banner leads with a status dot; PawTip leads with a paw, for friendly hints.
- **CatEmpty:** a Lavender cat face with a title and text, for empty lists.

### Inputs / Fields
- Standard widgets (LineEdit, ComboBox, ScrollView, TextEdit) come from Slint's Fluent style, switched to light or dark with Feline. Their corners (about 4px), their own blue focus underline and their drop-down menu can't be restyled from Feline's code (Slint limitation). Keep them few, and place them in setting rows so the Feline controls around them set the tone. Replacing them with Feline-drawn versions is open work.
- **Toggle:** a 42×24 pill. It is accent when on; when off it has a 1px strong border on Slate Hover, with a Dusk knob in the dark den and a white one in the light den. The 18px knob slides with an ease-out-back spring and rests on a nudge shadow.

### Navigation
- **Sidebar NavItem:** 38px tall with a 10px radius and an icon plus a 14px label. At rest it shows a Faint icon and Dusk text. On hover the item nudges 2px right and the text brightens; pressing darkens it a step. When active it gets the accent-soft background, an accent-ink icon and 600 Moon text. A "Soon" pill marks pages that aren't built yet. Groups are separated by a sentence-case group heading ("Help").
- **TabBar:** equal-width tabs inside a 40px Slate Surface track. The selected tab has a raised highlight (Slate Hover in the dark den, the floor colour in the light den) with a strong border and a nudge shadow, which slides between tabs (ease-out-quint). It also serves as a segmented control (Light or dark).

### Logo
- The Feline icon, rounded at 24% of its size, with an optional accent glow. With `playful`, it tilts −6° and grows 4% on hover. The name and logo are reserved (TRADEMARKS.md): never redraw or recolour them.
- The glow is for a page's own logo (the Home hero, About, the account notice). The small sidebar logo doesn't glow, so it never competes with the page's main action.

### Dialogs
- The account notice: Slate Surface, 20px radius, a 1px accent border at 35% and the pop shadow over a scrim. It settles in with a slight ease-out-back scale. The heading carries the message ("Before you pounce") with no label above it, and the acknowledgement ("I understand") is the primary button.

### Game rows (Home)
- A lifted 68px row: game icon, name, playtime, a Servers chip (wash), a drawn star (Amber and filled when it's a favourite, outlined otherwise) and a round play button. The play button stays neutral (Slate Hover, Dusk icon) until you point at the row, then turns accent. One Lamp: a list of games mustn't become a column of violet circles.

## Do's and Don'ts

### Do:
- **Do** take every accent shade from the Theme tokens (accent, accent-strong, accent-light, accent-hover, accent-soft, lavender, accent-deep). The player and the seasons recolour them.
- **Do** separate surfaces by tonal step plus a 1px border, and lift panels with the `lift-*` tokens (dialogs with `pop-*`). Take every colour from Theme: no hex values in page files.
- **Do** animate only in response to input or state, with Theme durations (130 / 240 / 360ms) and ease-out-back for springy controls.
- **Do** make hierarchy with weight and the Moon / Dusk / Faint text colours.
- **Do** check every screen in both dens. A colour that only works in one of them isn't a Theme colour yet.
- **Do** keep cat touches small and quiet: paws, the cat face, the desk cat and seasonal sprinkles.

### Don't:
- **Don't** imitate Roblox's interface, layouts, artwork or icons.
- **Don't** let a screen look like a stock Fluent or Material settings app. Dress standard widgets in Feline tokens.
- **Don't** add idle or looping animation, or decoration that redraws while a game runs.
- **Don't** use coloured shadows or glow anywhere except the accent's main action, the logo and live status.
- **Don't** use status colours as large fills, or use the accent to mean "warning" or "error".
- **Don't** use text glyphs (★, ✓) as icons. Draw them as paths in `icons.slint`.
- **Don't** put a shadow inside a lifted panel.
- **Don't** write ALL-CAPS labels or tracked-out eyebrows above headings.
- **Don't** show buttons that can never work in the current setup (e.g. a runtime self-check for Roblox's official client). Hide them instead.
