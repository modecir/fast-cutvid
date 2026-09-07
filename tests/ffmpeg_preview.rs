//! Exercise the Windows/Linux decoder on every host, including macOS CI.
#![allow(dead_code)]
#[path = "../src/media.rs"]
mod media;
#[path = "../src/model.rs"]
mod model;
#[path = "../src/player.rs"]
mod player;

#[test]
fn preview_uses_source_dimensions_without_a_landscape_canvas() {
    use std::{
        process::Command,
        thread,
        time::{Duration, Instant},
    };
    let temp = tempfile::tempdir().unwrap();
    for (name, dimensions, sar, expected) in [
        ("portrait", "180x320", "1", [540, 960]),
        ("square", "240x240", "1", [960, 960]),
        ("anamorphic", "320x240", "2", [960, 360]),
    ] {
        let path = temp.path().join(format!("{name}.mp4"));
        assert!(
            Command::new(media::ffmpeg_binary())
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("testsrc2=size={dimensions}:duration=0.2"),
                    "-vf",
                    &format!("setsar={sar}"),
                    "-c:v",
                    "libx264",
                    "-threads",
                    "1",
                ])
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        let asset = media::probe(&path).unwrap();
        let mut preview = player::PreviewPlayer::default();
        preview.start(uuid::Uuid::new_v4(), &asset, 0.0, 0.2, false, 0.0);
        let deadline = Instant::now() + Duration::from_secs(5);
        let frame = loop {
            if let Some(frame) = preview.latest() {
                break frame;
            }
            assert!(Instant::now() < deadline, "No preview for {name}");
            thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(frame.size, expected, "{name}");
        assert_eq!(frame.rgba.len(), expected[0] * expected[1] * 4);
        assert_eq!(frame.display_size, [expected[0] as f32, expected[1] as f32]);
        preview.stop();
    }
}
