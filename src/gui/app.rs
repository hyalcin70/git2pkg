//! eframe/egui-Oberfläche mit System-Theme, Sprachwahl, Engine-Wahl
//! und deutsch/englisch/türkisch/spanisch/französischer Internationalisierung.

use crate::core::container::{
    cached_repo_has_local_dependencies, cleanup_failed_build_artifacts, cleanup_failed_builds,
    container_engine_available, default_output_dir, detect_engine, run_build, BuildConfig,
    BuildEvent, BuildOutcome, BuildStrategy, ContainerEngine, TargetFormat,
};
use crate::core::distro::{detect_distro, DistroFamily, DistroInfo, TargetDistro};
use crate::core::resolver::MappingStore;
use crate::gui::i18n::{Language, Texts, ThemeMode};
use crate::gui::settings::GuiSettings;
use arboard::GetExtLinux;
use eframe::egui;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Eingebettetes Creator-Logo (PNG 600x160). Die Binary bleibt dadurch
/// vollständig selbstständig und benötigt keine externe Bilddatei.
const CREATOR_LOGO: &[u8] = include_bytes!("../../assets/ui/creator_logo.png");
/// Eingebettetes Programm-Icon (256x256, transparenter Rand).
const APP_ICON: &[u8] = include_bytes!("../../assets/app_icon.png");
/// Eingebettetes "Made for Linux"-Badge für die Statusleiste.
const LINUX_BADGE: &[u8] = include_bytes!("../../assets/ui/made_for_linux.png");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EngineMode {
    Auto,
    Podman,
    Docker,
}

impl EngineMode {
    const ALL: [EngineMode; 3] = [EngineMode::Auto, EngineMode::Podman, EngineMode::Docker];

    fn label(self) -> &'static str {
        match self {
            EngineMode::Auto => "Auto",
            EngineMode::Podman => "Podman",
            EngineMode::Docker => "Docker",
        }
    }

    /// `Auto` bevorzugt Podman, falls beide Engines installiert sind.
    fn resolve(self) -> ContainerEngine {
        match self {
            EngineMode::Auto => detect_engine(),
            EngineMode::Podman => ContainerEngine::Podman,
            EngineMode::Docker => ContainerEngine::Docker,
        }
    }
}

pub struct Git2PkgApp {
    url: String,
    target: TargetFormat,
    target_distro: TargetDistro,
    output_dir: PathBuf,
    output_dir_text: String,
    language: Language,
    theme_mode: ThemeMode,
    theme_applied: bool,
    last_theme_check: Instant,
    engine_mode: EngineMode,
    engine: ContainerEngine,
    engine_available: bool,
    distro: DistroInfo,
    log: String,
    log_snapshot: String,
    log_hovered: bool,
    step: String,
    build_progress: f32,
    cargo_crates: u32,
    building: bool,
    cancel_flag: Arc<AtomicBool>,
    container_id: Arc<Mutex<Option<String>>>,
    receiver: Option<Receiver<BuildEvent>>,
    install_receiver: Option<Receiver<String>>,
    outcome: Option<BuildOutcome>,
    error: Option<String>,
    last_failed_url: Option<String>,
    next_strategy: BuildStrategy,
    confirm_install: bool,
    hint: Option<String>,
    show_info: bool,
    install_dialog: Option<PathBuf>,
    retry_dialog: bool,
    cross_build_dialog: bool,
    last_build_failed: bool,
    had_build_in_session: bool,
    show_cleanup_dialog: bool,
    force_close: bool,
    logo: Option<egui::TextureHandle>,
    app_icon: Option<egui::TextureHandle>,
    linux_badge: Option<egui::TextureHandle>,
    clipboard: Option<arboard::Clipboard>,
}

fn is_foreign_target(target: TargetDistro, host: &DistroFamily) -> bool {
    target != TargetDistro::Auto && target.family(host) != *host
}

/// Read-only buffer for the live log: selectable and copyable, never editable.
struct ReadOnlyText<'a>(&'a str);

impl egui::TextBuffer for ReadOnlyText<'_> {
    fn is_mutable(&self) -> bool {
        false
    }

    fn as_str(&self) -> &str {
        self.0
    }

    fn insert_text(&mut self, _text: &str, _char_index: usize) -> usize {
        0
    }

    fn delete_char_range(&mut self, _char_range: std::ops::Range<usize>) {}

    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<ReadOnlyText<'static>>()
    }
}

impl Git2PkgApp {
    pub fn new() -> Self {
        // Wissensdatei beim Programmstart anlegen/befüllen.
        let _ = MappingStore::load();
        let language = Language::detect();
        let settings = GuiSettings::load();
        let output_dir = settings
            .output_dir
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(default_output_dir);
        let theme_mode = settings.theme.unwrap_or(ThemeMode::Auto);
        Self {
            url: String::new(),
            target: TargetFormat::NativePackage,
            target_distro: TargetDistro::Auto,
            output_dir: output_dir.clone(),
            output_dir_text: output_dir.to_string_lossy().into_owned(),
            language,
            theme_mode,
            theme_applied: false,
            last_theme_check: Instant::now(),
            engine_mode: EngineMode::Auto,
            engine: detect_engine(),
            engine_available: container_engine_available(),
            distro: detect_distro(),
            log: String::new(),
            log_snapshot: String::new(),
            log_hovered: false,
            step: String::new(),
            build_progress: 0.0,
            cargo_crates: 0,
            building: false,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            container_id: Arc::new(Mutex::new(None)),
            receiver: None,
            install_receiver: None,
            outcome: None,
            error: None,
            last_failed_url: None,
            next_strategy: BuildStrategy::Default,
            confirm_install: false,
            hint: None,
            show_info: false,
            install_dialog: None,
            retry_dialog: false,
            cross_build_dialog: false,
            last_build_failed: false,
            had_build_in_session: false,
            show_cleanup_dialog: false,
            force_close: false,
            logo: None,
            app_icon: None,
            linux_badge: None,
            clipboard: arboard::Clipboard::new().ok(),
        }
    }

    fn texts(&self) -> Texts {
        Texts::new(self.language)
    }

    /// Dekodiert das eingebettete PNG einmalig über die `image`-Crate in eine
    /// egui-Textur. So funktioniert das Logo auch ohne externe Bilddatei.
    fn ensure_logo(&mut self, ctx: &egui::Context) {
        if self.logo.is_some() {
            return;
        }
        let Ok(decoded) = image::load_from_memory(CREATOR_LOGO) else {
            return;
        };
        let rgba = decoded.to_rgba8();
        let (width, height) = rgba.dimensions();
        let pixels = egui::ColorImage::from_rgba_unmultiplied(
            [width as usize, height as usize],
            rgba.as_raw(),
        );
        self.logo = Some(ctx.load_texture("creator_logo", pixels, egui::TextureOptions::LINEAR));
    }

