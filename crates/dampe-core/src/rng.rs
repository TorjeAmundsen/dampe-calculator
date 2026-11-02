//! The game's RNG and everything that draws from it while the Yes/No box is open.
//!
//! Decomp references: `qrand.c` (the generator), `z_en_tk.c` `EnTk_UpdateEyes` (Dampe's blinks),
//! `z_actor.c` `FaceChange_UpdateBlinking` called from `z_player.c` (Link's), and `z_en_tk.c`
//! `EnTk_DigEff` and `EnTk_ChooseReward` (the dig). N64 and Wii VC run the same code.

/// N64 `Rand_ZeroOne` only uses the low 23 bits, and those only depend on the low 23 bits.
pub const STATES: u32 = 1 << 23;
const MASK: u32 = STATES - 1;

/// Dampe's idle animation repeats after this many game frames: `gDampeRestAnim`'s `frameCount`.
#[cfg(feature = "video")]
pub const IDLE_LOOP_FRAMES: usize = 49;

/// Draws on the dig's first dust frame before the roll: 3 for the position, 1 for the acceleration.
const DUST_DRAWS: usize = 4;
/// Heart Piece when the roll is at least this (`EnTk_ChooseReward`).
const HEART_PIECE_LUCK: f64 = 0.9;
/// The dig's first dust frame comes this many game frames after the text box disappears, measured
/// on Wii VC. The animation reaches frame 32 with the morph, so it's data, not timing.
pub const BOX_GONE_TO_ROLL: i64 = 33;

#[inline]
pub fn next(state: u32) -> u32 {
    state.wrapping_mul(1664525).wrapping_add(1013904223) & MASK
}

#[inline]
fn unit(state: u32) -> f32 {
    state as f32 / STATES as f32
}

/// `Rand_S16Offset`: `(s16)(Rand_ZeroOne() * range) + base`.
#[inline]
fn s16_offset(state: u32, base: i32, range: i32) -> i32 {
    (unit(state) * range as f32) as i32 + base
}

/// Blinks in a flutter, from the second of Dampe's two draws.
#[inline]
fn blink_count(state: u32) -> u8 {
    if unit(state) > 0.5 { 4 } else { 3 }
}

/// Game frames a flutter takes, from its first half-closed eye to the eye staying open: half,
/// closed and open for each blink.
#[inline]
pub const fn flutter_len(count: u8) -> i64 {
    3 * count as i64 - 1
}

/// Game frames Dampe waits between flutters: at least this, plus one of this many values.
const WAIT_BASE: i32 = 30;
const WAIT_VALUES: i32 = 30;
/// The same for Link's blink timer (`FaceChange_UpdateBlinking(20, 80, 6)`).
const LINK_WAIT_BASE: i32 = 20;
const LINK_WAIT_VALUES: i32 = 80;

/// Whether a dig rolled from `state` gives the Heart Piece, `state` being the RNG on the frame the
/// dig starts, before the dust.
pub fn rolls_heart_piece(state: u32) -> bool {
    let mut state = state;
    for _ in 0..=DUST_DRAWS {
        state = next(state);
    }
    f64::from(unit(state)) >= HEART_PIECE_LUCK
}

/// One step of the blink model. Game frames count from the renewal before the first flutter, and
/// Link's update runs before Dampe's within a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Blinks {
    pub state: u32,
    /// The frame of the next Dampe renewal: when the flutter in progress ends.
    pub renewal: i64,
    /// The frame Link's blink timer next draws.
    pub link_next: i64,
}

/// A flutter the model drew.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelFlutter {
    pub start: i64,
    pub count: u8,
}

impl ModelFlutter {
    /// The game frame it ends on.
    #[inline]
    pub const fn end(&self) -> i64 {
        self.start + flutter_len(self.count)
    }
}

impl Blinks {
    /// `state` just before a Dampe renewal at frame 0, with Link's next draw at `link_next`.
    pub fn new(state: u32, link_next: i64) -> Self {
        Self { state, renewal: 0, link_next }
    }

    /// Link's draw, and the frame of his next one.
    #[inline]
    fn link_draw(&mut self) {
        self.state = next(self.state);
        self.link_next += i64::from(s16_offset(self.state, LINK_WAIT_BASE, LINK_WAIT_VALUES));
    }

    /// Runs up to and including the next Dampe renewal, and returns the flutter it starts.
    #[inline]
    pub fn next_flutter(&mut self) -> ModelFlutter {
        while self.link_next <= self.renewal {
            self.link_draw();
        }
        self.state = next(self.state);
        let wait = i64::from(s16_offset(self.state, WAIT_BASE, WAIT_VALUES));
        self.state = next(self.state);
        let count = blink_count(self.state);
        let start = self.renewal + wait;
        self.renewal = start + flutter_len(count);
        ModelFlutter { start, count }
    }
}

/// The RNG state after each frame from a known point. Frames with no draw repeat the last one.
pub struct StateWalk {
    blinks: Blinks,
}

impl StateWalk {
    pub fn new(blinks: Blinks) -> Self {
        Self { blinks }
    }

    /// The state when `frame` starts, and whether Link's draw comes on that frame, advancing as
    /// far as needed. A dig on that frame rolls after Link's draw and before Dampe's.
    ///
    /// Frames must not go backwards.
    pub fn state_for_roll(&mut self, frame: i64) -> u32 {
        loop {
            let link = self.blinks.link_next;
            let dampe = self.blinks.renewal;
            if link <= frame && link <= dampe {
                self.blinks.link_draw();
            } else if dampe < frame {
                self.blinks.next_flutter();
            } else {
                return self.blinks.state;
            }
        }
    }

    /// The state after everything on `frame` ran. Frames must not go backwards.
    #[cfg(feature = "video")]
    pub fn state_after(&mut self, frame: i64) -> u32 {
        loop {
            let link = self.blinks.link_next;
            let dampe = self.blinks.renewal;
            if link <= frame && link <= dampe {
                self.blinks.link_draw();
            } else if dampe <= frame {
                self.blinks.next_flutter();
            } else {
                return self.blinks.state;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_bits_follow_the_full_generator() {
        // Two consecutive values from the Wii VC RNG watch, read off the recording
        let full = 0x35a3_4623u32;
        let stepped = full.wrapping_mul(1664525).wrapping_add(1013904223);
        assert_eq!(next(full & MASK), stepped & MASK);
    }

    #[test]
    fn counts_and_waits_stay_in_the_games_ranges() {
        let mut blinks = Blinks::new(12345, 50);
        let mut last_end = 0;
        for _ in 0..1000 {
            let flutter = blinks.next_flutter();
            assert!((30..=59).contains(&(flutter.start - last_end)));
            assert!(flutter.count == 3 || flutter.count == 4);
            last_end = flutter.end();
        }
    }

    #[test]
    fn heart_piece_rolls_about_one_time_in_ten() {
        let hits = (0..STATES).step_by(97).filter(|&s| rolls_heart_piece(s)).count();
        let total = (0..STATES).step_by(97).count();
        let share = hits as f64 / total as f64;
        assert!((0.09..0.11).contains(&share), "{share}");
    }
}
