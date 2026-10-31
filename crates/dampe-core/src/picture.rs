//! Scales any feed to one fixed brightness picture, so every threshold after this stays the same
//! whether the feed is 1080p, 720p or 480p.
//!
//! OBS Virtual Camera centers a 4:3 game on a wider canvas with black on both sides, and capture
//! setups are rarely cropped perfectly. So the side bars are measured, and only the middle of the
//! game is kept. Dampe is never near the edges.

/// The whole 4:3 game at the working scale.
pub const GAME_WIDTH: usize = 720;
pub const GAME_HEIGHT: usize = 540;
/// What's cut from each side of the game: bad crops, the top HUD row and the RNG watch's corner.
pub const MARGIN_X: usize = 72;
pub const MARGIN_Y: usize = 54;

pub const WIDTH: usize = GAME_WIDTH - 2 * MARGIN_X;
pub const HEIGHT: usize = GAME_HEIGHT - 2 * MARGIN_Y;

/// Frames measured before the bars count as known.
const BAR_FRAMES: usize = 30;
/// A column is game, not bar, when this many of its pixels are this bright. Video black is 16.
const BAR_BRIGHT: u8 = 40;
const BAR_MIN_SHARE: usize = 100;

/// A brightness frame from a capture device or a decoded file.
#[derive(Clone, Copy)]
pub struct Luma<'a> {
    pub data: &'a [u8],
    pub width: usize,
    pub height: usize,
    pub stride: usize,
}

impl Luma<'_> {
    pub(crate) fn row(&self, y: usize) -> &[u8] {
        &self.data[y * self.stride..y * self.stride + self.width]
    }
}

/// For each output sample, the source samples it averages.
fn spans(source_start: usize, source_len: usize, out_len: usize, skip: usize, keep: usize) -> Vec<(u32, u32)> {
    (skip..skip + keep)
        .map(|i| {
            let from = source_start + i * source_len / out_len;
            let to = source_start + ((i + 1) * source_len).div_ceil(out_len);
            (from as u32, to.max(from + 1) as u32)
        })
        .collect()
}

/// Where the game is in a feed's frames, in the feed's pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GameArea {
    pub left: usize,
    pub top: usize,
    pub width: usize,
    pub height: usize,
}

struct Scale {
    columns: Vec<(u32, u32)>,
    rows: Vec<(u32, u32)>,
    area: GameArea,
}

/// Turns device frames into [`WIDTH`] by [`HEIGHT`] pictures.
#[derive(Default)]
pub struct Normalizer {
    /// How many frames each column was bright in.
    bright: Vec<u32>,
    seen: usize,
    size: (usize, usize),
    scale: Option<Scale>,
}

