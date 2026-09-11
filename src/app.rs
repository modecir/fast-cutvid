use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, channel},
    thread,
    time::Instant,
};

use eframe::egui::{
    self, Align, Align2, Color32, ColorImage, FontId, Frame, Id, Key, Layout, Margin, Pos2, Rect,
    RichText, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2,
};
use uuid::Uuid;

use crate::{
    analysis::{FRAME_COUNT, ImportEvent, MediaAnalysis},
    media::{PEAKS_PER_SECOND, format_time},
    model::{Clip, Project, TimelineIndex},
    player::PreviewPlayer,
    render::{RenderEvent, render_project},
};

const BG: Color32 = Color32::from_rgb(13, 15, 20);
const PANEL: Color32 = Color32::from_rgb(20, 23, 30);
const SURFACE: Color32 = Color32::from_rgb(29, 33, 42);
const BORDER: Color32 = Color32::from_rgb(48, 54, 67);
const TEXT_MUTED: Color32 = Color32::from_rgb(145, 151, 165);
const ACCENT: Color32 = Color32::from_rgb(113, 240, 182);
const PURPLE: Color32 = Color32::from_rgb(116, 101, 245);
const TIMELINE_LEADING_SPACE: f32 = 55.0;

#[derive(Default)]
struct Waveform {
    peaks: Vec<f32>,
    maximum: f32,
}

impl Waveform {
    fn extend(&mut self, peaks: Vec<f32>) {
        self.maximum = peaks.iter().copied().fold(self.maximum, f32::max);
        self.peaks.extend(peaks);
    }
}

pub struct FastCutApp {
    project: Project,
    index: TimelineIndex,
    revision: u64,
    project_path: Option<PathBuf>,
    selected_clip: Option<Uuid>,
    selected_asset: Option<Uuid>,
    playhead: f64,
    playing: bool,
    playback_started: Option<(Instant, f64)>,
    pixels_per_second: f32,
    timeline_active: bool,
    filmstrips: HashMap<Uuid, Vec<Option<TextureHandle>>>,
    waveforms: HashMap<Uuid, Waveform>,
    visible_assets: Vec<Uuid>,
    resident_assets: Vec<Uuid>,
    frames_pending: HashSet<Uuid>,
    proxies: HashMap<Uuid, crate::model::MediaAsset>,
    proxies_pending: HashSet<Uuid>,
    proxies_changed: bool,
    use_proxies: bool,
    preview_texture: Option<TextureHandle>,
    preview_display_size: Option<Vec2>,
    player: PreviewPlayer,
    status: String,
    render_rx: Option<Receiver<anyhow::Result<RenderEvent>>>,
    render_progress: Option<f32>,
    analysis: MediaAnalysis,
    analysis_failed: bool,
    import_rx: Receiver<ImportEvent>,
    imports_pending: usize,
    dirty: bool,
    show_shortcuts: bool,
    show_help: bool,
    logo_texture: TextureHandle,
}

impl FastCutApp {
    pub fn new(cc: &eframe::CreationContext<'_>, opening_paths: Vec<PathBuf>) -> Self {
        #[cfg(target_os = "macos")]
        {
            crate::menu_macos::install(&cc.egui_ctx);
            crate::documents_macos::set_context(&cc.egui_ctx);
        }
        configure_style(&cc.egui_ctx);
        let logo_texture = load_logo_texture(&cc.egui_ctx);
        let (analysis, import_rx) = MediaAnalysis::new();
        let mut app = Self {
            project: Project::default(),
            index: TimelineIndex::default(),
            revision: 0,
            project_path: None,
            selected_clip: None,
            selected_asset: None,
            playhead: 0.0,
            playing: false,
            playback_started: None,
            pixels_per_second: 36.0,
            timeline_active: false,
            filmstrips: HashMap::new(),
            waveforms: HashMap::new(),
            visible_assets: Vec::new(),
            resident_assets: Vec::new(),
            frames_pending: HashSet::new(),
            proxies: HashMap::new(),
            proxies_pending: HashSet::new(),
            proxies_changed: false,
            use_proxies: true,
            preview_texture: None,
            preview_display_size: None,
            player: PreviewPlayer::default(),
            status: "Ready — import media to begin".to_owned(),
            render_rx: None,
            render_progress: None,
            analysis,
            analysis_failed: false,
            import_rx,
            imports_pending: 0,
            dirty: false,
            show_shortcuts: false,
            show_help: false,
            logo_texture,
        };
        // Open the project first regardless of argument order, so its channel
        // reset cannot discard imports requested by the same launch.
        if let Some(path) = opening_paths.iter().find(|path| is_project_path(path))
            && !app.open_project_path(path.clone())
        {
            return app;
        }
        let videos = opening_paths
            .into_iter()
            .filter(|path| is_video_path(path))
            .collect::<Vec<_>>();
        if !videos.is_empty() {
            app.queue_video_imports(videos);
        }
        app
    }

    fn project_changed(&mut self) {
        self.index = TimelineIndex::new(&self.project);
        self.revision = self.revision.wrapping_add(1);
        self.playhead = self.playhead.min(self.index.duration());
        if self.project.clips.is_empty() {
            self.playing = false;
            self.playback_started = None;
            self.player.stop();
        }
        self.dirty = true;
    }

    fn import_media(&mut self) {
        let Some(paths) = rfd::FileDialog::new()
            .add_filter(
                "Video",
                &[
                    "mp4", "mov", "mkv", "webm", "avi", "m4v", "mpeg", "mpg", "mts", "m2ts", "wmv",
                    "flv",
                ],
            )
            .pick_files()
        else {
            return;
        };
        self.queue_video_imports(paths);
    }

    fn queue_video_imports(&mut self, paths: Vec<PathBuf>) {
        let paths = paths
            .into_iter()
            .filter(|path| is_video_path(path))
            .collect::<Vec<_>>();
        if paths.is_empty() {
            self.status = "No supported video files were dropped".to_owned();
            return;
        }
        self.imports_pending += paths.len();
        self.status = format!(
            "Loading {} video{} in the background…",
            paths.len(),
            if paths.len() == 1 { "" } else { "s" }
        );
        if self.imports_pending == paths.len() {
            self.analysis_failed = false;
        }
        for path in paths {
            self.analysis.import(path);
        }
    }

    fn add_asset_to_timeline(&mut self, asset_id: Uuid) {
        let Some(asset) = self.index.asset(&self.project, asset_id) else {
            return;
        };
        let clip = Clip::from_asset(asset);
        self.selected_clip = Some(clip.id);
        self.project.clips.push(clip);
        self.project_changed();
        self.status = "Clip added to timeline".to_owned();
        if self.project.clips.len() == 1 {
            self.playhead = 0.0;
            self.load_preview_at_playhead(false);
        }
    }

    fn split_at_playhead(&mut self) {
        let Some((index, clip_start, clip)) = self.index.clip_at(&self.project, self.playhead)
        else {
            return;
        };
        let local = self.playhead - clip_start;
        if local <= 0.05 || local >= clip.duration() - 0.05 {
            self.status = "Move the playhead inside a clip to split".to_owned();
            return;
        }
        let split_source = clip.source_in + local;
        let mut right = clip.clone();
        right.id = Uuid::new_v4();
        right.source_in = split_source;
        self.project.clips[index].source_out = split_source;
        self.project.clips.insert(index + 1, right.clone());
        self.selected_clip = Some(right.id);
        self.project_changed();
        self.status = format!("Split at {}", format_time(self.playhead));
    }

    fn delete_selected(&mut self) {
        let Some(id) = self.selected_clip else { return };
        if let Some(index) = self.index.clip_index(id) {
            self.project.clips.remove(index);
            self.selected_clip = self
                .project
                .clips
                .get(index.saturating_sub(1))
                .map(|clip| clip.id);
            self.project_changed();
            self.playhead = self.playhead.min(self.index.duration());
            self.status = "Clip removed".to_owned();
            self.load_preview_at_playhead(false);
        }
    }

    fn move_selected(&mut self, direction: isize) {
        let Some(id) = self.selected_clip else { return };
        let Some(index) = self.index.clip_index(id) else {
            return;
        };
        let target = index as isize + direction;
        if target >= 0 && target < self.project.clips.len() as isize {
            self.project.clips.swap(index, target as usize);
            self.project_changed();
        }
    }

    fn toggle_playback(&mut self) {
        if self.project.clips.is_empty() {
            return;
        }
        self.playing = !self.playing;
        if self.playing {
            if self.playhead >= self.index.duration() {
                self.playhead = 0.0;
            }
            self.load_preview_at_playhead(true);
            self.playback_started = Some((Instant::now(), self.playhead));
        } else {
            self.playback_started = None;
            self.player.pause();
            if self.proxies_changed {
                self.load_preview_at_playhead(false);
            }
        }
    }

