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

/// A blob with this many others within this many pixels is part of something large that's changing,
/// like a drifting sky or a reference from before the scene settled. The eye stands alone, and one
/// busy part of the picture mustn't hide it.
const CROWD_BLOBS: usize = 3;
const CROWD_RADIUS: f32 = 30.0;
/// More blobs than this dropped means most of the frame differs: the reference is bad.
const BAD_REFERENCE_DROPPED: usize = 100;

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
/// Following the eye from one point of the loop to the next in the reference frames: the patch
/// compared, half its width and height, and how far it can have moved.
const FOLLOW_PATCH_X: i32 = 16;
const FOLLOW_PATCH_Y: i32 = 11;
const FOLLOW_SEARCH: i32 = 7;
/// A flutter's real position further than this from what was followed corrects it.
const FOLLOW_CORRECT: f32 = 3.0;
/// Once this many positions from clean flutters agree on the eye's path, about 4 flutters, only blobs
/// near that path count. The rest of the picture can't hold the eye.
const REGION_MIN_SAMPLES: usize = 24;
/// How far around the path still counts, in pixels: his other eye is about 40 away.
const REGION_MARGIN: u16 = 50;
/// Game frames without a flutter inside before the region is forgotten and learned again. Flutters
/// come every 3 seconds or so, so this is several missed in a row. It keeps a wrong region from
/// hiding the eye for good.
const REGION_MAX_SILENCE: i64 = 300;
/// Game frames kept for reading again once the eye's region is known: 30 seconds, about 150 MB.
const ARCHIVE_FRAMES: usize = 600;
/// Tracks in progress can still end in a flutter this many game frames back, so what's newer stays
/// as first read.
const REREAD_KEEP_FRAMES: i64 = 20;
/// The shown eye box, half its width and height.
const EYE_BOX_X: u16 = 10;
const EYE_BOX_Y: u16 = 7;
/// How much of a flutter's pattern has to match, including the quiet frames around it.
const MIN_FIT: f32 = 0.84;
/// The quiet frames checked on each side of a flutter.
const FLANK: i64 = 2;
/// How far past a track's first frame a flutter's start may be, for a stray frame before it.
const START_SLACK: i64 = 3;

