use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk, pango};

/// Normalized output corners: top-left, top-right, bottom-right, bottom-left.
pub type Corners = [[f32; 2]; 4];
pub const DEFAULT_CORNERS: Corners = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

/// Checkerboard with a centred ring, used to adjust the tilt without a slideshow.
pub fn test_pattern() -> image::RgbaImage {
    image::RgbaImage::from_fn(1920, 1080, |x, y| {
        let ring = ((x as f32 - 960.0).hypot(y as f32 - 540.0) - 400.0).abs();
        if ring < 6.0 {
            return image::Rgba([255, 255, 255, 255]);
        }
        let v = if (x / 120 + y / 120) % 2 == 0 { 70 } else { 40 };
        image::Rgba([v, v, v + 20, 255])
    })
}

/// Projective transform mapping the rect (0, 0, w, h) onto the quad spanned by `corners`.
fn homography(corners: &Corners, w: f32, h: f32) -> Option<graphene::Matrix> {
    let (w, h) = (f64::from(w), f64::from(h));
    let [(x0, y0), (x1, y1), (x2, y2), (x3, y3)] =
        corners.map(|[x, y]| (f64::from(x) * w, f64::from(y) * h));
    // Heckbert's unit-square-to-quad mapping.
    let (dx1, dx2, dx3) = (x1 - x2, x3 - x2, x0 - x1 + x2 - x3);
    let (dy1, dy2, dy3) = (y1 - y2, y3 - y2, y0 - y1 + y2 - y3);
    let det = dx1 * dy2 - dx2 * dy1;
    if det.abs() < 1e-9 {
        return None;
    }
    let g = (dx3 * dy2 - dx2 * dy3) / det;
    let k = (dx1 * dy3 - dx3 * dy1) / det;
    let (a, b) = (x1 - x0 + g * x1, x3 - x0 + k * x3);
    let (d, e) = (y1 - y0 + g * y1, y3 - y0 + k * y3);
    // graphene multiplies row vectors, so each column is one output coordinate.
    let m = [
        a / w, d / w, 0.0, g / w,
        b / h, e / h, 0.0, k / h,
        0.0, 0.0, 1.0, 0.0,
        x0, y0, 0.0, 1.0,
    ];
    Some(graphene::Matrix::from_float(m.map(|v| v as f32)))
}

fn draw_grid(snapshot: &gtk::Snapshot, w: f32, h: f32) {
    let color = gdk::RGBA::new(1.0, 1.0, 1.0, 0.8);
    let t = 3.0;
    for i in 0..=4 {
        let f = i as f32 / 4.0;
        snapshot.append_color(&color, &graphene::Rect::new((w - t) * f, 0.0, t, h));
        snapshot.append_color(&color, &graphene::Rect::new(0.0, (h - t) * f, w, t));
    }
}

/// Ken Burns parameters; pan values are fractions (-1..1) of the overflowing image area.
struct Motion {
    zoom_from: f32,
    zoom_to: f32,
    pan_from: (f32, f32),
    pan_to: (f32, f32),
}

impl Motion {
    fn random() -> Self {
        let max_zoom = 1.05 + fastrand::f32() * 0.04;
        let (zoom_from, zoom_to) = if fastrand::bool() { (1.0, max_zoom) } else { (max_zoom, 1.0) };
        let rand_pan = || (fastrand::f32() - 0.5, fastrand::f32() - 0.5);
        Self { zoom_from, zoom_to, pan_from: rand_pan(), pan_to: rand_pan() }
    }
}

