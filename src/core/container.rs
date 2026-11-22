//! Container-Steuerung und Build-Orchestrierung.
//! Alle Builds laufen in einem Wegwerf-Container (`podman run --rm`,
//! Fallback `docker run --rm`). Ausgaben werden über einen `mpsc::Sender`
//! live an die GUI weitergeleitet.

use crate::core::detector::{detect_build_system, has_local_deps, BuildSystem, ProjectFingerprint};
use crate::core::distro::{DistroFamily, DistroInfo, TargetDistro};
use crate::core::resolver::{
    analyze_output, candidate_names, cmake_alternative_flags, detect_build_fixes,
    detect_repo_script_distro_rejection, detect_source_group_error, family_key,
    has_eigen_version_mismatch, is_cmake_policy_error, is_compiled_build_system,
    package_from_search, package_lacks_executable, probe_commands, search_command,
    valid_package_name, BuildRecipe, MappingStore, MissingDep, SourceGroupError,
};
use crate::gui::i18n::{Language, Texts};
use regex::Regex;

use std::collections::HashSet;
use std::fs;
use std::io::BufRead;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{ChildStderr, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const OK_MARKER: &str = "__GIT2PKG_BUILD_OK__";
const DEPS_FAILURE_MARKER: &str = "[deps] Bau fehlgeschlagen";

fn deps_fallback_needed(output: &str, skip_deps: bool) -> bool {
    !skip_deps && output.contains(DEPS_FAILURE_MARKER)
}
const MAX_ATTEMPTS: u8 = 5;
const INITIAL_HEAL_ROUNDS: u8 = 5;
const MAX_HEAL_ROUNDS: u8 = 10;

/// Zustand eines Selbstheilungs-Durchlaufs. Nur wenn er sich aendert, ist ein
/// weiterer Retry derselben Strategie sinnvoll.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RetryState {
    packages: Vec<String>,
    cmake_flags: Vec<String>,
}

fn push_unique_string(list: &mut Vec<String>, value: String) {
    if !list.contains(&value) {
        list.push(value);
    }
}

/// Alle CMake-Flags, die ein Configure-Fehler fuer den naechsten Versuch nahelegt.
fn cmake_retry_flags(output: &str, current: &[String]) -> Vec<String> {
    let mut flags: Vec<String> = Vec::new();
    for flag in cmake_alternative_flags(output) {
        push_unique_string(&mut flags, flag);
    }
    if is_cmake_policy_error(output)
        && !current
            .iter()
            .chain(flags.iter())
            .any(|flag| flag.starts_with("-DCMAKE_POLICY_VERSION_MINIMUM="))
    {
        push_unique_string(&mut flags, "-DCMAKE_POLICY_VERSION_MINIMUM=3.5".to_string());
    }
    if has_eigen_version_mismatch(output) {
        for flag in [
            "-DEigen3_FIND_VERSION_MAJOR=3",
            "-DEigen3_FIND_VERSION_MINOR=3",
            "-DEIGEN3_VERSION_OK=TRUE",
        ] {
            push_unique_string(&mut flags, flag.to_string());
        }
    }
    flags.retain(|flag| !current.contains(flag));
    flags
}

fn retry_state_changed(before: &RetryState, after: &RetryState) -> bool {
    before != after
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetFormat {
    NativePackage,
    StandaloneBinary,
}

impl TargetFormat {
    pub fn from_key(key: &str) -> Self {
        match key.to_ascii_lowercase().as_str() {
            "binary" | "standalone" => Self::StandaloneBinary,
            _ => Self::NativePackage,
        }
    }

    pub fn key(&self) -> &'static str {
        match self {
            Self::NativePackage => "package",
            Self::StandaloneBinary => "binary",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerEngine {
    Podman,
    Docker,
}

impl ContainerEngine {
    pub fn binary(&self) -> &'static str {
        match self {
            ContainerEngine::Podman => "podman",
            ContainerEngine::Docker => "docker",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            ContainerEngine::Podman => "Podman",
            ContainerEngine::Docker => "Docker",
        }
    }
}

/// Ein einziger laufender Wegwerf-Container für den gesamten Build.
/// Suche, Nachinstallation und alle Build-Versuche laufen per `exec` darin,
/// damit Paketdatenbank und installierte Abhängigkeiten erhalten bleiben.
pub(crate) struct BuildContainer {
    engine: ContainerEngine,
    name: String,
}

impl BuildContainer {
    fn start(config: &BuildConfig, work: &Path, tx: &Sender<BuildEvent>) -> Result<Self, String> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let name = format!("git2pkg-build-{}-{}", std::process::id(), nanos);
        let args = vec![
            "run".to_string(),
            "-d".to_string(),
            "--name".to_string(),
            name.clone(),
            "-v".to_string(),
            format!("{}:/work", work.to_string_lossy()),
            "-w".to_string(),
            "/work".to_string(),
            config.target_distro.info(&config.distro).base_image.clone(),
            "sleep".to_string(),
            "infinity".to_string(),
        ];
        let started = run_streamed(config.engine.binary(), &args, tx)?;
        if !started.success {
            return Err("Container konnte nicht gestartet werden".to_string());
        }
        *config.container_id.lock().unwrap() = Some(name.clone());
        let _ = tx.send(BuildEvent::Log(format!("[container] {}", name)));
        let _ = tx.send(BuildEvent::ContainerStarted(name.clone()));
        Ok(Self {
            engine: config.engine.clone(),
            name,
        })
    }

    pub(crate) fn exec(
        &self,
        command: &str,
        tx: &Sender<BuildEvent>,
    ) -> Result<CommandOutput, String> {
        let args = vec![
            "exec".to_string(),
            self.name.clone(),
            "bash".to_string(),
            "-lc".to_string(),
            command.to_string(),
        ];
        run_streamed(self.engine.binary(), &args, tx)
    }

    pub(crate) fn stop(&self) {
        let args = vec!["rm", "-f", &self.name];
        let _ = Command::new(self.engine.binary())
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    fn exec_script(&self, tx: &Sender<BuildEvent>) -> Result<CommandOutput, String> {
        let args = vec![
            "exec".to_string(),
            self.name.clone(),
            "bash".to_string(),
            "/work/build.sh".to_string(),
        ];
        run_streamed(self.engine.binary(), &args, tx)
    }
}

impl Drop for BuildContainer {
    fn drop(&mut self) {
        let args = vec!["rm".to_string(), "-f".to_string(), self.name.clone()];
        let _ = Command::new(self.engine.binary())
            .args(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn package_database_sync(family: &DistroFamily) -> Option<&'static str> {
    match family {
        DistroFamily::Arch => Some("pacman -Sy --noconfirm && pacman -Fy --noconfirm"),
        DistroFamily::Debian => Some("apt-get update -y && (apt-file update -y || true)"),
        DistroFamily::Fedora => Some("dnf makecache -y || true"),
        DistroFamily::Suse => Some("zypper refresh || true"),
        DistroFamily::Unknown => None,
    }
}

fn ensure_package_database(
    config: &BuildConfig,
    container: &BuildContainer,
    tx: &Sender<BuildEvent>,
    synced: &mut bool,
) -> Result<(), String> {
    if *synced {
        return Ok(());
    }
    let target = config.target_distro.info(&config.distro);
    if let Some(command) = package_database_sync(&target.family) {
        let _ = container.exec(command, tx)?;
    }
    *synced = true;
    Ok(())
}

/// Build-Strategie, die bei "Erneut versuchen" durchgereicht wird.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStrategy {
    /// Stufe 1: Standard-Build inklusive deps/-Vorbau.
    Default,
    /// Stufe 2: Hauptprojekt direkt gegen Systembibliotheken, ohne deps/-Vorbau.
    SystemLibsOnly,
    /// Stufe 3: Repository-Buildskript mit erkannten Help-Flags.
    RepoScript,
    /// Stufe 4: Ignoriert PKGBUILD/Skripte, direktes DESTDIR-Staging.
    DirectStagingFallback,
}

impl BuildStrategy {
    pub fn index(self) -> u8 {
        match self {
            Self::Default => 1,
            Self::SystemLibsOnly => 2,
            Self::RepoScript => 3,
            Self::DirectStagingFallback => 4,
        }
    }

    /// Nächste sinnvolle Stufe. Ohne lokale deps/ wird Stufe 2 übersprungen.
    pub fn next(self, has_deps: bool) -> Self {
        match self {
            BuildStrategy::Default if has_deps => BuildStrategy::SystemLibsOnly,
            BuildStrategy::Default | BuildStrategy::SystemLibsOnly => BuildStrategy::RepoScript,
            BuildStrategy::RepoScript => BuildStrategy::DirectStagingFallback,
            BuildStrategy::DirectStagingFallback => BuildStrategy::Default,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            BuildStrategy::Default => "Default",
            BuildStrategy::SystemLibsOnly => "SystemLibsOnly",
            BuildStrategy::RepoScript => "RepoScript",
            BuildStrategy::DirectStagingFallback => "DirectStagingFallback",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "Default" => Some(Self::Default),
            "SystemLibsOnly" => Some(Self::SystemLibsOnly),
            "RepoScript" => Some(Self::RepoScript),
            "DirectStagingFallback" => Some(Self::DirectStagingFallback),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            BuildStrategy::Default => "Standard",
            BuildStrategy::SystemLibsOnly => "System-Bibliotheken ohne deps/-Vorbau",
            BuildStrategy::RepoScript => "Repository-Buildskript",
            BuildStrategy::DirectStagingFallback => "Direkter Build-System-Fallback",
        }
    }
}

/// Standard-Ausgabeordner: `~/Downloads/git2pkg`, relativ zum Home-Verzeichnis
/// des aktuellen Nutzers und nicht zum Arbeitsverzeichnis.
pub fn default_output_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    Path::new(&home).join("Downloads/git2pkg")
}

/// Markiert einen fehlgeschlagenen Build im Cache, damit die GUI beim Beenden
/// gezielt aufräumen kann. Erfolgreiche Builds behalten ihren Quellcode.
pub fn mark_failed_build(work: &Path) {
    let _ = fs::write(work.join("FAILED"), "failed\n");
}

fn build_cache_root() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    Path::new(&home).join(".cache/git2pkg/builds")
}

/// True, wenn mindestens ein fehlgeschlagener Build-Cache existiert.
#[allow(dead_code)]
pub fn has_failed_build_cache() -> bool {
    let Ok(entries) = fs::read_dir(build_cache_root()) else {
        return false;
    };
    entries
        .flatten()
        .any(|entry| entry.path().join("FAILED").is_file())
}

/// "Alles löschen" beim Beenden: kompletter Build-Cache, unabhängig davon,
/// ob ein Ordner als fehlgeschlagen markiert wurde.
pub fn cleanup_all_builds() -> usize {
    let Ok(entries) = fs::read_dir(build_cache_root()) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if remove_build_dir_recursively(&path).is_ok() {
                removed += 1;
            }
        } else if fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    let _ = fs::remove_dir(build_cache_root());
    removed
}

/// Löscht nur temporäre Build-/Paket-Verzeichnisse fehlgeschlagener Builds;
/// der geklonte Quellcode bleibt für einen erneuten Versuch erhalten.
pub fn cleanup_failed_build_artifacts() -> usize {
    cleanup_failed_build_artifacts_in(&build_cache_root())
}

fn remove_heavy_build_dirs(work: &Path) -> usize {
    let mut cleaned = 0;
    for name in [
        "build",
        "deps-build",
        "cargo-target",
        "cargo-home",
        "ccache",
        "out",
        "dist",
        "pkg",
    ] {
        let dir = work.join(name);
        if dir.is_dir() && fs::remove_dir_all(&dir).is_ok() {
            cleaned += 1;
        }
    }
    for name in ["build.sh", "BUILD_LOG.txt", "FAILED"] {
        let file = work.join(name);
        if file.is_file() && fs::remove_file(&file).is_ok() {
            cleaned += 1;
        }
    }
    cleaned
}

/// Setzt recursive `u+rwX`, damit auch schreibgeschuetzte Dateien aus
/// `.git/objects/...` das Entfernen des Build-Ordners nicht blockieren.
fn make_tree_writable(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_symlink() {
        return;
    }
    let mut permissions = metadata.permissions();
    let mode = permissions.mode();
    let writable = if metadata.is_dir() {
        mode | 0o700
    } else {
        mode | 0o600
    };
    if writable != mode {
        permissions.set_mode(writable);
        let _ = fs::set_permissions(path, permissions);
    }
    if metadata.is_dir() {
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                make_tree_writable(&entry.path());
            }
        }
    }
}

fn remove_build_dir_recursively(work: &Path) -> Result<(), String> {
    make_tree_writable(work);
    let result = match fs::remove_dir_all(work) {
        Ok(()) => Ok(()),
        Err(error) => {
            make_tree_writable(work);
            fs::remove_dir_all(work)
                .map_err(|retry| format!("{error}; nach Permissions-Reset: {retry}"))
        }
    };
    // Falls dies der letzte Build-Ordner war, auch das leere builds/ entfernen.
    if let Some(parent) = work.parent() {
        let _ = fs::remove_dir(parent);
    }
    result
}

fn cleanup_successful_build(work: &Path, tx: &Sender<BuildEvent>) {
    match remove_build_dir_recursively(work) {
        Ok(()) => {
            let _ = tx.send(BuildEvent::Log(
                "[cache] Vollständiger Build-Cache automatisch entfernt".to_string(),
            ));
        }
        Err(error) => {
            let _ = tx.send(BuildEvent::Log(format!(
                "[warn] Build-Cache konnte nicht vollständig entfernt werden: {error}"
            )));
        }
    }
}

fn cleanup_failed_build_artifacts_in(root: &Path) -> usize {
    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };
    let mut cleaned = 0;
    for entry in entries.flatten() {
        let work = entry.path();
        if !work.join("FAILED").is_file() {
            continue;
        }
        cleaned += remove_heavy_build_dirs(&work);
        // Der Cache gilt jetzt als bereinigt, der Quellcode bleibt aber nutzbar.
        let _ = fs::remove_file(work.join("FAILED"));
    }
    cleaned
}

