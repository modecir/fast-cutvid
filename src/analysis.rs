//! Bounded background analysis, progressive delivery, and disposable disk caches.
use std::{
    collections::{HashMap, VecDeque, hash_map::DefaultHasher},
    fs,
    hash::{Hash, Hasher},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{Receiver, SyncSender, sync_channel},
    },
    thread,
};

use eframe::egui::ColorImage;
use uuid::Uuid;

use crate::{media, model::MediaAsset};

pub const FRAME_COUNT: usize = 16;

pub enum ImportEvent {
    Asset(MediaAsset),
    Metadata {
        asset_id: Uuid,
        width: u32,
        height: u32,
        rotation: i32,
    },
    Frame {
        asset_id: Uuid,
        index: usize,
        image: ColorImage,
    },
    Peaks {
        asset_id: Uuid,
        peaks: Vec<f32>,
    },
    Error(String),
    ProxyFinished {
        asset_id: Uuid,
        result: Result<MediaAsset, String>,
    },
    FramesFinished(Uuid),
    Finished,
}

struct Job {
    task: Box<dyn FnOnce() + Send>,
    priority: Arc<AtomicUsize>,
    paused: Arc<AtomicBool>,
}

struct Pool(Arc<(Mutex<VecDeque<Job>>, Condvar)>);

impl Pool {
    fn new(count: usize) -> Self {
        let queue = Arc::new((Mutex::new(VecDeque::<Job>::new()), Condvar::new()));
        for _ in 0..count {
            let queue = queue.clone();
            thread::spawn(move || {
                loop {
                    let (lock, ready) = &*queue;
                    let mut jobs = lock.lock().unwrap();
                    let job = loop {
                        let next = jobs
                            .iter()
                            .enumerate()
                            .filter(|(_, job)| !job.paused.load(Ordering::Relaxed))
                            .min_by_key(|(_, job)| job.priority.load(Ordering::Relaxed))
                            .map(|(index, _)| index);
                        if let Some(index) = next {
                            break jobs.remove(index).unwrap();
                        }
                        jobs = ready.wait(jobs).unwrap();
                    };
                    drop(jobs);
                    (job.task)();
                }
            });
        }
        Self(queue)
    }

    fn submit(&self, task: impl FnOnce() + Send + 'static) {
        self.submit_analysis(
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicBool::new(false)),
            task,
        );
    }

    fn submit_analysis(
        &self,
        priority: Arc<AtomicUsize>,
        paused: Arc<AtomicBool>,
        task: impl FnOnce() + Send + 'static,
    ) {
        let (lock, ready) = &*self.0;
        lock.lock().unwrap().push_back(Job {
            task: Box::new(task),
            priority,
            paused,
        });
        ready.notify_all();
    }

    fn wake(&self) {
        // Hold the queue lock across notification to avoid a lost wake between
        // a worker checking its pause flag and entering the condition wait.
        let _guard = self.0.0.lock().unwrap();
        self.0.1.notify_all();
    }
}

#[derive(Default)]
struct Priorities {
    ranks: HashMap<Uuid, Arc<AtomicUsize>>,
    focus: Vec<Uuid>,
}

impl Priorities {
    fn rank(&mut self, id: Uuid) -> Arc<AtomicUsize> {
        self.ranks
            .entry(id)
            .or_insert_with(|| {
                Arc::new(AtomicUsize::new(
                    self.focus
                        .iter()
                        .position(|asset| *asset == id)
                        .unwrap_or(usize::MAX),
                ))
            })
            .clone()
    }
}

struct Workers {
    metadata: Pool,
    video: Pool,
    audio: Pool,
    proxy: Pool,
}

fn workers() -> &'static Workers {
    static WORKERS: OnceLock<Workers> = OnceLock::new();
    WORKERS.get_or_init(|| Workers {
        metadata: Pool::new(1),
        video: Pool::new(2),
        audio: Pool::new(1),
        proxy: Pool::new(1),
    })
}

pub struct MediaAnalysis {
    sender: SyncSender<ImportEvent>,
    cancelled: Arc<AtomicBool>,
    cache_root: PathBuf,
    priorities: Arc<Mutex<Priorities>>,
    paused: Arc<AtomicBool>,
}

impl MediaAnalysis {
    pub fn new() -> (Self, Receiver<ImportEvent>) {
        Self::with_cache_root(cache_root())
    }

