//! Reads Dampe's blinks from video and says when to press A for the Heart Piece. Video in, a cue
//! out: it never sends any input to the game.

#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod diag;
#[cfg(windows)]
mod gui;
#[cfg(windows)]
mod hotkey;
#[cfg(windows)]
mod live;
mod replay;
#[cfg(windows)]
mod sound;
#[cfg(windows)]
mod timing;

const USAGE: &str = "usage:
  dampe live [--device <name>] [--platform n64|vc] [--feed-delay-ms <ms>] [--no-window] [--no-sound] [--navi-key <F1-F24>]
                                                 read a device and cue the A press
  dampe diag [--device <name>] [--seconds <n>]   measure what the feed delivers
  dampe devices                                  list video devices
  dampe replay <video> [--truth <file>]          test on a recording, scored against RNG watch values
  dampe replay <video> --session [--navi-gone-at <video frame>]   run a recording the way live runs it";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("replay") => replay::run(&args[1..]),
        #[cfg(windows)]
        Some("live") => live::run(&args[1..]),
        #[cfg(windows)]
        Some("diag") => diag::run(&args[1..]),
        #[cfg(windows)]
        Some("devices") => capture::list_devices().map(|devices| devices.iter().for_each(|d| println!("{d}"))),
        _ => Err(USAGE.to_owned()),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
