use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const FORMAT_VERSION: &str = "fastcut.timeline/v1";

/// Disposable editor index. Rebuild after edits; never serialize derived timing.
#[derive(Default, Clone)]
pub struct TimelineIndex {
    starts: Vec<f64>,
    assets: HashMap<Uuid, usize>,
    clips: HashMap<Uuid, usize>,
}

impl TimelineIndex {
    pub fn new(project: &Project) -> Self {
        let mut starts = Vec::with_capacity(project.clips.len() + 1);
        starts.push(0.0);
        for clip in &project.clips {
            starts.push(starts.last().unwrap() + clip.duration());
        }
        Self {
            starts,
            assets: project
                .assets
                .iter()
                .enumerate()
                .map(|(i, a)| (a.id, i))
                .collect(),
            clips: project
                .clips
                .iter()
                .enumerate()
                .map(|(i, c)| (c.id, i))
                .collect(),
        }
    }

    pub fn duration(&self) -> f64 {
        self.starts.last().copied().unwrap_or(0.0)
    }
    pub fn start(&self, index: usize) -> f64 {
        self.starts[index]
    }
    pub fn clip_index(&self, id: Uuid) -> Option<usize> {
        self.clips.get(&id).copied()
    }
    pub fn asset<'a>(&self, project: &'a Project, id: Uuid) -> Option<&'a MediaAsset> {
        project.assets.get(*self.assets.get(&id)?)
    }
    pub fn clip_at<'a>(&self, project: &'a Project, time: f64) -> Option<(usize, f64, &'a Clip)> {
        if !time.is_finite() || time < 0.0 || time > self.duration() || project.clips.is_empty() {
            return None;
        }
        let i = self
            .starts
            .partition_point(|start| *start <= time)
            .saturating_sub(1)
            .min(project.clips.len() - 1);
        Some((i, self.starts[i], &project.clips[i]))
    }
    pub fn visible(&self, from: f64, to: f64) -> std::ops::Range<usize> {
        let count = self.starts.len().saturating_sub(1);
        let first = self
            .starts
            .partition_point(|start| *start <= from)
            .saturating_sub(1)
            .min(count);
        let end = self.starts.partition_point(|start| *start <= to).min(count);
        first..end.max(first)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub format: String,
    pub name: String,
    pub assets: Vec<MediaAsset>,
    pub clips: Vec<Clip>,
    pub render: RenderSettings,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            format: FORMAT_VERSION.to_owned(),
            name: "Untitled cut".to_owned(),
            assets: vec![],
            clips: vec![],
            render: RenderSettings::default(),
        }
    }
}

impl Project {
    pub fn duration(&self) -> f64 {
        self.clips.iter().map(Clip::duration).sum()
    }

    pub fn asset(&self, id: Uuid) -> Option<&MediaAsset> {
        self.assets.iter().find(|asset| asset.id == id)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let json = serde_json::to_string_pretty(self)?;
        fs::write(path, json).with_context(|| format!("could not save {}", path.display()))
    }

    pub fn load(path: &Path) -> Result<Self> {
        let data = fs::read_to_string(path)
            .with_context(|| format!("could not read {}", path.display()))?;
        let mut project: Self = serde_json::from_str(&data)
            .with_context(|| format!("{} is not a valid fastCutVid project", path.display()))?;
        project
            .validate()
            .with_context(|| format!("{} failed timeline validation", path.display()))?;

        // Agent-created projects can use paths relative to the JSON document,
        // making a timeline and its media folder portable as one unit.
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        for asset in &mut project.assets {
            let asset_path = PathBuf::from(&asset.path);
            if asset_path.is_relative() {
                asset.path = base.join(asset_path).to_string_lossy().into_owned();
            }
        }
        Ok(project)
    }

