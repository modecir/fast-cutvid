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

/// Scale to square pixels without baking a landscape canvas into the image.
/// `dar` includes sample aspect ratio and FFmpeg's automatic display rotation.
pub fn display_scale(max_edge: u32) -> String {
    format!(
        "scale=w='max(1,round(min({max_edge},{max_edge}*dar)))':h='max(1,round(min({max_edge},{max_edge}/dar)))':flags=fast_bilinear,setsar=1"
    )
}

pub fn thumbnail(path: &Path, timestamp: f64) -> Result<Vec<u8>> {
    // Input seeking decodes only the preceding GOP, including for short files.
    // Limit each decoder so background imports leave CPU available for playback.
    let output = Command::new(ffmpeg_binary())
        .args(["-v", "error", "-nostdin", "-threads", "1", "-ss"])
        .arg(format!("{timestamp:.6}"))
        .arg("-i")
        .arg(path)
        .args([
            "-map",
            "0:v:0",
            "-an",
            "-sn",
            "-dn",
            "-filter_threads",
            "1",
            "-vf",
        ])
        .arg(display_scale(240))
        .args([
            "-frames:v",
            "1",
            "-threads",
            "1",
            "-q:v",
            "4",
            "-f",
            "image2pipe",
            "-c:v",
            "mjpeg",
            "pipe:1",
        ])
        .output()
        .with_context(|| "FFmpeg was not found. Install FFmpeg or set FASTCUT_FFMPEG")?;
    if !output.status.success() || output.stdout.is_empty() {
        bail!(
            "could not generate timeline frame: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

pub fn thumbnail_image(bytes: &[u8]) -> Result<ColorImage> {
    let image = image::load_from_memory(bytes)?.to_rgba8();
    anyhow::ensure!(
        image.width() <= 240 && image.height() <= 240,
        "invalid cached frame size"
    );
    let size = [image.width() as usize, image.height() as usize];
    Ok(ColorImage::from_rgba_unmultiplied(size, image.as_raw()))
}

pub const PEAKS_PER_SECOND: usize = 50;

/// Stream raw peaks in small batches; returning false cancels and reaps FFmpeg.
/// Normalization belongs to the view so early peaks can appear immediately.
pub fn waveform(path: &Path, mut emit: impl FnMut(Vec<f32>) -> bool) -> Result<Vec<f32>> {
    const SAMPLE_RATE: usize = 4_000;
    const SAMPLES_PER_PEAK: usize = SAMPLE_RATE / PEAKS_PER_SECOND;

    let mut child = Command::new(ffmpeg_binary())
        .args(["-v", "error", "-nostdin", "-threads", "1", "-i"])
        .arg(path)
        .args([
            "-map",
            "0:a:0",
            "-vn",
            "-sn",
            "-dn",
            "-filter_threads",
            "1",
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
    let result = (|| -> Result<Vec<f32>> {
        let mut stdout = child.stdout.take().context("could not read FFmpeg audio")?;
        let mut peaks = Vec::new();
        let mut sent = 0;
        let mut peak = 0.0_f32;
        let mut count = 0;
        let mut buffer = [0_u8; 65_539];
        let mut carry = 0;
        loop {
            let read = stdout.read(&mut buffer[carry..])?;
            if read == 0 {
                break;
            }
            let total = carry + read;
            let aligned = total / 4 * 4;
            for sample in buffer[..aligned].chunks_exact(4) {
                let value = f32::from_le_bytes(sample.try_into().expect("four bytes"));
                if value.is_finite() {
                    peak = peak.max(value.abs());
                }
                count += 1;
                if count == SAMPLES_PER_PEAK {
                    peaks.push(peak);
                    peak = 0.0;
                    count = 0;
                }
            }
            buffer.copy_within(aligned..total, 0);
            carry = total - aligned;
            // First paint after 0.2s of audio; later batches cover about 2s.
            if peaks.len() - sent >= if sent == 0 { 10 } else { 100 } {
                anyhow::ensure!(emit(peaks[sent..].to_vec()), "analysis cancelled");
                sent = peaks.len();
            }
        }
        if count > 0 {
            peaks.push(peak);
        }
        if sent < peaks.len() {
            anyhow::ensure!(emit(peaks[sent..].to_vec()), "analysis cancelled");
        }
        Ok(peaks)
    })();
    if result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    let peaks = result?;
    anyhow::ensure!(status.success(), "could not decode audio waveform");
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
