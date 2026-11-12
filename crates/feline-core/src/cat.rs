// SPDX-License-Identifier: GPL-3.0-or-later
//! Feline's desk cat: a little pixel cat that follows your mouse pointer
//! around the screen, sits down next to it, washes, and falls asleep when
//! the pointer rests (in the spirit of the classic "neko" desktop toys).
//!
//! This module is the cat itself: its frames (drawn from scratch for Feline,
//! `assets/cat/sprites.txt`), its coats, and how it moves. The window it
//! lives in is the GUI's job (a separate always-on-top, click-through
//! window; the cat is never drawn into any game).

use std::sync::OnceLock;

use crate::png::Rgba;

/// The frames' source, 16×16 pixels each.
const SPRITES: &str = include_str!("../../../assets/cat/sprites.txt");
/// Width and height of a frame, in pixels (before scaling).
pub const FRAME: u32 = 16;

/// What the cat is doing right now (one frame each).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pose {
    Sit,
    Wash1,
    Wash2,
    Alert,
    Sleep1,
    Sleep2,
    Right1,
    Right2,
    Left1,
    Left2,
    Down1,
    Down2,
    Up1,
    Up2,
}

impl Pose {
    /// The frame's name in the sprite file, and whether it's mirrored.
    fn source(self) -> (&'static str, bool) {
        match self {
            Self::Sit => ("sit", false),
            Self::Wash1 => ("wash1", false),
            Self::Wash2 => ("wash2", false),
            Self::Alert => ("alert", false),
            Self::Sleep1 => ("sleep1", false),
            Self::Sleep2 => ("sleep2", false),
            Self::Right1 => ("right1", false),
            Self::Right2 => ("right2", false),
            Self::Left1 => ("right1", true),
            Self::Left2 => ("right2", true),
            Self::Down1 => ("down1", false),
            Self::Down2 => ("down2", false),
            Self::Up1 => ("up1", false),
            Self::Up2 => ("up2", false),
        }
    }
}

/// The cat's fur.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Coat {
    #[default]
    White,
    Ginger,
    Grey,
    Black,
}

impl Coat {
    pub const ALL: [Coat; 4] = [Self::White, Self::Ginger, Self::Grey, Self::Black];

    pub fn parse(value: &str) -> Self {
        match value {
            "ginger" => Self::Ginger,
            "grey" | "gray" => Self::Grey,
            "black" => Self::Black,
            _ => Self::White,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::White => "white",
            Self::Ginger => "ginger",
            Self::Grey => "grey",
            Self::Black => "black",
        }
    }

    /// Outline, coat, coat shade, pink, eyes (0xRRGGBB).
    fn colours(self) -> [u32; 5] {
        match self {
            Self::White => [0x1e1c24, 0xf5f0e8, 0xd6cec4, 0xf096aa, 0x3caa5a],
            Self::Ginger => [0x2a1a12, 0xf0a35a, 0xc9763a, 0xf5a3b0, 0x3caa5a],
            Self::Grey => [0x1c1d24, 0xa9adb8, 0x7f8390, 0xf096aa, 0xe6c04a],
            Self::Black => [0x0b0b0f, 0x3a3a45, 0x272730, 0xd97a8e, 0xe6c04a],
        }
    }
}

/// The parsed frames: name → 16 rows of 16 palette letters.
fn frames() -> &'static Vec<(String, Vec<Vec<u8>>)> {
    static FRAMES: OnceLock<Vec<(String, Vec<Vec<u8>>)>> = OnceLock::new();
    FRAMES.get_or_init(|| parse(SPRITES))
}

fn parse(text: &str) -> Vec<(String, Vec<Vec<u8>>)> {
    let mut list: Vec<(String, Vec<Vec<u8>>)> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        if let Some(name) = line.strip_prefix("= ") {
            list.push((name.trim().to_owned(), Vec::new()));
        } else if !line.is_empty()
            && !line.starts_with('#')
            && let Some((_, rows)) = list.last_mut()
        {
            rows.push(line.bytes().collect());
        }
    }
    list
}

/// Draws `pose` in `coat`, with the collar in `collar` (0xRRGGBB), each
/// pixel as a `scale`×`scale` block. Clear pixels are fully transparent,
/// the rest fully opaque.
pub fn render(pose: Pose, coat: Coat, collar: u32, scale: u32) -> Rgba {
    let scale = scale.clamp(1, 8);
    let size = FRAME * scale;
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    let (name, mirrored) = pose.source();
    let [outline, fur, shade, pink, eye] = coat.colours();
    if let Some((_, rows)) = frames().iter().find(|(n, _)| n == name) {
        for (y, row) in rows.iter().enumerate().take(FRAME as usize) {
            for (x, &letter) in row.iter().enumerate().take(FRAME as usize) {
                let colour = match letter {
                    b'o' => outline,
                    b'w' => fur,
                    b's' => shade,
                    b'p' => pink,
                    b'e' => eye,
                    b'a' => collar,
                    _ => continue,
                };
                let x = if mirrored { FRAME as usize - 1 - x } else { x };
                let rgba = [(colour >> 16) as u8, (colour >> 8) as u8, colour as u8, 255];
                for dy in 0..scale {
                    let py = y as u32 * scale + dy;
                    for dx in 0..scale {
                        let px = x as u32 * scale + dx;
                        let at = ((py * size + px) * 4) as usize;
                        pixels[at..at + 4].copy_from_slice(&rgba);
                    }
                }
            }
        }
    }
    Rgba {
        width: size,
        height: size,
        pixels,
    }
}

/// How often [`Cat::step`] should be called.
pub const TICK: std::time::Duration = std::time::Duration::from_millis(100);

