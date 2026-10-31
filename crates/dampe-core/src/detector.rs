//! Finds Dampe's flutters in the pictures from [`crate::picture`], with no face tracking.
//!
//! His idle animation loops every [`IDLE_LOOP_FRAMES`] game frames, and the game draws the same
//! pose the same way each time. So a frame minus the open-eye frames from the same point of the
//! loop is nothing but what really changed: the eye, plus whatever else moves on its own schedule,
//! like Link or the sky. Those are told apart by their pattern over time. Each blob is followed on
//! its own, and only the eye goes half closed, closed, open, one game frame each, 3 or 4 times.

use std::collections::VecDeque;
use std::sync::Arc;

use crate::eye::EyeCurve;
use crate::picture::{HEIGHT, Luma, WIDTH};
use crate::rng::{IDLE_LOOP_FRAMES, flutter_len};

/// A pixel changed when it's this far from the reference.
const CHANGE: i16 = 12;
/// A new game frame changes at least this many pixels by this much. Lossless repeats change none;
/// the margin is for noisy feeds.
const NEW_FRAME_CHANGE: i16 = 2;
const NEW_FRAME_PIXELS: usize = 8;
/// Every this many rows and columns are compared for a new game frame.
const CADENCE_STEP: usize = 3;

/// Pixels that keep changing, like Link's outline, are ignored above this share of recent frames.
/// A flutter changes the same eye pixels up to 8 times in 12 frames, which has to stay well below it.
const BUSY_SHARE: f32 = 0.35;
const BUSY_RATE: f32 = 0.02;

/// A blob that could be the eye, at the working scale. Half-closed eyes change few pixels.
const BLOB_MIN_AREA: u32 = 8;
const BLOB_MAX_AREA: u32 = 375;
const BLOB_MAX_WIDTH: u32 = 35;
const BLOB_MAX_HEIGHT: u32 = 30;
const BLOB_MIN_FILL: f32 = 0.3;

/// More blobs than this mean the reference is bad, from before the scene settled, and not that
/// there are many eyes. Such frames aren't tracked, and they replace the reference soon enough.
const MAX_BLOBS: usize = 12;

/// Open-eye frames kept per point of the loop, and how many are needed to compare at all.
const REFERENCE_FRAMES: usize = 5;
const REFERENCE_MIN: usize = 3;
/// Game frames a frame waits before joining the reference: long enough to know it wasn't part of a
/// flutter, which is only recognized after it ended.
const SETTLE_FRAMES: usize = 18;

/// A track of blobs ends after this many game frames without one. One lost half-closed frame in a
/// flutter leaves a gap of 3.
const TRACK_MAX_GAP: i64 = 3;
/// How far a blob may be from a track's last one, in pixels: head motion, per frame of gap.
const TRACK_RADIUS: f32 = 10.0;
const TRACK_RADIUS_PER_FRAME: f32 = 3.0;
/// A flutter is at most 11 frames. A track much longer than that is something else, and it's dropped
/// so it can't keep taking the eye's blobs when the two come close.
const TRACK_MAX_FRAMES: usize = 40;
/// The shown eye box, half its width and height.
const EYE_BOX_X: u16 = 10;
const EYE_BOX_Y: u16 = 7;
/// How much of a flutter's pattern has to match, including the quiet frames around it.
const MIN_FIT: f32 = 0.84;
/// The quiet frames checked on each side of a flutter.
const FLANK: i64 = 2;
/// How far past a track's first frame a flutter's start may be, for a stray frame before it.
const START_SLACK: i64 = 3;

/// A flutter seen in the video.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flutter {
    /// The game frame of the first half-closed eye, counted by [`Detector`].
    pub start: i64,
    pub count: u8,
    /// How much of the pattern matched, 0 to 1.
    pub fit: f32,
}

impl Flutter {
    /// Whether every frame matched. On real footage, about 97% of these are real flutters, against
    /// about 1 in 5 of the others, so these are what's worth starting a scan from.
    pub fn is_clean(&self) -> bool {
        self.fit >= 1.0
    }
}

/// A rectangle in the picture, in pixels, right and bottom inclusive.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: u16,
    pub top: u16,
    pub right: u16,
    pub bottom: u16,
}

impl Rect {
    pub fn overlaps(&self, other: &Rect) -> bool {
        self.left <= other.right && other.left <= self.right && self.top <= other.bottom && other.top <= self.bottom
    }

