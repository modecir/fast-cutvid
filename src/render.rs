use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail};

use crate::{media::ffmpeg_binary, model::Project};

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
    if project.clips.is_empty() {
        bail!("the timeline is empty");
    }
    let temp = tempfile::tempdir()?;
    let mut parts = Vec::with_capacity(project.clips.len());

    for (index, clip) in project.clips.iter().enumerate() {
        let asset = project
            .asset(clip.asset_id)
            .with_context(|| format!("clip {} references a missing asset", clip.id))?;
        let part = temp.path().join(format!("part-{index:05}.mp4"));
        report(RenderEvent {
            progress: index as f32 / (project.clips.len() + 1) as f32,
            message: format!("Encoding clip {} of {}", index + 1, project.clips.len()),
        });
        encode_part(project, clip, asset, &part)?;
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

    let result = Command::new(ffmpeg_binary())
        .args(["-y", "-v", "error", "-f", "concat", "-safe", "0", "-i"])
        .arg(&concat_path)
        .args(["-c", "copy", "-movflags", "+faststart"])
        .arg(output)
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
            .args(["-vf", &video_filter])
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
            ])
            .args(["-vf", &video_filter]);
    }

    let result = command
        .args([
            "-c:v",
            &settings.video_codec,
            "-preset",
            "veryfast",
            "-crf",
            "18",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            &settings.audio_codec,
            "-ar",
            "48000",
            "-ac",
            "2",
        ])
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

#[cfg(test)]
mod tests {
    use std::process::Command;

    use uuid::Uuid;

    use super::*;
    use crate::model::{Clip, MediaAsset, Project};

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
