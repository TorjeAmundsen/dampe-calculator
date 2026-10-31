//! Finds every RNG state and Link blink phase that explains the flutters seen. The search space is
//! small enough to walk: 2^23 states times 99 phases, and almost all of them fail on the first
//! flutters they're compared to.
//!
//! The video can show flutters that weren't (Link's eye, a flicker) and miss ones that were, so a
//! reading isn't compared to a chain. It draws its own flutters, and each one is looked up among
//! what was seen.

use crate::rng::{Blinks, STATES, flutter_len};

/// Link's first draw comes on one of these frames after the first renewal.
const LINK_PHASES: std::ops::RangeInclusive<i64> = 1..=99;
/// Flutters a reading may draw that the video didn't show, until it has enough hits to count.
pub const SEARCH_MISSES: u32 = 2;
/// How long after a flutter's end it's reported, at most: until then it can't count as missed.
const REPORT_DELAY: i64 = 8;

/// A flutter to match: its start in game frames, and its blink count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observation {
    pub start: i64,
    pub count: u8,
}

/// How a reading compares to the observations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Match {
    /// Its flutters that were seen.
    pub hits: u32,
    /// Its flutters that weren't.
    pub misses: u32,
    /// Observations it doesn't have.
    pub extras: u32,
    pub misses_since_hit: u32,
    pub extras_since_hit: u32,
}

/// One reading of the observations. Model frames plus `offset` are game frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub seed: u32,
    pub link_first: i64,
    pub offset: i64,
}

impl Candidate {
    /// The model, at the renewal before the first observation.
    pub fn blinks(&self) -> Blinks {
        Blinks::new(self.seed, self.link_first)
    }

    /// Compares this reading's flutters to `observations`, sorted by start, up to game frame `now`.
    pub fn compare(&self, observations: &[Observation], now: i64) -> Match {
        compare(self.blinks(), self.offset, observations, now, u32::MAX).expect("no limit on misses")
    }

    /// The state after the last flutter this reading has at or before game frame `frame`.
    pub fn state_at_flutter(&self, frame: i64) -> u32 {
        let mut blinks = self.blinks();
        let mut state = blinks.state;
        while blinks.next_flutter().start + self.offset <= frame {
            state = blinks.state;
        }
        state
    }
}

/// `None` as soon as there are more than `max_misses`.
#[inline]
fn compare(mut blinks: Blinks, offset: i64, observations: &[Observation], now: i64, max_misses: u32) -> Option<Match> {
    let mut result = Match::default();
    let mut next = 0;
    loop {
        let flutter = blinks.next_flutter();
        let start = flutter.start + offset;
        if start + flutter_len(flutter.count) + REPORT_DELAY > now {
            break;
        }
        let before = next;
        while observations.get(next).is_some_and(|o| o.start < start) {
            next += 1;
        }
        let passed = (next - before) as u32;
        result.extras += passed;
        result.extras_since_hit += passed;
        // Several tracks can report the same flutter, with different counts
        let mut hit = false;
        let mut partly = false;
        while let Some(observation) = observations.get(next)
            && observation.start == start
        {
            // Something can hide the last of 4 blinks, like his jaw
            let last_hidden = flutter.count == 4 && observation.count == 3;
            if observation.count == flutter.count && !hit {
                hit = true;
            } else if last_hidden && !partly {
                partly = true;
            } else {
                result.extras += 1;
                result.extras_since_hit += 1;
            }
            next += 1;
        }
        // Or the first of 4, and then it looks like a 3 starting one blink later
        if !hit
            && !partly
            && flutter.count == 4
            && let Some(observation) = observations.get(next)
            && observation.start == start + 3
            && observation.count == 3
        {
            partly = true;
            next += 1;
        }
        // Partly seen says little for a reading, since a wrong one can land there by chance too, but
        // it isn't against it either
        if partly && !hit {
            continue;
        }
        if hit {
            result.hits += 1;
            result.misses_since_hit = 0;
            result.extras_since_hit = 0;
        } else {
            result.misses += 1;
            result.misses_since_hit += 1;
            if result.misses > max_misses {
                return None;
            }
        }
    }
    let rest = (observations.len() - next) as u32;
    let unreported = observations[next..].iter().filter(|o| o.start > now - REPORT_DELAY).count() as u32;
    result.extras += rest - unreported;
    result.extras_since_hit += rest - unreported;
    Some(result)
}

fn scan(from: u32, to: u32, observations: &[Observation], now: i64, min_hits: u32) -> Vec<Candidate> {
    let mut found = Vec::new();
    let first = observations[0];
    for seed in from..to {
        // The first flutter has no Link draw before it, whatever the phase
        let mut start = Blinks::new(seed, i64::MAX);
        let flutter = start.next_flutter();
        if flutter.count != first.count {
            continue;
        }
        let offset = first.start - flutter.start;
        for link_first in LINK_PHASES {
            let blinks = Blinks { link_next: link_first, ..start };
            let Some(result) = compare(blinks, offset, &observations[1..], now, SEARCH_MISSES) else { continue };
            if result.hits + 1 >= min_hits {
                found.push(Candidate { seed, link_first, offset });
            }
        }
    }
    found
}

