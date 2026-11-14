//! Decks desktop shell: everything the native app does around [`iridium_decks_ui_egui::SlideApp`]
//! except what is process-wide (logging, GPU backend, the macOS process exit, the updater),
//! which `apps/iridium-decks` keeps.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod audio;
mod control_server;
// The system pasteboard. A browser tab has its own clipboard API and
// arboard has no wasm32 backend.
#[cfg(not(target_arch = "wasm32"))]
mod pasteboard;

use iridium_decks_engine::Session;
use iridium_decks_ui_egui::{Services, SlideApp};

/// What the app was started with.
#[derive(Default)]
pub struct Launch {
    /// `--control PORT` / `IRIDIUM_DECKS_CONTROL_PORT`: JSON-lines control server on 127.0.0.1.
    pub control_port: Option<u16>,
    /// Files to open; a `.ppsx` given last starts its slide show.
    pub files: Vec<String>,
    /// `--sample`: open the sample presentation.
    pub sample: bool,
    /// `--show`: start the slide show.
    pub show: bool,
    /// Files opened from Finder and quit requests (the process-wide Apple-event handler stays in `main`).
    #[cfg(target_os = "macos")]
    pub apple_events: fmv_macos_events::Inbox,
}

pub struct Shell {
    app: SlideApp,
    fonts: egui::FontDefinitions,
    style: egui::Style,
    zoom: f32,
    #[cfg(target_os = "macos")]
    apple_events: fmv_macos_events::Inbox,
}