    pub fn center(&self) -> (u16, u16) {
        ((self.left + self.right) / 2, (self.top + self.bottom) / 2)
    }
}

/// A blob that could be the eye.
#[derive(Clone, Copy, Debug)]
struct Blob {
    center: (f32, f32),
    rect: Rect,
}

/// How the detector has been doing, to explain a silence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Health {
    pub game_frames: u32,
    /// Frames ignored for having a bad reference.
    pub bad_reference: u32,
    /// Frames with blobs that could be the eye.
    pub with_blobs: u32,
    /// Tracks that ended without looking like a flutter.
    pub rejected_tracks: u32,
    /// Tracks being followed right now.
    pub tracks: usize,
}

/// What a game frame showed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameInfo {
    pub game_frame: i64,
    /// Whether the frame could be compared yet.
    pub compared: bool,
    /// Blobs that could be the eye.
    pub blobs: usize,
    /// So much differed from the reference that the frame was ignored.
    pub bad_reference: bool,
}

/// Counts game frames: a video frame that differs from the one before it starts a new one. Virtual
/// cameras repeat and skip video frames, which only changes how many copies are seen. Sparse samples
/// of the middle of the frame are enough, and most video frames are repeats that need nothing else.
#[derive(Default)]
pub struct Cadence {
    previous: Vec<u8>,
    samples: Vec<u8>,
}

impl Cadence {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_new_frame(&mut self, frame: Luma<'_>) -> bool {
        self.samples.clear();
        // The middle only: a corner can hold something that changes on its own, like an RNG watch
        let (x_from, x_to) = (frame.width / 10, frame.width - frame.width / 10);
        for y in (frame.height / 10..frame.height - frame.height / 10).step_by(CADENCE_STEP) {
            let row = frame.row(y);
            self.samples.extend(row[x_from..x_to].iter().step_by(CADENCE_STEP));
        }
        if self.previous.len() != self.samples.len() {
            self.previous.clone_from(&self.samples);
            return true;
        }
        let changed = self
            .samples
            .iter()
            .zip(&self.previous)
            .filter(|&(&a, &b)| (i16::from(a) - i16::from(b)).abs() > NEW_FRAME_CHANGE)
            .count();
        if changed < NEW_FRAME_PIXELS {
            return false;
        }
        self.previous.copy_from_slice(&self.samples);
        true
    }
}

#[derive(Default)]
struct LoopPoint {
    frames: VecDeque<Arc<[u8]>>,
    reference: Option<Vec<u8>>,
}

impl LoopPoint {
    fn reference(&mut self) -> Option<&[u8]> {
        if self.frames.len() < REFERENCE_MIN {
            return None;
        }
        if self.reference.is_none() {
            self.reference = Some(median(&self.frames));
        }
        self.reference.as_deref()
    }

    fn add(&mut self, frame: Arc<[u8]>) {
        self.frames.push_back(frame);
        if self.frames.len() > REFERENCE_FRAMES {
            self.frames.pop_front();
        }
        self.reference = None;
    }
}

fn median(frames: &VecDeque<Arc<[u8]>>) -> Vec<u8> {
    let mut out = vec![0; WIDTH * HEIGHT];
    let mut samples = [0u8; REFERENCE_FRAMES];
    let n = frames.len();
    for (i, pixel) in out.iter_mut().enumerate() {
        for (sample, frame) in samples.iter_mut().zip(frames) {
            *sample = frame[i];
        }
        samples[..n].sort_unstable();
        *pixel = samples[n / 2];
    }
    out
}

/// Finds the blobs in a mask that are shaped like an eye.
struct BlobFinder {
    closed: Vec<u8>,
    labels: Vec<u32>,
    parents: Vec<u32>,
    stats: Vec<BlobStats>,
}

#[derive(Clone, Copy)]
struct BlobStats {
    area: u32,
    left: u32,
    right: u32,
    top: u32,
    bottom: u32,
    sum_x: u64,
    sum_y: u64,
}

impl BlobFinder {
    fn new() -> Self {
        Self {
            closed: vec![0; WIDTH * HEIGHT],
            labels: vec![0; WIDTH * HEIGHT],
            parents: Vec::new(),
            stats: Vec::new(),
        }
    }

    fn find_root(parents: &mut [u32], mut label: u32) -> u32 {
        while parents[label as usize] != label {
            parents[label as usize] = parents[parents[label as usize] as usize];
            label = parents[label as usize];
        }
        label
    }