    /// Lädt das eingebettete Programm-Icon für den Info-Dialog.
    fn ensure_app_icon(&mut self, ctx: &egui::Context) {
        if self.app_icon.is_some() {
            return;
        }
        let Ok(decoded) = image::load_from_memory(APP_ICON) else {
            return;
        };
        let rgba = decoded.to_rgba8();
        let (width, height) = rgba.dimensions();
        let pixels = egui::ColorImage::from_rgba_unmultiplied(
            [width as usize, height as usize],
            rgba.as_raw(),
        );
        self.app_icon =
            Some(ctx.load_texture("git2pkg_app_icon", pixels, egui::TextureOptions::LINEAR));
    }

    /// Lädt das eingebettete "Made for Linux"-Badge für die Statusleiste.
    fn ensure_linux_badge(&mut self, ctx: &egui::Context) {
        if self.linux_badge.is_some() {
            return;
        }
        let Ok(decoded) = image::load_from_memory(LINUX_BADGE) else {
            return;
        };
        let rgba = decoded.to_rgba8();
        let (width, height) = rgba.dimensions();
        let pixels = egui::ColorImage::from_rgba_unmultiplied(
            [width as usize, height as usize],
            rgba.as_raw(),
        );
        self.linux_badge =
            Some(ctx.load_texture("made_for_linux_badge", pixels, egui::TextureOptions::LINEAR));
    }

    fn apply_theme(&self, ctx: &egui::Context) {
        if self.theme_mode.is_dark() {
            ctx.set_visuals(egui::Visuals::dark());
        } else {
            ctx.set_visuals(egui::Visuals::light());
        }
    }

    /// Dunkelgrün im Light-Mode, helles Grün im Dark-Mode.
    fn value_color(&self) -> egui::Color32 {
        if self.theme_mode.is_dark() {
            egui::Color32::from_rgb(80, 210, 100)
        } else {
            egui::Color32::from_rgb(0, 140, 0)
        }
    }

    fn language_changed(&mut self) {
        self.error = None;
        self.hint = None;
        if !self.building {
            self.step = self.texts().ready().to_string();
        }
    }

    fn persist_settings(&self) {
        GuiSettings {
            output_dir: Some(self.output_dir.clone()),
            theme: Some(self.theme_mode),
        }
        .save();
    }

    /// Übernimmt einen manuell eingegebenen Pfad als tatsächliches Ziel.
    fn sync_output_dir(&mut self) {
        let text = self.output_dir_text.trim();
        if !text.is_empty() {
            let selected = PathBuf::from(text);
            if selected != self.output_dir {
                self.output_dir = selected;
                self.persist_settings();
            }
        }
    }

    fn start_build(&mut self) {
        let url = self.url.trim().to_string();
        self.start_build_with(url, BuildStrategy::Default);
    }