    fn load_preview_at_playhead(&mut self, realtime: bool) {
        if self.proxies_changed {
            self.revision = self.revision.wrapping_add(1);
            self.proxies_changed = false;
        }
        #[cfg(target_os = "macos")]
        {
            let preview;
            let project = if self.use_proxies
                && !self.proxies.is_empty()
                && !self.player.timeline_matches(self.revision)
            {
                preview = {
                    let mut project = self.project.clone();
                    for asset in &mut project.assets {
                        if let Some(proxy) = self.proxies.get(&asset.id) {
                            *asset = proxy.clone();
                        }
                    }
                    project
                };
                &preview
            } else {
                &self.project
            };
            if let Err(error) =
                self.player
                    .prepare_timeline(project, self.revision, self.playhead, realtime)
            {
                self.player.stop();
                self.playing = false;
                self.status = format!("Playback failed: {error}");
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let Some((_, start, clip)) = self.index.clip_at(&self.project, self.playhead) else {
                self.player.stop();
                return;
            };
            let Some(asset) = self.index.asset(&self.project, clip.asset_id) else {
                return;
            };
            let asset = if self.use_proxies {
                self.proxies.get(&asset.id).unwrap_or(asset)
            } else {
                asset
            };
            let offset = (self.playhead - start).clamp(0.0, clip.duration());
            self.player.start(
                clip.id,
                asset,
                clip.source_in + offset,
                (clip.duration() - offset).max(0.04),
                realtime,
                if clip.muted { 0.0 } else { clip.audio_gain },
            );
        }
    }

    fn save_project(&mut self, save_as: bool) {
        let path = if !save_as {
            self.project_path.clone()
        } else {
            None
        }
        .or_else(|| {
            rfd::FileDialog::new()
                .set_file_name(format!("{}.fastcut", safe_name(&self.project.name)))
                .add_filter("fastCutVid project", &["fastcut"])
                .save_file()
        });
        let Some(path) = path else { return };
        match self.project.save(&path) {
            Ok(()) => {
                self.project_path = Some(path.clone());
                self.dirty = false;
                self.status = format!("Saved {}", path.display());
            }
            Err(error) => self.status = format!("Save failed: {error}"),
        }
    }

    fn export_cuts(&mut self) {
        let media = self
            .project
            .clips
            .first()
            .and_then(|clip| self.index.asset(&self.project, clip.asset_id))
            .or_else(|| self.project.assets.first());
        let default_path = cuts_export_path(
            self.project_path.as_deref(),
            media.map(|asset| Path::new(&asset.path)),
            &self.project.name,
        );
        let mut dialog = rfd::FileDialog::new()
            .set_file_name(
                default_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy(),
            )
            .add_filter("fastCutVid cuts", &["fastcut"]);
        if let Some(directory) = default_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
        {
            dialog = dialog.set_directory(directory);
        }
        let Some(path) = dialog.save_file() else {
            return;
        };
        // Export a copy: subsequent exports still use the original project or
        // media location, and Save continues targeting the editable project.
        match self.project.save(&path) {
            Ok(()) => self.status = format!("Exported cuts to {}", path.display()),
            Err(error) => self.status = format!("Export cuts failed: {error}"),
        }
    }

    fn open_project(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("fastCutVid project", &["fastcut", "json"])
            .pick_file()
        else {
            return;
        };
        self.open_project_path(path);
    }

    fn open_project_path(&mut self, path: PathBuf) -> bool {
        match Project::load(&path) {
            Ok(project) => {
                // Cancel old jobs and disconnect their results before switching projects.
                let (analysis, import_rx) = MediaAnalysis::new();
                self.analysis = analysis;
                self.analysis_failed = false;
                self.import_rx = import_rx;
                self.imports_pending = 0;
                self.project = project;
                self.project_changed();
                self.project_path = Some(path.clone());
                self.selected_clip = self.project.clips.first().map(|clip| clip.id);
                self.selected_asset = self.project.assets.first().map(|asset| asset.id);
                self.playhead = 0.0;
                self.playing = false;
                self.playback_started = None;
                self.player.stop();
                self.preview_texture = None;
                self.preview_display_size = None;
                self.filmstrips.clear();
                self.frames_pending.clear();
                self.proxies.clear();
                self.proxies_pending.clear();
                self.proxies_changed = false;
                self.resident_assets.clear();
                self.visible_assets.clear();
                self.waveforms.clear();
                let assets = self.project.assets.clone();
                self.imports_pending += assets.len();
                for asset in assets {
                    self.frames_pending.insert(asset.id);
                    self.analysis.rebuild(asset);
                }
                let missing = self
                    .project
                    .assets
                    .iter()
                    .filter(|asset| !PathBuf::from(&asset.path).is_file())
                    .count();
                self.status = if missing == 0 {
                    format!(
                        "Opened {} — loading media previews in the background",
                        path.display()
                    )
                } else {
                    format!(
                        "Opened {} — {missing} media file{} missing",
                        path.display(),
                        if missing == 1 { " is" } else { "s are" }
                    )
                };
                self.dirty = false;
                self.load_preview_at_playhead(false);
                true
            }
            Err(error) => {
                self.status = format!("Open failed: {error}");
                false
            }
        }
    }

    fn handle_file_drop(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|input| input.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }

        let mut timelines = Vec::new();
        let mut videos = Vec::new();
        let mut unsupported = 0;
        for file in dropped {
            let Some(path) = file.path else {
                unsupported += 1;
                continue;
            };
            if is_project_path(&path) {
                timelines.push(path);
            } else if is_video_path(&path) {
                videos.push(path);
            } else {
                unsupported += 1;
            }
        }

        if let Some(path) = timelines.into_iter().next() {
            self.open_project_path(path);
        }
        if !videos.is_empty() {
            self.queue_video_imports(videos);
        } else if unsupported > 0 {
            self.status = format!(
                "Ignored {unsupported} unsupported dropped file{}",
                if unsupported == 1 { "" } else { "s" }
            );
        }
    }

