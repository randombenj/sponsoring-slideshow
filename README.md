# Sponsoring Slideshow

A GTK4/libadwaita slideshow for club events. It shows all photos in a folder (including subfolders) in random order and shows a page of a sponsoring PDF after every N photos.

## Features

- Shows all JPG/PNG images found recursively in the chosen folder, shuffled, with no repeats until every image has been shown
- Shows one page of a sponsoring PDF in the folder after every N photos, cycling through its pages (default: `sponsoring.pdf`, every 10 photos)
- Optional slow zoom and pan effect
- Optional watermark (e.g. a transparent PNG logo) in the bottom right corner of photos, with adjustable opacity
- Skips images that are too small to look good fullscreen

Settings are in the ☰ menu and are saved to `~/.config/sponsoring-slideshow/settings.json`.

## Keyboard shortcuts

| Key | Action |
| --- | --- |
| <kbd>F11</kbd> | Toggle fullscreen |
| <kbd>Esc</kbd> | Stop the slideshow |
| <kbd>Space</kbd> / <kbd>→</kbd> | Next slide |
| <kbd>Z</kbd> | Toggle zoom and pan |

## Requirements

GTK ≥ 4.14, libadwaita ≥ 1.5 and `pdftoppm` (from poppler-utils) for rendering the PDF. On Ubuntu 24.04:

```sh
sudo apt install libgtk-4-dev libadwaita-1-dev poppler-utils
```

## Build and run

```sh
cargo run --release
```

## AppImage

```sh
./packaging/build-appimage.sh            # builds target/appimage/SponsoringSlideshow-x86_64.AppImage
./packaging/build-appimage.sh --install  # also copies it to ~/Applications
```

The AppImage uses GTK, libadwaita and `pdftoppm` from the host system instead of bundling them.