    /// Menschlich lesbarer Name der nächsten Strategie in der aktiven Sprache.
    fn next_strategy_label(&self) -> &'static str {
        let texts = self.texts();
        match self.next_strategy {
            BuildStrategy::Default => texts.strategy_default(),
            BuildStrategy::SystemLibsOnly => texts.strategy_system_libs(),
            BuildStrategy::RepoScript => texts.strategy_repo_script(),
            BuildStrategy::DirectStagingFallback => texts.strategy_direct(),
        }
    }

    /// Neuer Anlauf mit dem vorhandenen Quellcode und der nächsten Strategie.
    fn start_retry(&mut self) {
        let Some(url) = self.last_failed_url.clone() else {
            return;
        };
        let strategy = self.next_strategy;
        self.start_build_with(url, strategy);
    }

    fn start_build_with(&mut self, url: String, strategy: BuildStrategy) {
        if self.building || !self.engine_available {
            return;
        }
        let texts = self.texts();
        self.sync_output_dir();
        if let Err(error) = fs::create_dir_all(&self.output_dir) {
            self.error = Some(texts.output_dir_error().replace("{}", &error.to_string()));
            return;
        }
        if url.is_empty() {
            self.error = Some(texts.error_empty_url().to_string());
            return;
        }
        if !url.starts_with("https://") && !url.starts_with("http://") {
            self.error = Some(texts.error_bad_scheme().to_string());
            return;
        }

        self.url = url.clone();
        self.last_failed_url = Some(url.clone());
        let has_deps = cached_repo_has_local_dependencies(&url);
        self.next_strategy = strategy.next(has_deps);
        self.building = true;
        self.had_build_in_session = true;
        self.build_progress = 0.0;
        self.cargo_crates = 0;
        self.cancel_flag.store(false, Ordering::Relaxed);
        *self.container_id.lock().unwrap() = None;
        self.outcome = None;
        self.error = None;
        self.hint = None;
        self.confirm_install = false;
        self.log.clear();
        self.step = texts.starting().to_string();
        self.log.push_str(&format!(
            "$ git2pkg build {} ({})\n",
            url,
            self.target.key()
        ));

        let (tx, rx) = mpsc::channel();
        let config = BuildConfig {
            url,
            target: self.target.clone(),
            target_distro: self.target_distro,
            distro: self.distro.clone(),
            engine: self.engine.clone(),
            output_dir: self.output_dir.clone(),
            language: self.language,
            strategy,
            repo_script: None,
            cmake_compat: false,
            cstdint_fix: false,
            submodule_fix: false,
            skip_deps: false,
            cancel_flag: self.cancel_flag.clone(),
            container_id: self.container_id.clone(),
        };
        std::thread::spawn(move || run_build(config, tx));
        self.receiver = Some(rx);
    }

    /// Benutzerabbruch: Flag setzen und die Oberfläche sofort freigeben.
    fn cancel_build(&mut self) {
        if !self.building {
            return;
        }
        self.cancel_flag.store(true, Ordering::Relaxed);
        self.building = false;
        self.build_progress = 0.0;
        self.retry_dialog = false;
        self.install_dialog = None;
        self.log
            .push_str("[warn] Build vom Benutzer abgebrochen.\n");
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let mut changed = false;
        let mut clear_receiver = false;
        let mut clear_install_receiver = false;

        if let Some(rx) = &self.receiver {
            loop {
                match rx.try_recv() {
                    Ok(BuildEvent::Step(text)) => {
                        self.step = text;
                        changed = true;
                    }
                    Ok(BuildEvent::ContainerStarted(id)) => {
                        *self.container_id.lock().unwrap() = Some(id);
                        changed = true;
                    }
                    Ok(BuildEvent::Cancelled) => {
                        self.building = false;
                        self.last_build_failed = true;
                        self.retry_dialog = false;
                        self.install_dialog = None;
                        self.step = self.texts().failed_step().to_string();
                        self.build_progress = 0.0;
                        *self.container_id.lock().unwrap() = None;
                        clear_receiver = true;
                        changed = true;
                    }
                    Ok(BuildEvent::Log(text)) => {
                        self.log.push_str(&text);
                        self.log.push('\n');
                        if text.trim_start().starts_with("Compiling ") {
                            self.cargo_crates += 1;
                        }
                        self.build_progress =
                            progress_from_line(self.build_progress, &text, self.cargo_crates);
                        changed = true;
                    }
                    Ok(BuildEvent::Finished(outcome)) => {
                        self.build_progress = 1.0;
                        self.step = self.texts().success_step().to_string();
                        self.last_build_failed = false;
                        self.retry_dialog = false;
                        self.install_dialog = outcome
                            .artifacts
                            .iter()
                            .find(|path| is_system_package(path))
                            .cloned();
                        self.outcome = Some(outcome);
                        self.building = false;
                        clear_receiver = true;
                        changed = true;
                    }
                    Ok(BuildEvent::Failed(error)) => {
                        // Auch nach gescheitertem Direct-Fallback immer das Retry-Popup anzeigen.
                        self.retry_dialog = true;
                        self.step = self.texts().failed_step().to_string();
                        self.error = Some(error);
                        self.last_build_failed = true;
                        self.retry_dialog = true;
                        self.install_dialog = None;
                        self.building = false;
                        clear_receiver = true;
                        changed = true;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        if self.building {
                            self.building = false;
                            self.error = Some(self.texts().thread_error().to_string());
                        }
                        clear_receiver = true;
                        changed = true;
                        break;
                    }
                }
            }
        }

        if let Some(rx) = &self.install_receiver {
            loop {
                match rx.try_recv() {
                    Ok(text) => {
                        self.log.push_str(&text);
                        self.log.push('\n');
                        changed = true;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        clear_install_receiver = true;
                        changed = true;
                        break;
                    }
                }
            }
        }

        if clear_receiver {
            self.receiver = None;
        }
        if clear_install_receiver {
            self.install_receiver = None;
        }
        if changed {
            ctx.request_repaint();
        }
    }

    fn run_confirmed_install(&mut self, command: String) {
        self.log.push_str(&format!("[install] $ {}\n", command));
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(
            move || match Command::new("sh").arg("-c").arg(&command).output() {
                Ok(output) => {
                    let mut text = String::new();
                    text.push_str(&String::from_utf8_lossy(&output.stdout));
                    text.push_str(&String::from_utf8_lossy(&output.stderr));
                    if !output.status.success() {
                        text.push_str(&format!("[install] Exit: {}\n", output.status));
                    }
                    let _ = tx.send(text);
                }
                Err(error) => {
                    let _ = tx.send(format!("[install] {}", error));
                }
            },
        );
        self.install_receiver = Some(rx);
        self.confirm_install = false;
    }
}

impl Default for Git2PkgApp {
    fn default() -> Self {
        Self::new()
    }
}

impl Git2PkgApp {
    /// Distributionsspezifischer Installationsbefehl hinter `pkexec`.
    fn install_args(&self, path: &Path) -> Vec<String> {
        let package = path.to_string_lossy().into_owned();
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut args = vec!["pkexec".to_string()];
        if name.ends_with(".pkg.tar.zst") {
            args.extend([
                "pacman".to_string(),
                "-U".to_string(),
                "--noconfirm".to_string(),
                package,
            ]);
        } else if name.ends_with(".deb") {
            args.extend([
                "apt-get".to_string(),
                "install".to_string(),
                "-y".to_string(),
                package,
            ]);
        } else if self.distro.family == DistroFamily::Suse {
            args.extend([
                "zypper".to_string(),
                "--non-interactive".to_string(),
                "install".to_string(),
                "--allow-unsigned-rpm".to_string(),
                package,
            ]);
        } else {
            args.extend([
                "dnf".to_string(),
                "install".to_string(),
                "-y".to_string(),
                package,
            ]);
        }
        args
    }

    /// Fuehrt die Installation aus und schreibt das Ergebnis ins Live-Log.
    fn run_install(&mut self, path: &Path) {
        let texts = self.texts();
        let running = texts.install_running().to_string();
        let ok = texts.install_ok().to_string();
        let failed = texts.install_failed().to_string();
        let args = self.install_args(path);
        self.log.push_str(&format!("[install] {}\n", running));
        match Command::new(&args[0]).args(&args[1..]).output() {
            Ok(output) => {
                for line in String::from_utf8_lossy(&output.stdout).lines() {
                    self.log.push_str(&format!("[out] {}\n", line));
                }
                for line in String::from_utf8_lossy(&output.stderr).lines() {
                    self.log.push_str(&format!("[err] {}\n", line));
                }
                if output.status.success() {
                    self.log.push_str(&format!("[install] {}\n", ok));
                } else {
                    self.log.push_str(&format!(
                        "[install] {} {} {}\n",
                        failed,
                        output.status,
                        path.display()
                    ));
                }
            }
            Err(error) => {
                self.log.push_str(&format!(
                    "[install] {} {} {}\n",
                    failed,
                    error,
                    path.display()
                ));
            }
        }
        self.install_dialog = None;
    }
}

/// Nur echte Systempakete loesen den Installationsdialog aus.
fn is_system_package(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    name.ends_with(".pkg.tar.zst") || name.ends_with(".deb") || name.ends_with(".rpm")
}