    /// Bridges one-pixel gaps: a blob's inside can match the reference by chance.
    fn close(&mut self, mask: &[u8]) {
        let (w, h) = (WIDTH, HEIGHT);
        self.closed.copy_from_slice(mask);
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let i = y * w + x;
                if mask[i] != 0 {
                    continue;
                }
                let bridged = (mask[i - 1] != 0 && mask[i + 1] != 0) || (mask[i - w] != 0 && mask[i + w] != 0);
                if bridged {
                    self.closed[i] = 1;
                }
            }
        }
    }

    /// Writes the eye-shaped blobs in `mask` to `out`.
    fn find(&mut self, mask: &[u8], out: &mut Vec<Blob>) {
        self.close(mask);
        self.parents.clear();
        self.parents.push(0);
        self.labels.fill(0);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let i = y * WIDTH + x;
                if self.closed[i] == 0 {
                    continue;
                }
                let up = if y > 0 { self.labels[i - WIDTH] } else { 0 };
                let left = if x > 0 { self.labels[i - 1] } else { 0 };
                let label = match (up, left) {
                    (0, 0) => {
                        let new = self.parents.len() as u32;
                        self.parents.push(new);
                        new
                    }
                    (a, 0) | (0, a) => a,
                    (a, b) => {
                        let (a, b) = (Self::find_root(&mut self.parents, a), Self::find_root(&mut self.parents, b));
                        self.parents[a.max(b) as usize] = a.min(b);
                        a.min(b)
                    }
                };
                self.labels[i] = label;
            }
        }
        let empty = BlobStats { area: 0, left: u32::MAX, right: 0, top: u32::MAX, bottom: 0, sum_x: 0, sum_y: 0 };
        self.stats.clear();
        self.stats.resize(self.parents.len(), empty);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let label = self.labels[y * WIDTH + x];
                if label == 0 {
                    continue;
                }
                let root = Self::find_root(&mut self.parents, label) as usize;
                let stats = &mut self.stats[root];
                stats.area += 1;
                stats.left = stats.left.min(x as u32);
                stats.right = stats.right.max(x as u32);
                stats.top = stats.top.min(y as u32);
                stats.bottom = stats.bottom.max(y as u32);
                stats.sum_x += x as u64;
                stats.sum_y += y as u64;
            }
        }
        out.clear();
        for stats in &self.stats {
            if stats.area < BLOB_MIN_AREA || stats.area > BLOB_MAX_AREA {
                continue;
            }
            let (width, height) = (stats.right - stats.left + 1, stats.bottom - stats.top + 1);
            let fill = stats.area as f32 / (width * height) as f32;
            if width <= BLOB_MAX_WIDTH && height <= BLOB_MAX_HEIGHT && fill >= BLOB_MIN_FILL {
                out.push(Blob {
                    center: (stats.sum_x as f32 / stats.area as f32, stats.sum_y as f32 / stats.area as f32),
                    rect: Rect {
                        left: stats.left as u16,
                        top: stats.top as u16,
                        right: stats.right as u16,
                        bottom: stats.bottom as u16,
                    },
                });
            }
        }
    }
}

/// One blob followed over frames. The eye's track blinks; the sky's stays on.
struct Track {
    /// The game frames it had a blob on, ascending, and where.
    frames: Vec<i64>,
    centers: Vec<(f32, f32)>,
    rects: Vec<Rect>,
}

impl Track {
    fn last_frame(&self) -> i64 {
        *self.frames.last().expect("a track starts with a frame")
    }

    fn last_center(&self) -> (f32, f32) {
        *self.centers.last().expect("a track starts with a center")
    }

    fn has(&self, frame: i64) -> bool {
        self.frames.binary_search(&frame).is_ok()
    }

    /// The legal flutter that best matches this track. A half-closed frame can be too faint to see
    /// and a stray frame can come first, so the track's edges don't set the start.
    fn fit(&self) -> Option<Flutter> {
        let (first, last) = (self.frames[0], self.last_frame());
        let mut best: Option<Flutter> = None;
        for start in first - 1..=first + START_SLACK {
            for count in [3u8, 4] {
                let len = flutter_len(count);
                // A flutter has to explain the track's end, or a 4 could read as a 3
                if start + len - 1 < last - 1 {
                    continue;
                }
                let matched = (-FLANK..len + FLANK)
                    .filter(|&offset| {
                        let expected = (0..len).contains(&offset) && offset % 3 != 2;
                        self.has(start + offset) == expected
                    })
                    .count();
                let fit = matched as f32 / (len + 2 * FLANK) as f32;
                if best.is_none_or(|b| fit > b.fit) {
                    best = Some(Flutter { start, count, fit });
                }
            }
        }
        best.filter(|flutter| flutter.fit >= MIN_FIT)
    }
}