/// Every reading whose first flutter is `observations[0]`, with at least `min_hits` flutters seen,
/// as of game frame `now`. This walks every state, on half of the cores: video comes first.
pub fn scan_from(observations: &[Observation], now: i64, min_hits: u32) -> Vec<Candidate> {
    let threads = (std::thread::available_parallelism().map_or(4, |n| n.get()) as u32 / 2).max(1);
    let chunk = STATES.div_ceil(threads);
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|i| {
                let (from, to) = (i * chunk, ((i + 1) * chunk).min(STATES));
                scope.spawn(move || scan(from, to, observations, now, min_hits))
            })
            .collect();
        workers.into_iter().flat_map(|worker| worker.join().expect("a scan thread panicked")).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observed(seed: u32, link_first: i64, n: usize, base: i64) -> (Vec<Observation>, i64) {
        let mut blinks = Blinks::new(seed, link_first);
        let observations: Vec<_> = (0..n)
            .map(|_| {
                let flutter = blinks.next_flutter();
                Observation { start: flutter.start + base, count: flutter.count }
            })
            .collect();
        let last = observations.last().expect("n > 0");
        let now = last.start + flutter_len(last.count) + REPORT_DELAY;
        (observations, now)
    }

    fn finds(observations: &[Observation], now: i64, seed: u32, link: i64) -> bool {
        let truth = Candidate { seed, link_first: link, offset: 0 };
        let state = truth_state(truth, observations, now);
        scan_from(observations, now, 6).iter().any(|c| c.state_at_flutter(now) == state)
    }

    fn truth_state(truth: Candidate, observations: &[Observation], now: i64) -> u32 {
        let base = observations[0].start - truth.blinks().next_flutter().start;
        Candidate { offset: base, ..truth }.state_at_flutter(now)
    }

    #[test]
    fn finds_the_true_state_from_exact_flutters() {
        for (seed, link) in [(1_234_567, 37), (8_000_001, 99), (5, 1)] {
            let (observations, now) = observed(seed, link, 8, 1000);
            assert!(finds(&observations, now, seed, link), "{seed} {link}");
        }
    }

    /// Strays must not stop the search, or one flicker of Link's eye would cost a whole run.
    #[test]
    fn finds_the_true_state_through_a_stray_and_a_miss() {
        let (mut observations, now) = observed(1_234_567, 37, 10, 1000);
        observations.remove(4);
        let stray = Observation { start: observations[5].start + 9, count: 3 };
        observations.insert(6, stray);
        assert!(finds(&observations, now, 1_234_567, 37));
    }

    /// The jaw can hide one blink of 4, at either end. Those can't count against a reading, or his
    /// head-on angle can't ever lock, but they don't count for it either: when they did, a wrong
    /// reading got locked on a real recording.
    #[test]
    fn a_four_with_an_end_hidden_is_a_hit() {
        let (observations, now) = observed(1_234_567, 37, 12, 0);
        let candidate = Candidate { seed: 1_234_567, link_first: 37, offset: 0 };
        let four = observations.iter().position(|o| o.count == 4).expect("no 4 in there");
        let exact = Match { hits: 11, ..Match::default() };
        let mut last_hidden = observations.clone();
        last_hidden[four].count = 3;
        assert_eq!(candidate.compare(&last_hidden, now), exact);
        let mut first_hidden = observations.clone();
        first_hidden[four].count = 3;
        first_hidden[four].start += 3;
        assert_eq!(candidate.compare(&first_hidden, now), exact);
        // The control: one frame off isn't a hidden blink
        let mut off = observations.clone();
        off[four].count = 3;
        off[four].start += 2;
        let result = candidate.compare(&off, now);
        assert_eq!((result.hits, result.misses), (11, 1));
    }

    /// A wrong count has to count against a reading, or wrong reads could never be noticed.
    #[test]
    fn a_wrong_count_is_a_miss_and_an_extra() {
        let (mut observations, now) = observed(1_234_567, 37, 8, 0);
        let candidate = Candidate { seed: 1_234_567, link_first: 37, offset: 0 };
        assert_eq!(candidate.compare(&observations, now), Match { hits: 8, ..Match::default() });
        // A 3 can't look like a 4: that takes a blink that wasn't there
        let three = observations.iter().position(|o| o.count == 3).expect("no 3 in there");
        observations[three].count = 4;
        let result = candidate.compare(&observations, now);
        assert_eq!((result.hits, result.misses, result.extras), (7, 1, 1));
    }
}