fn engine_is_available(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// True, wenn Podman oder Docker installiert und aufrufbar ist.
pub fn container_engine_available() -> bool {
    engine_is_available("podman") || engine_is_available("docker")
}

pub fn detect_engine() -> ContainerEngine {
    for (name, engine) in [
        ("podman", ContainerEngine::Podman),
        ("docker", ContainerEngine::Docker),
    ] {
        if engine_is_available(name) {
            return engine;
        }
    }
    ContainerEngine::Docker
}

#[derive(Debug, Clone)]
pub struct BuildConfig {
    pub url: String,
    pub target: TargetFormat,
    /// Gewähltes Paketziel; `Auto` nutzt die erkannte Host-Distribution.
    pub target_distro: TargetDistro,
    pub distro: DistroInfo,
    pub engine: ContainerEngine,
    pub output_dir: PathBuf,
    pub language: Language,
    pub strategy: BuildStrategy,
    pub repo_script: Option<String>,
    /// Gesetzt, wenn ein CMake-Policy-Fehler den Retry mit Compat-Flags erzwingt.
    pub cmake_compat: bool,
    /// Gelernter Fix für fehlende Standard-Integer-Header.
    pub cstdint_fix: bool,
    /// Gelernter Fix für fehlende Git-Submodule.
    pub submodule_fix: bool,
    /// Gelernter Self-Healing-Fix: stabile rustc-Toolchain via rustup.
    pub rustup_fix: bool,
    /// Nach gescheitertem deps/-Vorbau: Hauptprojekt gegen Systembibliotheken bauen.
    pub skip_deps: bool,
    /// Vom GUI-Thread gesetzt, um einen laufenden Build sofort zu stoppen.
    pub cancel_flag: Arc<AtomicBool>,
    /// ID des aktuellen Wegwerf-Containers für den sofortigen Stopp.
    pub container_id: Arc<Mutex<Option<String>>>,
}

#[derive(Debug, Clone)]
pub enum BuildEvent {
    Step(String),
    Log(String),
    Finished(BuildOutcome),
    Failed(String),
    ContainerStarted(String),
    ProgressReset,
    Cancelled,
}

const CANCEL_TOKEN: &str = "__GIT2PKG_CANCELLED__";

struct CancelWatchDone(Arc<AtomicBool>);
impl Drop for CancelWatchDone {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

fn abort_running_build(
    container: &BuildContainer,
    config: &BuildConfig,
    tx: &Sender<BuildEvent>,
) -> String {
    container.stop();
    *config.container_id.lock().unwrap() = None;
    let _ = tx.send(BuildEvent::Log(
        "[warn] Build vom Benutzer abgebrochen.".to_string(),
    ));
    CANCEL_TOKEN.to_string()
}

#[derive(Debug, Clone)]
pub struct BuildOutcome {
    pub artifacts: Vec<PathBuf>,
    pub recipe: String,
    pub install_command: String,
}

pub(crate) struct CommandOutput {
    pub(crate) success: bool,
    pub(crate) text: String,
}

fn step(tx: &Sender<BuildEvent>, message: impl Into<String>) {
    let _ = tx.send(BuildEvent::Step(message.into()));
}

pub fn run_build(mut config: BuildConfig, tx: Sender<BuildEvent>) {
    match execute(&mut config, &tx) {
        Ok(outcome) => {
            let _ = tx.send(BuildEvent::Finished(outcome));
        }
        Err(err) if err == CANCEL_TOKEN => {
            let _ = tx.send(BuildEvent::Cancelled);
        }
        Err(err) => {
            let _ = tx.send(BuildEvent::Failed(err));
        }
    }
}

/// Paketprobe im laufenden Container. Erfolg = exit code 0.
fn try_install(
    config: &BuildConfig,
    container: &BuildContainer,
    package: &str,
    tx: &Sender<BuildEvent>,
    synced: &mut bool,
) -> bool {
    let target = config.target_distro.info(&config.distro);
    let command = install_packages_command(&target.family, &[package.to_string()]);
    if command.is_empty() {
        return false;
    }
    if ensure_package_database(config, container, tx, synced).is_err() {
        return false;
    }
    match container.exec(&command, tx) {
        Ok(output) => output.success,
        Err(_) => false,
    }
}

/// Findet eine im CMake-Fehler genannte Datei relativ zum Quellcode-Cache.
fn find_source_file(root: &Path, relative: &str) -> Option<PathBuf> {
    let direct = root.join(relative);
    if direct.is_file() {
        return Some(direct);
    }
    let needle = relative.replace('\\', "/");
    fn walk(dir: &Path, needle: &str) -> Option<PathBuf> {
        for entry in fs::read_dir(dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = walk(&path, needle) {
                    return Some(found);
                }
            } else if path.to_string_lossy().replace('\\', "/").ends_with(needle) {
                return Some(path);
            }
        }
        None
    }
    walk(root, &needle)
}

/// Kommentiert einen `source_group(...)`-Block ab der angegebenen 1-basierten Zeile aus.
fn comment_source_group_block(path: &Path, line: usize) -> bool {
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    let trailing_newline = text.ends_with('\n');
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let start = line.saturating_sub(1);
    if start >= lines.len() || !lines[start].contains("source_group") {
        return false;
    }
    let mut balance = 0_i32;
    let mut end = start;
    for (index, line) in lines.iter().enumerate().skip(start) {
        balance += line.matches('(').count() as i32 - line.matches(')').count() as i32;
        end = index;
        if balance <= 0 {
            break;
        }
    }
    for line in lines.iter_mut().take(end + 1).skip(start) {
        let trimmed = line.trim_start().to_string();
        if !trimmed.starts_with('#') {
            let indent = line.len() - trimmed.len();
            *line = format!("{}# {}", &line[..indent], trimmed);
        }
    }
    let mut output = lines.join("\n");
    if trailing_newline {
        output.push('\n');
    }
    fs::write(path, output).is_ok()
}

/// Leitet das Wurzelverzeichnis aus dem absoluten Pfad der Fehlermeldung ab.
fn find_source_file_from_hint(hint: &Path, relative: &str) -> Option<PathBuf> {
    let mut current = hint.parent();
    while let Some(dir) = current {
        let candidate = dir.join(relative);
        if candidate.is_file() {
            return Some(candidate);
        }
        current = dir.parent();
    }
    None
}

/// Selbstheilung: `source_group` ist unter Linux rein kosmetisch und darf entfallen.
fn heal_source_group(work: &Path, error: &SourceGroupError, tx: &Sender<BuildEvent>) -> bool {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(hint) = &error.hint {
        if let Some(path) = find_source_file_from_hint(Path::new(hint), &error.file) {
            candidates.push(path);
        }
    }
    for root in [
        work.join("deps-build"),
        work.join("src"),
        work.to_path_buf(),
    ] {
        if let Some(path) = find_source_file(&root, &error.file) {
            if !candidates.contains(&path) {
                candidates.push(path);
            }
        }
    }
    let Some(path) = candidates.into_iter().next() else {
        return false;
    };
    if !comment_source_group_block(&path, error.line) {
        return false;
    }
    let _ = tx.send(BuildEvent::Log(format!(
        "[learn] CMake source_group-Inkompatibilität in {} automatisch behoben",
        error.file
    )));
    true
}

fn execute(config: &mut BuildConfig, tx: &Sender<BuildEvent>) -> Result<BuildOutcome, String> {
    let texts = Texts::new(config.language);
    let mut mappings = MappingStore::load();
    let mut failed_candidates: HashSet<String> = HashSet::new();
    let mut healed_source_groups: HashSet<(String, usize, String)> = HashSet::new();
    let mut skipped_incomplete: HashSet<String> = HashSet::new();

    step(tx, texts.step_workdir());
    let work = temp_workdir(&config.url)?;
    let _ = fs::remove_file(work.join("FAILED"));
    // Ausgabeordner sofort anlegen – auch wenn der Clone später fehlschlägt.
    fs::create_dir_all(&config.output_dir).map_err(|e| format!("output dir: {}", e))?;

    let src = work.join("src");
    if src.join(".git").is_dir() {
        let _ = tx.send(BuildEvent::Log(format!(
            "[cache] Vorhandenen Quellcode wieder verwenden: {}",
            src.display()
        )));
        let _ = run_streamed(
            "git",
            &[
                "-C".to_string(),
                src.to_string_lossy().into_owned(),
                "reset".to_string(),
                "--hard".to_string(),
            ],
            tx,
        );
        let _ = run_streamed(
            "git",
            &[
                "-C".to_string(),
                src.to_string_lossy().into_owned(),
                "clean".to_string(),
                "-fd".to_string(),
            ],
            tx,
        );
    } else {
        step(tx, texts.step_clone());
        let clone_args = vec![
            "clone".to_string(),
            "--depth".to_string(),
            "1".to_string(),
            config.url.clone(),
            src.to_string_lossy().into_owned(),
        ];
        let cloned = run_streamed("git", &clone_args, tx)?;
        if !cloned.success {
            mark_failed_build(&work);
            return Err(texts.err_clone_failed().to_string());
        }
    }

    let mut repo_script = config
        .repo_script
        .clone()
        .or_else(|| find_repo_script(&src));
    let repo_pkgbuild = src.join("PKGBUILD").is_file();
    let identity = project_identity(&config.url, &src);
    let _ = tx.send(BuildEvent::Log(format!(
        "[project] {} {}",
        identity.name, identity.version
    )));

    let fingerprint = ProjectFingerprint::from_root(&src);
    let mut learned_packages: Vec<String> = Vec::new();
    if let Some((recipe, _exact)) =
        mappings.apply_learned_recipe(&config.url, &fingerprint.signature)
    {
        if let Some(strategy) = BuildStrategy::from_key(&recipe.strategy) {
            config.strategy = strategy;
        }
        config.skip_deps = recipe.skip_deps;
        config.cmake_compat |= recipe.cmake_compat;
        config.cstdint_fix |= recipe.cstdint_fix;
        config.submodule_fix |= recipe.submodule_fix;
        learned_packages = recipe
            .packages
            .into_iter()
            .filter(|name| valid_package_name(name))
            .collect();
        let _ = tx.send(BuildEvent::Log(format!(
            "[learn] Bekannte Bauweise für Struktur '{}' angewendet",
            fingerprint.signature
        )));
        if !learned_packages.is_empty() {
            let _ = tx.send(BuildEvent::Log(format!(
                "[learn] Gespeicherte Pakete für diesen Build vorinstallieren: {}",
                learned_packages.join(" ")
            )));
        }
    }

    if config.strategy == BuildStrategy::RepoScript {
        let _ = tx.send(BuildEvent::Log(format!(
            "[strategy] {} - Repo-Skript: {}",
            config.strategy.label(),
            repo_script.as_deref().unwrap_or("keines gefunden")
        )));
    }
    let _ = tx.send(BuildEvent::Log(format!(
        "[strategy] {}",
        config.strategy.label()
    )));

    let system = detect_build_system(&src);
    step(tx, texts.step_build_system(system.label()));
    if system == BuildSystem::Unknown {
        let _ = tx.send(BuildEvent::Log(texts.unknown_system().to_string()));
    }

    let has_deps = has_local_dependencies(&src);

    // Ein Container fuer den gesamten Build inklusive Suche und Nachinstallation.
    let container = BuildContainer::start(config, &work, tx)?;
    let watch_done = Arc::new(AtomicBool::new(false));
    let _watch_guard = CancelWatchDone(watch_done.clone());
    {
        let cancel = config.cancel_flag.clone();
        let container_id = config.container_id.clone();
        let engine = config.engine.clone();
        std::thread::spawn(move || {
            while !watch_done.load(Ordering::Relaxed) {
                if cancel.load(Ordering::Relaxed) {
                    if let Some(id) = container_id.lock().unwrap().take() {
                        let _ = Command::new(engine.binary())
                            .args(["rm", "-f", &id])
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .status();
                    }
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
    }
    let mut db_synced = false;
    let mut extra_packages: Vec<String> = learned_packages;
    let mut cmake_flags: Vec<String> = Vec::new();
    let mut last_output = String::new();
    let mut forced_strategy: Option<BuildStrategy> = None;
    let mut direct_tried = false;

    let mut strategy_attempt: u8 = 1;
    let mut heal_round: u8 = 1;
    let mut allowed_rounds: u8 = INITIAL_HEAL_ROUNDS;
    'attempt: while strategy_attempt <= MAX_ATTEMPTS {
        if config.cancel_flag.load(Ordering::Relaxed) {
            return Err(abort_running_build(&container, config, tx));
        }
        let round_state = RetryState {
            packages: extra_packages.clone(),
            cmake_flags: cmake_flags.clone(),
        };
        let effective_strategy = forced_strategy.unwrap_or(config.strategy);
        let strategy_name = effective_strategy.label();
        let _ = tx.send(BuildEvent::ProgressReset);
        step(
            tx,
            texts.step_strategy_round(
                strategy_name,
                effective_strategy.index(),
                MAX_ATTEMPTS,
                heal_round,
                allowed_rounds,
            ),
        );
        // Frische Artefakte pro Versuch, der geklonte Quellcode bleibt erhalten.
        for name in ["out", "dist", "pkg"] {
            let dir = work.join(name);
            if dir.exists() {
                let _ = fs::remove_dir_all(&dir);
            }
            fs::create_dir_all(&dir).map_err(|e| format!("cache dir: {}", e))?;
        }
        if mappings.is_incomplete_strategy(&config.url, strategy_name)
            && !skipped_incomplete.contains(strategy_name)
        {
            skipped_incomplete.insert(strategy_name.to_string());
            let (next, message) = if effective_strategy == BuildStrategy::Default
                && mappings.has_incomplete_default(&config.url)
            {
                (
                    BuildStrategy::SystemLibsOnly,
                    "[learn] Gespeichertes Lern-Profil aktiv — überspringe deps/ und baue direkt gegen System-Bibliotheken".to_string(),
                )
            } else {
                let next = effective_strategy.next(has_deps);
                (
                    next,
                    format!(
                        "[learn] Bekanntes unvollständiges Profil für Strategie '{}' übersprungen — starte direkt mit alternativer Strategie",
                        strategy_name
                    ),
                )
            };
            let _ = tx.send(BuildEvent::Log(message));
            config.skip_deps = next == BuildStrategy::SystemLibsOnly;
            strategy_attempt += 1;
            heal_round = 1;
            allowed_rounds = INITIAL_HEAL_ROUNDS;
            forced_strategy = Some(next);
            continue 'attempt;
        }
        // Nur Stufe 1 baut lokale deps/ vor; alle späteren Stufen nutzen Systemlibs.
        let deps_first = has_deps && effective_strategy == BuildStrategy::Default;
        if deps_first {
            let _ = tx.send(BuildEvent::Log(
                "[deps] 'deps/CMakeLists.txt' erkannt - lokale Abhaengigkeiten werden zuerst gebaut"
                    .to_string(),
            ));
        }
        let mut script = build_script_with_flags(
            config,
            &system,
            &extra_packages,
            repo_script.as_deref(),
            &identity,
            effective_strategy,
            repo_pkgbuild,
            &cmake_flags,
        );
        if deps_first
            && effective_strategy != BuildStrategy::DirectStagingFallback
            && !config.skip_deps
        {
            script = with_deps_prebuild_flags(&script, &cmake_flags);
        }
        let result = match run_container(&container, &work, &script, tx) {
            Ok(result) => result,
            Err(error) => {
                if config.cancel_flag.load(Ordering::Relaxed) {
                    return Err(abort_running_build(&container, config, tx));
                }
                return Err(error);
            }
        };
        if config.cancel_flag.load(Ordering::Relaxed) {
            return Err(abort_running_build(&container, config, tx));
        }
        last_output = result.text.clone();
        let deps_failed = deps_fallback_needed(&last_output, config.skip_deps);

        // Repo-Skripte pruefen oft nur wenige Distributionen. Wenn sie diese
        // Distro ablehnen, generisch weiterbauen statt den Lauf zu beenden.
        if effective_strategy == BuildStrategy::RepoScript
            && detect_repo_script_distro_rejection(&last_output)
        {
            let _ = tx.send(BuildEvent::Log(
                "[fallback] Repo-Buildskript unterstuetzt diese Distribution nicht - wechsle zum generischen Build-System"
                    .to_string(),
            ));
            repo_script = None;
            strategy_attempt += 1;
            heal_round = 1;
            allowed_rounds = INITIAL_HEAL_ROUNDS;
            config.skip_deps = false;
            forced_strategy = Some(BuildStrategy::Default);
            continue 'attempt;
        }

        if result.text.contains(OK_MARKER) {
            if !verify_built_packages(
                &container,
                config,
                tx,
                is_compiled_build_system(system.label()),
                strategy_name,
                &mut mappings,
            ) {
                // Lernprofil: kompiliertes Projekt ohne ausführbare Datei im Paket.
                let mut incomplete_profile = false;
                let unresolved_dependencies = !analyze_output(&last_output).is_empty();
                if is_compiled_build_system(system.label())
                    && !deps_failed
                    && !unresolved_dependencies
                {
                    if let Ok(listing) = container.exec("find /work/dist -maxdepth 1 -type f", tx) {
                        for raw in listing.text.lines() {
                            let path = raw.trim();
                            let kind = if path.ends_with(".pkg.tar.zst") {
                                "pkg"
                            } else if path.ends_with(".deb") {
                                "deb"
                            } else if path.ends_with(".rpm") {
                                "rpm"
                            } else {
                                continue;
                            };
                            if let Ok(archive) =
                                container.exec(&archive_listing_command(kind, path), tx)
                            {
                                if package_lacks_executable(&archive.text) {
                                    let _ = tx.send(BuildEvent::Log(
                                        "[warn] Erzeugtes Paket enthält keine ausführbare Programmdatei — merke Strategie für dieses Repository als unvollständig".to_string(),
                                    ));
                                    mappings.mark_incomplete_strategy(&config.url, strategy_name);
                                    incomplete_profile = true;
                                    break;
                                }
                            }
                        }
                    }
                }
                if incomplete_profile {
                    let next = effective_strategy.next(has_deps);
                    strategy_attempt += 1;
                    heal_round = 1;
                    allowed_rounds = INITIAL_HEAL_ROUNDS;
                    config.skip_deps = next == BuildStrategy::SystemLibsOnly;
                    forced_strategy = Some(next);
                    let _ = tx.send(BuildEvent::Log(format!(
                        "[strategy] {} (unvollständiges Profil)",
                        next.label()
                    )));
                    continue;
                }
                if direct_tried {
                    mark_failed_build(&work);
                    return Err(
                        "Build erzeugte auch mit direktem Build-System kein nutzbares Paket"
                            .to_string(),
                    );
                }
                direct_tried = true;
                strategy_attempt += 1;
                heal_round = 1;
                allowed_rounds = INITIAL_HEAL_ROUNDS;
                forced_strategy = Some(BuildStrategy::DirectStagingFallback);
                let _ = tx.send(BuildEvent::Log(format!(
                    "[strategy] {}",
                    BuildStrategy::DirectStagingFallback.label()
                )));
                continue;
            }
            if config.cancel_flag.load(Ordering::Relaxed) {
                return Err(abort_running_build(&container, config, tx));
            }
            if config.target == TargetFormat::NativePackage {
                let recipe = BuildRecipe {
                    signature: fingerprint.signature.clone(),
                    strategy: effective_strategy.key().to_string(),
                    skip_deps: config.skip_deps,
                    cmake_compat: config.cmake_compat,
                    cstdint_fix: config.cstdint_fix,
                    submodule_fix: config.submodule_fix,
                    packages: extra_packages.clone(),
                };
                mappings.record_build_recipe(&config.url, recipe);
                let _ = tx.send(BuildEvent::Log(format!(
                    "[learn] Erfolgreiche Bauweise für Struktur '{}' gespeichert",
                    fingerprint.signature
                )));
            }
            let outcome = package_result(config, &work, &identity, &last_output, tx)?;
            cleanup_successful_build(&work, tx);
            return Ok(outcome);
        }

        // CMake nennt im Fehlertext oft alternative -D-Flags oder benoetigt
        // Policy-/Eigen-Kompatibilitaetsflags. Das gilt nur fuer den naechsten
        // Versuch derselben Strategie, nie fuer spaetere Laeufe.
        let discovered_flags = cmake_retry_flags(&last_output, &cmake_flags);
        if !discovered_flags.is_empty() {
            let _ = tx.send(BuildEvent::Log(format!(
                "[cmake] Retry-Flags aus Fehlermeldung übernommen: {}",
                discovered_flags.join(" ")
            )));
            for flag in discovered_flags {
                push_unique_string(&mut cmake_flags, flag);
            }
            heal_round += 1;
            continue;
        }

        if let Some(error) = detect_source_group_error(&last_output) {
            let key = (
                error.file.clone(),
                error.line,
                error.hint.clone().unwrap_or_default(),
            );
            if !healed_source_groups.contains(&key) && heal_source_group(&work, &error, tx) {
                healed_source_groups.insert(key);
                heal_round += 1;
                if heal_round > allowed_rounds {
                    let next = effective_strategy.next(has_deps);
                    strategy_attempt += 1;
                    heal_round = 1;
                    allowed_rounds = INITIAL_HEAL_ROUNDS;
                    config.skip_deps = next == BuildStrategy::SystemLibsOnly;
                    forced_strategy = Some(next);
                }
                continue;
            }
        }

        for fix in detect_build_fixes(&last_output) {
            if mappings.learn_build_fix(&fix.key, &fix.value) {
                let _ = tx.send(BuildEvent::Log(format!(
                    "[learn] Fehlermuster erkannt -> Fix angewendet & gespeichert: {} -> {}",
                    fix.description, fix.value
                )));
            }
        }
        config.cmake_compat |= mappings.build_fix("cmake_policy").is_some();
        config.cstdint_fix = mappings.build_fix("cstdint").is_some();
        config.submodule_fix = mappings.build_fix("submodules").is_some();
        if !config.rustup_fix && mappings.build_fix("rustup_toolchain").is_some() {
            let _ = tx.send(BuildEvent::Log(
                "[heal] Veraltete rustc-Toolchain erkannt – installiere stable ueber rustup"
                    .to_string(),
            ));
        }
        config.rustup_fix = mappings.build_fix("rustup_toolchain").is_some();

        let missing = analyze_output(&last_output);
        if missing.is_empty() {
            if deps_failed {
                let _ = tx.send(BuildEvent::Log(
                    "[warn] Lokaler deps/-Vorbau fehlgeschlagen – wechsle auf System-Bibliotheken der Distribution..."
                        .to_string(),
                ));
                config.skip_deps = true;
                strategy_attempt += 1;
                heal_round = 1;
                allowed_rounds = INITIAL_HEAL_ROUNDS;
                forced_strategy = Some(BuildStrategy::SystemLibsOnly);
                continue 'attempt;
            }
            mark_failed_build(&work);
            return Err(texts.err_no_pattern().to_string());
        }

        for dep in &missing {
            step(tx, texts.step_search(&dep.label()));
            let term = dep.search_term();
            let raw_name = dep.raw_name().to_string();
            let target = config.target_distro.info(&config.distro);
            let family = family_key(&target.family);
            let manager = &target.package_manager;

            // Gelernte Zuordnung zuerst, dann die Kaskade aus Stufe 1.
            let mut candidates: Vec<String> = Vec::new();
            if let Some(learned) = mappings.get(family, &raw_name) {
                candidates.push(learned);
            }
            candidates.extend(candidate_names(dep, manager));

            let mut installed: Option<String> = None;
            for candidate in &candidates {
                let package = sanitize_package(candidate);
                if package.is_empty()
                    || failed_candidates.contains(&package)
                    || mappings.is_negative(&package)
                {
                    continue;
                }
                step(tx, texts.step_install(&package));
                if try_install(config, &container, &package, tx, &mut db_synced) {
                    installed = Some(package);
                    break;
                }
                failed_candidates.insert(package.clone());
                mappings.learn_negative(&package);
            }

            // Stufe 2/3: CMake-Config, .pc-Datei oder Header in der Paketdatenbank suchen.
            if installed.is_none() {
                for command in probe_commands(manager, dep) {
                    if ensure_package_database(config, &container, tx, &mut db_synced).is_err() {
                        continue;
                    }
                    let Ok(found) = container.exec(&command, tx) else {
                        continue;
                    };
                    for candidate in found
                        .text
                        .lines()
                        .filter_map(|line| package_from_search(line, &term, manager))
                    {
                        let package = sanitize_package(&candidate);
                        if package.is_empty() || failed_candidates.contains(&package) {
                            continue;
                        }
                        step(tx, texts.step_install(&package));
                        if try_install(config, &container, &package, tx, &mut db_synced) {
                            installed = Some(package);
                            break;
                        }
                        failed_candidates.insert(package);
                    }
                    if installed.is_some() {
                        break;
                    }
                }
            }

            // Letzter Fallback für unbekannte Paketmanager.
            if installed.is_none() {
                let search = search_command(manager, dep);
                if !search.is_empty()
                    && ensure_package_database(config, &container, tx, &mut db_synced).is_ok()
                {
                    if let Ok(found) = container.exec(&search, tx) {
                        if let Some(candidate) = package_from_search(&found.text, &term, manager) {
                            let package = sanitize_package(&candidate);
                            if package.is_empty() || failed_candidates.contains(&package) {
                                continue;
                            }
                            if try_install(config, &container, &package, tx, &mut db_synced) {
                                installed = Some(package);
                            } else {
                                failed_candidates.insert(package);
                            }
                        }
                    }
                }
            }

            let Some(package) = installed else {
                if deps_failed {
                    let _ = tx.send(BuildEvent::Log(
                        "[warn] Lokaler deps/-Vorbau fehlgeschlagen – wechsle auf System-Bibliotheken der Distribution..."
                            .to_string(),
                    ));
                    config.skip_deps = true;
                    strategy_attempt += 1;
                    heal_round = 1;
                    allowed_rounds = INITIAL_HEAL_ROUNDS;
                    forced_strategy = Some(BuildStrategy::SystemLibsOnly);
                    continue 'attempt;
                }
                mark_failed_build(&work);
                return Err(texts.err_no_package(&dep.label()));
            };
            // Nach der ersten erfolgreichen Nachinstallation brauchen große
            // C++-Workspaces mehr als 5 Runden für alle find_package()-Aufrufe.
            allowed_rounds = MAX_HEAL_ROUNDS;
            mappings.learn(family, &raw_name, &package);
            let _ = tx.send(BuildEvent::Log(format!(
                "[learn] Aus Fehler gelernt & gespeichert: {} -> {}",
                raw_name, package
            )));
            if matches!(dep, MissingDep::PkgConfig(_)) {
                let _ = tx.send(BuildEvent::Log(format!(
                    "[learn] Fehlermuster erkannt -> Fix angewendet & gespeichert: pkg-config {} -> {}",
                    raw_name, package
                )));
            }
            if !extra_packages.contains(&package) {
                extra_packages.push(package.clone());
            }
            mappings.record_repo_packages(&config.url, &fingerprint.signature, &extra_packages);
        }
        let after_state = RetryState {
            packages: extra_packages.clone(),
            cmake_flags: cmake_flags.clone(),
        };
        if !retry_state_changed(&round_state, &after_state) {
            let next = effective_strategy.next(has_deps);
            strategy_attempt += 1;
            heal_round = 1;
            allowed_rounds = INITIAL_HEAL_ROUNDS;
            config.skip_deps = next == BuildStrategy::SystemLibsOnly;
            forced_strategy = Some(next);
            let _ = tx.send(BuildEvent::Log(format!(
                "[retry] Keine neuen Pakete oder Flags – wechsle zu Strategie {}",
                next.label()
            )));
            continue;
        }

        if heal_round >= allowed_rounds {
            let next = effective_strategy.next(has_deps);
            strategy_attempt += 1;
            heal_round = 1;
            allowed_rounds = INITIAL_HEAL_ROUNDS;
            config.skip_deps = next == BuildStrategy::SystemLibsOnly;
            forced_strategy = Some(next);
            let _ = tx.send(BuildEvent::Log(format!(
                "[strategy] {} (Selbstheilungs-Runden erschöpft)",
                next.label()
            )));
        } else {
            heal_round += 1;
        }
    }

    mark_failed_build(&work);
    Err(texts.err_max_attempts(strategy_attempt.saturating_sub(1), &last_output))
}

/// Nur echte Fehlermeldungen als `[err]` markieren - nicht alles, was auf stderr kommt.
fn is_error_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("error:")
        || lower.contains("fatal:")
        || lower.contains("fehler:")
        || lower.starts_with("error")
        || lower.starts_with("fatal")
}

fn spawn_reader<R: std::io::Read + Send + 'static>(
    reader: R,
    tx: Sender<BuildEvent>,
    buffer: Arc<Mutex<String>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(reader)
            .lines()
            .map_while(Result::ok)
        {
            let tag = if is_error_line(&line) { "err" } else { "out" };
            let _ = tx.send(BuildEvent::Log(format!("[{}] {}", tag, line)));
            if let Ok(mut shared) = buffer.lock() {
                shared.push_str(&line);
                shared.push('\n');
            }
        }
    })
}