    fn with_cache_root(cache_root: PathBuf) -> (Self, Receiver<ImportEvent>) {
        let (sender, receiver) = sync_channel(64);
        let cleanup_root = cache_root.clone();
        workers()
            .metadata
            .submit(move || prune_cache(&cleanup_root));
        (
            Self {
                sender,
                cancelled: Arc::new(AtomicBool::new(false)),
                cache_root,
                priorities: Arc::new(Mutex::new(Priorities::default())),
                paused: Arc::new(AtomicBool::new(false)),
            },
            receiver,
        )
    }

    pub fn set_focus(&self, focus: Vec<Uuid>, playing: bool) {
        let mut priorities = self.priorities.lock().unwrap();
        if priorities.focus != focus {
            for (id, rank) in &priorities.ranks {
                rank.store(
                    focus
                        .iter()
                        .position(|asset| asset == id)
                        .unwrap_or(usize::MAX),
                    Ordering::Relaxed,
                );
            }
            priorities.focus = focus;
        }
        if self.paused.swap(playing, Ordering::Relaxed) != playing {
            workers().video.wake();
            workers().audio.wake();
            workers().proxy.wake();
        }
    }

    /// Restore evicted thumbnail textures from the existing disk cache.
    pub fn frames(&self, asset: MediaAsset) {
        let rank = self.priorities.lock().unwrap().rank(asset.id);
        schedule(
            asset,
            self.sender.clone(),
            self.cancelled.clone(),
            self.cache_root.clone(),
            rank,
            self.paused.clone(),
            false,
        );
    }

    pub fn prepare_proxy(&self, asset: MediaAsset) {
        let sender = self.sender.clone();
        let cancelled = self.cancelled.clone();
        let root = self.cache_root.join("proxies-v1");
        let priority = self.priorities.lock().unwrap().rank(asset.id);
        workers()
            .proxy
            .submit_analysis(priority, self.paused.clone(), move || {
                if cancelled.load(Ordering::Relaxed) {
                    return;
                }
                let result = (|| -> anyhow::Result<MediaAsset> {
                    let cache = Cache::new(&root, &asset)
                        .ok_or_else(|| anyhow::anyhow!("Source file is unavailable"))?;
                    fs::create_dir_all(&root)?;
                    let path = cache.path("preview.mp4");
                    let read = || -> anyhow::Result<MediaAsset> {
                        let mut proxy = media::probe(&path)?;
                        anyhow::ensure!(
                            (proxy.duration - asset.duration).abs() <= (1.0 / asset.fps).max(0.05)
                                && proxy.has_audio == asset.has_audio
                                && proxy.width.max(proxy.height) <= 960,
                            "Preview timing or dimensions do not match the source"
                        );
                        media::verify_proxy(Path::new(&asset.path), &path, asset.fps)?;
                        proxy.id = asset.id;
                        proxy.duration = asset.duration;
                        Ok(proxy)
                    };
                    if path.is_file()
                        && let Ok(proxy) = read()
                    {
                        return Ok(proxy);
                    }
                    let temp = tempfile::Builder::new().suffix(".mp4").tempfile_in(&root)?;
                    media::create_proxy(Path::new(&asset.path), temp.path(), || {
                        !cancelled.load(Ordering::Relaxed)
                    })?;
                    temp.persist(&path)?;
                    read()
                })()
                .map_err(|error| error.to_string());
                if !cancelled.load(Ordering::Relaxed) {
                    let _ = sender.send(ImportEvent::ProxyFinished {
                        asset_id: asset.id,
                        result,
                    });
                }
            });
    }

    pub fn import(&self, path: PathBuf) {
        self.prepare(path, None);
    }

    pub fn rebuild(&self, asset: MediaAsset) {
        self.prepare(PathBuf::from(&asset.path), Some(asset));
    }

    fn prepare(&self, path: PathBuf, existing: Option<MediaAsset>) {
        let sender = self.sender.clone();
        let cancelled = self.cancelled.clone();
        let root = self.cache_root.clone();
        let priorities = self.priorities.clone();
        let paused = self.paused.clone();
        workers().metadata.submit(move || {
            if cancelled.load(Ordering::Relaxed) {
                return;
            }
            let existing_id = existing.as_ref().map(|asset| asset.id);
            match media::probe(&path) {
                Ok(mut asset) => {
                    if let Some(existing) = existing {
                        let _ = sender.send(ImportEvent::Metadata {
                            asset_id: existing.id,
                            width: asset.width,
                            height: asset.height,
                            rotation: asset.rotation,
                        });
                        // Keep the project's time mapping, even if a source was replaced.
                        asset.id = existing.id;
                        asset.duration = existing.duration;
                    } else {
                        let _ = sender.send(ImportEvent::Asset(asset.clone()));
                    }
                    let rank = priorities.lock().unwrap().rank(asset.id);
                    schedule(asset, sender, cancelled, root, rank, paused, true);
                }
                Err(error) => {
                    if let Some(id) = existing_id {
                        let _ = sender.send(ImportEvent::FramesFinished(id));
                    }
                    let _ = sender.send(ImportEvent::Error(format!(
                        "Could not read {}: {error}",
                        path.display()
                    )));
                    let _ = sender.send(ImportEvent::Finished);
                }
            }
        });
    }
}