/// The cat's position and mood. Call [`Cat::step`] every [`TICK`].
#[derive(Debug, Clone)]
pub struct Cat {
    /// The cat's centre, in screen pixels.
    pub x: f32,
    pub y: f32,
    /// Ticks spent resting near the pointer.
    idle: u32,
    /// Ticks left of looking up before running off.
    alert: u32,
    tick: u32,
}

/// Ticks of sitting before the cat washes, and when it falls asleep.
const WASH_AFTER: u32 = 25;
const SIT_AGAIN_AFTER: u32 = 55;
const SLEEP_AFTER: u32 = 75;
/// A cat that rested this long looks up first when the pointer leaves.
const WAKES_AFTER: u32 = 20;

impl Cat {
    pub fn new(x: f32, y: f32) -> Self {
        Self {
            x,
            y,
            idle: 0,
            alert: 0,
            tick: 0,
        }
    }

    /// One step toward the pointer at `pointer`, for a cat `size` pixels
    /// big. Returns the frame to show.
    pub fn step(&mut self, pointer: (f32, f32), size: f32) -> Pose {
        self.tick = self.tick.wrapping_add(1);
        let dx = pointer.0 - self.x;
        let dy = pointer.1 - self.y;
        let distance = (dx * dx + dy * dy).sqrt();
        // Close enough: sit next to the pointer, not on it.
        let near = size * 0.9;
        if distance <= near {
            self.idle = self.idle.saturating_add(1);
            self.alert = 0;
            return match self.idle {
                i if i < WASH_AFTER => Pose::Sit,
                i if i < SIT_AGAIN_AFTER => {
                    if (self.tick / 3).is_multiple_of(2) {
                        Pose::Wash1
                    } else {
                        Pose::Wash2
                    }
                }
                i if i < SLEEP_AFTER => Pose::Sit,
                _ => {
                    if (self.tick / 8).is_multiple_of(2) {
                        Pose::Sleep1
                    } else {
                        Pose::Sleep2
                    }
                }
            };
        }
        if self.idle >= WAKES_AFTER {
            self.alert = 5;
        }
        self.idle = 0;
        if self.alert > 0 {
            self.alert -= 1;
            return Pose::Alert;
        }
        let speed = (size * 0.35).max(4.0);
        let travel = speed.min(distance - near * 0.5);
        self.x += dx / distance * travel;
        self.y += dy / distance * travel;
        let even = self.tick.is_multiple_of(2);
        if dy.abs() > dx.abs() * 1.5 {
            match (dy < 0.0, even) {
                (true, true) => Pose::Up1,
                (true, false) => Pose::Up2,
                (false, true) => Pose::Down1,
                (false, false) => Pose::Down2,
            }
        } else {
            match (dx < 0.0, even) {
                (true, true) => Pose::Left1,
                (true, false) => Pose::Left2,
                (false, true) => Pose::Right1,
                (false, false) => Pose::Right2,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_frame_is_16_by_16_with_known_letters() {
        let list = parse(SPRITES);
        assert_eq!(list.len(), 12);
        for (name, rows) in &list {
            assert_eq!(rows.len(), 16, "{name}");
            for row in rows {
                assert_eq!(row.len(), 16, "{name}");
                assert!(row.iter().all(|c| b".oswpea".contains(c)), "{name}");
            }
        }
        for pose in [Pose::Sit, Pose::Left1, Pose::Up2, Pose::Sleep2, Pose::Alert] {
            let (name, _) = pose.source();
            assert!(list.iter().any(|(n, _)| n == name), "{name}");
        }
    }

    #[test]
    fn render_scales_and_mirrors() {
        let right = render(Pose::Right1, Coat::White, 0x8f7ef7, 2);
        let left = render(Pose::Left1, Coat::White, 0x8f7ef7, 2);
        assert_eq!((right.width, right.height), (32, 32));
        let at = |img: &Rgba, x: u32, y: u32| {
            let i = ((y * img.width + x) * 4) as usize;
            img.pixels[i..i + 4].to_vec()
        };
        for y in 0..32 {
            for x in 0..32 {
                assert_eq!(at(&right, x, y), at(&left, 31 - x, y));
            }
        }
        // Some pixels are drawn, some are clear.
        assert!(right.pixels.chunks(4).any(|p| p[3] == 255));
        assert!(right.pixels.chunks(4).any(|p| p[3] == 0));
    }

    #[test]
    fn the_cat_runs_to_the_pointer_then_rests_and_sleeps() {
        let mut cat = Cat::new(0.0, 0.0);
        let pose = cat.step((500.0, 0.0), 32.0);
        assert!(matches!(pose, Pose::Right1 | Pose::Right2));
        assert!(cat.x > 0.0);
        for _ in 0..200 {
            cat.step((500.0, 0.0), 32.0);
        }
        assert!((cat.x - 500.0).abs() <= 32.0);
        assert!(matches!(cat.step((500.0, 0.0), 32.0), Pose::Sleep1 | Pose::Sleep2));
        // The pointer leaves: it looks up first, then runs (left, up).
        assert_eq!(cat.step((0.0, 0.0), 32.0), Pose::Alert);
        for _ in 0..4 {
            cat.step((0.0, 0.0), 32.0);
        }
        assert!(matches!(cat.step((0.0, 0.0), 32.0), Pose::Left1 | Pose::Left2));
        let mut cat = Cat::new(0.0, 500.0);
        assert!(matches!(cat.step((0.0, 0.0), 32.0), Pose::Up1 | Pose::Up2));
    }

    #[test]
    fn coats_round_trip() {
        for coat in Coat::ALL {
            assert_eq!(Coat::parse(coat.as_str()), coat);
        }
        assert_eq!(Coat::parse("purple"), Coat::White);
    }
}
