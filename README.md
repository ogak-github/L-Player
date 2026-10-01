# L-Player

Video/audio player built with Flutter. Playback runs on libmpv, driven from Rust
through flutter_rust_bridge. See `CoreConcept.md` for the feature list.

# Progress
- CPU Rendering (In Progress)  
- GPU Rendering (In Progress, Linux)
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
                           └─ render thread (rust/src/video): mpv GPU or SW render -> irondash_texture
```

- The playlist (order, shuffle, subtitles per item) lives in Dart. Rust plays a
  single file at a time and reports `endOfFile` so Dart can move to the next item.
- On Linux, mpv renders with OpenGL ES in its own EGL context on the render
  thread. Each frame texture is shared with Flutter as an EGLImage and shown
  through a Flutter GL texture (irondash_texture), so frames never touch the
  CPU and hardware decoding can stay zero-copy (VA-API). EGL is loaded at
  runtime, no extra packages needed.
- When the GPU path can't start (or with `LPLAYER_RENDERER=sw`), mpv's software
  render API draws into RGBA buffers shown through a pixel buffer texture. Big
  videos are downscaled to 1080p there to keep the copy cheap.

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