struct Pending {
    game_frame: i64,
    frame: Arc<[u8]>,
}

/// Turns game frames into flutters.
pub struct Detector {
    points: Vec<LoopPoint>,
    pending: VecDeque<Pending>,
    busy: Vec<f32>,
    mask: Vec<u8>,
    blobs: BlobFinder,
    centers: Vec<Blob>,
    tracks: Vec<Track>,
    game_frame: i64,
    bad_reference: bool,
    health: Health,
    /// Where the eye is around the loop. Only for showing it: reading flutters doesn't use this.
    eye: EyeCurve,
    /// How far the latest clean flutter was from where the curve said, before it was added.
    last_eye_error: Option<f32>,
}

impl Default for Detector {
    fn default() -> Self {
        Self::new()
    }
}

impl Detector {
    pub fn new() -> Self {
        Self {
            points: (0..IDLE_LOOP_FRAMES).map(|_| LoopPoint::default()).collect(),
            pending: VecDeque::new(),
            busy: vec![0.0; WIDTH * HEIGHT],
            mask: vec![0; WIDTH * HEIGHT],
            blobs: BlobFinder::new(),
            centers: Vec::new(),
            tracks: Vec::new(),
            game_frame: -1,
            bad_reference: false,
            health: Health::default(),
            eye: EyeCurve::default(),
            last_eye_error: None,
        }
    }

