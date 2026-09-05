use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, SyncSender, sync_channel},
    },
    thread,
    time::Duration,
};

use uuid::Uuid;

use crate::{media::ffmpeg_binary, model::MediaAsset};

pub struct DecodedFrame {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
}

#[derive(Default)]
pub struct PreviewPlayer {
    pub clip_id: Option<Uuid>,
    receiver: Option<Receiver<DecodedFrame>>,
    stop: Option<Arc<AtomicBool>>,
}

impl PreviewPlayer {
    pub fn seek(
        &mut self,
        _clip_id: Uuid,
        _source_time: f64,
        _source_end: f64,
        _realtime: bool,
        _volume: f32,
    ) -> bool {
        false
    }

    pub fn start(
        &mut self,
        clip_id: Uuid,
        asset: &MediaAsset,
        source_time: f64,
        length: f64,
        realtime: bool,
        _volume: f32,
    ) {
        self.stop();
        // A fixed 16:9 decode surface makes the raw frame size predictable.
        // FFmpeg letterboxes the source inside it, preserving rotation, sample
        // aspect ratio, and portrait/landscape display aspect ratios.
        let (width, height) = (960, 540);
        let (sender, receiver) = sync_channel(2);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let path = asset.path.clone();
        thread::spawn(move || {
            decode_frames(
                Path::new(&path),
                source_time,
                length,
                width,
                height,
                realtime,
                sender,
                stop_thread,
            );
        });
        self.clip_id = Some(clip_id);
        self.receiver = Some(receiver);
        self.stop = Some(stop);
    }

    pub fn latest(&self) -> Option<DecodedFrame> {
        let receiver = self.receiver.as_ref()?;
        let mut latest = None;
        while let Ok(frame) = receiver.try_recv() {
            latest = Some(frame);
        }
        latest
    }

    pub fn source_time(&self) -> Option<f64> {
        None
    }

    pub fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        self.clip_id = None;
        self.receiver = None;
    }
}

impl Drop for PreviewPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_frames(
    path: &Path,
    source_time: f64,
    length: f64,
    width: u32,
    height: u32,
    realtime: bool,
    sender: SyncSender<DecodedFrame>,
    stop: Arc<AtomicBool>,
) {
    let mut command = Command::new(ffmpeg_binary());
    command
        .args(["-v", "error", "-ss"])
        .arg(format!("{source_time:.4}"))
        .arg("-i")
        .arg(path)
        .args(["-t", &format!("{:.4}", length.max(0.04)), "-an", "-vf"])
        .arg(format!(
            "scale={width}:{height}:force_original_aspect_ratio=decrease,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2:color=black,setsar=1,fps=30,format=rgba"
        ))
        .args(["-f", "rawvideo", "-pix_fmt", "rgba"])
        .arg(if realtime { "pipe:1" } else { "-frames:v" });
    if !realtime {
        command.args(["1", "pipe:1"]);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::null());

    let Ok(mut child) = command.spawn() else {
        return;
    };
    let Some(mut stdout) = child.stdout.take() else {
        return;
    };
    let frame_len = width as usize * height as usize * 4;
    loop {
        if stop.load(Ordering::Relaxed) {
            let _ = child.kill();
            break;
        }
        let mut rgba = vec![0; frame_len];
        if stdout.read_exact(&mut rgba).is_err() {
            break;
        }
        let _ = sender.try_send(DecodedFrame {
            size: [width as usize, height as usize],
            rgba,
        });
        if !realtime {
            break;
        }
        thread::sleep(Duration::from_millis(32));
    }
    let _ = child.kill();
    let _ = child.wait();
}
