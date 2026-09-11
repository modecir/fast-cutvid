use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail};

use crate::{
    media::{ffmpeg_binary, ffprobe_binary},
    model::Project,
};

#[derive(Debug, Clone)]
pub struct RenderEvent {
    pub progress: f32,
    pub message: String,
}

pub fn render_project(
    project: &Project,
    output: &Path,
    mut report: impl FnMut(RenderEvent),
) -> Result<()> {
    project.validate()?;
    protect_sources(project, output)?;
    if project.clips.is_empty() {
        bail!("the timeline is empty");
    }
    let copy_video = can_copy_video(project).unwrap_or(false);
    let temp = tempfile::tempdir()?;
    let mut parts = Vec::with_capacity(project.clips.len());

    for (index, clip) in project.clips.iter().enumerate() {
        let asset = project
            .asset(clip.asset_id)
            .with_context(|| format!("clip {} references a missing asset", clip.id))?;
        let part = temp.path().join(format!("part-{index:05}.mp4"));
        report(RenderEvent {
            progress: index as f32 / (project.clips.len() + 1) as f32,
            message: format!(
                "{} clip {} of {}",
                if copy_video {
                    "Copying video for"
                } else {
                    "Encoding"
                },
                index + 1,
                project.clips.len()
            ),
        });
        encode_part(project, clip, asset, &part, copy_video)?;
        parts.push(part);
    }

    report(RenderEvent {
        progress: project.clips.len() as f32 / (project.clips.len() + 1) as f32,
        message: "Joining timeline".to_owned(),
    });
    let concat_path = temp.path().join("timeline.txt");
    let concat = parts
        .iter()
        .map(|path| format!("file '{}'", path.to_string_lossy().replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&concat_path, concat)?;

    let suffix = format!(
        ".{}",
        output
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("mp4")
    );
    let destination = tempfile::Builder::new().suffix(&suffix).tempfile_in(
        output
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new(".")),
    )?;
    let result = Command::new(ffmpeg_binary())
        .args(["-y", "-v", "error", "-f", "concat", "-safe", "0", "-i"])
        .arg(&concat_path)
        .args(["-c", "copy", "-movflags", "+faststart"])
        .arg(destination.path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| "could not run FFmpeg")?;
    if !result.status.success() {
        bail!(
            "FFmpeg could not join the encoded clips: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    destination
        .persist(output)
        .context("Could not save the completed export")?;
    report(RenderEvent {
        progress: 1.0,
        message: format!("Finished {}", output.display()),
    });
    Ok(())
}

fn encode_part(
    project: &Project,
    clip: &crate::model::Clip,
    asset: &crate::model::MediaAsset,
    output: &PathBuf,
    copy_video: bool,
) -> Result<()> {
    let settings = &project.render;
    let duration = clip.duration();
    let video_filter = format!(
        "scale={}:{}:force_original_aspect_ratio=decrease,pad={}:{}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps={}",
        settings.width, settings.height, settings.width, settings.height, settings.fps
    );

    let mut command = Command::new(ffmpeg_binary());
    command
        .args(["-y", "-v", "error", "-ss"])
        .arg(format!("{:.6}", clip.source_in))
        .arg("-i")
        .arg(&asset.path);

    if asset.has_audio && !clip.muted {
        command
            .args([
                "-t",
                &format!("{duration:.6}"),
                "-map",
                "0:v:0",
                "-map",
                "0:a:0",
            ])
            .args([
                "-af",
                &format!("volume={:.3},aresample=48000", clip.audio_gain),
            ]);
    } else {
        command
            .args([
                "-f",
                "lavfi",
                "-t",
                &format!("{duration:.6}"),
                "-i",
                "anullsrc=r=48000:cl=stereo",
            ])
            .args([
                "-t",
                &format!("{duration:.6}"),
                "-map",
                "0:v:0",
                "-map",
                "1:a:0",
            ]);
    }

    if copy_video {
        command.args(["-c:v", "copy"]);
    } else {
        command.args([
            "-vf",
            &video_filter,
            "-c:v",
            &settings.video_codec,
            "-preset",
            "veryfast",
            "-crf",
            "18",
            "-pix_fmt",
            "yuv420p",
        ]);
    }
    let result = command
        .args(["-c:a", &settings.audio_codec, "-ar", "48000", "-ac", "2"])
        .arg(output)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| "could not run FFmpeg")?;
    if !result.status.success() {
        bail!(
            "FFmpeg failed while encoding {}: {}",
            asset.name,
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    Ok(())
}

fn protect_sources(project: &Project, output: &Path) -> Result<()> {
    if let Ok(destination) = fs::canonicalize(output) {
        for asset in &project.assets {
            let Ok(source) = fs::canonicalize(&asset.path) else {
                continue;
            };
            anyhow::ensure!(
                source != destination,
                "Export cannot overwrite source footage"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let a = fs::metadata(&source)?;
                let b = fs::metadata(&destination)?;
                anyhow::ensure!(
                    (a.dev(), a.ino()) != (b.dev(), b.ino()),
                    "Export cannot overwrite a source hard link"
                );
            }
        }
    }
    Ok(())
}

/// Copy video only when every part shares one H.264 configuration, has no
/// reordered frames, and cuts begin on independently decodable keyframes.
/// Audio is still normalized, so gain, mute, and silent gaps retain semantics.
fn can_copy_video(project: &Project) -> Result<bool> {
    let Some(first) = project.clips.first() else {
        return Ok(false);
    };
    let settings = &project.render;
    if settings.video_codec != "libx264"
        || project
            .clips
            .iter()
            .any(|clip| clip.asset_id != first.asset_id)
    {
        return Ok(false);
    }
    let asset = project.asset(first.asset_id).context("Missing source")?;
    if asset.rotation != 0 {
        return Ok(false);
    }
    let probe = Command::new(ffprobe_binary())
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_streams",
            "-of",
            "json",
        ])
        .arg(&asset.path)
        .output()?;
    if !probe.status.success() {
        return Ok(false);
    }
    let data: serde_json::Value = serde_json::from_slice(&probe.stdout)?;
    let video = &data["streams"][0];
    let number = |name: &str| video[name].as_str().and_then(|s| s.parse::<f64>().ok());
    let fps = settings.fps as f64;
    let rate = format!("{}/1", settings.fps);
    if video["codec_name"] != "h264"
        || video["has_b_frames"] != 0
        || video["width"] != settings.width
        || video["height"] != settings.height
        || video["pix_fmt"] != "yuv420p"
        || video["sample_aspect_ratio"] != "1:1"
        || video["avg_frame_rate"] != rate
        || video["r_frame_rate"] != rate
        || number("start_time") != Some(0.0)
        || video["field_order"]
            .as_str()
            .is_some_and(|field| field != "progressive" && field != "unknown")
        || video["side_data_list"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item.get("rotation").is_some()))
    {
        return Ok(false);
    }
    let Some(duration) = number("duration") else {
        return Ok(false);
    };
    let Some(frames) = number("nb_frames") else {
        return Ok(false);
    };
    if (frames / fps - duration).abs() > 0.00001 {
        return Ok(false);
    }
    let mut checked = std::collections::HashSet::new();
    for clip in &project.clips {
        if clip.source_out > duration + 0.00001
            || [clip.source_in, clip.source_out]
                .iter()
                .any(|time| (time * fps - (time * fps).round()).abs() > 0.00001)
        {
            return Ok(false);
        }
        if !checked.insert(clip.source_in.to_bits()) {
            continue;
        }
        let packets = Command::new(ffprobe_binary())
            .args(["-v", "error", "-select_streams", "v:0", "-read_intervals"])
            .arg(format!("{:.6}%+{:.6}", clip.source_in, 1.0 / fps))
            .args([
                "-show_packets",
                "-show_entries",
                "packet=pts_time,dts_time,flags",
                "-of",
                "json",
            ])
            .arg(&asset.path)
            .output()?;
        if !packets.status.success() {
            return Ok(false);
        }
        let data: serde_json::Value = serde_json::from_slice(&packets.stdout)?;
        let Some(packets) = data["packets"].as_array() else {
            return Ok(false);
        };
        let exact = packets.iter().any(|packet| {
            let pts = packet["pts_time"]
                .as_str()
                .and_then(|v| v.parse::<f64>().ok());
            let dts = packet["dts_time"]
                .as_str()
                .and_then(|v| v.parse::<f64>().ok());
            pts.is_some_and(|pts| (pts - clip.source_in).abs() < 0.00001)
                && pts == dts
                && packet["flags"]
                    .as_str()
                    .is_some_and(|flags| flags.contains('K'))
        });
        if !exact {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use uuid::Uuid;

    use super::*;
    use crate::model::{Clip, MediaAsset, Project};

    #[test]
    fn eligible_cuts_copy_video_exactly_and_preserve_mute_with_safe_fallbacks() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.mp4");
        assert!(
            Command::new(ffmpeg_binary())
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=320x180:rate=30:duration=2",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=duration=2",
                    "-vf",
                    "setsar=1",
                    "-c:v",
                    "libx264",
                    "-preset",
                    "ultrafast",
                    "-g",
                    "15",
                    "-keyint_min",
                    "15",
                    "-sc_threshold",
                    "0",
                    "-bf",
                    "0",
                    "-threads",
                    "1",
                    "-c:a",
                    "aac",
                    "-t",
                    "2"
                ])
                .arg(&source)
                .status()
                .unwrap()
                .success()
        );
        let asset = crate::media::probe(&source).unwrap();
        let mut first = Clip::from_asset(&asset);
        first.source_in = 0.5;
        first.source_out = 1.0;
        let mut second = first.clone();
        second.id = Uuid::new_v4();
        second.source_in = 1.5;
        second.source_out = 2.0;
        second.muted = true;
        let mut project = Project {
            assets: vec![asset],
            clips: vec![first, second],
            ..Project::default()
        };
        project.render.width = 320;
        project.render.height = 180;
        assert!(can_copy_video(&project).unwrap());
        let output = temp.path().join("cut.mp4");
        let mut copied = false;
        render_project(&project, &output, |event| {
            copied |= event.message.starts_with("Copying video")
        })
        .unwrap();
        assert!(copied);
        let hashes = |path: &Path| -> Vec<String> {
            let output = Command::new(ffmpeg_binary())
                .args(["-v", "error", "-i"])
                .arg(path)
                .args(["-map", "0:v:0", "-f", "framemd5", "-"])
                .output()
                .unwrap();
            assert!(output.status.success());
            String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .filter(|line| !line.starts_with('#'))
                .map(|line| line.rsplit(',').next().unwrap().trim().to_owned())
                .collect()
        };
        let original = hashes(&source);
        let expected: Vec<_> = original[15..30]
            .iter()
            .chain(&original[45..60])
            .cloned()
            .collect();
        assert_eq!(
            hashes(&output),
            expected,
            "Video frames changed or cut timing moved"
        );
        let peaks = crate::media::waveform(&output, |_| true).unwrap();
        assert!(peaks[5..20].iter().any(|p| *p > 0.01));
        assert!(
            peaks[30..].iter().all(|p| *p < 0.001),
            "Muted segment contains audio"
        );
        project.clips[0].source_in = 0.51;
        assert!(!can_copy_video(&project).unwrap());
        project.clips[0].source_in = 0.6;
        assert!(
            !can_copy_video(&project).unwrap(),
            "Non-keyframe cut must encode"
        );
        project.clips[0].source_in = 0.5;
        project.render.width = 640;
        assert!(!can_copy_video(&project).unwrap(), "Resizing must encode");
        project.render.width = 320;
        project.assets[0].rotation = 90;
        assert!(
            !can_copy_video(&project).unwrap(),
            "Rotated media must encode"
        );
        assert!(render_project(&project, &source, |_| {}).is_err());
    }

    #[test]
    fn renders_a_cut_with_audio_and_a_muted_segment() {
        if Command::new(ffmpeg_binary())
            .arg("-version")
            .output()
            .is_err()
        {
            eprintln!("skipping render test because FFmpeg is unavailable");
            return;
        }

        let temp = tempfile::tempdir().expect("temporary directory");
        let source = temp.path().join("source.mp4");
        let generated = Command::new(ffmpeg_binary())
            .args([
                "-y",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=30:duration=2",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000:duration=2",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&source)
            .status()
            .expect("create test source");
        assert!(generated.success());

        let asset_id = Uuid::new_v4();
        let mut project = Project::default();
        project.render.width = 320;
        project.render.height = 180;
        project.assets.push(MediaAsset {
            id: asset_id,
            name: "source.mp4".to_owned(),
            path: source.to_string_lossy().into_owned(),
            duration: 2.0,
            width: 320,
            height: 180,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
        });
        project.clips = vec![
            Clip {
                id: Uuid::new_v4(),
                asset_id,
                source_in: 0.25,
                source_out: 0.75,
                audio_gain: 1.0,
                muted: false,
            },
            Clip {
                id: Uuid::new_v4(),
                asset_id,
                source_in: 1.0,
                source_out: 1.75,
                audio_gain: 1.0,
                muted: true,
            },
        ];

        let output = temp.path().join("render.mp4");
        render_project(&project, &output, |_| {}).expect("render timeline");
        assert!(output.metadata().expect("render metadata").len() > 1_000);
    }
}