struct Current {
    texture: gdk::Texture,
    sponsor: bool,
    shown_at: Instant,
    motion: Motion,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct SlideView {
        pub(super) current: RefCell<Option<Current>>,
        pub(super) zoom_pan: Cell<bool>,
        pub(super) duration: Cell<Duration>,
        pub(super) watermark: RefCell<Option<gdk::Texture>>,
        pub(super) watermark_opacity: Cell<f32>,
        pub(super) tilt: Cell<bool>,
        pub(super) corners: Cell<Corners>,
        pub(super) adjusting: Cell<bool>,
        /// Corner being dragged and its position when the drag started.
        pub(super) drag: Cell<Option<(usize, [f32; 2])>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SlideView {
        const NAME: &'static str = "SlideView";
        type Type = super::SlideView;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for SlideView {
        fn constructed(&self) {
            self.parent_constructed();
            self.corners.set(DEFAULT_CORNERS);

            let obj = self.obj();
            let drag = gtk::GestureDrag::new();
            let weak = obj.downgrade();
            drag.connect_drag_begin(move |_, x, y| {
                if let Some(v) = weak.upgrade() {
                    v.imp().begin_drag(x as f32, y as f32);
                }
            });
            let weak = obj.downgrade();
            drag.connect_drag_update(move |_, dx, dy| {
                if let Some(v) = weak.upgrade() {
                    v.imp().update_drag(dx as f32, dy as f32);
                }
            });
            let weak = obj.downgrade();
            drag.connect_drag_end(move |_, _, _| {
                if let Some(v) = weak.upgrade() {
                    v.imp().drag.set(None);
                }
            });
            obj.add_controller(drag);
        }
    }

    impl SlideView {
        fn size(&self) -> (f32, f32) {
            let obj = self.obj();
            (obj.width() as f32, obj.height() as f32)
        }

        fn begin_drag(&self, x: f32, y: f32) {
            if !self.adjusting.get() {
                return;
            }
            let (w, h) = self.size();
            let corners = self.corners.get();
            let dist = |i: usize| (corners[i][0] * w - x).hypot(corners[i][1] * h - y);
            let nearest = (0..4).min_by(|&a, &b| dist(a).total_cmp(&dist(b))).expect("4 corners");
            self.drag.set(Some((nearest, corners[nearest])));
        }

        fn update_drag(&self, dx: f32, dy: f32) {
            let Some((i, [sx, sy])) = self.drag.get() else { return };
            let (w, h) = self.size();
            let mut corners = self.corners.get();
            corners[i] = [(sx + dx / w).clamp(0.0, 1.0), (sy + dy / h).clamp(0.0, 1.0)];
            self.corners.set(corners);
            self.obj().queue_draw();
        }

        fn draw_handles(&self, snapshot: &gtk::Snapshot, w: f32, h: f32) {
            let accent = gdk::RGBA::new(0.21, 0.52, 0.89, 1.0);
            for [x, y] in self.corners.get() {
                for (r, color) in [(18.0, gdk::RGBA::WHITE), (12.0, accent)] {
                    let rect = graphene::Rect::new(x * w - r, y * h - r, 2.0 * r, 2.0 * r);
                    snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(rect, r));
                    snapshot.append_color(&color, &rect);
                    snapshot.pop();
                }
            }
        }

        fn draw_hint(&self, snapshot: &gtk::Snapshot, w: f32, h: f32) {
            let layout = self.obj().create_pango_layout(None);
            layout.set_markup(
                "<span size='x-large'>Drag the corners until the picture is rectangular on the wall</span>\n\
                 R reset · K done",
            );
            layout.set_alignment(pango::Alignment::Center);
            let (lw, lh) = layout.pixel_size();
            let (lw, lh) = (lw as f32, lh as f32);
            let (x, y) = ((w - lw) / 2.0, (h - lh) / 2.0);
            let bg = graphene::Rect::new(x - 20.0, y - 14.0, lw + 40.0, lh + 28.0);
            snapshot.append_color(&gdk::RGBA::new(0.0, 0.0, 0.0, 0.75), &bg);
            snapshot.save();
            snapshot.translate(&graphene::Point::new(x, y));
            snapshot.append_layout(&layout, &gdk::RGBA::WHITE);
            snapshot.restore();
        }

        fn draw_slide(&self, snapshot: &gtk::Snapshot, w: f32, h: f32) {
            let bounds = graphene::Rect::new(0.0, 0.0, w, h);
            snapshot.append_color(&gdk::RGBA::BLACK, &bounds);

            let current = self.current.borrow();
            let Some(cur) = current.as_ref() else { return };
            let (tw, th) = (cur.texture.width() as f32, cur.texture.height() as f32);
            let fit = (w / tw).min(h / th);

            let (zoom, (px, py)) = if self.zoom_pan.get() && !cur.sponsor {
                let t = (cur.shown_at.elapsed().as_secs_f32() / self.duration.get().as_secs_f32())
                    .min(1.0);
                let m = &cur.motion;
                let lerp = |a: f32, b: f32| a + (b - a) * t;
                (
                    lerp(m.zoom_from, m.zoom_to),
                    (lerp(m.pan_from.0, m.pan_to.0), lerp(m.pan_from.1, m.pan_to.1)),
                )
            } else {
                (1.0, (0.0, 0.0))
            };

            // Zoom and pan happen inside the fitted frame, so the frame on screen never changes size.
            let (bw, bh) = (tw * fit, th * fit);
            let frame = graphene::Rect::new((w - bw) / 2.0, (h - bh) / 2.0, bw, bh);
            let (iw, ih) = (bw * zoom, bh * zoom);
            let cx = w / 2.0 + px * (iw - bw) / 2.0;
            let cy = h / 2.0 + py * (ih - bh) / 2.0;
            let rect = graphene::Rect::new(cx - iw / 2.0, cy - ih / 2.0, iw, ih);

            snapshot.push_clip(&frame);
            snapshot.append_scaled_texture(&cur.texture, gsk::ScalingFilter::Trilinear, &rect);
            snapshot.pop();

            if !cur.sponsor
                && let Some(wm) = self.watermark.borrow().as_ref()
            {
                let (right, bottom) = ((w + bw) / 2.0, (h + bh) / 2.0);
                let (ww, wh) = (wm.width() as f32, wm.height() as f32);
                let scale = (bw * 0.2 / ww).min(bh * 0.12 / wh);
                let (ww, wh) = (ww * scale, wh * scale);
                let margin = bw.min(bh) * 0.03;
                let rect = graphene::Rect::new(right - margin - ww, bottom - margin - wh, ww, wh);
                snapshot.push_opacity(self.watermark_opacity.get().into());
                snapshot.append_scaled_texture(wm, gsk::ScalingFilter::Trilinear, &rect);
                snapshot.pop();
            }
        }
    }

    impl WidgetImpl for SlideView {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let (w, h) = self.size();
            snapshot.append_color(&gdk::RGBA::BLACK, &graphene::Rect::new(0.0, 0.0, w, h));

            let adjusting = self.adjusting.get();
            let warp = if self.tilt.get() || adjusting {
                homography(&self.corners.get(), w, h)
            } else {
                None
            };
            snapshot.save();
            if let Some(m) = &warp {
                snapshot.transform_matrix(m);
            }
            self.draw_slide(snapshot, w, h);
            if adjusting {
                draw_grid(snapshot, w, h);
            }
            snapshot.restore();

            if adjusting {
                self.draw_handles(snapshot, w, h);
                self.draw_hint(snapshot, w, h);
            }
        }
    }
}

