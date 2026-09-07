use std::{
    io::{self, BufRead, BufReader},
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

use crate::{
    media::{display_scale, ffmpeg_binary},
    model::MediaAsset,
};

pub struct DecodedFrame {
    pub size: [usize; 2],
    pub display_size: [f32; 2],
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
        // Self-describing frames preserve portrait, square, and anamorphic media.
        let (sender, receiver) = sync_channel(2);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let path = asset.path.clone();
        thread::spawn(move || {
            decode_frames(
                Path::new(&path),
                source_time,
                length,
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

fn decode_frames(
    path: &Path,
    source_time: f64,
    length: f64,
    realtime: bool,
    sender: SyncSender<DecodedFrame>,
    stop: Arc<AtomicBool>,
) {
    let mut command = Command::new(ffmpeg_binary());
    command
        .args(["-v", "error", "-nostdin", "-threads", "2", "-ss"])
        .arg(format!("{source_time:.4}"))
        .arg("-i")
        .arg(path)
        .args(["-t", &format!("{:.4}", length.max(0.04)), "-an", "-vf"])
        .arg(format!("{},fps=30", display_scale(960)))
        .args([
            "-filter_threads",
            "1",
            "-threads",
            "1",
            "-f",
            "image2pipe",
            "-c:v",
            "ppm",
        ])
        .arg(if realtime { "pipe:1" } else { "-frames:v" });
    if !realtime {
        command.args(["1", "pipe:1"]);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::null());

    let Ok(mut child) = command.spawn() else {
        return;
    };
    let Some(stdout) = child.stdout.take() else {
        return;
    };
    let mut stdout = BufReader::new(stdout);
    loop {
        if stop.load(Ordering::Relaxed) {
            let _ = child.kill();
            break;
        }
        let Ok(frame) = read_frame(&mut stdout) else {
            break;
        };
        let _ = sender.try_send(frame);
        if !realtime {
            break;
        }
        thread::sleep(Duration::from_millis(32));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// FFmpeg's PPM encoder supplies dimensions per frame, without a padded canvas.
fn read_frame(reader: &mut impl BufRead) -> io::Result<DecodedFrame> {
    fn token(reader: &mut impl BufRead) -> io::Result<String> {
        let mut bytes = Vec::new();
        loop {
            let mut byte = [0];
            reader.read_exact(&mut byte)?;
            if byte[0].is_ascii_whitespace() {
                if !bytes.is_empty() {
                    break;
                }
            } else {
                bytes.push(byte[0]);
                if bytes.len() > 32 {
                    return Err(io::ErrorKind::InvalidData.into());
                }
            }
        }
        String::from_utf8(bytes).map_err(|_| io::ErrorKind::InvalidData.into())
    }
    if token(reader)? != "P6" {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let width = token(reader)?
        .parse::<usize>()
        .map_err(|_| io::ErrorKind::InvalidData)?;
    let height = token(reader)?
        .parse::<usize>()
        .map_err(|_| io::ErrorKind::InvalidData)?;
    if width == 0 || height == 0 || width > 960 || height > 960 || token(reader)? != "255" {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut rgb = vec![0; width * height * 3];
    reader.read_exact(&mut rgb)?;
    let mut rgba = Vec::with_capacity(width * height * 4);
    for pixel in rgb.chunks_exact(3) {
        rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
    }
    Ok(DecodedFrame {
        size: [width, height],
        display_size: [width as f32, height as f32],
        rgba,
    })
}
