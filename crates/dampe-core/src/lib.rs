//! Reads Dampe's blinks from video to recover Ocarina of Time's RNG state, then finds when a dig
//! gives the Heart Piece. Video goes in and a time to press A comes out. It never touches input.
//!
//! The pipeline, one module each:
//! - [`textbox`] tells when the Yes/No box is up, which is the only time anything counts.
//! - [`picture`] scales any feed to one fixed picture, without the black bars or the edges.
//! - [`detector`] turns those pictures into flutters: when each blink burst starts and its count.
//! - [`solver`] finds the RNG states that explain a run of flutters.
//! - [`tracker`] decides when to trust a state, and when to give up on it.
//! - [`session`] runs all of it for one read, and says when to press.
//! - [`rng`] is the game's model: `Rand_ZeroOne`, the blink timers and the dig roll.

pub mod detector;
pub mod eye;
pub mod navi;
pub mod picture;
pub mod rng;
pub mod session;
pub mod solver;
pub mod textbox;
pub mod tracker;
