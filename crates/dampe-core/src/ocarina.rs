//! Reaching a Heart Piece sooner by freezing Dampe with the Ocarina.
//!
//! Dampe draws in pairs, in one frame, so the state between a pair is never held and a dig can't
//! roll from it. Link draws one at a time, which changes which states get held. While the Ocarina is
//! out `Actor_UpdateAll` only updates actors with `ACTOR_FLAG_UPDATE_DURING_OCARINA`. Link has it and
//! Dampe doesn't, so Link's blinks go on, and Dampe's timer waits. Taking it out at the right moment
//! puts Link's single draw ahead of Dampe's next pair.
//!
//! Not checked against the RNG watch. The plan assumes nothing else draws between the two talks: see
//! the README.

use crate::reading::{Candidate, Window};
use crate::rng::{BOX_GONE_TO_ROLL, Blinks, FREEZE_EXTRA_SPREAD, StateWalk, rolls_heart_piece};
#[cfg(test)]
use crate::rng::FREEZE_EXTRA_FRAMES;

/// Game frames from pressing No to the Ocarina being out, with a reasonable mash. From a user: 8 is
/// frame perfect, and 15 is consistent.
const TO_OCARINA_FRAMES: i64 = 15;
/// How much sooner than that it can come out, so how much longer Dampe can be frozen.
const OCARINA_EARLY_FRAMES: i64 = 15 - 8;
/// Game frames from starting to put it away to being able to say Yes. Measured at 21 to 23 on fast
/// cycles and about 40 on slower ones, so this reaches sooner windows at some risk: raise it if real
/// use misses windows. Only the total matters, since Dampe and Link draw the same with the box open or not.
const LEAVE_TO_BOX_FRAMES: i64 = 30;
/// Game frames from a plan being found to pressing No, for pressing the key and following a countdown,
/// like a likely cue's.
const FOLLOW_FRAMES: i64 = 70;
/// Too close to pressing No to still offer a plan: a key press and a short countdown.
const LAST_OFFER_FRAMES: i64 = 40;
/// The latest a plan's No press can be, in game frames from when it's found: 45 seconds, the longest wait
/// the user wanted.
const LATEST_NO_FRAMES: i64 = 900;
/// How far off a press of the Ocarina button or of putting it away can be, either way.
const PRESS_SPREAD_FRAMES: i64 = 4;
/// Game frames to stay after Link's blink, so it surely came first. More than the spread.
const AFTER_BLINK_FRAMES: i64 = PRESS_SPREAD_FRAMES + 2;
/// The most of Link's blinks to wait for. Each one costs up to 5 seconds.
const MAX_BLINKS: usize = 3;
/// The shortest window worth going for, like the ordinary cues.
const MIN_WINDOW_FRAMES: i64 = 13;
/// How far past putting it away to look for a window.
const WINDOW_SEARCH_FRAMES: i64 = 600;
/// A plan has to beat waiting by this much, for all it risks.
const MIN_GAIN_FRAMES: i64 = 40;
/// How much Dampe's next renewal can move from the plan's, for how far off the two presses can be.
const SHORTEST_FREEZE_SHIFT: i64 = -2 * PRESS_SPREAD_FRAMES - FREEZE_EXTRA_SPREAD;
const LONGEST_FREEZE_SHIFT: i64 = 2 * PRESS_SPREAD_FRAMES + OCARINA_EARLY_FRAMES;

/// What to do, in the game frames of the candidates' offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    /// The last frame the Ocarina can come out on.
    pub enter: i64,
    /// The last of Link's blinks waited for.
    pub link_blink: i64,
    /// The frame to start putting it away on.
    pub leave: i64,
    pub window: Window,
}

impl Plan {
    /// The frame to press No on.
    pub fn press_no(&self) -> i64 {
        self.enter - TO_OCARINA_FRAMES
    }

