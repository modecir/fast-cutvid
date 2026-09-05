use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail};
use eframe::egui::ColorImage;
use serde::Deserialize;
use uuid::Uuid;

use crate::model::MediaAsset;

#[derive(Deserialize)]
struct ProbeResult {
    streams: Vec<ProbeStream>,
    format: ProbeFormat,
}

#[derive(Deserialize)]
struct ProbeStream {
    codec_type: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
    #[serde(default)]
    side_data_list: Vec<ProbeSideData>,
    tags: Option<ProbeTags>,
}

#[derive(Deserialize)]
struct ProbeSideData {
    rotation: Option<f64>,
}

#[derive(Deserialize)]
struct ProbeTags {
    rotate: Option<String>,
}

#[derive(Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
}

pub fn probe(path: &Path) -> Result<MediaAsset> {
    let output = Command::new(ffprobe_binary())
        .args([
            "-v",
            "error",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .with_context(|| "FFprobe was not found. Install FFmpeg or set FASTCUT_FFPROBE")?;
    if !output.status.success() {
        bail!("FFprobe could not read {}", path.display());
    }
    let result: ProbeResult = serde_json::from_slice(&output.stdout)?;
    let video = result
        .streams
        .iter()
        .find(|stream| stream.codec_type.as_deref() == Some("video"))
        .context("file contains no video stream")?;
    let duration = result
        .format
        .duration
        .as_deref()
        .unwrap_or("0")
        .parse::<f64>()?;
    if duration <= 0.0 {
        bail!("video duration is zero");
    }
    let fps = video
        .avg_frame_rate
        .as_deref()
        .and_then(parse_rate)
        .unwrap_or(30.0);
    // FFprobe's display-matrix value uses the opposite sign from the legacy
    // clockwise `rotate` tag. Store one normalized clockwise value so native
    // players do not have to understand both container representations.
    let rotation = video
        .side_data_list
        .iter()
        .find_map(|data| data.rotation)
        .map(|degrees| -degrees)
        .or_else(|| {
            video
                .tags
                .as_ref()
                .and_then(|tags| tags.rotate.as_deref())
                .and_then(|degrees| degrees.parse::<f64>().ok())
        })
        .map(normalize_rotation)
        .unwrap_or(0);

    Ok(MediaAsset {
        id: Uuid::new_v4(),
        name: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        path: path.to_string_lossy().into_owned(),
        duration,
        width: video.width.unwrap_or(1920),
        height: video.height.unwrap_or(1080),
        fps,
        has_audio: result
            .streams
            .iter()
            .any(|stream| stream.codec_type.as_deref() == Some("audio")),
        rotation,
    })
}

fn normalize_rotation(degrees: f64) -> i32 {
    let quarter_turns = (degrees / 90.0).round() as i32;
    (quarter_turns.rem_euclid(4)) * 90
}

/// Decode an evenly spaced filmstrip in one FFmpeg invocation. The images keep
/// their source aspect ratio; layout code decides how to letterbox each cell.
pub fn timeline_thumbnails(path: &Path, duration: f64, count: usize) -> Result<Vec<ColorImage>> {
    if duration > 45.0 {
        return seek_thumbnails(path, duration, count);
    }
    let temp = tempfile::tempdir()?;
    let pattern = temp.path().join("frame-%03d.jpg");
    let sample_rate = count as f64 / duration.max(0.1);
    let output = Command::new(ffmpeg_binary())
        .args(["-v", "error", "-i"])
        .arg(path)
        .arg("-vf")
        .arg(format!(
            "fps={sample_rate:.8},scale=240:-2:force_original_aspect_ratio=decrease"
        ))
        .args(["-frames:v", &count.to_string(), "-q:v", "4"])
        .arg(&pattern)
        .output()
        .with_context(|| "FFmpeg was not found. Install FFmpeg or set FASTCUT_FFMPEG")?;
    if !output.status.success() {
        bail!("could not generate timeline filmstrip");
    }

    let mut paths = std::fs::read_dir(temp.path())?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect::<Vec<_>>();
    paths.sort();
    let mut frames = Vec::with_capacity(paths.len());
    for frame_path in paths {
        let image = image::open(frame_path)?.to_rgba8();
        let size = [image.width() as usize, image.height() as usize];
        frames.push(ColorImage::from_rgba_unmultiplied(size, image.as_raw()));
    }
    Ok(frames)
}

/// Long files should not be decoded from beginning to end just to populate a
/// filmstrip. Input-side seeks jump near each requested timestamp instead.
fn seek_thumbnails(path: &Path, duration: f64, count: usize) -> Result<Vec<ColorImage>> {
    let mut frames = Vec::with_capacity(count);
    for index in 0..count {
        let timestamp = duration * (index as f64 + 0.5) / count as f64;
        let output = Command::new(ffmpeg_binary())
            .args(["-v", "error", "-ss"])
            .arg(format!("{timestamp:.4}"))
            .arg("-i")
            .arg(path)
            .args([
                "-frames:v",
                "1",
                "-vf",
                "scale=240:-2:force_original_aspect_ratio=decrease",
                "-f",
                "image2pipe",
                "-vcodec",
                "mjpeg",
                "pipe:1",
            ])
            .output()
            .with_context(|| "FFmpeg was not found. Install FFmpeg or set FASTCUT_FFMPEG")?;
        if !output.status.success() {
            continue;
        }
        let image = image::load_from_memory(&output.stdout)?.to_rgba8();
        let size = [image.width() as usize, image.height() as usize];
        frames.push(ColorImage::from_rgba_unmultiplied(size, image.as_raw()));
    }
    if frames.is_empty() {
        bail!("could not generate timeline filmstrip");
    }
    Ok(frames)
}

/// Produce 50 normalized peak values per second. Samples are consumed as a
/// stream, so even long source files do not require a large audio buffer.
pub fn waveform(path: &Path) -> Result<Vec<f32>> {
    const SAMPLE_RATE: usize = 4_000;
    const SAMPLES_PER_PEAK: usize = SAMPLE_RATE / 50;

    let mut child = Command::new(ffmpeg_binary())
        .args(["-v", "error", "-i"])
        .arg(path)
        .args([
            "-vn",
            "-ac",
            "1",
            "-ar",
            &SAMPLE_RATE.to_string(),
            "-f",
            "f32le",
            "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| "FFmpeg was not found. Install FFmpeg or set FASTCUT_FFMPEG")?;
    let mut stdout = child.stdout.take().context("could not read FFmpeg audio")?;
    let mut peaks = Vec::new();
    let mut peak = 0.0_f32;
    let mut count = 0;
    let mut buffer = [0_u8; 65_536];
    let mut carry = Vec::with_capacity(3);
    loop {
        let read = stdout.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let mut bytes = Vec::with_capacity(carry.len() + read);
        bytes.extend_from_slice(&carry);
        bytes.extend_from_slice(&buffer[..read]);
        let aligned = bytes.len() / 4 * 4;
        for sample in bytes[..aligned].chunks_exact(4) {
            peak = peak.max(f32::from_le_bytes(sample.try_into().expect("four bytes")).abs());
            count += 1;
            if count == SAMPLES_PER_PEAK {
                peaks.push(peak);
                peak = 0.0;
                count = 0;
            }
        }
        carry.clear();
        carry.extend_from_slice(&bytes[aligned..]);
    }
    if count > 0 {
        peaks.push(peak);
    }
    let status = child.wait()?;
    if !status.success() {
        bail!("could not decode audio waveform");
    }

    let max = peaks.iter().copied().fold(0.0_f32, f32::max);
    if max > 0.000_01 {
        for peak in &mut peaks {
            *peak = (*peak / max).sqrt();
        }
    }
    Ok(peaks)
}

fn parse_rate(value: &str) -> Option<f64> {
    let (numerator, denominator) = value.split_once('/')?;
    let numerator: f64 = numerator.parse().ok()?;
    let denominator: f64 = denominator.parse().ok()?;
    (denominator != 0.0).then_some(numerator / denominator)
}

pub fn ffmpeg_binary() -> String {
    std::env::var("FASTCUT_FFMPEG").unwrap_or_else(|_| "ffmpeg".to_owned())
}

pub fn ffprobe_binary() -> String {
    std::env::var("FASTCUT_FFPROBE").unwrap_or_else(|_| "ffprobe".to_owned())
}

pub fn format_time(seconds: f64) -> String {
    let total_frames = (seconds.max(0.0) * 30.0).round() as u64;
    let frames = total_frames % 30;
    let total_seconds = total_frames / 30;
    let secs = total_seconds % 60;
    let minutes = (total_seconds / 60) % 60;
    let hours = total_seconds / 3600;
    format!("{hours:02}:{minutes:02}:{secs:02}:{frames:02}")
}

#[cfg(test)]
mod rotation_tests {
    use super::normalize_rotation;

    #[test]
    fn normalizes_display_rotation_to_clockwise_quarter_turns() {
        assert_eq!(normalize_rotation(90.0), 90);
        assert_eq!(normalize_rotation(-90.0), 270);
        assert_eq!(normalize_rotation(450.0), 90);
        assert_eq!(normalize_rotation(179.8), 180);
    }
}