    fn file_drop_overlay(&self, ctx: &egui::Context) {
        let hovered = ctx.input(|input| !input.raw.hovered_files.is_empty());
        if !hovered {
            return;
        }
        let rect = ctx.screen_rect().shrink(18.0);
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            Id::new("file_drop_overlay"),
        ));
        painter.rect_filled(rect, 12.0, Color32::from_black_alpha(220));
        painter.rect_stroke(rect, 12.0, Stroke::new(2.0_f32, ACCENT), StrokeKind::Inside);
        painter.text(
            rect.center() - Vec2::new(0.0, 12.0),
            Align2::CENTER_CENTER,
            "DROP TO IMPORT",
            FontId::proportional(22.0),
            Color32::WHITE,
        );
        painter.text(
            rect.center() + Vec2::new(0.0, 18.0),
            Align2::CENTER_CENTER,
            "Videos are added to Media  •  .fastcut files open as projects",
            FontId::proportional(13.0),
            TEXT_MUTED,
        );
    }

    fn export_video(&mut self) {
        if self.project.clips.is_empty() || self.render_rx.is_some() {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("{}.mp4", safe_name(&self.project.name)))
            .add_filter("MP4 video", &["mp4"])
            .save_file()
        else {
            return;
        };
        let project = self.project.clone();
        let (tx, rx) = channel();
        self.render_rx = Some(rx);
        self.render_progress = Some(0.0);
        self.status = "Starting render…".to_owned();
        thread::spawn(move || {
            let result = render_project(&project, &path, |event| {
                let _ = tx.send(Ok(event));
            });
            if let Err(error) = result {
                let _ = tx.send(Err(error));
            }
        });
    }

    fn maintain_previews(&mut self) {
        let mut focus = Vec::new();
        if let Some((index, _, _)) = self.index.clip_at(&self.project, self.playhead) {
            for i in index.saturating_sub(1)..(index + 3).min(self.project.clips.len()) {
                focus.push(self.project.clips[i].asset_id);
            }
        }
        focus.extend(self.selected_asset);
        focus.extend(self.visible_assets.iter().copied());
        let mut seen = HashSet::new();
        focus.retain(|id| seen.insert(*id));
        self.analysis.set_focus(focus.clone(), self.playing);
        // At most 16 full filmstrips: < 57 MiB at the maximum 240x240 RGBA size.
        let mut resident: Vec<_> = focus.into_iter().take(16).collect();
        for id in &self.resident_assets {
            if resident.len() == 16 {
                break;
            }
            if !resident.contains(id) {
                resident.push(*id);
            }
        }
        self.resident_assets = resident;
        self.filmstrips
            .retain(|id, _| self.resident_assets.contains(id));
        for id in &self.resident_assets {
            if !self.filmstrips.contains_key(id)
                && !self.frames_pending.contains(id)
                && let Some(asset) = self.index.asset(&self.project, *id)
            {
                self.filmstrips.insert(*id, vec![None; FRAME_COUNT]);
                self.frames_pending.insert(*id);
                self.analysis.frames(asset.clone());
            }
        }
    }

    fn update_background_work(&mut self, ctx: &egui::Context) {
        // Bound texture uploads per UI update, including a warm-cache import.
        let started = Instant::now();
        while started.elapsed() < std::time::Duration::from_millis(4) {
            let Ok(event) = self.import_rx.try_recv() else {
                break;
            };
            match event {
                ImportEvent::Asset(asset) => {
                    self.frames_pending.insert(asset.id);
                    self.selected_asset = Some(asset.id);
                    self.status = format!("{} ready — analyzing frames and audio…", asset.name);
                    self.project.assets.push(asset);
                    self.project_changed();
                }
                ImportEvent::Metadata {
                    asset_id,
                    width,
                    height,
                    rotation,
                } => {
                    let current_asset = self
                        .index
                        .clip_at(&self.project, self.playhead)
                        .map(|(_, _, clip)| clip.asset_id);
                    let mut changed = false;
                    if let Some(asset) = self.project.assets.iter_mut().find(|a| a.id == asset_id) {
                        changed = asset.width != width
                            || asset.height != height
                            || asset.rotation != rotation;
                        asset.width = width;
                        asset.height = height;
                        asset.rotation = rotation;
                    }
                    if changed {
                        self.project_changed();
                        if current_asset == Some(asset_id) {
                            self.load_preview_at_playhead(self.playing);
                        }
                    }
                }
                ImportEvent::Frame {
                    asset_id,
                    index,
                    image,
                } => {
                    if !self.resident_assets.contains(&asset_id) {
                        continue;
                    }
                    let frames = self
                        .filmstrips
                        .entry(asset_id)
                        .or_insert_with(|| vec![None; FRAME_COUNT]);
                    frames[index] = Some(ctx.load_texture(
                        format!("filmstrip-{asset_id}-{index}"),
                        image,
                        TextureOptions::LINEAR,
                    ));
                }
                ImportEvent::Peaks { asset_id, peaks } => {
                    self.waveforms.entry(asset_id).or_default().extend(peaks);
                }
                ImportEvent::Error(error) => {
                    self.analysis_failed = true;
                    self.status = error;
                }
                ImportEvent::ProxyFinished { asset_id, result } => {
                    self.proxies_pending.remove(&asset_id);
                    match result {
                        Ok(proxy) => {
                            self.proxies.insert(asset_id, proxy);
                            self.proxies_changed = true;
                            self.status = "Lightweight preview ready; exports use original footage"
                                .to_owned();
                            if !self.playing {
                                self.load_preview_at_playhead(false);
                            }
                        }
                        Err(error) => self.status = format!("Preview preparation failed: {error}"),
                    }
                }
                ImportEvent::FramesFinished(id) => {
                    self.frames_pending.remove(&id);
                }
                ImportEvent::Finished => {
                    self.imports_pending = self.imports_pending.saturating_sub(1);
                    if self.imports_pending == 0
                        && self.render_rx.is_none()
                        && !self.analysis_failed
                    {
                        self.status = "Media analysis complete".to_owned();
                    }
                }
            }
        }
        if let Some(frame) = self.player.latest() {
            self.preview_display_size = Some(Vec2::from(frame.display_size));
            let image = ColorImage::from_rgba_unmultiplied(frame.size, &frame.rgba);
            if let Some(texture) = &mut self.preview_texture {
                texture.set(image, TextureOptions::LINEAR);
            } else {
                self.preview_texture =
                    Some(ctx.load_texture("program-monitor", image, TextureOptions::LINEAR));
            }
        }

        let mut finished = false;
        if let Some(rx) = &self.render_rx {
            while let Ok(event) = rx.try_recv() {
                match event {
                    Ok(event) => {
                        self.render_progress = Some(event.progress);
                        self.status = event.message;
                        finished = event.progress >= 1.0;
                    }
                    Err(error) => {
                        self.status = format!("Render failed: {error}");
                        finished = true;
                    }
                }
            }
        }
        if finished {
            self.render_rx = None;
            self.render_progress = None;
        }
    }

    fn update_playback(&mut self, ctx: &egui::Context) {
        if !self.playing {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            if !self.player.timeline_matches(self.revision) {
                self.load_preview_at_playhead(true);
            }
            if let Some(time) = self.player.timeline_time() {
                self.playhead = time.min(self.index.duration());
                if self.playhead >= self.index.duration() - 0.000_02 {
                    self.playhead = self.index.duration();
                    self.playing = false;
                    self.playback_started = None;
                    self.player.stop();
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        #[cfg(not(target_os = "macos"))]
        self.update_clip_playback(ctx);
    }

    #[cfg(not(target_os = "macos"))]
    fn update_clip_playback(&mut self, ctx: &egui::Context) {
        if let Some(source_time) = self.player.source_time() {
            let mut cursor = 0.0;
            for clip in &self.project.clips {
                if self.player.clip_id == Some(clip.id) {
                    self.playhead =
                        cursor + (source_time - clip.source_in).clamp(0.0, clip.duration());
                    break;
                }
                cursor += clip.duration();
            }
        } else if let Some((started, from)) = self.playback_started {
            self.playhead = from + started.elapsed().as_secs_f64();
        }
        if self.playhead >= self.index.duration() {
            self.playhead = self.index.duration();
            self.playing = false;
            self.playback_started = None;
            self.player.stop();
        } else if let Some((_, _, clip)) = self.index.clip_at(&self.project, self.playhead)
            && self.player.clip_id != Some(clip.id)
        {
            self.load_preview_at_playhead(true);
            self.playback_started = Some((Instant::now(), self.playhead));
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }

    fn seek_timeline(&mut self, time: f64) {
        self.playing = false;
        self.playback_started = None;
        self.playhead = time.clamp(0.0, self.index.duration());
        self.load_preview_at_playhead(false);
    }

    fn step_frames(&mut self, frames: f64) {
        let fps = self
            .index
            .clip_at(&self.project, self.playhead)
            .and_then(|(_, _, clip)| self.index.asset(&self.project, clip.asset_id))
            .map(|asset| asset.fps)
            .unwrap_or(self.project.render.fps as f64)
            .max(1.0);
        self.seek_timeline(self.playhead + frames / fps);
    }

    fn jump_to_edit(&mut self, next: bool) {
        let epsilon = 0.000_1;
        let mut cursor = 0.0;
        let mut target = if next { self.index.duration() } else { 0.0 };
        for clip in &self.project.clips {
            if next {
                if cursor > self.playhead + epsilon {
                    target = cursor;
                    break;
                }
            } else if cursor < self.playhead - epsilon {
                target = cursor;
            }
            cursor += clip.duration();
        }
        self.seek_timeline(target);
    }

    fn toggle_selected_mute(&mut self) {
        let Some(id) = self.selected_clip else { return };
        let Some(clip) = self.project.clips.iter_mut().find(|clip| clip.id == id) else {
            return;
        };
        clip.muted = !clip.muted;
        let muted = clip.muted;
        self.project_changed();
        self.status = if muted {
            "Selected clip muted".to_owned()
        } else {
            "Selected clip unmuted".to_owned()
        };
        if self.player.clip_id == Some(id) {
            self.load_preview_at_playhead(self.playing);
        }
    }

    #[cfg(target_os = "macos")]
    fn handle_macos_menu(&mut self, ctx: &egui::Context) {
        use crate::menu_macos::MenuCommand;

        for command in crate::menu_macos::take_commands() {
            match command {
                MenuCommand::ImportVideos => self.import_media(),
                MenuCommand::OpenProject => self.open_project(),
                MenuCommand::Save => self.save_project(false),
                MenuCommand::SaveAs => self.save_project(true),
                MenuCommand::ExportCuts => self.export_cuts(),
                MenuCommand::ExportVideo => self.export_video(),
                MenuCommand::Split => self.split_at_playhead(),
                MenuCommand::DeleteClip => self.delete_selected(),
                MenuCommand::MoveClipLeft => self.move_selected(-1),
                MenuCommand::MoveClipRight => self.move_selected(1),
                MenuCommand::ToggleMute => self.toggle_selected_mute(),
                MenuCommand::PlayPause => self.toggle_playback(),
                MenuCommand::JumpBackSecond => self.seek_timeline(self.playhead - 1.0),
                MenuCommand::JumpForwardSecond => self.seek_timeline(self.playhead + 1.0),
                MenuCommand::PreviousFrame => self.step_frames(-1.0),
                MenuCommand::NextFrame => self.step_frames(1.0),
                MenuCommand::PreviousEdit => self.jump_to_edit(false),
                MenuCommand::NextEdit => self.jump_to_edit(true),
                MenuCommand::ZoomIn => {
                    self.pixels_per_second = (self.pixels_per_second * 1.25).clamp(8.0, 120.0);
                }
                MenuCommand::ZoomOut => {
                    self.pixels_per_second = (self.pixels_per_second / 1.25).clamp(8.0, 120.0);
                }
                MenuCommand::ZoomFit => {
                    let usable_width = (ctx.screen_rect().width() - 80.0).max(200.0);
                    self.pixels_per_second = if self.index.duration() > 0.0 {
                        (usable_width / self.index.duration() as f32).clamp(8.0, 120.0)
                    } else {
                        36.0
                    };
                }
                MenuCommand::ShowShortcuts => self.show_shortcuts = true,
                MenuCommand::ShowHelp => self.show_help = true,
            }
        }
    }

    fn keyboard(&mut self, ctx: &egui::Context) {
        use egui::Modifiers;

        let command_shift = Modifiers::COMMAND.plus(Modifiers::SHIFT);
        let save_as = ctx.input_mut(|input| input.consume_key(command_shift, Key::S));
        let export_cuts = ctx.input_mut(|input| input.consume_key(command_shift, Key::E));
        let import = ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::I));
        let open = ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::O));
        let save = ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::S));
        let export_video = ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::E));
        let command_split = ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::K));
        let zoom_in = ctx.input_mut(|input| {
            input.consume_key(Modifiers::COMMAND, Key::Plus)
                || input.consume_key(Modifiers::COMMAND, Key::Equals)
        });
        let zoom_out = ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::Minus));
        let zoom_fit = ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::Num0));

        if save_as {
            self.save_project(true);
        } else if save {
            self.save_project(false);
        }
        if export_cuts {
            self.export_cuts();
        } else if export_video {
            self.export_video();
        }
        if import {
            self.import_media();
        }
        if open {
            self.open_project();
        }
        if command_split {
            self.split_at_playhead();
        }
        if zoom_in {
            self.pixels_per_second = (self.pixels_per_second * 1.25).clamp(8.0, 120.0);
        }
        if zoom_out {
            self.pixels_per_second = (self.pixels_per_second / 1.25).clamp(8.0, 120.0);
        }
        if zoom_fit {
            let usable_width = (ctx.screen_rect().width() - 80.0).max(200.0);
            self.pixels_per_second = if self.index.duration() > 0.0 {
                (usable_width / self.index.duration() as f32).clamp(8.0, 120.0)
            } else {
                36.0
            };
        }

        if ctx.wants_keyboard_input() {
            return;
        }

        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::F1)) {
            self.show_help = true;
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape)) {
            self.show_shortcuts = false;
            self.show_help = false;
        }

        let shift_left = ctx.input_mut(|input| input.consume_key(Modifiers::SHIFT, Key::ArrowLeft));
        let shift_right =
            ctx.input_mut(|input| input.consume_key(Modifiers::SHIFT, Key::ArrowRight));
        let move_left = ctx.input_mut(|input| input.consume_key(Modifiers::ALT, Key::ArrowLeft));
        let move_right = ctx.input_mut(|input| input.consume_key(Modifiers::ALT, Key::ArrowRight));
        let show_shortcuts = ctx.input_mut(|input| {
            input.consume_key(Modifiers::SHIFT, Key::Questionmark)
                || input.consume_key(Modifiers::SHIFT, Key::Slash)
        });

        if show_shortcuts {
            self.show_shortcuts = true;
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Space)) {
            self.toggle_playback();
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::S)) {
            self.split_at_playhead();
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::M)) {
            self.toggle_selected_mute();
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Delete))
            || ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Backspace))
        {
            self.delete_selected();
        }
        if shift_left {
            self.step_frames(-1.0);
        } else if shift_right {
            self.step_frames(1.0);
        } else if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::ArrowLeft)) {
            self.seek_timeline(self.playhead - 1.0);
        } else if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::ArrowRight)) {
            self.seek_timeline(self.playhead + 1.0);
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::ArrowUp)) {
            self.jump_to_edit(false);
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::ArrowDown)) {
            self.jump_to_edit(true);
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Home)) {
            self.seek_timeline(0.0);
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::End)) {
            self.seek_timeline(self.index.duration());
        }
        if move_left {
            self.move_selected(-1);
        }
        if move_right {
            self.move_selected(1);
        }
    }

    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top_bar")
            .exact_height(58.0)
            .frame(
                Frame::new()
                    .fill(PANEL)
                    .stroke(Stroke::new(1.0_f32, BORDER))
                    .inner_margin(Margin::symmetric(16, 10)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    #[cfg(target_os = "macos")]
                    ui.add_space(68.0);
                    paint_brand(ui, &self.logo_texture);
                    ui.add_space(18.0);
                    let name = ui.add(
                        egui::TextEdit::singleline(&mut self.project.name)
                            .desired_width(250.0)
                            .font(FontId::proportional(15.0))
                            .frame(false),
                    );
                    if name.changed() {
                        self.dirty = true;
                    }
                    if self.dirty {
                        ui.label(RichText::new("*").color(TEXT_MUTED).size(12.0));
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add_enabled(self.render_rx.is_none(), primary_button("Export video"))
                            .on_hover_text(command_hint("E", "Export video"))
                            .clicked()
                        {
                            self.export_video();
                        }
                        if ui
                            .button("Export cuts")
                            .on_hover_text(command_shift_hint(
                                "E",
                                "Export agent-readable timeline JSON",
                            ))
                            .clicked()
                        {
                            self.export_cuts();
                        }
                        if ui
                            .button("Save")
                            .on_hover_text(command_hint("S", "Save project"))
                            .clicked()
                        {
                            self.save_project(false);
                        }
                        if ui
                            .button("Open")
                            .on_hover_text(command_hint("O", "Open project"))
                            .clicked()
                        {
                            self.open_project();
                        }
                        if ui
                            .button("Help")
                            .on_hover_text("Operator quick start  F1")
                            .clicked()
                        {
                            self.show_help = true;
                        }
                    });
                });
            });
    }

    fn media_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("media_panel")
            .exact_width(270.0)
            .frame(
                Frame::new()
                    .fill(PANEL)
                    .stroke(Stroke::new(1.0_f32, BORDER))
                    .inner_margin(Margin::same(14)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("MEDIA").strong().size(12.0).color(TEXT_MUTED));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .button("+ Import")
                            .on_hover_text(command_hint("I", "Import videos"))
                            .clicked()
                        {
                            self.import_media();
                        }
                    });
                });
                ui.add_space(10.0);
                if self.project.assets.is_empty() {
                    ui.add_space(80.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("Drop in footage or a timeline").size(16.0));
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("Video files add to Media • .fastcut opens a project")
                                .color(TEXT_MUTED)
                                .size(12.0),
                        );
                        ui.add_space(14.0);
                        if ui.add(primary_button("Import videos")).clicked() {
                            self.import_media();
                        }
                    });
                    return;
                }

                egui::ScrollArea::vertical().show_rows(
                    ui,
                    76.0,
                    self.project.assets.len(),
                    |ui, range| {
                        for index in range {
                            let asset = &self.project.assets[index];
                            let id = asset.id;
                            self.visible_assets.push(id);
                            let (rect, response) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 76.0),
                                Sense::click(),
                            );
                            ui.painter().rect_filled(
                                rect,
                                7.0,
                                if self.selected_asset == Some(id) {
                                    Color32::from_rgb(39, 47, 55)
                                } else {
                                    SURFACE
                                },
                            );
                            let image_rect = Rect::from_min_size(
                                rect.min + Vec2::new(7.0, 11.0),
                                Vec2::new(92.0, 54.0),
                            );
                            ui.painter().rect_filled(image_rect, 4.0, Color32::BLACK);
                            if let Some(texture) = self
                                .filmstrips
                                .get(&id)
                                .and_then(|frames| frames.iter().flatten().next())
                            {
                                ui.painter().image(
                                    texture.id(),
                                    Rect::from_center_size(
                                        image_rect.center(),
                                        fit_size(texture.size_vec2(), image_rect.size()),
                                    ),
                                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                    Color32::WHITE,
                                );
                            }
                            let text_rect = Rect::from_min_max(
                                rect.min + Vec2::new(106.0, 7.0),
                                rect.max - Vec2::splat(7.0),
                            );
                            let painter = ui
                                .painter()
                                .with_clip_rect(text_rect.intersect(ui.clip_rect()));
                            painter.text(
                                text_rect.min,
                                Align2::LEFT_TOP,
                                &asset.name,
                                FontId::proportional(12.5),
                                Color32::WHITE,
                            );
                            painter.text(
                                text_rect.min + Vec2::new(0.0, 20.0),
                                Align2::LEFT_TOP,
                                format!(
                                    "{} | {}x{}",
                                    format_time(asset.duration),
                                    asset.width,
                                    asset.height
                                ),
                                FontId::proportional(10.5),
                                TEXT_MUTED,
                            );
                            let button = ui.put(
                                Rect::from_min_size(
                                    text_rect.min + Vec2::new(0.0, 41.0),
                                    Vec2::new(82.0, 21.0),
                                ),
                                egui::Button::new("+ Timeline").small(),
                            );
                            if response.clicked() {
                                self.selected_asset = Some(id);
                            }
                            if button.clicked() || response.double_clicked() {
                                self.add_asset_to_timeline(id);
                            }
                        }
                    },
                );
            });
    }

    fn inspector_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("inspector")
            .exact_width(248.0)
            .frame(
                Frame::new()
                    .fill(PANEL)
                    .stroke(Stroke::new(1.0_f32, BORDER))
                    .inner_margin(Margin::same(14)),
            )
            .show(ctx, |ui| {
                ui.label(
                    RichText::new("INSPECTOR")
                        .strong()
                        .size(12.0)
                        .color(TEXT_MUTED),
                );
                ui.add_space(14.0);
                let Some(id) = self.selected_clip else {
                    ui.label(RichText::new("Select a timeline clip to edit it.").color(TEXT_MUTED));
                    return;
                };
                let Some(index) = self.index.clip_index(id) else {
                    return;
                };
                let asset_id = self.project.clips[index].asset_id;
                let asset = self.index.asset(&self.project, asset_id).cloned();
                if let Some(asset) = asset {
                    ui.label(RichText::new(&asset.name).strong());
                    ui.label(
                        RichText::new(format!(
                            "{} | {:.2} fps",
                            format_time(asset.duration),
                            asset.fps
                        ))
                        .size(11.0)
                        .color(TEXT_MUTED),
                    );
                    ui.separator();
                    let pending = self.proxies_pending.contains(&asset_id);
                    if !self.proxies.contains_key(&asset_id) {
                        if ui.add_enabled(!pending, egui::Button::new(if pending { "Preparing preview…" } else { "Prepare lightweight preview" }))
                            .on_hover_text("Create a smaller editing copy with faster seeking. Exports always use the original.")
                            .clicked() {
                            self.proxies_pending.insert(asset_id);
                            self.analysis.prepare_proxy(asset.clone());
                        }
                    } else if ui.checkbox(&mut self.use_proxies, "Use lightweight previews").changed() {
                        self.proxies_changed = true;
                        self.load_preview_at_playhead(self.playing);
                    }
                    ui.separator();
                    let clip = &mut self.project.clips[index];
                    let mut changed = false;
                    ui.label(RichText::new("SOURCE RANGE").size(10.0).color(TEXT_MUTED));
                    ui.horizontal(|ui| {
                        ui.label("In");
                        if ui
                            .add(
                                egui::DragValue::new(&mut clip.source_in)
                                    .range(0.0..=clip.source_out - 0.04)
                                    .speed(0.05)
                                    .suffix(" s"),
                            )
                            .changed()
                        {
                            changed = true;
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Out");
                        if ui
                            .add(
                                egui::DragValue::new(&mut clip.source_out)
                                    .range(clip.source_in + 0.04..=asset.duration)
                                    .speed(0.05)
                                    .suffix(" s"),
                            )
                            .changed()
                        {
                            changed = true;
                        }
                    });
                    ui.label(
                        RichText::new(format!("Duration  {}", format_time(clip.duration())))
                            .color(TEXT_MUTED),
                    );
                    ui.add_space(12.0);
                    ui.label(RichText::new("AUDIO").size(10.0).color(TEXT_MUTED));
                    changed |= ui.checkbox(&mut clip.muted, "Mute clip").changed();
                    changed |= ui.add_enabled(
                        !clip.muted,
                        egui::Slider::new(&mut clip.audio_gain, 0.0..=2.0).text("Gain"),
                    ).changed();
                    if changed { self.project_changed(); self.load_preview_at_playhead(self.playing); }
                    ui.add_space(18.0);
                    ui.horizontal(|ui| {
                        if icon_button(ui, EditorIcon::ArrowLeft, "Move clip left").clicked() {
                            self.move_selected(-1);
                        }
                        if icon_button(ui, EditorIcon::ArrowRight, "Move clip right").clicked() {
                            self.move_selected(1);
                        }
                        if ui.button("Split").clicked() {
                            self.split_at_playhead();
                        }
                        if ui
                            .button(RichText::new("Delete").color(Color32::from_rgb(255, 130, 130)))
                            .clicked()
                        {
                            self.delete_selected();
                        }
                    });
                }

                ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                    ui.label(
                        RichText::new("Press ? to view keyboard shortcuts")
                            .size(10.5)
                            .color(TEXT_MUTED),
                    );
                });
            });
    }

    fn program_monitor(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_size();
        let controls_height = 54.0;
        let max_video = Vec2::new(available.x, (available.y - controls_height).max(100.0));
        let source_size = self
            .preview_display_size
            .or_else(|| {
                let (_, _, clip) = self.index.clip_at(&self.project, self.playhead)?;
                let asset = self.index.asset(&self.project, clip.asset_id)?;
                let (width, height) = if matches!(asset.rotation, 90 | 270) {
                    (asset.height, asset.width)
                } else {
                    (asset.width, asset.height)
                };
                Some(Vec2::new(width as f32, height as f32))
            })
            .unwrap_or(max_video);
        let target = fit_size(source_size, max_video);
        ui.vertical_centered(|ui| {
            let (rect, response) = ui.allocate_exact_size(target, Sense::click());
            ui.painter().rect_filled(rect, 4.0, Color32::BLACK);
            if let Some(texture) = &self.preview_texture {
                let image_size = fit_size(source_size, rect.size());
                let image_rect = Rect::from_center_size(rect.center(), image_size);
                ui.painter().image(
                    texture.id(),
                    image_rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            } else {
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    "IMPORT MEDIA TO START",
                    FontId::proportional(13.0),
                    TEXT_MUTED,
                );
            }
            if response.clicked() {
                self.toggle_playback();
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let transport_icon = if self.playing {
                    EditorIcon::Pause
                } else {
                    EditorIcon::Play
                };
                if icon_button(
                    ui,
                    transport_icon,
                    if self.playing {
                        "Pause (Space)"
                    } else {
                        "Play (Space)"
                    },
                )
                .clicked()
                {
                    self.toggle_playback();
                }
                ui.label(
                    RichText::new(format_time(self.playhead))
                        .monospace()
                        .size(14.0),
                );
                ui.label(RichText::new("/").color(TEXT_MUTED));
                ui.label(
                    RichText::new(format_time(self.index.duration()))
                        .monospace()
                        .size(14.0)
                        .color(TEXT_MUTED),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!(
                            "{}x{}  {}fps",
                            self.project.render.width,
                            self.project.render.height,
                            self.project.render.fps
                        ))
                        .size(11.0)
                        .color(TEXT_MUTED),
                    );
                });
            });
        });
    }

    fn timeline_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("timeline")
            .resizable(true)
            .default_height(286.0)
            .min_height(190.0)
            .max_height(430.0)
            .frame(
                Frame::new()
                    .fill(PANEL)
                    .stroke(Stroke::new(1.0_f32, BORDER))
                    .inner_margin(Margin::same(10)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("TIMELINE")
                            .strong()
                            .size(12.0)
                            .color(TEXT_MUTED),
                    );
                    ui.separator();
                    if icon_button(
                        ui,
                        EditorIcon::Split,
                        &format!("Split at playhead (S / {}K)", command_key()),
                    )
                    .clicked()
                    {
                        self.split_at_playhead();
                    }
                    if icon_button(ui, EditorIcon::Trash, "Remove selected clip (Delete)").clicked()
                    {
                        self.delete_selected();
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add(
                            egui::Slider::new(&mut self.pixels_per_second, 8.0..=120.0)
                                .show_value(false)
                                .text("Zoom"),
                        );
                        ui.label(RichText::new("Zoom").size(11.0).color(TEXT_MUTED));
                    });
                });
                ui.add_space(8.0);

                let scroll_offset = timeline_zoom(
                    ui,
                    &mut self.timeline_active,
                    &mut self.pixels_per_second,
                    self.index.duration(),
                );
                let track_height = 132.0;
                let total_width = (self.index.duration() as f32 * self.pixels_per_second)
                    .max(ui.available_width() - TIMELINE_LEADING_SPACE);
                egui::ScrollArea::horizontal()
                    .id_salt("timeline_scroll")
                    .horizontal_scroll_offset(scroll_offset)
                    .show(ui, |ui| {
                        ui.set_min_width(total_width + TIMELINE_LEADING_SPACE);
                        let origin_x = ui.cursor().left() + TIMELINE_LEADING_SPACE;
                        let ruler_y = ui.cursor().top();
                        let track_y = ruler_y + 30.0;
                        let end_y = track_y + track_height;

                        ui.painter().rect_filled(
                            Rect::from_min_max(
                                Pos2::new(origin_x, track_y),
                                Pos2::new(origin_x + total_width, end_y),
                            ),
                            5.0,
                            Color32::from_rgb(16, 18, 24),
                        );
                        ui.painter().text(
                            Pos2::new(origin_x - 10.0, track_y + 30.0),
                            Align2::RIGHT_CENTER,
                            "V1",
                            FontId::proportional(11.0),
                            TEXT_MUTED,
                        );
                        ui.painter().text(
                            Pos2::new(origin_x - 10.0, track_y + 98.0),
                            Align2::RIGHT_CENTER,
                            "A1",
                            FontId::proportional(11.0),
                            TEXT_MUTED,
                        );

                        let step = ruler_step(self.pixels_per_second);
                        let viewport = ui.clip_rect();
                        let from =
                            ((viewport.left() - origin_x) / self.pixels_per_second).max(0.0) as f64;
                        let to = ((viewport.right() - origin_x) / self.pixels_per_second).max(0.0)
                            as f64;
                        let mut tick = (from / step).floor() * step;
                        while tick <= to {
                            let x = origin_x + tick as f32 * self.pixels_per_second;
                            ui.painter().line_segment(
                                [Pos2::new(x, ruler_y + 18.0), Pos2::new(x, ruler_y + 27.0)],
                                Stroke::new(1.0_f32, BORDER),
                            );
                            ui.painter().text(
                                Pos2::new(x + 3.0, ruler_y + 7.0),
                                Align2::LEFT_CENTER,
                                short_time(tick),
                                FontId::monospace(9.5),
                                TEXT_MUTED,
                            );
                            tick += step;
                        }

                        let ruler_rect = Rect::from_min_max(
                            Pos2::new(origin_x, ruler_y),
                            Pos2::new(origin_x + total_width, track_y),
                        );
                        let ruler_seek = ui.interact(
                            ruler_rect,
                            Id::new("timeline_ruler_seek"),
                            Sense::click_and_drag(),
                        );
                        let mut requested_seek = scrub_time(
                            &ruler_seek,
                            origin_x,
                            self.pixels_per_second,
                            self.index.duration(),
                        );

                        let mut select = None;
                        let mut edited = false;
                        // Include a minimum-width clip overlapping the left edge,
                        // and retain the selected widget while a drag leaves view.
                        let visible = self
                            .index
                            .visible(from - 12.0 / self.pixels_per_second as f64, to);
                        let selected = self
                            .selected_clip
                            .and_then(|id| self.index.clip_index(id))
                            .filter(|i| !visible.contains(i));
                        for index in visible.chain(selected) {
                            let cursor = self.index.start(index);
                            let clip = self.project.clips[index].clone();
                            self.visible_assets.push(clip.asset_id);
                            let width = (clip.duration() as f32 * self.pixels_per_second).max(12.0);
                            let rect = Rect::from_min_size(
                                Pos2::new(
                                    origin_x + cursor as f32 * self.pixels_per_second,
                                    track_y + 5.0,
                                ),
                                Vec2::new(width, track_height - 10.0),
                            );
                            let selected = self.selected_clip == Some(clip.id);
                            let fill = if selected {
                                Color32::from_rgb(75, 63, 172)
                            } else {
                                Color32::from_rgb(54, 48, 117)
                            };
                            ui.painter().rect_filled(rect, 5.0, fill);
                            ui.painter().rect_stroke(
                                rect,
                                5.0,
                                Stroke::new(
                                    if selected { 2.0_f32 } else { 1.0_f32 },
                                    if selected { ACCENT } else { PURPLE },
                                ),
                                StrokeKind::Inside,
                            );

                            if let Some(asset) = self.index.asset(&self.project, clip.asset_id) {
                                let video_rect = Rect::from_min_max(
                                    rect.min + Vec2::splat(4.0),
                                    Pos2::new(rect.max.x - 4.0, rect.min.y + 73.0),
                                );
                                let audio_rect = Rect::from_min_max(
                                    Pos2::new(rect.min.x + 4.0, video_rect.max.y + 2.0),
                                    rect.max - Vec2::splat(4.0),
                                );
                                ui.painter().rect_filled(video_rect, 2.0, Color32::BLACK);
                                ui.painter().rect_filled(
                                    audio_rect,
                                    2.0,
                                    Color32::from_rgb(26, 39, 48),
                                );
                                if let Some(frames) = self.filmstrips.get(&clip.asset_id) {
                                    paint_filmstrip(
                                        ui.painter(),
                                        video_rect,
                                        frames,
                                        &clip,
                                        asset.duration,
                                        self.pixels_per_second,
                                    );
                                }
                                if let Some(peaks) = self.waveforms.get(&clip.asset_id) {
                                    paint_waveform(ui.painter(), audio_rect, peaks, &clip);
                                } else {
                                    ui.painter().line_segment(
                                        [audio_rect.left_center(), audio_rect.right_center()],
                                        Stroke::new(1.0_f32, Color32::from_rgb(58, 77, 88)),
                                    );
                                }
                                ui.painter().text(
                                    video_rect.left_top() + Vec2::new(5.0, 5.0),
                                    Align2::LEFT_TOP,
                                    &asset.name,
                                    FontId::proportional(10.0),
                                    Color32::WHITE,
                                );
                            }

                            let handle_width = (width * 0.25).clamp(3.0, 7.0);
                            let clip_body = rect.shrink2(Vec2::new(handle_width, 0.0));
                            let response = ui.interact(
                                clip_body,
                                Id::new(("clip", clip.id)),
                                Sense::click_and_drag(),
                            );
                            if let Some(time) = scrub_time(
                                &response,
                                origin_x,
                                self.pixels_per_second,
                                self.index.duration(),
                            ) {
                                select = Some(clip.id);
                                requested_seek = Some(time);
                            }

                            let left_handle = Rect::from_min_max(
                                rect.min,
                                Pos2::new(rect.min.x + handle_width, rect.max.y),
                            );
                            let right_handle = Rect::from_min_max(
                                Pos2::new(rect.max.x - handle_width, rect.min.y),
                                rect.max,
                            );
                            let left =
                                ui.interact(left_handle, Id::new(("left", clip.id)), Sense::drag());
                            let right = ui.interact(
                                right_handle,
                                Id::new(("right", clip.id)),
                                Sense::drag(),
                            );
                            if left.hovered() || left.dragged() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                            }
                            if right.hovered() || right.dragged() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                            }
                            if left.dragged() {
                                let delta =
                                    left.drag_delta().x as f64 / self.pixels_per_second as f64;
                                self.project.clips[index].source_in =
                                    (clip.source_in + delta).clamp(0.0, clip.source_out - 0.04);
                                edited = true;
                            }
                            if right.dragged()
                                && let Some(asset) = self.index.asset(&self.project, clip.asset_id)
                            {
                                let delta =
                                    right.drag_delta().x as f64 / self.pixels_per_second as f64;
                                self.project.clips[index].source_out = (clip.source_out + delta)
                                    .clamp(clip.source_in + 0.04, asset.duration);
                                edited = true;
                            }
                        }
                        if edited {
                            self.project_changed();
                            self.load_preview_at_playhead(self.playing);
                        }
                        if let Some(id) = select {
                            self.selected_clip = Some(id);
                        }

                        let content_width = self.index.duration() as f32 * self.pixels_per_second;
                        let empty_rect = Rect::from_min_max(
                            Pos2::new(origin_x + content_width, track_y),
                            Pos2::new(origin_x + total_width, end_y),
                        );
                        if empty_rect.is_positive() {
                            let empty_seek = ui.interact(
                                empty_rect,
                                Id::new("timeline_empty_seek"),
                                Sense::click_and_drag(),
                            );
                            if let Some(time) = scrub_time(
                                &empty_seek,
                                origin_x,
                                self.pixels_per_second,
                                self.index.duration(),
                            ) {
                                requested_seek = Some(time);
                            }
                        }
                        if let Some(time) = requested_seek {
                            let changed = (self.playhead - time).abs() > 0.000_1;
                            let was_playing = self.playing;
                            self.playing = false;
                            self.playback_started = None;
                            self.playhead = time;
                            if changed || was_playing {
                                self.load_preview_at_playhead(false);
                            }
                        }
                        let playhead_x = origin_x + self.playhead as f32 * self.pixels_per_second;
                        ui.painter().line_segment(
                            [
                                Pos2::new(playhead_x, ruler_y + 18.0),
                                Pos2::new(playhead_x, end_y + 4.0),
                            ],
                            Stroke::new(2.0_f32, ACCENT),
                        );
                        ui.painter().circle_filled(
                            Pos2::new(playhead_x, ruler_y + 17.0),
                            5.0,
                            ACCENT,
                        );
                        ui.allocate_space(Vec2::new(
                            total_width + TIMELINE_LEADING_SPACE,
                            track_height + 36.0,
                        ));
                    });
            });
    }

    fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status")
            .exact_height(25.0)
            .frame(
                Frame::new()
                    .fill(Color32::from_rgb(16, 19, 25))
                    .inner_margin(Margin::symmetric(10, 4)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if self.imports_pending > 0
                        || !self.frames_pending.is_empty()
                        || !self.proxies_pending.is_empty()
                    {
                        ui.spinner();
                    }
                    ui.label(RichText::new(&self.status).size(10.5).color(TEXT_MUTED));
                    if let Some(progress) = self.render_progress {
                        ui.add(
                            egui::ProgressBar::new(progress)
                                .desired_width(160.0)
                                .show_percentage(),
                        );
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!(
                                "{} clips  |  {} media",
                                self.project.clips.len(),
                                self.project.assets.len()
                            ))
                            .size(10.5)
                            .color(TEXT_MUTED),
                        );
                    });
                });
            });
    }

    fn shortcuts_window(&mut self, ctx: &egui::Context) {
        if !self.show_shortcuts {
            return;
        }
        let mut open = true;
        egui::Window::new("Keyboard shortcuts")
            .id(Id::new("keyboard_shortcuts"))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(440.0)
            .frame(
                Frame::window(&ctx.style())
                    .fill(PANEL)
                    .stroke(Stroke::new(1.0_f32, BORDER)),
            )
            .show(ctx, |ui| {
                shortcut_section(ui, "PLAYBACK & NAVIGATION");
                egui::Grid::new("shortcut_transport_grid")
                    .num_columns(2)
                    .min_col_width(150.0)
                    .spacing(Vec2::new(20.0, 7.0))
                    .show(ui, |ui| {
                        shortcut_row(ui, "Space", "Play / pause");
                        shortcut_row(ui, "← / →", "Move one second");
                        shortcut_row(ui, "⇧← / ⇧→", "Previous / next frame");
                        shortcut_row(ui, "↑ / ↓", "Previous / next edit");
                        shortcut_row(ui, "Home / End", "Timeline start / end");
                    });

                ui.add_space(14.0);
                shortcut_section(ui, "EDITING");
                egui::Grid::new("shortcut_edit_grid")
                    .num_columns(2)
                    .min_col_width(150.0)
                    .spacing(Vec2::new(20.0, 7.0))
                    .show(ui, |ui| {
                        shortcut_row(ui, &format!("S / {}K", command_key()), "Split at playhead");
                        shortcut_row(ui, "Delete / ⌫", "Remove selected clip");
                        shortcut_row(ui, "M", "Mute / unmute selected clip");
                        shortcut_row(
                            ui,
                            &format!("{}← / {}→", alt_key(), alt_key()),
                            "Move selected clip",
                        );
                        shortcut_row(
                            ui,
                            &format!("{}+ / {}−", command_key(), command_key()),
                            "Zoom timeline",
                        );
                        shortcut_row(ui, &format!("{}0", command_key()), "Fit timeline");
                    });

                ui.add_space(14.0);
                shortcut_section(ui, "PROJECT");
                egui::Grid::new("shortcut_project_grid")
                    .num_columns(2)
                    .min_col_width(150.0)
                    .spacing(Vec2::new(20.0, 7.0))
                    .show(ui, |ui| {
                        shortcut_row(ui, &format!("{}I", command_key()), "Import videos");
                        shortcut_row(ui, &format!("{}O", command_key()), "Open project");
                        shortcut_row(ui, &format!("{}S", command_key()), "Save project");
                        shortcut_row(ui, &format!("⇧{}S", command_key()), "Save as");
                        shortcut_row(ui, &format!("{}E", command_key()), "Export video");
                        shortcut_row(ui, &format!("⇧{}E", command_key()), "Export cuts JSON");
                        shortcut_row(ui, "F1", "Operator quick start");
                        shortcut_row(ui, "?", "Show this window");
                    });
            });
        self.show_shortcuts = open;
    }

    fn help_window(&mut self, ctx: &egui::Context) {
        if !self.show_help {
            return;
        }
        let mut open = true;
        let mut show_shortcuts = false;
        egui::Window::new("fastCutVid quick start")
            .id(Id::new("operator_help"))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(500.0)
            .frame(
                Frame::window(&ctx.style())
                    .fill(PANEL)
                    .stroke(Stroke::new(1.0_f32, BORDER)),
            )
            .show(ctx, |ui| {
                ui.label(
                    RichText::new("FAST ASSEMBLY AND CUTTING")
                        .strong()
                        .size(11.0)
                        .color(ACCENT),
                );
                ui.add_space(5.0);
                ui.label(
                    RichText::new(
                        "fastCutVid is for arranging source videos and trimming them into one sequence. Effects, titles, transitions, and finishing happen elsewhere.",
                    )
                    .color(TEXT_MUTED),
                );
                ui.add_space(14.0);

                help_step(ui, "1", "Import", "Drop video files anywhere, or use + Import. Drop a .fastcut project file to open a complete saved timeline.");
                help_step(ui, "2", "Assemble", "Double-click media or choose + Timeline. Clips play consecutively from left to right.");
                help_step(ui, "3", "Find cuts", "Click-drag over a clip to scrub. Use Left/Right for one second and Shift+Left/Right for one frame.");
                help_step(ui, "4", "Edit", "Drag clip edges to trim, press S to split, Delete to remove, and Option/Alt+Arrow to reorder.");
                help_step(ui, "5", "Deliver", "Save keeps an editable .fastcut project. Export cuts creates a project copy for an agent; Export video renders an MP4.");

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(
                    RichText::new("FFmpeg and FFprobe must be installed and available on PATH for media analysis and export.")
                        .size(11.0)
                        .color(TEXT_MUTED),
                );
                ui.add_space(8.0);
                if ui.button("View keyboard shortcuts").clicked() {
                    show_shortcuts = true;
                }
            });
        self.show_help = open;
        if show_shortcuts {
            self.show_shortcuts = true;
        }
    }
}

