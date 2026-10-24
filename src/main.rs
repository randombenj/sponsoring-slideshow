mod slide_view;
mod slides;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use serde::{Deserialize, Serialize};

use slide_view::{Corners, DEFAULT_CORNERS, SlideView, test_pattern};
use slides::Msg;

const APP_ID: &str = "io.github.randombenj.SponsoringSlideshow";

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    pdf_name: String,
    slide_secs: f64,
    images_per_sponsor: u32,
    zoom_pan: bool,
    watermark: Option<PathBuf>,
    watermark_opacity: f64,
    tilt: bool,
    tilt_corners: Corners,
    recent_folders: Vec<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            pdf_name: "sponsoring.pdf".into(),
            slide_secs: 6.0,
            images_per_sponsor: 10,
            zoom_pan: true,
            watermark: None,
            watermark_opacity: 0.2,
            tilt: false,
            tilt_corners: DEFAULT_CORNERS,
            recent_folders: Vec::new(),
        }
    }
}

impl Settings {
    fn path() -> PathBuf {
        glib::user_config_dir().join("sponsoring-slideshow/settings.json")
    }

    fn load() -> Self {
        std::fs::read(Self::path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        let path = Self::path();
        let json = serde_json::to_vec_pretty(self).expect("settings are serializable");
        let result = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, json));
        if let Err(e) = result {
            eprintln!("Failed to save settings to {}: {e}", path.display());
        }
    }
}

struct Slideshow {
    settings: Rc<RefCell<Settings>>,
    stack: gtk::Stack,
    view: SlideView,
    message: gtk::Label,
    recent: gtk::Box,
    rx: RefCell<Option<Receiver<Msg>>>,
    tick: RefCell<Option<gtk::TickCallbackId>>,
}

impl Slideshow {
    fn is_running(&self) -> bool {
        self.rx.borrow().is_some()
    }

    fn start(self: &Rc<Self>, folder: PathBuf) {
        self.stop();
        {
            let mut s = self.settings.borrow_mut();
            s.recent_folders.retain(|f| *f != folder);
            s.recent_folders.insert(0, folder.clone());
            s.recent_folders.truncate(2);
            s.save();
        }
        self.refresh_recent();
        let cfg = {
            let s = self.settings.borrow();
            slides::Config {
                pdf: folder.join(&s.pdf_name),
                folder,
                images_per_sponsor: s.images_per_sponsor as usize,
            }
        };
        self.rx.replace(Some(slides::spawn(cfg)));
        self.show_message("Loading images…");
        self.stack.set_visible_child_name("show");

        let weak = Rc::downgrade(self);
        let id = self.view.add_tick_callback(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.advance(false);
            }
            glib::ControlFlow::Continue
        });
        self.tick.replace(Some(id));
    }

    fn refresh_recent(self: &Rc<Self>) {
        while let Some(child) = self.recent.first_child() {
            self.recent.remove(&child);
        }
        let folders = self.settings.borrow().recent_folders.clone();
        for (i, folder) in folders.into_iter().filter(|f| f.is_dir()).enumerate() {
            let name = folder
                .file_name()
                .map_or_else(|| folder.display().to_string(), |n| n.to_string_lossy().into_owned());
            let button = gtk::Button::builder()
                .label(format!("Resume “{name}”"))
                .tooltip_text(folder.display().to_string())
                .css_classes(["pill"])
                .build();
            if i == 0 {
                button.add_css_class("suggested-action");
            }
            let this = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(this) = this.upgrade() {
                    this.start(folder.clone());
                }
            });
            self.recent.append(&button);
        }
    }

    fn stop(&self) {
        self.set_adjusting(false);
        if let Some(id) = self.tick.take() {
            id.remove();
        }
        self.rx.take();
        self.view.clear();
        self.stack.set_visible_child_name("empty");
    }

    fn advance(&self, skip: bool) {
        let (zoom_pan, duration, watermark_opacity) = {
            let s = self.settings.borrow();
            (s.zoom_pan, Duration::from_secs_f64(s.slide_secs), s.watermark_opacity as f32)
        };
        if skip || self.view.elapsed().is_none_or(|e| e >= duration) {
            let msg = self.rx.borrow().as_ref().and_then(|rx| rx.try_recv().ok());
            match msg {
                Some(Msg::Slide(slide)) => {
                    self.view.set_slide(slide.image, slide.sponsor);
                    self.message.set_visible(false);
                }
                Some(Msg::Error(e)) => self.show_message(&format!("{e}\n\nPress Esc to go back")),
                None => {}
            }
        }
        self.view.update(zoom_pan, duration, watermark_opacity);
    }

    fn adjust_tilt(&self) {
        if !self.is_running() {
            self.view.set_slide(test_pattern(), true);
            self.message.set_visible(false);
            self.stack.set_visible_child_name("show");
        }
        self.set_adjusting(true);
    }

    fn set_adjusting(&self, adjusting: bool) {
        if !adjusting && self.view.is_adjusting() {
            let mut s = self.settings.borrow_mut();
            s.tilt_corners = self.view.corners();
            s.save();
            if !self.is_running() {
                self.view.clear();
                self.stack.set_visible_child_name("empty");
            }
        }
        self.view.set_adjusting(adjusting);
    }

    fn show_message(&self, text: &str) {
        self.message.set_markup(&format!(
            "<span foreground=\"white\">{}</span>",
            glib::markup_escape_text(text)
        ));
        self.message.set_visible(true);
    }
}

