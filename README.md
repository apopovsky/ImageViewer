# ImageViewer

Fast, lightweight image and video viewer and quick editor for Windows, built with Rust and `egui`.

## Features

- **Wide format support**: Standard images (`JPG`, `PNG`, `WebP`, `BMP`, `TIFF`, `TGA`, `ICO`, `QOI`, `HDR`, `EXR`), animated `GIF`/`WebP`, vector `SVG`/`SVGZ`, extended/RAW formats (`HEIC`, `AVIF`, `JXL`, `PSD`, `CR2`/`CR3`/`NEF`/`ARW`/`DNG`, etc. via ImageMagick/FFmpeg), and video playback (via FFmpeg).
- **Gallery & preloading**: Thumbnail grid with search, sorting, and multi-threaded background decoding.
- **Quick editor**: Rotate, flip, crop with aspect ratios, resize with resampling filters, undo stack, overwrite or save copy.
- **Metadata inspector**: Searchable EXIF & file metadata panel with GPS Google Maps link.
- **Windows integration**: User-level installer & file association manager (`--setup`, `--associate`, `--uninstall`).
- **Multi-language**: English (default), Español, Português, Français, Deutsch, Italiano — switchable from the top-right `🌐` menu (persisted in `HKCU\Software\ImageViewer`).

## Build & Run

Requires [Rust](https://www.rust-lang.org/tools/install) (and optionally `ffmpeg`/`ffprobe` and ImageMagick `magick` in `PATH` for video and RAW/HEIC/AVIF decoding).

```bash
cargo build --release
```

The binary will be at `target/release/ImageViewer.exe`.

- Launch installer / file associations: `ImageViewer.exe --setup` (or `Instalar-ImageViewer.bat`)
- Optional CLI language override: `ImageViewer.exe --lang en` (`en`, `es`, `pt`, `fr`, `de`, `it`)

## Shortcuts

- `Ctrl+O` / `Ctrl+Shift+O`: Open file / folder
- `Left` / `Right` / Mouse wheel: Previous / next item
- `Enter` / `Esc`: Toggle Gallery / Single view
- `Ctrl+Wheel` / `+` / `-` / `0` / `1`: Zoom, Fit, 100%
- `Space`: Play / pause video or animation
- `C` / `Ctrl+E` / `Ctrl+L` / `Ctrl+R` / `Ctrl+Z` / `Ctrl+S`: Crop, resize, rotate, undo, save
- `I` / `T` / `F11` / `Del`: Info panel, transparency grid, fullscreen, Recycle Bin
