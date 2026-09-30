# L-Player

Video/audio player built with Flutter. Playback runs on libmpv, driven from Rust
through flutter_rust_bridge. See `CoreConcept.md` for the feature list.

# Progress
- CPU Rendering (In Progress)  
- GPU Rendering (Not Yet)
Linux (InProgress)
MacOS (Not Yet)
Windows (Not Yet)
iOS (Not Yet)
Android (Not Yet)

## How it fits together

```
Flutter UI (lib/ui)  ──>  PlayerController + PlaylistController (lib/player)
        │                              │ flutter_rust_bridge
        │ Texture(textureId)           ▼
        └──────────────  LPlayer (rust/src/api/player.rs)
                           ├─ event thread: mpv events -> PlayerEvent stream
                           └─ render thread: mpv SW render -> irondash_texture
```

- The playlist (order, shuffle, subtitles per item) lives in Dart. Rust plays a
  single file at a time and reports `endOfFile` so Dart can move to the next item.
- Video frames are rendered by mpv's software render API into RGBA buffers and
  shown through a Flutter pixel buffer texture (irondash_texture). Big videos are
  downscaled to 1080p to keep the copy cheap.

## Setup (Linux)

```sh
sudo apt install libmpv-dev
flutter pub get
flutter run -d linux
```

After changing the public API in `rust/src/api/`, regenerate the bindings:

```sh
flutter_rust_bridge_codegen generate
```

## Shortcuts

| Key | Action |
| --- | --- |
| Space | Play / pause |
| ← / → | Seek back / forward by the selected step |
| ↑ / ↓ | Volume ±5 |
| `[` / `]` | Slower / faster |
| N / P | Next / previous |
| S | Subtitles on / off |
| F | Fullscreen on / off |
| Esc | Exit fullscreen |
| L | Show / hide playlist |