impl eframe::App for FastCutApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        #[cfg(target_os = "macos")]
        self.handle_macos_menu(ctx);
        #[cfg(target_os = "macos")]
        for paths in crate::documents_macos::take_open_requests() {
            match crate::opening_paths(paths) {
                Ok(paths) => {
                    if let Some(path) = paths.into_iter().find(|path| is_project_path(path)) {
                        self.open_project_path(path);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                }
                Err(error) => self.status = format!("Open failed: {error}"),
            }
        }
        self.handle_file_drop(ctx);
        self.update_background_work(ctx);
        self.update_playback(ctx);
        self.keyboard(ctx);
        self.top_bar(ctx);
        self.status_bar(ctx);
        self.visible_assets.clear();
        self.timeline_panel(ctx);
        self.media_panel(ctx);
        self.inspector_panel(ctx);
        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG).inner_margin(Margin::same(18)))
            .show(ctx, |ui| self.program_monitor(ui));
        self.help_window(ctx);
        self.shortcuts_window(ctx);
        self.file_drop_overlay(ctx);
        self.maintain_previews();
        // Schedule after input handling as well: a newly started seek, import,
        // cache reload, or export must wake even if the editor was idle before it.
        if self.player.needs_repaint() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        if self.imports_pending > 0
            || !self.frames_pending.is_empty()
            || !self.proxies_pending.is_empty()
            || self.render_rx.is_some()
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(40));
        }
    }
}