fn open_folder(window: &adw::ApplicationWindow, show: &Rc<Slideshow>) {
    let dialog = gtk::FileDialog::builder().title("Open Slideshow Folder").modal(true).build();
    let show = show.clone();
    dialog.select_folder(Some(window), gio::Cancellable::NONE, move |res| {
        if let Some(path) = res.ok().and_then(|f| f.path()) {
            show.start(path);
        }
    });
}

fn load_watermark(path: &Path) -> Option<gdk::Texture> {
    gdk::Texture::from_filename(path)
        .inspect_err(|e| eprintln!("Failed to load watermark {}: {e}", path.display()))
        .ok()
}

fn file_label(path: Option<&Path>) -> String {
    path.and_then(Path::file_name)
        .map_or_else(|| "None".into(), |n| n.to_string_lossy().into_owned())
}

fn watermark_rows(settings: &Rc<RefCell<Settings>>, view: &SlideView) -> (adw::ActionRow, adw::SpinRow) {
    let s = settings.borrow();
    view.set_watermark(s.watermark.as_deref().and_then(load_watermark));

    let row = adw::ActionRow::builder()
        .title("Watermark")
        .subtitle(file_label(s.watermark.as_deref()))
        .use_markup(false)
        .build();
    let choose = gtk::Button::builder()
        .icon_name("document-open-symbolic")
        .tooltip_text("Choose Watermark Image")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let clear = gtk::Button::builder()
        .icon_name("edit-clear-symbolic")
        .tooltip_text("Remove Watermark")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .sensitive(s.watermark.is_some())
        .build();
    row.add_suffix(&choose);
    row.add_suffix(&clear);

    let (st, v, r, c) = (settings.clone(), view.clone(), row.clone(), clear.clone());
    choose.connect_clicked(move |btn| {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Images"));
        filter.add_mime_type("image/png");
        filter.add_mime_type("image/jpeg");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title("Choose Watermark Image")
            .modal(true)
            .filters(&filters)
            .build();
        let parent = btn.root().and_downcast::<gtk::Window>();
        let (st, v, r, c) = (st.clone(), v.clone(), r.clone(), c.clone());
        dialog.open(parent.as_ref(), gio::Cancellable::NONE, move |res| {
            let Some(path) = res.ok().and_then(|f| f.path()) else { return };
            let Some(texture) = load_watermark(&path) else { return };
            v.set_watermark(Some(texture));
            r.set_subtitle(&file_label(Some(&path)));
            c.set_sensitive(true);
            let mut s = st.borrow_mut();
            s.watermark = Some(path);
            s.save();
        });
    });

    let (st, v, r) = (settings.clone(), view.clone(), row.clone());
    clear.connect_clicked(move |btn| {
        v.set_watermark(None);
        r.set_subtitle(&file_label(None));
        btn.set_sensitive(false);
        let mut s = st.borrow_mut();
        s.watermark = None;
        s.save();
    });

    let opacity = adw::SpinRow::with_range(0.05, 1.0, 0.05);
    opacity.set_title("Watermark opacity");
    opacity.set_digits(2);
    opacity.set_value(s.watermark_opacity);
    let st = settings.clone();
    opacity.connect_value_notify(move |row| {
        let mut s = st.borrow_mut();
        s.watermark_opacity = row.value();
        s.save();
    });

    (row, opacity)
}