/// The per-user settings directory: `ui.json` and `prefs.json` live here.
fn config_dir() -> Option<std::path::PathBuf> {
    if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Library/Application Support/Decks"))
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(|a| std::path::PathBuf::from(a).join("Decks"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
            .map(|c| c.join("iridium-decks"))
    }
}

fn load_prefs(app: &mut SlideApp) {
    if std::env::var_os("IRIDIUM_DECKS_NO_PREFS").is_some() {
        return;
    }
    let Some(dir) = config_dir() else { return };
    let (ui, prefs) = iridium_decks_ui_egui::settings::load(&dir);
    if let Some(ui) = ui {
        app.ui = ui;
    }
    if let Some(prefs) = prefs {
        app.session.prefs = prefs;
    }
}

fn services() -> Services {
    Services {
        // The native file dialog is blocking; rfd has only the async dialog in a browser, which a sync
        // closure cannot await. Unset on wasm32: the web host supplies its own file flow.
        #[cfg(not(target_arch = "wasm32"))]
        pick_open: Some(Box::new(|purpose: &str| {
            let d = rfd::FileDialog::new();
            let d = match purpose {
                "picture" => d.add_filter("Pictures", &["png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff"]),
                "audio" => d.add_filter(
                    "Audio",
                    &["wav", "mp3", "m4a", "m4b", "aac", "flac", "ogg", "oga", "opus", "aif", "aiff", "aifc", "caf", "wma", "weba", "mka"],
                ),
                "video" => d.add_filter("Video", &["mp4", "m4v", "mov", "webm", "mkv", "wmv"]),
                "theme" => d.add_filter("Themes and presentations", &["thmx", "pptx", "potx", "ppsx"]).add_filter("Office Theme", &["thmx"]),
                _ => d
                    .add_filter("Presentations", &["iridium-decks", "pptx", "potx", "ppsx"])
                    .add_filter("Decks", &["iridium-decks"])
                    .add_filter("PowerPoint", &["pptx", "potx", "ppsx"])
                    .add_filter("Outline", &["txt", "md"]),
            };
            d.pick_file().map(|p| p.to_string_lossy().to_string())
        })),
        #[cfg(not(target_arch = "wasm32"))]
        pick_save: Some(Box::new(|name: &str| rfd::FileDialog::new().set_file_name(name).save_file().map(|p| p.to_string_lossy().to_string()))),
        read: Some(Box::new(|p: &str| std::fs::read(p).map_err(|e| e.to_string()))),
        write: Some(Box::new(|p: &str, b: &[u8]| std::fs::write(p, b).map_err(|e| e.to_string()))),
        #[cfg(not(target_arch = "wasm32"))]
        clipboard_image: Some(Box::new(pasteboard::image_png)),
        #[cfg(not(target_arch = "wasm32"))]
        clipboard_html: Some(Box::new(pasteboard::html)),
        #[cfg(not(target_arch = "wasm32"))]
        clipboard_set_html: Some(Box::new(pasteboard::set_html)),
        // Media is mixed and clocked by the output device; `IRIDIUM_DECKS_NO_AUDIO` plays silently.
        audio_out: std::env::var_os("IRIDIUM_DECKS_NO_AUDIO")
            .is_none()
            .then(|| Box::new(audio::CpalOut::default()) as Box<dyn iridium_decks_media::AudioOut>),
        ..Default::default()
    }
}

/// Open one path, reporting a failure in the log and the status line.
fn open_reporting(app: &mut SlideApp, p: &str) {
    if let Err(e) = app.open_path(p) {
        let safe_p = p.replace(['\r', '\n'], " ");
        log::error!("{safe_p}: {e}");
        app.set_status(format!("Couldn't open {safe_p}: {e}"));
    }
}

impl Shell {
    /// Build the shell: services, preferences, AutoRecover, the control server, then the files
    /// and flags the app was started with.
    ///
    /// Thin wrapper over [`Shell::new_for_host`]; Decks has no extra that needs the real window.
    pub fn new(cc: &eframe::CreationContext<'_>, launch: Launch) -> Self {
        Self::new_for_host(&cc.egui_ctx, cc.storage, launch)
    }

    /// Host-facing constructor: no `CreationContext`, so a host can build the module later.
    /// `storage` is unused: Decks keeps its settings in files (`config_dir`).
    ///
    /// `SlideApp` installs its fonts, theme and zoom lazily on its first frame, so this installs
    /// the same ones now (the first frame repeats them harmlessly) and captures them.
    pub fn new_for_host(ctx: &egui::Context, _storage: Option<&dyn eframe::Storage>, launch: Launch) -> Self {
        iridium_decks_ui_egui::install_ime_pass_hooks(ctx);
        let mut app = SlideApp::new(Session::new(), services());
        load_prefs(&mut app);
        // The web build never touches the network on its own.
        #[cfg(not(target_arch = "wasm32"))]
        app.enable_update_checks();
        // AutoRecover: reopen what a previous run left unsaved, then keep it current.
        if std::env::var_os("IRIDIUM_DECKS_NO_RECOVERY").is_none() {
            app.session.recovery_dir = iridium_decks_engine::recovery::default_dir();
            if let Ok(v) = app.session.execute("file.recovery.open", &serde_json::json!({}))
                && let Some(n) = v.get("opened").and_then(|o| o.as_array()).map(Vec::len).filter(|n| *n > 0)
            {
                app.set_status(format!("Recovered {n} unsaved presentation{} from the last session", if n == 1 { "" } else { "s" }));
            }
        }
        app.integrated_titlebar = cfg!(target_os = "macos");
        if let Some(port) = launch.control_port {
            let rx = control_server::start(port, ctx.clone());
            app = app.with_control(rx);
        }
        if launch.sample
            && let Err(e) = iridium_decks_engine::sample::open_sample(&mut app.session)
        {
            log::error!("sample: {e}");
        }
        // A .ppsx opened last starts its slide show (#862).
        for (f, e) in app.open_from_system(&launch.files) {
            log::error!("{f}: {e}");
        }
        if launch.show {
            app.start_show(0, false);
        }
        use iridium_decks_ui_egui::theme;
        let zoom = app.ui.ui_scale.clamp(0.5, 3.0);
        ctx.set_zoom_factor(zoom);
        let fonts = theme::font_definitions(&app.ui_fonts.faces);
        ctx.set_fonts(fonts.clone());
        theme::apply(ctx, &theme::Tokens::for_brightness(app.ui.brightness));
        Self {
            app,
            fonts,
            style: ctx.global_style().as_ref().clone(),
            zoom,
            #[cfg(target_os = "macos")]
            apple_events: launch.apple_events,
        }
    }

    /// The font set this module installed, captured at construction.
    pub fn fonts(&self) -> egui::FontDefinitions {
        self.fonts.clone()
    }

    /// The style this module left behind, captured at construction.
    pub fn style(&self) -> egui::Style {
        self.style.clone()
    }

    /// The zoom factor this module left behind, captured at construction.
    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    /// Re-install the native menu. Decks builds no native menu of its own (the menu bar is
    /// drawn in egui; the macOS app menu is the windowing library's), so this is a no-op.
    pub fn install_native_menu(&self) {}

    /// Per-frame logic, called before `ui`. On macOS first opens the documents and acts on the
    /// quit requests that arrived as Apple events since the last frame.
    pub fn logic(&mut self, ctx: &egui::Context) {
        #[cfg(target_os = "macos")]
        for e in self.apple_events.drain() {
            match e {
                fmv_macos_events::Event::Open(paths) => {
                    for p in paths.iter().filter_map(|p| p.to_str()) {
                        open_reporting(&mut self.app, p);
                    }
                }
                fmv_macos_events::Event::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }
        self.app.logic(ctx);
    }

    /// Native raw input, forwarded to the app once per pass.
    pub fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.app.raw_input_hook(ctx, raw);
    }

    /// Draw the app into the given region.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.app.ui(ui);
    }

    /// Open a presentation; a failure is logged and shown in the status line.
    pub fn open_path(&mut self, path: &str) {
        open_reporting(&mut self.app, path);
    }

    /// Save the preferences and UI state (and, on macOS, remove the temporary print PDFs).
    pub fn save_state(&mut self) {
        if std::env::var_os("IRIDIUM_DECKS_NO_PREFS").is_none()
            && let Some(dir) = config_dir()
        {
            iridium_decks_ui_egui::settings::save(&dir, &self.app.ui, &self.app.session.prefs);
        }
        #[cfg(target_os = "macos")]
        self.app.cleanup_print_previews();
    }

    /// False while a presentation has unsaved changes: the same test `SlideApp::logic` uses to
    /// ask before closing the window. Decks has no separate in-flight-job guard.
    pub fn can_close(&self) -> bool {
        !self.app.session.documents().iter().any(|d| d.is_dirty())
    }
}