/// The first read of the frames up to `through`, redone now that more is known.
#[derive(Clone, Debug, PartialEq)]
pub struct Reread {
    pub through: i64,
    pub flutters: Vec<Flutter>,
}

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
    /// Frames that differed so much from the reference that it must be bad.
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
    /// So much differed from the reference that it must be bad.
    pub bad_reference: bool,
    /// The reference became ready on this frame, so the flutters with it were found looking back.
    pub finished_learning: bool,
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

    /// Takes a frame out again, as long as enough remain to compare with.
    fn remove(&mut self, frame: &Arc<[u8]>) {
        if self.frames.len() > REFERENCE_MIN {
            self.frames.retain(|kept| !Arc::ptr_eq(kept, frame));
            self.reference = None;
        }
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

/// Where the patch around `position` in `from` is in `to`, nearby.
fn best_match(from: &[u8], to: &[u8], position: (f32, f32)) -> (f32, f32) {
    let (center_x, center_y) = (position.0.round() as i32, position.1.round() as i32);
    let margin_x = FOLLOW_PATCH_X + FOLLOW_SEARCH;
    let margin_y = FOLLOW_PATCH_Y + FOLLOW_SEARCH;
    let center_x = center_x.clamp(margin_x, WIDTH as i32 - margin_x - 1);
    let center_y = center_y.clamp(margin_y, HEIGHT as i32 - margin_y - 1);
    let mut best = (u32::MAX, 0, 0);
    for dy in -FOLLOW_SEARCH..=FOLLOW_SEARCH {
        for dx in -FOLLOW_SEARCH..=FOLLOW_SEARCH {
            let mut difference = 0u32;
            for y in -FOLLOW_PATCH_Y..=FOLLOW_PATCH_Y {
                let a = ((center_y + y) as usize) * WIDTH + (center_x - FOLLOW_PATCH_X) as usize;
                let b = ((center_y + y + dy) as usize) * WIDTH + (center_x + dx - FOLLOW_PATCH_X) as usize;
                let len = (2 * FOLLOW_PATCH_X + 1) as usize;
                for (&p, &q) in from[a..a + len].iter().zip(&to[b..b + len]) {
                    difference += u32::from(p.abs_diff(q));
                }
            }
            // Ties go to the smallest move
            let moved = (dx * dx + dy * dy) as u32;
            if (difference, moved) < (best.0, (best.1 * best.1 + best.2 * best.2) as u32) {
                best = (difference, dx, dy);
            }
        }
    }
    (position.0 + best.1 as f32, position.1 + best.2 as f32)
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
    /// The frames so far, until the eye's region is known and they've been read again with it.
    archive: Option<VecDeque<(i64, Arc<[u8]>)>>,
    reread: Option<Reread>,
    /// The frames so far, until the reference is ready, to go through once it is. Flutters don't
    /// wait for the reference, and every one counts.
    learning: Option<Vec<(i64, Arc<[u8]>)>>,
    game_frame: i64,
    bad_reference: bool,
    health: Health,
    /// Where the eye is around the loop. Only for showing it: reading flutters doesn't use this.
    eye: EyeCurve,
    /// How far the latest clean flutter was from where the eye was shown, before it was added.
    last_eye_error: Option<f32>,
    /// Where the eye is at each point of the loop, followed through the reference frames.
    eye_followed: Vec<Option<(f32, f32)>>,
    /// Clean flutters came in since it was last followed.
    follow_due: bool,
    /// How far the worst point moved the last time it was followed.
    last_eye_snap: Option<f32>,
    /// The only part of the picture that can hold the eye, once that's known.
    region: Option<Rect>,
    reread_due: bool,
    /// The latest game frame a flutter was recognized on, for giving up on a region.
    last_flutter_frame: i64,
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
            archive: Some(VecDeque::new()),
            reread: None,
            learning: Some(Vec::new()),
            game_frame: -1,
            bad_reference: false,
            health: Health::default(),
            eye: EyeCurve::default(),
            last_eye_error: None,
            eye_followed: vec![None; IDLE_LOOP_FRAMES],
            follow_due: false,
            last_eye_snap: None,
            region: None,
            last_flutter_frame: 0,
            reread_due: false,
        }
    }

    /// The blobs on the latest frame that could be the eye.
    pub fn blobs(&self) -> impl Iterator<Item = Rect> + '_ {
        self.centers.iter().map(|blob| blob.rect)
    }

    /// Where the eye should be on the latest frame: a fixed-size box on the eye's path around the
    /// loop. `None` until enough clean flutters have shown it.
    pub fn expected_eye(&self) -> Option<Rect> {
        let point = self.game_frame.max(0) as usize % IDLE_LOOP_FRAMES;
        let (x, y) = self.eye_followed[point].or_else(|| self.eye.at(point))?;
        let (x, y) = (x.clamp(0.0, (WIDTH - 1) as f32) as u16, y.clamp(0.0, (HEIGHT - 1) as f32) as u16);
        Some(Rect {
            left: x.saturating_sub(EYE_BOX_X),
            top: y.saturating_sub(EYE_BOX_Y),
            right: (x + EYE_BOX_X).min(WIDTH as u16 - 1),
            bottom: (y + EYE_BOX_Y).min(HEIGHT as u16 - 1),
        })
    }

    /// The bounding box of the eye's path around the loop, with room around it.
    fn eye_region(&self) -> Option<Rect> {
        if self.eye.samples() < REGION_MIN_SAMPLES {
            return None;
        }
        let (mut left, mut top, mut right, mut bottom) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for &(x, y) in self.eye_followed.iter().flatten() {
            left = left.min(x);
            right = right.max(x);
            top = top.min(y);
            bottom = bottom.max(y);
        }
        (left <= right).then(|| Rect {
            left: (left.max(0.0) as u16).saturating_sub(REGION_MARGIN),
            top: (top.max(0.0) as u16).saturating_sub(REGION_MARGIN),
            right: (right.max(0.0) as u16 + REGION_MARGIN).min(WIDTH as u16 - 1),
            bottom: (bottom.max(0.0) as u16 + REGION_MARGIN).min(HEIGHT as u16 - 1),
        })
    }

    /// The part of the picture blobs count in, if it's known.
    pub fn region(&self) -> Option<Rect> {
        self.region
    }

    /// How far the worst point of the loop moved the last time the eye was followed, in pixels.
    pub fn last_eye_snap(&self) -> Option<f32> {
        self.last_eye_snap
    }

    /// Follows the eye around the loop through the reference frames, which show the open eye at
    /// every point, also where flutters rarely land because something hides the eye then. It starts
    /// from where flutters really saw the eye, and they correct it whenever they disagree.
    fn follow_eye(&mut self) {
        let seen = self.eye.seen_at_points();
        let Some(start) = (0..IDLE_LOOP_FRAMES).find(|&point| seen[point].is_some()) else { return };
        for point in &mut self.points {
            point.reference();
        }
        if self.points.iter().any(|point| point.reference.is_none()) {
            return;
        }
        let mut followed = vec![None; IDLE_LOOP_FRAMES];
        let mut position = seen[start].expect("found above");
        followed[start] = Some(position);
        for step in 1..IDLE_LOOP_FRAMES {
            let (from, to) = ((start + step - 1) % IDLE_LOOP_FRAMES, (start + step) % IDLE_LOOP_FRAMES);
            let found = best_match(
                self.points[from].reference.as_deref().expect("checked above"),
                self.points[to].reference.as_deref().expect("checked above"),
                position,
            );
            position = match seen[to] {
                Some(real) if (real.0 - found.0).hypot(real.1 - found.1) > FOLLOW_CORRECT => real,
                _ => found,
            };
            followed[to] = Some(position);
        }
        self.last_eye_snap = self
            .eye_followed
            .iter()
            .zip(&followed)
            .filter_map(|(old, new)| Some((*old)?.0 - (*new)?.0).zip(Some((*old)?.1 - (*new)?.1)))
            .map(|(x, y): (f32, f32)| x.hypot(y))
            .max_by(f32::total_cmp);
        self.eye_followed = followed;
        self.region = self.eye_region();
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

    /// The next game frame's picture. Returns what it showed, and the flutters just recognized, a
    /// few game frames after they ended. Those from while the reference was being built all come at
    /// once when it's ready.
    pub fn push(&mut self, picture: &[u8]) -> (FrameInfo, Vec<Flutter>) {
        self.game_frame += 1;
        let frame: Arc<[u8]> = Arc::from(picture);
        if let Some(archive) = &mut self.archive {
            archive.push_back((self.game_frame, frame.clone()));
            if archive.len() > ARCHIVE_FRAMES {
                archive.pop_front();
            }
        }
        if let Some(learning) = &mut self.learning {
            learning.push((self.game_frame, frame.clone()));
            self.points[self.game_frame as usize % IDLE_LOOP_FRAMES].add(frame);
            let info = FrameInfo {
                game_frame: self.game_frame,
                compared: false,
                blobs: 0,
                bad_reference: false,
                finished_learning: false,
            };
            if self.points.iter().any(|point| point.frames.len() < REFERENCE_MIN) {
                return (info, Vec::new());
            }
            return (FrameInfo { finished_learning: true, ..info }, self.read_learned());
        }
        let (compared, flutter) = self.read(picture);
        let blobs = self.centers.len();
        // Only a clean flutter keeps its frames out of the reference. Most others aren't flutters, and
        // keeping frames out on their word can stop a bad reference from ever being replaced
        if let Some(flutter) = flutter.filter(Flutter::is_clean) {
            let range = flutter.start..flutter.start + flutter_len(flutter.count);
            self.pending.retain(|pending| !range.contains(&pending.game_frame));
        }
        self.pending.push_back(Pending { game_frame: self.game_frame, frame });
        while self.pending.len() > SETTLE_FRAMES {
            let oldest = self.pending.pop_front().expect("checked the length");
            self.points[oldest.game_frame as usize % IDLE_LOOP_FRAMES].add(oldest.frame);
        }
        if std::mem::take(&mut self.reread_due) {
            self.read_archive();
        }
        let info = FrameInfo {
            game_frame: self.game_frame,
            compared,
            blobs,
            bad_reference: self.bad_reference,
            finished_learning: false,
        };
        (info, flutter.into_iter().collect())
    }

    /// A redone first read, once there is one. It's only there once.
    pub fn take_reread(&mut self) -> Option<Reread> {
        self.reread.take()
    }

    /// Reads the kept frames again, now that the eye's region and better references are known. This
    /// changes nothing else: the tracks in progress and what's known about the eye stay as they are.
    fn read_archive(&mut self) {
        let Some(archive) = self.archive.take() else { return };
        let latest = self.game_frame;
        let tracks = std::mem::take(&mut self.tracks);
        let eye = std::mem::take(&mut self.eye);
        let error = self.last_eye_error;
        let health = self.health;
        let last_flutter = self.last_flutter_frame;
        let busy = self.busy.clone();
        let through = latest - REREAD_KEEP_FRAMES;
        let mut flutters = Vec::new();
        for (game_frame, frame) in archive.iter().filter(|(game_frame, _)| *game_frame <= through) {
            self.game_frame = *game_frame;
            self.compare(*game_frame as usize % IDLE_LOOP_FRAMES, frame);
            flutters.extend(self.track());
        }
        // What's still open at the end can't be judged yet
        self.tracks = tracks;
        self.eye = eye;
        self.last_eye_error = error;
        self.health = health;
        self.last_flutter_frame = last_flutter;
        self.busy = busy;
        self.follow_due = false;
        self.game_frame = latest;
        let first = archive.front().map_or(0, |(game_frame, _)| *game_frame);
        flutters.retain(|flutter| flutter.start >= first);
        self.reread = Some(Reread { through: through - flutter_len(4) - TRACK_MAX_GAP, flutters });
    }

    /// Reads the frames from while the reference was being built, now that it's ready. Frames that
    /// turn out to be part of a flutter leave the reference.
    fn read_learned(&mut self) -> Vec<Flutter> {
        let learned = self.learning.take().unwrap_or_default();
        let latest = self.game_frame;
        let mut flutters = Vec::new();
        for (game_frame, frame) in &learned {
            self.game_frame = *game_frame;
            flutters.extend(self.read(frame).1);
        }
        self.game_frame = latest;
        for flutter in flutters.iter().filter(|flutter| flutter.is_clean()) {
            let range = flutter.start..flutter.start + flutter_len(flutter.count);
            for (game_frame, frame) in &learned {
                if range.contains(game_frame) {
                    self.points[*game_frame as usize % IDLE_LOOP_FRAMES].remove(frame);
                }
            }
        }
        flutters
    }

    /// Compares one frame and follows what's on it. Whether it could be compared, and the flutter
    /// just recognized, if any.
    fn read(&mut self, picture: &[u8]) -> (bool, Option<Flutter>) {
        let compared = self.compare(self.game_frame as usize % IDLE_LOOP_FRAMES, picture);
        self.health.game_frames += 1;
        self.health.bad_reference += u32::from(self.bad_reference);
        self.health.with_blobs += u32::from(!self.centers.is_empty());
        let flutter = self.track();
        if flutter.is_some() {
            self.last_flutter_frame = self.game_frame;
        }
        if std::mem::take(&mut self.follow_due) {
            self.follow_eye();
            if self.region.is_some() {
                self.reread_due = true;
            }
        }
        (compared, flutter)
    }

    fn compare(&mut self, point: usize, picture: &[u8]) -> bool {
        if self.region.is_some() && self.game_frame - self.last_flutter_frame > REGION_MAX_SILENCE {
            self.region = None;
            self.eye = EyeCurve::default();
            self.eye_followed.fill(None);
        }
        let Self { points, busy, mask, blobs, centers, bad_reference, region, .. } = self;
        centers.clear();
        let Some(reference) = points[point].reference() else { return false };
        for (((out, &pixel), &reference), busy) in mask.iter_mut().zip(picture).zip(reference).zip(busy.iter_mut()) {
            let changed = (i16::from(pixel) - i16::from(reference)).abs() > CHANGE;
            *busy += (f32::from(u8::from(changed)) - *busy) * BUSY_RATE;
            *out = u8::from(changed && *busy < BUSY_SHARE);
        }
        blobs.find(mask, centers);
        if let Some(region) = region {
            centers.retain(|blob| blob.rect.overlaps(region));
        }
        let crowded: Vec<bool> = centers
            .iter()
            .map(|blob| {
                let near = centers
                    .iter()
                    .filter(|other| {
                        (other.center.0 - blob.center.0).hypot(other.center.1 - blob.center.1) <= CROWD_RADIUS
                    })
                    .count();
                near > CROWD_BLOBS
            })
            .collect();
        let dropped = crowded.iter().filter(|&&crowded| crowded).count();
        *bad_reference = dropped > BAD_REFERENCE_DROPPED;
        let mut keep = crowded.iter().map(|&crowded| !crowded);
        centers.retain(|_| keep.next().unwrap_or(false));
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
        let (eye, last_eye_error, table, follow_due) =
            (&mut self.eye, &mut self.last_eye_error, &self.eye_followed, &mut self.follow_due);
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
                            table[point]
                                .or_else(|| eye.at(point))
                                .map(|expected| (expected.0 - x).hypot(expected.1 - y))
                        });
                        *last_eye_error = errors
                            .clone()
                            .max_by(f32::total_cmp)
                            .map(|_| errors.clone().sum::<f32>() / errors.count() as f32);
                        eye.add(positions);
                        *follow_due = true;
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
            let (_, flutters) = detector.push(&scene(frame as i64, states[frame], false));
            read.extend(flutters.iter().map(|f| (f.start, f.count)));
            frame += 1;
        }
        assert_eq!(after_warmup(&read), after_warmup(&flutters));
    }

    /// Following is what fills the stretches of the loop no flutter has reached, so it has to find
    /// a feature that moved, and not move when nothing did.
    #[test]
    fn following_finds_what_moved() {
        let picture = |x: usize, y: usize| {
            let mut out = vec![50u8; WIDTH * HEIGHT];
            for row in y..y + 10 {
                out[row * WIDTH + x..row * WIDTH + x + 16].fill(200);
            }
            out
        };
        let from = picture(200, 150);
        assert_eq!(best_match(&from, &picture(205, 147), (208.0, 155.0)), (213.0, 152.0));
        assert_eq!(best_match(&from, &from, (208.0, 155.0)), (208.0, 155.0));
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
            let (_, flutters) = detector.push(&scene(frame as i64, state, sky));
            read.extend(flutters.iter().map(|flutter| (flutter.start, flutter.count)));
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

    /// The live log says how many flutters were found looking back, so it has to know which those are.
    #[test]
    fn finishing_learning_is_reported_once_with_its_flutters() {
        let (_, states) = eye_track(777_777, 800);
        let mut detector = Detector::new();
        let mut finished = Vec::new();
        for (frame, &state) in states.iter().enumerate() {
            let (info, flutters) = detector.push(&scene(frame as i64, state, false));
            if info.finished_learning {
                finished.push(flutters.len());
            } else if frame < IDLE_LOOP_FRAMES * REFERENCE_MIN - 1 {
                assert_eq!(flutters, [], "nothing can be read before the reference exists");
            }
        }
        assert_eq!(finished.len(), 1);
        assert!(finished[0] >= 2, "{finished:?}");
    }

    /// The whole point of reading again: something that read as a flutter early on, far from where
    /// the eye turned out to be, doesn't survive it, and the real ones do.
    #[test]
    fn reading_again_drops_early_strays_and_keeps_real_flutters() {
        let (flutters, states) = eye_track(777_777, 1600);
        let mut detector = Detector::new();
        let mut reread = None;
        for (frame, &state) in states.iter().enumerate() {
            let mut picture = scene(frame as i64, state, false);
            let phase = (frame + 70) % 170;
            if phase < 8 && phase % 3 != 2 {
                for row in 390..405 {
                    picture[row * WIDTH + 520..row * WIDTH + 535].fill(200);
                }
            }
            detector.push(&picture);
            reread = reread.or(detector.take_reread());
        }
        let reread = reread.expect("it never read again");
        let read: Vec<_> = reread.flutters.iter().map(|f| (f.start, f.count)).collect();
        let real: Vec<_> = flutters.iter().copied().filter(|f| f.0 <= reread.through).collect();
        assert!(real.len() >= 2, "the control: {real:?}");
        let found = real.iter().filter(|f| read.contains(f)).count();
        assert!(found * 10 >= real.len() * 8, "{found} of {}: {read:?}", real.len());
        let strays = read.iter().filter(|f| !real.contains(f) && f.0 <= reread.through).count();
        assert_eq!(strays, 0, "{read:?}");
    }

    /// Flutters from before the reference is ready count too, or the read starts seconds late.
    #[test]
    fn flutters_from_while_learning_are_read_too() {
        let (flutters, states) = eye_track(777_777, 800);
        let ready = (IDLE_LOOP_FRAMES * REFERENCE_MIN) as i64;
        let early: Vec<_> = flutters.iter().copied().filter(|&(start, _)| start < ready - 20).collect();
        assert!(early.len() >= 2, "the control: there have to be some {early:?}");
        let read = read(&states, false);
        for flutter in early {
            assert!(read.contains(&flutter), "{flutter:?} not in {read:?}");
        }
    }

    /// The sky's blob is bigger than the eye and always there. It must not hide the eye, and it
    /// must not read as a flutter.
    #[test]
    fn a_big_blob_that_stays_on_changes_nothing() {
        let (flutters, states) = eye_track(777_777, 1500);
        assert_eq!(after_warmup(&read(&states, true)), after_warmup(&flutters));
    }

    /// Something blinking just like an eye, somewhere the eye never is, stops counting once the
    /// eye's place is known. The real flutters have to keep coming.
    #[test]
    fn a_flutter_outside_the_eyes_region_is_ignored() {
        let (flutters, states) = eye_track(777_777, 2500);
        let mut detector = Detector::new();
        let mut read = Vec::new();
        for (frame, &state) in states.iter().enumerate() {
            let mut picture = scene(frame as i64, state, false);
            // A perfect 3 blink flutter every 170 frames, far from the head
            let phase = frame % 170;
            if phase < 8 && phase % 3 != 2 {
                for row in 390..405 {
                    picture[row * WIDTH + 520..row * WIDTH + 535].fill(200);
                }
            }
            let (_, found) = detector.push(&picture);
            read.extend(found.iter().map(|f| (f.start, f.count)));
        }
        assert!(detector.region().is_some(), "the control: it has to have learned a region");
        let late = |list: &[(i64, u8)]| -> Vec<(i64, u8)> { list.iter().copied().filter(|f| f.0 > 1200).collect() };
        let real = late(&flutters);
        let strays: Vec<_> = late(&read).into_iter().filter(|f| !real.contains(f)).collect();
        assert_eq!(strays, [], "read {:?}", late(&read));
        let found = real.iter().filter(|f| late(&read).contains(f)).count();
        assert!(found * 10 >= real.len() * 9, "{found} of {}", real.len());
    }

    /// A wrong region can't be allowed to hide the eye for good.
    #[test]
    fn a_region_with_nothing_in_it_is_forgotten() {
        let (_, states) = eye_track(777_777, 1200);
        let mut detector = Detector::new();
        for (frame, &state) in states.iter().enumerate() {
            detector.push(&scene(frame as i64, state, false));
        }
        assert!(detector.region().is_some(), "the control: it has to have learned a region");
        for frame in 1200..1200 + REGION_MAX_SILENCE as usize + 20 {
            detector.push(&scene(frame as i64, 0, false));
        }
        assert_eq!(detector.region(), None);
    }

    /// A busy patch full of small changes, like a drifting sky, must not hide the eye elsewhere.
    #[test]
    fn a_crowd_of_small_changes_does_not_hide_the_eye() {
        let (flutters, states) = eye_track(777_777, 1500);
        let mut detector = Detector::new();
        let mut read = Vec::new();
        for (frame, &state) in states.iter().enumerate() {
            let mut picture = scene(frame as i64, state, false);
            // Dozens of blobs that change on their own schedule, far from the head
            for i in 0..40 {
                let (x, y) = (360 + (i % 8) * 25, 20 + (i / 8) * 25);
                if (frame + i * 7) % 5 == 0 {
                    for row in y..y + 6 {
                        picture[row * WIDTH + x..row * WIDTH + x + 6].fill(160);
                    }
                }
            }
            let (_, found) = detector.push(&picture);
            read.extend(found.iter().map(|f| (f.start, f.count)));
        }
        let read = after_warmup(&read);
        for flutter in after_warmup(&flutters) {
            assert!(read.contains(&flutter), "{flutter:?} not read");
        }
    }

    /// With the eye never closing there's nothing to read, even though the head keeps moving.
    #[test]
    fn no_flutters_without_blinks() {
        let states = vec![0u8; 800];
        assert_eq!(read(&states, true), []);
    }
}