glib::wrapper! {
    pub struct SlideView(ObjectSubclass<imp::SlideView>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl SlideView {
    pub fn new() -> Self {
        glib::Object::builder().property("hexpand", true).property("vexpand", true).build()
    }

    pub fn set_slide(&self, image: image::RgbaImage, sponsor: bool) {
        let (w, h) = image.dimensions();
        let texture = gdk::MemoryTexture::new(
            w as i32,
            h as i32,
            gdk::MemoryFormat::R8g8b8a8,
            &glib::Bytes::from_owned(image.into_raw()),
            w as usize * 4,
        );
        self.imp().current.replace(Some(Current {
            texture: texture.upcast(),
            sponsor,
            shown_at: Instant::now(),
            motion: Motion::random(),
        }));
        self.queue_draw();
    }

    pub fn clear(&self) {
        self.imp().current.replace(None);
        self.queue_draw();
    }

    pub fn elapsed(&self) -> Option<Duration> {
        self.imp().current.borrow().as_ref().map(|c| c.shown_at.elapsed())
    }

    pub fn set_watermark(&self, texture: Option<gdk::Texture>) {
        self.imp().watermark.replace(texture);
        self.queue_draw();
    }

    pub fn set_tilt(&self, enabled: bool) {
        self.imp().tilt.set(enabled);
        self.queue_draw();
    }

    pub fn corners(&self) -> Corners {
        self.imp().corners.get()
    }

    pub fn set_corners(&self, corners: Corners) {
        self.imp().corners.set(corners);
        self.queue_draw();
    }

    pub fn is_adjusting(&self) -> bool {
        self.imp().adjusting.get()
    }

    /// Shows draggable corner handles to fit the picture to a tilted projector.
    pub fn set_adjusting(&self, adjusting: bool) {
        let imp = self.imp();
        imp.adjusting.set(adjusting);
        imp.drag.set(None);
        self.set_cursor_from_name(Some(if adjusting { "crosshair" } else { "none" }));
        self.queue_draw();
    }

    pub fn update(&self, zoom_pan: bool, duration: Duration, watermark_opacity: f32) {
        let imp = self.imp();
        imp.duration.set(duration);
        let changed = imp.zoom_pan.replace(zoom_pan) != zoom_pan
            || imp.watermark_opacity.replace(watermark_opacity) != watermark_opacity;
        let animating = zoom_pan && imp.current.borrow().as_ref().is_some_and(|c| !c.sponsor);
        if changed || animating {
            self.queue_draw();
        }
    }
}
