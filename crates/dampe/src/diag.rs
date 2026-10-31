//! `dampe diag`: measures what a device really delivers, to tell a bad feed from a bad read.

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use dampe_core::detector::{Cadence, Detector};
use dampe_core::picture::{HEIGHT, Luma, Normalizer, WIDTH};

use crate::capture::Capture;

pub fn run(args: &[String]) -> Result<(), String> {
    let mut device = "OBS Virtual Camera".to_owned();
    let mut seconds = 60u64;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--device" => device = args.next().ok_or("--device needs a name")?.clone(),
            "--seconds" => seconds = args.next().and_then(|v| v.parse().ok()).ok_or("--seconds needs a number")?,
            other => return Err(format!("unknown option {other}")),
        }
    }
    let capture = Capture::open(&device, false).map_err(|e| format!("couldn't open '{device}': {e}"))?;
    println!("{device}: {}x{}, measuring for {seconds} s", capture.size.0, capture.size.1);

    let mut picture = vec![0u8; WIDTH * HEIGHT];
    let mut normalizer = Normalizer::new();
    let mut cadence = Cadence::new();
    let mut detector = Detector::new();
    let started = Instant::now();
    let (mut video_frames, mut game_frames, mut compared, mut bad, mut with_blobs) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut last_game: Option<Instant> = None;
    let mut last_video: Option<Instant> = None;
    let mut game_gaps = BTreeMap::<u64, u64>::new();
    let mut video_gaps = BTreeMap::<u64, u64>::new();
    let mut slips = Vec::new();

    while started.elapsed() < Duration::from_secs(seconds) {
        let Ok(frame) = capture.frames.recv_timeout(Duration::from_secs(1)) else {
            println!("no frames for a second");
            continue;
        };
        video_frames += 1;
        if let Some(last) = last_video {
            *video_gaps.entry(((frame.arrival - last).as_secs_f64() * 1000.0 / 5.0).round() as u64 * 5).or_default() +=
                1;
        }
        last_video = Some(frame.arrival);
        let luma = Luma { data: &frame.luma, width: frame.width, height: frame.height, stride: frame.width };
        if !cadence.is_new_frame(luma) || !normalizer.process(luma, &mut picture) {
            continue;
        }
        game_frames += 1;
        if let Some(last) = last_game {
            let gap = (frame.arrival - last).as_secs_f64() * 1000.0;
            *game_gaps.entry((gap / 10.0).round() as u64 * 10).or_default() += 1;
            // Two game frames' time between changes means one wasn't seen, or the game lagged
            if gap > 80.0 {
                slips.push((frame.arrival - started).as_secs_f64());
            }
        }
        last_game = Some(frame.arrival);
        let (info, flutter) = detector.push(&picture);
        compared += u64::from(info.compared);
        bad += u64::from(info.bad_reference);
        with_blobs += u64::from(info.blobs > 0);
        if let Some(flutter) = flutter {
            println!(
                "{:6.1} s  flutter of {} at game frame {}, fit {:.2}",
                (frame.arrival - started).as_secs_f64(),
                flutter.count,
                flutter.start,
                flutter.fit
            );
        }
    }
    let elapsed = started.elapsed().as_secs_f64();
    println!("video {:.2} fps, game frames {:.2} per s", video_frames as f64 / elapsed, game_frames as f64 / elapsed);
    println!("dropped by this program: {}", capture.state.dropped.load(Ordering::Relaxed));
    println!("video frame gaps, ms: {video_gaps:?}");
    println!("game frame gaps, ms: {game_gaps:?}");
    println!(
        "long game frame gaps: {} at {:?}",
        slips.len(),
        slips.iter().map(|s| (s * 10.0).round() / 10.0).collect::<Vec<_>>()
    );
    println!("compared {compared}, bad reference {bad}, had blobs {with_blobs}");
    Ok(())
}