impl Drop for MediaAnalysis {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.paused.store(false, Ordering::Relaxed);
        workers().video.wake();
        workers().audio.wake();
        workers().proxy.wake();
    }
}

struct Analysis {
    asset: MediaAsset,
    sender: SyncSender<ImportEvent>,
    cancelled: Arc<AtomicBool>,
    cache: Option<Cache>,
    remaining: AtomicUsize,
    frames_remaining: AtomicUsize,
    reported_error: AtomicBool,
    import: bool,
}

impl Analysis {
    fn active(&self) -> bool {
        !self.cancelled.load(Ordering::Relaxed)
    }

    fn send(&self, event: ImportEvent) -> bool {
        self.active() && self.sender.send(event).is_ok()
    }

    fn finish(&self, result: anyhow::Result<()>) {
        if let Err(error) = result
            && self.active()
            && !self.reported_error.swap(true, Ordering::Relaxed)
        {
            self.send(ImportEvent::Error(format!(
                "Analysis failed for {}: {error}",
                self.asset.name
            )));
        }
        if self.remaining.fetch_sub(1, Ordering::AcqRel) == 1 && self.import {
            self.send(ImportEvent::Finished);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn schedule(
    asset: MediaAsset,
    sender: SyncSender<ImportEvent>,
    cancelled: Arc<AtomicBool>,
    root: PathBuf,
    priority: Arc<AtomicUsize>,
    paused: Arc<AtomicBool>,
    import: bool,
) {
    let analysis = Arc::new(Analysis {
        frames_remaining: AtomicUsize::new(FRAME_COUNT),
        remaining: AtomicUsize::new(FRAME_COUNT + usize::from(import && asset.has_audio)),
        cache: Cache::new(&root, &asset),
        asset,
        sender,
        cancelled,
        reported_error: AtomicBool::new(false),
        import,
    });
    for index in 0..FRAME_COUNT {
        let job = analysis.clone();
        workers()
            .video
            .submit_analysis(priority.clone(), paused.clone(), move || {
                let result = (|| {
                    if !job.active() {
                        return Ok(());
                    }
                    let filename = format!("{index}.jpg");
                    let cached = job
                        .cache
                        .as_ref()
                        .and_then(|cache| cache.read(&filename, 1_000_000))
                        .and_then(|bytes| media::thumbnail_image(&bytes).ok());
                    let image = match cached {
                        Some(image) => image,
                        None => {
                            let timestamp =
                                thumbnail_time(job.asset.duration, job.asset.fps, index);
                            let bytes = media::thumbnail(Path::new(&job.asset.path), timestamp)?;
                            let image = media::thumbnail_image(&bytes)?;
                            if job.active()
                                && let Some(cache) = &job.cache
                            {
                                cache.write(&filename, &bytes);
                            }
                            image
                        }
                    };
                    job.send(ImportEvent::Frame {
                        asset_id: job.asset.id,
                        index,
                        image,
                    });
                    Ok(())
                })();
                if job.frames_remaining.fetch_sub(1, Ordering::AcqRel) == 1 {
                    job.send(ImportEvent::FramesFinished(job.asset.id));
                }
                job.finish(result);
            });
    }
    if import && analysis.asset.has_audio {
        workers().audio.submit_analysis(priority, paused, move || {
            let job = analysis;
            let result = (|| {
                if !job.active() {
                    return Ok(());
                }
                let cached = job
                    .cache
                    .as_ref()
                    .and_then(|cache| cache.read("wave", 64 * 1024 * 1024))
                    .and_then(|bytes| decode_peaks(&bytes));
                if let Some(peaks) = cached {
                    job.send(ImportEvent::Peaks {
                        asset_id: job.asset.id,
                        peaks,
                    });
                } else {
                    let peaks = media::waveform(Path::new(&job.asset.path), |peaks| {
                        job.send(ImportEvent::Peaks {
                            asset_id: job.asset.id,
                            peaks,
                        })
                    })?;
                    if job.active()
                        && let Some(cache) = &job.cache
                    {
                        let bytes: Vec<_> =
                            peaks.iter().flat_map(|peak| peak.to_le_bytes()).collect();
                        cache.write("wave", &bytes);
                    }
                }
                Ok(())
            })();
            job.finish(result);
        });
    }
}

fn thumbnail_time(duration: f64, fps: f64, index: usize) -> f64 {
    // The last midpoint of a very short clip can lie after its final frame.
    (duration * (index as f64 + 0.5) / FRAME_COUNT as f64)
        .min((duration - 1.0 / fps.max(1.0)).max(0.0))
}

fn decode_peaks(bytes: &[u8]) -> Option<Vec<f32>> {
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    bytes
        .chunks_exact(4)
        .map(|bytes| {
            let peak = f32::from_le_bytes(bytes.try_into().unwrap());
            (peak.is_finite() && peak >= 0.0).then_some(peak)
        })
        .collect()
}

struct Cache {
    root: PathBuf,
    key: String,
}

impl Cache {
    fn new(root: &Path, asset: &MediaAsset) -> Option<Self> {
        let path = fs::canonicalize(&asset.path).ok()?;
        let metadata = fs::metadata(&path).ok()?;
        let mut hash = DefaultHasher::new();
        path.hash(&mut hash);
        metadata.len().hash(&mut hash);
        metadata.modified().ok()?.hash(&mut hash);
        asset.duration.to_bits().hash(&mut hash);
        asset.fps.to_bits().hash(&mut hash);
        (asset.width, asset.height, asset.rotation, FRAME_COUNT).hash(&mut hash);
        Some(Self {
            root: root.to_owned(),
            key: format!("{:016x}", hash.finish()),
        })
    }

    fn path(&self, suffix: &str) -> PathBuf {
        self.root.join(format!("{}-{suffix}", self.key))
    }

    fn read(&self, suffix: &str, limit: u64) -> Option<Vec<u8>> {
        let path = self.path(suffix);
        if fs::metadata(&path).ok()?.len() > limit {
            return None;
        }
        fs::read(path).ok()
    }

    fn write(&self, suffix: &str, bytes: &[u8]) {
        // Cache failures are harmless. Atomic replacement never exposes a partial file.
        let _ = (|| -> anyhow::Result<()> {
            fs::create_dir_all(&self.root)?;
            let mut temp = tempfile::NamedTempFile::new_in(&self.root)?;
            temp.write_all(bytes)?;
            temp.persist(self.path(suffix))?;
            Ok(())
        })();
    }
}

fn cache_root() -> PathBuf {
    if let Some(path) = std::env::var_os("FASTCUT_CACHE_DIR") {
        return PathBuf::from(path).join("analysis-v1");
    }
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Caches"));
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")));
    base.unwrap_or_else(std::env::temp_dir)
        .join("fastCutVid/analysis-v1")
}

fn prune_cache(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut files = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name();
            let name = name.to_str()?;
            if !(name.ends_with(".jpg") || name.ends_with("-wave")) {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            Some((metadata.modified().ok()?, metadata.len(), entry.path()))
        })
        .collect::<Vec<_>>();
    let mut size: u64 = files.iter().map(|(_, size, _)| size).sum();
    files.sort_by_key(|(modified, _, _)| *modified);
    for (_, bytes, path) in files {
        if size <= 256 * 1024 * 1024 {
            break;
        }
        if fs::remove_file(path).is_ok() {
            size = size.saturating_sub(bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        process::Command,
        time::{Duration, Instant},
    };

    fn fixture(path: &Path, size: &str, duration: &str, sar: &str) {
        assert!(
            Command::new(media::ffmpeg_binary())
                .args([
                    "-v",
                    "error",
                    "-nostdin",
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("testsrc2=size={size}:rate=30:duration={duration}"),
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("sine=frequency=440:duration={duration}"),
                    "-vf",
                    &format!("setsar={sar}"),
                    "-c:v",
                    "libx264",
                    "-preset",
                    "ultrafast",
                    "-threads",
                    "2",
                    "-g",
                    "30",
                    "-c:a",
                    "aac",
                    "-shortest",
                ])
                .arg(path)
                .status()
                .expect("FFmpeg is required for media regression tests")
                .success()
        );
    }

    #[test]
    fn queued_work_obeys_live_priority_and_playback_pause() {
        let pool = Pool::new(1);
        let paused = Arc::new(AtomicBool::new(true));
        let (tx, rx) = sync_channel(3);
        let ranks: Vec<_> = (0..3).map(|i| Arc::new(AtomicUsize::new(i))).collect();
        for (i, rank) in ranks.iter().enumerate() {
            let tx = tx.clone();
            pool.submit_analysis(rank.clone(), paused.clone(), move || {
                tx.send(i).unwrap();
            });
        }
        assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
        ranks[2].store(0, Ordering::Relaxed);
        ranks[0].store(5, Ordering::Relaxed);
        paused.store(false, Ordering::Relaxed);
        pool.wake();
        let order: Vec<_> = (0..3)
            .map(|_| rx.recv_timeout(Duration::from_secs(2)).unwrap())
            .collect();
        assert_eq!(order, [2, 1, 0]);
    }

    #[test]
    fn lightweight_previews_keep_timing_geometry_audio_and_reuse_cache() {
        let temp = tempfile::tempdir().unwrap();
        for (size, sar) in [("360x640", "1"), ("640x360", "2")] {
            let path = temp.path().join(format!("{size}.mp4"));
            fixture(&path, size, "1.2", sar);
            let source = media::probe(&path).unwrap();
            let before = fs::read(&path).unwrap();
            let (analysis, rx) = MediaAnalysis::with_cache_root(temp.path().join("cache"));
            analysis.prepare_proxy(source.clone());
            let proxy = match rx.recv_timeout(Duration::from_secs(15)).unwrap() {
                ImportEvent::ProxyFinished { asset_id, result } => {
                    assert_eq!(asset_id, source.id);
                    result.unwrap()
                }
                _ => panic!("Expected proxy completion"),
            };
            assert_eq!(proxy.id, source.id);
            assert_eq!(proxy.duration, source.duration);
            assert!(proxy.width.max(proxy.height) <= 960);
            let expected_ratio =
                source.width as f64 * sar.parse::<f64>().unwrap() / source.height as f64;
            assert!((proxy.width as f64 / proxy.height as f64 - expected_ratio).abs() < 0.02);
            assert!(proxy.has_audio);
            assert!(
                media::waveform(Path::new(&proxy.path), |_| true)
                    .unwrap()
                    .iter()
                    .any(|p| *p > 0.01)
            );
            assert_eq!(
                fs::read(&path).unwrap(),
                before,
                "Proxy preparation changed source media"
            );
            let modified = fs::metadata(&proxy.path).unwrap().modified().unwrap();
            analysis.prepare_proxy(source);
            match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
                ImportEvent::ProxyFinished { result, .. } => {
                    assert_eq!(result.unwrap().path, proxy.path);
                    assert_eq!(
                        fs::metadata(&proxy.path).unwrap().modified().unwrap(),
                        modified
                    );
                }
                _ => panic!("Expected cached proxy completion"),
            }
        }
    }

    #[test]
    fn thumbnails_preserve_portrait_square_rotation_and_sample_aspect() {
        let temp = tempfile::tempdir().unwrap();
        for (name, size, sar, expected) in [
            ("portrait", "180x320", "1", [135, 240]),
            ("square", "240x240", "1", [240, 240]),
            ("anamorphic", "320x240", "2", [240, 90]),
        ] {
            let path = temp.path().join(format!("{name}.mp4"));
            fixture(&path, size, "0.2", sar);
            let asset = media::probe(&path).unwrap();
            for index in [0, FRAME_COUNT - 1] {
                let bytes =
                    media::thumbnail(&path, thumbnail_time(asset.duration, asset.fps, index))
                        .unwrap();
                assert_eq!(
                    media::thumbnail_image(&bytes).unwrap().size,
                    expected,
                    "{name}, frame {index}"
                );
            }
        }
        let rotated = temp.path().join("rotated.mp4");
        let output = Command::new(media::ffmpeg_binary())
            .args(["-v", "error", "-display_rotation", "90", "-i"])
            .arg(temp.path().join("portrait.mp4"))
            .args(["-c", "copy"])
            .arg(&rotated)
            .output()
            .unwrap();
        // Older FFmpeg uses output metadata; newer versions need the input
        // display_rotation option to write the rotation matrix.
        let output = if !output.status.success()
            && String::from_utf8_lossy(&output.stderr)
                .contains("Unrecognized option 'display_rotation'")
        {
            Command::new(media::ffmpeg_binary())
                .args(["-v", "error", "-i"])
                .arg(temp.path().join("portrait.mp4"))
                .args(["-c", "copy", "-metadata:s:v:0", "rotate=90"])
                .arg(&rotated)
                .output()
                .unwrap()
        } else {
            output
        };
        assert!(
            output.status.success(),
            "Could not create rotated fixture: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let frame = media::thumbnail_image(&media::thumbnail(&rotated, 0.0).unwrap()).unwrap();
        assert_eq!(frame.size, [240, 135]);
    }

    struct Received {
        frames: usize,
        peaks: Vec<f32>,
        first_frame: Duration,
        first_peaks: Duration,
        elapsed: Duration,
    }

    fn receive(rx: &Receiver<ImportEvent>) -> Received {
        let start = Instant::now();
        let mut result = Received {
            frames: 0,
            peaks: Vec::new(),
            first_frame: Duration::ZERO,
            first_peaks: Duration::ZERO,
            elapsed: Duration::ZERO,
        };
        loop {
            match rx
                .recv_timeout(Duration::from_secs(30))
                .expect("analysis stalled")
            {
                ImportEvent::Frame { index, image, .. } => {
                    if result.frames == 0 {
                        result.first_frame = start.elapsed();
                    }
                    assert!(index < FRAME_COUNT);
                    assert_eq!(image.size, [240, 135]);
                    result.frames += 1;
                }
                ImportEvent::Peaks { peaks, .. } => {
                    if result.peaks.is_empty() {
                        result.first_peaks = start.elapsed();
                    }
                    result.peaks.extend(peaks);
                }
                ImportEvent::Error(error) => panic!("{error}"),
                ImportEvent::Finished => {
                    result.elapsed = start.elapsed();
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(result.frames, FRAME_COUNT);
        result
    }

    #[test]
    fn analysis_streams_results_reuses_cache_and_repairs_corruption() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("landscape.mp4");
        fixture(&path, "1280x720", "12", "1");
        let asset = media::probe(&path).unwrap();
        let root = temp.path().join("cache");
        let (analysis, rx) = MediaAnalysis::with_cache_root(root.clone());
        analysis.rebuild(asset.clone());
        let cold = receive(&rx);
        assert!(
            (600..=604).contains(&cold.peaks.len()),
            "{} peaks",
            cold.peaks.len()
        );
        assert!(cold.first_frame < cold.elapsed);
        assert!(cold.first_peaks < cold.elapsed);
        let cache = Cache::new(&root, &asset).unwrap();
        let modified = fs::metadata(cache.path("0.jpg"))
            .unwrap()
            .modified()
            .unwrap();
        analysis.rebuild(asset.clone());
        let warm = receive(&rx);
        assert_eq!(cold.peaks, warm.peaks);
        assert_eq!(
            modified,
            fs::metadata(cache.path("0.jpg"))
                .unwrap()
                .modified()
                .unwrap(),
            "warm cache was decoded again"
        );

        fs::write(cache.path("0.jpg"), b"interrupted frame").unwrap();
        fs::write(cache.path("wave"), b"bad").unwrap();
        analysis.rebuild(asset.clone());
        let repaired = receive(&rx);
        assert_eq!(repaired.peaks, cold.peaks);
        assert!(media::thumbnail_image(&fs::read(cache.path("0.jpg")).unwrap()).is_ok());

        // Replacing media at the same path must invalidate cached analysis.
        let old_key = cache.key;
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"changed")
            .unwrap();
        assert_ne!(old_key, Cache::new(&root, &asset).unwrap().key);
        println!(
            "Analysis: first frame {:?}, first waveform {:?}, complete {:?}; warm cache {:?}",
            cold.first_frame, cold.first_peaks, cold.elapsed, warm.elapsed
        );
    }

    #[test]
    fn cancelling_analysis_disconnects_queued_work() {
        let temp = tempfile::tempdir().unwrap();
        let (analysis, rx) = MediaAnalysis::with_cache_root(temp.path().to_owned());
        let token = analysis.cancelled.clone();
        analysis.import(temp.path().join("missing.mp4"));
        drop(analysis);
        assert!(token.load(Ordering::Relaxed));
        // A job already probing may finish, but the queue must drain promptly.
        while rx.recv_timeout(Duration::from_secs(5)).is_ok() {}
        assert!(matches!(
            rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ));
    }
}