#[derive(Clone, Copy)]
enum EditorIcon {
    Play,
    Pause,
    ArrowLeft,
    ArrowRight,
    Split,
    Trash,
}

fn icon_button(ui: &mut egui::Ui, icon: EditorIcon, tooltip: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(31.0, 27.0), Sense::click());
    let fill = if response.is_pointer_button_down_on() {
        Color32::from_rgb(55, 62, 75)
    } else if response.hovered() {
        Color32::from_rgb(43, 49, 61)
    } else {
        SURFACE
    };
    ui.painter().rect_filled(rect, 4.0, fill);
    ui.painter()
        .rect_stroke(rect, 4.0, Stroke::new(1.0_f32, BORDER), StrokeKind::Inside);
    paint_icon(ui.painter(), rect.shrink(6.0), icon, Color32::WHITE);
    response.on_hover_text(tooltip)
}

fn paint_brand(ui: &mut egui::Ui, logo: &TextureHandle) {
    ui.add(
        egui::Image::new(logo)
            .fit_to_exact_size(Vec2::splat(31.0))
            .sense(Sense::hover()),
    );

    let mut wordmark = egui::text::LayoutJob::default();
    for (text, color) in [
        ("fast", Color32::WHITE),
        ("Cut", ACCENT),
        ("Vid", Color32::from_rgb(166, 155, 255)),
    ] {
        wordmark.append(
            text,
            0.0,
            egui::TextFormat {
                font_id: FontId::proportional(17.0),
                color,
                ..Default::default()
            },
        );
    }
    ui.label(wordmark);
}

