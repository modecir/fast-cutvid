use std::{path::Path, ptr, slice};

use objc2::{AnyThread, MainThreadMarker, rc::Retained, runtime::AnyObject};
use objc2_av_foundation::{AVPlayer, AVPlayerItem, AVPlayerItemVideoOutput};
use objc2_core_media::CMTime;
use objc2_core_video::{
    CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight,
    CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
    CVPixelBufferUnlockBaseAddress, kCVPixelFormatType_32BGRA,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL, ns_string};
use uuid::Uuid;

use crate::model::MediaAsset;

pub struct DecodedFrame {
    pub size: [usize; 2],
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
}

impl PreviewPlayer {
    pub fn seek(
        &mut self,
        clip_id: Uuid,
        source_time: f64,
        source_end: f64,
        realtime: bool,
        volume: f32,
    ) -> bool {
        if self.clip_id != Some(clip_id) {
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

        // Request BGRA so CoreVideo gives us a single packed plane that can be
        // converted to egui's RGBA texture without a color-space decoder.
        let pixel_format = NSNumber::numberWithUnsignedInt(kCVPixelFormatType_32BGRA);
        let pixel_format_object: &AnyObject = &pixel_format;
        let attributes = NSDictionary::<NSString, AnyObject>::from_slices(
            &[ns_string!("PixelFormatType")],
            &[pixel_format_object],
        );

        // SAFETY: AVFoundation playback objects are created and used on the
        // main UI thread. The output settings contain the documented CoreVideo
        // pixel-format key and an NSNumber value.
        unsafe {
            let item = AVPlayerItem::playerItemWithURL(&url, mtm);
            let output = AVPlayerItemVideoOutput::initWithPixelBufferAttributes(
                AVPlayerItemVideoOutput::alloc(),
                Some(&attributes),
            );
            output.setSuppressesPlayerRendering(true);
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
            let pixel_buffer =
                output.copyPixelBufferForItemTime_itemTimeForDisplay(item_time, ptr::null_mut())?;
            let flags = CVPixelBufferLockFlags::ReadOnly;
            if CVPixelBufferLockBaseAddress(&pixel_buffer, flags) != 0 {
                return None;
            }
            let width = CVPixelBufferGetWidth(&pixel_buffer);
            let height = CVPixelBufferGetHeight(&pixel_buffer);
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
            let (size, rgba) = rotate_rgba([width, height], rgba, self.rotation);
            Some(DecodedFrame { size, rgba })
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
