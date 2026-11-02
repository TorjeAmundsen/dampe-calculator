//! Flutter presses logged next to what the video read, to measure how well a person can do by hand:
//! timing spread, late or early, wrong counts and missed flutters. `dampe live --log-presses` writes
//! the file, and `dampe press-stats` compares its presses to the flutters of the state that locked.
//! These presses only go in the file, and the read doesn't use them.

use std::fs::File;
use std::io::Write;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use dampe_core::manual::FLIP_SECONDS;
use dampe_core::reading::{CONFIRMATIONS, Candidate, Reading};
use dampe_core::rng::flutter_len;
use dampe_core::session::Event;

/// Game frames are 50 ms, near enough for reporting.
const GAME_FRAME_MS: f64 = 50.0;
/// A press can belong to a flutter from this far around the flutter's end, in game frames. Flutters
/// are at least 30 apart.
const PAIRED_FRAMES: std::ops::RangeInclusive<f64> = -10.0..=25.0;
/// Where a press is expected after a flutter's end, for choosing between flutters.
const EXPECTED_FRAMES: f64 = 5.0;

/// The file being written, and the presses coming in from the window.
pub struct PressLog {
    file: File,
    presses: Receiver<(u8, Instant)>,
    /// The likeliest reading last written, and its predictions.
    reading: Option<(Candidate, u32)>,
    /// When this read's last flutter was pressed, not counting flips, and its count as it stands.
    last_press: Option<(f64, u8)>,
}

impl PressLog {
    /// Adds to `path`, so a mistyped command can't lose earlier sessions.
    pub fn open(path: &str, presses: Receiver<(u8, Instant)>, timing: &str) -> Result<Self, String> {
        let file = File::options()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| format!("couldn't open '{path}' for the press log: {e}"))?;
        let mut log = Self { file, presses, reading: None, last_press: None };
        log.line(format_args!("run {timing}"));
        Ok(log)
    }

    fn line(&mut self, text: impl std::fmt::Display) {
        let _ = writeln!(self.file, "{text}");
    }

    /// Whatever came before this can't be compared to what comes after.
    pub fn read_started(&mut self) {
        self.reading = None;
        self.last_press = None;
        self.line("read");
    }

    /// Writes the presses so far, with `frame_at` placing a time in game frames while reading.
    /// Returns what to tell the user about each.
    pub fn take_presses(&mut self, started: Instant, frame_at: impl Fn(f64) -> Option<f64>) -> Vec<String> {
        let presses: Vec<_> = self.presses.try_iter().collect();
        presses
            .into_iter()
            .map(|(count, at)| {
                let seconds = at.saturating_duration_since(started).as_secs_f64();
                match frame_at(seconds) {
                    Some(frame) => self.line(format_args!("press {count} {seconds:.3} {frame:.2}")),
                    None => self.line(format_args!("press {count} {seconds:.3} -")),
                }
                if let Some((last, last_count)) = self.last_press
                    && seconds - last < FLIP_SECONDS
                {
                    self.last_press = Some((last, 7 - last_count));
                    return format!("Flipped the last press to {}.", 7 - last_count);
                }
                self.last_press = Some((seconds, count));
                format!("Pressed {count}.")
            })
            .collect()
    }

    /// Notes the likeliest reading, when it or its predictions changed.
    pub fn top_reading(&mut self, reading: &Reading) {
        let noted = (reading.candidate, reading.confirmed);
        if self.reading != Some(noted) {
            self.reading = Some(noted);
            let Candidate { seed, link_first, offset } = reading.candidate;
            self.line(format_args!("reading {seed} {link_first} {offset} {}", reading.confirmed));
        }
    }

    pub fn event(&mut self, event: &Event) {
        match event {
            Event::Flutter(flutter) => {
                self.line(format_args!("flutter {} {} {:.2}", flutter.start, flutter.count, flutter.fit));
            }
            Event::Cue { box_frame, seconds, .. } => self.line(format_args!("cue {box_frame} {seconds:.3}")),
            Event::LikelyCue { box_frame, seconds, chance, .. } => {
                self.line(format_args!("likely {box_frame} {seconds:.3} {chance:.3}"));
            }
            Event::BoxGone { game_frame, seconds, heart_piece } => {
                self.line(format_args!("gone {game_frame} {seconds:.3} {heart_piece:?}"));
                self.read_started();
            }
            Event::BoxOpened | Event::TextOpened | Event::TextGone | Event::NaviSeen => self.read_started(),
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Press {
    pub frame: f64,
    pub count: u8,
    /// The count as first pressed, before any corrections.
    first_count: u8,
}

/// One stretch of the log that shares its game frames.
#[derive(Debug, Default, PartialEq)]
pub struct Read {
    pub presses: Vec<Press>,
    /// When each press came, in seconds since the program started.
    pub seconds: Vec<f64>,
    /// The last reading that had proven itself.
    pub proven: Option<Candidate>,
}

pub fn parse(text: &str) -> Vec<Read> {
    let mut reads = vec![Read::default()];
    let mut last_press = None;
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let read = reads.last_mut().expect("there's always one");
        match fields.as_slice() {
            ["run", ..] | ["read"] => {
                reads.push(Read::default());
                last_press = None;
            }
            ["press", count, seconds, frame] => {
                if let (Ok(count), Ok(seconds), Ok(frame)) = (count.parse(), seconds.parse::<f64>(), frame.parse()) {
                    match read.presses.last_mut() {
                        Some(press) if last_press.is_some_and(|last| seconds - last < FLIP_SECONDS) => {
                            press.count = 7 - press.count;
                        }
                        _ => {
                            read.presses.push(Press { frame, count, first_count: count });
                            read.seconds.push(seconds);
                            last_press = Some(seconds);
                        }
                    }
                }
            }
            // Older logs had a key for switching the last press between 3 and 4
            ["flip", _] => {
                if let Some(press) = read.presses.last_mut() {
                    press.count = 7 - press.count;
                }
            }
            ["reading", seed, link_first, offset, confirmed] => {
                if let (Ok(seed), Ok(link_first), Ok(offset), Ok(confirmed)) =
                    (seed.parse(), link_first.parse(), offset.parse(), confirmed.parse::<u32>())
                    && confirmed >= CONFIRMATIONS
                {
                    read.proven = Some(Candidate { seed, link_first, offset });
                }
            }
            _ => {}
        }
    }
    reads
}

/// A press compared to its flutter. `after_end` is in game frames from the flutter's end.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Paired {
    truth: u8,
    key: u8,
    first_key: u8,
    after_end: f64,
}

