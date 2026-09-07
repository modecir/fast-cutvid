//! Real decoder regression check. Requires FFmpeg, as does the application.
#![allow(dead_code, unused_imports)] // Source modules also contain unit tests.

#[cfg(target_os = "macos")]
#[path = "../src/model.rs"]
mod model;
#[cfg(target_os = "macos")]
#[path = "../src/player_macos.rs"]
mod player;

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    use model::{Clip, MediaAsset, Project};
    use objc2_foundation::{NSDate, NSRunLoop};
    use player::PreviewPlayer;
    use std::{
        process::Command,
        time::{Duration, Instant},
    };
    use uuid::Uuid;

    let temp = tempfile::tempdir().unwrap();
    let mut assets = Vec::new();
    for (name, color, size, audio) in [
        ("red", "red", "160x90", true),
        ("blue", "blue", "90x160", false),
    ] {
        let path = temp.path().join(format!("{name}.mp4"));
        let mut command =
            Command::new(std::env::var("FASTCUT_FFMPEG").unwrap_or_else(|_| "ffmpeg".into()));
        command.args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            &format!("color=c={color}:s={size}:r=30:d=3"),
        ]);
        if audio {
            command.args([
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=3",
                "-c:a",
                "aac",
            ]);
        }
        assert!(
            command
                .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-t", "3"])
                .arg(&path)
                .status()
                .expect("FFmpeg is required for playback integration tests")
                .success()
        );
        let (width, height) = if audio { (160, 90) } else { (90, 160) };
        assets.push(MediaAsset {
            id: Uuid::new_v4(),
            name: name.into(),
            path: path.to_string_lossy().into(),
            duration: 3.0,
            width,
            height,
            fps: 30.0,
            has_audio: audio,
            rotation: 0,
        });
    }
    let make_clip = |asset: usize, source_in, source_out, muted| Clip {
        id: Uuid::new_v4(),
        asset_id: assets[asset].id,
        source_in,
        source_out,
        muted,
        audio_gain: 1.0,
    };
    let clips = vec![
        make_clip(0, 0.0, 0.5, false),
        make_clip(0, 0.5, 1.0, true),  // contiguous split
        make_clip(0, 2.0, 2.5, false), // seek within same source
        make_clip(1, 0.0, 0.5, false), // different size and no audio
        make_clip(0, 1.0, 1.5, false), // audio resumes
    ];
    let project = Project {
        assets,
        clips,
        ..Project::default()
    };
    project.validate().unwrap();
    let project_path = temp.path().join("playback.fastcut");
    project.save(&project_path).unwrap();
    assert!(
        Command::new(env!("CARGO_BIN_EXE_fast-cutvid"))
            .arg("--validate")
            .arg(&project_path)
            .status()
            .unwrap()
            .success()
    );
    let mut preview = PreviewPlayer::default();
    preview.start_timeline(&project, 0.0).unwrap();
    assert!(preview.timeline_matches(&project));
    let mut edited = project.clone();
    edited.clips[1].muted = false;
    assert!(!preview.timeline_matches(&edited));
    edited = project.clone();
    edited.clips.swap(0, 2);
    assert!(!preview.timeline_matches(&edited));

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut previous_time = 0.0;
    let mut last_frame = None;
    let mut largest_gap = Duration::ZERO;
    let mut seen = std::collections::HashSet::new();
    let mut saw_portrait = false;
    let mut clock_origin = None;
    while Instant::now() < deadline {
        // AVFoundation's asynchronous preparation needs the main run loop.
        NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.005));
        let time = preview.timeline_time().unwrap();
        assert!(
            time >= previous_time,
            "Timeline clock went backwards at a cut"
        );
        previous_time = time;
        if time > 0.05 && clock_origin.is_none() {
            clock_origin = Some((Instant::now(), time));
        }
        if let Some(frame) = preview.latest() {
            let now = Instant::now();
            // Exclude initial preparation; measure only running playback.
            if time > 0.15 {
                if let Some(last) = last_frame {
                    largest_gap = largest_gap.max(now - last);
                }
                last_frame = Some(now);
            }
            seen.insert(preview.clip_id.unwrap());
            if frame.size == [90, 160] {
                saw_portrait = true;
                assert!(
                    frame.rgba[2] > 200 && frame.rgba[0] < 30,
                    "Wrong source at portrait cut"
                );
            }
        }
        if time >= project.duration() - 0.00002 {
            break;
        }
    }
    assert!(
        previous_time >= project.duration() - 0.00002,
        "Playback stalled"
    );
    assert_eq!(
        seen.len(),
        project.clips.len(),
        "A cut had no decoded frames"
    );
    assert!(saw_portrait, "Different-source cut was never displayed");
    assert!(
        largest_gap < Duration::from_millis(150),
        "Visible pause between frames: {largest_gap:?}"
    );
    let (origin, from) = clock_origin.unwrap();
    let drift = (origin.elapsed().as_secs_f64() - (previous_time - from)).abs();
    assert!(drift < 0.15, "Playback lost {drift:.3}s at cut boundaries");
    preview.stop();
    preview.start_timeline(&project, 1.7).unwrap();
    assert_eq!(preview.clip_id, Some(project.clips[3].id));
    let seek_deadline = Instant::now() + Duration::from_secs(5);
    let mut seek_frame = None;
    while Instant::now() < seek_deadline {
        NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.005));
        if let Some(frame) = preview.latest() {
            seek_frame = Some(frame);
            break;
        }
    }
    let frame = seek_frame.expect("Seeking into the sequence produced no frame");
    assert_eq!(frame.size, [90, 160]);
    assert!(frame.rgba[2] > 200 && frame.rgba[0] < 30);
    preview.stop();
    let mut shortened = project.clone();
    shortened.clips.truncate(1);
    preview.start_timeline(&shortened, 2.4).unwrap();
    assert!(preview.timeline_time().unwrap() <= shortened.duration());
    preview.stop();
    assert!(preview.timeline_time().is_none());
    println!(
        "Native playback passed: 5 cuts, maximum frame gap {largest_gap:?}, clock drift {drift:.3}s"
    );
}
