use std::{
    io::Read,
    path::{Path, PathBuf},
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
    start_time: Option<String>,
    duration: Option<String>,
    sample_aspect_ratio: Option<String>,
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

fn probe_streams(path: &Path) -> Result<ProbeResult> {
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
    Ok(serde_json::from_slice(&output.stdout)?)
}

pub fn probe(path: &Path) -> Result<MediaAsset> {
    let result = probe_streams(path)?;
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

/// Reject a cached/generated proxy if stream placement or display shape differs.
/// In particular, format duration alone cannot detect shifted audio tracks.
pub fn verify_proxy(source: &Path, proxy: &Path, fps: f64) -> Result<()> {
    let original = probe_streams(source)?;
    let preview = probe_streams(proxy)?;
    let tolerance = (1.0 / fps.max(1.0)).max(0.025);
    for kind in ["video", "audio"] {
        let original = original
            .streams
            .iter()
            .find(|stream| stream.codec_type.as_deref() == Some(kind));
        let preview = preview
            .streams
            .iter()
            .find(|stream| stream.codec_type.as_deref() == Some(kind));
        match (original, preview) {
            (Some(a), Some(b)) => {
                for (a, b) in [(&a.start_time, &b.start_time), (&a.duration, &b.duration)] {
                    let a = a
                        .as_deref()
                        .and_then(|v| v.parse::<f64>().ok())
                        .context("Source stream timing is unavailable")?;
                    let b = b
                        .as_deref()
                        .and_then(|v| v.parse::<f64>().ok())
                        .context("Preview stream timing is unavailable")?;
                    anyhow::ensure!(
                        (a - b).abs() <= tolerance,
                        "Preview would shift source timing"
                    );
                }
                if kind == "video" {
                    let ratio = |stream: &ProbeStream| -> f64 {
                        let sar = stream
                            .sample_aspect_ratio
                            .as_deref()
                            .and_then(|v| v.split_once(':'))
                            .and_then(|(a, b)| {
                                Some(a.parse::<f64>().ok()? / b.parse::<f64>().ok()?)
                            })
                            .unwrap_or(1.0);
                        let ratio = stream.width.unwrap_or(0) as f64 * sar
                            / stream.height.unwrap_or(0) as f64;
                        let rotation = stream
                            .side_data_list
                            .iter()
                            .find_map(|s| s.rotation)
                            .or_else(|| stream.tags.as_ref()?.rotate.as_ref()?.parse::<f64>().ok())
                            .unwrap_or(0.0);
                        if matches!(normalize_rotation(rotation), 90 | 270) {
                            1.0 / ratio
                        } else {
                            ratio
                        }
                    };
                    let expected = ratio(a);
                    let actual = ratio(b);
                    anyhow::ensure!(
                        expected.is_finite()
                            && actual.is_finite()
                            && (actual - expected).abs() / expected < 0.01,
                        "Preview display shape differs from the source"
                    );
                }
            }
            (None, None) => {}
            _ => bail!("Preview media tracks differ from the source"),
        }
    }
    Ok(())
}

/// Disposable editing copy. Preserve timestamps and audio gaps, normalize
/// display geometry, and use frequent keyframes to bound seek decode work.
pub fn create_proxy(source: &Path, output: &Path, active: impl Fn() -> bool) -> Result<()> {
    match create_proxy_attempt(source, output, &active, ["-fps_mode", "passthrough"]) {
        Err(error)
            if error.to_string().contains("Unrecognized option 'fps_mode'")
                || error.to_string().contains("Option fps_mode not found") =>
        {
            // FFmpeg before 5.1 uses vsync; newer releases removed that option.
            create_proxy_attempt(source, output, &active, ["-vsync", "0"])
        }
        result => result,
    }
}

fn create_proxy_attempt(
    source: &Path,
    output: &Path,
    active: &impl Fn() -> bool,
    sync: [&str; 2],
) -> Result<()> {
    let log = tempfile::NamedTempFile::new()?;
    let mut child = Command::new(ffmpeg_binary())
        .args([
            "-y",
            "-v",
            "error",
            "-nostdin",
            "-threads",
            "2",
            "-copyts",
            "-start_at_zero",
            "-i",
        ])
        .arg(source)
        .args(["-map", "0:v:0", "-map", "0:a:0?", "-vf"])
        .arg(format!(
            "{},scale=trunc(iw/2)*2:trunc(ih/2)*2",
            display_scale(960)
        ))
        .args([
            "-filter_threads",
            "1",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-crf",
            "23",
            "-g",
            "15",
            "-keyint_min",
            "15",
            "-bf",
            "0",
            "-pix_fmt",
            "yuv420p",
            "-threads",
            "2",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
            "-movflags",
            "+faststart",
        ])
        .args(sync)
        .arg(output)
        .stdout(Stdio::null())
        .stderr(Stdio::from(log.reopen()?))
        .spawn()
        .context("Could not start lightweight preview preparation")?;
    loop {
        if !active() {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Preview preparation cancelled");
        }
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                let mut detail = String::new();
                log.reopen()?.take(16_384).read_to_string(&mut detail)?;
                bail!("Could not create a lightweight preview: {}", detail.trim());
            }
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

pub fn ffmpeg_binary() -> PathBuf {
    media_binary("ffmpeg", "FASTCUT_FFMPEG")
}

pub fn ffprobe_binary() -> PathBuf {
    media_binary("ffprobe", "FASTCUT_FFPROBE")
}

fn media_binary(binary: &str, variable: &str) -> PathBuf {
    let configured = std::env::var_os(variable);
    #[cfg(target_os = "macos")]
    {
        resolve_macos_binary(
            binary,
            configured,
            std::env::var_os("PATH").as_deref(),
            &["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"],
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        configured
            .map(PathBuf::from)
            .unwrap_or_else(|| binary.into())
    }
}

#[cfg(any(target_os = "macos", all(test, unix)))]
fn resolve_macos_binary(
    binary: &str,
    configured: Option<std::ffi::OsString>,
    search_path: Option<&std::ffi::OsStr>,
    fallback_dirs: &[&str],
) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    // Explicit overrides are authoritative, even when the path is invalid.
    if let Some(path) = configured {
        return path.into();
    }
    // Finder does not inherit the shell's PATH. Preserve PATH priority, then
    // check the standard Apple silicon/Intel Homebrew and MacPorts locations.
    search_path
        .into_iter()
        .flat_map(std::env::split_paths)
        .chain(fallback_dirs.iter().map(PathBuf::from))
        .map(|directory| directory.join(binary))
        .find(|path| {
            path.metadata().is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
        .unwrap_or_else(|| binary.into())
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

#[cfg(all(test, unix))]
mod binary_tests {
    use super::resolve_macos_binary;
    use std::{ffi::OsString, fs, os::unix::fs::PermissionsExt, path::Path};

    fn executable(directory: &Path, binary: &str) -> std::path::PathBuf {
        fs::create_dir_all(directory).unwrap();
        let path = directory.join(binary);
        fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn finder_path_falls_back_to_package_manager_tools() {
        let temp = tempfile::tempdir().unwrap();
        let system = temp.path().join("system");
        let package_manager = temp.path().join("package manager/bin");
        for binary in ["ffmpeg", "ffprobe"] {
            let expected = executable(&package_manager, binary);
            for search_path in [Some(system.as_os_str()), None] {
                assert_eq!(
                    resolve_macos_binary(
                        binary,
                        None,
                        search_path,
                        &[package_manager.to_str().unwrap()],
                    ),
                    expected
                );
            }
        }
    }

    #[test]
    fn explicit_override_and_path_keep_priority_over_fallbacks() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        let fallback = temp.path().join("fallback");
        let expected = executable(&first, "ffprobe");
        executable(&second, "ffprobe");
        executable(&fallback, "ffprobe");
        let search_path = std::env::join_paths([&first, &second]).unwrap();
        let fallbacks = [fallback.to_str().unwrap()];
        assert_eq!(
            resolve_macos_binary("ffprobe", None, Some(&search_path), &fallbacks),
            expected
        );
        let configured = OsString::from("/custom tools/missing-ffprobe");
        assert_eq!(
            resolve_macos_binary(
                "ffprobe",
                Some(configured.clone()),
                Some(&search_path),
                &fallbacks,
            ),
            Path::new(&configured)
        );
    }

    #[test]
    fn skips_non_executable_files_directories_and_broken_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let non_executable = temp.path().join("non-executable");
        let directory = temp.path().join("directory");
        let broken_link = temp.path().join("broken-link");
        let fallback = temp.path().join("fallback");
        let path = executable(&non_executable, "ffmpeg");
        fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
        fs::create_dir_all(directory.join("ffmpeg")).unwrap();
        fs::create_dir_all(&broken_link).unwrap();
        std::os::unix::fs::symlink("missing", broken_link.join("ffmpeg")).unwrap();
        let expected = executable(&fallback, "ffmpeg");
        let search_path = std::env::join_paths([non_executable, directory, broken_link]).unwrap();
        assert_eq!(
            resolve_macos_binary(
                "ffmpeg",
                None,
                Some(&search_path),
                &[fallback.to_str().unwrap()],
            ),
            expected
        );
        assert_eq!(
            resolve_macos_binary("ffmpeg", None, Some(&search_path), &[]),
            Path::new("ffmpeg")
        );
    }
}
