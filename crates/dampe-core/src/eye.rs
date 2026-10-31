//! Where the eye is at each point of the idle loop, for showing it.
//!
//! The head's pose only depends on the point of the loop, and the animation moves it smoothly, so
//! the eye's path is one smooth closed curve. That's what gets fitted: a short Fourier series
//! through every position clean flutters showed. Large harmonics are held back, so stretches of the
//! loop no flutter has passed through yet stay on a sensible path, and samples far off the curve
//! are left out, so a stray can't pull it away.
//!
//! Dampe has two eyes, and they blink together. From some angles both show, and a flutter can land
//! on either, about 40 pixels apart. So there can be several paths: each flutter joins the path it
//! fits, and what's shown is the path most flutters agree on. A stray starts a path of its own that
//! nothing else joins. Switching takes a clear majority, so the box stays on one eye.

use std::collections::VecDeque;

use crate::rng::IDLE_LOOP_FRAMES;

/// Harmonics in the series, and so the coefficients per axis.
const HARMONICS: usize = 4;
const COEFFICIENTS: usize = 1 + 2 * HARMONICS;
/// How strongly harmonic k is held back, times k^4.
const SMOOTHING: f32 = 0.1;
/// The samples kept: about 50 flutters.
const MAX_SAMPLES: usize = 400;
/// Samples before there's a curve: about 2 flutters.
const MIN_SAMPLES: usize = 12;
/// A sample this far from the curve, in pixels, is left out of the next pass.
const OUTLIER_DISTANCE: f32 = 10.0;
const FIT_PASSES: usize = 4;

/// A flutter joins a path when it's on average this close to it, in pixels.
const JOIN_DISTANCE: f32 = 12.0;
/// Flutters are compared with a path's samples this far along the loop, with this much more room per
/// point for the head's motion. Not with its curve: a young curve is only a guess between samples.
const NEAR_POINTS: usize = 4;
const NEAR_ROOM: f32 = 6.0;
/// Two paths this close, on average, are the same one seen in two stretches of the loop.
const MERGE_DISTANCE: f32 = 8.0;
const MAX_PATHS: usize = 6;

/// A position: the point of the loop, x and y.
type Sample = (usize, f32, f32);

/// One path around the idle loop.
#[derive(Default)]
struct Path {
    samples: VecDeque<Sample>,
    coefficients: Option<([f32; COEFFICIENTS], [f32; COEFFICIENTS])>,
}

/// The eyes' paths around the idle loop, and which one is shown.
#[derive(Default)]
pub struct EyeCurve {
    paths: Vec<Path>,
    shown: Option<usize>,
}

impl EyeCurve {
    /// Adds the positions a clean flutter showed, as (point of the loop, x, y).
    pub fn add(&mut self, positions: impl IntoIterator<Item = Sample>) {
        let positions: Vec<Sample> = positions.into_iter().collect();
        let distances: Vec<(usize, f32)> = self
            .paths
            .iter()
            .enumerate()
            .filter_map(|(i, path)| path.distance(&positions).map(|distance| (i, distance)))
            .collect();
        let target = if distances.is_empty() {
            // Nothing's known about this stretch of the loop yet. Most flutters are the same eye,
            // and the fit leaves out what turns out not to belong
            (0..self.paths.len()).max_by_key(|&i| self.paths[i].samples.len())
        } else {
            distances
                .iter()
                .filter(|&&(_, distance)| distance <= JOIN_DISTANCE)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|&(i, _)| i)
        };
        match target {
            Some(i) => self.paths[i].add(positions),
            None => {
                let mut path = Path::default();
                path.add(positions);
                self.paths.push(path);
            }
        }
        self.merge();
        self.limit();
        self.choose();
    }

    /// Where the eye is at a point of the loop, once enough has been seen.
    pub fn at(&self, point: usize) -> Option<(f32, f32)> {
        self.paths.get(self.shown?)?.at(point)
    }

    /// How many positions the shown path has.
    pub fn samples(&self) -> usize {
        self.shown.and_then(|shown| self.paths.get(shown)).map_or(0, |path| path.samples.len())
    }

    /// For each point of the loop, where the shown path's flutters really had the eye, if they
    /// passed there: the median of its samples that fit the curve.
    pub fn seen_at_points(&self) -> Vec<Option<(f32, f32)>> {
        let mut out = vec![None; IDLE_LOOP_FRAMES];
        let Some(path) = self.shown.and_then(|shown| self.paths.get(shown)) else { return out };
        for (point, out) in out.iter_mut().enumerate() {
            let Some(curve) = path.at(point) else { continue };
            let (mut xs, mut ys): (Vec<f32>, Vec<f32>) = path
                .samples
                .iter()
                .filter(|&&(at, x, y)| at == point && (x - curve.0).hypot(y - curve.1) <= OUTLIER_DISTANCE)
                .map(|&(_, x, y)| (x, y))
                .unzip();
            if xs.is_empty() {
                continue;
            }
            xs.sort_by(f32::total_cmp);
            ys.sort_by(f32::total_cmp);
            *out = Some((xs[xs.len() / 2], ys[ys.len() / 2]));
        }
        out
    }

    /// Joins paths that turn out to be one.
    fn merge(&mut self) {
        for a in 0..self.paths.len() {
            for b in (a + 1..self.paths.len()).rev() {
                let samples: Vec<Sample> = self.paths[b].samples.iter().copied().collect();
                if self.paths[a].distance(&samples).is_some_and(|distance| distance <= MERGE_DISTANCE) {
                    self.paths.remove(b);
                    self.paths[a].add(samples);
                    self.shown = self.shown.map(|shown| if shown == b { a } else { shown - usize::from(shown > b) });
                }
            }
        }
    }

    /// Drops the smallest path when there are too many: those are strays.
    fn limit(&mut self) {
        if self.paths.len() <= MAX_PATHS {
            return;
        }
        let smallest = (0..self.paths.len())
            .filter(|&i| Some(i) != self.shown)
            .min_by_key(|&i| self.paths[i].samples.len())
            .expect("there's more than one path");
        self.paths.remove(smallest);
        self.shown = self.shown.map(|shown| shown - usize::from(shown > smallest));
    }

    fn choose(&mut self) {
        let Some(best) = (0..self.paths.len())
            .filter(|&i| self.paths[i].coefficients.is_some())
            .max_by_key(|&i| self.paths[i].samples.len())
        else {
            return;
        };
        let keep = self.shown.is_some_and(|shown| {
            self.paths[shown].coefficients.is_some()
                && self.paths[best].samples.len() * 2 <= self.paths[shown].samples.len() * 3
        });
        if !keep {
            self.shown = Some(best);
        }
    }
}