impl eframe::App for Git2PkgApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.building {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if ctx.input(|i| i.viewport().close_requested())
            && !self.force_close
            && self.had_build_in_session
            && self.last_build_failed
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.show_cleanup_dialog = true;
        }
        if !self.theme_applied
            || (self.theme_mode == ThemeMode::Auto
                && self.last_theme_check.elapsed() >= Duration::from_secs(3))
        {
            self.apply_theme(ctx);
            self.theme_applied = true;
            self.last_theme_check = Instant::now();
        }

        self.ensure_logo(ctx);
        self.ensure_app_icon(ctx);
        self.ensure_linux_badge(ctx);
        self.poll(ctx);
        let texts = self.texts();

        // Werkzeugleiste: oben rechts Design, Sprache und Container-Engine.
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.strong("git2pkg");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Button::new(texts.info_button())
                                .corner_radius(egui::CornerRadius::same(8))
                                .min_size(egui::vec2(46.0, 20.0))
                                .selected(self.show_info),
                        )
                        .on_hover_text(texts.info_button())
                        .clicked()
                    {
                        self.show_info = !self.show_info;
                    }

                    let theme_text = format!(
                        "{} {}",
                        texts.status_theme(),
                        texts.theme_label(self.theme_mode)
                    );
                    if ui.button(theme_text).clicked() {
                        self.theme_mode = self.theme_mode.next();
                        self.apply_theme(ctx);
                        self.persist_settings();
                    }

                    let lang_text =
                        format!("{} {}", texts.status_language(), self.language.label());
                    egui::ComboBox::from_id_salt("language")
                        .selected_text(lang_text)
                        .show_ui(ui, |ui| {
                            for option in Language::ALL {
                                if ui
                                    .selectable_value(&mut self.language, option, option.label())
                                    .clicked()
                                {
                                    self.language_changed();
                                }
                            }
                        });

                    let engine_text =
                        format!("{} {}", texts.status_engine(), self.engine_mode.label());
                    egui::ComboBox::from_id_salt("engine")
                        .selected_text(engine_text)
                        .show_ui(ui, |ui| {
                            for option in EngineMode::ALL {
                                if ui
                                    .selectable_value(&mut self.engine_mode, option, option.label())
                                    .clicked()
                                {
                                    self.engine = self.engine_mode.resolve();
                                }
                            }
                        });
                });
            });
        });

        // Statusleiste: fest zwei Zeilen - Systeminfos oben, Fortschritt mittig darunter.
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                let green = self.value_color();
                ui.strong(texts.status_host());
                ui.colored_label(green, self.distro.name.as_str());
                ui.separator();
                ui.strong(texts.status_id());
                ui.colored_label(green, self.distro.id.as_str());
                ui.separator();
                ui.strong(texts.status_image());
                ui.colored_label(green, self.distro.base_image.as_str());
                ui.separator();
                ui.strong(texts.status_search());
                ui.colored_label(green, self.distro.search_command.as_str());
                ui.separator();
                ui.strong(texts.status_engine());
                ui.colored_label(green, self.engine.label());
                ui.separator();
                ui.strong(texts.status_step());
                ui.colored_label(green, self.step.as_str());
            });
            if self.building {
                ui.add_space(3.0);
                ui.vertical_centered(|ui| {
                    let percent = (self.build_progress * 100.0).round() as u32;
                    let label = format!("{} % — {}", percent, self.step);
                    let width = 420.0_f32.min(ui.available_width());
                    ui.add(
                        egui::ProgressBar::new(self.build_progress)
                            .text(label)
                            .desired_width(width),
                    );
                });
            }
        });

        // Über / Info-Dialog, zentriert und in allen fünf Sprachen.
        let mut about_open = self.show_info;
        let mut close_clicked = false;
        egui::Window::new(texts.info_title())
            .open(&mut about_open)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                if let Some(texture) = &self.app_icon {
                    ui.vertical_centered(|ui| {
                        ui.add(
                            egui::Image::from_texture(texture)
                                .max_size(egui::vec2(64.0, 64.0))
                                .maintain_aspect_ratio(true),
                        );
                    });
                    ui.add_space(6.0);
                }
                if let Some(texture) = &self.logo {
                    ui.vertical_centered(|ui| {
                        ui.add(
                            egui::Image::from_texture(texture)
                                .max_size(egui::vec2(190.0, 60.0))
                                .maintain_aspect_ratio(true),
                        );
                    });
                    ui.add_space(6.0);
                }
                ui.heading(format!("git2pkg v{}", env!("CARGO_PKG_VERSION")));
                ui.add_space(4.0);
                ui.label(texts.about_description());
                ui.add_space(6.0);
                ui.label("License: GNU GPLv3");
                ui.label(format!(
                    "{} {}",
                    texts.learned_rules(),
                    MappingStore::load().learned_rule_count()
                ));
                ui.add_space(6.0);
                if ui.link(texts.link_repository()).clicked() {
                    let _ = open::that(env!("CARGO_PKG_REPOSITORY"));
                }
                if ui.link(texts.link_support()).clicked() {
                    let _ = open::that("https://ko-fi.com");
                }
                ui.add_space(8.0);
                if ui.button(texts.close()).clicked() {
                    close_clicked = true;
                }
            });
        if close_clicked {
            about_open = false;
        }
        self.show_info = about_open;

        // Automatischer Installationsdialog nach einem erfolgreichen Paketbau.
        if let Some(package) = self.install_dialog.clone() {
            let title = self.texts().install_success_title().to_string();
            let body = self.texts().install_success_body().to_string();
            let question = self.texts().install_question().to_string();
            let now = self.texts().install_now().to_string();
            let later = self.texts().install_later().to_string();
            let target_family = self.target_distro.family(&self.distro.family);
            let is_native = target_family == self.distro.family;
            let target_name = match target_family {
                DistroFamily::Arch => "Arch Linux / CachyOS",
                DistroFamily::Debian => "Debian / Ubuntu / Linux Mint",
                DistroFamily::Fedora => "Fedora / RHEL",
                DistroFamily::Suse => "openSUSE",
                DistroFamily::Unknown => "Linux",
            };
            let cross_hint = self.texts().cross_build_hint(target_name);
            let open_folder = self.texts().open_output_folder().to_string();
            let close_only = self.texts().close_only().to_string();
            let mut close_clicked = false;
            let mut install_clicked = false;
            let mut open_clicked = false;
            egui::Window::new(&title)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .resizable(false)
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label(&body);
                    ui.monospace(package.to_string_lossy());
                    ui.add_space(6.0);
                    if is_native {
                        ui.label(&question);
                    } else {
                        ui.label(&cross_hint);
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if is_native {
                            if ui.button(&now).clicked() {
                                install_clicked = true;
                            }
                            if ui.button(&later).clicked() {
                                close_clicked = true;
                            }
                        } else {
                            if ui.button(&open_folder).clicked() {
                                open_clicked = true;
                            }
                            if ui.button(&close_only).clicked() {
                                close_clicked = true;
                            }
                        }
                    });
                });
            if install_clicked {
                self.run_install(&package);
            } else if open_clicked {
                if let Err(error) = Command::new("xdg-open").arg(&self.output_dir).spawn() {
                    self.error = Some(
                        self.texts()
                            .open_folder_error()
                            .replace("{}", &error.to_string()),
                    );
                }
            } else if close_clicked {
                self.install_dialog = None;
            }
        }

        // Hinweis bei aktiv ausgewähltem Cross-Build-Ziel, zentriert und gut lesbar.
        if self.cross_build_dialog {
            let title = self.texts().target_format().to_string();
            let body = self.texts().cross_build_warning().to_string();
            let ok = self.texts().ok_understood().to_string();
            let mut open = true;
            let mut close = false;
            egui::Window::new(&title)
                .open(&mut open)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .resizable(false)
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.set_max_width(440.0);
                    ui.label(&body);
                    ui.add_space(8.0);
                    if ui.button(&ok).clicked() {
                        close = true;
                    }
                });
            if !open || close {
                self.cross_build_dialog = false;
            }
        }

        // Automatisches Retry-Popup, wenn der Auto-Resolver nichts mehr fand.
        if self.retry_dialog {
            let title = self.texts().retry_failed_title().to_string();
            let reason_label = self.texts().retry_failed_reason().to_string();
            let reason = self.error.clone().unwrap_or_else(|| {
                self.log
                    .lines()
                    .rev()
                    .find(|s| !s.trim().is_empty())
                    .unwrap_or("")
                    .to_string()
            });
            let question = self.texts().retry_failed_question().to_string();
            let yes = self.texts().retry_failed_yes().to_string();
            let no = self.texts().retry_failed_cancel().to_string();
            let mut retry = false;
            let mut cancel = false;
            egui::Window::new(&title)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .resizable(false)
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label(&reason_label);
                    ui.monospace(&reason);
                    ui.add_space(6.0);
                    let next = self.texts().next_strategy(self.next_strategy_label());
                    ui.strong(&next);
                    ui.add_space(6.0);
                    ui.label(&question);
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(&yes).clicked() {
                            retry = true;
                        }
                        if ui.button(&no).clicked() {
                            cancel = true;
                        }
                    });
                });
            if retry || cancel {
                self.retry_dialog = false;
                if retry {
                    self.start_retry();
                }
            }
        }

        // Aufraeum-Abfrage beim Beenden nach fehlgeschlagenem Build.
        if self.show_cleanup_dialog {
            let title = self.texts().cleanup_title().to_string();
            let body = self.texts().cleanup_body().to_string();
            let all = self.texts().cleanup_delete_all().to_string();
            let artifacts = self.texts().cleanup_delete_artifacts().to_string();
            let nothing = self.texts().cleanup_delete_nothing().to_string();
            // 1 = alles, 2 = nur Build-Reste, 3 = nichts.
            let mut choice: u8 = 0;
            egui::Window::new(&title)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .resizable(false)
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label(&body);
                    ui.add_space(8.0);
                    if ui.button(&all).clicked() {
                        choice = 1;
                    }
                    if ui.button(&artifacts).clicked() {
                        choice = 2;
                    }
                    if ui.button(&nothing).clicked() {
                        choice = 3;
                    }
                });
            if choice > 0 {
                match choice {
                    1 => {
                        cleanup_failed_builds();
                        self.last_build_failed = false;
                    }
                    2 => {
                        cleanup_failed_build_artifacts();
                        self.last_build_failed = false;
                    }
                    _ => {}
                }
                self.show_cleanup_dialog = false;
                self.force_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }

        // Ergebnis-Aktionen in einem eigenen unteren Panel: wird nie vom
        // Log-Fenster verdeckt oder am Fensterrand abgeschnitten.
        if self.outcome.is_some() {
            let artifacts: Vec<String> = self
                .outcome
                .as_ref()
                .map(|outcome| {
                    outcome
                        .artifacts
                        .iter()
                        .map(|path| path.to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            let recipe = self
                .outcome
                .as_ref()
                .map(|outcome| outcome.recipe.clone())
                .unwrap_or_default();
            let install_command = self
                .outcome
                .as_ref()
                .map(|outcome| outcome.install_command.clone())
                .unwrap_or_default();

            egui::TopBottomPanel::bottom("result").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(texts.result());
                    for artifact in &artifacts {
                        ui.monospace(artifact);
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.button(texts.open_dir()).clicked() {
                        self.sync_output_dir();
                        if let Err(error) = Command::new("xdg-open").arg(&self.output_dir).spawn() {
                            self.error =
                                Some(texts.open_folder_error().replace("{}", &error.to_string()));
                        }
                    }
                    if ui.button(texts.save_recipe()).clicked() {
                        ctx.copy_text(recipe.clone());
                        self.hint = Some(texts.recipe_hint().to_string());
                    }
                    if ui.button(texts.install_package()).clicked() {
                        self.confirm_install = true;
                    }
                });

                if self.confirm_install {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(texts.confirm_heading());
                        ui.monospace(&install_command);
                        if ui.button(texts.confirm_run()).clicked() {
                            self.run_confirmed_install(install_command.clone());
                        }
                        if ui.button(texts.cancel()).clicked() {
                            self.confirm_install = false;
                        }
                    });
                }
            });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                if let Some(texture) = &self.app_icon {
                    ui.add(
                        egui::Image::from_texture(texture)
                            .max_size(egui::vec2(38.0, 38.0))
                            .maintain_aspect_ratio(true),
                    );
                }
                ui.vertical(|ui| {
                    ui.heading("git2pkg");
                    ui.label(texts.subtitle());
                });
            });
            ui.separator();

            // Eingabeblock zentriert, Badge rechts daneben - ohne den Block zu verengen.
            ui.horizontal(|ui| {
                let badge_area = 270.0_f32;
                let input_width = (ui.available_width() - 32.0 - badge_area).min(820.0);
                let left_space = ((ui.available_width() - input_width - badge_area) / 2.0).max(0.0);
                ui.add_space(left_space);
                ui.allocate_ui_with_layout(
                    egui::vec2(input_width, ui.available_height()),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        ui.group(|ui| {
                            let label_area = 130.0_f32;
                            let button_area = 130.0_f32;
                            let field_width =
                                (input_width - label_area - button_area - 16.0).max(120.0);

                            // Zeile 1: Repository-URL + Build-Button rechts bündig.
                            ui.horizontal_wrapped(|ui| {
                                ui.allocate_ui_with_layout(
                                    egui::vec2(label_area, ui.available_height()),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        ui.label(texts.repo_url());
                                    },
                                );
                                let pasted_now = ui.input(|input| {
                                    input
                                        .events
                                        .iter()
                                        .any(|event| matches!(event, egui::Event::Paste(_)))
                                });
                                let clear_width = 24.0_f32;
                                let url_field_width = (field_width - clear_width - 4.0).max(100.0);
                                let mut response_slot = None;
                                let mut clear_clicked = false;
                                ui.allocate_ui_with_layout(
                                    egui::vec2(field_width, ui.available_height()),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        response_slot = Some(
                                            ui.add(
                                                egui::TextEdit::singleline(&mut self.url)
                                                    .id(egui::Id::new("repo-url"))
                                                    .hint_text(texts.repo_hint())
                                                    .desired_width(url_field_width),
                                            ),
                                        );
                                        clear_clicked = ui
                                            .add_enabled(
                                                !self.building && !self.url.is_empty(),
                                                egui::Button::new(
                                                    egui::RichText::new("×").size(15.0),
                                                ),
                                            )
                                            .on_hover_text(texts.clear_url())
                                            .clicked();
                                    },
                                );
                                let url_response =
                                    response_slot.expect("URL-Textfeld wurde nicht erzeugt");
                                if clear_clicked {
                                    self.url.clear();
                                    url_response.request_focus();
                                }
                                // Linux: Mittelklick fügt die Primary-Selection ein.
                                if !self.building
                                    && self.engine_available
                                    && url_response.lost_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                                {
                                    self.start_build();
                                }
                                let middle_pasted = url_response.middle_clicked();
                                if middle_pasted {
                                    if let Some(clipboard) = self.clipboard.as_mut() {
                                        if let Ok(text) = clipboard
                                            .get()
                                            .clipboard(arboard::LinuxClipboardKind::Primary)
                                            .text()
                                        {
                                            self.url.push_str(&text);
                                        }
                                    }
                                }
                                // Nach Paste den Cursor sauber ans Ende der URL setzen.
                                if pasted_now || middle_pasted {
                                    let id = egui::Id::new("repo-url");
                                    if let Some(mut state) =
                                        egui::TextEdit::load_state(ui.ctx(), id)
                                    {
                                        let end =
                                            egui::text::CCursor::new(self.url.chars().count());
                                        state.cursor.set_char_range(Some(
                                            egui::text::CCursorRange::one(end),
                                        ));
                                        egui::TextEdit::store_state(ui.ctx(), id, state);
                                    }
                                }
                                ui.allocate_ui_with_layout(
                                    egui::vec2(button_area, ui.available_height()),
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if self.building {
                                            let cancel = egui::Button::new(
                                                egui::RichText::new(texts.cancel_build())
                                                    .color(egui::Color32::from_rgb(220, 60, 60)),
                                            );
                                            if ui.add(cancel).clicked() {
                                                self.cancel_build();
                                            }
                                        } else if ui
                                            .add_enabled(
                                                self.engine_available,
                                                egui::Button::new(texts.build_start()),
                                            )
                                            .clicked()
                                        {
                                            self.start_build();
                                        }
                                    },
                                );
                            });

                            // Zeile 2: schlaues Zielformat-Dropdown (ohne Standalone-Binary).
                            ui.horizontal_wrapped(|ui| {
                                ui.allocate_ui_with_layout(
                                    egui::vec2(label_area, ui.available_height()),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        ui.label(texts.target_format());
                                    },
                                );
                                let extension = match self.target_distro.family(&self.distro.family)
                                {
                                    DistroFamily::Arch => ".pkg.tar.zst",
                                    DistroFamily::Debian => ".deb",
                                    DistroFamily::Fedora | DistroFamily::Suse => ".rpm",
                                    DistroFamily::Unknown => ".pkg.tar.zst",
                                };
                                let auto_label = texts.target_auto(extension, &self.distro.name);
                                let selected = match self.target_distro {
                                    TargetDistro::Auto => auto_label.clone(),
                                    TargetDistro::Arch => texts.target_arch().to_string(),
                                    TargetDistro::Debian => texts.target_debian().to_string(),
                                    TargetDistro::Fedora => texts.target_fedora().to_string(),
                                    TargetDistro::Suse => texts.target_suse().to_string(),
                                };
                                let combo_width = (input_width - label_area - 16.0).max(180.0);
                                let previous_target = self.target_distro;
                                egui::ComboBox::from_id_salt("target-distro")
                                    .selected_text(selected)
                                    .width(combo_width)
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut self.target_distro,
                                            TargetDistro::Auto,
                                            auto_label,
                                        );
                                        ui.selectable_value(
                                            &mut self.target_distro,
                                            TargetDistro::Arch,
                                            texts.target_arch(),
                                        );
                                        ui.selectable_value(
                                            &mut self.target_distro,
                                            TargetDistro::Debian,
                                            texts.target_debian(),
                                        );
                                        ui.selectable_value(
                                            &mut self.target_distro,
                                            TargetDistro::Fedora,
                                            texts.target_fedora(),
                                        );
                                        ui.selectable_value(
                                            &mut self.target_distro,
                                            TargetDistro::Suse,
                                            texts.target_suse(),
                                        );
                                    });
                                if self.target_distro != previous_target
                                    && is_foreign_target(self.target_distro, &self.distro.family)
                                {
                                    self.cross_build_dialog = true;
                                }
                            });

                            // Zeile 3: Ausgabeordner + Ändern-Button rechts bündig.
                            ui.horizontal_wrapped(|ui| {
                                ui.allocate_ui_with_layout(
                                    egui::vec2(label_area, ui.available_height()),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        ui.label(texts.output_folder());
                                    },
                                );
                                let folder_response = ui.add(
                                    egui::TextEdit::singleline(&mut self.output_dir_text)
                                        .desired_width(field_width),
                                );
                                if folder_response.middle_clicked() {
                                    if let Some(clipboard) = self.clipboard.as_mut() {
                                        if let Ok(text) = clipboard
                                            .get()
                                            .clipboard(arboard::LinuxClipboardKind::Primary)
                                            .text()
                                        {
                                            self.output_dir_text.push_str(&text);
                                            self.sync_output_dir();
                                        }
                                    }
                                }
                                ui.allocate_ui_with_layout(
                                    egui::vec2(button_area, ui.available_height()),
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.button(texts.browse()).clicked() {
                                            if let Some(folder) =
                                                rfd::FileDialog::new().pick_folder()
                                            {
                                                self.output_dir = folder.clone();
                                                self.output_dir_text =
                                                    folder.to_string_lossy().into_owned();
                                                self.persist_settings();
                                            }
                                        }
                                    },
                                );
                            });
                        });
                    },
                );
                ui.allocate_ui_with_layout(
                    egui::vec2(badge_area, ui.available_height()),
                    egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                    |ui| {
                        if let Some(texture) = &self.linux_badge {
                            ui.add(
                                egui::Image::from_texture(texture)
                                    .max_size(egui::vec2(260.0, 107.0))
                                    .maintain_aspect_ratio(true),
                            );
                        }
                    },
                );
            });

            if !self.engine_available {
                let install_command = match self.distro.family {
                    DistroFamily::Arch => "sudo pacman -S podman",
                    DistroFamily::Debian => "sudo apt install podman",
                    DistroFamily::Fedora => "sudo dnf install podman",
                    DistroFamily::Suse => "sudo zypper install podman",
                    DistroFamily::Unknown => "sudo pacman -S podman",
                };
                let fill = if self.theme_mode.is_dark() {
                    egui::Color32::from_rgb(70, 35, 30)
                } else {
                    egui::Color32::from_rgb(255, 235, 230)
                };
                egui::Frame::NONE
                    .fill(fill)
                    .stroke(egui::Stroke::new(
                        1.0_f32,
                        egui::Color32::from_rgb(200, 80, 60),
                    ))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.strong(texts.engine_missing_title());
                        ui.label(texts.engine_missing_text());
                        ui.horizontal(|ui| {
                            ui.monospace(install_command);
                            if ui.button(texts.copy()).clicked() {
                                ctx.copy_text(install_command.to_string());
                            }
                        });
                    });
            }

            let error_text = self.error.clone();
            if let Some(error) = &error_text {
                ui.colored_label(egui::Color32::RED, error);
            }
            if let Some(hint) = &self.hint {
                ui.colored_label(egui::Color32::GREEN, hint);
            }

            ui.separator();
            ui.horizontal(|ui| {
                ui.strong(texts.live_log());
                if ui.button(texts.copy_log()).clicked() {
                    ctx.copy_text(self.log.clone());
                }
            });

            // Das Log nutzt den verbleibenden Platz dynamisch; die unteren
            // Panels werden dadurch nie überdeckt oder abgeschnitten.
            if self.log_snapshot != self.log {
                self.log_snapshot = self.log.clone();
            }
            let dark = self.theme_mode.is_dark();
            let log_height = ui.available_height().max(120.0);
            let log_id = egui::Id::new("live-log-text");
            let selection_active = egui::TextEdit::load_state(ctx, log_id)
                .map(|state| !state.cursor.is_empty())
                .unwrap_or(false)
                || (self.log_hovered && ui.input(|input| input.pointer.primary_down()));
            let saved_selection = egui::TextEdit::load_state(ctx, log_id);
            let secondary_click = ui.input(|input| input.pointer.secondary_clicked());
            let mut log_hovered = false;
            egui::ScrollArea::both()
                .stick_to_bottom(self.building && !selection_active)
                .max_height(log_height)
                .show(ui, |ui| {
                    // Read-only TextEdit: volle Textauswahl mit Maus, Doppelklick
                    // und Strg+C/Kontextmenü, ohne dass der Inhalt änderbar ist.
                    let mut log_buffer = ReadOnlyText(&self.log_snapshot);
                    let response = ui.add(
                        egui::TextEdit::multiline(&mut log_buffer)
                            .id(log_id)
                            .frame(false)
                            .desired_width(f32::INFINITY)
                            .font(egui::FontId::monospace(13.0))
                            .text_color(log_line_color("", dark)),
                    );
                    log_hovered = response.hovered();
                    if secondary_click && log_hovered {
                        // Ein Secondary Click setzt bei egui sonst den Cursor
                        // zurück und löscht die laufende Markierung.
                        if let Some(saved) = saved_selection {
                            egui::TextEdit::store_state(ctx, log_id, saved);
                        }
                        let texts = self.texts();
                        let snapshot = self.log_snapshot.clone();
                        response.context_menu(|ui| {
                            if ui.button(texts.context_copy()).clicked() {
                                let selected = egui::TextEdit::load_state(ui.ctx(), log_id)
                                    .and_then(|state| state.cursor.char_range())
                                    .and_then(|range| {
                                        if range.is_empty() {
                                            return None;
                                        }
                                        let char_range = range.as_sorted_char_range();
                                        let chars: Vec<char> = snapshot.chars().collect();
                                        if char_range.end > chars.len() {
                                            return None;
                                        }
                                        Some(
                                            chars[char_range.start..char_range.end]
                                                .iter()
                                                .collect::<String>(),
                                        )
                                    })
                                    .filter(|text| !text.is_empty())
                                    .unwrap_or_else(|| snapshot.clone());
                                ui.ctx().copy_text(selected);
                            }
                            if ui.button(texts.context_select_all()).clicked() {
                                let mut state = egui::TextEdit::load_state(ui.ctx(), log_id)
                                    .unwrap_or_default();
                                let end = egui::text::CCursor::new(snapshot.chars().count());
                                state
                                    .cursor
                                    .set_char_range(Some(egui::text::CCursorRange::two(
                                        egui::text::CCursor::new(0),
                                        end,
                                    )));
                                egui::TextEdit::store_state(ui.ctx(), log_id, state);
                            }
                        });
                    }
                    if self.building {
                        let dim = if dark {
                            egui::Color32::from_gray(150)
                        } else {
                            egui::Color32::from_gray(110)
                        };
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.colored_label(dim, self.texts().build_running());
                        });
                    }
                });
            self.log_hovered = log_hovered;
            ui.add_space(2.0);
        });
    }
}

