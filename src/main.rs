mod app;
#[cfg(target_os = "macos")]
mod documents_macos;
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
    /// Open one fastCutVid project and/or import videos into the GUI media bin.
    #[arg(value_name = "FILE", conflicts_with_all = ["validate", "render", "output"])]
    files: Vec<PathBuf>,

    /// Validate a fastCutVid timeline without opening the GUI or rendering.
    #[arg(long, value_name = "PROJECT.fastcut", conflicts_with = "render")]
    validate: Option<PathBuf>,

    /// Render a fastCutVid project without opening the GUI.
    #[arg(long, value_name = "PROJECT.fastcut")]
    render: Option<PathBuf>,

    /// Output video used with --render.
    #[arg(long, value_name = "VIDEO.mp4", requires = "render")]
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

    let files = opening_paths(args.files)?;
    #[cfg(target_os = "macos")]
    documents_macos::install();

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
        Box::new(move |cc| Ok(Box::new(app::FastCutApp::new(cc, files)))),
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))
}

fn opening_paths(paths: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    let cwd = std::env::current_dir().context("Could not resolve the working directory")?;
    let mut projects = 0;
    paths
        .into_iter()
        .map(|path| {
            anyhow::ensure!(
                path.is_file(),
                "File does not exist or is not a regular file: {}",
                path.display()
            );
            if app::is_project_path(&path) {
                projects += 1;
                anyhow::ensure!(
                    projects <= 1,
                    "Only one timeline JSON can be opened at a time"
                );
            } else {
                anyhow::ensure!(
                    app::is_video_path(&path),
                    "Unsupported file type: {} (expected a .fastcut project or video)",
                    path.display()
                );
            }
            // Keep the supplied directory semantics for symlinked projects,
            // while ensuring imported media paths remain valid after saving elsewhere.
            Ok(cwd.join(path))
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gui_arguments_accept_no_files_or_multiple_files() {
        assert!(
            Args::try_parse_from(["fast-cutvid"])
                .unwrap()
                .files
                .is_empty()
        );
        let args = Args::try_parse_from([
            "fast-cutvid",
            "first clip.mp4",
            "edit.fastcut.json",
            "second.MOV",
        ])
        .unwrap();
        assert_eq!(
            args.files,
            vec![
                PathBuf::from("first clip.mp4"),
                PathBuf::from("edit.fastcut.json"),
                PathBuf::from("second.MOV")
            ]
        );
        let args = Args::try_parse_from(["fast-cutvid", "--", "-clip.mp4"]).unwrap();
        assert_eq!(args.files, vec![PathBuf::from("-clip.mp4")]);
    }

    #[test]
    fn gui_files_cannot_be_mixed_with_headless_modes() {
        for arguments in [
            vec!["fast-cutvid", "clip.mp4", "--validate", "edit.json"],
            vec![
                "fast-cutvid",
                "clip.mp4",
                "--render",
                "edit.json",
                "--output",
                "out.mp4",
            ],
            vec!["fast-cutvid", "--output", "out.mp4"],
            vec![
                "fast-cutvid",
                "--validate",
                "edit.json",
                "--render",
                "edit.json",
            ],
        ] {
            assert!(Args::try_parse_from(arguments).is_err());
        }
        assert!(Args::try_parse_from(["fast-cutvid", "--validate", "edit.json"]).is_ok());
        assert!(
            Args::try_parse_from([
                "fast-cutvid",
                "--render",
                "edit.json",
                "--output",
                "out.mp4"
            ])
            .is_ok()
        );
    }

    #[test]
    fn opening_paths_accept_supported_files_and_reject_bad_inputs() {
        let directory = tempfile::tempdir().unwrap();
        // Empty files are sufficient here: this checks launch routing, not decoding.
        let video = directory.path().join("a clip.MOV");
        let project = directory.path().join("edit.FASTCUT");
        let other_project = directory.path().join("other.json");
        let unsupported = directory.path().join("notes.txt");
        for path in [&video, &project, &other_project, &unsupported] {
            std::fs::File::create(path).unwrap();
        }
        assert_eq!(
            opening_paths(vec![video.clone(), project.clone()]).unwrap(),
            vec![video, project.clone()]
        );
        assert!(
            opening_paths(vec![project, other_project])
                .unwrap_err()
                .to_string()
                .contains("Only one timeline")
        );
        assert!(
            opening_paths(vec![unsupported])
                .unwrap_err()
                .to_string()
                .contains("Unsupported file type")
        );
        assert!(opening_paths(vec![directory.path().join("missing.mp4")]).is_err());
        assert!(opening_paths(vec![directory.path().to_owned()]).is_err());
        assert!(opening_paths(Vec::new()).unwrap().is_empty());
    }

    #[test]
    fn routes_new_and_legacy_project_extensions() {
        for name in [
            "edit.fastcut",
            "café.FASTCUT",
            "edit.fastcut.json",
            "edit.JSON",
        ] {
            assert!(app::is_project_path(std::path::Path::new(name)));
        }
        for name in ["clip.mp4", "edit.fastcut.bak", "fastcut", "notes.txt"] {
            assert!(!app::is_project_path(std::path::Path::new(name)));
        }
    }

    #[test]
    fn relative_opening_paths_become_absolute() {
        // Use a disposable file under the working directory without changing
        // process-wide cwd, so this test can run alongside the rest of the suite.
        let video = tempfile::Builder::new()
            .suffix(".mp4")
            .tempfile_in(".")
            .unwrap();
        let relative = PathBuf::from(video.path().file_name().unwrap());
        let resolved = opening_paths(vec![relative.clone()]).unwrap();
        assert_eq!(
            resolved,
            vec![std::env::current_dir().unwrap().join(relative)]
        );
    }
}