#[derive(Debug, Default, PartialEq)]
struct Compared {
    paired: Vec<Paired>,
    /// Presses with no flutter of their own.
    strays: u32,
    /// Flutters with no press, between the first and last that have one.
    missed: u32,
}

fn compare(presses: &[Press], candidate: Candidate) -> Compared {
    let Some(last) = presses.iter().map(|p| p.frame).max_by(f64::total_cmp) else { return Compared::default() };
    let mut blinks = candidate.blinks();
    let mut flutters = Vec::new();
    loop {
        let flutter = blinks.next_flutter();
        let start = flutter.start + candidate.offset;
        if start as f64 > last + 1.0 {
            break;
        }
        flutters.push((start, flutter.count));
    }
    let mut best: Vec<Option<Paired>> = vec![None; flutters.len()];
    let mut strays = 0;
    let first = flutters.first().map_or(f64::MAX, |&(start, _)| start as f64 + PAIRED_FRAMES.start());
    for press in presses.iter().filter(|press| press.frame >= first) {
        let closest = flutters
            .iter()
            .enumerate()
            .map(|(i, &(start, count))| (i, count, press.frame - (start + flutter_len(count)) as f64))
            .filter(|(_, _, after_end)| PAIRED_FRAMES.contains(after_end))
            .min_by(|a, b| (a.2 - EXPECTED_FRAMES).abs().total_cmp(&(b.2 - EXPECTED_FRAMES).abs()));
        let Some((i, truth, after_end)) = closest else {
            strays += 1;
            continue;
        };
        let new = Paired { truth, key: press.count, first_key: press.first_count, after_end };
        match best[i] {
            Some(old) if (old.after_end - EXPECTED_FRAMES).abs() <= (after_end - EXPECTED_FRAMES).abs() => strays += 1,
            Some(_) => {
                strays += 1;
                best[i] = Some(new);
            }
            None => best[i] = Some(new),
        }
    }
    let first_pressed = best.iter().position(Option::is_some);
    let last_pressed = best.iter().rposition(Option::is_some);
    let missed = match (first_pressed, last_pressed) {
        (Some(first), Some(last)) => best[first..=last].iter().filter(|paired| paired.is_none()).count() as u32,
        _ => 0,
    };
    Compared { paired: best.into_iter().flatten().collect(), strays, missed }
}

/// Mean and standard deviation.
fn spread(values: &[f64]) -> (f64, f64) {
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    (mean, (values.iter().map(|value| (value - mean).powi(2)).sum::<f64>() / n).sqrt())
}

fn timing_line(name: &str, paired: &[&Paired]) -> String {
    if paired.is_empty() {
        return format!("{name}: no presses");
    }
    let frames: Vec<f64> = paired.iter().map(|p| p.after_end).collect();
    let (mean, deviation) = spread(&frames);
    format!(
        "{name}: {} presses, {:.0} ms after the flutter's end on average, spread {:.0} ms (1 sigma)",
        paired.len(),
        mean * GAME_FRAME_MS,
        deviation * GAME_FRAME_MS,
    )
}

