//! Decks desktop app.
//!
//! Usage: `iridium-decks [--control <port>] [--sample] [--show] [files…]`
//! A PowerPoint Show (`.ppsx`) given last starts its slide show, as a double-click does in PowerPoint.
//! `iridium-decks --help` prints the flags and exits. `iridium-decks --version` prints the version and exits.
//!
//! `--control <port>` (or `IRIDIUM_DECKS_CONTROL_PORT`) starts a localhost JSON-lines control server:
//! `{"id":1,"method":"ui.inspect","params":{}}` → `{"id":1,"ok":true,"result":…}`.
//! See `iridium_decks_ui_egui::control` for the methods.
//!
//! `log` records go to standard error and `<settings dir>/logs/iridium-decks.log` (`RUST_LOG` sets the
//! levels); see [`logging`].
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

#[cfg(target_os = "macos")]
mod apple_events;
#[cfg(all(unix, not(target_os = "macos")))]
mod display;
#[cfg(any(target_os = "windows", test))]
mod graphics;
mod logging;

use iridium_decks_shell::{Launch, Shell};

struct App(Shell);

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.0.logic(ctx);
    }
    /// Forward native raw input to the wrapped application once per pass.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.0.raw_input_hook(ctx, raw);
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.ui(ui);
    }
    fn on_exit(&mut self) {
        self.0.save_state();
        end_process();
    }
}

/// macOS (#279): end the process here, before eframe tears the window down. AccessKit's
/// macOS adapter subclasses the window's content view and restores its class on teardown;
/// AppKit's Touch Bar support observes that view (KVO), so in the next display cycle
/// `-[_NSTouchBarFinderObservation invalidate]` throws "not registered" and the quit ends
/// in a crash report. eframe calls `on_exit` on every way of quitting (⌘Q, the close
/// button, `app.quit`), once the unsaved-changes prompt has let the window close, and
/// nothing after it is needed: the preferences are saved and the log file is unbuffered.
/// `on_exit` only runs once the app exists, never on a failed start. Same as the upstream app does.
/// This stays out of the shell library: in a suite it would end every app.
fn end_process() {
    #[cfg(target_os = "macos")]
    std::process::exit(0);
}

/// The per-user settings directory: `ui.json`, `prefs.json` and `logs/` live here.
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

const APP_ID: &str = "ph.decks";

const USAGE: &str = "\
Usage: iridium-decks [--control PORT] [--sample] [--show] [FILE...]

  --control PORT  JSON control server on 127.0.0.1:PORT
  --sample        open the sample presentation
  --show          start the slide show
  --help, -h      print this help and exit
  --version, -V   print the version and exit

A PowerPoint Show (.ppsx) given last starts its slide show.
";

enum Startup {
    Help,
    Version,
    Run { control_port: Option<u16>, files: Vec<String>, sample: bool, show: bool },
}

/// `--help` and `--version` leave before the window and the log file.
fn startup(mut control_port: Option<u16>, args: impl IntoIterator<Item = String>) -> Startup {
    let mut files = Vec::new();
    let mut sample = false;
    let mut show = false;
    let mut args = args.into_iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--control" => control_port = args.next().and_then(|p| p.parse().ok()),
            "--sample" => sample = true,
            "--show" => show = true,
            "--version" | "-V" => return Startup::Version,
            "--help" | "-h" => return Startup::Help,
            // The process serial number older macOS adds when Finder launches the app.
            a if a.starts_with("-psn_") => {}
            _ => files.push(a),
        }
    }
    Startup::Run { control_port, files, sample, show }
}

fn app_icon() -> Option<egui::IconData> {
    let png: &[u8] = include_bytes!("../../../assets/app-icon/hicolor/256x256/apps/ph.decks.png");
    eframe::icon_data::from_png_bytes(png).map_err(|e| log::warn!("app icon: {e}")).ok()
}