fn run_streamed(
    program: &str,
    args: &[String],
    tx: &Sender<BuildEvent>,
) -> Result<CommandOutput, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("'{}': {}", program, e))?;

    let stdout: ChildStdout = child.stdout.take().ok_or("no stdout")?;
    let stderr: ChildStderr = child.stderr.take().ok_or("no stderr")?;
    let buffer = Arc::new(Mutex::new(String::new()));

    let h1 = spawn_reader(stdout, tx.clone(), buffer.clone());
    let h2 = spawn_reader(stderr, tx.clone(), buffer.clone());
    let _ = h1.join();
    let _ = h2.join();

    let status = child.wait().map_err(|e| e.to_string())?;
    let mut text = buffer.lock().map(|b| b.clone()).unwrap_or_default();
    if !status.success() {
        let code = status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "?".to_string());
        let line = format!("[err] exit code {}", code);
        let _ = tx.send(BuildEvent::Log(line.clone()));
        text.push_str(&line);
        text.push('\n');
    }
    Ok(CommandOutput {
        success: status.success(),
        text,
    })
}

fn run_container(
    container: &BuildContainer,
    work: &Path,
    script: &str,
    tx: &Sender<BuildEvent>,
) -> Result<CommandOutput, String> {
    let script_path = work.join("build.sh");
    fs::write(&script_path, script).map_err(|e| format!("script: {}", e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    container.exec_script(tx)
}

fn has_local_dependencies(src: &Path) -> bool {
    has_local_deps(src)
}

/// Standard-CMake-Flags plus alle aus Fehlermeldungen gelernten Zusatz-Flags.
fn cmake_configure_flags(extra: &[String]) -> String {
    let mut flags = vec![
        "-DCMAKE_BUILD_TYPE=Release".to_string(),
        "-DCMAKE_POLICY_VERSION_MINIMUM=3.5".to_string(),
        "-DCMAKE_SUPPRESS_DEVELOPER_ERRORS=ON".to_string(),
    ];
    for flag in extra {
        push_unique_string(&mut flags, flag.clone());
    }
    flags.join(" ")
}

/// CMake-Projekte mit `deps/` oder `third_party/` erwarten vorgebaute lokale
/// Abhängigkeiten. Das lokale Install-Prefix wird an das Hauptprojekt gereicht.
#[cfg(test)]
fn with_deps_prebuild(script: &str) -> String {
    with_deps_prebuild_flags(script, &[])
}

fn with_deps_prebuild_flags(script: &str, flags: &[String]) -> String {
    let configure_flags = cmake_configure_flags(flags);
    let mut lines: Vec<String> = script.lines().map(str::to_string).collect();
    let anchor = lines
        .iter()
        .position(|line| line == "mkdir -p /work/out /work/dist");
    if let Some(index) = anchor {
        lines.insert(
            index + 1,
            "export CMAKE_PREFIX_PATH=/work/deps-install".to_string(),
        );
        lines.insert(
            index + 2,
            format!(
                "for sub in /work/src/deps /work/src/third_party; do if [ -f \"$sub/CMakeLists.txt\" ]; then echo '[deps] Nachinstallation abgeschlossen - konfiguriere lokale Abhaengigkeiten neu'; rm -rf /work/deps-build; cmake -S \"$sub\" -B /work/deps-build -DCMAKE_INSTALL_PREFIX=/work/deps-install {configure_flags} && cmake --build /work/deps-build && cmake --install /work/deps-build || {{ echo '[deps] Bau fehlgeschlagen - Resolver uebernimmt'; exit 1; }}; fi; done"
            ),
        );
    }
    lines.join("\n")
}

fn toolchain_packages(family: &DistroFamily, system: &BuildSystem) -> Vec<&'static str> {
    let mut packages: Vec<&'static str> = Vec::new();
    match system {
        BuildSystem::Cargo => match family {
            DistroFamily::Arch => packages.push("rust"),
            DistroFamily::Debian => packages.extend(["cargo", "rustc"]),
            DistroFamily::Fedora | DistroFamily::Suse => packages.extend(["cargo", "rust"]),
            DistroFamily::Unknown => {}
        },
        BuildSystem::Go => match family {
            DistroFamily::Arch | DistroFamily::Suse => packages.push("go"),
            DistroFamily::Debian | DistroFamily::Fedora => packages.push("golang"),
            DistroFamily::Unknown => {}
        },
        BuildSystem::Qmake => match family {
            DistroFamily::Arch => packages.push("qt6-base"),
            DistroFamily::Debian => packages.push("qt6-base-dev"),
            DistroFamily::Fedora | DistroFamily::Suse => packages.push("qt6-qtbase-devel"),
            DistroFamily::Unknown => {}
        },
        _ => {}
    }
    packages
}

fn base_dependencies(family: &DistroFamily, system: &BuildSystem) -> String {
    let base = match family {
        DistroFamily::Arch => {
            "pacman -Syu --noconfirm --needed -- base-devel git pkgconf python cmake meson ninja"
        }
        DistroFamily::Debian => {
            "apt-get update -y && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends build-essential git pkg-config python3-pip cmake meson ninja-build debhelper devscripts fakeroot dpkg-dev"
        }
        DistroFamily::Fedora => {
            "dnf install -y @development-tools git pkgconf-pkg-config python3-pip cmake meson ninja-build rpm-build rpmdevtools dnf-plugins-core"
        }
        DistroFamily::Suse => {
            "zypper --non-interactive install git pkgconf-pkg-config python3 gcc gcc-c++ make cmake meson ninja rpm-build"
        }
        DistroFamily::Unknown => "true",
    };
    let tools = toolchain_packages(family, system).join(" ");
    if tools.is_empty() {
        base.to_string()
    } else {
        format!("{base} {tools}")
    }
}

/// Installiert bei zu alter System-`rustc` eine aktuelle stabile Toolchain
/// ueber rustup und stellt deren `bin` vor `/usr/bin` im PATH.
fn rustup_bootstrap_step(family: &DistroFamily) -> Vec<String> {
    let base: &[&str] = match family {
        DistroFamily::Debian => &["curl", "ca-certificates", "build-essential"],
        DistroFamily::Fedora | DistroFamily::Suse => {
            &["curl", "ca-certificates", "gcc", "glibc-devel"]
        }
        DistroFamily::Arch => &["curl", "ca-certificates", "base-devel"],
        DistroFamily::Unknown => &["curl", "ca-certificates"],
    };
    let packages: Vec<String> = base.iter().map(|name| name.to_string()).collect();
    let mut lines = Vec::new();
    let command = install_packages_command(family, &packages);
    if !command.is_empty() {
        lines.push(command);
    }
    lines.extend([
        "if [ ! -x \"$HOME/.cargo/bin/rustc\" ]; then".to_string(),
        "  unset CARGO_HOME".to_string(),
        "  export RUSTUP_HOME=\"$HOME/.rustup\"".to_string(),
        "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal".to_string(),
        "fi".to_string(),
        "export PATH=\"$HOME/.cargo/bin:$PATH\"".to_string(),
        "export CARGO_HOME=/work/cargo-home".to_string(),
        "\"$HOME/.cargo/bin/cargo\" --version".to_string(),
    ]);
    lines
}

fn install_packages_command(family: &DistroFamily, packages: &[String]) -> String {
    let cleaned: Vec<String> = packages
        .iter()
        .map(|p| sanitize_package(p))
        .filter(|p| !p.is_empty())
        .collect();
    if cleaned.is_empty() {
        return String::new();
    }
    let list = cleaned.join(" ");
    match family {
        // `pacman --noconfirm` reicht; `yes | pacman` liefert unter pipefail
        // beim Schliessen der Pipe einen harmlosen SIGPIPE (141).
        DistroFamily::Arch => format!("pacman -S --noconfirm --needed -- {}", list),
        DistroFamily::Debian => {
            format!("DEBIAN_FRONTEND=noninteractive apt-get install -y {}", list)
        }
        DistroFamily::Fedora => format!("dnf install -y {}", list),
        DistroFamily::Suse => format!("zypper --non-interactive install {}", list),
        DistroFamily::Unknown => String::new(),
    }
}

/// Name und Version des gebauten Projekts, aus URL und Quellcode ermittelt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectIdentity {
    pub name: String,
    pub version: String,
}