fn basis(point: usize) -> [f32; COEFFICIENTS] {
    let angle = point as f32 / IDLE_LOOP_FRAMES as f32 * std::f32::consts::TAU;
    let mut out = [1.0; COEFFICIENTS];
    for k in 1..=HARMONICS {
        out[2 * k - 1] = (k as f32 * angle).cos();
        out[2 * k] = (k as f32 * angle).sin();
    }
    out
}

fn evaluate(coefficients: &[f32; COEFFICIENTS], basis: &[f32; COEFFICIENTS]) -> f32 {
    coefficients.iter().zip(basis).map(|(c, b)| c * b).sum()
}

/// Solves `matrix * x = right` by Gaussian elimination. The smoothing keeps `matrix` well behaved.
#[allow(clippy::needless_range_loop)]
fn solve(mut matrix: [[f32; COEFFICIENTS]; COEFFICIENTS], mut right: [f32; COEFFICIENTS]) -> [f32; COEFFICIENTS] {
    for column in 0..COEFFICIENTS {
        let pivot = (column..COEFFICIENTS)
            .max_by(|&a, &b| matrix[a][column].abs().total_cmp(&matrix[b][column].abs()))
            .expect("the range isn't empty");
        matrix.swap(column, pivot);
        right.swap(column, pivot);
        for row in column + 1..COEFFICIENTS {
            let factor = matrix[row][column] / matrix[column][column];
            for i in column..COEFFICIENTS {
                matrix[row][i] -= factor * matrix[column][i];
            }
            right[row] -= factor * right[column];
        }
    }
    let mut out = [0.0; COEFFICIENTS];
    for row in (0..COEFFICIENTS).rev() {
        let known: f32 = (row + 1..COEFFICIENTS).map(|i| matrix[row][i] * out[i]).sum();
        out[row] = (right[row] - known) / matrix[row][row];
    }
    out
}

impl Path {
    /// How far `positions` are from this path on average. `None` when nothing's known nearby.
    fn distance(&self, positions: &[Sample]) -> Option<f32> {
        let distances: Vec<f32> = positions
            .iter()
            .filter_map(|&(point, x, y)| {
                self.samples
                    .iter()
                    .filter_map(|&(near, near_x, near_y)| {
                        let along = near.abs_diff(point);
                        let along = along.min(IDLE_LOOP_FRAMES - along);
                        (along <= NEAR_POINTS)
                            .then(|| ((near_x - x).hypot(near_y - y) - NEAR_ROOM * along as f32).max(0.0))
                    })
                    .min_by(f32::total_cmp)
            })
            .collect();
        (!distances.is_empty()).then(|| distances.iter().sum::<f32>() / distances.len() as f32)
    }

    fn add(&mut self, positions: impl IntoIterator<Item = Sample>) {
        self.samples.extend(positions);
        let excess = self.samples.len().saturating_sub(MAX_SAMPLES);
        self.samples.drain(..excess);
        self.fit();
    }

    fn at(&self, point: usize) -> Option<(f32, f32)> {
        let (x, y) = self.coefficients.as_ref()?;
        let basis = basis(point);
        Some((evaluate(x, &basis), evaluate(y, &basis)))
    }

