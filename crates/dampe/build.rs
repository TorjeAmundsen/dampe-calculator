//! The plain build's window is made from the same file, without what's marked as only for video, so
//! none of that ends up in its program. Lines taken out leave blank ones, so errors keep their lines.

use std::path::Path;

const VIDEO: (&str, &str) = ("// video {", "// } video");
const PLAIN: (&str, &str) = ("// plain {", "// } plain");

/// `source` without the blocks between `markers`, and without any marker's own line.
fn without(source: &str, markers: (&str, &str)) -> String {
    let mut inside = false;
    let lines = source.lines().map(|line| {
        let marker = line.trim();
        if marker == markers.0 {
            inside = true;
        }
        let drop = inside || [VIDEO.0, VIDEO.1, PLAIN.0, PLAIN.1].contains(&marker);
        if marker == markers.1 {
            inside = false;
        }
        if drop { "" } else { line }
    });
    lines.collect::<Vec<_>>().join("\n")
}

fn main() {
    println!("cargo:rerun-if-changed=ui/main.slint");
    let source = std::fs::read_to_string("ui/main.slint").expect("the UI is there");
    let out = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    let path = Path::new(&out).join("main.slint");
    let markers = if std::env::var_os("CARGO_FEATURE_VIDEO").is_some() { PLAIN } else { VIDEO };
    std::fs::write(&path, without(&source, markers)).expect("OUT_DIR can be written");
    slint_build::compile(path).expect("the UI compiles");
}