fn package_slug(raw: &str) -> String {
    let lower = raw.trim().to_ascii_lowercase();
    let cleaned: String = lower
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches(|c| c == '-' || c == '.').to_string();
    if trimmed.is_empty() {
        "app".to_string()
    } else {
        trimmed
    }
}

/// Zentrale Versionsbereinigung für alle Paketformate.
/// Entfernt `v`/`V`-Präfixe, ersetzt ungültige Zeichen durch Punkte,
/// fasst Mehrfachpunkte zusammen und erzwingt einen führenden Ziffer.
fn version_slug(raw: &str) -> String {
    let mut text = raw.trim();
    while let Some(rest) = text.strip_prefix('v').or_else(|| text.strip_prefix('V')) {
        text = rest;
    }

    let mut cleaned = String::with_capacity(text.len());
    let mut last_dot = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            cleaned.push(ch);
            last_dot = false;
        } else if !cleaned.is_empty() && !last_dot {
            cleaned.push('.');
            last_dot = true;
        }
    }

    let result = cleaned.trim_matches('.').to_string();
    if result.is_empty() || !result.as_bytes()[0].is_ascii_digit() {
        "0.1.0".to_string()
    } else {
        result
    }
}

fn repo_name_from_url(url: &str) -> Option<String> {
    let clean = url.split('?').next().unwrap_or(url).trim_end_matches('/');
    let last = clean.rsplit('/').next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn toml_version(text: &str) -> Option<String> {
    let re = Regex::new(r#"(?m)^\s*version\s*=\s*"([^"]+)""#).unwrap();
    re.captures(text).map(|caps| caps[1].to_string())
}

fn cmake_version(text: &str) -> Option<String> {
    let re = Regex::new(r"(?is)project\s*\([^)]*?\bVERSION\s+([0-9][0-9A-Za-z.\-+]*)").unwrap();
    re.captures(text).map(|caps| caps[1].to_string())
}

fn git_version(src: &Path) -> Option<String> {
    for args in [
        vec!["describe", "--tags", "--abbrev=0"],
        vec!["describe", "--tags"],
    ] {
        if let Ok(output) = Command::new("git").arg("-C").arg(src).args(args).output() {
            if output.status.success() {
                let tag = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !tag.is_empty() {
                    return Some(tag);
                }
            }
        }
    }
    None
}

fn project_identity(url: &str, src: &Path) -> ProjectIdentity {
    let name = repo_name_from_url(url)
        .map(|raw| package_slug(&raw))
        .unwrap_or_else(|| "app".to_string());
    let version = if src.as_os_str().is_empty() {
        None
    } else {
        let from_cargo = fs::read_to_string(src.join("Cargo.toml"))
            .ok()
            .and_then(|text| toml_version(&text));
        let from_cmake = fs::read_to_string(src.join("CMakeLists.txt"))
            .ok()
            .and_then(|text| cmake_version(&text));
        let from_python = fs::read_to_string(src.join("pyproject.toml"))
            .ok()
            .and_then(|text| toml_version(&text));
        from_cargo
            .or(from_cmake)
            .or(from_python)
            .or_else(|| git_version(src))
    }
    .map(|raw| version_slug(&raw))
    .unwrap_or_else(|| "0.1.0".to_string());
    ProjectIdentity { name, version }
}

/// Paketinstallationsschritt mit dem echten Projektnamen.
fn project_install_step(system: &BuildSystem, name: &str) -> String {
    let step = match system {
        BuildSystem::Cargo => crate::core::packaging::cargo_install_step().to_string(),
        BuildSystem::CMake => {
            "cmake --build build -j$(nproc) && [ -f build/cmake_install.cmake ] && DESTDIR=\"$pkgdir\" cmake --install build"
                .to_string()
        }
        BuildSystem::Meson => "DESTDIR=\"$pkgdir\" ninja -C build install".to_string(),
        BuildSystem::Go => format!("install -Dm755 app \"$pkgdir/usr/bin/{}\"", name),
        BuildSystem::Python => {
            "python3 -m pip install --no-deps --prefix=\"$pkgdir/usr\" .".to_string()
        }
        BuildSystem::Make => "make DESTDIR=\"$pkgdir\" PREFIX=/usr install".to_string(),
        BuildSystem::Qmake => "make INSTALL_ROOT=\"$pkgdir\" install".to_string(),
        BuildSystem::Unknown => "true".to_string(),
    };
    format!(
        "mkdir -p \"$pkgdir/usr/bin\" \"$pkgdir/usr/lib\" \"$pkgdir/usr/share\"\n{}\n{}",
        step,
        crate::core::packaging::strip_binaries_step()
    )
}

fn binary_build_step(system: &BuildSystem) -> &'static str {
    match system {
        BuildSystem::Cargo => crate::core::packaging::cargo_build_step(),
        BuildSystem::CMake => {
            "cmake -S . -B build -DCMAKE_BUILD_TYPE=Release -DCMAKE_PREFIX_PATH=/work/deps-install -DCMAKE_POLICY_VERSION_MINIMUM=3.5 -DCMAKE_SUPPRESS_DEVELOPER_ERRORS=ON && cmake --build build -j$(nproc) && find build -maxdepth 3 -type f -perm -u+x -exec cp {} /work/dist/ \\;"
        }
        BuildSystem::Meson => {
            "meson setup build --prefix=/usr && ninja -C build && find build -maxdepth 3 -type f -perm -u+x -exec cp {} /work/dist/ \\;"
        }
        BuildSystem::Go => "go build -o /work/dist/app .",
        BuildSystem::Python => "python3 -m pip wheel --no-deps -w /work/dist .",
        BuildSystem::Make => "make && find . -maxdepth 2 -type f -perm -u+x -exec cp {} /work/dist/ \\;",
        BuildSystem::Qmake => {
            "(qmake6 . 2>/dev/null || qmake .) && make && find . -maxdepth 2 -type f -perm -u+x -exec cp {} /work/dist/ \\;"
        }
        BuildSystem::Unknown => "true",
    }
}

fn binary_build_step_with_flags(system: &BuildSystem, flags: &[String]) -> String {
    if *system == BuildSystem::CMake {
        return format!(
            "cmake -S . -B build {} -DCMAKE_PREFIX_PATH=/work/deps-install && cmake --build build -j$(nproc) && find build -maxdepth 3 -type f -perm -u+x -exec cp {{}} /work/dist/ \\;",
            cmake_configure_flags(flags)
        );
    }
    binary_build_step(system).to_string()
}

/// Logtexte niemals als nackte Shell-Zeilen einbetten: Klammern wie in
/// `== Setup (pacman) ==` würden von `sh` als Syntaxfehler interpretiert.
fn echo_line(text: &str) -> String {
    format!("echo '{}'", text.replace('\'', "'\\''"))
}

/// Paketnamen aus Suchergebnissen bereinigen: `foo (bar)` -> `foo`.
fn sanitize_package(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with('-') {
        return String::new();
    }
    let head = trimmed.split('(').next().unwrap_or(trimmed).trim();
    let head = head.rsplit('/').next().unwrap_or(head);
    let head = head.split(':').next().unwrap_or(head).trim();
    let cleaned: String = head
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || "-_+.".contains(*c))
        .collect();
    if valid_package_name(&cleaned) {
        cleaned
    } else {
        String::new()
    }
}

#[allow(dead_code)]
fn build_script(config: &BuildConfig, system: &BuildSystem, extra: &[String]) -> String {
    let identity = project_identity(&config.url, Path::new(""));
    build_script_with(
        config,
        system,
        extra,
        config.repo_script.as_deref(),
        &identity,
        config.strategy,
        false,
    )
}

/// Build-Schritt unter Berücksichtigung der ausgewählten Strategie.
///
/// Generisches Nachinstallieren von Desktop-Dateien und deren Icons.
/// Wirkt für jedes Build-System und jede Distribution - ohne Projektname.
const ASSET_STAGING_SCRIPT: &str = r#"
mkdir -p "$pkgdir/usr/share/applications" \
  "$pkgdir/usr/share/icons/hicolor/scalable/apps" \
  "$pkgdir/usr/share/icons/hicolor/256x256/apps" \
  "$pkgdir/usr/share/icons/hicolor/128x128/apps" \
  "$pkgdir/usr/share/pixmaps"