fn load_logo_texture(ctx: &egui::Context) -> TextureHandle {
    let logo = image::load_from_memory(include_bytes!("../assets/fastcutvid-logo.png"))
        .expect("embedded fastCutVid logo must be valid")
        .resize_exact(96, 96, image::imageops::FilterType::Lanczos3)
        .into_rgba8();
    let image = ColorImage::from_rgba_unmultiplied([96, 96], logo.as_raw());
    ctx.load_texture("fastcutvid-logo", image, TextureOptions::LINEAR)
}

fn paint_icon(painter: &egui::Painter, rect: Rect, icon: EditorIcon, color: Color32) {
    let center = rect.center();
    let stroke = Stroke::new(1.8_f32, color);
    match icon {
        EditorIcon::Play => {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(rect.left() + 2.0, rect.top()),
                    Pos2::new(rect.left() + 2.0, rect.bottom()),
                    Pos2::new(rect.right(), center.y),
                ],
                color,
                Stroke::NONE,
            ));
        }
        EditorIcon::Pause => {
            let bar_width = rect.width() * 0.24;
            painter.rect_filled(
                Rect::from_min_max(
                    rect.left_top(),
                    Pos2::new(rect.left() + bar_width, rect.bottom()),
                ),
                1.0,
                color,
            );
            painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(rect.right() - bar_width, rect.top()),
                    rect.right_bottom(),
                ),
                1.0,
                color,
            );
        }
        EditorIcon::ArrowLeft | EditorIcon::ArrowRight => {
            let direction = if matches!(icon, EditorIcon::ArrowLeft) {
                -1.0
            } else {
                1.0
            };
            painter.line_segment(
                [
                    Pos2::new(center.x - 7.0 * direction, center.y),
                    Pos2::new(center.x + 7.0 * direction, center.y),
                ],
                stroke,
            );
            let tip = Pos2::new(center.x + 7.0 * direction, center.y);
            painter.line_segment(
                [tip, Pos2::new(tip.x - 5.0 * direction, tip.y - 5.0)],
                stroke,
            );
            painter.line_segment(
                [tip, Pos2::new(tip.x - 5.0 * direction, tip.y + 5.0)],
                stroke,
            );
        }
        EditorIcon::Split => {
            painter.circle_stroke(Pos2::new(rect.left() + 3.5, center.y - 4.0), 2.8, stroke);
            painter.circle_stroke(Pos2::new(rect.left() + 3.5, center.y + 4.0), 2.8, stroke);
            painter.line_segment(
                [
                    Pos2::new(rect.left() + 6.0, center.y - 2.5),
                    rect.right_bottom(),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(rect.left() + 6.0, center.y + 2.5),
                    rect.right_top(),
                ],
                stroke,
            );
        }
        EditorIcon::Trash => {
            painter.rect_stroke(
                Rect::from_min_max(
                    Pos2::new(center.x - 5.0, center.y - 4.0),
                    Pos2::new(center.x + 5.0, center.y + 7.0),
                ),
                1.0,
                stroke,
                StrokeKind::Inside,
            );
            painter.line_segment(
                [
                    Pos2::new(center.x - 7.0, center.y - 6.0),
                    Pos2::new(center.x + 7.0, center.y - 6.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(center.x - 2.5, center.y - 8.0),
                    Pos2::new(center.x + 2.5, center.y - 8.0),
                ],
                stroke,
            );
        }
    }
}

fn shortcut_section(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).strong().size(10.5).color(TEXT_MUTED));
    ui.add_space(5.0);
}

fn help_step(ui: &mut egui::Ui, number: &str, title: &str, body: &str) {
    ui.horizontal_top(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 11.0, PURPLE);
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            number,
            FontId::proportional(11.0),
            Color32::WHITE,
        );
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.label(RichText::new(title).strong().size(12.5));
            ui.add(egui::Label::new(RichText::new(body).size(11.0).color(TEXT_MUTED)).wrap());
        });
    });
    ui.add_space(8.0);
}

