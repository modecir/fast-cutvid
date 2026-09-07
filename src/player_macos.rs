use std::{collections::HashMap, path::Path, slice};

use anyhow::{Context, Result, anyhow};
use objc2::{AnyThread, MainThreadMarker, rc::Retained, runtime::AnyObject};
use objc2_av_foundation::{
    AVAudioMixInputParameters, AVMediaTypeAudio, AVMediaTypeVideo, AVMutableAudioMix,
    AVMutableAudioMixInputParameters, AVMutableComposition, AVPlayer, AVPlayerItem,
    AVPlayerItemVideoOutput, AVURLAsset,
};
use objc2_core_media::{CMTime, CMTimeRange};
use objc2_core_video::{
    CVImageBufferGetDisplaySize, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetHeight, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress, kCVPixelFormatType_32BGRA,
};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSURL, ns_string};
use uuid::Uuid;

use crate::model::{MediaAsset, Project};

pub struct DecodedFrame {
    pub size: [usize; 2],
    pub display_size: [f32; 2],
    pub rgba: Vec<u8>,
}

/// macOS preview driven by one AVPlayer timebase. AVFoundation decodes and
/// presents audio itself; video frames are pulled from the same player item and
/// copied into an egui texture, so the two streams cannot drift independently.
#[derive(Default)]
pub struct PreviewPlayer {
    pub clip_id: Option<Uuid>,
    player: Option<Retained<AVPlayer>>,
    output: Option<Retained<AVPlayerItemVideoOutput>>,
    source_start: f64,
    source_end: f64,
    rotation: i32,
    timeline: Option<Project>,
}

impl PreviewPlayer {
    /// Assemble source ranges once. Cut boundaries are handled by AVFoundation,
    /// without a UI-thread seek, decoder replacement, or audio restart.
    #[allow(deprecated)] // Synchronous local-file track discovery; no network assets.
    pub fn start_timeline(&mut self, project: &Project, time: f64) -> Result<()> {
        let mtm = MainThreadMarker::new().context("Playback requires the main thread")?;
        // Live edits can shorten the sequence beneath the current playhead.
        let time = time.clamp(0.0, project.duration());
        // SAFETY: All AVFoundation objects are created and used on the UI thread.
        // Source ranges have already been validated by the project model.
        unsafe {
            let video_type = AVMediaTypeVideo.context("Video media type unavailable")?;
            let audio_type = AVMediaTypeAudio.context("Audio media type unavailable")?;
            let composition = AVMutableComposition::composition();
            let video = composition
                .addMutableTrackWithMediaType_preferredTrackID(video_type, 0)
                .context("Could not create preview video track")?;
            let audio = composition
                .addMutableTrackWithMediaType_preferredTrackID(audio_type, 0)
                .context("Could not create preview audio track")?;
            let levels =
                AVMutableAudioMixInputParameters::audioMixInputParametersWithTrack(Some(&audio));
            let mut sources = HashMap::new();
            let mut cursor = 0.0;
            for clip in &project.clips {
                let asset = project
                    .asset(clip.asset_id)
                    .context("Missing preview asset")?;
                if let std::collections::hash_map::Entry::Vacant(entry) = sources.entry(asset.id) {
                    let url = NSURL::from_file_path(Path::new(&asset.path))
                        .context("Invalid preview source path")?;
                    entry.insert(AVURLAsset::URLAssetWithURL_options(&url, None));
                }
                let source = &sources[&asset.id];
                let source_video = source
                    .tracksWithMediaType(video_type)
                    .firstObject()
                    .with_context(|| format!("No video track in {}", asset.name))?;
                // Round shared boundaries, rather than individual durations, so
                // fractional frame rates cannot accumulate gaps between cuts.
                let start = CMTime::with_seconds(cursor, 60_000);
                let end = CMTime::with_seconds(cursor + clip.duration(), 60_000);
                let range = CMTimeRange::new(
                    CMTime::with_seconds(clip.source_in, 60_000),
                    end.subtract(start),
                );
                video
                    .insertTimeRange_ofTrack_atTime_error(range, &source_video, start)
                    .map_err(|error| anyhow!("Could not prepare {}: {error}", asset.name))?;
                if let Some(source_audio) = source.tracksWithMediaType(audio_type).firstObject() {
                    // Audio can begin later or end earlier than the video. Leave
                    // silence outside its actual range instead of shifting it.
                    let available = range.intersection(source_audio.timeRange());
                    if available.duration.seconds() > 0.0 {
                        let audio_start = start.add(available.start.subtract(range.start));
                        audio
                            .insertTimeRange_ofTrack_atTime_error(
                                available,
                                &source_audio,
                                audio_start,
                            )
                            .map_err(|error| {
                                anyhow!("Could not prepare audio for {}: {error}", asset.name)
                            })?;
                    }
                }
                levels.setVolume_atTime(
                    if clip.muted {
                        0.0
                    } else {
                        clip.audio_gain.clamp(0.0, 1.0)
                    },
                    start,
                );
                cursor += clip.duration();
            }
            let mix = AVMutableAudioMix::audioMix();
            mix.setInputParameters(&NSArray::<AVAudioMixInputParameters>::from_slice(&[
                &levels,
            ]));
            let item = AVPlayerItem::playerItemWithAsset(&composition, mtm);
            item.setAudioMix(Some(&mix));
            let output = video_output();
            item.addOutput(&output);
            let player = AVPlayer::playerWithPlayerItem(Some(&item), mtm);
            player.seekToTime_toleranceBefore_toleranceAfter(
                CMTime::with_seconds(time, 60_000),
                CMTime::with_seconds(0.0, 60_000),
                CMTime::with_seconds(0.0, 60_000),
            );
            self.stop();
            self.clip_id = project.clip_at(time).map(|(_, _, clip)| clip.id);
            self.source_start = time;
            self.source_end = cursor;
            self.timeline = Some(project.clone());
            self.output = Some(output);
            player.play();
            self.player = Some(player);
        }
        Ok(())
    }

