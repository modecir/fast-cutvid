//! Exercise the Windows/Linux decoder on every host, including macOS CI.
#![allow(dead_code)]
#[path = "../src/media.rs"]
mod media;
#[path = "../src/model.rs"]
mod model;
#[path = "../src/player.rs"]
mod player;

#[cfg(target_os = "macos")]
#[test]
fn finder_launch_generates_thumbnails_and_waveforms() {
    use std::process::Command;

    const SOURCE_ENV: &str = "FASTCUT_TEST_FINDER_SOURCE";
    if let Some(source) = std::env::var_os(SOURCE_ENV) {
        let source = std::path::Path::new(&source);
        let asset = media::probe(source).unwrap();
        assert!(asset.has_audio);
        let frame = media::thumbnail_image(&media::thumbnail(source, 0.0).unwrap()).unwrap();
        assert_eq!(frame.size[0].max(frame.size[1]), 240);
        assert!(frame.size[0].min(frame.size[1]) > 0);
        let peaks = media::waveform(source, |_| true).unwrap();
        assert!(peaks.iter().any(|peak| *peak > 0.01));
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source with spaces.mov");
    assert!(
        Command::new(media::ffmpeg_binary())
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x90:duration=0.2",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=0.2",
                "-c:v",
                "libx264",
                "-threads",
                "1",
                "-c:a",
                "aac",
            ])
            .arg(&source)
            .status()
            .unwrap()
            .success()
    );
    // Re-execute in a child so parallel tests never see a mutated environment.
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "finder_launch_generates_thumbnails_and_waveforms",
            "--nocapture",
        ])
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env_remove("FASTCUT_FFMPEG")
        .env_remove("FASTCUT_FFPROBE")
        .env(SOURCE_ENV, &source)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Finder-style analysis failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

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

#[test]
fn rapid_scrubs_settle_on_the_latest_frame_and_stop_polling() {
    use std::{
        process::Command,
        thread,
        time::{Duration, Instant},
    };
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("scrub.mp4");
    assert!(
        Command::new(media::ffmpeg_binary())
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x90:rate=30:duration=1",
                "-c:v",
                "libx264",
                "-threads",
                "1"
            ])
            .arg(&path)
            .status()
            .unwrap()
            .success()
    );
    let asset = media::probe(&path).unwrap();
    let mut preview = player::PreviewPlayer::default();
    for i in 0..200 {
        preview.start(
            uuid::Uuid::new_v4(),
            &asset,
            (i % 20) as f64 / 30.0,
            0.03,
            false,
            0.0,
        );
    }
    let final_id = uuid::Uuid::new_v4();
    preview.start(final_id, &asset, 0.8, 0.03, false, 0.0);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut result = None;
    while Instant::now() < deadline {
        if let Some(frame) = preview.latest() {
            result = Some(frame);
        }
        if result.is_some() && !preview.needs_repaint() {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(preview.clip_id, Some(final_id));
    assert!(!preview.needs_repaint(), "Paused decoder keeps polling");
    let result = result.expect("Latest scrub produced no frame");
    preview.start(final_id, &asset, 0.8, 0.03, false, 0.0);
    let deadline = Instant::now() + Duration::from_secs(5);
    let reference = loop {
        if let Some(frame) = preview.latest() {
            break frame;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(
        result.rgba, reference.rgba,
        "Obsolete scrub frame was displayed"
    );
}