/// Fortschritt aus echten Log-Signalen ableiten, monoton und nie rückwärts.
fn progress_from_line(current: f32, line: &str, cargo_crates: u32) -> f32 {
    let lower = line.to_ascii_lowercase();
    let next = if lower.contains("clone")
        || lower.contains("receiving objects")
        || lower.contains("resolving deltas")
    {
        extract_percent(line)
            .map(|value| 0.15 * value)
            .unwrap_or_else(|| current.clamp(0.03, 0.15))
    } else if lower.contains("setup") || lower.contains("install") {
        0.15 + extract_percent(line)
            .map(|value| 0.15 * value)
            .unwrap_or(0.08)
    } else if lower.contains("starting package()")
        || lower.contains("dpkg-deb")
        || lower.contains("rpmbuild")
        || lower.contains("makepkg")
    {
        current.max(0.85)
    } else if lower.contains("verif") || lower.contains("prüf") {
        current.max(0.95)
    } else if let Some((done, total)) = extract_fraction(line) {
        0.30 + 0.55 * (done / total)
    } else if let Some(value) = extract_percent(line) {
        0.30 + 0.55 * value
    } else if lower.contains("finished") && lower.contains("target(s)") {
        0.85
    } else if lower.contains("compiling") {
        let crates = cargo_crates.max(1) as f32;
        0.30 + 0.52 * (1.0 - 1.0 / (1.0 + crates / 120.0))
    } else if lower.contains("[") {
        (current + 0.012).min(0.83)
    } else {
        current
    };
    next.max(current)
        .min(if lower.contains("setup") { 0.30 } else { 1.0 })
}

