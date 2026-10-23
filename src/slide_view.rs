use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

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
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SlideView {
        const NAME: &'static str = "SlideView";
        type Type = super::SlideView;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for SlideView {}

    impl WidgetImpl for SlideView {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let (w, h) = (widget.width() as f32, widget.height() as f32);
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

            let (iw, ih) = (tw * fit * zoom, th * fit * zoom);
            let cx = w / 2.0 + px * (iw - w).max(0.0) / 2.0;
            let cy = h / 2.0 + py * (ih - h).max(0.0) / 2.0;
            let rect = graphene::Rect::new(cx - iw / 2.0, cy - ih / 2.0, iw, ih);

            snapshot.push_clip(&bounds);
            snapshot.append_scaled_texture(&cur.texture, gsk::ScalingFilter::Trilinear, &rect);
            snapshot.pop();

            if !cur.sponsor
                && let Some(wm) = self.watermark.borrow().as_ref()
            {
                // Anchored to the unzoomed image so it stays put during zoom and pan.
                let (bw, bh) = (tw * fit, th * fit);
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