    pub fn timeline_matches(&self, project: &Project) -> bool {
        self.timeline
            .as_ref()
            .is_some_and(|cached| cached.clips == project.clips && cached.assets == project.assets)
    }

    pub fn timeline_time(&mut self) -> Option<f64> {
        let time = self.source_time()?;
        let project = self.timeline.as_ref()?;
        self.clip_id = project.clip_at(time).map(|(_, _, clip)| clip.id);
        Some(time)
    }

    pub fn seek(
        &mut self,
        clip_id: Uuid,
        source_time: f64,
        source_end: f64,
        realtime: bool,
        volume: f32,
    ) -> bool {
        if self.timeline.is_some() || self.clip_id != Some(clip_id) {
            return false;
        }
        let Some(player) = self.player.as_ref() else {
            return false;
        };
        // SAFETY: AVPlayer access is confined to the main UI thread.
        unsafe {
            player.setVolume(volume.clamp(0.0, 1.0));
            player.seekToTime_toleranceBefore_toleranceAfter(
                CMTime::with_seconds(source_time, 60_000),
                CMTime::with_seconds(0.0, 60_000),
                CMTime::with_seconds(0.0, 60_000),
            );
            if realtime {
                player.playImmediatelyAtRate(1.0);
            } else {
                player.pause();
            }
        }
        self.source_start = source_time;
        self.source_end = source_end;
        true
    }

    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &mut self,
        clip_id: Uuid,
        asset: &MediaAsset,
        source_time: f64,
        length: f64,
        realtime: bool,
        volume: f32,
    ) {
        self.stop();
        let Some(mtm) = MainThreadMarker::new() else {
            eprintln!("AVPlayer must be created on the main thread");
            return;
        };
        let Some(url) = NSURL::from_file_path(Path::new(&asset.path)) else {
            eprintln!("AVPlayer could not create a file URL for {}", asset.path);
            return;
        };

        // SAFETY: AVFoundation playback objects are created and used on the
        // main UI thread. The output settings contain the documented CoreVideo
        // pixel-format key and an NSNumber value.
        unsafe {
            let item = AVPlayerItem::playerItemWithURL(&url, mtm);
            let output = video_output();
            item.addOutput(&output);
            let player = AVPlayer::playerWithPlayerItem(Some(&item), mtm);
            player.setVolume(volume.clamp(0.0, 1.0));
            player.setAutomaticallyWaitsToMinimizeStalling(false);
            player.seekToTime_toleranceBefore_toleranceAfter(
                CMTime::with_seconds(source_time, 60_000),
                CMTime::with_seconds(0.0, 60_000),
                CMTime::with_seconds(0.0, 60_000),
            );
            if realtime {
                player.playImmediatelyAtRate(1.0);
            } else {
                player.pause();
            }
            self.clip_id = Some(clip_id);
            self.source_start = source_time;
            self.source_end = source_time + length;
            self.rotation = asset.rotation;
            self.output = Some(output);
            self.player = Some(player);
        }
    }

    pub fn latest(&self) -> Option<DecodedFrame> {
        let player = self.player.as_ref()?;
        let output = self.output.as_ref()?;
        // SAFETY: Both AVPlayer and its video output live on the main thread.
        // CoreVideo memory is read only while its base address is locked.
        unsafe {
            let item_time = player.currentTime();
            if !output.hasNewPixelBufferForItemTime(item_time) {
                return None;
            }
            let mut display_time = item_time;
            let pixel_buffer = output
                .copyPixelBufferForItemTime_itemTimeForDisplay(item_time, &mut display_time)?;
            let flags = CVPixelBufferLockFlags::ReadOnly;
            if CVPixelBufferLockBaseAddress(&pixel_buffer, flags) != 0 {
                return None;
            }
            let width = CVPixelBufferGetWidth(&pixel_buffer);
            let height = CVPixelBufferGetHeight(&pixel_buffer);
            let display = CVImageBufferGetDisplaySize(&pixel_buffer);
            let row_bytes = CVPixelBufferGetBytesPerRow(&pixel_buffer);
            let base = CVPixelBufferGetBaseAddress(&pixel_buffer).cast::<u8>();
            if base.is_null() {
                let _ = CVPixelBufferUnlockBaseAddress(&pixel_buffer, flags);
                return None;
            }
            let bgra = slice::from_raw_parts(base, row_bytes * height);
            let mut rgba = vec![0_u8; width * height * 4];
            for y in 0..height {
                let source_row = &bgra[y * row_bytes..y * row_bytes + width * 4];
                let target_row = &mut rgba[y * width * 4..(y + 1) * width * 4];
                for (source, target) in source_row
                    .chunks_exact(4)
                    .zip(target_row.chunks_exact_mut(4))
                {
                    target.copy_from_slice(&[source[2], source[1], source[0], source[3]]);
                }
            }
            let _ = CVPixelBufferUnlockBaseAddress(&pixel_buffer, flags);
            let rotation = self
                .timeline
                .as_ref()
                .and_then(|project| {
                    let (_, _, clip) = project.clip_at(display_time.seconds())?;
                    project.asset(clip.asset_id).map(|asset| asset.rotation)
                })
                .unwrap_or(self.rotation);
            let (size, rgba) = rotate_rgba([width, height], rgba, rotation);
            let mut display_size = if display.width > 0.0 && display.height > 0.0 {
                [display.width as f32, display.height as f32]
            } else {
                [width as f32, height as f32]
            };
            if matches!(rotation, 90 | 270) {
                display_size.swap(0, 1);
            }
            Some(DecodedFrame {
                size,
                display_size,
                rgba,
            })
        }
    }

    pub fn source_time(&self) -> Option<f64> {
        let player = self.player.as_ref()?;
        // SAFETY: AVPlayer access is confined to the main UI thread.
        let seconds = unsafe { player.currentTime().seconds() };
        if seconds.is_finite() {
            Some(seconds.clamp(self.source_start, self.source_end))
        } else {
            Some(self.source_start)
        }
    }

    pub fn stop(&mut self) {
        if let Some(player) = self.player.take() {
            // SAFETY: AVPlayer access is confined to the main UI thread.
            unsafe { player.pause() };
        }
        self.output = None;
        self.clip_id = None;
        self.timeline = None;
    }
}