fn report(text: &str) -> String {
    let reads = parse(text);
    let with_presses = reads.iter().filter(|read| !read.presses.is_empty()).count();
    let mut all = Compared::default();
    let mut used = 0;
    for read in &reads {
        let Some(candidate) = read.proven else { continue };
        if read.presses.is_empty() {
            continue;
        }
        used += 1;
        let compared = compare(&read.presses, candidate);
        all.paired.extend(compared.paired);
        all.strays += compared.strays;
        all.missed += compared.missed;
    }
    // Timing is of presses that were right straight away: a miscount may be timed differently
    let of_count =
        |count: u8| all.paired.iter().filter(|p| p.truth == count && p.first_key == count).collect::<Vec<&Paired>>();
    let right: Vec<&Paired> = all.paired.iter().filter(|p| p.truth == p.first_key).collect();
    let fixed: Vec<&Paired> = all.paired.iter().filter(|p| p.truth != p.first_key && p.truth == p.key).collect();
    let spoiled = all.paired.iter().filter(|p| p.truth == p.first_key && p.truth != p.key).count();
    let first_wrong_as = |truth: u8| all.paired.iter().filter(|p| p.truth == truth && p.first_key != truth).count();
    let wrong_as = |truth: u8| all.paired.iter().filter(|p| p.truth == truth && p.key != truth).count();
    [
        format!("{used} of {with_presses} reads with presses had a proven state to compare to."),
        timing_line("Real 3s", &of_count(3)),
        timing_line("Real 4s", &of_count(4)),
        timing_line("Both", &right),
        timing_line("Corrected", &fixed),
        format!("Wrong at first: {} of 3 pressed as 4, {} of 4 pressed as 3.", first_wrong_as(3), first_wrong_as(4)),
        format!("Corrections that made a right count wrong: {spoiled}."),
        format!("Wrong in the end: {} of 3 pressed as 4, {} of 4 pressed as 3.", wrong_as(3), wrong_as(4)),
        format!("Missed flutters: {}. Stray presses: {}.", all.missed, all.strays),
        "Times are against frames arriving at this program, so what the screen you watched showed may differ by the feed delay."
            .to_owned(),
    ]
    .join("\n")
}

