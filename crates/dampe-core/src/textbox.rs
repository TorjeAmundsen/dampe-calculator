//! Tells when the Yes/No box is up, by its colors: red for the price, green for the choices and blue
//! for the arrow, all at once. On Wii VC recordings those three only appear together in that box, in
//! one unbroken run from when it opens to when it closes. Navi is long gone by then, so nothing from
//! before it is worth keeping, and the end of the run is the frame the dig was chosen on.

use crate::picture::{GameArea, Luma};

/// Where the box is in the game's picture, as fractions of it.
const TOP: f32 = 0.60;
const BOTTOM: f32 = 0.88;
const LEFT: f32 = 0.08;
const RIGHT: f32 = 0.92;
/// Sampling every this many pixels of a 1080p feed is enough, and the same share of a smaller one.
const SAMPLES_ACROSS: usize = 300;
const SAMPLES_DOWN: usize = 75;
/// The share of samples each color needs, out of 22500. Set low so other text lengths still pass:
/// each color alone can show up elsewhere, but not all three.
const MIN_SAMPLES: [u32; 3] = [20, 30, 20];
/// Game frames in a row that decide it, so one odd frame changes nothing.
pub const FRAMES_TO_OPEN: u32 = 3;
pub const FRAMES_TO_CLOSE: u32 = 2;

/// What changed on a game frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Opened,
    Closed,
}

#[inline]
fn rgb(y: u8, u: u8, v: u8) -> (i32, i32, i32) {
    let (y, u, v) = (298 * (i32::from(y) - 16), i32::from(u) - 128, i32::from(v) - 128);
    ((y + 459 * v) >> 8, (y - 55 * u - 136 * v) >> 8, (y + 541 * u) >> 8)
}

/// How many samples of the box's area have each color: blue, green, red.
fn count_colors(frame: Luma<'_>, chroma: &[u8], area: GameArea) -> [u32; 3] {
    let mut counts = [0; 3];
    let chroma_stride = frame.width.next_multiple_of(2);
    for row in 0..SAMPLES_DOWN {
        let along = TOP + (BOTTOM - TOP) * row as f32 / SAMPLES_DOWN as f32;
        let y = (area.top + (area.height as f32 * along) as usize).min(frame.height - 1);
        let luma = frame.row(y);
        let chroma_row = y / 2 * chroma_stride;
        for column in 0..SAMPLES_ACROSS {
            let along = LEFT + (RIGHT - LEFT) * column as f32 / SAMPLES_ACROSS as f32;
            let x = (area.left + (area.width as f32 * along) as usize).min(frame.width - 1);
            let i = chroma_row + x / 2 * 2;
            let (Some(&u), Some(&v)) = (chroma.get(i), chroma.get(i + 1)) else { continue };
            let (r, g, b) = rgb(luma[x], u, v);
            counts[0] += u32::from(b > 150 && r < 110 && b - g > 60);
            counts[1] += u32::from(g > 150 && r < 130 && b < 130);
            counts[2] += u32::from(r > 150 && g < 90 && b < 90);
        }
    }
    counts
}

/// Follows whether the box is up.
#[derive(Default)]
pub struct TextboxWatch {
    open: bool,
    /// Game frames in a row disagreeing with `open`.
    against: u32,
}

impl TextboxWatch {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The next game frame. `chroma` is U and V, alternating, for every 2 by 2 pixels.
    pub fn observe(&mut self, frame: Luma<'_>, chroma: &[u8], area: GameArea) -> Option<Change> {
        let counts = count_colors(frame, chroma, area);
        let present = counts.iter().zip(&MIN_SAMPLES).all(|(count, min)| count >= min);
        if present == self.open {
            self.against = 0;
            return None;
        }
        self.against += 1;
        if self.against < if self.open { FRAMES_TO_CLOSE } else { FRAMES_TO_OPEN } {
            return None;
        }
        self.against = 0;
        self.open = present;
        Some(if present { Change::Opened } else { Change::Closed })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDTH: usize = 640;
    const HEIGHT: usize = 480;

    fn area() -> GameArea {
        GameArea { left: 0, top: 0, width: WIDTH, height: HEIGHT }
    }

    /// A gray frame with patches of pure color in the box's area, each big enough to count.
    fn frame(colors: &[(u8, u8, u8)]) -> (Vec<u8>, Vec<u8>) {
        let mut luma = vec![60u8; WIDTH * HEIGHT];
        let mut chroma = vec![128u8; WIDTH * HEIGHT / 2];
        for (i, &(y, u, v)) in colors.iter().enumerate() {
            let left = 80 + i * 150;
            for row in 320..360 {
                luma[row * WIDTH + left..row * WIDTH + left + 100].fill(y);
                for x in (left..left + 100).step_by(2) {
                    chroma[row / 2 * WIDTH + x] = u;
                    chroma[row / 2 * WIDTH + x + 1] = v;
                }
            }
        }
        (luma, chroma)
    }

    const BLUE: (u8, u8, u8) = (60, 220, 110);
    const GREEN: (u8, u8, u8) = (150, 70, 60);
    const RED: (u8, u8, u8) = (70, 110, 220);

    fn step(watch: &mut TextboxWatch, colors: &[(u8, u8, u8)]) -> Option<Change> {
        let (luma, chroma) = frame(colors);
        watch.observe(Luma { data: &luma, width: WIDTH, height: HEIGHT, stride: WIDTH }, &chroma, area())
    }

    #[test]
    fn opens_on_all_three_colors_only() {
        for colors in [vec![BLUE, GREEN], vec![BLUE, RED], vec![GREEN, RED], vec![]] {
            let mut watch = TextboxWatch::default();
            for _ in 0..10 {
                assert_eq!(step(&mut watch, &colors), None, "{colors:?}");
            }
        }
        let mut watch = TextboxWatch::default();
        let all = [BLUE, GREEN, RED];
        assert_eq!(step(&mut watch, &all), None);
        assert_eq!(step(&mut watch, &all), None);
        assert_eq!(step(&mut watch, &all), Some(Change::Opened));
        assert!(watch.is_open());
    }

    #[test]
    fn one_odd_frame_does_not_close_it() {
        let mut watch = TextboxWatch::default();
        let all = [BLUE, GREEN, RED];
        for _ in 0..FRAMES_TO_OPEN {
            step(&mut watch, &all);
        }
        assert_eq!(step(&mut watch, &[]), None);
        assert_eq!(step(&mut watch, &all), None);
        assert_eq!(step(&mut watch, &[]), None);
        assert_eq!(step(&mut watch, &[]), Some(Change::Closed));
    }
}
