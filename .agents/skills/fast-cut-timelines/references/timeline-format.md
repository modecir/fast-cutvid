# fastCutVid timeline format

## Editing model

`fastcut.timeline/v1` is a non-destructive edit decision list. `assets` describe source files. `clips` are played consecutively in array order, so the timeline contains no explicit start positions or gaps. Reuse an asset ID in multiple clips to take several ranges from the same source.

The final duration is the sum of every `source_out - source_in`. All times are seconds and may be fractional.

fastCutVid v1 has one linked video/audio sequence. It does not represent overlaps, transitions, effects, titles, nested sequences, independent audio tracks, or speed changes.

## Project fields

- `format`: Must be `fastcut.timeline/v1`.
- `name`: Human-readable project name.
- `assets`: Source media records.
- `clips`: Ordered edit decisions.
- `render`: Output dimensions, frame rate, and FFmpeg codec names.

## Asset fields

- `id`: Unique UUID referenced by clips.
- `name`: Display name, normally the source filename.
- `path`: Absolute path or path relative to the timeline JSON file.
- `duration`: Exact source duration greater than zero.
- `width`, `height`: Encoded source dimensions greater than zero.
- `fps`: Source frame rate greater than zero.
- `has_audio`: Whether the source has an audio stream.
- `rotation`: Clockwise display rotation: `0`, `90`, `180`, or `270`. Omit only for legacy projects where zero is intended.

Use `ffprobe -v error -show_streams -show_format -of json VIDEO` to obtain these values. FFprobe display-matrix rotation uses the opposite sign from fastCutVid's clockwise `rotation` value.

## Clip fields

- `id`: Unique UUID for this edit decision.
- `asset_id`: UUID of an entry in `assets`.
- `source_in`: Inclusive source start time, at least zero.
- `source_out`: Source end time, greater than `source_in` and no greater than asset duration.
- `audio_gain`: Linear gain. `1.0` is unchanged, `0.5` is half amplitude, and `2.0` is double amplitude.
- `muted`: When true, this clip contributes silence.

Clip source ranges are unaffected by their position in the array. Reordering clips changes the program order without changing their trims.

## Complete multi-asset example

```json
{
  "format": "fastcut.timeline/v1",
  "name": "Interview assembly",
  "assets": [
    {
      "id": "7cadf900-4e0a-4e68-bc12-d0493b6c70e2",
      "name": "interview.mov",
      "path": "media/interview.mov",
      "duration": 184.52,
      "width": 3840,
      "height": 2160,
      "fps": 29.97,
      "has_audio": true,
      "rotation": 0
    },
    {
      "id": "046882d1-a4c1-44c7-96c6-bffe579cd9ee",
      "name": "broll.mp4",
      "path": "media/broll.mp4",
      "duration": 63.2,
      "width": 1920,
      "height": 1080,
      "fps": 30.0,
      "has_audio": false,
      "rotation": 0
    }
  ],
  "clips": [
    {
      "id": "c022eb4d-bd67-4ad0-a181-e3745e91d3e8",
      "asset_id": "7cadf900-4e0a-4e68-bc12-d0493b6c70e2",
      "source_in": 12.4,
      "source_out": 24.8,
      "audio_gain": 1.0,
      "muted": false
    },
    {
      "id": "cedaa6ae-f7e6-498a-b64e-917958c0e9fd",
      "asset_id": "046882d1-a4c1-44c7-96c6-bffe579cd9ee",
      "source_in": 3.0,
      "source_out": 7.0,
      "audio_gain": 1.0,
      "muted": true
    },
    {
      "id": "258150ac-72cf-4e7c-919e-12214df72dc0",
      "asset_id": "7cadf900-4e0a-4e68-bc12-d0493b6c70e2",
      "source_in": 40.0,
      "source_out": 52.0,
      "audio_gain": 1.0,
      "muted": false
    }
  ],
  "render": {
    "width": 1920,
    "height": 1080,
    "fps": 30,
    "video_codec": "libx264",
    "audio_codec": "aac"
  }
}
```

This example produces a 28.4-second sequence. Dropping the JSON onto fastCutVid opens the complete project and rebuilds filmstrips and waveforms in the background.