fn video_output() -> Retained<AVPlayerItemVideoOutput> {
    let pixel_format = NSNumber::numberWithUnsignedInt(kCVPixelFormatType_32BGRA);
    let attributes = NSDictionary::<NSString, AnyObject>::from_slices(
        &[ns_string!("PixelFormatType")],
        &[&*pixel_format],
    );
    // SAFETY: Called on the main thread with a documented CoreVideo pixel format.
    unsafe {
        let output = AVPlayerItemVideoOutput::initWithPixelBufferAttributes(
            AVPlayerItemVideoOutput::alloc(),
            Some(&attributes),
        );
        output.setSuppressesPlayerRendering(true);
        output
    }
}

fn rotate_rgba(size: [usize; 2], rgba: Vec<u8>, rotation: i32) -> ([usize; 2], Vec<u8>) {
    let [width, height] = size;
    let rotation = rotation.rem_euclid(360);
    if rotation == 0 || width == 0 || height == 0 {
        return (size, rgba);
    }

    let output_size = if rotation == 90 || rotation == 270 {
        [height, width]
    } else {
        size
    };
    let mut output = vec![0_u8; rgba.len()];
    for y in 0..height {
        for x in 0..width {
            let (target_x, target_y) = match rotation {
                90 => (height - 1 - y, x),
                180 => (width - 1 - x, height - 1 - y),
                270 => (y, width - 1 - x),
                _ => (x, y),
            };
            let source = (y * width + x) * 4;
            let target = (target_y * output_size[0] + target_x) * 4;
            output[target..target + 4].copy_from_slice(&rgba[source..source + 4]);
        }
    }
    (output_size, output)
}

#[cfg(test)]
mod tests {
    use super::rotate_rgba;

    fn pixels(values: &[u8]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| [*value, 0, 0, 255])
            .collect()
    }

    fn red_values(rgba: &[u8]) -> Vec<u8> {
        rgba.chunks_exact(4).map(|pixel| pixel[0]).collect()
    }

    #[test]
    fn rotates_rectangular_frame_clockwise() {
        // 1 2 3     4 1
        // 4 5 6  -> 5 2
        //           6 3
        let (size, rotated) = rotate_rgba([3, 2], pixels(&[1, 2, 3, 4, 5, 6]), 90);
        assert_eq!(size, [2, 3]);
        assert_eq!(red_values(&rotated), [4, 1, 5, 2, 6, 3]);
    }

    #[test]
    fn rotates_frame_counterclockwise() {
        let (size, rotated) = rotate_rgba([3, 2], pixels(&[1, 2, 3, 4, 5, 6]), 270);
        assert_eq!(size, [2, 3]);
        assert_eq!(red_values(&rotated), [3, 6, 2, 5, 1, 4]);
    }
}