    /// The blobs on the latest frame that could be the eye.
    pub fn blobs(&self) -> impl Iterator<Item = Rect> + '_ {
        self.centers.iter().map(|blob| blob.rect)
    }

    /// Where the eye should be on the latest frame: a fixed-size box on the eye's path around the
    /// loop. `None` until enough clean flutters have shown it.
    pub fn expected_eye(&self) -> Option<Rect> {
        let (x, y) = self.eye.at(self.game_frame.max(0) as usize % IDLE_LOOP_FRAMES)?;
        let (x, y) = (x.clamp(0.0, (WIDTH - 1) as f32) as u16, y.clamp(0.0, (HEIGHT - 1) as f32) as u16);
        Some(Rect {
            left: x.saturating_sub(EYE_BOX_X),
            top: y.saturating_sub(EYE_BOX_Y),
            right: (x + EYE_BOX_X).min(WIDTH as u16 - 1),
            bottom: (y + EYE_BOX_Y).min(HEIGHT as u16 - 1),
        })
    }

    /// How far the latest clean flutter was from where the curve expected it, in pixels.
    pub fn last_eye_error(&self) -> Option<f32> {
        self.last_eye_error
    }

    /// What happened since the last call.
    pub fn take_health(&mut self) -> Health {
        let tracks = self.tracks.len();
        Health { tracks, ..std::mem::take(&mut self.health) }
    }

    /// `frames` game frames went by unseen: the feed skipped them. Counting them keeps the point of
    /// the loop and every flutter's timing right.
    pub fn skip(&mut self, frames: i64) {
        self.game_frame += frames;
    }

    /// The next game frame's picture. Returns what it showed, and the flutter that was just
    /// recognized, a few game frames after it ended.
    pub fn push(&mut self, picture: &[u8]) -> (FrameInfo, Option<Flutter>) {
        self.game_frame += 1;
        let compared = self.compare(self.game_frame as usize % IDLE_LOOP_FRAMES, picture);
        let blobs = self.centers.len();
        self.health.game_frames += 1;
        self.health.bad_reference += u32::from(self.bad_reference);
        self.health.with_blobs += u32::from(blobs > 0);
        let flutter = self.track();
        if let Some(flutter) = flutter {
            let range = flutter.start..flutter.start + flutter_len(flutter.count);
            self.pending.retain(|pending| !range.contains(&pending.game_frame));
        }
        self.pending.push_back(Pending { game_frame: self.game_frame, frame: Arc::from(picture) });
        while self.pending.len() > SETTLE_FRAMES {
            let oldest = self.pending.pop_front().expect("checked the length");
            self.points[oldest.game_frame as usize % IDLE_LOOP_FRAMES].add(oldest.frame);
        }
        (FrameInfo { game_frame: self.game_frame, compared, blobs, bad_reference: self.bad_reference }, flutter)
    }

    fn compare(&mut self, point: usize, picture: &[u8]) -> bool {
        let Self { points, busy, mask, blobs, centers, bad_reference, .. } = self;
        centers.clear();
        let Some(reference) = points[point].reference() else { return false };
        for (((out, &pixel), &reference), busy) in mask.iter_mut().zip(picture).zip(reference).zip(busy.iter_mut()) {
            let changed = (i16::from(pixel) - i16::from(reference)).abs() > CHANGE;
            *busy += (f32::from(u8::from(changed)) - *busy) * BUSY_RATE;
            *out = u8::from(changed && *busy < BUSY_SHARE);
        }
        blobs.find(mask, centers);
        *bad_reference = centers.len() > MAX_BLOBS;
        if *bad_reference {
            centers.clear();
        }
        true
    }

    /// Adds this frame's blobs to the tracks, and judges the tracks that ended.
    fn track(&mut self) -> Option<Flutter> {
        let frame = self.game_frame;
        for &Blob { center, rect } in &self.centers {
            let nearest = self
                .tracks
                .iter_mut()
                .filter(|track| track.last_frame() < frame)
                .filter_map(|track| {
                    let gap = frame - track.last_frame();
                    let last = track.last_center();
                    let distance = (center.0 - last.0).hypot(center.1 - last.1);
                    (distance <= TRACK_RADIUS + TRACK_RADIUS_PER_FRAME * gap as f32).then_some((distance, track))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0));
            match nearest {
                Some((_, track)) => {
                    track.centers.push(center);
                    track.rects.push(rect);
                    track.frames.push(frame);
                }
                None => self.tracks.push(Track { frames: vec![frame], centers: vec![center], rects: vec![rect] }),
            }
        }
        let mut best: Option<Flutter> = None;
        let mut rejected = 0;
        let (eye, last_eye_error) = (&mut self.eye, &mut self.last_eye_error);
        self.tracks.retain(|track| {
            if track.frames.len() > TRACK_MAX_FRAMES {
                return false;
            }
            if frame - track.last_frame() <= TRACK_MAX_GAP {
                return true;
            }
            match track.fit() {
                Some(flutter) if best.is_none_or(|b| flutter.fit > b.fit) => {
                    best = Some(flutter);
                    if flutter.is_clean() {
                        let positions: Vec<_> = track
                            .frames
                            .iter()
                            .zip(&track.centers)
                            .map(|(&seen, center)| (seen as usize % IDLE_LOOP_FRAMES, center.0, center.1))
                            .collect();
                        let errors = positions.iter().filter_map(|&(point, x, y)| {
                            eye.at(point).map(|expected| (expected.0 - x).hypot(expected.1 - y))
                        });
                        *last_eye_error = errors
                            .clone()
                            .max_by(f32::total_cmp)
                            .map(|_| errors.clone().sum::<f32>() / errors.count() as f32);
                        eye.add(positions);
                    }
                }
                Some(_) => {}
                None => rejected += 1,
            }
            false
        });
        self.health.rejected_tracks += rejected;
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Blinks;

    /// A scene with what the real footage has: a head that moves with the loop, an eye on it, and
    /// a big patch of drifting sky that never stops changing.
    fn scene(game_frame: i64, eye_state: u8, sky: bool) -> Vec<u8> {
        let mut picture = vec![40u8; WIDTH * HEIGHT];
        let phase = (game_frame as usize % IDLE_LOOP_FRAMES) as f32 / IDLE_LOOP_FRAMES as f32;
        let angle = phase * std::f32::consts::TAU;
        let head = ((200.0 + 40.0 * angle.sin()) as usize, (200.0 + 30.0 * angle.cos()) as usize);
        for y in head.1 - 40..head.1 + 40 {
            picture[y * WIDTH + head.0 - 40..y * WIDTH + head.0 + 40].fill(120);
        }
        // Open is a dark eye, half closed covers its top half, closed covers all of it
        let covered = [0, 5, 10][usize::from(eye_state)];
        for y in head.1 - 5 + covered..head.1 + 5 {
            picture[y * WIDTH + head.0 - 8..y * WIDTH + head.0 + 8].fill(30);
        }
        if sky {
            let shift = (game_frame / 3) as usize % 200;
            for y in 10..30 {
                picture[y * WIDTH + 300 + shift..y * WIDTH + 320 + shift].fill(200);
            }
        }
        picture
    }

    /// Flutters from the model, and what the eye shows on each game frame.
    fn eye_track(seed: u32, frames: i64) -> (Vec<(i64, u8)>, Vec<u8>) {
        let mut blinks = Blinks::new(seed, 40);
        let mut states = vec![0u8; frames as usize];
        let mut flutters = Vec::new();
        loop {
            let flutter = blinks.next_flutter();
            if flutter.start + flutter_len(flutter.count) >= frames {
                return (flutters, states);
            }
            flutters.push((flutter.start, flutter.count));
            for blink in 0..i64::from(flutter.count) {
                states[(flutter.start + 3 * blink) as usize] = 1;
                states[(flutter.start + 3 * blink + 1) as usize] = 2;
            }
        }
    }

    /// A skipped game frame must not move the loop, or every reference would be wrong after it.
    #[test]
    fn skipped_frames_keep_the_flutters_in_place() {
        let (flutters, states) = eye_track(777_777, 1500);
        let quiet = |frame: usize| states[frame.saturating_sub(4)..frame + 4].iter().all(|&state| state == 0);
        let skipped: Vec<usize> = (300..1500 - 4).step_by(211).filter(|&frame| quiet(frame)).collect();
        assert!(skipped.len() >= 3);
        let mut detector = Detector::new();
        let mut read = Vec::new();
        let mut frame = 0;
        while frame < states.len() {
            if skipped.contains(&frame) {
                detector.skip(1);
                frame += 1;
                continue;
            }
            let (_, flutter) = detector.push(&scene(frame as i64, states[frame], false));
            read.extend(flutter.map(|f| (f.start, f.count)));
            frame += 1;
        }
        assert_eq!(after_warmup(&read), after_warmup(&flutters));
    }

    /// The box has to end up on the eye, which moves around the picture with the loop, and it has
    /// to get there without jumping.
    #[test]
    fn the_expected_eye_follows_the_loop_smoothly() {
        let (_, states) = eye_track(777_777, 1500);
        let mut detector = Detector::new();
        let mut checked = 0;
        for (frame, &state) in states.iter().enumerate() {
            detector.push(&scene(frame as i64, state, true));
            let Some(eye) = detector.expected_eye() else { continue };
            if frame < 900 {
                continue;
            }
            let angle = (frame % IDLE_LOOP_FRAMES) as f32 / IDLE_LOOP_FRAMES as f32 * std::f32::consts::TAU;
            let head = (200.0 + 40.0 * angle.sin(), 200.0 + 30.0 * angle.cos());
            let (x, y) = eye.center();
            assert!((f32::from(x) - head.0).hypot(f32::from(y) - head.1) < 8.0, "frame {frame}");
            checked += 1;
        }
        assert!(checked > 500, "{checked}");
    }

    fn read(states: &[u8], sky: bool) -> Vec<(i64, u8)> {
        let mut detector = Detector::new();
        let mut read = Vec::new();
        for (frame, &state) in states.iter().enumerate() {
            let (_, flutter) = detector.push(&scene(frame as i64, state, sky));
            if let Some(flutter) = flutter {
                read.push((flutter.start, flutter.count));
            }
        }
        read
    }

    fn after_warmup(flutters: &[(i64, u8)]) -> Vec<(i64, u8)> {
        let warm = (IDLE_LOOP_FRAMES * (REFERENCE_MIN + 1)) as i64;
        flutters.iter().copied().filter(|&(start, _)| start >= warm).collect()
    }

    #[test]
    fn reads_every_flutter_exactly_on_a_moving_head() {
        let (flutters, states) = eye_track(777_777, 1500);
        assert!(flutters.len() > 20);
        assert_eq!(after_warmup(&read(&states, false)), after_warmup(&flutters));
    }

    /// The sky's blob is bigger than the eye and always there. It must not hide the eye, and it
    /// must not read as a flutter.
    #[test]
    fn a_big_blob_that_stays_on_changes_nothing() {
        let (flutters, states) = eye_track(777_777, 1500);
        assert_eq!(after_warmup(&read(&states, true)), after_warmup(&flutters));
    }

    /// With the eye never closing there's nothing to read, even though the head keeps moving.
    #[test]
    fn no_flutters_without_blinks() {
        let states = vec![0u8; 800];
        assert_eq!(read(&states, true), []);
    }
}