/// `dampe press-stats <file>`.
pub fn run(args: &[String]) -> Result<(), String> {
    let [path] = args else { return Err("usage: dampe press-stats <file>".to_owned()) };
    let text = std::fs::read_to_string(path).map_err(|e| format!("couldn't read '{path}': {e}"))?;
    println!("{}", report(&text));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANDIDATE: Candidate = Candidate { seed: 3_141_592, link_first: 40, offset: 100 };

    /// The flutters of `CANDIDATE`, as start and count.
    fn flutters(n: usize) -> Vec<(i64, u8)> {
        let mut blinks = CANDIDATE.blinks();
        (0..n)
            .map(|_| {
                let flutter = blinks.next_flutter();
                (flutter.start + CANDIDATE.offset, flutter.count)
            })
            .collect()
    }

    fn end(flutter: (i64, u8)) -> f64 {
        (flutter.0 + flutter_len(flutter.1)) as f64
    }

    fn press(frame: f64, count: u8) -> Press {
        Press { frame, count, first_count: count }
    }

    #[test]
    fn a_quick_second_press_flips_the_count_and_keeps_the_time() {
        let reads = parse("read\npress 3 2.000 50.25\npress 3 5.000 110.50\npress 4 5.800 126.50\n");
        assert_eq!(reads[1].presses, vec![press(50.25, 3), Press { frame: 110.5, count: 4, first_count: 3 }]);
        // Either key does it, and a third press flips it back
        let reads = parse("read\npress 4 5.000 100.00\npress 4 5.300 106.00\n");
        assert_eq!(reads[1].presses, vec![Press { frame: 100.0, count: 3, first_count: 4 }]);
        let reads = parse("read\npress 4 5.000 100.00\npress 4 5.300 106.00\npress 3 5.600 112.00\n");
        assert_eq!(reads[1].presses, vec![press(100.0, 4)]);
        // The window runs from the press it corrects, so corrections can't chain into the next flutter
        let reads = parse("read\npress 3 5.000 100.00\npress 4 5.800 116.00\npress 3 6.400 128.00\n");
        assert_eq!(reads[1].presses, vec![Press { frame: 100.0, count: 4, first_count: 3 }, press(128.0, 3)]);
    }

    #[test]
    fn a_new_read_has_nothing_to_correct_or_flip() {
        let reads = parse("read\npress 3 2.000 50.25\nread\npress 4 2.500 60.00\nread\nflip 4.000\n");
        assert_eq!(reads[1].presses, vec![press(50.25, 3)]);
        assert_eq!(reads[2].presses, vec![press(60.0, 4)]);
        assert!(reads[3].presses.is_empty());
    }

    #[test]
    fn older_logs_keep_their_flips() {
        let reads = parse("read\npress 3 3.000 60.50\nflip 3.200\nflip 3.300\nflip 3.400\n");
        assert_eq!(reads[1].presses, vec![Press { frame: 60.5, count: 4, first_count: 3 }]);
    }

    #[test]
    fn the_report_tells_first_counts_from_final_ones() {
        let all = flutters(12);
        let four = all.iter().position(|f| f.1 == 4).expect("no 4 in there");
        let three = all.iter().position(|f| f.1 == 3).expect("no 3 in there");
        let mut text = String::from("read\nreading 3141592 40 100 2\n");
        for (i, &flutter) in all.iter().enumerate() {
            // The 4 is pressed as a 3 and corrected. The 3 is right and corrected wrong.
            let pressed = if i == four { 3 } else { flutter.1 };
            let seconds = i as f64 * 3.0;
            text += &format!("press {pressed} {seconds:.3} {:.2}\n", end(flutter) + 5.0);
            if i == four || i == three {
                text += &format!("press {} {:.3} {:.2}\n", 7 - pressed, seconds + 0.5, end(flutter) + 15.0);
            }
        }
        let report = report(&text);
        assert!(report.contains("Corrected: 1 presses"), "{report}");
        assert!(report.contains("250 ms after"), "the corrected press keeps its time: {report}");
        assert!(report.contains("Wrong at first: 0 of 3 pressed as 4, 1 of 4 pressed as 3."), "{report}");
        assert!(report.contains("made a right count wrong: 1."), "{report}");
        assert!(report.contains("Wrong in the end: 1 of 3 pressed as 4, 0 of 4 pressed as 3."), "{report}");
    }

    #[test]
    fn presses_are_timed_from_the_end_of_their_own_flutter() {
        let all = flutters(6);
        let presses: Vec<Press> = all.iter().map(|&f| press(end(f) + 4.0, f.1)).collect();
        let compared = compare(&presses, CANDIDATE);
        assert_eq!((compared.paired.len(), compared.strays, compared.missed), (6, 0, 0));
        assert!(compared.paired.iter().all(|p| p.after_end == 4.0 && p.truth == p.key));
    }

    #[test]
    fn a_skipped_flutter_is_missed_and_an_extra_press_is_a_stray() {
        let all = flutters(6);
        let mut presses: Vec<Press> = all.iter().map(|&f| press(end(f) + 4.0, f.1)).collect();
        presses.remove(2);
        presses.insert(3, press(end(all[3]) + 6.5, all[3].1));
        let compared = compare(&presses, CANDIDATE);
        assert_eq!((compared.paired.len(), compared.strays, compared.missed), (5, 1, 1));
        assert!(compared.paired.iter().all(|p| p.after_end == 4.0), "the press closer to expected is kept");
    }

    #[test]
    fn a_wrong_key_still_pairs_and_shows_as_a_wrong_count() {
        let all = flutters(1);
        let wrong = if all[0].1 == 3 { 4 } else { 3 };
        let compared = compare(&[press(end(all[0]) + 3.0, wrong)], CANDIDATE);
        assert_eq!(compared.paired, vec![Paired { truth: all[0].1, key: wrong, first_key: wrong, after_end: 3.0 }]);
    }

    #[test]
    fn reads_only_use_their_own_proven_reading() {
        let reads = parse(
            "run Input 150 ms\npress 3 1.000 -\nread\nreading 5 6 7 0\npress 3 2.000 50.25\nreading 8 9 10 2\nread\npress 4 3.000 60.00\n",
        );
        assert_eq!(reads.len(), 4);
        assert!(reads[1].presses.is_empty(), "a press from before reading has no frame");
        assert_eq!(reads[2].presses, vec![press(50.25, 3)]);
        assert_eq!(reads[2].proven, Some(Candidate { seed: 8, link_first: 9, offset: 10 }));
        assert_eq!(reads[3].proven, None);
    }

    #[test]
    fn the_report_gives_timing_for_each_count() {
        let all = flutters(12);
        let mut text = String::from("read\nreading 3141592 40 100 2\n");
        for (i, &flutter) in all.iter().enumerate() {
            text += &format!("press {} {:.3} {:.2}\n", flutter.1, i as f64 * 3.0, end(flutter) + 5.0);
        }
        let report = report(&text);
        assert!(report.contains("1 of 1 reads"), "{report}");
        assert!(report.contains("250 ms after"), "{report}");
        assert!(report.contains("spread 0 ms"), "{report}");
        assert!(report.contains("Missed flutters: 0. Stray presses: 0."), "{report}");
    }
}
