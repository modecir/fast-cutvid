use std::{
    collections::HashMap,
    path::PathBuf,
    sync::mpsc::{Receiver, Sender, channel},
    thread,
    time::Instant,
};

use eframe::egui::{
    self, Align, Align2, Color32, ColorImage, FontId, Frame, Id, Key, Layout, Margin, Pos2, Rect,
    RichText, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2,
};
use uuid::Uuid;

use crate::{
    media::{format_time, probe, timeline_thumbnails, waveform},
    model::{Clip, MediaAsset, Project},
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

enum ImportEvent {
    Asset(MediaAsset),
    Metadata {
        asset_id: Uuid,
        width: u32,
        height: u32,
        rotation: i32,
    },
    Visuals {
        asset_id: Uuid,
        frames: Option<Vec<ColorImage>>,
        peaks: Option<Vec<f32>>,
    },
    Error(String),
    Finished,
}

pub struct FastCutApp {
    project: Project,
    project_path: Option<PathBuf>,
    selected_clip: Option<Uuid>,
    selected_asset: Option<Uuid>,
    playhead: f64,
    playing: bool,
    playback_started: Option<(Instant, f64)>,
    pixels_per_second: f32,
    filmstrips: HashMap<Uuid, Vec<TextureHandle>>,
    waveforms: HashMap<Uuid, Vec<f32>>,
    preview_texture: Option<TextureHandle>,
    player: PreviewPlayer,
    status: String,
    render_rx: Option<Receiver<anyhow::Result<RenderEvent>>>,
    render_progress: Option<f32>,
    import_tx: Sender<ImportEvent>,
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
        crate::menu_macos::install(&cc.egui_ctx);
        configure_style(&cc.egui_ctx);
        let logo_texture = load_logo_texture(&cc.egui_ctx);
        let (import_tx, import_rx) = channel();
        let mut app = Self {
            project: Project::default(),
            project_path: None,
            selected_clip: None,
            selected_asset: None,
            playhead: 0.0,
            playing: false,
            playback_started: None,
            pixels_per_second: 36.0,
            filmstrips: HashMap::new(),
            waveforms: HashMap::new(),
            preview_texture: None,
            player: PreviewPlayer::default(),
            status: "Ready — import media to begin".to_owned(),
            render_rx: None,
            render_progress: None,
            import_tx,
            import_rx,
            imports_pending: 0,
            dirty: false,
            show_shortcuts: false,
            show_help: false,
            logo_texture,
        };
        // Open the project first regardless of argument order, so its channel
        // reset cannot discard imports requested by the same launch.
        if let Some(path) = opening_paths.iter().find(|path| is_json_path(path))
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
        for path in paths {
            let sender = self.import_tx.clone();
            thread::spawn(move || match probe(&path) {
                Ok(asset) => {
                    let _ = sender.send(ImportEvent::Asset(asset.clone()));
                    analyze_asset(asset, sender);
                }
                Err(error) => {
                    let _ = sender.send(ImportEvent::Error(format!(
                        "Could not import {}: {error}",
                        path.display()
                    )));
                    let _ = sender.send(ImportEvent::Finished);
                }
            });
        }
    }

    fn add_asset_to_timeline(&mut self, asset_id: Uuid) {
        let Some(asset) = self.project.asset(asset_id) else {
            return;
        };
        let clip = Clip::from_asset(asset);
        self.selected_clip = Some(clip.id);
        self.project.clips.push(clip);
        self.dirty = true;
        self.status = "Clip added to timeline".to_owned();
        if self.project.clips.len() == 1 {
            self.playhead = 0.0;
            self.load_preview_at_playhead(false);
        }
    }

    fn split_at_playhead(&mut self) {
        let Some((index, clip_start, clip)) = self.project.clip_at(self.playhead) else {
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
        self.dirty = true;
        self.status = format!("Split at {}", format_time(self.playhead));
    }

    fn delete_selected(&mut self) {
        let Some(id) = self.selected_clip else { return };
        if let Some(index) = self.project.clips.iter().position(|clip| clip.id == id) {
            self.project.clips.remove(index);
            self.selected_clip = self
                .project
                .clips
                .get(index.saturating_sub(1))
                .map(|clip| clip.id);
            self.playhead = self.playhead.min(self.project.duration());
            self.dirty = true;
            self.status = "Clip removed".to_owned();
            self.load_preview_at_playhead(false);
        }
    }

    fn move_selected(&mut self, direction: isize) {
        let Some(id) = self.selected_clip else { return };
        let Some(index) = self.project.clips.iter().position(|clip| clip.id == id) else {
            return;
        };
        let target = index as isize + direction;
        if target >= 0 && target < self.project.clips.len() as isize {
            self.project.clips.swap(index, target as usize);
            self.dirty = true;
        }
    }

    fn toggle_playback(&mut self) {
        if self.project.clips.is_empty() {
            return;
        }
        self.playing = !self.playing;
        if self.playing {
            if self.playhead >= self.project.duration() {
                self.playhead = 0.0;
            }
            self.load_preview_at_playhead(true);
            self.playback_started = Some((Instant::now(), self.playhead));
        } else {
            self.playback_started = None;
            self.player.stop();
            self.load_preview_at_playhead(false);
        }
    }

    fn load_preview_at_playhead(&mut self, realtime: bool) {
        let Some((_, clip_start, clip)) = self.project.clip_at(self.playhead) else {
            self.player.stop();
            return;
        };
        let Some(asset) = self.project.asset(clip.asset_id) else {
            return;
        };
        let offset = (self.playhead - clip_start).clamp(0.0, clip.duration());
        let source_time = clip.source_in + offset;
        let volume = if clip.muted { 0.0 } else { clip.audio_gain };
        if self
            .player
            .seek(clip.id, source_time, clip.source_out, realtime, volume)
        {
            return;
        }
        self.player.start(
            clip.id,
            asset,
            source_time,
            (clip.duration() - offset).max(0.04),
            realtime,
            volume,
        );
    }

    fn save_project(&mut self, save_as: bool) {
        let path = if !save_as {
            self.project_path.clone()
        } else {
            None
        }
        .or_else(|| {
            rfd::FileDialog::new()
                .set_file_name(format!("{}.fastcut.json", safe_name(&self.project.name)))
                .add_filter("fastCutVid project", &["json"])
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

    fn open_project(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("fastCutVid project", &["json"])
            .pick_file()
        else {
            return;
        };
        self.open_project_path(path);
    }

    fn open_project_path(&mut self, path: PathBuf) -> bool {
        match Project::load(&path) {
            Ok(project) => {
                // Detach any analysis jobs from the previous project. Their
                // senders keep the old channel, so stale results cannot leak
                // into a timeline that was just opened by file drop.
                let (import_tx, import_rx) = channel();
                self.import_tx = import_tx;
                self.import_rx = import_rx;
                self.imports_pending = 0;
                self.project = project;
                self.project_path = Some(path.clone());
                self.selected_clip = self.project.clips.first().map(|clip| clip.id);
                self.selected_asset = self.project.assets.first().map(|asset| asset.id);
                self.playhead = 0.0;
                self.playing = false;
                self.playback_started = None;
                self.player.stop();
                self.preview_texture = None;
                self.filmstrips.clear();
                self.waveforms.clear();
                let assets = self.project.assets.clone();
                self.imports_pending += assets.len();
                for asset in assets {
                    let sender = self.import_tx.clone();
                    thread::spawn(move || {
                        if let Ok(metadata) = probe(&PathBuf::from(&asset.path)) {
                            let _ = sender.send(ImportEvent::Metadata {
                                asset_id: asset.id,
                                width: metadata.width,
                                height: metadata.height,
                                rotation: metadata.rotation,
                            });
                        }
                        analyze_asset(asset, sender);
                    });
                }
                let missing = self
                    .project
                    .assets
                    .iter()
                    .filter(|asset| !PathBuf::from(&asset.path).is_file())
                    .count();
                self.status = if missing == 0 {
                    format!(
                        "Opened {} — rebuilding media previews in the background",
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
            if is_json_path(&path) {
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
        painter.rect_stroke(rect, 12.0, Stroke::new(2.0, ACCENT), StrokeKind::Inside);
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
            "Videos are added to Media  •  Timeline JSON opens as a project",
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

    fn update_background_work(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.import_rx.try_recv() {
            match event {
                ImportEvent::Asset(asset) => {
                    self.selected_asset = Some(asset.id);
                    self.status = format!("{} ready — analyzing frames and audio…", asset.name);
                    self.project.assets.push(asset);
                    self.dirty = true;
                }
                ImportEvent::Metadata {
                    asset_id,
                    width,
                    height,
                    rotation,
                } => {
                    let current_asset = self
                        .project
                        .clip_at(self.playhead)
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
                        self.dirty = true;
                        if current_asset == Some(asset_id) {
                            self.load_preview_at_playhead(self.playing);
                        }
                    }
                }
                ImportEvent::Visuals {
                    asset_id,
                    frames,
                    peaks,
                } => {
                    if let Some(frames) = frames {
                        let textures = frames
                            .into_iter()
                            .enumerate()
                            .map(|(index, image)| {
                                ctx.load_texture(
                                    format!("filmstrip-{asset_id}-{index}"),
                                    image,
                                    TextureOptions::LINEAR,
                                )
                            })
                            .collect();
                        self.filmstrips.insert(asset_id, textures);
                    }
                    if let Some(peaks) = peaks {
                        self.waveforms.insert(asset_id, peaks);
                    }
                }
                ImportEvent::Error(error) => self.status = error,
                ImportEvent::Finished => {
                    self.imports_pending = self.imports_pending.saturating_sub(1);
                    if self.imports_pending == 0 && self.render_rx.is_none() {
                        self.status = "Media analysis complete".to_owned();
                    }
                }
            }
        }
        if self.imports_pending > 0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(40));
        }
        if self.player.clip_id.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }

        if let Some(frame) = self.player.latest() {
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
        if self.playhead >= self.project.duration() {
            self.playhead = self.project.duration();
            self.playing = false;
            self.playback_started = None;
            self.player.stop();
        } else if let Some((_, _, clip)) = self.project.clip_at(self.playhead)
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
        self.playhead = time.clamp(0.0, self.project.duration());
        self.load_preview_at_playhead(false);
    }

    fn step_frames(&mut self, frames: f64) {
        let fps = self
            .project
            .clip_at(self.playhead)
            .and_then(|(_, _, clip)| self.project.asset(clip.asset_id))
            .map(|asset| asset.fps)
            .unwrap_or(self.project.render.fps as f64)
            .max(1.0);
        self.seek_timeline(self.playhead + frames / fps);
    }

    fn jump_to_edit(&mut self, next: bool) {
        let epsilon = 0.000_1;
        let mut cursor = 0.0;
        let mut target = if next { self.project.duration() } else { 0.0 };
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
        self.dirty = true;
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
                MenuCommand::SaveAs | MenuCommand::ExportCuts => self.save_project(true),
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
                    self.pixels_per_second = if self.project.duration() > 0.0 {
                        (usable_width / self.project.duration() as f32).clamp(8.0, 120.0)
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
            self.save_project(true);
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
            self.pixels_per_second = if self.project.duration() > 0.0 {
                (usable_width / self.project.duration() as f32).clamp(8.0, 120.0)
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
            self.seek_timeline(self.project.duration());
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
                    .stroke(Stroke::new(1.0, BORDER))
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
                            self.save_project(true);
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
                    .stroke(Stroke::new(1.0, BORDER))
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
                            RichText::new("Video files add to Media • JSON opens a project")
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

                let assets = self.project.assets.clone();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for asset in assets {
                        let selected = self.selected_asset == Some(asset.id);
                        let bg = if selected {
                            Color32::from_rgb(39, 47, 55)
                        } else {
                            SURFACE
                        };
                        Frame::new()
                            .fill(bg)
                            .corner_radius(7.0)
                            .inner_margin(Margin::same(7))
                            .show(ui, |ui| {
                                let response = ui
                                    .horizontal(|ui| {
                                        if let Some(texture) = self
                                            .filmstrips
                                            .get(&asset.id)
                                            .and_then(|frames| frames.first())
                                        {
                                            let (rect, _) = ui.allocate_exact_size(
                                                Vec2::new(92.0, 54.0),
                                                Sense::hover(),
                                            );
                                            ui.painter().rect_filled(rect, 4.0, Color32::BLACK);
                                            let image_size =
                                                fit_size(texture.size_vec2(), rect.size());
                                            ui.painter().image(
                                                texture.id(),
                                                Rect::from_center_size(rect.center(), image_size),
                                                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                                Color32::WHITE,
                                            );
                                        } else {
                                            let (rect, _) = ui.allocate_exact_size(
                                                Vec2::new(92.0, 54.0),
                                                Sense::hover(),
                                            );
                                            ui.painter().rect_filled(rect, 4.0, Color32::BLACK);
                                        }
                                        ui.vertical(|ui| {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&asset.name).size(12.5),
                                                )
                                                .truncate(),
                                            );
                                            ui.label(
                                                RichText::new(format!(
                                                    "{}  |  {}x{}",
                                                    format_time(asset.duration),
                                                    asset.width,
                                                    asset.height
                                                ))
                                                .size(10.5)
                                                .color(TEXT_MUTED),
                                            );
                                            if ui.small_button("+ Timeline").clicked() {
                                                self.add_asset_to_timeline(asset.id);
                                            }
                                        });
                                    })
                                    .response
                                    .interact(Sense::click());
                                if response.clicked() {
                                    self.selected_asset = Some(asset.id);
                                }
                                if response.double_clicked() {
                                    self.add_asset_to_timeline(asset.id);
                                }
                            });
                        ui.add_space(7.0);
                    }
                });
            });
    }

    fn inspector_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("inspector")
            .exact_width(248.0)
            .frame(
                Frame::new()
                    .fill(PANEL)
                    .stroke(Stroke::new(1.0, BORDER))
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
                let Some(index) = self.project.clips.iter().position(|clip| clip.id == id) else {
                    return;
                };
                let asset_id = self.project.clips[index].asset_id;
                let asset = self.project.asset(asset_id).cloned();
                if let Some(asset) = asset {
                    ui.label(RichText::new(asset.name).strong());
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
                    let clip = &mut self.project.clips[index];
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
                            self.dirty = true;
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
                            self.dirty = true;
                        }
                    });
                    ui.label(
                        RichText::new(format!("Duration  {}", format_time(clip.duration())))
                            .color(TEXT_MUTED),
                    );
                    ui.add_space(12.0);
                    ui.label(RichText::new("AUDIO").size(10.0).color(TEXT_MUTED));
                    ui.checkbox(&mut clip.muted, "Mute clip");
                    ui.add_enabled(
                        !clip.muted,
                        egui::Slider::new(&mut clip.audio_gain, 0.0..=2.0).text("Gain"),
                    );
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
        let target = fit_size(Vec2::new(16.0, 9.0), max_video);
        ui.vertical_centered(|ui| {
            let (rect, response) = ui.allocate_exact_size(target, Sense::click());
            ui.painter().rect_filled(rect, 4.0, Color32::BLACK);
            if let Some(texture) = &self.preview_texture {
                let image_size = fit_size(texture.size_vec2(), rect.size());
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
                    RichText::new(format_time(self.project.duration()))
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
                    .stroke(Stroke::new(1.0, BORDER))
                    .inner_margin(Margin::same(10)),
            )
            .show(ctx, |ui| {
                let zoom_focus = Id::new("timeline_zoom_focus");
                let timeline_hovered = ui.rect_contains_pointer(ui.max_rect());
                let pointer_pressed = ui.input(|input| input.pointer.any_pressed());
                if timeline_hovered && pointer_pressed {
                    ui.memory_mut(|memory| memory.request_focus(zoom_focus));
                } else if !timeline_hovered && pointer_pressed {
                    ui.memory_mut(|memory| memory.surrender_focus(zoom_focus));
                }
                let timeline_focused = ui.memory(|memory| memory.has_focus(zoom_focus));
                let gesture_zoom = ui.input(|input| input.zoom_delta());
                if (timeline_hovered || timeline_focused)
                    && (gesture_zoom - 1.0).abs() > f32::EPSILON
                {
                    self.pixels_per_second =
                        (self.pixels_per_second * gesture_zoom).clamp(8.0, 120.0);
                }

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

                let track_height = 132.0;
                let total_width = (self.project.duration() as f32 * self.pixels_per_second)
                    .max(ui.available_width() - 55.0);
                egui::ScrollArea::horizontal()
                    .id_salt("timeline_scroll")
                    .show(ui, |ui| {
                        ui.set_min_width(total_width + 55.0);
                        let origin_x = ui.cursor().left() + 55.0;
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
                        let mut tick = 0.0;
                        while tick
                            <= self
                                .project
                                .duration()
                                .max(total_width as f64 / self.pixels_per_second as f64)
                        {
                            let x = origin_x + tick as f32 * self.pixels_per_second;
                            ui.painter().line_segment(
                                [Pos2::new(x, ruler_y + 18.0), Pos2::new(x, ruler_y + 27.0)],
                                Stroke::new(1.0, BORDER),
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
                            self.project.duration(),
                        );

                        let mut cursor = 0.0;
                        let mut select = None;
                        for index in 0..self.project.clips.len() {
                            let clip = self.project.clips[index].clone();
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
                                    if selected { 2.0 } else { 1.0 },
                                    if selected { ACCENT } else { PURPLE },
                                ),
                                StrokeKind::Inside,
                            );

                            if let Some(asset) = self.project.asset(clip.asset_id) {
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
                                    paint_waveform(
                                        ui.painter(),
                                        audio_rect,
                                        peaks,
                                        &clip,
                                        asset.duration,
                                    );
                                } else {
                                    ui.painter().line_segment(
                                        [audio_rect.left_center(), audio_rect.right_center()],
                                        Stroke::new(1.0, Color32::from_rgb(58, 77, 88)),
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
                                self.project.duration(),
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
                                self.dirty = true;
                            }
                            if right.dragged()
                                && let Some(asset) = self.project.asset(clip.asset_id)
                            {
                                let delta =
                                    right.drag_delta().x as f64 / self.pixels_per_second as f64;
                                self.project.clips[index].source_out = (clip.source_out + delta)
                                    .clamp(clip.source_in + 0.04, asset.duration);
                                self.dirty = true;
                            }
                            cursor += clip.duration();
                        }
                        if let Some(id) = select {
                            self.selected_clip = Some(id);
                        }

                        let content_width = self.project.duration() as f32 * self.pixels_per_second;
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
                                self.project.duration(),
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
                            Stroke::new(2.0, ACCENT),
                        );
                        ui.painter().circle_filled(
                            Pos2::new(playhead_x, ruler_y + 17.0),
                            5.0,
                            ACCENT,
                        );
                        ui.allocate_space(Vec2::new(total_width + 55.0, track_height + 36.0));
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
                    if self.imports_pending > 0 {
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
                    .stroke(Stroke::new(1.0, BORDER)),
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
                    .stroke(Stroke::new(1.0, BORDER)),
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

                help_step(ui, "1", "Import", "Drop video files anywhere, or use + Import. Drop a fastCutVid JSON file to open a complete saved timeline.");
                help_step(ui, "2", "Assemble", "Double-click media or choose + Timeline. Clips play consecutively from left to right.");
                help_step(ui, "3", "Find cuts", "Click-drag over a clip to scrub. Use Left/Right for one second and Shift+Left/Right for one frame.");
                help_step(ui, "4", "Edit", "Drag clip edges to trim, press S to split, Delete to remove, and Option/Alt+Arrow to reorder.");
                help_step(ui, "5", "Deliver", "Save keeps an editable JSON project. Export cuts hands JSON to an agent; Export video renders an MP4.");

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
        self.handle_file_drop(ctx);
        self.update_background_work(ctx);
        self.update_playback(ctx);
        self.keyboard(ctx);
        self.top_bar(ctx);
        self.status_bar(ctx);
        self.timeline_panel(ctx);
        self.media_panel(ctx);
        self.inspector_panel(ctx);
        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG).inner_margin(Margin::same(18)))
            .show(ctx, |ui| self.program_monitor(ui));
        self.help_window(ctx);
        self.shortcuts_window(ctx);
        self.file_drop_overlay(ctx);
    }
}

fn analyze_asset(asset: MediaAsset, sender: Sender<ImportEvent>) {
    let frame_path = PathBuf::from(&asset.path);
    let frame_duration = asset.duration;
    let frame_job = thread::spawn(move || timeline_thumbnails(&frame_path, frame_duration, 16));

    let audio_job = asset.has_audio.then(|| {
        let audio_path = PathBuf::from(&asset.path);
        thread::spawn(move || waveform(&audio_path))
    });

    let frames = frame_job.join().ok().and_then(Result::ok);
    let peaks = audio_job
        .and_then(|job| job.join().ok())
        .and_then(Result::ok);
    let _ = sender.send(ImportEvent::Visuals {
        asset_id: asset.id,
        frames,
        peaks,
    });
    let _ = sender.send(ImportEvent::Finished);
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
        .rect_stroke(rect, 4.0, Stroke::new(1.0, BORDER), StrokeKind::Inside);
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
    let stroke = Stroke::new(1.8, color);
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
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(42, 47, 58);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(75, 83, 101));
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

fn paint_filmstrip(
    painter: &egui::Painter,
    rect: Rect,
    frames: &[TextureHandle],
    clip: &Clip,
    asset_duration: f64,
    pixels_per_second: f32,
) {
    if frames.is_empty() || rect.width() <= 1.0 {
        return;
    }
    let painter = painter.with_clip_rect(rect);
    let cell_width = 88.0_f32.min((clip.duration() as f32 * pixels_per_second).max(18.0));
    let cell_count = (rect.width() / cell_width).ceil().max(1.0) as usize;
    for cell in 0..cell_count {
        let left = rect.left() + cell as f32 * cell_width;
        let cell_rect = Rect::from_min_max(
            Pos2::new(left, rect.top()),
            Pos2::new((left + cell_width).min(rect.right()), rect.bottom()),
        );
        let local_seconds = ((cell_rect.center().x - rect.left()) / pixels_per_second) as f64;
        let source_time = (clip.source_in + local_seconds).clamp(0.0, asset_duration);
        let normalized = source_time / asset_duration.max(0.001);
        let index = (normalized * (frames.len().saturating_sub(1)) as f64).round() as usize;
        let texture = &frames[index.min(frames.len() - 1)];
        let image_size = fit_size(texture.size_vec2(), cell_rect.size());
        painter.image(
            texture.id(),
            Rect::from_center_size(cell_rect.center(), image_size),
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::from_white_alpha(225),
        );
        painter.line_segment(
            [cell_rect.right_top(), cell_rect.right_bottom()],
            Stroke::new(1.0, Color32::from_black_alpha(120)),
        );
    }
}

fn paint_waveform(
    painter: &egui::Painter,
    rect: Rect,
    peaks: &[f32],
    clip: &Clip,
    asset_duration: f64,
) {
    if peaks.is_empty() || rect.width() <= 1.0 {
        return;
    }
    let center = rect.center().y;
    painter.line_segment(
        [rect.left_center(), rect.right_center()],
        Stroke::new(1.0, Color32::from_rgb(52, 75, 87)),
    );
    let columns = (rect.width() / 2.0).ceil().max(1.0) as usize;
    let color = if clip.muted {
        Color32::from_rgb(75, 101, 109)
    } else {
        Color32::from_rgb(77, 211, 224)
    };
    for column in 0..columns {
        let fraction = if columns <= 1 {
            0.0
        } else {
            column as f64 / (columns - 1) as f64
        };
        let source_time = clip.source_in + clip.duration() * fraction;
        let sample = ((source_time / asset_duration.max(0.001))
            * peaks.len().saturating_sub(1) as f64)
            .round() as usize;
        let amplitude = peaks[sample.min(peaks.len() - 1)] * rect.height() * 0.44;
        let x = rect.left() + column as f32 * 2.0;
        painter.line_segment(
            [
                Pos2::new(x, center - amplitude),
                Pos2::new(x, center + amplitude),
            ],
            Stroke::new(1.35, color),
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

pub(crate) fn is_json_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}

fn short_time(seconds: f64) -> String {
    let seconds = seconds.round() as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
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