    /// Whether there's still time to follow it, as of game frame `now`.
    pub fn can_follow(&self, now: f64) -> bool {
        (self.press_no() - LAST_OFFER_FRAMES) as f64 > now
    }
}

/// The first run of `MIN_WINDOW_FRAMES` or more in `frames` where every walk rolls the Heart Piece. Walks
/// are model frames, each with what to add to get game frames.
fn first_run(walks: &mut [(StateWalk, i64)], frames: std::ops::Range<i64>) -> Option<Window> {
    let mut run_start = None;
    for frame in frames {
        let good = walks.iter_mut().all(|(walk, offset)| rolls_heart_piece(walk.state_for_roll(frame - *offset)));
        if good {
            run_start.get_or_insert(frame);
        } else if let Some(start) = run_start.take()
            && frame - start >= MIN_WINDOW_FRAMES
        {
            return Some(Window { first_roll: start, last_roll: frame - 1 });
        }
    }
    None
}

/// Every way `candidate` can go from this plan, for how far off the two presses can be.
fn walks_of(candidate: &Candidate, enter: i64, leave: i64) -> Option<Vec<(StateWalk, i64)>> {
    let (enter, leave) = (enter - candidate.offset, leave - candidate.offset);
    let mut blinks = candidate.blinks();
    blinks.run_through(enter - OCARINA_EARLY_FRAMES - PRESS_SPREAD_FRAMES);
    // A draw within reach of the Ocarina coming out might come on either side of it
    if blinks.renewal <= enter + PRESS_SPREAD_FRAMES || blinks.link_next <= enter + PRESS_SPREAD_FRAMES {
        return None;
    }
    blinks.freeze_dampe(enter, leave);
    if blinks.link_next <= leave + PRESS_SPREAD_FRAMES {
        return None;
    }
    // Frozen for less if either press is late, and for more if the Ocarina is early too
    let walks = SHORTEST_FREEZE_SHIFT..=LONGEST_FREEZE_SHIFT;
    let walks = walks.map(|shift| {
        let shifted = Blinks { renewal: blinks.renewal + shift, ..blinks };
        (StateWalk::new(shifted), candidate.offset)
    });
    Some(walks.collect())
}