fn shortcut_row(ui: &mut egui::Ui, shortcut: &str, action: &str) {
    ui.label(
        RichText::new(shortcut)
            .monospace()
            .strong()
            .color(Color32::WHITE),
    );
    ui.label(RichText::new(action).color(TEXT_MUTED));
    ui.end_row();
}

#[cfg(target_os = "macos")]
fn command_key() -> &'static str {
    "⌘"
}

#[cfg(not(target_os = "macos"))]
fn command_key() -> &'static str {
    "Ctrl+"
}

#[cfg(target_os = "macos")]
fn alt_key() -> &'static str {
    "⌥"
}

#[cfg(not(target_os = "macos"))]
fn alt_key() -> &'static str {
    "Alt+"
}

fn command_hint(key: &str, action: &str) -> String {
    format!("{action}  {}{key}", command_key())
}

fn command_shift_hint(key: &str, action: &str) -> String {
    format!("{action}  ⇧{}{key}", command_key())
}

fn configure_style(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = PANEL;
    visuals.window_fill = PANEL;
    visuals.extreme_bg_color = Color32::from_rgb(10, 12, 16);
    visuals.widgets.inactive.bg_fill = SURFACE;
    visuals.widgets.inactive.weak_bg_fill = SURFACE;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(42, 47, 58);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(75, 83, 101));
    visuals.widgets.active.bg_fill = Color32::from_rgb(52, 58, 70);
    visuals.selection.bg_fill = PURPLE;
    visuals.hyperlink_color = ACCENT;
    visuals.window_corner_radius = 9.0.into();
    ctx.set_visuals(visuals);
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(11.0, 6.0);
    ctx.set_style(style);
}

fn primary_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(
        RichText::new(text)
            .strong()
            .color(Color32::from_rgb(10, 20, 16)),
    )
    .fill(ACCENT)
    .stroke(Stroke::NONE)
}

fn fit_size(content: Vec2, bounds: Vec2) -> Vec2 {
    if content.x <= 0.0 || content.y <= 0.0 {
        return bounds;
    }
    let scale = (bounds.x / content.x).min(bounds.y / content.y);
    content * scale
}

fn filmstrip_cell_width(size: Vec2, height: f32) -> f32 {
    (height * size.x / size.y.max(1.0)).max(1.0)
}

fn paint_filmstrip(
    painter: &egui::Painter,
    rect: Rect,
    frames: &[Option<TextureHandle>],
    clip: &Clip,
    asset_duration: f64,
    pixels_per_second: f32,
) {
    let visible = rect.intersect(painter.clip_rect());
    if !visible.is_positive() {
        return;
    }
    let Some(first) = frames.iter().flatten().next() else {
        return;
    };
    let painter = painter.with_clip_rect(visible);
    let cell_width = filmstrip_cell_width(first.size_vec2(), rect.height());
    let first_cell = ((visible.left() - rect.left()) / cell_width).floor() as usize;
    let last_cell = ((visible.right() - rect.left()) / cell_width).ceil() as usize;
    for cell in first_cell..last_cell {
        // Keep full-sized cells at clip edges; the painter clips the partial cell.
        let left = rect.left() + cell as f32 * cell_width;
        let cell_rect = Rect::from_min_size(
            Pos2::new(left, rect.top()),
            Vec2::new(cell_width, rect.height()),
        );
        let local_seconds =
            ((cell_rect.center().x.min(rect.right()) - rect.left()) / pixels_per_second) as f64;
        let source_time = (clip.source_in + local_seconds).clamp(0.0, asset_duration);
        let index = ((source_time / asset_duration.max(0.001) * frames.len() as f64).floor()
            as usize)
            .min(frames.len() - 1);
        let texture = frames[index].as_ref().unwrap_or_else(|| {
            frames
                .iter()
                .enumerate()
                .filter_map(|(i, frame)| frame.as_ref().map(|frame| (i.abs_diff(index), frame)))
                .min_by_key(|(distance, _)| *distance)
                .unwrap()
                .1
        });
        painter.image(
            texture.id(),
            cell_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::from_white_alpha(225),
        );
        painter.line_segment(
            [cell_rect.right_top(), cell_rect.right_bottom()],
            Stroke::new(1.0_f32, Color32::from_black_alpha(120)),
        );
    }
}

fn paint_waveform(painter: &egui::Painter, rect: Rect, waveform: &Waveform, clip: &Clip) {
    let visible = rect.intersect(painter.clip_rect());
    if waveform.peaks.is_empty() || !visible.is_positive() {
        return;
    }
    let painter = painter.with_clip_rect(visible);
    let center = rect.center().y;
    painter.line_segment(
        [visible.left_center(), visible.right_center()],
        Stroke::new(1.0_f32, Color32::from_rgb(52, 75, 87)),
    );
    let first = ((visible.left() - rect.left()) / 2.0).floor() as usize;
    let last = ((visible.right() - rect.left()) / 2.0).ceil() as usize;
    let color = if clip.muted {
        Color32::from_rgb(75, 101, 109)
    } else {
        Color32::from_rgb(77, 211, 224)
    };
    for column in first..last {
        let fraction = (column as f64 * 2.0 / rect.width() as f64).clamp(0.0, 1.0);
        let source_time = clip.source_in + clip.duration() * fraction;
        // Index by source seconds, so a growing waveform never stretches across
        // the unfinished portion of the clip.
        let sample = (source_time * PEAKS_PER_SECOND as f64).floor() as usize;
        let Some(peak) = waveform.peaks.get(sample) else {
            continue;
        };
        let amplitude = (peak / waveform.maximum.max(0.000_01)).sqrt() * rect.height() * 0.44;
        let x = rect.left() + column as f32 * 2.0;
        painter.line_segment(
            [
                Pos2::new(x, center - amplitude),
                Pos2::new(x, center + amplitude),
            ],
            Stroke::new(1.35_f32, color),
        );
    }
}

fn ruler_step(pixels_per_second: f32) -> f64 {
    if pixels_per_second > 80.0 {
        1.0
    } else if pixels_per_second > 32.0 {
        5.0
    } else if pixels_per_second > 15.0 {
        10.0
    } else {
        30.0
    }
}

fn scrub_time(
    response: &egui::Response,
    origin_x: f32,
    pixels_per_second: f32,
    duration: f64,
) -> Option<f64> {
    let active = response.clicked()
        || response.dragged()
        || response.drag_started()
        || response.is_pointer_button_down_on();
    active.then(|| {
        let position = response
            .interact_pointer_pos()
            .unwrap_or(response.rect.left_center());
        (((position.x - origin_x) / pixels_per_second).max(0.0) as f64).min(duration)
    })
}

pub(crate) fn is_video_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "mp4"
                    | "mov"
                    | "mkv"
                    | "webm"
                    | "avi"
                    | "m4v"
                    | "mpeg"
                    | "mpg"
                    | "mts"
                    | "m2ts"
                    | "wmv"
                    | "flv"
            )
        })
}

pub(crate) fn is_project_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("fastcut") || extension.eq_ignore_ascii_case("json")
        })
}

fn short_time(seconds: f64) -> String {
    let seconds = seconds.round() as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn timeline_zoom(
    ui: &egui::Ui,
    active: &mut bool,
    pixels_per_second: &mut f32,
    duration: f64,
) -> f32 {
    let scroll_id = ui.make_persistent_id(Id::new("timeline_scroll"));
    let offset = egui::scroll_area::State::load(ui.ctx(), scroll_id)
        .unwrap_or_default()
        .offset
        .x;
    let hovered = ui.rect_contains_pointer(ui.max_rect());
    let (pressed, window_focused, zoom, pointer) = ui.input(|input| {
        (
            input.pointer.any_pressed(),
            input.focused,
            input.zoom_delta(),
            input.pointer.hover_pos(),
        )
    });
    // This is pointer activity, not keyboard focus. Requesting focus for a
    // synthetic ID without a widget creates an invalid AccessKit tree and
    // crashes when accessibility clients observe a timeline click.
    if !window_focused {
        *active = false;
    } else if pressed {
        *active = hovered;
    }
    if window_focused && (hovered || *active) && (zoom - 1.0).abs() > f32::EPSILON {
        let previous_scale = *pixels_per_second;
        *pixels_per_second = (*pixels_per_second * zoom).clamp(8.0, 120.0);
        if *pixels_per_second != previous_scale {
            let viewport = ui.available_rect_before_wrap();
            let pointer_x = pointer.map_or(0.0, |pos| {
                pos.x.clamp(viewport.left(), viewport.right())
                    - viewport.left()
                    - TIMELINE_LEADING_SPACE
            });
            // Keep the time beneath the pointer at the same screen position,
            // accounting for the scrolled content and the track-label gutter.
            let anchor = (offset + pointer_x).max(0.0);
            let offset = offset + anchor * (*pixels_per_second / previous_scale - 1.0);
            let max_offset = (duration as f32 * *pixels_per_second + TIMELINE_LEADING_SPACE
                - viewport.width())
            .max(0.0);
            // Clamp before drawing as well as in ScrollArea, so reaching an
            // edge or fitting the sequence does not cause a one-frame jump.
            return offset.clamp(0.0, max_offset);
        }
    }
    offset
}

fn cuts_export_path(project: Option<&Path>, media: Option<&Path>, name: &str) -> PathBuf {
    let Some(source) = project.or(media) else {
        return PathBuf::from(format!("{}-cutted.fastcut", safe_name(name)));
    };
    let stem = source.file_stem().unwrap_or_default();
    // Treat .fastcut.json as one project suffix, but keep other dots and the
    // original spelling (including spaces and Unicode) in the suggested name.
    let stem = if project.is_some()
        && source
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        && Path::new(stem)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("fastcut"))
    {
        Path::new(stem).file_stem().unwrap_or(stem)
    } else {
        stem
    };
    let filename = format!("{}-cutted.fastcut", stem.to_string_lossy());
    source
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join(filename)
}

fn safe_name(name: &str) -> String {
    let value: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect();
    let value = value
        .trim_matches('-')
        .to_lowercase()
        .chars()
        .take(64)
        .collect::<String>();
    if value.is_empty() {
        "cut".to_owned()
    } else {
        value
    }
}

#[cfg(test)]
mod timeline_tests {
    use super::*;