    pub fn validate(&self) -> Result<()> {
        if self.format != FORMAT_VERSION {
            bail!("unsupported project format: {}", self.format);
        }
        if self.name.trim().is_empty() {
            bail!("project name cannot be empty");
        }
        if self.render.width == 0 || self.render.height == 0 || self.render.fps == 0 {
            bail!("render width, height, and fps must be greater than zero");
        }
        if self.render.video_codec.trim().is_empty() || self.render.audio_codec.trim().is_empty() {
            bail!("render codec names cannot be empty");
        }

        let mut asset_ids = HashSet::new();
        let mut assets = HashMap::new();
        for asset in &self.assets {
            if !asset_ids.insert(asset.id) {
                bail!("duplicate asset id: {}", asset.id);
            }
            if asset.name.trim().is_empty() || asset.path.trim().is_empty() {
                bail!("asset {} must have a name and path", asset.id);
            }
            if !asset.duration.is_finite() || asset.duration <= 0.0 {
                bail!("asset {} has an invalid duration", asset.id);
            }
            if asset.width == 0 || asset.height == 0 || !asset.fps.is_finite() || asset.fps <= 0.0 {
                bail!("asset {} has invalid video dimensions or fps", asset.id);
            }
            if !matches!(asset.rotation, 0 | 90 | 180 | 270) {
                bail!("asset {} has an invalid display rotation", asset.id);
            }
            assets.insert(asset.id, asset);
        }

        let mut clip_ids = HashSet::new();
        for clip in &self.clips {
            if !clip_ids.insert(clip.id) {
                bail!("duplicate clip id: {}", clip.id);
            }
            let Some(asset) = assets.get(&clip.asset_id) else {
                bail!("clip {} refers to missing asset {}", clip.id, clip.asset_id);
            };
            if !clip.source_in.is_finite()
                || !clip.source_out.is_finite()
                || clip.source_in < 0.0
                || clip.source_out <= clip.source_in
                || clip.source_out > asset.duration + 0.000_1
            {
                bail!("clip {} has an invalid source range", clip.id);
            }
            if !clip.audio_gain.is_finite() || clip.audio_gain < 0.0 {
                bail!("clip {} has an invalid audio gain", clip.id);
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn clip_at(&self, time: f64) -> Option<(usize, f64, &Clip)> {
        let mut cursor = 0.0;
        for (index, clip) in self.clips.iter().enumerate() {
            let end = cursor + clip.duration();
            if time >= cursor && (time < end || (index + 1 == self.clips.len() && time <= end)) {
                return Some((index, cursor, clip));
            }
            cursor = end;
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaAsset {
    pub id: Uuid,
    pub name: String,
    pub path: String,
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub has_audio: bool,
    /// Clockwise display rotation from the source container metadata.
    #[serde(default)]
    pub rotation: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: Uuid,
    pub asset_id: Uuid,
    pub source_in: f64,
    pub source_out: f64,
    #[serde(default = "default_gain")]
    pub audio_gain: f32,
    #[serde(default)]
    pub muted: bool,
}

fn default_gain() -> f32 {
    1.0
}

impl Clip {
    pub fn from_asset(asset: &MediaAsset) -> Self {
        Self {
            id: Uuid::new_v4(),
            asset_id: asset.id,
            source_in: 0.0,
            source_out: asset.duration,
            audio_gain: 1.0,
            muted: false,
        }
    }

    pub fn duration(&self) -> f64 {
        (self.source_out - self.source_in).max(0.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderSettings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub video_codec: String,
    pub audio_codec: String,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 30,
            video_codec: "libx264".to_owned(),
            audio_codec: "aac".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_with_one_clip() -> Project {
        let asset = MediaAsset {
            id: Uuid::new_v4(),
            name: "source.mp4".to_owned(),
            path: "media/source.mp4".to_owned(),
            duration: 10.0,
            width: 1920,
            height: 1080,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
        };
        let clip = Clip {
            id: Uuid::new_v4(),
            asset_id: asset.id,
            source_in: 2.0,
            source_out: 5.0,
            audio_gain: 1.0,
            muted: false,
        };
        Project {
            assets: vec![asset],
            clips: vec![clip],
            ..Project::default()
        }
    }

    #[test]
    fn index_matches_linear_timing_after_edits_and_bounds_visible_work() {
        let mut project = project_with_one_clip();
        for _ in 0..10_000 {
            let mut clip = project.clips[0].clone();
            clip.id = Uuid::new_v4();
            project.clips.push(clip);
        }
        for edit in 0..3 {
            if edit == 1 {
                project.clips[5].source_out -= 0.3;
            }
            if edit == 2 {
                project.clips.remove(4);
                project.clips.swap(0, 5);
            }
            let index = TimelineIndex::new(&project);
            assert_eq!(index.duration(), project.duration());
            for time in [
                -1.0,
                0.0,
                2.99,
                3.0,
                15.0,
                27_005.5,
                index.duration(),
                index.duration() + 0.1,
            ] {
                assert_eq!(index.clip_at(&project, time), project.clip_at(time));
            }
            let visible = index.visible(27_000.0, 27_060.0);
            assert!(
                visible.len() <= 22,
                "Offscreen clips entered visible layout"
            );
            assert_eq!(
                index.asset(&project, project.assets[0].id),
                Some(&project.assets[0])
            );
            assert_eq!(index.clip_index(project.clips[5].id), Some(5));
        }
        assert!(TimelineIndex::default().visible(0.0, 10.0).is_empty());
    }

    #[test]
    fn validates_agent_constructed_timeline() {
        project_with_one_clip().validate().expect("valid project");
    }

    #[test]
    fn rejects_clip_outside_source_duration() {
        let mut project = project_with_one_clip();
        project.clips[0].source_out = 12.0;
        assert!(project.validate().is_err());
    }

    #[test]
    fn project_round_trips_with_new_and_legacy_extensions() {
        let temp = tempfile::tempdir().unwrap();
        for name in ["edit.fastcut", "edit.fastcut.json", "edit.json"] {
            let path = temp.path().join(name);
            let project = Project::default();
            project.save(&path).unwrap();
            let loaded = Project::load(&path).unwrap();
            assert_eq!(loaded.name, project.name);
            assert_eq!(loaded.format, FORMAT_VERSION);
        }
    }

    #[test]
    fn resolves_paths_relative_to_timeline_json() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let path = temp.path().join("edit.fastcut.json");
        fs::write(&path, serde_json::to_vec(&project_with_one_clip()).unwrap()).unwrap();
        let loaded = Project::load(&path).expect("load project");
        assert_eq!(
            Path::new(&loaded.assets[0].path),
            temp.path().join("media/source.mp4")
        );
    }
}