    #[allow(clippy::needless_range_loop)]
    fn fit(&mut self) {
        if self.samples.len() < MIN_SAMPLES {
            return;
        }
        let mut used = vec![true; self.samples.len()];
        let mut fitted = None;
        for _ in 0..FIT_PASSES {
            let (mut matrix, mut right_x, mut right_y) =
                ([[0.0; COEFFICIENTS]; COEFFICIENTS], [0.0; COEFFICIENTS], [0.0; COEFFICIENTS]);
            for (&(point, x, y), _) in self.samples.iter().zip(&used).filter(|&(_, &used)| used) {
                let basis = basis(point);
                for i in 0..COEFFICIENTS {
                    right_x[i] += basis[i] * x;
                    right_y[i] += basis[i] * y;
                    for j in 0..COEFFICIENTS {
                        matrix[i][j] += basis[i] * basis[j];
                    }
                }
            }
            for k in 1..=HARMONICS {
                let penalty = SMOOTHING * (k as f32).powi(4);
                matrix[2 * k - 1][2 * k - 1] += penalty;
                matrix[2 * k][2 * k] += penalty;
            }
            matrix[0][0] += 1e-3;
            let (coefficients_x, coefficients_y) = (solve(matrix, right_x), solve(matrix, right_y));
            for (&(point, x, y), used) in self.samples.iter().zip(used.iter_mut()) {
                let basis = basis(point);
                let distance = (evaluate(&coefficients_x, &basis) - x).hypot(evaluate(&coefficients_y, &basis) - y);
                *used = distance <= OUTLIER_DISTANCE;
            }
            fitted = Some((coefficients_x, coefficients_y));
        }
        self.coefficients = fitted;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(point: usize) -> (f32, f32) {
        let angle = point as f32 / IDLE_LOOP_FRAMES as f32 * std::f32::consts::TAU;
        (200.0 + 40.0 * angle.sin() + 8.0 * (2.0 * angle).cos(), 150.0 + 30.0 * angle.cos())
    }

    /// A flutter's worth of samples: 8 neighbouring points of the loop, with the blob's wobble.
    fn flutter(at: usize) -> Vec<(usize, f32, f32)> {
        (0..8)
            .map(|i| {
                let point = (at + i) % IDLE_LOOP_FRAMES;
                let wobble = if i % 2 == 0 { 1.5 } else { -1.5 };
                let (x, y) = path(point);
                (point, x + wobble, y - wobble)
            })
            .collect()
    }

    fn worst_error(curve: &EyeCurve) -> f32 {
        (0..IDLE_LOOP_FRAMES)
            .map(|point| {
                let (x, y) = curve.at(point).expect("no curve");
                let truth = path(point);
                (x - truth.0).hypot(y - truth.1)
            })
            .fold(0.0, f32::max)
    }

    #[test]
    fn nothing_until_enough_is_seen() {
        let mut curve = EyeCurve::default();
        curve.add(flutter(0));
        assert_eq!(curve.at(0), None);
        curve.add(flutter(20));
        assert!(curve.at(0).is_some());
    }

    #[test]
    fn follows_the_path_around_the_whole_loop() {
        let mut curve = EyeCurve::default();
        for at in [0, 8, 17, 25, 33, 41] {
            curve.add(flutter(at));
        }
        assert!(worst_error(&curve) < 3.0, "{}", worst_error(&curve));
    }

    /// A stray that made it in is far from the path, and mustn't drag the curve to it.
    #[test]
    fn a_few_strays_do_not_move_it() {
        let mut curve = EyeCurve::default();
        for at in [0, 8, 17, 25, 33, 41, 4, 13, 21, 29, 37, 45] {
            curve.add(flutter(at));
        }
        let stray: Vec<_> = (0..8).map(|i| ((3 + i) % IDLE_LOOP_FRAMES, 450.0, 40.0)).collect();
        curve.add(stray);
        assert!(worst_error(&curve) < 3.0, "{}", worst_error(&curve));
    }

    /// Both eyes show from some angles. The curve has to stay on one, not settle between them.
    #[test]
    fn a_second_eye_does_not_pull_it_away() {
        let mut curve = EyeCurve::default();
        for at in [0, 8, 17, 25, 33, 41, 4, 13, 21, 29, 37, 45] {
            curve.add(flutter(at));
            let second: Vec<_> = flutter(at + 2).into_iter().map(|(point, x, y)| (point, x + 40.0, y)).collect();
            if at % 3 == 0 {
                curve.add(second);
            }
        }
        assert!(worst_error(&curve) < 3.0, "{}", worst_error(&curve));
    }

    /// What the box is for: no jumps from one frame to the next.
    #[test]
    fn neighbouring_points_stay_close() {
        let mut curve = EyeCurve::default();
        for at in [0, 8, 17] {
            curve.add(flutter(at));
        }
        for point in 0..IDLE_LOOP_FRAMES {
            let (a, b) =
                (curve.at(point).expect("no curve"), curve.at((point + 1) % IDLE_LOOP_FRAMES).expect("no curve"));
            assert!((a.0 - b.0).hypot(a.1 - b.1) < 15.0, "at {point}");
        }
    }
}