/// Findet die erste Prozentangabe, z. B. `45%`, `[ 28%]` oder `100 %`.
fn extract_percent(text: &str) -> Option<f32> {
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index].is_ascii_digit() {
            let start = index;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
            let mut probe = index;
            while probe < bytes.len() && bytes[probe].is_ascii_whitespace() {
                probe += 1;
            }
            if probe < bytes.len() && bytes[probe] == b'%' {
                let value: f32 = text[start..index].parse().ok()?;
                if (0.0..=100.0).contains(&value) {
                    return Some(value / 100.0);
                }
            }
        } else {
            index += 1;
        }
    }
    None
}

/// Findet Ninja-Fortschritt wie `[140/420]`.
fn extract_fraction(text: &str) -> Option<(f32, f32)> {
    let open = text.find('[')?;
    let rest = &text[open + 1..];
    let slash = rest.find('/')?;
    let close = rest.find(']')?;
    if close < slash {
        return None;
    }
    let done: f32 = rest[..slash].trim().parse().ok()?;
    let total: f32 = rest[slash + 1..close].trim().parse().ok()?;
    if total <= 0.0 || done > total {
        return None;
    }
    Some((done, total))
}

fn log_line_color(line: &str, dark: bool) -> egui::Color32 {
    let lower = line.to_ascii_lowercase();
    if line.starts_with("[err]")
        || lower.contains("error:")
        || lower.contains("cmake error")
        || lower.contains("fatal error:")
        || lower.contains("command not found")
        || lower.contains("exit code")
    {
        if dark {
            egui::Color32::from_rgb(255, 120, 120)
        } else {
            egui::Color32::from_rgb(190, 30, 30)
        }
    } else if line.starts_with("[warn]") {
        if dark {
            egui::Color32::from_rgb(255, 190, 80)
        } else {
            egui::Color32::from_rgb(170, 100, 0)
        }
    } else if line.starts_with("[learn]") {
        if dark {
            egui::Color32::from_rgb(80, 210, 100)
        } else {
            egui::Color32::from_rgb(0, 140, 0)
        }
    } else if dark {
        egui::Color32::from_gray(210)
    } else {
        egui::Color32::from_gray(35)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_readonly_buffer_ist_nicht_editierbar() {
        use egui::TextBuffer as _;
        let buffer = ReadOnlyText("zeile eins\nzeile zwei\n");
        assert!(!buffer.is_mutable());
        assert_eq!(buffer.as_str(), "zeile eins\nzeile zwei\n");
    }

    #[test]
    fn fortschritt_wird_aus_log_signalen_lesen() {
        assert_eq!(
            extract_percent("Receiving objects: 45% (123/456)"),
            Some(0.45)
        );
        assert_eq!(extract_percent("[ 28%] Building CXX object"), Some(0.28));
        assert_eq!(extract_fraction("[140/420] Linking"), Some((140.0, 420.0)));
        assert!(progress_from_line(0.2, "== git2pkg: Setup ==", 0) <= 0.30);
        assert!(progress_from_line(0.5, "Starting package() ...", 200) >= 0.85);
        assert!(progress_from_line(0.5, "[140/420] Linking", 200) >= 0.48);
        let p50 = progress_from_line(0.0, "   Compiling repo v0.1.0", 50);
        let p200 = progress_from_line(0.0, "   Compiling repo v0.1.0", 200);
        assert!((0.44..0.47).contains(&p50), "50 Crates: {p50}");
        assert!((0.61..0.64).contains(&p200), "200 Crates: {p200}");
        assert!(progress_from_line(p200, "Finished `release` target(s) in 1s", 200) >= 0.85);
    }

    #[test]
    fn log_fehlerzeilen_werden_rot_gefaerbt() {
        let red = log_line_color("[err] cargo: command not found", false);
        assert!(red.r() > 150 && red.g() < 80);
        let red2 = log_line_color("CMake Error at CMakeLists.txt:1", true);
        assert!(red2.r() > red2.g());
        let warn = log_line_color("[warn] Paket leer", false);
        assert!(warn.r() > warn.b());
        let learn = log_line_color("[learn] Zuordnung gespeichert", false);
        assert!(learn.g() > learn.r());
    }

    #[test]
    fn erkennt_nur_systempakete_fuer_install_dialog() {
        assert!(is_system_package(Path::new(
            "/tmp/repo-1.0-1-x86_64.pkg.tar.zst"
        )));
        assert!(is_system_package(Path::new("/tmp/repo_1.0_amd64.deb")));
        assert!(is_system_package(Path::new("/tmp/repo-1.0-1.x86_64.rpm")));
        assert!(!is_system_package(Path::new("/tmp/repo")));
        assert!(!is_system_package(Path::new("/tmp/BUILD_LOG.txt")));
    }

    #[test]
    fn neue_dialogtexte_sind_in_fuenf_sprachen_vorhanden() {
        let mut seen = std::collections::HashSet::new();
        for lang in Language::ALL {
            let t = Texts::new(lang);
            for text in [
                t.install_success_title(),
                t.install_question(),
                t.install_now(),
                t.cleanup_body(),
                t.cleanup_delete_all(),
                t.cleanup_delete_artifacts(),
                t.cleanup_delete_nothing(),
                t.retry_failed_title(),
                t.retry_failed_question(),
                t.retry_failed_yes(),
                t.retry_failed_cancel(),
                t.build_running(),
            ] {
                assert!(!text.is_empty());
                assert!(seen.insert(format!("{:?}:{}", lang, text)));
            }
        }
    }
}
