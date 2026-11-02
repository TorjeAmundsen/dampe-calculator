//! Recovers Ocarina of Time's RNG state from Dampe's blinks, then finds when a dig gives the Heart
//! Piece. Blinks pressed by hand go in, or video with the `video` feature, and a time to press A comes
//! out. It never touches input.
//!
//! In every build:
//! - [`rng`] is the game's model: `Rand_ZeroOne`, the blink timers and the dig roll.
//! - [`manual`] finds the state from flutters pressed by hand.
//! - [`reading`] is what a read has to say: its readings, and when a dig gives the Heart Piece.
//!
//! With the `video` feature, which is everything about seeing flutters instead:
//! - `textbox` tells when the Yes/No box is up, which is the only time anything counts.
//! - `picture` scales any feed to one fixed picture, without the black bars or the edges.
//! - `detector`, `eye` and `navi` turn those pictures into flutters: when each blink burst starts and
//!   its count.
//! - `solver` finds the RNG states that explain a run of flutters.
//! - `tracker` decides when to trust a state, and when to give up on it.
//! - `session` runs all of it for one read, and says when to press.

#[cfg(feature = "video")]
pub mod detector;
#[cfg(feature = "video")]
pub mod eye;
pub mod manual;
#[cfg(feature = "video")]
pub mod navi;
#[cfg(feature = "video")]
pub mod picture;
pub mod reading;
pub mod rng;
#[cfg(feature = "video")]
pub mod session;
#[cfg(feature = "video")]
pub mod solver;
#[cfg(feature = "video")]
pub mod textbox;
#[cfg(feature = "video")]
pub mod tracker;