fn settings_popover(
    settings: &Rc<RefCell<Settings>>,
    view: &SlideView,
) -> (gtk::Popover, adw::SwitchRow, adw::SwitchRow, adw::ActionRow) {
    let (watermark, watermark_opacity) = watermark_rows(settings, view);
    let s = settings.borrow();

    let secs = adw::SpinRow::with_range(1.0, 120.0, 1.0);
    secs.set_title("Seconds per slide");
    secs.set_value(s.slide_secs);
    let st = settings.clone();
    secs.connect_value_notify(move |row| {
        let mut s = st.borrow_mut();
        s.slide_secs = row.value();
        s.save();
    });

    let every = adw::SpinRow::with_range(0.0, 100.0, 1.0);
    every.set_title("Images between sponsor pages");
    every.set_subtitle("0 disables sponsor pages");
    every.set_value(s.images_per_sponsor.into());
    let st = settings.clone();
    every.connect_value_notify(move |row| {
        let mut s = st.borrow_mut();
        s.images_per_sponsor = row.value() as u32;
        s.save();
    });

    let pdf = adw::EntryRow::builder().title("Sponsoring PDF in folder").text(&s.pdf_name).build();
    let st = settings.clone();
    pdf.connect_changed(move |row| {
        let mut s = st.borrow_mut();
        s.pdf_name = row.text().into();
        s.save();
    });

    let zoom = adw::SwitchRow::builder().title("Zoom and pan effect").active(s.zoom_pan).build();
    let st = settings.clone();
    zoom.connect_active_notify(move |row| {
        let mut s = st.borrow_mut();
        s.zoom_pan = row.is_active();
        s.save();
    });

    view.set_tilt(s.tilt);
    view.set_corners(s.tilt_corners);
    let tilt = adw::SwitchRow::builder()
        .title("Compensate projector tilt")
        .active(s.tilt)
        .build();
    let (st, v) = (settings.clone(), view.clone());
    tilt.connect_active_notify(move |row| {
        v.set_tilt(row.is_active());
        let mut s = st.borrow_mut();
        s.tilt = row.is_active();
        s.save();
    });

    let adjust = adw::ActionRow::builder()
        .title("Adjust tilt")
        .subtitle("Drag the corners on a test image")
        .activatable(true)
        .build();
    adjust.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));

    let group = adw::PreferencesGroup::builder()
        .title("Slideshow")
        .description("F11 fullscreen · Esc stop · Space next slide · Z zoom and pan · K adjust tilt")
        .build();
    group.add(&secs);
    group.add(&every);
    group.add(&pdf);
    group.add(&zoom);
    group.add(&watermark);
    group.add(&watermark_opacity);
    group.add(&tilt);
    group.add(&adjust);
    group.set_margin_top(6);
    group.set_margin_bottom(6);
    group.set_margin_start(6);
    group.set_margin_end(6);
    group.set_width_request(380);

    (gtk::Popover::builder().child(&group).build(), zoom, tilt, adjust)
}

