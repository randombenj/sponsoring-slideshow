use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread;

use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageReader, RgbaImage};
use walkdir::WalkDir;

/// Larger images are downscaled to keep decoding and GPU uploads fast.
const MAX_DIM: u32 = 2560;
/// Photos below this resolution (either orientation) look too pixelated when shown fullscreen.
const MIN_LONG_SIDE: u32 = 500;
const MIN_SHORT_SIDE: u32 = 300;
const IMAGE_EXTENSIONS: [&str; 3] = ["jpg", "jpeg", "png"];

pub struct Slide {
    pub image: RgbaImage,
    pub sponsor: bool,
}

pub enum Msg {
    Slide(Slide),
    Error(String),
}

pub struct Config {
    pub folder: PathBuf,
    pub pdf: PathBuf,
    /// 0 disables sponsor pages.
    pub images_per_sponsor: usize,
}

/// Starts a producer thread that decodes slides ahead of time. It stops once the receiver is dropped.
pub fn spawn(cfg: Config) -> Receiver<Msg> {
    let (tx, rx) = sync_channel(2);
    thread::spawn(move || {
        if let Err(e) = run(&cfg, &tx) {
            let _ = tx.send(Msg::Error(e));
        }
    });
    rx
}

fn run(cfg: &Config, tx: &SyncSender<Msg>) -> Result<(), String> {
    let mut images = scan_images(&cfg.folder);
    if images.is_empty() {
        return Err(format!("No JPG/PNG images found in {}", cfg.folder.display()));
    }

    let sponsors = if cfg.images_per_sponsor > 0 && cfg.pdf.is_file() {
        render_pdf(&cfg.pdf)?
    } else {
        Vec::new()
    };

    let mut order: Vec<PathBuf> = Vec::new();
    let mut since_sponsor = 0;
    let mut sponsor_idx = 0;

    loop {
        let slide = if !sponsors.is_empty() && since_sponsor >= cfg.images_per_sponsor {
            since_sponsor = 0;
            let image = sponsors[sponsor_idx % sponsors.len()].clone();
            sponsor_idx += 1;
            Slide { image, sponsor: true }
        } else {
            if order.is_empty() {
                order = images.clone();
                fastrand::shuffle(&mut order);
            }
            let path = order.pop().expect("order was refilled");
            match check_resolution(&path).and_then(|()| load_image(&path)) {
                Ok(image) => {
                    since_sponsor += 1;
                    Slide { image, sponsor: false }
                }
                Err(e) => {
                    eprintln!("Skipping {}: {e}", path.display());
                    images.retain(|p| *p != path);
                    if images.is_empty() {
                        return Err("No usable images (unreadable or resolution too low)".into());
                    }
                    continue;
                }
            }
        };
        if tx.send(Msg::Slide(slide)).is_err() {
            return Ok(());
        }
    }
}

fn scan_images(folder: &Path) -> Vec<PathBuf> {
    WalkDir::new(folder)
        .follow_links(true)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| IMAGE_EXTENSIONS.iter().any(|x| ext.eq_ignore_ascii_case(x)))
        })
        .map(|e| e.into_path())
        .collect()
}

/// Renders all PDF pages via poppler's `pdftoppm`.
fn render_pdf(pdf: &Path) -> Result<Vec<RgbaImage>, String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let output = Command::new("pdftoppm")
        .args(["-png", "-scale-to", &MAX_DIM.to_string()])
        .arg(pdf)
        .arg(dir.path().join("page"))
        .output()
        .map_err(|e| format!("Failed to run pdftoppm (install poppler-utils): {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "pdftoppm failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    // pdftoppm zero-pads page numbers, so lexical order equals page order.
    let mut pages: Vec<PathBuf> = std::fs::read_dir(dir.path())
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    pages.sort();
    pages.iter().map(|p| load_image(p)).collect()
}

fn check_resolution(path: &Path) -> Result<(), String> {
    let (w, h) = image::image_dimensions(path).map_err(|e| e.to_string())?;
    if w.max(h) < MIN_LONG_SIDE || w.min(h) < MIN_SHORT_SIDE {
        return Err(format!("resolution too low ({w}x{h})"));
    }
    Ok(())
}

fn load_image(path: &Path) -> Result<RgbaImage, String> {
    let mut decoder = ImageReader::open(path)
        .and_then(|r| r.with_guessed_format())
        .map_err(|e| e.to_string())?
        .into_decoder()
        .map_err(|e| e.to_string())?;
    let orientation = decoder.orientation().map_err(|e| e.to_string())?;
    let mut img = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    img.apply_orientation(orientation);
    if img.width() > MAX_DIM || img.height() > MAX_DIM {
        img = img.resize(MAX_DIM, MAX_DIM, FilterType::Triangle);
    }
    Ok(img.into_rgba8())
}