impl Normalizer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Where the game is in the feed's frames, once that's known.
    pub fn game_area(&self) -> Option<GameArea> {
        self.scale.as_ref().map(|scale| scale.area)
    }

    /// Writes `frame` to `out`. `false` while the bars are still being measured, and `out` isn't
    /// written.
    pub fn process(&mut self, frame: Luma<'_>, out: &mut [u8]) -> bool {
        debug_assert_eq!(out.len(), WIDTH * HEIGHT);
        if self.size != (frame.width, frame.height) {
            *self = Self { size: (frame.width, frame.height), bright: vec![0; frame.width], ..Self::default() };
        }
        let Some(scale) = self.scale_for(frame) else { return false };

        let mut sums = vec![0u32; WIDTH];
        for (row_out, &(from, to)) in out.as_chunks_mut::<WIDTH>().0.iter_mut().zip(&scale.rows) {
            sums.fill(0);
            for y in from..to {
                let row = frame.row(y as usize);
                for (sum, &(left, right)) in sums.iter_mut().zip(&scale.columns) {
                    let mut value = 0u32;
                    for &pixel in &row[left as usize..right as usize] {
                        value += u32::from(pixel);
                    }
                    *sum += value;
                }
            }
            let rows = to - from;
            for ((pixel, sum), &(left, right)) in row_out.iter_mut().zip(&sums).zip(&scale.columns) {
                *pixel = (sum / (rows * (right - left))) as u8;
            }
        }
        true
    }

    fn scale_for(&mut self, frame: Luma<'_>) -> Option<&Scale> {
        if self.scale.is_none() {
            self.measure(frame);
            self.seen += 1;
            if self.seen < BAR_FRAMES {
                return None;
            }
            self.scale = Some(self.build_scale(frame.width, frame.height));
        }
        self.scale.as_ref()
    }

    fn measure(&mut self, frame: Luma<'_>) {
        let mut counts = vec![0u32; frame.width];
        for y in (0..frame.height).step_by(4) {
            for (count, &pixel) in counts.iter_mut().zip(frame.row(y)) {
                *count += u32::from(pixel > BAR_BRIGHT);
            }
        }
        let needed = (frame.height / 4 / BAR_MIN_SHARE).max(1) as u32;
        for (bright, &count) in self.bright.iter_mut().zip(&counts) {
            *bright += u32::from(count >= needed);
        }
    }

    fn build_scale(&self, width: usize, height: usize) -> Scale {
        let left = self.bright.iter().position(|&b| b > 0).unwrap_or(0);
        let right = self.bright.iter().rposition(|&b| b > 0).map_or(width, |i| i + 1);
        let mut game_width = right.saturating_sub(left).max(1);
        // Dark scenes can hide the game's edges, so the bars only count when they fit a 4:3 game.
        // A wide measurement is a stretched or uncropped feed: use it whole.
        let expected = height * 4 / 3;
        let (game_left, game_height);
        if game_width.abs_diff(expected) <= expected / 8 || width <= expected {
            game_left = left;
            game_height = (game_width * 3 / 4).min(height);
        } else {
            // Not a 4:3 shape: assume a centered 4:3 game if there's room for one, else the frame
            game_width = expected.min(width);
            game_left = (width - game_width) / 2;
            game_height = height;
        }
        let game_top = (height - game_height) / 2;
        Scale {
            columns: spans(game_left, game_width, GAME_WIDTH, MARGIN_X, WIDTH),
            rows: spans(game_top, game_height, GAME_HEIGHT, MARGIN_Y, HEIGHT),
            area: GameArea { left: game_left, top: game_top, width: game_width, height: game_height },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame with a game area of value 100 and one bright marker at game position (x, y).
    fn framed(width: usize, height: usize, game_left: usize, game_width: usize, mark: (usize, usize)) -> Vec<u8> {
        let mut data = vec![16u8; width * height];
        for y in 0..height {
            data[y * width + game_left..y * width + game_left + game_width].fill(100);
        }
        let game_height = game_width * 3 / 4;
        let top = (height - game_height) / 2;
        let scale = game_width as f64 / GAME_WIDTH as f64;
        let (mx, my) = (game_left + (mark.0 as f64 * scale) as usize, top + (mark.1 as f64 * scale) as usize);
        let size = (scale * 8.0) as usize;
        for y in my..my + size {
            data[y * width + mx..y * width + mx + size].fill(255);
        }
        data
    }

    fn run(width: usize, height: usize, game_left: usize, game_width: usize) -> Vec<u8> {
        let data = framed(width, height, game_left, game_width, (360, 270));
        let mut normalizer = Normalizer::new();
        let mut out = vec![0; WIDTH * HEIGHT];
        let frame = Luma { data: &data, width, height, stride: width };
        let mut done = false;
        for _ in 0..BAR_FRAMES {
            done = normalizer.process(frame, &mut out);
        }
        assert!(done);
        out
    }

    /// The same game spot lands on the same output pixel, whatever the feed's size and bars.
    #[test]
    fn game_position_does_not_depend_on_the_feed() {
        let center = (270 - MARGIN_Y + 3) * WIDTH + (360 - MARGIN_X + 3);
        for (width, height, left, game) in [(1920, 1080, 240, 1440), (1440, 1080, 0, 1440), (1280, 720, 160, 960)] {
            let out = run(width, height, left, game);
            assert!(out[center] > 200, "{width}x{height}: {}", out[center]);
            assert_eq!(out[0], 100, "{width}x{height}: the corner is game, not bar");
        }
    }

    #[test]
    fn nothing_until_the_bars_are_measured() {
        let data = vec![100u8; 640 * 480];
        let mut normalizer = Normalizer::new();
        let mut out = vec![0; WIDTH * HEIGHT];
        let frame = Luma { data: &data, width: 640, height: 480, stride: 640 };
        assert!(!normalizer.process(frame, &mut out));
    }
}