/// Install logging, parse startup arguments and environment, launch the native app, and
/// propagate an eframe startup failure.
fn main() -> eframe::Result {
    // First, so every start-up record (and the engine's panic hook, installed with the first
    // Session) is captured; see `logging`.
    let logger = logging::install();
    if let Some(done) = craft_update::self_update::finish(&iridium_decks_ui_egui::UPDATE_APP, &std::env::args_os().skip(1).collect::<Vec<_>>()) {
        if let Err(e) = done {
            eprintln!("Decks update helper failed: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }
    let (control_port, files, sample, show) =
        match startup(std::env::var("IRIDIUM_DECKS_CONTROL_PORT").ok().and_then(|p| p.parse().ok()), std::env::args().skip(1)) {
            Startup::Help => {
                print!("{USAGE}");
                return Ok(());
            }
            Startup::Version => {
                println!("iridium-decks {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            Startup::Run { control_port, files, sample, show } => (control_port, files, sample, show),
        };
    log::info!("Decks {} ({} {})", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH);
    // The log file lives under the settings directory; opened after the arguments, so `--help` and
    // `--version` leave no file behind. Records logged until now are written to it first.
    if let Some(logger) = logger {
        match config_dir().map(|d| logger.attach_dir(&d.join(logging::LOG_DIR))) {
            Some(Ok(path)) => log::info!("log file {}", path.display()),
            Some(Err(e)) => log::warn!("no log file: {e}"),
            None => {
                logger.stderr_only();
                log::warn!("no log file: no settings directory (HOME, XDG_CONFIG_HOME or APPDATA is not set)");
            }
        }
    }
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Decks")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([900.0, 560.0])
            .with_drag_and_drop(true)
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false),
        ..Default::default()
    };
    options.viewport = options.viewport.with_app_id(APP_ID);
    if let Some(icon) = app_icon() {
        options.viewport = options.viewport.with_icon(icon);
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    let x11 = match display::choose_from_env() {
        display::Choice::X11 => true,
        display::Choice::Default => false,
        display::Choice::None => {
            // winit's own error says little to someone who started Decks from a console.
            log::error!("no display: DISPLAY and WAYLAND_DISPLAY are not set");
            eprintln!("Decks needs a graphical session: neither DISPLAY nor WAYLAND_DISPLAY is set.");
            false
        }
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    if x11 {
        options.event_loop_builder = Some(Box::new(|b| {
            use winit::platform::x11::EventLoopBuilderExtX11;
            b.with_x11();
        }));
    }
    // Before eframe creates the wgpu instance: default Windows to DirectX 12 with FXC (see graphics.rs).
    #[cfg(target_os = "windows")]
    graphics::configure(&mut options, eframe::wgpu::Backends::from_env(), eframe::wgpu::Dx12Compiler::from_env());
    // Registered before the event loop starts, so it catches the Finder event that launched us as
    // well as later ones. Lives until the event loop returns; the app creator only borrows it.
    #[cfg(target_os = "macos")]
    let apple_events = apple_events::AppleEvents::install();
    #[cfg(target_os = "macos")]
    let apple_events = &apple_events;
    eframe::run_native(
        "Decks",
        options,
        Box::new(move |cc| {
            Ok(Box::new(App(Shell::new(
                cc,
                Launch {
                    control_port,
                    files,
                    sample,
                    show,
                    #[cfg(target_os = "macos")]
                    apple_events: apple_events.connect(&cc.egui_ctx),
                },
            ))))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #279: on macOS the process ends inside `on_exit`, before eframe would tear the window down.
    /// The check runs in a child (this test binary again), since on macOS it ends the process.
    #[test]
    fn on_exit_ends_the_process_on_macos() {
        if std::env::var_os("IRIDIUM_DECKS_TEST_ON_EXIT").is_some() {
            println!("calling on_exit");
            end_process();
            println!("on_exit returned");
            return;
        }
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::on_exit_ends_the_process_on_macos", "--nocapture"])
            .env("IRIDIUM_DECKS_TEST_ON_EXIT", "1")
            .env("IRIDIUM_DECKS_NO_PREFS", "1")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{stdout}");
        assert!(stdout.contains("calling on_exit"), "the child didn't run the test: {stdout}");
        assert_eq!(stdout.contains("on_exit returned"), !cfg!(target_os = "macos"), "{stdout}");
    }

    #[test]
    fn help_exits_instead_of_opening_a_file() {
        assert!(matches!(startup(None, ["--help".to_string()]), Startup::Help));
        assert!(matches!(startup(None, ["-h".to_string()]), Startup::Help));
        assert!(matches!(startup(None, ["talk.pptx".to_string(), "--help".to_string()]), Startup::Help));
        assert!(matches!(startup(None, ["--version".to_string()]), Startup::Version));
        let launch = startup(Some(9), ["--sample".to_string(), "--control".to_string(), "4242".to_string(), "talk.pptx".to_string()]);
        assert!(matches!(launch, Startup::Run { .. }));
        if let Startup::Run { control_port, files, sample, show } = launch {
            assert_eq!(control_port, Some(4242));
            assert_eq!(files, ["talk.pptx".to_string()]);
            assert!(sample);
            assert!(!show);
        }
    }

    #[test]
    fn finder_process_serial_number_is_not_a_file() {
        let launch = startup(None, ["-psn_0_1234567".to_string(), "talk.pptx".to_string()]);
        assert!(matches!(launch, Startup::Run { .. }));
        if let Startup::Run { files, .. } = launch {
            assert_eq!(files, ["talk.pptx".to_string()]);
        }
    }

    /// The `<string>` values of every `<array>` that follows a `<key>{key}</key>` line.
    fn plist_strings(plist: &str, key: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut in_array = false;
        for line in plist.lines() {
            let line = line.trim();
            if line == format!("<key>{key}</key>") {
                in_array = true;
                continue;
            }
            if in_array {
                for part in line.split("<string>").skip(1) {
                    if let Some((s, _)) = part.split_once("</string>") {
                        out.push(s.to_string());
                    }
                }
                if line.ends_with("</array>") {
                    in_array = false;
                }
            }
        }
        out
    }

    /// Every desktop offers the same file types: macOS (`CFBundleTypeExtensions` and the native
    /// type's extension in `Info.plist.in`), Windows (`SupportedTypes` in `iridium-decks.wxs`) and
    /// Linux (`MimeType` in the desktop file). A format added to one installer must reach the
    /// others, and each must be one Finder, Explorer or the file manager can hand to Decks.
    #[test]
    fn every_desktop_offers_the_same_file_types() {
        let plist = include_str!("../../../packaging/macos/Info.plist.in");
        let wxs = include_str!("../../../packaging/windows/iridium-decks.wxs");
        let desktop = include_str!("../../../packaging/linux/ph.decks.desktop");

        let mut mac = plist_strings(plist, "CFBundleTypeExtensions");
        // The native type is declared by its UTI; its extension is in the exported type.
        mac.extend(plist_strings(plist, "public.filename-extension").into_iter().filter(|e| e == "iridium-decks"));
        mac.sort();
        mac.dedup();
        assert!(mac.iter().any(|e| e == "pptx") && mac.iter().any(|e| e == "iridium-decks"), "Info.plist.in: {mac:?}");
        // Every document type names a content type too, so "Change All" in Finder's Get Info works.
        let names = plist.matches("<key>CFBundleTypeName</key>").count();
        let content_types = plist.matches("<key>LSItemContentTypes</key>").count();
        assert!(names > 0 && names == content_types, "{names} document types, {content_types} with LSItemContentTypes");

        let mut win: Vec<String> = wxs
            .lines()
            .filter(|l| l.contains("SupportedTypes\" Name=\"."))
            .filter_map(|l| l.split("Name=\".").nth(1)?.split('"').next().map(str::to_string))
            .collect();
        win.sort();

        let mime_ext = |m: &str| match m {
            "application/x-iridium-decks" => Some("iridium-decks"),
            "application/vnd.openxmlformats-officedocument.presentationml.presentation" => Some("pptx"),
            "application/vnd.openxmlformats-officedocument.presentationml.template" => Some("potx"),
            "application/vnd.openxmlformats-officedocument.presentationml.slideshow" => Some("ppsx"),
            _ => None,
        };
        let mut linux: Vec<String> = desktop
            .lines()
            .find_map(|l| l.strip_prefix("MimeType="))
            .unwrap_or_default()
            .split(';')
            .filter(|m| !m.is_empty())
            .map(|m| mime_ext(m).unwrap_or(m).to_string()) // an unknown MIME type shows up in the diff
            .collect();
        linux.sort();

        assert_eq!(mac, win, "macOS and Windows offer different file types");
        assert_eq!(mac, linux, "macOS and Linux offer different file types");
    }

    /// #1114: a shortcut that names an MSI `<Icon>` shows a blank icon once an update deletes the
    /// cached copy of it, and a taskbar pin (a copy of the shortcut) is never repaired. Shortcuts
    /// take the icon embedded in `iridium-decks.exe`; only `ARPPRODUCTICON` uses the MSI `<Icon>`.
    #[test]
    fn windows_shortcuts_use_the_icon_embedded_in_the_exe() {
        let wxs = include_str!("../../../packaging/windows/iridium-decks.wxs");
        let shortcuts: Vec<&str> = wxs.split("<Shortcut").skip(1).filter_map(|s| s.split("/>").next()).collect();
        assert!(!shortcuts.is_empty(), "iridium-decks.wxs has no shortcut");
        for shortcut in shortcuts {
            assert!(!shortcut.contains("Icon="), "a shortcut names an MSI <Icon>:{shortcut}");
        }
        assert!(wxs.contains("<Property Id=\"ARPPRODUCTICON\""), "Apps and features keeps its icon");
    }

    /// The shortcuts take the exe's own icon, and `package.ps1 -SkipBuild` packages an existing
    /// exe without `IRIDIUM_DECKS_REQUIRE_WINRES`, so the script has to refuse one with no icon.
    #[test]
    fn the_windows_package_script_rejects_an_exe_without_an_icon() {
        let script = include_str!("../../../packaging/windows/package.ps1");
        assert!(script.contains("ExtractIconEx"), "package.ps1 does not count the exe's icons");
        assert!(script.contains("has no embedded icon"), "package.ps1 does not reject an iconless exe");
    }
}