    #[test]
    fn portrait_filmstrip_uses_narrow_cells_and_only_draws_the_viewport() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "portrait",
            ColorImage::filled([90, 160], Color32::WHITE),
            TextureOptions::LINEAR,
        );
        let frames = vec![Some(texture)];
        let clip = Clip {
            id: Uuid::new_v4(),
            asset_id: Uuid::new_v4(),
            source_in: 0.0,
            source_out: 3600.0,
            audio_gain: 1.0,
            muted: false,
        };
        let visible = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 70.0));
        let mut meshes = Vec::new();
        ctx.run(egui::RawInput::default(), |ctx| {
            let painter = ctx
                .layer_painter(egui::LayerId::background())
                .with_clip_rect(visible);
            paint_filmstrip(
                &painter,
                Rect::from_min_size(Pos2::new(-20_000.0, 0.0), Vec2::new(129_600.0, 70.0)),
                &frames,
                &clip,
                3600.0,
                36.0,
            );
        })
        .shapes
        .into_iter()
        .for_each(|shape| {
            if let egui::Shape::Mesh(mesh) = shape.shape {
                meshes.push(mesh);
            }
        });
        assert!((20..=22).contains(&meshes.len()), "{} cells", meshes.len());
        for mesh in meshes {
            let bounds = mesh.calc_bounds();
            assert!((bounds.width() / bounds.height() - 90.0 / 160.0).abs() < 0.001);
        }
    }

    #[test]
    fn partial_waveform_stays_at_its_source_time() {
        let ctx = egui::Context::default();
        let mut waveform = Waveform::default();
        waveform.extend(vec![0.5; PEAKS_PER_SECOND]); // Only the first second is decoded.
        let clip = Clip {
            id: Uuid::new_v4(),
            asset_id: Uuid::new_v4(),
            source_in: 0.0,
            source_out: 10.0,
            audio_gain: 1.0,
            muted: false,
        };
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 40.0));
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            paint_waveform(
                &ctx.layer_painter(egui::LayerId::background())
                    .with_clip_rect(rect),
                rect,
                &waveform,
                &clip,
            );
        });
        let bars = output
            .shapes
            .iter()
            .filter_map(|shape| match shape.shape {
                egui::Shape::LineSegment { points, .. } if points[0].x == points[1].x => {
                    Some(points)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(bars.len(), 50);
        assert!(bars.iter().all(|points| points[0].x < 100.0));
    }

    fn frame(
        ctx: &egui::Context,
        active: &mut bool,
        scale: &mut f32,
        events: Vec<egui::Event>,
        focused: bool,
    ) -> egui::scroll_area::ScrollAreaOutput<f32> {
        let mut timeline = None;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
                events,
                focused,
                ..Default::default()
            },
            |ctx| {
                egui::TopBottomPanel::bottom("timeline_test")
                    .exact_height(200.0)
                    .show(ctx, |ui| {
                        ui.label("Timeline");
                        ui.add_space(8.0);
                        let offset = timeline_zoom(ui, active, scale, 90.0);
                        timeline = Some(
                            egui::ScrollArea::horizontal()
                                .id_salt("timeline_scroll")
                                .horizontal_scroll_offset(offset)
                                .show(ui, |ui| {
                                    let width = (90.0 * *scale + TIMELINE_LEADING_SPACE)
                                        .max(ui.available_width());
                                    let origin_x = ui.cursor().left() + TIMELINE_LEADING_SPACE;
                                    ui.allocate_space(Vec2::new(width, 168.0));
                                    origin_x
                                }),
                        );
                    });
            },
        );
        let tree = output
            .platform_output
            .accesskit_update
            .expect("accessibility enabled");
        assert!(
            tree.nodes.iter().any(|(id, _)| *id == tree.focus),
            "Focused ID must exist in the accessibility tree: {:?}",
            tree.focus
        );
        timeline.unwrap()
    }

    fn pointer(pos: Pos2, pressed: bool) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    #[test]
    fn timeline_click_and_drag_keep_accessibility_focus_valid() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut active = false;
        let mut scale = 36.0;
        frame(&ctx, &mut active, &mut scale, vec![], true);
        frame(
            &ctx,
            &mut active,
            &mut scale,
            pointer(Pos2::new(100.0, 500.0), true),
            true,
        );
        assert!(active);
        assert!(ctx.memory(|memory| memory.focused()).is_none());
        frame(
            &ctx,
            &mut active,
            &mut scale,
            vec![egui::Event::PointerMoved(Pos2::new(450.0, 500.0))],
            true,
        );
        frame(
            &ctx,
            &mut active,
            &mut scale,
            pointer(Pos2::new(450.0, 500.0), false),
            true,
        );
        assert!(active);
    }

    #[test]
    fn pinch_works_while_hovered_or_active_and_stops_after_outside_click() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut active = false;
        let mut scale = 36.0;
        frame(&ctx, &mut active, &mut scale, vec![], true);
        frame(
            &ctx,
            &mut active,
            &mut scale,
            vec![
                egui::Event::PointerMoved(Pos2::new(100.0, 500.0)),
                egui::Event::Zoom(2.0),
            ],
            true,
        );
        assert_eq!(scale, 72.0);
        frame(
            &ctx,
            &mut active,
            &mut scale,
            pointer(Pos2::new(100.0, 500.0), true),
            true,
        );
        frame(
            &ctx,
            &mut active,
            &mut scale,
            pointer(Pos2::new(100.0, 500.0), false),
            true,
        );
        frame(
            &ctx,
            &mut active,
            &mut scale,
            vec![
                egui::Event::PointerMoved(Pos2::new(100.0, 100.0)),
                egui::Event::Zoom(0.5),
            ],
            true,
        );
        assert_eq!(scale, 36.0);
        frame(
            &ctx,
            &mut active,
            &mut scale,
            pointer(Pos2::new(100.0, 100.0), true),
            true,
        );
        assert!(!active);
        frame(
            &ctx,
            &mut active,
            &mut scale,
            vec![egui::Event::Zoom(2.0)],
            true,
        );
        assert_eq!(scale, 36.0);
    }

    #[test]
    fn pinch_keeps_the_time_under_the_pointer_fixed_across_frames() {
        for initial_offset in [0.0, 450.0] {
            for pointer_fraction in [0.1, 0.5, 0.95] {
                let ctx = egui::Context::default();
                ctx.enable_accesskit();
                let mut active = false;
                let mut scale = 36.0;
                let initial = frame(&ctx, &mut active, &mut scale, vec![], true);
                let mut state = initial.state;
                state.offset.x = initial_offset;
                state.store(&ctx, initial.id);
                let pointer = Pos2::new(
                    initial.inner_rect.left() + initial.inner_rect.width() * pointer_fraction,
                    initial.inner_rect.center().y,
                );
                let initial = frame(
                    &ctx,
                    &mut active,
                    &mut scale,
                    vec![egui::Event::PointerMoved(pointer)],
                    true,
                );
                let time = (pointer.x - initial.inner) / scale;
                for zoom in [1.5, 1.25, 0.8, 2.0 / 3.0] {
                    let zoomed = frame(
                        &ctx,
                        &mut active,
                        &mut scale,
                        vec![egui::Event::Zoom(zoom)],
                        true,
                    );
                    assert!(
                        (zoomed.inner + time * scale - pointer.x).abs() < 0.001,
                        "The time under the cursor moved at offset {initial_offset}, \
                         pointer fraction {pointer_fraction}, zoom {zoom}"
                    );
                    let settled = frame(&ctx, &mut active, &mut scale, vec![], true);
                    assert!(
                        (settled.inner - zoomed.inner).abs() < 0.001,
                        "zoomed origin {} offset {}, settled origin {} offset {}",
                        zoomed.inner,
                        zoomed.state.offset.x,
                        settled.inner,
                        settled.state.offset.x
                    );
                }
            }
        }
    }

    #[test]
    fn pinch_respects_zoom_limits_and_clamps_scroll_before_drawing() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut active = false;
        let mut scale = 110.0;
        let initial = frame(&ctx, &mut active, &mut scale, vec![], true);
        let mut state = initial.state;
        state.offset.x = 600.0;
        state.store(&ctx, initial.id);
        let pointer = initial.inner_rect.center();
        let initial = frame(
            &ctx,
            &mut active,
            &mut scale,
            vec![egui::Event::PointerMoved(pointer)],
            true,
        );
        let time = (pointer.x - initial.inner) / scale;
        for _ in 0..2 {
            let zoomed = frame(
                &ctx,
                &mut active,
                &mut scale,
                vec![egui::Event::Zoom(2.0)],
                true,
            );
            assert_eq!(scale, 120.0);
            assert!(
                (zoomed.inner + time * scale - pointer.x).abs() < 0.001,
                "origin {} offset {}, time {time}, scale {scale}, pointer {pointer:?}",
                zoomed.inner,
                zoomed.state.offset.x
            );
        }
        for _ in 0..2 {
            let fitted = frame(
                &ctx,
                &mut active,
                &mut scale,
                vec![egui::Event::Zoom(0.01)],
                true,
            );
            assert_eq!(scale, 8.0);
            assert_eq!(fitted.state.offset.x, 0.0);
            assert_eq!(
                fitted.inner,
                fitted.inner_rect.left() + TIMELINE_LEADING_SPACE
            );
            let settled = frame(&ctx, &mut active, &mut scale, vec![], true);
            assert_eq!(settled.inner, fitted.inner);
        }
    }

    #[test]
    fn losing_window_focus_clears_timeline_activity() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut active = true;
        let mut scale = 36.0;
        frame(
            &ctx,
            &mut active,
            &mut scale,
            vec![egui::Event::Zoom(2.0)],
            false,
        );
        assert!(!active);
        assert_eq!(scale, 36.0);
    }
}

#[cfg(test)]
mod export_tests {
    use super::*;

    #[test]
    fn cuts_default_to_project_folder_before_media_folder() {
        assert_eq!(
            cuts_export_path(
                Some(Path::new("projects/Interview.fastcut.json")),
                Some(Path::new("media/source.mov")),
                "Untitled"
            ),
            PathBuf::from("projects/Interview-cutted.fastcut")
        );
    }

    #[test]
    fn cuts_handle_plain_json_and_case_insensitive_compound_suffix() {
        for source in [
            "projects/Edit.json",
            "projects/Edit.FASTCUT.JSON",
            "projects/Edit.fastcut",
            "projects/Edit.FASTCUT",
        ] {
            assert_eq!(
                cuts_export_path(Some(Path::new(source)), None, "Untitled"),
                PathBuf::from("projects/Edit-cutted.fastcut")
            );
        }
    }

    #[test]
    fn cuts_default_to_media_folder_preserving_the_source_name() {
        assert_eq!(
            cuts_export_path(
                None,
                Some(Path::new("media/My café.take.2.MOV")),
                "Untitled"
            ),
            PathBuf::from("media/My café.take.2-cutted.fastcut")
        );
    }

    #[test]
    fn cuts_support_absolute_and_bare_source_paths() {
        let source = std::env::temp_dir().join("clip.mp4");
        assert_eq!(
            cuts_export_path(None, Some(&source), "Untitled"),
            std::env::temp_dir().join("clip-cutted.fastcut")
        );
        assert_eq!(
            cuts_export_path(None, Some(Path::new("clip.mp4")), "Untitled"),
            PathBuf::from("./clip-cutted.fastcut")
        );
    }

    #[test]
    fn empty_project_uses_its_name() {
        assert_eq!(
            cuts_export_path(None, None, "My edit"),
            PathBuf::from("my-edit-cutted.fastcut")
        );
    }
}