fn build_ui(app: &adw::Application) {
    let settings = Rc::new(RefCell::new(Settings::load()));
    let view = SlideView::new();
    let (popover, zoom_row, tilt_row, adjust_row) = settings_popover(&settings, &view);

    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text("Main Menu")
        .popover(&popover)
        .build();
    let header = adw::HeaderBar::new();
    header.pack_end(&menu_button);

    let empty = adw::StatusPage::builder()
        .icon_name("folder-pictures-symbolic")
        .title("Click to Open Slideshow Folder")
        .description("All JPG and PNG images in the folder and its subfolders are shown in random order")
        .build();
    empty.set_cursor_from_name(Some("pointer"));
    let recent = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .halign(gtk::Align::Center)
        .build();
    empty.set_child(Some(&recent));

    view.set_cursor_from_name(Some("none"));
    let message = gtk::Label::builder()
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .justify(gtk::Justification::Center)
        .wrap(true)
        .css_classes(["title-2"])
        .can_target(false)
        .build();
    let overlay = gtk::Overlay::builder().child(&view).build();
    overlay.add_overlay(&message);

    let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();
    stack.add_named(&empty, Some("empty"));
    stack.add_named(&overlay, Some("show"));

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&stack));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Sponsoring Slideshow")
        .default_width(1280)
        .default_height(720)
        .content(&toolbar)
        .build();
    window.connect_fullscreened_notify(move |w| toolbar.set_reveal_top_bars(!w.is_fullscreen()));

    let show = Rc::new(Slideshow {
        settings,
        stack,
        view,
        message,
        recent: recent.clone(),
        rx: RefCell::new(None),
        tick: RefCell::new(None),
    });
    show.refresh_recent();

    let s = show.clone();
    window.connect_close_request(move |_| {
        s.stop();
        glib::Propagation::Proceed
    });

    let click = gtk::GestureClick::new();
    let (w, s, e) = (window.clone(), show.clone(), empty.clone());
    click.connect_released(move |_, _, x, y| {
        // Clicks on the Resume buttons must not also open the folder dialog.
        if e.pick(x, y, gtk::PickFlags::DEFAULT).is_some_and(|t| t == recent || t.is_ancestor(&recent)) {
            return;
        }
        open_folder(&w, &s);
    });
    empty.add_controller(click);

    let (p, t, s) = (popover.clone(), tilt_row.clone(), show.clone());
    adjust_row.connect_activated(move |_| {
        p.popdown();
        t.set_active(true);
        s.adjust_tilt();
    });

    // Capture phase so Space isn't swallowed by a focused header button.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let w = window.clone();
    keys.connect_key_pressed(move |_, key, _, _| {
        let menu_open = popover.is_visible();
        let running = show.is_running() && !menu_open;
        let adjusting = !menu_open && show.view.is_adjusting();
        match key {
            gdk::Key::F11 => w.set_fullscreened(!w.is_fullscreen()),
            gdk::Key::Escape if adjusting => show.set_adjusting(false),
            gdk::Key::Escape if running => {
                show.stop();
                w.unfullscreen();
            }
            gdk::Key::Escape if !menu_open && w.is_fullscreen() => w.unfullscreen(),
            gdk::Key::space | gdk::Key::Right if running => show.advance(true),
            gdk::Key::z | gdk::Key::Z if running => zoom_row.set_active(!zoom_row.is_active()),
            gdk::Key::k | gdk::Key::K if adjusting => show.set_adjusting(false),
            gdk::Key::k | gdk::Key::K if running => {
                tilt_row.set_active(true);
                show.set_adjusting(true);
            }
            gdk::Key::r | gdk::Key::R if adjusting => show.view.set_corners(DEFAULT_CORNERS),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    window.add_controller(keys);

    window.present();
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}