/// The soonest plan as of game frame `now` that every one of `candidates` agrees on, if it beats the
/// window that waiting gets, `waiting`, by enough to be worth it.
pub fn find(candidates: &[Candidate], now: i64, waiting: Option<Window>) -> Option<Plan> {
    let top = candidates.first()?;
    let earliest = now + FOLLOW_FRAMES + TO_OCARINA_FRAMES;
    let mut best = waiting.map(|window| window.first_roll - MIN_GAIN_FRAMES).unwrap_or(i64::MAX);
    let mut plan = None;
    for enter in earliest..=now + LATEST_NO_FRAMES + TO_OCARINA_FRAMES {
        let model_enter = enter - top.offset;
        let mut blinks = top.blinks();
        blinks.run_through(model_enter);
        let mut blink = blinks.link_next;
        for _ in 0..MAX_BLINKS {
            let leave = blink + top.offset + AFTER_BLINK_FRAMES;
            let first = leave + LEAVE_TO_BOX_FRAMES + BOX_GONE_TO_ROLL;
            if first >= best {
                break;
            }
            let mut after = blinks;
            after.freeze_dampe(model_enter, blink);
            let next_blink = after.link_next;
            let walks: Option<Vec<_>> =
                candidates.iter().map(|candidate| walks_of(candidate, enter, leave)).collect::<Option<Vec<_>>>();
            if let Some(mut walks) = walks.map(|walks| walks.into_iter().flatten().collect::<Vec<_>>())
                && let Some(window) = first_run(&mut walks, first..first + WINDOW_SEARCH_FRAMES)
                && window.first_roll < best
            {
                best = window.first_roll;
                plan = Some(Plan { enter, link_blink: blink + top.offset, leave, window });
            }
            blink = next_blink;
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reading::{HORIZON, first_window};
    use crate::rng::{STATES, next};

    fn candidate(seed: u32) -> Candidate {
        Candidate { seed, link_first: 40, offset: 500 }
    }

    /// The first seed with a plan that beats waiting.
    fn with_a_plan() -> (Candidate, Plan) {
        (0..STATES)
            .step_by(99_991)
            .find_map(|seed| {
                let candidate = candidate(seed);
                let waiting = first_window(&[candidate], 800, MIN_WINDOW_FRAMES, HORIZON);
                find(&[candidate], 700, waiting).map(|plan| (candidate, plan))
            })
            .expect("no seed has a plan")
    }

    /// The game's order of draws, frame by frame, for checking the plan against something it doesn't use.
    fn state_on_roll(candidate: Candidate, plan: Plan, shift: i64, roll: i64) -> u32 {
        let (mut state, mut link, mut renewal) = (candidate.seed, candidate.link_first, 0i64);
        let draw_link = |state: &mut u32, link: &mut i64| {
            *state = next(*state);
            *link += i64::from((*state as f32 / STATES as f32 * 80.0) as i32 + 20);
        };
        let (enter, leave) = (plan.enter - candidate.offset, plan.leave - candidate.offset);
        for frame in 0..=roll - candidate.offset {
            if frame == roll - candidate.offset {
                if link <= frame {
                    draw_link(&mut state, &mut link);
                }
                return state;
            }
            if link <= frame {
                draw_link(&mut state, &mut link);
            }
            let frozen = frame > enter && frame <= leave + shift + FREEZE_EXTRA_FRAMES;
            if !frozen && renewal <= frame {
                state = next(state);
                let wait = (state as f32 / STATES as f32 * 30.0) as i64 + 30;
                state = next(state);
                let count = if state as f32 / STATES as f32 > 0.5 { 4 } else { 3 };
                renewal = frame + wait + 3 * count - 1;
            }
            if frozen {
                renewal += 1;
            }
        }
        unreachable!()
    }

    #[test]
    fn a_plan_is_soon_enough_to_be_worth_it_and_rolls_the_heart_piece() {
        let (candidate, plan) = with_a_plan();
        let waiting = first_window(&[candidate], 800, MIN_WINDOW_FRAMES, HORIZON).map(|window| window.first_roll);
        assert!(waiting.is_none_or(|first| plan.window.first_roll <= first - MIN_GAIN_FRAMES));
        assert!(plan.enter < plan.link_blink && plan.link_blink < plan.leave);
        assert!(plan.window.first_roll >= plan.leave + LEAVE_TO_BOX_FRAMES + BOX_GONE_TO_ROLL);
        for shift in SHORTEST_FREEZE_SHIFT..=LONGEST_FREEZE_SHIFT {
            for roll in plan.window.first_roll..=plan.window.last_roll {
                assert!(rolls_heart_piece(state_on_roll(candidate, plan, shift, roll)), "{shift} {roll}");
            }
        }
    }

    #[test]
    fn the_ocarina_only_holds_back_dampe() {
        let mut blinks = Blinks::new(12345, 20);
        blinks.run_through(10);
        let (renewal, state) = (blinks.renewal, blinks.state);
        assert!(renewal > 10);
        blinks.freeze_dampe(10, 30);
        assert_eq!(blinks.state, next(state), "Link's one blink, on frame 20");
        assert_eq!(blinks.renewal, renewal + 20 + FREEZE_EXTRA_FRAMES);
    }

    /// A reading that disagrees about Link's blink can't be planned for, since nothing can be cued.
    #[test]
    fn readings_that_disagree_get_no_plan() {
        let (first, plan) = with_a_plan();
        let other = Candidate { link_first: first.link_first + 7, ..first };
        let found = find(&[first, other], 700, None);
        assert_ne!(found, Some(plan));
    }
}
