mod app;
mod media;
#[cfg(target_os = "macos")]
mod menu_macos;
mod model;
#[cfg(not(target_os = "macos"))]
mod player;
#[cfg(target_os = "macos")]
#[path = "player_macos.rs"]
mod player;
mod render;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    version,
    about = "fastCutVid — native video cutting for humans and agents"
)]
struct Args {
    /// Validate a fastCutVid timeline without opening the GUI or rendering.
    #[arg(long, value_name = "PROJECT.fastcut.json", conflicts_with = "render")]
    validate: Option<PathBuf>,

    /// Render a fastCutVid project without opening the GUI.
    #[arg(long, value_name = "PROJECT.fastcut.json")]
    render: Option<PathBuf>,

    /// Output video used with --render.
    #[arg(long, value_name = "VIDEO.mp4")]
    output: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    if let Some(project_path) = args.validate {
        let project = model::Project::load(&project_path)?;
        println!(
            "Valid {}: {} assets, {} clips, {:.3}s",
            project.format,
            project.assets.len(),
            project.clips.len(),
            project.duration()
        );
        return Ok(());
    }
    if let Some(project_path) = args.render {
        let output = args
            .output
            .context("--output is required when using --render")?;
        let project = model::Project::load(&project_path)?;
        render::render_project(&project, &output, |event| {
            eprintln!("{}", event.message);
        })?;
        println!("{}", output.display());
        return Ok(());
    }

    let viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size([980.0, 680.0])
        .with_decorations(true)
        .with_drag_and_drop(true)
        .with_icon(app_icon());
    #[cfg(target_os = "macos")]
    let viewport = viewport
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false)
        .with_titlebar_buttons_shown(true);

    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    eframe::run_native(
        "fastCutVid",
        options,
        Box::new(|cc| Ok(Box::new(app::FastCutApp::new(cc)))),
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))
}

fn app_icon() -> eframe::egui::IconData {
    let image = image::load_from_memory(include_bytes!("../assets/fastcutvid-logo.png"))
        .expect("embedded fastCutVid logo must be valid")
        .into_rgba8();
    let (width, height) = image.dimensions();
    eframe::egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    }
}