find_repo_assets() {
  find . -maxdepth 4 -type f \
    \( -iname '*.svg' -o -iname '*.png' -o -iname '*.ico' -o -iname '*.xpm' \
       -o -iname '*.desktop' -o -iname '*.desktop.in' \) \
    ! -path './.git/*' ! -path '*/.git/*' \
    ! -path './target/*' ! -path '*/target/*' \
    ! -path './cargo-target/*' ! -path '*/cargo-target/*' \
    ! -path './build/*' ! -path '*/build/*' \
    ! -path './deps-build/*' ! -path '*/deps-build/*' \
    ! -path './out/*' ! -path '*/out/*' \
    ! -path './dist/*' ! -path '*/dist/*' \
    ! -path './pkg/*' ! -path '*/pkg/*' \
    ! -path './node_modules/*' ! -path '*/node_modules/*' \
    ! -path './tests/*' ! -path '*/tests/*' \
    ! -path './docs/*' ! -path '*/docs/*' \
    ! -path './ui/*' ! -path '*/ui/*' \
    -print0 2>/dev/null
}
png_dimensions() {
  local f="$1" size=""
  if command -v identify >/dev/null 2>&1; then
    size=$(identify -format '%wx%h' "$f" 2>/dev/null)
  fi
  if [ -z "$size" ] && command -v file >/dev/null 2>&1; then
    size=$(file -b "$f" 2>/dev/null | sed -nE 's/.*, ([0-9]+) x ([0-9]+).*/\1x\2/p')
  fi
  if [ -z "$size" ]; then
    local w h
    w=$(dd if="$f" bs=1 skip=16 count=4 2>/dev/null | od -An -tu1 | awk '{print $1*16777216+$2*65536+$3*256+$4}')
    h=$(dd if="$f" bs=1 skip=20 count=4 2>/dev/null | od -An -tu1 | awk '{print $1*16777216+$2*65536+$3*256+$4}')
    if [ "${w:-0}" -gt 0 ] 2>/dev/null && [ "${h:-0}" -gt 0 ] 2>/dev/null; then
      size="${w}x${h}"
    fi
  fi
  printf '%s' "$size"
}
install_icons() {
  local expected="$1" fallback="$2"
  local expected_lc fallback_lc
  expected_lc=$(printf '%s' "$expected" | tr '[:upper:]' '[:lower:]')
  fallback_lc=$(printf '%s' "$fallback" | tr '[:upper:]' '[:lower:]')
  local ext best best_score score base base_lc dir special dims icon_w icon_h
  for ext in svg png ico xpm; do
    best=""
    best_score=99
    while IFS= read -r -d '' candidate; do
      case "$candidate" in *."$ext") ;; *) continue ;; esac
      base=$(basename "$candidate")
      base_lc=$(printf '%s' "$base" | tr '[:upper:]' '[:lower:]')
      case "$base_lc" in
        screenshot*|*screenshot*|banner*|*banner*|preview*|*preview*|bg*|background*|*background*|wallpaper*) continue ;;
      esac
      if [ "$ext" = "png" ]; then
        dims=$(png_dimensions "$candidate")
        case "$dims" in
          *x*)
            icon_w=${dims%x*}
            icon_h=${dims#*x}
            [ "$icon_w" = "$icon_h" ] || continue
            ;;
        esac
      fi
      score=9
      case "$base_lc" in
        "$expected_lc".*) score=0 ;;
        app_icon.*|icon.*) score=1 ;;
        "$fallback_lc".*) score=2 ;;
        app.*|logo.*) score=3 ;;
        *icon*|*logo*) score=4 ;;
        *)
          dir=$(dirname "$candidate" | tr '[:upper:]' '[:lower:]')
          for special in icons assets data resources pixmaps; do
            case "$dir" in
              */$special|*/$special/*|./$special|./$special/*) score=5 ;;
            esac
          done
          ;;
      esac
      if [ "$score" -lt "$best_score" ]; then
        best="$candidate"
        best_score="$score"
      fi
    done < <(find_repo_assets)
    [ -z "$best" ] && continue
    case "$ext" in
      svg)
        install -Dm644 "$best" "$pkgdir/usr/share/icons/hicolor/scalable/apps/$expected.svg" 2>/dev/null || true
        ;;
      png)
        dims=$(png_dimensions "$best")
        case "$dims" in
          16x16|22x22|24x24|32x32|48x48|64x64|72x72|96x96|128x128|256x256|512x512)
            install -Dm644 "$best" "$pkgdir/usr/share/icons/hicolor/$dims/apps/$expected.png" 2>/dev/null || true
            ;;
          *)
            install -Dm644 "$best" "$pkgdir/usr/share/icons/hicolor/256x256/apps/$expected.png" 2>/dev/null || true
            install -Dm644 "$best" "$pkgdir/usr/share/icons/hicolor/128x128/apps/$expected.png" 2>/dev/null || true
            ;;
        esac
        install -Dm644 "$best" "$pkgdir/usr/share/pixmaps/$expected.png" 2>/dev/null || true
        ;;
      ico)
        install -Dm644 "$best" "$pkgdir/usr/share/pixmaps/$expected.ico" 2>/dev/null || true
        ;;
      xpm)
        install -Dm644 "$best" "$pkgdir/usr/share/pixmaps/$expected.xpm" 2>/dev/null || true
        ;;
    esac
  done
}
desktop_count=0
while IFS= read -r -d '' desktop; do
  base=$(basename "$desktop")
  case "$base" in
    *.desktop.in) name="${base%.desktop.in}" ;;
    *) name="${base%.desktop}" ;;
  esac
  desktop_count=$((desktop_count + 1))
  target="$pkgdir/usr/share/applications/$name.desktop"
  [ -e "$target" ] && continue
  case "$base" in
    *.desktop.in)
      sed -e "s|@EXEC@|$name|g" -e "s|@ICON@|$name|g" "$desktop" > "$target" || true
      ;;
    *)
      install -Dm644 "$desktop" "$target"
      ;;
  esac
  chmod 0644 "$target" 2>/dev/null || true
  icon=$(grep -m1 '^Icon=' "$target" 2>/dev/null | cut -d= -f2- | tr -d '\r')
  [ -z "$icon" ] && icon="$name"
  icon=$(printf '%s' "$icon" | sed "s|@ICON@|$name|g" | xargs basename 2>/dev/null || printf '%s' "$icon")
  [ -z "$icon" ] && icon="$name"
  install_icons "$icon" "$name"
done < <(find_repo_assets | while IFS= read -r -d '' f; do case "$f" in *.desktop|*.desktop.in) printf '%s\0' "$f" ;; esac; done)
if [ "$desktop_count" -eq 0 ]; then
  target="$pkgdir/usr/share/applications/$PKG_NAME.desktop"
  cat > "$target" <<DESKTOP_EOF
[Desktop Entry]
Type=Application
Name=$PKG_NAME
Exec=$PKG_NAME
Icon=$PKG_NAME
Categories=Utility;
DESKTOP_EOF
  chmod 0644 "$target" 2>/dev/null || true
  install_icons "$PKG_NAME" "$PKG_NAME"
fi
"#;

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\\''"))
}

fn asset_staging_step(pkg_name: &str) -> String {
    format!(
        "PKG_NAME={}\n{}",
        shell_quote(pkg_name),
        ASSET_STAGING_SCRIPT
    )
}

#[cfg(test)]
fn build_step_text(
    system: &BuildSystem,
    strategy: BuildStrategy,
    repo_script: Option<&str>,
) -> String {
    build_step_text_with_flags(system, strategy, repo_script, &[])
}

fn build_step_text_with_flags(
    system: &BuildSystem,
    strategy: BuildStrategy,
    repo_script: Option<&str>,
    flags: &[String],
) -> String {
    // Direkter Fallback ignoriert Repo-Skripte und Generator-Sonderwege.
    if strategy == BuildStrategy::DirectStagingFallback {
        if *system == BuildSystem::CMake {
            return format!(
                r#"cmake -S . -B build -G "Unix Makefiles" {} -DCMAKE_INSTALL_PREFIX=/usr -DCMAKE_PREFIX_PATH=/work/deps-install && cmake --build build -j$(nproc) && find build -maxdepth 3 -type f -perm -u+x -exec cp {{}} /work/dist/ \;"#,
                cmake_configure_flags(flags)
            );
        }
        return binary_build_step(system).to_string();
    }
    let repo_script = if strategy == BuildStrategy::RepoScript {
        repo_script
    } else {
        None
    };
    if let Some(script) = repo_script {
        return format!(
            r#"script="./{name}"
probe=$(bash "$script" --help 2>&1 || true)
flags=""
if printf '%s
' "$probe" | grep -qiE 'usage|and then'; then
  flags=$(printf '%s
' "$probe" \
    | grep -Ei 'usage|and then|--[a-z]|release|build' \
    | grep -oE -- '--?[A-Za-z0-9][A-Za-z0-9-]*' \
    | sort -u \
    | grep -viE '^(-h|--help|-i|--interactive|--appimage|-u|--update|--upgrade|--gui)$' \
    | tr '\n' ' ' || true)
fi
if [ -n "$flags" ]; then
  bash "$script" $flags
else
  bash "$script"
fi
find . -maxdepth 3 -type f -perm -u+x -exec cp {{}} /work/dist/ \;"#,
            name = script
        );
    }
    binary_build_step_with_flags(system, flags)
}

fn build_script_with(
    config: &BuildConfig,
    system: &BuildSystem,
    extra: &[String],
    repo_script: Option<&str>,
    identity: &ProjectIdentity,
    strategy: BuildStrategy,
    has_pkgbuild: bool,
) -> String {
    build_script_with_flags(
        config,
        system,
        extra,
        repo_script,
        identity,
        strategy,
        has_pkgbuild,
        &[],
    )
}

#[allow(clippy::too_many_arguments)]
fn build_script_with_flags(
    config: &BuildConfig,
    system: &BuildSystem,
    extra: &[String],
    repo_script: Option<&str>,
    identity: &ProjectIdentity,
    strategy: BuildStrategy,
    has_pkgbuild: bool,
    cmake_flags: &[String],
) -> String {
    let texts = Texts::new(config.language);
    let target = config.target_distro.info(&config.distro);
    let extra: Vec<String> = extra
        .iter()
        .map(|p| sanitize_package(p))
        .filter(|p| !p.is_empty())
        .collect();
    let mut lines: Vec<String> = vec![
        "#!/usr/bin/env bash".to_string(),
        "set -e".to_string(),
        "set -o pipefail".to_string(),
        "export CMAKE_POLICY_VERSION_MINIMUM=3.5".to_string(),
        "export CARGO_TARGET_DIR=/work/cargo-target".to_string(),
        "export CARGO_HOME=/work/cargo-home".to_string(),
        "export CCACHE_DIR=/work/ccache".to_string(),
        "mkdir -p /work/cargo-target /work/cargo-home /work/ccache".to_string(),
        echo_line(&texts.log_setup(&target.package_manager)),
        base_dependencies(&target.family, system),
    ];
    if config.cmake_compat {
        lines.push("export CMAKE_SUPPRESS_DEVELOPER_ERRORS=ON".to_string());
    }
    if config.cstdint_fix {
        lines.push("export CXXFLAGS=\"${CXXFLAGS:+$CXXFLAGS }-include cstdint\"".to_string());
        lines.push(
            "export CMAKE_CXX_FLAGS=\"${CMAKE_CXX_FLAGS:+$CMAKE_CXX_FLAGS }-include cstdint\""
                .to_string(),
        );
    }
    if config.submodule_fix {
        lines.push("git -C /work/src submodule update --init --recursive || true".to_string());
    }

    let extra_cmd = install_packages_command(&target.family, &extra);
    if !extra_cmd.is_empty() {
        lines.push(echo_line(&texts.log_extra(&extra.join(" "))));
        lines.push(extra_cmd);
    }
    if config.rustup_fix {
        lines.extend(rustup_bootstrap_step(&target.family));
    }

    lines.push("mkdir -p /work/out /work/dist".to_string());

    if has_pkgbuild
        && !matches!(
            (&config.target, &target.family),
            (TargetFormat::NativePackage, DistroFamily::Arch)
        )
    {
        lines.push(echo_line(
            "[pkgbuild] Arch-PKGBUILD wird fuer dieses Zielformat ignoriert - universelles Build-System wird verwendet",
        ));
    }

    let build_step = build_step_text_with_flags(system, strategy, repo_script, cmake_flags);
    let install_step = project_install_step(system, &identity.name);
    let asset_step = asset_staging_step(&identity.name);
    let pkg_ctx = crate::core::packaging::PackageContext {
        name: &identity.name,
        version: &identity.version,
        family: &target.family,
        strategy,
        has_pkgbuild,
        arch: crate::core::packaging::PackageArch::detect(),
        build_step: &build_step,
        install_step: &install_step,
        is_cargo: system == &BuildSystem::Cargo,
        asset_step: &asset_step,
    };

    match &config.target {
        TargetFormat::StandaloneBinary => {
            lines.push(echo_line(texts.log_binary_build()));
            lines.push(format!("cd /work/src && {}", build_step));
        }
        TargetFormat::NativePackage => {
            if target.family == DistroFamily::Unknown {
                lines.push(format!("cd /work/src && {}", build_step));
            } else {
                lines.extend(crate::core::packaging::generate(&pkg_ctx));
            }
        }
    }

    // Erfolg nur melden, wenn wirklich ein Artefakt im Zielordner liegt.
    let artifact_glob = match &config.target {
        TargetFormat::NativePackage => {
            "artifacts=(/work/dist/*.pkg.tar.zst /work/dist/*.deb /work/dist/*.rpm)"
        }
        TargetFormat::StandaloneBinary => "artifacts=(/work/dist/*)",
    };
    lines.push(artifact_glob.to_string());
    lines.push(
        "if [ ! -f \"${artifacts[0]}\" ]; then echo '__GIT2PKG_NO_ARTIFACT__' >&2; exit 1; fi"
            .to_string(),
    );
    // Ein bloß vorhandenes Paket ist kein Erfolg: nach einem CMake-/Compiler-
    // Fehler darf kein leeres Artefakt den OK-Marker ausloesen.
    lines.push(
        r#"artifact="${artifacts[0]}"
case "$artifact" in
  *.pkg.tar.zst|*.tar.zst)
    if ! tar -tf "$artifact" 2>/dev/null | grep -Eq '(^|/)usr/(bin|lib|lib64)/'; then
      echo '__GIT2PKG_EMPTY_PACKAGE__' >&2
      exit 1
    fi
    ;;
  *.deb)
    if ! dpkg-deb -c "$artifact" 2>/dev/null | grep -Eq 'usr/(bin|lib|lib64)/'; then
      echo '__GIT2PKG_EMPTY_PACKAGE__' >&2
      exit 1
    fi
    ;;
  *.rpm)
    if ! rpm -qlp "$artifact" 2>/dev/null | grep -Eq 'usr/(bin|lib|lib64)/'; then
      echo '__GIT2PKG_EMPTY_PACKAGE__' >&2
      exit 1
    fi
    ;;
esac"#
            .to_string(),
    );
    // Root-eigene Bauordner muessen im Container entfernt werden; /work/dist
    // bleibt bis zum Kopieren der Artefakte durch den Host erhalten.
    lines.push(
        "rm -rf /work/cargo-target /work/cargo-home /work/ccache /work/build /work/deps-build /work/out /work/pkg 2>/dev/null || true"
            .to_string(),
    );
    // Der Host muss den gemounteten Ordner danach restlos loeschen koennen.
    lines.push("chown -R --reference=/work /work 2>/dev/null || true".to_string());
    lines.push("chmod -R u+rwX /work 2>/dev/null || true".to_string());
    lines.push(format!("echo '{}'", OK_MARKER));
    lines.join("\n")
}

/// Stabiler Cache-Key aus der Repository-URL (FNV-1a), damit ein erneuter
/// Build desselben Repos den vorhandenen Quellcode wiederfindet.
fn source_cache_key(url: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in url.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", hash)
}

/// Prüft im erhaltenen Quellcode-Cache, ob lokale deps/ vorhanden sind.
/// Die GUI nutzt das, um Stufe 2 bei Bedarf zu überspringen.
pub fn cached_repo_has_local_dependencies(url: &str) -> bool {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let src = Path::new(&home)
        .join(".cache/git2pkg/builds")
        .join(source_cache_key(url))
        .join("src");
    has_local_dependencies(&src)
}

/// Arbeitsverzeichnis im Cache - bei Fehlschlag bleibt der Quellcode erhalten.
fn temp_workdir(url: &str) -> Result<PathBuf, String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let dir = Path::new(&home)
        .join(".cache/git2pkg/builds")
        .join(source_cache_key(url));
    fs::create_dir_all(dir.join("out")).map_err(|e| e.to_string())?;
    fs::create_dir_all(dir.join("dist")).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn find_repo_script(src: &Path) -> Option<String> {
    for name in [
        "BuildLinux.sh",
        "build.sh",
        "build_linux.sh",
        "build-linux.sh",
        "compile.sh",
    ] {
        if src.join(name).is_file() {
            return Some(name.to_string());
        }
    }
    None
}

fn install_command(config: &BuildConfig, artifacts: &[PathBuf]) -> String {
    let file = artifacts
        .first()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "<artifact>".to_string());
    match (&config.target, &config.distro.family) {
        (TargetFormat::StandaloneBinary, _) => format!("install -m755 {} ~/.local/bin/", file),
        (_, DistroFamily::Arch) | (_, DistroFamily::Unknown) => {
            format!("sudo pacman -U {}", file)
        }
        (_, DistroFamily::Debian) => format!("sudo apt install {}", file),
        (_, DistroFamily::Fedora) => format!("sudo dnf install {}", file),
        (_, DistroFamily::Suse) => format!("sudo zypper install {}", file),
    }
}

#[allow(unused_imports)]
pub use crate::core::packaging::verify::{
    archive_listing_command, listing_payload, verify_built_packages,
};

fn package_result(
    config: &BuildConfig,
    work: &Path,
    identity: &ProjectIdentity,
    output: &str,
    tx: &Sender<BuildEvent>,
) -> Result<BuildOutcome, String> {
    let texts = Texts::new(config.language);
    step(tx, texts.step_copy_artifacts());
    let mut artifacts: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = fs::read_dir(work.join("dist")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(name) = path.file_name() {
                    let file_name = name.to_string_lossy();
                    let is_debug = file_name.contains("-debug-") || file_name.contains("-dbgsym");
                    let is_artifact = match &config.target {
                        TargetFormat::NativePackage => {
                            file_name.ends_with(".pkg.tar.zst")
                                || file_name.ends_with(".deb")
                                || file_name.ends_with(".rpm")
                        }
                        TargetFormat::StandaloneBinary => true,
                    };
                    if is_debug || !is_artifact {
                        continue;
                    }
                    let mut final_name = name.to_string_lossy().into_owned();
                    if config.target == TargetFormat::StandaloneBinary
                        && (final_name == "app" || final_name == "git2pkg-build")
                    {
                        final_name = identity.name.clone();
                    }
                    let target = config.output_dir.join(&final_name);
                    if fs::copy(&path, &target).is_ok() {
                        artifacts.push(target);
                    }
                }
            }
        }
    }
    if artifacts.is_empty() {
        return Err(texts.err_no_artifacts().to_string());
    }

    let recipe = fs::read_to_string(work.join("build.sh"))
        .unwrap_or_else(|_| "build.sh im Cache".to_string());
    let cmd = install_command(config, &artifacts);
    // Log und Rezept bleiben intern im Cache; der Ausgabeordner enthält nur Artefakte.
    let _ = fs::write(work.join("BUILD_LOG.txt"), output);

    Ok(BuildOutcome {
        artifacts,
        recipe,
        install_command: cmd,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(family: DistroFamily, target: TargetFormat) -> BuildConfig {
        let distro = DistroInfo::from_os_release(match family {
            DistroFamily::Debian => "ID=ubuntu\nID_LIKE=debian\n",
            DistroFamily::Fedora => "ID=fedora\n",
            DistroFamily::Suse => "ID=opensuse-leap\nID_LIKE=\"suse opensuse\"\n",
            _ => "ID=arch\n",
        });
        BuildConfig {
            url: "example-app".to_string(),
            target,
            target_distro: TargetDistro::Auto,
            distro,
            engine: ContainerEngine::Docker,
            output_dir: PathBuf::from("dist"),
            language: Language::En,
            strategy: BuildStrategy::Default,
            repo_script: None,
            cmake_compat: false,
            cstdint_fix: false,
            submodule_fix: false,
            rustup_fix: false,
            skip_deps: false,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            container_id: Arc::new(Mutex::new(None)),
        }
    }

    #[test]
    fn script_bereinigt_paketnamen_mit_klammern() {
        let script = build_script(
            &config(DistroFamily::Arch, TargetFormat::StandaloneBinary),
            &BuildSystem::CMake,
            &["foo (bar)".to_string()],
        );
        assert!(script.contains("pacman -S --noconfirm --needed -- foo"));
        assert!(!script.contains("--needed foo (bar)"));
    }

    #[test]
    fn script_startet_mit_bash_shebang() {
        let script = build_script(
            &config(DistroFamily::Arch, TargetFormat::NativePackage),
            &BuildSystem::Cargo,
            &[],
        );
        assert!(script.starts_with("#!/usr/bin/env bash\n"));
        assert!(!script.contains("#!/bin/sh"));
        // Logzeilen müssen als echo-Befehle eingebettet sein, nicht als nackter Text.
        assert!(script.contains("echo '== git2pkg: Setup (pacman) =='"));
        assert!(script.lines().all(|line| !line.starts_with("==")));
    }

    #[test]
    fn desktop_dateien_und_icons_werden_generisch_gestaget() {
        let identity = ProjectIdentity {
            name: "repo".to_string(),
            version: "1.0.0".to_string(),
        };
        for family in [
            DistroFamily::Arch,
            DistroFamily::Debian,
            DistroFamily::Fedora,
        ] {
            let cfg = config(family.clone(), TargetFormat::NativePackage);
            let script = build_script_with(
                &cfg,
                &BuildSystem::CMake,
                &[],
                None,
                &identity,
                BuildStrategy::Default,
                false,
            );
            assert!(script.contains("*.desktop"), "{:?}", family);
            assert!(script.contains("usr/share/applications"), "{:?}", family);
            assert!(
                script.contains("usr/share/icons/hicolor/scalable/apps"),
                "{:?}",
                family
            );
            assert!(script.contains("usr/share/pixmaps"), "{:?}", family);
            assert!(script.contains("Icon="), "{:?}", family);
        }
    }

    #[test]
    fn version_wird_fuer_alle_paketformate_saniert() {
        let cases = [
            ("v1.2.3", "1.2.3"),
            ("  V1.2.3  ", "1.2.3"),
            ("v1.0.0-13", "1.0.0.13"),
            ("2.1.0-rc1", "2.1.0.rc1"),
            ("1.0.0-13-gabcdef", "1.0.0.13.gabcdef"),
            ("2.0-beta/1", "2.0.beta.1"),
            ("1.0.0+git", "1.0.0.git"),
            ("1..2..3", "1.2.3"),
            (".1.2.", "1.2"),
            ("release: 3.1", "0.1.0"),
            ("abc123", "0.1.0"),
            ("", "0.1.0"),
        ];
        for (input, expected) in cases {
            assert_eq!(version_slug(input), expected, "Version: {:?}", input);
        }
    }

    #[test]
    fn asset_staging_findet_desktop_dateien_in_unterordnern() {
        let script = asset_staging_step("example-app");
        assert!(script.contains("PKG_NAME='example-app'"));
        assert!(!script.contains(":-package"));
        assert!(script.contains("-maxdepth 4"));
        assert!(script.contains("*.desktop.in"));
        assert!(script.contains("@EXEC@"));
        assert!(script.contains("@ICON@"));
        assert!(script.contains("usr/share/applications"));
        assert!(script.contains("usr/share/icons/hicolor/scalable/apps"));
        assert!(script.contains("usr/share/pixmaps"));
        assert!(script.contains("cargo-target"));
        assert!(script.contains("! -path '*/.git/*'"));
        assert!(script.contains("! -path '*/node_modules/*'"));
        assert!(script.contains("! -path '*/tests/*'"));
        assert!(script.contains("! -path '*/ui/*'"));
        assert!(script.contains("! -path '*/docs/*'"));
        assert!(script.contains("app_icon.*|icon.*"));
        assert!(script.contains("*icon*|*logo*"));
        let path = std::env::temp_dir().join(format!("git2pkg-assets-{}.sh", std::process::id()));
        std::fs::write(&path, script).unwrap();
        let ok = std::process::Command::new("bash")
            .arg("-n")
            .arg(&path)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        let _ = std::fs::remove_file(&path);
        assert!(ok, "ungueltiges Asset-Staging-Skript");
    }

    #[test]
    fn rustup_healing_setzt_toolchain_vor_usr_bin() {
        let identity = ProjectIdentity {
            name: "sample-tool".to_string(),
            version: "1.0.0".to_string(),
        };
        for family in [
            DistroFamily::Debian,
            DistroFamily::Fedora,
            DistroFamily::Suse,
            DistroFamily::Arch,
        ] {
            let mut cfg = config(family, TargetFormat::NativePackage);
            cfg.rustup_fix = true;
            let script = build_script_with(
                &cfg,
                &BuildSystem::Cargo,
                &[],
                None,
                &identity,
                BuildStrategy::Default,
                false,
            );
            assert!(script.contains(
                "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal"
            ));
            assert!(script.contains("export PATH=\"$HOME/.cargo/bin:$PATH\""));
            assert!(script.contains("\"$HOME/.cargo/bin/cargo\" --version"));
        }
    }

    #[test]
    fn asset_staging_installt_icon_unter_erwartetem_namen() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "git2pkg-icon-stage-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let repo = root.join("repo");
        let out = root.join("out");
        fs::create_dir_all(repo.join("packaging")).unwrap();
        fs::create_dir_all(repo.join("assets")).unwrap();
        fs::write(
            repo.join("packaging/sample-tool.desktop"),
            "[Desktop Entry]\nName=Sample Tool\nExec=sample-tool\nIcon=sample-tool\nType=Application\n",
        )
        .unwrap();

        let png_bytes = |width: u32, height: u32| {
            let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
            png.extend_from_slice(&13u32.to_be_bytes());
            png.extend_from_slice(b"IHDR");
            png.extend_from_slice(&width.to_be_bytes());
            png.extend_from_slice(&height.to_be_bytes());
            png.extend_from_slice(&[8, 2, 0, 0, 0, 0, 0, 0, 0]);
            png
        };
        fs::write(repo.join("assets/app_icon.png"), png_bytes(16, 16)).unwrap();
        // Nicht-quadratischer Kandidat mit exakt erwartetem Namen muss ignoriert werden.
        fs::write(repo.join("assets/sample-tool.png"), png_bytes(300, 150)).unwrap();
        // UI-Grafiken unter ui/ dürfen nie als Programm-Icon-Kandidaten.
        fs::create_dir_all(repo.join("assets/ui")).unwrap();
        fs::write(repo.join("assets/ui/icon.png"), png_bytes(64, 64)).unwrap();

        let command = format!(
            "pkgdir='{}'; cd '{}'; {}",
            out.display(),
            repo.display(),
            asset_staging_step("example-app")
        );
        let status = std::process::Command::new("bash")
            .arg("-c")
            .arg(&command)
            .status()
            .unwrap();
        assert!(status.success(), "Asset-Staging-Skript fehlgeschlagen");

        let desktop = out.join("usr/share/applications/sample-tool.desktop");
        assert!(desktop.is_file(), "Desktop-Datei nicht gestaged");
        let icon = out.join("usr/share/icons/hicolor/16x16/apps/sample-tool.png");
        assert!(
            icon.is_file(),
            "Icon wurde nicht unter dem erwarteten Namen installiert"
        );
        assert!(
            !out.join("usr/share/icons/hicolor/64x64/apps/sample-tool.png")
                .exists(),
            "UI-Icon aus ui/ wurde faelschlich als Programm-Icon installiert"
        );
        assert!(
            out.join("usr/share/pixmaps/sample-tool.png").is_file(),
            "Icon fehlt in /usr/share/pixmaps"
        );
        assert!(fs::read_to_string(&desktop)
            .unwrap()
            .contains("Icon=sample-tool"));
        assert_eq!(
            fs::metadata(&desktop).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(
            fs::metadata(&icon).unwrap().permissions().mode() & 0o777,
            0o644
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn asset_staging_erzeugt_fallback_desktop_mit_pkg_name() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "git2pkg-fallback-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let repo = root.join("repo");
        let out = root.join("out");
        fs::create_dir_all(repo.join("assets")).unwrap();

        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&48u32.to_be_bytes());
        png.extend_from_slice(&48u32.to_be_bytes());
        png.extend_from_slice(&[8, 2, 0, 0, 0, 0, 0, 0, 0]);
        fs::write(repo.join("assets/app_icon.png"), png).unwrap();

        let command = format!(
            "pkgdir='{}'; cd '{}'; {}",
            out.display(),
            repo.display(),
            asset_staging_step("sample-tool")
        );
        let status = std::process::Command::new("bash")
            .arg("-c")
            .arg(&command)
            .status()
            .unwrap();
        assert!(status.success());

        let desktop = out.join("usr/share/applications/sample-tool.desktop");
        let icon = out.join("usr/share/icons/hicolor/48x48/apps/sample-tool.png");
        assert!(desktop.is_file(), "Fallback-Desktop fehlt");
        assert!(
            icon.is_file(),
            "Icon wurde nicht unter sample-tool.png installiert"
        );
        let desktop_text = fs::read_to_string(&desktop).unwrap();
        assert!(desktop_text.contains("Name=sample-tool"));
        assert!(desktop_text.contains("Exec=sample-tool"));
        assert!(desktop_text.contains("Icon=sample-tool"));
        assert_eq!(
            fs::metadata(&desktop).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(
            fs::metadata(&icon).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert!(
            out.join("usr/share/pixmaps/sample-tool.png").is_file(),
            "Fallback-Icon fehlt in /usr/share/pixmaps"
        );

        assert!(
            !out.join("usr/share/pixmaps/package.png").exists(),
            "Fallback-Name package.png darf nicht entstehen"
        );
        assert_eq!(
            fs::read_dir(out.join("usr/share/applications"))
                .unwrap()
                .count(),
            1,
            "usr/share/applications darf nicht leer sein"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn qmake_kette_bricht_bei_konfigurationsfehler_ab() {
        let step = binary_build_step(&BuildSystem::Qmake);
        assert!(step.contains("&& make"), "{step}");
        assert!(step.contains("&& find"), "{step}");
    }

    #[test]
    fn alle_buildsysteme_und_strategien_erzeugen_gueltige_skripte() {
        let config = config(DistroFamily::Arch, TargetFormat::NativePackage);
        let identity = project_identity("example-app", Path::new(""));
        let systems = [
            BuildSystem::Cargo,
            BuildSystem::CMake,
            BuildSystem::Meson,
            BuildSystem::Qmake,
            BuildSystem::Go,
            BuildSystem::Python,
            BuildSystem::Make,
            BuildSystem::Unknown,
        ];
        let strategies = [
            BuildStrategy::Default,
            BuildStrategy::SystemLibsOnly,
            BuildStrategy::RepoScript,
            BuildStrategy::DirectStagingFallback,
        ];
        let path =
            std::env::temp_dir().join(format!("git2pkg-buildmatrix-{}.sh", std::process::id()));
        for system in systems {
            for strategy in strategies {
                let script =
                    build_script_with(&config, &system, &[], None, &identity, strategy, false);
                std::fs::write(&path, script).unwrap();
                let ok = std::process::Command::new("bash")
                    .arg("-n")
                    .arg(&path)
                    .status()
                    .map(|status| status.success())
                    .unwrap_or(false);
                assert!(ok, "ungueltiges Buildskript: {:?} / {:?}", system, strategy);
            }
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn projektidentitaet_aus_url_und_metadaten() {
        let root = std::env::temp_dir().join(format!(
            "g2p-ident-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();

        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"foo\"\nversion = \"2.3.4\"\n",
        )
        .unwrap();
        let cargo = project_identity("example-cargo", &root);
        assert_eq!(cargo.name, "example-cargo");
        assert_eq!(cargo.version, "2.3.4");

        fs::remove_file(root.join("Cargo.toml")).unwrap();
        fs::write(
            root.join("CMakeLists.txt"),
            "project(sample VERSION 2.20.0 LANGUAGES C)\n",
        )
        .unwrap();
        let cmake = project_identity("example-cmake", &root);
        assert_eq!(cmake.name, "example-cmake");
        assert_eq!(cmake.version, "2.20.0");

        fs::remove_file(root.join("CMakeLists.txt")).unwrap();
        fs::write(
            root.join("pyproject.toml"),
            "[project]\nversion = \"1.2.3\"\n",
        )
        .unwrap();
        let python = project_identity("example-python", &root);
        assert_eq!(python.name, "example-python");
        assert_eq!(python.version, "1.2.3");

        let fallback = project_identity("example-app", Path::new(""));
        assert_eq!(fallback.name, "example-app");
        assert_eq!(fallback.version, "0.1.0");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ausgabeordner_enthlt_nur_das_artefakt() {
        let base = std::env::temp_dir().join(format!(
            "g2p-out-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let work = base.join("work");
        fs::create_dir_all(work.join("dist")).unwrap();
        fs::write(work.join("dist/repo-0.1.0-1-x86_64.pkg.tar.zst"), "inhalt").unwrap();
        fs::write(
            work.join("dist/repo-0.1.0-1-x86_64-debug-1-x86_64.pkg.tar.zst"),
            "debug",
        )
        .unwrap();
        fs::write(work.join("dist/repo-dbgsym_0.1.0_amd64.deb"), "dbgsym").unwrap();
        let output = base.join("out");
        fs::create_dir_all(&output).unwrap();

        let mut config = config(DistroFamily::Arch, TargetFormat::NativePackage);
        config.output_dir = output.clone();
        let identity = ProjectIdentity {
            name: "repo".to_string(),
            version: "0.1.0".to_string(),
        };
        let (tx, _rx) = std::sync::mpsc::channel();
        let outcome = package_result(&config, &work, &identity, "log-zeile", &tx).unwrap();
        assert_eq!(outcome.artifacts.len(), 1);
        let names: Vec<String> = fs::read_dir(&output)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 1);
        assert!(names[0].ends_with(".pkg.tar.zst"));
        assert!(work.join("BUILD_LOG.txt").exists());
        assert!(!output.join("BUILD_LOG.txt").exists());
        assert!(!output.join("BUILD_RECIPE.sh").exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn sanierte_version_und_debug_pakete_in_allen_formaten() {
        let root = std::env::temp_dir().join(format!(
            "g2p-version-out-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"repo\"\nversion = \"v1.0.0-13\"\n",
        )
        .unwrap();
        let identity = project_identity("example-app", &root);
        assert_eq!(identity.version, "1.0.0.13");
        fs::remove_dir_all(root).unwrap();

        let identity = ProjectIdentity {
            name: "repo".to_string(),
            version: "1.0.0.13".to_string(),
        };
        let arch_config = config(DistroFamily::Arch, TargetFormat::NativePackage);
        let arch = build_script_with(
            &arch_config,
            &BuildSystem::Unknown,
            &[],
            None,
            &identity,
            BuildStrategy::Default,
            false,
        );
        assert!(arch.contains("pkgver=1.0.0.13"));
        assert!(arch.contains("'!debug'"));
        assert!(arch.contains("rm -f /work/pkg/*-debug-*.pkg.tar.zst"));

        let deb_config = config(DistroFamily::Debian, TargetFormat::NativePackage);
        let deb = build_script_with(
            &deb_config,
            &BuildSystem::Unknown,
            &[],
            None,
            &identity,
            BuildStrategy::Default,
            false,
        );
        assert!(deb.contains("repo_1.0.0.13_amd64.deb"));
        assert!(deb.contains("rm -f /work/dist/*-dbgsym*.deb"));

        let rpm_config = config(DistroFamily::Fedora, TargetFormat::NativePackage);
        let rpm = build_script_with(
            &rpm_config,
            &BuildSystem::Unknown,
            &[],
            None,
            &identity,
            BuildStrategy::Default,
            false,
        );
        assert!(rpm.contains("Version: 1.0.0.13"));
        assert!(rpm.contains("%define debug_package %{nil}"));
    }

    #[test]
    fn paket_inhalt_wird_generisch_geprueft() {
        let tar = "-rw-r--r-- 1 root/root 4096 2026-11-17 14:00 usr/bin/app\n-rw-r--r-- 1 root/root 120 2026-11-17 14:00 .PKGINFO\n";
        let (files, bytes, has_size) = listing_payload(tar, "pkg");
        assert_eq!(files, 1);
        assert_eq!(bytes, 4096);
        assert!(has_size);

        let empty = "-rw-r--r-- 1 root/root 120 2026-11-17 14:00 .PKGINFO\n-rw-r--r-- 1 root/root 80 2026-11-17 14:00 .BUILDINFO\n-rw-r--r-- 1 root/root 60 2026-11-17 14:00 .MTREE\n";
        let (files, _, _) = listing_payload(empty, "pkg");
        assert_eq!(files, 0);

        let deb = "-rw-r--r-- root/root 2048 2026-11-17 14:00 ./usr/lib/libapp.so\n";
        let (files, bytes, has_size) = listing_payload(deb, "deb");
        assert_eq!(files, 1);
        assert_eq!(bytes, 2048);
        assert!(has_size);

        let rpm = "/usr/bin/app\n/usr/share/app/app.desktop\n";
        let (files, _, has_size) = listing_payload(rpm, "rpm");
        assert_eq!(files, 2);
        assert!(!has_size);

        assert!(archive_listing_command("pkg", "/work/dist/app.pkg.tar.zst").contains("tar -tvf"));
        assert!(archive_listing_command("deb", "/work/dist/app.deb").contains("dpkg-deb -c"));
        assert!(archive_listing_command("rpm", "/work/dist/app.rpm").contains("rpm -qlp"));
    }

    #[test]
    fn cache_key_und_strategien_sind_stabil() {
        assert_eq!(
            source_cache_key("sample-tool/y.git"),
            source_cache_key("sample-tool/y.git")
        );
        assert_ne!(
            source_cache_key("sample-tool/y.git"),
            source_cache_key("sample-tool/z.git")
        );
        assert_eq!(
            BuildStrategy::Default.next(true),
            BuildStrategy::SystemLibsOnly
        );
        assert_eq!(
            BuildStrategy::Default.next(false),
            BuildStrategy::RepoScript
        );
        assert_eq!(
            BuildStrategy::SystemLibsOnly.next(true),
            BuildStrategy::RepoScript
        );
        assert_eq!(
            BuildStrategy::RepoScript.next(true),
            BuildStrategy::DirectStagingFallback
        );
        assert_eq!(
            BuildStrategy::DirectStagingFallback.next(true),
            BuildStrategy::Default
        );
    }

    #[test]
    fn alternative_build_schritte() {
        let script = build_step_text(
            &BuildSystem::CMake,
            BuildStrategy::RepoScript,
            Some("BuildLinux.sh"),
        );
        assert!(script.contains("./BuildLinux.sh"));
        assert!(script.contains("bash \"$script\""));
        assert!(script.contains("usage|and then"));
        let default_with_script = build_step_text(
            &BuildSystem::CMake,
            BuildStrategy::Default,
            Some("BuildLinux.sh"),
        );
        assert!(!default_with_script.contains("BuildLinux.sh"));
        let make = build_step_text(
            &BuildSystem::CMake,
            BuildStrategy::DirectStagingFallback,
            None,
        );
        assert!(make.contains("Unix Makefiles"));
        let default = build_step_text(&BuildSystem::CMake, BuildStrategy::Default, None);
        assert!(default.contains("cmake -S . -B build"));
        assert!(default.contains("cmake --build build"));
    }

    #[test]
    fn pacman_bekommt_doppelten_bindestrich() {
        let cmd = install_packages_command(
            &DistroFamily::Arch,
            &["libpng".to_string(), "eigen".to_string()],
        );
        assert!(cmd.contains("pacman -S --noconfirm --needed -- libpng eigen"));
    }

    #[test]
    fn paketnamen_werden_von_sonderzeichen_befreit() {
        assert_eq!(sanitize_package("foo (bar)"), "foo");
        assert_eq!(sanitize_package("--foundexpat:/usr/lib/libexpat.so"), "");
        assert_eq!(sanitize_package("-boom"), "");
        assert_eq!(sanitize_package("libpng-dev"), "libpng-dev");
    }

    #[test]
    fn paketinstallation_vermeidet_sippipe_pipelinen() {
        assert_eq!(
            install_packages_command(&DistroFamily::Arch, &["libpng".to_string()]),
            "pacman -S --noconfirm --needed -- libpng"
        );
        for family in [
            DistroFamily::Arch,
            DistroFamily::Debian,
            DistroFamily::Fedora,
            DistroFamily::Suse,
        ] {
            let script = build_script(
                &config(family.clone(), TargetFormat::NativePackage),
                &BuildSystem::Cargo,
                &["libpng".to_string()],
            );
            assert!(!script.contains("yes '' |"), "{:?}", family);
        }
    }

    #[test]
    fn paketnamen_mit_klammern_werden_bereinigt() {
        assert_eq!(sanitize_package("foo (bar)"), "foo");
        assert_eq!(sanitize_package("  extra/openssl-3.0  "), "openssl-3.0");
        assert_eq!(sanitize_package("libfoo-dev:amd64"), "libfoo-dev");
    }

    #[test]
    fn datenbank_sync_und_deps_vorbau() {
        assert_eq!(
            package_database_sync(&DistroFamily::Arch),
            Some("pacman -Sy --noconfirm && pacman -Fy --noconfirm")
        );
        assert!(package_database_sync(&DistroFamily::Debian)
            .unwrap()
            .contains("apt-get update"));
        let script = "#!/usr/bin/env bash\nmkdir -p /work/out /work/dist\necho ok\n";
        let patched = with_deps_prebuild(script);
        assert!(patched.contains("export CMAKE_PREFIX_PATH=/work/deps-install"));
        assert!(patched.contains("-DCMAKE_INSTALL_PREFIX=/work/deps-install"));
        assert!(patched.contains("rm -rf /work/deps-build"));
        assert!(patched.contains("konfiguriere lokale Abhaengigkeiten neu"));
        assert!(patched.contains("Resolver uebernimmt"));
        assert!(patched.lines().count() == script.lines().count() + 2);
    }

    #[test]
    fn script_ist_strikt_und_prueft_artefakte() {
        for family in [
            DistroFamily::Arch,
            DistroFamily::Debian,
            DistroFamily::Fedora,
            DistroFamily::Suse,
        ] {
            let script = build_script(
                &config(family.clone(), TargetFormat::NativePackage),
                &BuildSystem::Cargo,
                &[],
            );
            assert!(script.contains("set -e"), "{:?}", family);
            assert!(script.contains("set -o pipefail"), "{:?}", family);
            assert!(
                !script.contains("yes '' |"),
                "{:?}: SIGPIPE-Pipeline",
                family
            );
            let check = script
                .find("__GIT2PKG_NO_ARTIFACT__")
                .expect("artefaktcheck");
            let ok = script.find(OK_MARKER).expect("ok-marker");
            assert!(check < ok, "{:?}: check muss vor OK stehen", family);
            let empty = script
                .find("__GIT2PKG_EMPTY_PACKAGE__")
                .expect("leerpaketcheck");
            assert!(
                empty < ok,
                "{:?}: leerpaketcheck muss vor OK stehen",
                family
            );
        }
    }

    #[test]
    fn cleanup_entfernt_schreibgeschuetzten_git_ordner() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!(
            "g2p-cleanup-git-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let work = root.join("builds/example-hash");
        let object = work.join("src/.git/objects/readonly");
        fs::create_dir_all(object.parent().unwrap()).unwrap();
        fs::write(&object, b"objekt").unwrap();
        let mut permissions = fs::metadata(&object).unwrap().permissions();
        permissions.set_mode(0o444);
        fs::set_permissions(&object, permissions).unwrap();
        fs::write(work.join("FAILED"), b"").unwrap();

        assert!(remove_build_dir_recursively(&work).is_ok());
        assert!(!work.exists());
        assert!(!work.parent().unwrap().exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn arch_baut_als_unprivilegierter_benutzer() {
        let script = build_script(
            &config(DistroFamily::Arch, TargetFormat::NativePackage),
            &BuildSystem::Cargo,
            &[],
        );
        assert!(script.contains("useradd -m -s /bin/bash builder"));
        assert!(script.contains("chown -R builder:builder"));
        assert!(script.contains("su builder -c 'cd /work/pkg && makepkg -s --noconfirm'"));
        assert!(!script.contains("--asroot"));
    }

    #[test]
    fn deb_und_rpm_bauen_mit_fakeroot_und_rpmbuild_verzeichnis() {
        let deb = build_script(
            &config(DistroFamily::Debian, TargetFormat::NativePackage),
            &BuildSystem::Cargo,
            &[],
        );
        assert!(deb.contains("fakeroot dpkg-deb --build"));
        let rpm = build_script(
            &config(DistroFamily::Fedora, TargetFormat::NativePackage),
            &BuildSystem::Cargo,
            &[],
        );
        assert!(rpm.contains("/root/rpmbuild/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}"));
        assert!(rpm.contains("rpmbuild -bb --clean --define \"_topdir /root/rpmbuild\""));
    }

    #[test]
    fn nur_echte_fehler_werden_als_err_markiert() {
        assert!(!is_error_line("Klone nach /work/src ..."));
        assert!(!is_error_line("Compiling git2pkg v0.1.0"));
        assert!(is_error_line("error: could not compile"));
        assert!(is_error_line("fatal: not a git repository"));
        assert!(is_error_line("FEHLER: Paket fehlt"));
    }

    #[test]
    fn pkgbuild_wird_bei_deb_und_rpm_ignoriert() {
        let identity = ProjectIdentity {
            name: "repo".to_string(),
            version: "0.1.0".to_string(),
        };

        let deb_cfg = config(DistroFamily::Debian, TargetFormat::NativePackage);
        let deb = build_script_with(
            &deb_cfg,
            &BuildSystem::CMake,
            &[],
            None,
            &identity,
            BuildStrategy::Default,
            true,
        );
        assert!(deb.contains("Arch-PKGBUILD wird fuer dieses Zielformat ignoriert"));
        assert!(deb.contains("cmake --install build"));
        assert!(deb.contains(
            "cmake --build build -j$(nproc) && [ -f build/cmake_install.cmake ] && DESTDIR="
        ));
        assert!(deb.contains("dpkg-deb --build /work/deb /work/dist/repo_0.1.0_amd64.deb"));
        assert!(!deb.contains("makepkg"));

        let rpm_cfg = config(DistroFamily::Fedora, TargetFormat::NativePackage);
        let rpm = build_script_with(
            &rpm_cfg,
            &BuildSystem::CMake,
            &[],
            None,
            &identity,
            BuildStrategy::Default,
            true,
        );
        assert!(rpm.contains("rpmbuild -bb"));
        assert!(!rpm.contains("makepkg"));
    }

    #[test]
    fn vorhandene_pkgbuild_laeuft_im_repo_verzeichnis() {
        let cfg = config(DistroFamily::Arch, TargetFormat::NativePackage);
        let identity = ProjectIdentity {
            name: "repo".to_string(),
            version: "0.1.0".to_string(),
        };
        let with_repo = build_script_with(
            &cfg,
            &BuildSystem::Unknown,
            &[],
            None,
            &identity,
            BuildStrategy::Default,
            true,
        );
        assert!(with_repo.contains("cd /work/src && makepkg -s --noconfirm"));
        assert!(with_repo.contains("cp /work/src/*.pkg.tar.zst /work/dist/"));
        assert!(!with_repo.contains("cat > /work/pkg/PKGBUILD"));

        let generated = build_script_with(
            &cfg,
            &BuildSystem::Unknown,
            &[],
            None,
            &identity,
            BuildStrategy::Default,
            false,
        );
        assert!(generated.contains("cat > /work/pkg/PKGBUILD"));
    }

    #[test]
    fn script_enthaelt_makerpkg_fuer_arch() {
        let script = build_script(
            &config(DistroFamily::Arch, TargetFormat::NativePackage),
            &BuildSystem::Cargo,
            &[],
        );
        assert!(script.contains("makepkg"));
        assert!(script.contains("pkgname=example-app"));
        assert!(script.contains(OK_MARKER));
        assert!(script.contains("rm -rf /work/cargo-target /work/cargo-home"));
        assert!(script.contains("chown -R --reference=/work /work"));
        assert!(script.contains("chmod -R u+rwX /work"));
    }

    #[test]
    fn script_enthaelt_dpkg_deb_fuer_debian() {
        let script = build_script(
            &config(DistroFamily::Debian, TargetFormat::NativePackage),
            &BuildSystem::Cargo,
            &[],
        );
        assert!(script.contains("dpkg-deb"));
        assert!(script.contains("example-app_0.1.0_amd64.deb"));
    }

    #[test]
    fn script_enthaelt_rpmbuild_fuer_fedora() {
        let script = build_script(
            &config(DistroFamily::Fedora, TargetFormat::NativePackage),
            &BuildSystem::Cargo,
            &[],
        );
        assert!(script.contains("rpmbuild"));
        assert!(script.contains("example-app.spec"));
    }

    #[test]
    fn script_nachinstallation_wird_eingebettet() {
        let script = build_script(
            &config(DistroFamily::Arch, TargetFormat::StandaloneBinary),
            &BuildSystem::CMake,
            &["openssl".to_string()],
        );
        assert!(script.contains("pacman -S --noconfirm --needed -- openssl"));
        assert!(script.contains("cmake -S . -B build"));
        assert!(script.contains("export CMAKE_POLICY_VERSION_MINIMUM=3.5"));
        assert!(script.contains("-DCMAKE_POLICY_VERSION_MINIMUM=3.5"));
        assert!(script.contains("-DCMAKE_SUPPRESS_DEVELOPER_ERRORS=ON"));
    }

    #[test]
    fn deps_fehler_erkennt_system_fallback() {
        assert!(deps_fallback_needed(
            "[err] [deps] Bau fehlgeschlagen - Resolver uebernimmt",
            false
        ));
        assert!(!deps_fallback_needed(
            "[err] [deps] Bau fehlgeschlagen - Resolver uebernimmt",
            true
        ));
        assert!(!deps_fallback_needed("[out] normals Build-Log", false));
    }

    #[test]
    fn alle_install_schritte_legen_zielverzeichnisse_an() {
        use crate::core::detector::BuildSystem;
        for system in [
            BuildSystem::Cargo,
            BuildSystem::Go,
            BuildSystem::CMake,
            BuildSystem::Meson,
            BuildSystem::Python,
            BuildSystem::Make,
            BuildSystem::Qmake,
        ] {
            let step = project_install_step(&system, "app");
            assert!(
                step.contains("mkdir -p \"$pkgdir/usr/bin\""),
                "Install-Schritt für {system:?} legt usr/bin nicht an"
            );
        }
        let assets = asset_staging_step("example-app");
        assert!(assets.contains("mkdir -p \"$pkgdir/usr/share/applications\""));
    }

    #[test]
    fn build_reste_bereinigen_laesst_quellcode_stehen() {
        let root = std::env::temp_dir().join(format!(
            "git2pkg-cleanup-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let work = root.join("hash");
        fs::create_dir_all(work.join("src/.git")).unwrap();
        for name in [
            "build",
            "deps-build",
            "cargo-target",
            "cargo-home",
            "ccache",
            "out",
            "dist",
            "pkg",
        ] {
            fs::create_dir_all(work.join(name)).unwrap();
        }
        fs::write(work.join("FAILED"), "failed\n").unwrap();
        fs::write(work.join("src/.git/config"), "x").unwrap();

        assert!(cleanup_failed_build_artifacts_in(&root) >= 8);
        assert!(work.join("src/.git/config").is_file());
        for name in [
            "build",
            "deps-build",
            "cargo-target",
            "cargo-home",
            "ccache",
            "out",
            "dist",
            "pkg",
        ] {
            assert!(!work.join(name).exists());
        }
        assert!(!work.join("FAILED").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn erfolgreicher_build_entfernt_hashordner_und_leeres_builds_verzeichnis() {
        let builds = std::env::temp_dir().join(format!(
            "git2pkg-success-cleanup-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let build_dir = builds.join("abc123");
        let _ = fs::remove_dir_all(&builds);
        fs::create_dir_all(build_dir.join("src/.git/objects/aa")).unwrap();
        fs::create_dir_all(build_dir.join("cargo-target")).unwrap();
        fs::write(build_dir.join("src/README.md"), "source").unwrap();
        fs::write(build_dir.join("src/.git/objects/aa/readonly"), "object").unwrap();
        fs::write(build_dir.join("build.sh"), "script").unwrap();
        let read_only = build_dir.join("src/.git/objects/aa/readonly");
        fs::set_permissions(&read_only, fs::Permissions::from_mode(0o444)).unwrap();

        let (tx, _rx) = std::sync::mpsc::channel();
        cleanup_successful_build(&build_dir, &tx);
        assert!(!build_dir.exists(), "Hash-Ordner wurde nicht entfernt");
        assert!(!builds.exists(), "Leeres builds/ wurde nicht entfernt");
    }

    #[test]
    fn source_group_datei_aus_external_project_hint_findet() {
        let root = std::env::temp_dir().join(format!(
            "git2pkg-hint-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let extension =
            root.join("deps-build/dep_example-prefix/src/dep_example/libs/locale/include/boost");
        fs::create_dir_all(&extension).unwrap();
        let cmakelists =
            root.join("deps-build/dep_example-prefix/src/dep_example/libs/locale/CMakeLists.txt");
        fs::create_dir_all(cmakelists.parent().unwrap()).unwrap();
        fs::write(&cmakelists, "source_group(x)\n").unwrap();
        let hint = extension.join("locale.hpp");
        assert_eq!(
            find_source_file_from_hint(&hint, "libs/locale/CMakeLists.txt"),
            Some(cmakelists)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn source_group_block_wird_kommentiert() {
        let path = std::env::temp_dir().join(format!(
            "git2pkg-source-group-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(
            &path,
            "project(x)\nsource_group(\"grp\" FILES\n  a.cpp\n  b.cpp\n)\nadd_executable(x a.cpp b.cpp)\n",
        )
        .unwrap();
        assert!(comment_source_group_block(&path, 2));
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("# source_group(\"grp\" FILES"));
        assert!(text.contains("# a.cpp"));
        assert!(text.contains("# b.cpp"));
        assert!(text.contains("# )"));
        assert!(text.contains("add_executable(x a.cpp b.cpp)"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn cmake_kompatibilitaets_flags_im_retry() {
        let mut cfg = config(DistroFamily::Arch, TargetFormat::NativePackage);
        cfg.cmake_compat = true;
        let identity = ProjectIdentity {
            name: "repo".to_string(),
            version: "0.1.0".to_string(),
        };
        let retry = build_script_with(
            &cfg,
            &BuildSystem::CMake,
            &[],
            None,
            &identity,
            BuildStrategy::Default,
            false,
        );
        assert!(retry.contains("export CMAKE_SUPPRESS_DEVELOPER_ERRORS=ON"));
    }

    #[test]
    fn install_command_passt_zur_distro() {
        let artifact = vec![PathBuf::from(
            "/tmp/dist/git2pkg-build-1.0.0-1-x86_64.pkg.tar.zst",
        )];
        assert_eq!(
            install_command(
                &config(DistroFamily::Arch, TargetFormat::NativePackage),
                &artifact
            ),
            "sudo pacman -U /tmp/dist/git2pkg-build-1.0.0-1-x86_64.pkg.tar.zst"
        );
        assert_eq!(
            install_command(
                &config(DistroFamily::Debian, TargetFormat::NativePackage),
                &artifact
            ),
            "sudo apt install /tmp/dist/git2pkg-build-1.0.0-1-x86_64.pkg.tar.zst"
        );
        assert_eq!(
            install_command(
                &config(DistroFamily::Suse, TargetFormat::NativePackage),
                &artifact
            ),
            "sudo zypper install /tmp/dist/git2pkg-build-1.0.0-1-x86_64.pkg.tar.zst"
        );
    }

    #[test]
    fn target_format_schluessel() {
        assert_eq!(
            TargetFormat::from_key("binary"),
            TargetFormat::StandaloneBinary
        );
        assert_eq!(
            TargetFormat::from_key("package"),
            TargetFormat::NativePackage
        );
        assert_eq!(TargetFormat::NativePackage.key(), "package");
    }

    #[test]
    fn cmake_retry_flags_aus_fehlertext_und_policy() {
        let output = "CMake Error: Missing backend, or pass -DFOO_BAR=ON\nmirrors: set -DUSE_SYSTEM_FOO=ON\nCompatibility with CMake < 3.5 has been removed.\nEigen3 version 5.0.0 found, but at least version 3.3 is required\n";
        let flags = cmake_retry_flags(output, &[]);
        assert!(flags.contains(&"-DFOO_BAR=ON".to_string()));
        assert!(flags.contains(&"-DUSE_SYSTEM_FOO=ON".to_string()));
        assert!(flags.contains(&"-DCMAKE_POLICY_VERSION_MINIMUM=3.5".to_string()));
        assert!(flags.contains(&"-DEIGEN3_VERSION_OK=TRUE".to_string()));
        let again = cmake_retry_flags(output, &flags);
        assert!(again.is_empty());
    }

    #[test]
    fn retry_state_erkennt_leerlauf() {
        let before = RetryState {
            packages: vec!["boost".to_string()],
            cmake_flags: vec!["-DFOO=ON".to_string()],
        };
        let same = RetryState {
            packages: vec!["boost".to_string()],
            cmake_flags: vec!["-DFOO=ON".to_string()],
        };
        let changed = RetryState {
            packages: vec!["boost".to_string(), "openssl".to_string()],
            cmake_flags: vec!["-DFOO=ON".to_string()],
        };
        assert!(!retry_state_changed(&before, &same));
        assert!(retry_state_changed(&before, &changed));
    }

    #[test]
    fn cmake_retry_flags_landen_im_buildschritt() {
        let flags = vec!["-DENABLE_XYZ=OFF".to_string()];
        let step =
            build_step_text_with_flags(&BuildSystem::CMake, BuildStrategy::Default, None, &flags);
        assert!(step.contains("--build build"));
        assert!(step.contains("-DENABLE_XYZ=OFF"));
        let with_deps = with_deps_prebuild_flags(
            "#!/usr/bin/env bash\nmkdir -p /work/out /work/dist\n",
            &flags,
        );
        assert!(with_deps.contains("-DENABLE_XYZ=OFF"));
        assert!(with_deps.contains("-DCMAKE_POLICY_VERSION_MINIMUM=3.5"));
    }
}
