//! Container-Steuerung und Build-Orchestrierung.
//! Alle Builds laufen in einem Wegwerf-Container (`podman run --rm`,
//! Fallback `docker run --rm`). Ausgaben werden über einen `mpsc::Sender`
//! live an die GUI weitergeleitet.

use crate::core::detector::{detect_build_system, has_local_deps, BuildSystem, ProjectFingerprint};
use crate::core::distro::{DistroFamily, DistroInfo, TargetDistro};
use crate::core::resolver::{
    analyze_output, candidate_names, classify_failure, cmake_alternative_flags,
    cmake_option_flags_from_text, dependency_id, detect_abi_mismatch, detect_build_fixes,
    detect_repo_script_distro_rejection, detect_source_group_error, family_key,
    is_cmake_policy_error, is_compiled_build_system, is_compiler_too_old,
    is_transient_network_error, package_from_search, packages_from_file_database, probe_commands,
    search_command, should_penalize_mapping, tool_file_database_commands, valid_package_name,
    BuildRecipe, FailureKind, MappingStore, MissingDep, SourceGroupError,
};
use crate::gui::i18n::{Language, Texts};
use regex::Regex;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::BufRead;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{ChildStderr, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(crate) const OK_MARKER: &str = "__GIT2PKG_BUILD_OK__";
const DEPS_FAILURE_MARKER: &str = "[deps] build failed";

fn deps_fallback_needed(output: &str, skip_deps: bool) -> bool {
    !skip_deps && output.contains(DEPS_FAILURE_MARKER)
}
const MAX_ATTEMPTS: u8 = 4;
const INITIAL_HEAL_ROUNDS: u8 = 5;
/// Obergrenze der Heil-Runden je Strategie, sobald Abhaengigkeiten geloest
/// werden. Ueber `GIT2PKG_MAX_HEAL_ROUNDS` anpassbar (1-200).
fn max_heal_rounds() -> u8 {
    std::env::var("GIT2PKG_MAX_HEAL_ROUNDS")
        .ok()
        .and_then(|value| value.trim().parse::<u16>().ok())
        .map(|value| value.clamp(1, 200) as u8)
        .unwrap_or(20)
}

/// Zustand eines Selbstheilungs-Durchlaufs. Nur wenn er sich aendert, ist ein
/// weiterer Retry derselben Strategie sinnvoll.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RetryState {
    packages: Vec<String>,
    cmake_flags: Vec<String>,
    include_dirs: Vec<String>,
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
    flags.retain(|flag| !current.contains(flag));
    flags
}

fn retry_state_changed(before: &RetryState, after: &RetryState) -> bool {
    before != after
}

/// Strategie-abhaengige `option(...)`-Flags aus dem Projekt und lokalen deps/.
fn cmake_option_flags(src: &Path, strategy: BuildStrategy) -> Vec<String> {
    let system_libs = matches!(
        strategy,
        BuildStrategy::SystemLibsOnly | BuildStrategy::DirectStagingFallback
    );
    let mut flags: Vec<String> = Vec::new();
    for file in [src.join("CMakeLists.txt"), src.join("deps/CMakeLists.txt")] {
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };
        for flag in cmake_option_flags_from_text(&text, system_libs, system_libs) {
            push_unique_string(&mut flags, flag);
        }
    }
    flags
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
            workspace_mount(work),
            "-w".to_string(),
            "/work".to_string(),
            choose_build_image(
                config,
                &config.target_distro.info(&config.distro).base_image,
            ),
            "sleep".to_string(),
            "infinity".to_string(),
        ];
        let started = run_streamed(config.engine.binary(), &args, tx)?;
        if !started.success {
            return Err("container could not be started".to_string());
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
            BuildStrategy::Default => "Default",
            BuildStrategy::SystemLibsOnly => "System libraries only (no deps/ prebuild)",
            BuildStrategy::RepoScript => "Repository build script",
            BuildStrategy::DirectStagingFallback => "Direct build-system fallback",
        }
    }
}

/// Nächste noch nicht versuchte Strategie. Es gibt keinen Rücksprung zum
/// Anfang: Sind alle sinnvollen Stufen versucht, endet der Build.
fn next_unvisited_strategy(
    current: BuildStrategy,
    has_deps: bool,
    visited: &HashSet<&'static str>,
) -> Option<BuildStrategy> {
    let order: &[BuildStrategy] = if has_deps {
        &[
            BuildStrategy::Default,
            BuildStrategy::SystemLibsOnly,
            BuildStrategy::RepoScript,
            BuildStrategy::DirectStagingFallback,
        ]
    } else {
        &[
            BuildStrategy::Default,
            BuildStrategy::RepoScript,
            BuildStrategy::DirectStagingFallback,
        ]
    };
    let start = order.iter().position(|strategy| *strategy == current)?;
    order
        .iter()
        .skip(start + 1)
        .copied()
        .find(|strategy| !visited.contains(strategy.key()))
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

fn git2pkg_cache_root() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    Path::new(&home).join(".cache/git2pkg")
}

fn build_cache_root() -> PathBuf {
    git2pkg_cache_root().join("builds")
}

/// Dauerhafter, sauber gehaltener Klon-Cache. Er wird niemals vom Build-Cache-
/// Cleanup geloescht und bei jedem Lauf nur aktualisiert.
fn repo_cache_root() -> PathBuf {
    git2pkg_cache_root().join("repos")
}

fn repo_cache_dir_name(url: &str) -> String {
    format!("repo-{}", source_cache_key(url))
}

fn build_dir_name(millis: u128, pid: u32) -> String {
    format!("build-{millis}-{pid}")
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|error| format!("copy {}: {error}", to.display()))?;
    for entry in fs::read_dir(from).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        let metadata = entry.file_type().map_err(|error| error.to_string())?;
        if metadata.is_dir() {
            copy_tree(&source, &target)?;
        } else if metadata.is_symlink() {
            #[cfg(unix)]
            {
                let link = fs::read_link(&source).map_err(|error| error.to_string())?;
                let _ = fs::remove_file(&target);
                std::os::unix::fs::symlink(link, &target)
                    .map_err(|error| format!("symlink {}: {error}", target.display()))?;
            }
        } else {
            fs::copy(&source, &target)
                .map_err(|error| format!("copy {}: {error}", source.display()))?;
        }
    }
    Ok(())
}

/// Submodule werden flach und parallel geholt: grosse Projekte (z. B. der
/// Rust-Compiler mit LLVM und GCC als Submodule) brauchen sonst ein
/// Vielfaches an Zeit und Plattenplatz, obwohl nur der Arbeitsbaum gebraucht
/// wird.
fn submodule_update_args() -> Vec<String> {
    vec![
        "submodule".to_string(),
        "update".to_string(),
        "--init".to_string(),
        "--recursive".to_string(),
        "--depth=1".to_string(),
        "--jobs=4".to_string(),
    ]
}

fn prepare_repo_cache(url: &str, cache: &Path, tx: &Sender<BuildEvent>) -> Result<(), String> {
    if !cache.join(".git").is_dir() {
        if cache.exists() {
            remove_build_dir_recursively(cache)?;
        }
        fs::create_dir_all(cache.parent().unwrap_or_else(|| Path::new("/")))
            .map_err(|error| error.to_string())?;
        let cloned = run_streamed(
            "git",
            &[
                "clone".to_string(),
                "--depth".to_string(),
                "1".to_string(),
                "--tags".to_string(),
                url.to_string(),
                cache.to_string_lossy().into_owned(),
            ],
            tx,
        )?;
        if !cloned.success {
            return Err("clone failed".to_string());
        }
        // `--depth 1 --tags` liefert bei GitHub keine Tags; separat nachladen.
        let _ = run_streamed(
            "git",
            &[
                "-C".to_string(),
                cache.to_string_lossy().into_owned(),
                "fetch".to_string(),
                "--tags".to_string(),
                "--depth=1".to_string(),
                "origin".to_string(),
            ],
            tx,
        );
    } else {
        let path = cache.to_string_lossy().into_owned();
        let run = |args: &[&str]| {
            let mut command = vec!["-C".to_string(), path.clone()];
            command.extend(args.iter().map(|arg| arg.to_string()));
            run_streamed("git", &command, tx)
        };
        let _ = run(&["fetch", "--all", "--tags", "--prune"]);
        // Tags auch bei Shallow-Repositories zuverlaessig nachziehen.
        let _ = run(&["fetch", "--tags", "--depth=1", "origin"]);
        let _ = run(&["remote", "set-head", "origin", "--auto"]);
        let reset = run(&["reset", "--hard", "origin/HEAD"]);
        if !reset.map(|result| result.success).unwrap_or(false) {
            let _ = run(&["reset", "--hard", "FETCH_HEAD"]);
        }
        let _ = run(&["clean", "-fdx"]);
    }
    if cache.join(".gitmodules").is_file() {
        let path = cache.to_string_lossy().into_owned();
        let mut args = vec!["-C".to_string(), path.clone()];
        args.extend(submodule_update_args());
        let _ = run_streamed("git", &args, tx);
        let _ = run_streamed(
            "git",
            &[
                "-C".to_string(),
                path,
                "submodule".to_string(),
                "foreach".to_string(),
                "--recursive".to_string(),
                "git reset --hard && git clean -fdx".to_string(),
            ],
            tx,
        );
    }
    Ok(())
}

/// Aktualisiert den dauerhaften Repo-Cache und stellt einen frischen
/// Arbeitsbereich unter `builds/<id>/src` bereit. Der Cache selbst bleibt sauber.
fn prepare_source_workspace(
    url: &str,
    src: &Path,
    tx: &Sender<BuildEvent>,
    texts: &Texts,
) -> Result<(), String> {
    let cache = repo_cache_root().join(repo_cache_dir_name(url));
    step(tx, texts.step_clone());
    if let Err(error) = prepare_repo_cache(url, &cache, tx) {
        mark_failed_build(src.parent().unwrap_or(src));
        return Err(if error == "clone failed" {
            texts.err_clone_failed().to_string()
        } else {
            error
        });
    }
    let _ = tx.send(BuildEvent::Log(format!(
        "[cache] repository cache updated: {}",
        cache.display()
    )));
    if src.exists() {
        make_tree_writable(src);
        fs::remove_dir_all(src).map_err(|error| format!("workspace: {error}"))?;
    }
    copy_tree(&cache, src)
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
    cleanup_all_builds_in(&build_cache_root())
}

/// Shell-Kommando für den Fall, dass root-eigene Dateien im Build-Cache liegen.
fn container_cleanup_args(builds: &Path, image: &str) -> (String, Vec<String>) {
    let script = "rm -rf /cleanup/* /cleanup/.[!.]* /cleanup/..?*";
    let mount = format!("{}:/cleanup:z", builds.display());
    (
        "docker".to_string(),
        vec![
            "run".to_string(),
            "--rm".to_string(),
            "-v".to_string(),
            mount,
            image.to_string(),
            "sh".to_string(),
            "-c".to_string(),
            script.to_string(),
        ],
    )
}

/// Name des optionalen, vorgebauten Builder-Images einer Distribution.
///
/// Solche Images bringen die komplette Toolchain bereits mit (siehe
/// `packaging/builders/`). Ist eines vorhanden, wird es statt des nackten
/// Basis-Images verwendet und die Paketinstallation im Build entfaellt -
/// das macht Laeufe reproduzierbar und deutlich schneller.
fn builder_image_name(family: &DistroFamily) -> String {
    let suffix = match family {
        DistroFamily::Arch => "arch",
        DistroFamily::Debian => "debian",
        DistroFamily::Fedora => "fedora",
        DistroFamily::Suse => "suse",
        DistroFamily::Unknown => return String::new(),
    };
    format!("git2pkg-builder:{suffix}")
}

/// Waehlt das Builder-Image, wenn es lokal vorhanden ist, sonst das Basis-Image.
///
/// Ein versionsgebundenes Basis-Image (z. B. `ubuntu:24.04` fuer Mint) darf
/// nicht durch ein familienweites Builder-Image ersetzt werden - dessen glibc
/// waere die des `latest`-Images und damit zu neu fuer den Host.
fn choose_build_image(config: &BuildConfig, base_image: &str) -> String {
    let info = config.target_distro.info(&config.distro);
    let family_default =
        crate::core::distro::DistroInfo::for_family(info.family.clone()).base_image;
    if base_image != family_default {
        return base_image.to_string();
    }
    let builder = builder_image_name(&info.family);
    if !builder.is_empty() && local_image_exists(&config.engine, &builder) {
        return builder;
    }
    base_image.to_string()
}

/// Bind-Mount des Arbeitsverzeichnisses. Das `:z` relabelt die Dateien fuer
/// SELinux (Standard auf openSUSE Tumbleweed und Fedora); ohne das Flag
/// scheitert `/work/build.sh` mit "Permission denied" und Exit-Code 126.
/// Auf Hosts ohne SELinux ignoriert Docker das Suffix.
fn workspace_mount(work: &Path) -> String {
    format!("{}:/work:z", work.display())
}

fn local_image_exists(engine: &ContainerEngine, image: &str) -> bool {
    Command::new(engine.binary())
        .args(["image", "inspect", image])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn build_cache_has_entries() -> bool {
    fs::read_dir(build_cache_root())
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false)
}

/// Erst normal löschen; wenn root-eigene Reste bleiben, einmalig in einem
/// Wegwerf-Container aufräumen. Repo-Cache und Lerndaten bleiben unberührt.
#[allow(dead_code)]
pub fn cleanup_all_builds_with(engine: ContainerEngine, image: &str) -> usize {
    let mut removed = cleanup_all_builds();
    if !build_cache_has_entries() {
        return removed;
    }
    if !local_image_exists(&engine, image) {
        return removed;
    }
    let builds = build_cache_root();
    let mut command_spec = container_cleanup_args(&builds, image);
    command_spec.0 = engine.binary().to_string();
    let ok = Command::new(&command_spec.0)
        .args(&command_spec.1)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if ok {
        removed += cleanup_all_builds();
    }
    removed
}

fn remove_tree_in_place(path: &Path) -> bool {
    if !path.exists() {
        return true;
    }
    make_tree_writable(path);
    match fs::remove_dir_all(path) {
        Ok(()) => true,
        Err(_) => {
            make_tree_writable(path);
            fs::remove_dir_all(path).is_ok()
        }
    }
}

fn local_cleanup_image(engine: &ContainerEngine, preferred: &str) -> Option<String> {
    if local_image_exists(engine, preferred) {
        return Some(preferred.to_string());
    }
    let output = Command::new(engine.binary())
        .arg("images")
        .arg("-q")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

fn stop_build_containers(engine: &ContainerEngine) {
    let Ok(output) = Command::new(engine.binary())
        .args(["ps", "-aq", "--filter", "name=git2pkg-build-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    else {
        return;
    };
    for id in String::from_utf8_lossy(&output.stdout).lines() {
        let id = id.trim();
        if !id.is_empty() {
            let _ = Command::new(engine.binary())
                .args(["rm", "-f", id])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

fn cleanup_all_cache_in(cache: &Path) -> bool {
    remove_tree_in_place(cache)
}

/// Beenden-Auswahl „Löschen“: kompletter Cache, inkl. Repos und Builds.
/// Lern-Daten unter `~/.config/git2pkg/` bleiben bewusst unberührt.
pub fn cleanup_all_cache_with(engine: ContainerEngine, image: &str) -> bool {
    stop_build_containers(&engine);
    let cache = git2pkg_cache_root();
    if cleanup_all_cache_in(&cache) {
        return true;
    }
    let Some(local_image) = local_cleanup_image(&engine, image) else {
        return false;
    };
    let mut spec = container_cleanup_args(&cache, &local_image);
    spec.0 = engine.binary().to_string();
    let ok = Command::new(&spec.0)
        .args(&spec.1)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    ok && cleanup_all_cache_in(&cache)
}

fn cleanup_all_builds_in(builds: &Path) -> usize {
    let Ok(entries) = fs::read_dir(builds) else {
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
    let _ = fs::remove_dir(builds);
    removed
}

/// Löscht nur temporäre Build-/Paket-Verzeichnisse fehlgeschlagener Builds;
/// der geklonte Quellcode bleibt für einen erneuten Versuch erhalten.
#[allow(dead_code)]
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

/// Nach einer Paket Nachinstallation muessen alle `find_package()`-Ergebnisse
/// verworfen werden, sonst haelt CMake den alten Misserfolg im Cache fest.
fn reset_cmake_caches(work: &Path) -> usize {
    let mut removed = 0usize;
    for dir in ["build", "deps-build", "deps/build"] {
        let cache = work.join(dir).join("CMakeCache.txt");
        if cache.is_file() && fs::remove_file(&cache).is_ok() {
            removed += 1;
        }
        let files_dir = work.join(dir).join("CMakeFiles");
        if files_dir.is_dir() {
            make_tree_writable(&files_dir);
            if fs::remove_dir_all(&files_dir).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

/// Entfernt nur die Configure-Stempel der ExternalProjects. Downloads und
/// entpackte Quellen bleiben erhalten; der nächste Versuch konfiguriert die
/// Abhängigkeiten aber neu und erkennt frisch installierte Systempakete.
fn reset_dependency_configure_stamps(work: &Path) -> usize {
    fn walk(dir: &Path, removed: &mut usize) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, removed);
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with("-configure"))
                && fs::remove_file(&path).is_ok()
            {
                *removed += 1;
            }
        }
    }
    let mut removed = 0usize;
    walk(&work.join("deps-build"), &mut removed);
    removed
}

fn cleanup_successful_build(work: &Path, tx: &Sender<BuildEvent>) {
    match remove_build_dir_recursively(work) {
        Ok(()) => {
            let _ = tx.send(BuildEvent::Log(
                "[cache] full build cache removed automatically".to_string(),
            ));
        }
        Err(error) => {
            let _ = tx.send(BuildEvent::Log(format!(
                "[warn] build cache could not be removed completely: {error}"
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

/// True, wenn genau die ausgewählte Container-Engine installiert und aufrufbar ist.
pub fn container_engine_installed(engine: &ContainerEngine) -> bool {
    engine_is_available(engine.binary())
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
    /// Gelernter Self-Healing-Fix: neuester verfuegbarer C++-Compiler.
    pub newer_gxx: bool,
    /// Nach gescheitertem deps/-Vorbau: Hauptprojekt gegen Systembibliotheken bauen.
    pub skip_deps: bool,
    /// Zusätzliche Include-Pfade, die aus fehlenden Headern/pkg-config ermittelt wurden.
    pub include_dirs: Vec<String>,
    /// Vorhandener Arbeitsbereich eines fehlgeschlagenen GUI-Versuchs.
    /// Wird für einen Strategie-Retry wiederverwendet, statt erneut zu klonen.
    pub workspace: Option<PathBuf>,
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
    Workspace(PathBuf),
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
        "[warn] build cancelled by the user.".to_string(),
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

/// Kürzt lange Build-Ausgaben für Fehlermeldungen auf die letzten Zeilen.
fn tail_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= count {
        return text.to_string();
    }
    format!(
        "... (last {} of {} lines)\n{}",
        count,
        lines.len(),
        lines[lines.len() - count..].join("\n")
    )
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

/// Installiert einen Kandidaten und prüft anschließend, ob er die fehlende
/// Abhängigkeit wirklich bereitstellt. Nur dann gilt er als Lösung.
#[allow(clippy::too_many_arguments)]
fn try_candidate(
    config: &BuildConfig,
    container: &BuildContainer,
    dep: &MissingDep,
    family: &str,
    candidate: &str,
    mappings: &mut MappingStore,
    failed_candidates: &mut HashSet<String>,
    db_synced: &mut bool,
    tx: &Sender<BuildEvent>,
    texts: &Texts,
) -> Option<(String, String)> {
    let package = sanitize_package(candidate);
    if package.is_empty()
        || failed_candidates.contains(&package)
        || mappings.is_negative(family, &package)
    {
        return None;
    }
    // Vor der Installation prüfen: Ist das Artefakt bereits vorhanden, kann
    // kein Paket etwas reparieren. Das verhindert falsche Learnings, wenn eine
    // Datei bereits aus einem anderen Paket stammt (z. B. aus einem
    // Dokumentationspaket statt aus dem eigentlichen Entwicklerpaket).
    if let Some(evidence) = verify_dependency(container, dep, tx) {
        let _ = tx.send(BuildEvent::Log(format!(
            "[learn] {} is already present ({}) - not installing '{}'",
            dep.label(),
            evidence,
            package
        )));
        return None;
    }
    step(tx, texts.step_install(&package));
    if !try_install(config, container, &package, tx, db_synced) {
        failed_candidates.insert(package.clone());
        mappings.learn_negative(family, &package);
        return None;
    }
    if let Some(evidence) = verify_dependency(container, dep, tx) {
        return Some((package, evidence));
    }
    let _ = tx.send(BuildEvent::Log(format!(
        "[learn] package '{}' installed but does not provide {}",
        package,
        dep.label()
    )));
    mappings.penalize_dependency(family, dep, &package);
    failed_candidates.insert(package);
    None
}

/// Prüft im Container, ob eine Abhängigkeit nach der Installation vorhanden ist.
/// Nur dann darf ein Paket als Lösung gelernt werden.
fn verify_dependency(
    container: &BuildContainer,
    dep: &MissingDep,
    tx: &Sender<BuildEvent>,
) -> Option<String> {
    match dep {
        MissingDep::Header(header) => {
            let base = Path::new(header).file_name()?.to_str()?;
            if base.is_empty()
                || !base
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c))
            {
                return None;
            }
            // Nicht nur /usr/include: manche Pakete installieren Header tiefer
            // (z. B. /usr/include/<arch>/... oder /usr/lib/.../include).
            let command = format!(
                "find /usr /usr/local -maxdepth 8 -path '*/include/*' -name {} -print -quit 2>/dev/null",
                shell_quote(base)
            );
            container
                .exec(&command, tx)
                .ok()
                .and_then(|output| first_output_line(&output))
        }
        MissingDep::PkgConfig(name) => {
            if !valid_package_name(name) {
                return None;
            }
            let command = format!(
                "pkg-config --exists {} && pkg-config --modversion {}",
                shell_quote(name),
                shell_quote(name)
            );
            first_line(container.exec(&command, tx).ok()?)
        }
        MissingDep::CMakeModule(module) => verify_cmake_module(container, module, tx),
        MissingDep::Tool(tool) => {
            if !valid_package_name(tool) {
                return None;
            }
            let command = format!("command -v {} 2>/dev/null", shell_quote(tool));
            first_line(container.exec(&command, tx).ok()?)
        }
        MissingDep::FinderBroken(_) => None,
        MissingDep::Library(lib) => {
            let name = lib.trim_start_matches("lib").to_lowercase();
            if !valid_package_name(&name) {
                return None;
            }
            // Der Linker braucht die unversionierte Entwicklungsdatei; eine
            // bereits installierte Laufzeitbibliothek (`libfoo.so.1`) genügt
            // nicht.
            let command = format!(
                "find /usr/lib /usr/lib64 /usr/local/lib -maxdepth 4 -name {} -print -quit 2>/dev/null",
                shell_quote(&format!("lib{}.so", name))
            );
            container
                .exec(&command, tx)
                .ok()
                .and_then(|output| first_output_line(&output))
        }
    }
}

/// Wie `first_line`, ignoriert aber den Exit-Code: `find` liefert 1, sobald
/// ein Suchpfad fehlt oder nicht lesbar ist, obwohl es Treffer ausgibt.
fn first_output_line(output: &CommandOutput) -> Option<String> {
    output
        .text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

fn first_line(output: CommandOutput) -> Option<String> {
    if !output.success {
        return None;
    }
    output
        .text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

/// Prüft ein CMake-Modul mit einem minimalen `find_package`-Projekt. Das ist
/// auch für eingebaute Find-Module (ZLIB, OpenSSL, Threads) aussagekräftig,
/// die keine eigene Config-Datei besitzen und bei denen ein reiner
/// Datei-Treffer nur das mitgelieferte CMake-Modul selbst finden würde.
fn verify_cmake_module(
    container: &BuildContainer,
    module: &str,
    tx: &Sender<BuildEvent>,
) -> Option<String> {
    let safe = sanitized_cmake_name(module)?;
    let probe = cmake_verification_probe(&safe);
    if let Some(evidence) = first_line(container.exec(&probe, tx).ok()?) {
        return Some(evidence);
    }
    // Ein Modul im CMake-Modulordner kann von CMake selbst stammen - dann ist
    // der Fehlschlag eindeutig. Es kann aber auch von einem Paket mitgeliefert
    // werden (OpenVDB installiert dort sein FindOpenVDB.cmake); dann ist das
    // Modul der Beweis, dass die Bibliothek vorhanden ist.
    let module_path = container
        .exec(
            &format!(
                "find /usr/share/cmake*/Modules -maxdepth 1 -name 'Find{}.cmake' -print -quit 2>/dev/null",
                safe
            ),
            tx,
        )
        .ok()
        .and_then(|output| first_output_line(&output));
    if let Some(path) = module_path {
        let manager = detect_package_manager(container, tx);
        let owner = package_owner(container, &manager, &path, tx);
        if owner.contains("cmake") {
            return None;
        }
        return Some(path);
    }
    // Projekt-eigene Module (Qt, ECM, OpenCV, …) über ihre Config-Datei.
    if let Some(evidence) = find_cmake_config(container, &safe, tx) {
        return Some(evidence);
    }
    if let Some(evidence) = find_cmake_finder_module(container, &safe, tx) {
        return Some(evidence);
    }
    // Manche Projekte melden "Could not find libfoo!" nach eigenem
    // find_path/find_library. Dann ist der Name nur ein Bibliotheks- bzw.
    // Header-Name und kein echtes CMake-Modul.
    let lower = safe.to_ascii_lowercase();
    let stem = lower.strip_prefix("lib").unwrap_or(&lower);
    for name in [format!("lib{}.so", stem), format!("{}.h", stem)] {
        let command = format!(
            "find /usr /usr/local -maxdepth 8 \\( -path '*/include/*' -o -path '*/lib*/*' \\) -name {} -print -quit 2>/dev/null",
            shell_quote(&name)
        );
        let Ok(output) = container.exec(&command, tx) else {
            continue;
        };
        if let Some(found) = first_output_line(&output) {
            return Some(found);
        }
    }
    None
}

fn sanitized_cmake_name(module: &str) -> Option<String> {
    let safe: String = module
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || "_+-".contains(*c))
        .collect();
    (!safe.is_empty()).then_some(safe)
}

/// Baut das Shell-Skript, das ein CMake-Modul wirklich per `find_package`
/// prüft. Reine Funktion, damit die Verifikationsart testbar bleibt.
fn cmake_verification_probe(safe: &str) -> String {
    format!(
        r#"d=$(mktemp -d) && printf '%s\n' \
'cmake_minimum_required(VERSION 3.5)' \
'project(g2pkg_verify C)' \
'find_package({safe} QUIET)' \
'if(NOT {safe}_FOUND)' \
'  message(FATAL_ERROR "g2pkg-missing")' \
'endif()' > "$d/CMakeLists.txt" \
&& cmake -S "$d" -B "$d/build" >/dev/null 2>&1 \
&& echo "find_package({safe})"; rc=$?; rm -rf "$d"; exit $rc"#,
        safe = safe
    )
}

fn find_cmake_config(
    container: &BuildContainer,
    safe: &str,
    tx: &Sender<BuildEvent>,
) -> Option<String> {
    let lower = safe.to_ascii_lowercase();
    let command = format!(
        "find /usr /usr/local -maxdepth 6 \\( -name '{}Config.cmake' -o -name '{}-config.cmake' \\) -print -quit 2>/dev/null",
        safe, lower
    );
    container
        .exec(&command, tx)
        .ok()
        .and_then(|output| first_output_line(&output))
}

fn find_cmake_finder_module(
    container: &BuildContainer,
    safe: &str,
    tx: &Sender<BuildEvent>,
) -> Option<String> {
    let command = format!(
        "find /usr /usr/local -maxdepth 6 -name 'Find{}.cmake' -print -quit 2>/dev/null",
        safe
    );
    first_line(container.exec(&command, tx).ok()?)
}

/// Lenkt einen defekten Versions-Finder generisch auf den installierten
/// CMake-Config-Modus um. Es werden keine Paketnamen aus dem Modulnamen
/// geraten.
fn finder_broken_flags(
    container: &BuildContainer,
    module: &str,
    tx: &Sender<BuildEvent>,
) -> Vec<String> {
    let mut flags = vec!["-DCMAKE_FIND_PACKAGE_PREFER_CONFIG=ON".to_string()];
    let Some(safe) = sanitized_cmake_name(module) else {
        return flags;
    };
    if let Some(config) = find_cmake_config(container, &safe, tx) {
        if let Some(dir) = Path::new(&config).parent() {
            // CMake erwartet <PackageName>_DIR; nur bei einfachen Namen setzen.
            if safe.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                flags.push(shell_quote(&format!("-D{}_DIR={}", safe, dir.display())));
            }
            let _ = tx.send(BuildEvent::Log(format!(
                "[cmake] installed config for '{}' found at {}",
                safe, config
            )));
        }
    }
    flags
}

/// Löst eine fehlende Abhängigkeit auf: gelernte Zuordnung, Datei-Datenbank,
/// Namensvarianten, Tool-Datenbank und zuletzt eine generische Suche.
#[allow(clippy::too_many_arguments)]
fn resolve_dependency(
    config: &BuildConfig,
    container: &BuildContainer,
    dep: &MissingDep,
    family: &str,
    manager: &str,
    mappings: &mut MappingStore,
    failed_candidates: &mut HashSet<String>,
    db_synced: &mut bool,
    tx: &Sender<BuildEvent>,
) -> Option<(String, String)> {
    let texts = Texts::new(config.language);
    let term = dep.search_term();

    // 1. Gelernte, verifizierte Zuordnung.
    if let Some(learning) = mappings.lookup_dependency(family, dep) {
        if let Some(result) = try_candidate(
            config,
            container,
            dep,
            family,
            &learning.package,
            mappings,
            failed_candidates,
            db_synced,
            tx,
            &texts,
        ) {
            return Some(result);
        }
    }

    // 2. Datei-Datenbank: liefert das Paket, das die Datei/ das Modul enthält.
    for command in probe_commands(manager, dep) {
        if ensure_package_database(config, container, tx, db_synced).is_err() {
            continue;
        }
        let Ok(found) = container.exec(&command, tx) else {
            continue;
        };
        let mut probe_packages: Vec<String> = Vec::new();
        if manager == "pacman" {
            probe_packages.extend(packages_from_file_database(&found.text, &term));
        }
        probe_packages.extend(
            found
                .text
                .lines()
                .filter_map(|line| package_from_search(line, &term, manager)),
        );
        for candidate in probe_packages {
            if let Some(result) = try_candidate(
                config,
                container,
                dep,
                family,
                &candidate,
                mappings,
                failed_candidates,
                db_synced,
                tx,
                &texts,
            ) {
                return Some(result);
            }
        }
    }

    // 3. Namensvarianten (exakt, lib…, …-dev, …-devel).
    for candidate in candidate_names(dep, manager) {
        if let Some(result) = try_candidate(
            config,
            container,
            dep,
            family,
            &candidate,
            mappings,
            failed_candidates,
            db_synced,
            tx,
            &texts,
        ) {
            return Some(result);
        }
    }

    // 4. Tool-Dateidatenbank (Binary heißt anders als das Paket).
    if let MissingDep::Tool(tool) = dep {
        for command in tool_file_database_commands(manager, tool) {
            if ensure_package_database(config, container, tx, db_synced).is_err() {
                continue;
            }
            let Ok(found) = container.exec(&command, tx) else {
                continue;
            };
            for package in packages_from_file_database(&found.text, tool) {
                if let Some(result) = try_candidate(
                    config,
                    container,
                    dep,
                    family,
                    &package,
                    mappings,
                    failed_candidates,
                    db_synced,
                    tx,
                    &texts,
                ) {
                    return Some(result);
                }
            }
        }
    }

    // 5. Generische Suche.
    let search = search_command(manager, dep);
    if !search.is_empty() && ensure_package_database(config, container, tx, db_synced).is_ok() {
        if let Ok(found) = container.exec(&search, tx) {
            if let Some(candidate) = package_from_search(&found.text, &term, manager) {
                return try_candidate(
                    config,
                    container,
                    dep,
                    family,
                    &candidate,
                    mappings,
                    failed_candidates,
                    db_synced,
                    tx,
                    &texts,
                );
            }
        }
    }
    None
}

/// Findet den Include-Pfad eines fehlenden Headers in den Systemverzeichnissen.
fn header_include_dirs(
    container: &BuildContainer,
    header: &str,
    tx: &Sender<BuildEvent>,
) -> Vec<String> {
    let base = Path::new(header)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(header);
    if base.is_empty()
        || !base
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c))
    {
        return Vec::new();
    }
    let command = format!(
        "find /usr/include /usr/local/include -maxdepth 3 -name {} -printf '%h\\n' 2>/dev/null | sort -u",
        shell_quote(base)
    );
    include_dirs_from_command(container, &command, tx)
}

/// Fragt pkg-config nach den Include-Pfaden eines erkannten Moduls.
fn pkgconfig_include_dirs(
    container: &BuildContainer,
    name: &str,
    tx: &Sender<BuildEvent>,
) -> Vec<String> {
    if !valid_package_name(name) {
        return Vec::new();
    }
    let command = format!("pkg-config --cflags {}", shell_quote(name));
    let Ok(output) = container.exec(&command, tx) else {
        return Vec::new();
    };
    output
        .text
        .split_whitespace()
        .filter_map(|token| token.strip_prefix("-I"))
        .filter(|dir| dir.starts_with('/') && *dir != "/usr/include")
        .map(str::to_string)
        .collect()
}

fn include_dirs_from_command(
    container: &BuildContainer,
    command: &str,
    tx: &Sender<BuildEvent>,
) -> Vec<String> {
    let Ok(output) = container.exec(command, tx) else {
        return Vec::new();
    };
    output
        .text
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('/') && !line.contains(' ') && *line != "/usr/include")
        .map(str::to_string)
        .collect()
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
        Ok(output) if output.success => true,
        // Paketmanager-Hooks (z. B. `ConditionNeedsUpdate` unter Arch oder
        // ldconfig unter Debian) koennen den Exit-Code verfaelschen. Wenn das
        // Paket danach installiert ist, gilt die Installation als Erfolg.
        _ => package_is_installed(container, &target.package_manager, package, tx),
    }
}

/// Erkennt den Paketmanager im Container (das Image folgt der Zielfamilie).
fn detect_package_manager(container: &BuildContainer, tx: &Sender<BuildEvent>) -> String {
    let command = "if command -v pacman >/dev/null 2>&1; then echo pacman; \
elif command -v apt-get >/dev/null 2>&1; then echo apt; \
elif command -v dnf >/dev/null 2>&1; then echo dnf; \
elif command -v zypper >/dev/null 2>&1; then echo zypper; fi";
    container
        .exec(command, tx)
        .ok()
        .and_then(|output| first_output_line(&output))
        .unwrap_or_default()
}

/// Paket, dem eine Datei im Container gehoert (leer, wenn unbekannt).
fn package_owner(
    container: &BuildContainer,
    manager: &str,
    path: &str,
    tx: &Sender<BuildEvent>,
) -> String {
    if !path.starts_with('/') || path.contains('\'') {
        return String::new();
    }
    let command = owner_command(manager, path);
    if command.is_empty() {
        return String::new();
    }
    container
        .exec(&command, tx)
        .ok()
        .and_then(|output| first_output_line(&output))
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// Kommando, das den Besitzer einer Datei nennt.
fn owner_command(manager: &str, path: &str) -> String {
    match manager {
        "pacman" => format!("pacman -Qoq {} 2>/dev/null", path),
        "apt" => format!("dpkg -S {} 2>/dev/null | head -n 1", path),
        "dnf" | "zypper" => format!("rpm -qf {} 2>/dev/null", path),
        _ => String::new(),
    }
}

/// Prueft, ob ein Paket im Container installiert ist - unabhaengig davon, was
/// der Installationsbefehl fuer einen Exit-Code geliefert hat.
fn package_is_installed(
    container: &BuildContainer,
    package_manager: &str,
    package: &str,
    tx: &Sender<BuildEvent>,
) -> bool {
    if !valid_package_name(package) {
        return false;
    }
    let command = match package_manager {
        "pacman" => format!("pacman -Q {} 2>/dev/null", shell_quote(package)),
        "apt" => format!(
            "dpkg -s {} 2>/dev/null | grep -q '^Status: install ok installed'",
            shell_quote(package)
        ),
        "dnf" | "zypper" => format!("rpm -q {} 2>/dev/null", shell_quote(package)),
        _ => return false,
    };
    container
        .exec(&command, tx)
        .map(|output| output.success)
        .unwrap_or(false)
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
    // Bereits geheilt (z. B. nach einem früheren Versuch): nichts zu tun.
    if lines[start].trim_start().starts_with('#') {
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
        "[learn] fixed CMake source_group incompatibility in {} automatically",
        error.file
    )));
    true
}

fn execute(config: &mut BuildConfig, tx: &Sender<BuildEvent>) -> Result<BuildOutcome, String> {
    let texts = Texts::new(config.language);
    // 32-Bit-Ziele werden nicht unterstuetzt: lieber sofort und klar abbrechen
    // als ein Paket mit falscher Architektur zu erzeugen.
    let arch = crate::core::packaging::PackageArch::detect();
    if !arch.is_supported() {
        return Err(texts.err_unsupported_arch(&arch.unsupported_label()));
    }
    let mut mappings = MappingStore::load();
    let mut failed_candidates: HashSet<String> = HashSet::new();
    // Bereits erfolgreich installierte Abhängigkeiten dieses Builds. Fehlt eine
    // davon später erneut, hat das Paket das Problem nicht gelöst.
    let mut resolved_deps: HashMap<String, String> = HashMap::new();
    let mut skipped_incomplete: HashSet<String> = HashSet::new();

    step(tx, texts.step_workdir());
    let reused = reusable_workspace(config.workspace.as_deref());
    let work = match &reused {
        Some(path) => {
            let _ = tx.send(BuildEvent::Log(format!(
                "[cache] reusing build workspace: {}",
                path.display()
            )));
            path.clone()
        }
        None => temp_workdir(&config.url)?,
    };
    let _ = fs::remove_file(work.join("FAILED"));
    // Ausgabeordner sofort anlegen – auch wenn der Clone später fehlschlägt.
    fs::create_dir_all(&config.output_dir).map_err(|e| format!("output dir: {}", e))?;

    let src = work.join("src");
    if reused.is_none() {
        prepare_source_workspace(&config.url, &src, tx, &texts)?;
    }
    let _ = tx.send(BuildEvent::Workspace(work.clone()));
    let family = family_key(&config.target_distro.info(&config.distro).family);

    let mut repo_script = config
        .repo_script
        .clone()
        .or_else(|| find_repo_script(&src, family));
    let repo_pkgbuild = src.join("PKGBUILD").is_file();
    let identity = project_identity(&config.url, &src);
    let _ = tx.send(BuildEvent::Log(format!(
        "[project] {} {}",
        identity.name, identity.version
    )));

    let fingerprint = ProjectFingerprint::from_root(&src);
    let mut learned_packages: Vec<String> = Vec::new();
    if let Some((recipe, _exact)) =
        mappings.apply_learned_recipe(&config.url, &fingerprint.signature, family)
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
            "[learn] applied known build recipe for structure '{}'",
            fingerprint.signature
        )));
        if !learned_packages.is_empty() {
            let _ = tx.send(BuildEvent::Log(format!(
                "[learn] pre-installing saved packages for this build: {}",
                learned_packages.join(" ")
            )));
        }
    }

    if config.strategy == BuildStrategy::RepoScript {
        let _ = tx.send(BuildEvent::Log(format!(
            "[strategy] {} - repository script: {}",
            config.strategy.label(),
            repo_script.as_deref().unwrap_or("none found")
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
    // Der Initialwert ist nur ein Platzhalter; jeder Versuch setzt ihn neu.
    #[allow(unused_assignments)]
    let mut last_output = String::new();
    let mut forced_strategy: Option<BuildStrategy> = None;
    // Einmal pro Lauf dürfen die Abhängigkeiten nachinstallierter Pakete neu
    // konfiguriert werden, wenn CMake den alten Stand zwischengespeichert hat.
    let mut deps_reconfigured = false;

    let mut strategy_attempt: u8 = 1;
    let mut visited_strategies: HashSet<&'static str> = HashSet::new();
    let mut heal_round: u8 = 1;
    // Module, die im Container nachweislich vorhanden sind, aber vom
    // projekt-eigenen Find-Modul weiter abgelehnt werden.
    let mut rejected_modules: HashSet<String> = HashSet::new();
    // Die MSRV-Heilung wird pro Lauf hoechstens einmal angewandt.
    let mut toolchain_healed = false;
    // Ebenso die C++-Compiler-Heilung.
    let mut gxx_healed = false;
    let mut allowed_rounds: u8 = INITIAL_HEAL_ROUNDS;
    'attempt: loop {
        if config.cancel_flag.load(Ordering::Relaxed) {
            return Err(abort_running_build(&container, config, tx));
        }
        let round_state = RetryState {
            packages: extra_packages.clone(),
            cmake_flags: cmake_flags.clone(),
            include_dirs: config.include_dirs.clone(),
        };
        let effective_strategy = forced_strategy.unwrap_or(config.strategy);
        let strategy_name = effective_strategy.label();
        step(
            tx,
            texts.step_strategy_round(
                texts.strategy_label(effective_strategy),
                strategy_attempt,
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
            if let Some(next) =
                next_unvisited_strategy(effective_strategy, has_deps, &visited_strategies)
            {
                let _ = tx.send(BuildEvent::Log(format!(
                    "[learn] stored profile marks '{}' as incomplete - trying '{}'",
                    strategy_name,
                    next.label()
                )));
                config.skip_deps = next == BuildStrategy::SystemLibsOnly;
                strategy_attempt += 1;
                heal_round = 1;
                allowed_rounds = INITIAL_HEAL_ROUNDS;
                forced_strategy = Some(next);
                continue 'attempt;
            }
            // Alle Strategien sind markiert: Marker verwerfen und trotzdem
            // einen echten Versuch starten, statt ohne Build abzubrechen.
            let _ = tx.send(BuildEvent::Log(
                "[learn] every strategy is marked incomplete - clearing the stored markers and retrying"
                    .to_string(),
            ));
            mappings.clear_incomplete_strategies(&config.url);
            skipped_incomplete.clear();
            // Von vorn beginnen, statt mit der zuletzt übrig gebliebenen
            // Ausweichstrategie weiterzumachen.
            visited_strategies.clear();
            strategy_attempt += 1;
            heal_round = 1;
            allowed_rounds = INITIAL_HEAL_ROUNDS;
            config.skip_deps = config.strategy == BuildStrategy::SystemLibsOnly;
            forced_strategy = Some(config.strategy);
            continue 'attempt;
        }
        visited_strategies.insert(effective_strategy.key());
        // Nur Stufe 1 baut lokale deps/ vor; alle späteren Stufen nutzen Systemlibs.
        let deps_first = has_deps && effective_strategy == BuildStrategy::Default;
        if deps_first {
            let _ = tx.send(BuildEvent::Log(
                "[deps] 'deps/CMakeLists.txt' detected - building local dependencies first"
                    .to_string(),
            ));
        }
        for flag in cmake_option_flags(&src, effective_strategy) {
            push_unique_string(&mut cmake_flags, flag);
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

        // Ein transienter Netzwerkfehler (HTTP 5xx, Timeout) ist kein Grund,
        // die Strategie zu wechseln. Die Abhängigkeitsquelle bleibt erhalten.
        if deps_failed
            && heal_round < allowed_rounds
            && is_transient_network_error(&tail_lines(&last_output, 120))
        {
            heal_round += 1;
            let _ = tx.send(BuildEvent::Log(
                "[retry] transient network error - retrying dependency download".to_string(),
            ));
            continue 'attempt;
        }

        // Repo-Skripte pruefen oft nur wenige Distributionen. Wenn sie diese
        // Distro ablehnen, generisch weiterbauen statt den Lauf zu beenden.
        if effective_strategy == BuildStrategy::RepoScript
            && detect_repo_script_distro_rejection(&last_output)
        {
            let _ = tx.send(BuildEvent::Log(
                "[fallback] repository build script does not support this distribution - switching to the generic build system"
                    .to_string(),
            ));
            repo_script = None;
            let next = if !visited_strategies.contains(BuildStrategy::Default.key()) {
                Some(BuildStrategy::Default)
            } else {
                next_unvisited_strategy(effective_strategy, has_deps, &visited_strategies)
            };
            let Some(next) = next else {
                break;
            };
            strategy_attempt += 1;
            heal_round = 1;
            allowed_rounds = INITIAL_HEAL_ROUNDS;
            config.skip_deps = false;
            forced_strategy = Some(next);
            continue 'attempt;
        }

        if result.text.contains(OK_MARKER) {
            let verification = verify_built_packages(
                &container,
                config,
                tx,
                is_compiled_build_system(system.label()),
                strategy_name,
                &mut mappings,
            );
            if verification != VerifyOutcome::Ok {
                // Die Prüfung hat das Paket bereits verworfen. Im direkten
                // Fallback gibt es keine weitere Stufe mehr.
                if effective_strategy == BuildStrategy::DirectStagingFallback {
                    mark_failed_build(&work);
                    return Err(
                        "build produced no usable package even with the direct build system"
                            .to_string(),
                    );
                }
                let next =
                    next_unvisited_strategy(effective_strategy, has_deps, &visited_strategies);
                let Some(next) = next else {
                    break;
                };
                strategy_attempt += 1;
                heal_round = 1;
                allowed_rounds = INITIAL_HEAL_ROUNDS;
                config.skip_deps = next == BuildStrategy::SystemLibsOnly;
                forced_strategy = Some(next);
                let _ = tx.send(BuildEvent::Log(format!(
                    "[strategy] {} (verification failed)",
                    next.label()
                )));
                continue;
            }
            if config.cancel_flag.load(Ordering::Relaxed) {
                return Err(abort_running_build(&container, config, tx));
            }
            if config.target == TargetFormat::NativePackage {
                let recipe = BuildRecipe {
                    signature: fingerprint.signature.clone(),
                    family: family.to_string(),
                    strategy: effective_strategy.key().to_string(),
                    skip_deps: config.skip_deps,
                    cmake_compat: config.cmake_compat,
                    cstdint_fix: config.cstdint_fix,
                    submodule_fix: config.submodule_fix,
                    packages: extra_packages.clone(),
                    verified: false,
                };
                mappings.record_build_recipe(&config.url, recipe);
                mappings.record_repo_packages(
                    &config.url,
                    &fingerprint.signature,
                    family,
                    &extra_packages,
                );
                mappings.clear_incomplete_strategy(&config.url, strategy_name);
                let _ = tx.send(BuildEvent::Log(format!(
                    "[learn] saved verified build recipe for structure '{}'",
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
                "[cmake] retry flags taken from the error message: {}",
                discovered_flags.join(" ")
            )));
            for flag in discovered_flags {
                push_unique_string(&mut cmake_flags, flag);
            }
            heal_round += 1;
            if heal_round > allowed_rounds {
                let Some(next) =
                    next_unvisited_strategy(effective_strategy, has_deps, &visited_strategies)
                else {
                    break;
                };
                strategy_attempt += 1;
                heal_round = 1;
                allowed_rounds = INITIAL_HEAL_ROUNDS;
                config.skip_deps = next == BuildStrategy::SystemLibsOnly;
                forced_strategy = Some(next);
                let _ = tx.send(BuildEvent::Log(format!(
                    "[strategy] {} (self-healing rounds exhausted)",
                    next.label()
                )));
            }
            continue;
        }

        if let Some(error) = detect_source_group_error(&last_output) {
            if heal_source_group(&work, &error, tx) {
                heal_round += 1;
                if heal_round > allowed_rounds {
                    let Some(next) =
                        next_unvisited_strategy(effective_strategy, has_deps, &visited_strategies)
                    else {
                        break;
                    };
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
                    "[learn] error pattern detected -> fix applied and saved: {} -> {}",
                    fix.description, fix.value
                )));
            }
        }
        config.cmake_compat |= mappings.build_fix("cmake_policy").is_some();
        config.cstdint_fix = mappings.build_fix("cstdint").is_some();
        config.submodule_fix = mappings.build_fix("submodules").is_some();
        let cargo_project = system == BuildSystem::Cargo;
        if !config.rustup_fix && cargo_project && mappings.build_fix("rustup_toolchain").is_some() {
            let _ = tx.send(BuildEvent::Log(
                "[heal] outdated rustc toolchain detected - installing stable via rustup"
                    .to_string(),
            ));
        }
        config.rustup_fix = cargo_project && mappings.build_fix("rustup_toolchain").is_some();
        config.newer_gxx |= mappings.build_fix("newer_gxx").is_some();

        let missing = analyze_output(&last_output);
        if missing.is_empty() {
            // Der Toolchain-Fix wurde gerade erst gelernt; das laufende Skript
            // kennt ihn noch nicht. Einmal mit rustup wiederholen, statt mit
            // einer nichtssagenden Diagnose abzubrechen.
            if !toolchain_healed
                && config.rustup_fix
                && classify_failure(&last_output) == FailureKind::Toolchain
            {
                toolchain_healed = true;
                let _ = tx.send(BuildEvent::Log(
                    "[heal] retrying with a current stable toolchain (rustup)".to_string(),
                ));
                continue;
            }
            if !gxx_healed && config.newer_gxx && is_compiler_too_old(&last_output) {
                gxx_healed = true;
                let _ = tx.send(BuildEvent::Log(
                    "[heal] retrying with the newest available C++ compiler".to_string(),
                ));
                continue;
            }
            if deps_failed {
                let _ = tx.send(BuildEvent::Log(
                    "[warn] local deps/ prebuild failed - switching to the distribution's system libraries..."
                        .to_string(),
                ));
                let Some(next) =
                    next_unvisited_strategy(effective_strategy, has_deps, &visited_strategies)
                else {
                    break;
                };
                config.skip_deps = next == BuildStrategy::SystemLibsOnly;
                strategy_attempt += 1;
                heal_round = 1;
                allowed_rounds = INITIAL_HEAL_ROUNDS;
                forced_strategy = Some(next);
                continue 'attempt;
            }
            mark_failed_build(&work);
            let kind = classify_failure(&last_output);
            let _ = tx.send(BuildEvent::Log(format!(
                "[failure] {} - {}",
                kind.label(),
                kind.hint()
            )));
            return Err(texts.err_failure_class(kind.label()));
        }

        if let Some(detail) = detect_abi_mismatch(&last_output) {
            let _ = tx.send(BuildEvent::Log(format!(
                "[failure] version/ABI mismatch - installing packages cannot help: {}",
                detail
            )));
            mark_failed_build(&work);
            return Err(texts.err_failure_class("version/ABI mismatch"));
        }

        let mut unresolved: Option<MissingDep> = None;
        // Hat diese Runde überhaupt etwas verändert? Nur dann lohnt ein
        // erneuter Buildversuch trotz unauflösbarer (z. B. optionaler) Deps.
        let mut round_progress = false;
        let mut resolved_now = 0usize;
        for dep in &missing {
            if let MissingDep::FinderBroken(module) = dep {
                // Kein Paket raten: Der Finder des Projekts parst die Version
                // falsch. Stattdessen den installierten Config-Modus erzwingen.
                let before = cmake_flags.len();
                for flag in finder_broken_flags(&container, module, tx) {
                    push_unique_string(&mut cmake_flags, flag);
                }
                if cmake_flags.len() > before {
                    round_progress = true;
                }
                // Eine frühere Auflösung desselben Moduls darf jetzt nicht als
                // Fehlschlag abgewertet werden.
                resolved_deps.remove(&dependency_id(&MissingDep::CMakeModule(module.clone())));
                let _ = tx.send(BuildEvent::Log(format!(
                    "[cmake] finder for '{}' reports a broken version probe - using CMake config mode",
                    module
                )));
                continue;
            }
            // Fehlt ein Header, obwohl das Paket installiert ist, liegt er oft
            // in einem Unterordner von /usr/include. Diesen Pfad ergänzen.
            if let MissingDep::Header(header) = dep {
                for dir in header_include_dirs(&container, header, tx) {
                    if !config.include_dirs.contains(&dir) {
                        let _ = tx.send(BuildEvent::Log(format!(
                            "[include] header '{}' found in {} - adding include path",
                            header, dir
                        )));
                        config.include_dirs.push(dir);
                        round_progress = true;
                    }
                }
            }
            if let MissingDep::PkgConfig(name) = dep {
                for dir in pkgconfig_include_dirs(&container, name, tx) {
                    if !config.include_dirs.contains(&dir) {
                        let _ = tx.send(BuildEvent::Log(format!(
                            "[include] pkg-config '{}' adds include path {}",
                            name, dir
                        )));
                        config.include_dirs.push(dir);
                        round_progress = true;
                    }
                }
            }
            step(tx, texts.step_search(&dep.label()));
            let target = config.target_distro.info(&config.distro);
            let family = family_key(&target.family);
            let manager = &target.package_manager;

            let dep_id = dependency_id(dep);
            if let Some(previous) = resolved_deps.remove(&dep_id) {
                if let MissingDep::CMakeModule(name) = dep {
                    rejected_modules.insert(name.clone());
                }
                if should_penalize_mapping(dep) {
                    let _ = tx.send(BuildEvent::Log(format!(
                        "[learn] '{}' is still missing after installing '{}' - scoring the mapping down",
                        dep.label(),
                        previous
                    )));
                    mappings.penalize_dependency(family, dep, &previous);
                }
                failed_candidates.insert(previous);
            }

            let Some((package, evidence)) = resolve_dependency(
                config,
                &container,
                dep,
                family,
                manager,
                &mut mappings,
                &mut failed_candidates,
                &mut db_synced,
                tx,
            ) else {
                // Eine unauflösbare Abhängigkeit darf die späteren, vielleicht
                // lösbaren nicht blockieren.
                if unresolved.is_none() {
                    unresolved = Some(dep.clone());
                }
                continue;
            };
            allowed_rounds = max_heal_rounds();
            let verified = !evidence.is_empty();
            mappings.learn_dependency(family, dep, &package, verified, &evidence);
            let _ = tx.send(BuildEvent::Log(format!(
                "[learn] {} -> {} | verified={} evidence={}",
                dep.raw_name(),
                package,
                verified,
                evidence
            )));
            if matches!(dep, MissingDep::PkgConfig(_)) {
                let _ = tx.send(BuildEvent::Log(format!(
                    "[learn] error pattern detected -> fix applied and saved: pkg-config {} -> {}",
                    dep.raw_name(),
                    package
                )));
            }
            resolved_deps.insert(dep_id, package.clone());
            round_progress = true;
            resolved_now += 1;
            if !extra_packages.contains(&package) {
                extra_packages.push(package.clone());
            }
        }
        if resolved_now > 0 {
            let _ = tx.send(BuildEvent::Log(format!(
                "[retry] round {}/{} - {} new dependencies resolved",
                heal_round, allowed_rounds, resolved_now
            )));
        }
        if let Some(dep) = unresolved {
            if deps_failed {
                // Wenn in dieser Runde neue Pakete oder Flags dazugekommen sind,
                // lohnt ein erneuter Versuch derselben Strategie: Der neue Stand
                // kann genau die vorher fehlende Abhängigkeit liefern.
                let after_state = RetryState {
                    packages: extra_packages.clone(),
                    cmake_flags: cmake_flags.clone(),
                    include_dirs: config.include_dirs.clone(),
                };
                if retry_state_changed(&round_state, &after_state) && heal_round < allowed_rounds {
                    heal_round += 1;
                    let _ = reset_cmake_caches(&work);
                    reset_dependency_configure_stamps(&work);
                    let _ = tx.send(BuildEvent::Log(
                        "[retry] installed new packages - retrying the dependency build"
                            .to_string(),
                    ));
                    continue 'attempt;
                }
                let _ = tx.send(BuildEvent::Log(
                    "[warn] local deps/ prebuild failed - switching to the distribution's system libraries..."
                        .to_string(),
                ));
                let Some(next) =
                    next_unvisited_strategy(effective_strategy, has_deps, &visited_strategies)
                else {
                    break;
                };
                config.skip_deps = next == BuildStrategy::SystemLibsOnly;
                strategy_attempt += 1;
                heal_round = 1;
                allowed_rounds = INITIAL_HEAL_ROUNDS;
                forced_strategy = Some(next);
                continue 'attempt;
            }
            if !round_progress {
                mark_failed_build(&work);
                if let MissingDep::CMakeModule(name) = &dep {
                    if rejected_modules.contains(name) {
                        let _ = tx.send(BuildEvent::Log(format!(
                            "[failure] CMake module '{}' rejected by the project - a library or header is installed, but its finder still fails (version or components)",
                            name
                        )));
                        return Err(texts.err_module_rejected(name));
                    }
                }
                return Err(texts.err_no_package(&dep.label()));
            }
            // Andere Abhängigkeiten wurden gelöst: erst erneut bauen. Viele
            // CMake-Module sind optional, ein fehlendes darf den Build nicht
            // verhindern, solange er noch erfolgreich werden kann.
            let _ = tx.send(BuildEvent::Log(format!(
                "[warn] could not resolve {} - retrying anyway (optional dependencies are not fatal)",
                dep.label()
            )));
        }

        let cleared_cmake = reset_cmake_caches(&work);
        if cleared_cmake > 0 {
            let _ = tx.send(BuildEvent::Log(
                "[cmake] removed CMakeCache.txt and CMakeFiles after installing packages"
                    .to_string(),
            ));
        }

        let after_state = RetryState {
            packages: extra_packages.clone(),
            cmake_flags: cmake_flags.clone(),
            include_dirs: config.include_dirs.clone(),
        };
        if !retry_state_changed(&round_state, &after_state) {
            // Pakete sind installiert, aber CMake hat den alten Stand noch im
            // Cache (z. B. ein Paket mit veralteten Include-Pfaden). Einmalig
            // die Configure-Stempel der Abhängigkeiten verwerfen.
            if deps_failed && !deps_reconfigured {
                let removed = reset_dependency_configure_stamps(&work);
                if removed > 0 {
                    deps_reconfigured = true;
                    heal_round += 1;
                    let _ = reset_cmake_caches(&work);
                    let _ = tx.send(BuildEvent::Log(format!(
                        "[retry] reconfiguring {} dependency project(s) after package installation",
                        removed
                    )));
                    continue;
                }
            }
            let Some(next) =
                next_unvisited_strategy(effective_strategy, has_deps, &visited_strategies)
            else {
                break;
            };
            strategy_attempt += 1;
            heal_round = 1;
            allowed_rounds = INITIAL_HEAL_ROUNDS;
            config.skip_deps = next == BuildStrategy::SystemLibsOnly;
            forced_strategy = Some(next);
            let _ = tx.send(BuildEvent::Log(format!(
                "[retry] no new packages or flags - switching to strategy {}",
                next.label()
            )));
            continue;
        }

        if heal_round >= allowed_rounds {
            let Some(next) =
                next_unvisited_strategy(effective_strategy, has_deps, &visited_strategies)
            else {
                break;
            };
            strategy_attempt += 1;
            heal_round = 1;
            allowed_rounds = INITIAL_HEAL_ROUNDS;
            config.skip_deps = next == BuildStrategy::SystemLibsOnly;
            forced_strategy = Some(next);
            let _ = tx.send(BuildEvent::Log(format!(
                "[strategy] {} (self-healing rounds exhausted)",
                next.label()
            )));
        } else {
            heal_round += 1;
        }
    }

    mark_failed_build(&work);
    Err(texts.err_max_attempts(
        visited_strategies.len().max(1) as u8,
        &tail_lines(&last_output, 80),
    ))
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

/// Shell-Funktion, die nur existierende lokale Superbuild-Prefixe als
/// `CMAKE_PREFIX_PATH` übergibt. Wird erst zur Laufzeit im Container
/// ausgewertet, damit ein nicht existierender Pfad nie in CMake landet.
fn cmake_prefix_flag_function() -> String {
    r#"g2p_cmake_prefix_flag() {
  local out=""
  local dir
  for dir in /work/deps-install /work/src/deps-install /work/install /work/src/install; do
    [ -d "$dir" ] || continue
    if [ -z "$out" ]; then
      out="-DCMAKE_PREFIX_PATH=$dir"
    else
      out="$out;$dir"
    fi
  done
  printf '%s' "$out"
}
"#
    .to_string()
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
            format!(
                "for sub in /work/src/deps /work/src/third_party; do if [ -f \"$sub/CMakeLists.txt\" ]; then echo '[deps] reconfiguring local dependencies (downloaded sources are kept)'; rm -f /work/deps-build/CMakeCache.txt; rm -rf /work/deps-build/CMakeFiles; cmake -S \"$sub\" -B /work/deps-build -DCMAKE_INSTALL_PREFIX=/work/deps-install {configure_flags} && cmake --build /work/deps-build && cmake --install /work/deps-build || {{ echo '[deps] build failed - resolver takes over'; exit 1; }}; fi; done"
            ),
        );
    }
    lines.join("\n")
}

/// Standard-Libdir der Ziel-Distribution (RPM-Distributionen nutzen lib64).
fn target_libdir(family: &DistroFamily) -> &'static str {
    match family {
        DistroFamily::Fedora | DistroFamily::Suse => "/usr/lib64",
        _ => "/usr/lib",
    }
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
        BuildSystem::Autotools => match family {
            DistroFamily::Arch
            | DistroFamily::Debian
            | DistroFamily::Fedora
            | DistroFamily::Suse => packages.extend(["autoconf", "automake", "libtool", "gettext"]),
            DistroFamily::Unknown => {}
        },
        _ => {}
    }
    packages
}

/// Zusaetzliche Pakete fuer Upstream-Bootstrap-Builds (`x.py`): sie laden eine
/// Stage0-Toolchain und ein LLVM-Archiv und brauchen curl, xz, zlib und OpenSSL.
fn bootstrap_packages(family: &DistroFamily) -> Vec<String> {
    let names: &[&str] = match family {
        DistroFamily::Arch => &["curl", "xz", "zlib", "openssl"],
        DistroFamily::Debian => &["curl", "xz-utils", "zlib1g-dev", "libssl-dev"],
        DistroFamily::Fedora => &["curl", "xz", "zlib-devel", "openssl-devel"],
        DistroFamily::Suse => &["curl", "xz", "zlib-devel", "libopenssl-devel"],
        DistroFamily::Unknown => &[],
    };
    names.iter().map(|name| (*name).to_string()).collect()
}

fn base_dependencies(family: &DistroFamily, system: &BuildSystem) -> String {
    let base = match family {
        DistroFamily::Arch => {
            "pacman -Syu --noconfirm --needed -- base-devel git pkgconf python python-pip cmake meson ninja"
        }
        DistroFamily::Debian => {
            "apt-get update -y && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends build-essential git pkg-config python3-pip cmake meson ninja-build debhelper devscripts fakeroot dpkg-dev"
        }
        DistroFamily::Fedora => {
            // `@development-tools` bringt in neueren Fedora-Versionen keinen
            // C++-Compiler mehr mit; CMake-Projekte brauchen ihn zwingend.
            "dnf install -y @development-tools gcc gcc-c++ make git pkgconf-pkg-config python3-pip cmake meson ninja-build rpm-build rpmdevtools dnf-plugins-core"
        }
        DistroFamily::Suse => {
            "zypper --non-interactive install git pkgconf-pkg-config python3 python3-pip gcc gcc-c++ make cmake meson ninja rpm-build"
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

/// Installiert den neuesten verfuegbaren C++-Compiler und stellt ihn als
/// `CC`/`CXX` bereit. Aeltere Distributionen bringen moderne C++23-Projekte
/// nicht mit dem Standard-Compiler zusammen, bieten aber neuere Pakete an
/// (Ubuntu 24.04 liefert g++-13 als Standard, g++-14 als Paket).
fn newer_gxx_step(family: &DistroFamily) -> Vec<String> {
    let install = match family {
        DistroFamily::Debian => {
            "pkg=$(apt-cache search --names-only '^g\\+\\+-[0-9]+$' 2>/dev/null | awk '{print $1}' | sort -V | tail -n 1); [ -n \"$pkg\" ] && DEBIAN_FRONTEND=noninteractive apt-get install -y \"$pkg\""
        }
        DistroFamily::Fedora => "dnf install -y gcc-c++",
        DistroFamily::Suse => "zypper --non-interactive install gcc gcc-c++",
        DistroFamily::Arch | DistroFamily::Unknown => "true",
    };
    vec![
        echo_line("[build] looking for a newer C++ compiler"),
        format!(
            "if ! ( {} ); then echo '[pkg] newer compiler unavailable - continuing' >&2; fi",
            install
        ),
        r#"for candidate in $(ls /usr/bin/g++-[0-9]* 2>/dev/null | sort -V -r); do
  export CXX="$candidate"
  export CC="${candidate//g++/gcc}"
  echo "[build] using $candidate for C++"
  break
done"#
            .to_string(),
    ]
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

/// Name, Version und redaktionelle Metadaten des gebauten Projekts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProjectIdentity {
    pub name: String,
    pub version: String,
    pub description: String,
    pub license: String,
    pub url: String,
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

fn autotools_version(text: &str) -> Option<String> {
    let ac_init =
        Regex::new(r#"(?is)AC_INIT\s*\(\s*\[?[^,\]]+\]?\s*,\s*\[?([0-9][0-9A-Za-z._\-+]*)"#)
            .ok()?;
    let package_version =
        Regex::new(r#"(?m)PACKAGE_VERSION=['"]?([0-9][0-9A-Za-z._\-+]*)"#).ok()?;
    ac_init
        .captures(text)
        .or_else(|| package_version.captures(text))
        .map(|caps| caps[1].to_string())
}

fn cmake_version(text: &str) -> Option<String> {
    let re = Regex::new(r"(?is)project\s*\([^)]*?\bVERSION\s+([0-9][0-9A-Za-z.\-+]*)").unwrap();
    re.captures(text).map(|caps| caps[1].to_string())
}

fn git_version(src: &Path) -> Option<String> {
    let git = |args: &[&str]| -> Option<String> {
        let output = Command::new("git")
            .arg("-C")
            .arg(src)
            .args(args)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    };
    // Ein Tag direkt am HEAD ist die genaueste Aussage.
    if let Some(tag) = git(&["describe", "--tags", "--exact-match", "HEAD"]) {
        return Some(tag);
    }
    // Sonst der naechste erreichbare Tag (nur bei vollstaendiger Historie).
    if let Some(tag) = git(&["describe", "--tags", "--abbrev=0"]) {
        return Some(tag);
    }
    // Shallow-Clone: HEAD liegt in keiner getaggten Historie, aber die Tags
    // selbst sind vorhanden. Dann gilt der neueste Versions-Tag.
    git(&["tag", "--sort=-v:refname"]).and_then(|tags| pick_version_tag(&tags))
}

/// Waehlt aus der Tag-Liste die plausibelste Projektversion.
///
/// Kriterien in dieser Reihenfolge: hoehere Versionsnummer (damit gewinnt
/// `1.102.0` gegen `release-0.7`), stabile Version vor `alpha`/`beta`/`rc`,
/// dann kurzes Namenspraefix ("v2.0.0" vor "release-2.0.0").
fn pick_version_tag(tags: &str) -> Option<String> {
    let re = Regex::new(r"(?i)^([a-z][a-z0-9_-]*[-_]?)?v?(\d+)(?:\.(\d+))?(?:\.(\d+))?").ok()?;
    let mut best: Option<(usize, Vec<u32>, bool, String)> = None;
    for tag in tags.lines().map(str::trim).filter(|tag| !tag.is_empty()) {
        let Some(caps) = re.captures(tag) else {
            continue;
        };
        let prefix_len = caps.get(1).map(|m| m.as_str().len()).unwrap_or(0);
        let numbers: Vec<u32> = (2..=4)
            .filter_map(|index| caps.get(index))
            .filter_map(|m| m.as_str().parse().ok())
            .collect();
        if numbers.len() < 2 {
            continue;
        }
        let candidate = (prefix_len, numbers, !is_prerelease(tag), tag.to_string());
        if best.as_ref().map(|current| better_tag(&candidate, current)) != Some(false) {
            best = Some(candidate);
        }
    }
    best.map(|(_, _, _, tag)| tag)
}

/// `true`, wenn `candidate` den bisherigen Kandidaten schlaegt.
fn better_tag(
    candidate: &(usize, Vec<u32>, bool, String),
    current: &(usize, Vec<u32>, bool, String),
) -> bool {
    if candidate.1 != current.1 {
        return candidate.1 > current.1;
    }
    if candidate.2 != current.2 {
        return candidate.2;
    }
    if candidate.0 != current.0 {
        return candidate.0 < current.0;
    }
    candidate.3 > current.3
}

fn is_prerelease(tag: &str) -> bool {
    let lower = tag.to_ascii_lowercase();
    ["alpha", "beta", "rc", "pre", "dev", "nightly"]
        .iter()
        .any(|marker| lower.contains(marker))
}

/// Erkennt die Lizenz an den Lizenzdateien im Projektwurzelverzeichnis.
///
/// Viele Projekte deklarieren ihre Lizenz nur in `LICENSE`/`COPYING` und nicht
/// in einem Manifest. Mehrere Treffer (duale Lizenz) werden in fester
/// Reihenfolge mit ` OR ` verbunden.
fn license_from_files(src: &Path) -> Option<String> {
    let Ok(entries) = fs::read_dir(src) else {
        return None;
    };
    let mut files: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().to_ascii_uppercase())
                .unwrap_or_default();
            name.starts_with("LICENSE")
                || name.starts_with("LICENCE")
                || name.starts_with("COPYING")
                || name.starts_with("COPYRIGHT")
        })
        .collect();
    files.sort();

    let mut found: Vec<&'static str> = Vec::new();
    for path in files.iter().take(6) {
        let Ok(bytes) = fs::read(path) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes[..bytes.len().min(8192)]).to_ascii_lowercase();
        if let Some(id) = spdx_from_text(&text) {
            if !found.contains(&id) {
                found.push(id);
            }
        }
    }
    if found.is_empty() {
        None
    } else {
        Some(found.join(" OR "))
    }
}

fn spdx_from_text(text: &str) -> Option<&'static str> {
    let has = |needle: &str| text.contains(needle);
    if has("gnu affero general public license") && has("version 3") {
        return Some("AGPL-3.0-or-later");
    }
    if has("gnu lesser general public license") && has("version 3") {
        return Some("LGPL-3.0-or-later");
    }
    if has("gnu lesser general public license") && has("version 2.1") {
        return Some("LGPL-2.1-or-later");
    }
    if has("gnu general public license") && has("version 3") {
        return Some("GPL-3.0-or-later");
    }
    if has("gnu general public license") && has("version 2") {
        return Some("GPL-2.0-or-later");
    }
    if has("apache license") && has("version 2.0") {
        return Some("Apache-2.0");
    }
    if has("mozilla public license") && has("2.0") {
        return Some("MPL-2.0");
    }
    if has("permission is hereby granted, free of charge") {
        return Some("MIT");
    }
    if has("redistribution and use in source and binary forms") && has("neither the name") {
        return Some("BSD-3-Clause");
    }
    if has("redistribution and use in source and binary forms") {
        return Some("BSD-2-Clause");
    }
    if has("this is free and unencumbered software released into the public domain") {
        return Some("Unlicense");
    }
    if has("isc license") {
        return Some("ISC");
    }
    None
}

fn toml_string_field(text: &str, key: &str) -> Option<String> {
    let pattern = format!(r#"(?m)^\s*{}\s*=\s*"([^"]*)""#, regex::escape(key));
    Regex::new(&pattern)
        .ok()?
        .captures(text)
        .map(|caps| metadata_text(&caps[1]))
        .filter(|value| !value.is_empty())
}

fn yaml_string_field(text: &str, key: &str) -> Option<String> {
    let pattern = format!(r#"(?m)^\s*{}\s*:\s*([^|>#\n][^\n#]*)$"#, regex::escape(key));
    Regex::new(&pattern)
        .ok()?
        .captures(text)
        .map(|caps| metadata_text(caps[1].trim().trim_matches('"')))
        .filter(|value| !value.is_empty())
}

fn yaml_description(text: &str) -> Option<String> {
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("description:") {
            let rest = rest.trim();
            if !rest.is_empty() && !matches!(rest, "|" | "|-" | ">" | ">-") {
                let value = metadata_text(rest.trim_matches('"'));
                if !value.is_empty() {
                    return Some(value);
                }
            }
            for next in lines.by_ref() {
                let value = metadata_text(next);
                if !value.is_empty() {
                    return Some(value);
                }
            }
        }
    }
    None
}

fn metadata_text(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn desktop_comment(src: &Path) -> Option<String> {
    fn walk(dir: &Path, depth: usize) -> Option<String> {
        if depth > 4 {
            return None;
        }
        let entries = fs::read_dir(dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy())
                    .unwrap_or_default();
                if matches!(
                    name.as_ref(),
                    ".git" | "target" | "build" | "dist" | "deps-build" | "node_modules"
                ) {
                    continue;
                }
                if let Some(comment) = walk(&path, depth + 1) {
                    return Some(comment);
                }
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("desktop") {
                if let Ok(text) = fs::read_to_string(&path) {
                    for line in text.lines() {
                        if let Some(value) = line.strip_prefix("Comment=") {
                            let value = metadata_text(value);
                            if !value.is_empty() {
                                return Some(value);
                            }
                        }
                    }
                }
            }
        }
        None
    }
    walk(src, 0)
}

fn git_remote_url(src: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(src)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn nfpm_metadata(src: &Path) -> Option<(Option<String>, Option<String>, Option<String>)> {
    for relative in [
        "packaging/linux/nfpm.yaml",
        "nfpm.yaml",
        "packaging/nfpm.yaml",
    ] {
        if let Ok(text) = fs::read_to_string(src.join(relative)) {
            return Some((
                yaml_description(&text).or_else(|| yaml_string_field(&text, "description")),
                yaml_string_field(&text, "license"),
                yaml_string_field(&text, "homepage"),
            ));
        }
    }
    None
}

fn http_url(value: Option<String>) -> Option<String> {
    value.filter(|url| url.starts_with("https://") || url.starts_with("http://"))
}

/// Paketname aus `debian/control` (`Source:` zuerst, dann `Package:`).
/// Name und Version aus der ersten `.spec` im Projektwurzelverzeichnis.
fn spec_identity(src: &Path) -> Option<(String, String)> {
    let entries = fs::read_dir(src).ok()?;
    let mut specs: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .map(|ext| ext.eq_ignore_ascii_case("spec"))
                    .unwrap_or(false)
        })
        .collect();
    specs.sort();
    let text = fs::read_to_string(specs.first()?).ok()?;
    let field = |key: &str| -> Option<String> {
        let pattern = format!(r"(?mi)^\s*{}\s*:\s*([^\s#]+)", regex::escape(key));
        Regex::new(&pattern)
            .ok()?
            .captures(&text)
            .map(|caps| caps[1].to_string())
    };
    let name = field("Name")?;
    let version = field("Version")?;
    Some((name, version))
}

fn debian_package_name(text: &str) -> Option<String> {
    let re = Regex::new(r"(?m)^(?:Source|Package):\s*([A-Za-z0-9][A-Za-z0-9+.-]*)").ok()?;
    re.captures(text).map(|caps| caps[1].to_string())
}

/// Version aus der ersten Zeile von `debian/changelog`.
fn debian_changelog_version(text: &str) -> Option<String> {
    let first = text.lines().next()?;
    let re = Regex::new(r"^\S+\s+\(([^)]+)\)").ok()?;
    re.captures(first).map(|caps| caps[1].to_string())
}

fn project_identity(source_url: &str, src: &Path) -> ProjectIdentity {
    // Bringt das Projekt eine eigene Debian-Paketierung mit, sind deren Angaben
    // verbindlich - so heisst das Paket wie beim Distributions-Paketbauer.
    let debian_name = if src.as_os_str().is_empty() {
        None
    } else {
        fs::read_to_string(src.join("debian/control"))
            .ok()
            .and_then(|text| debian_package_name(&text))
    };
    let spec = if src.as_os_str().is_empty() {
        None
    } else {
        spec_identity(src)
    };
    let name = debian_name
        .map(|raw| package_slug(&raw))
        .or_else(|| spec.as_ref().map(|(name, _)| package_slug(name)))
        .or_else(|| repo_name_from_url(source_url).map(|raw| package_slug(&raw)))
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
        let from_autotools = ["configure.ac", "configure"].iter().find_map(|file| {
            fs::read_to_string(src.join(file))
                .ok()
                .and_then(|text| autotools_version(&text))
        });
        let from_debian = fs::read_to_string(src.join("debian/changelog"))
            .ok()
            .and_then(|text| debian_changelog_version(&text));
        let from_spec = spec.as_ref().map(|(_, version)| version.clone());
        from_debian
            .or(from_spec)
            .or(from_cargo)
            .or(from_cmake)
            .or(from_python)
            .or(from_autotools)
            .or_else(|| git_version(src))
    }
    .map(|raw| version_slug(&raw))
    .unwrap_or_else(|| "0.1.0".to_string());

    let cargo = if src.as_os_str().is_empty() {
        None
    } else {
        fs::read_to_string(src.join("Cargo.toml")).ok()
    };
    let pyproject = if src.as_os_str().is_empty() {
        None
    } else {
        fs::read_to_string(src.join("pyproject.toml")).ok()
    };
    let (nfpm_description, nfpm_license, nfpm_url) = if src.as_os_str().is_empty() {
        (None, None, None)
    } else {
        nfpm_metadata(src).unwrap_or((None, None, None))
    };
    let description = nfpm_description
        .or_else(|| {
            cargo
                .as_deref()
                .and_then(|text| toml_string_field(text, "description"))
        })
        .or_else(|| {
            pyproject
                .as_deref()
                .and_then(|text| toml_string_field(text, "description"))
        })
        .or_else(|| {
            if src.as_os_str().is_empty() {
                None
            } else {
                desktop_comment(src)
            }
        })
        .unwrap_or_else(|| "Built by git2pkg".to_string());
    let license = nfpm_license
        .or_else(|| {
            cargo
                .as_deref()
                .and_then(|text| toml_string_field(text, "license"))
        })
        .or_else(|| {
            pyproject
                .as_deref()
                .and_then(|text| toml_string_field(text, "license"))
        })
        .or_else(|| {
            if src.as_os_str().is_empty() {
                None
            } else {
                license_from_files(src)
            }
        })
        .unwrap_or_else(|| "unknown".to_string());
    let url = http_url(nfpm_url)
        .or_else(|| {
            cargo
                .as_deref()
                .and_then(|text| toml_string_field(text, "repository"))
        })
        .or_else(|| {
            cargo
                .as_deref()
                .and_then(|text| toml_string_field(text, "homepage"))
        })
        .or_else(|| {
            if src.as_os_str().is_empty() {
                None
            } else {
                http_url(git_remote_url(src))
            }
        })
        .or_else(|| {
            if source_url.starts_with("https://") || source_url.starts_with("http://") {
                Some(source_url.to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "https://github.com/hyalcin70/git2pkg".to_string());

    ProjectIdentity {
        name,
        version,
        description,
        license,
        url,
    }
}

/// Paketinstallationsschritt mit dem echten Projektnamen.
fn project_install_step(system: &BuildSystem, name: &str) -> String {
    let step = match system {
        BuildSystem::Cargo => format!(
            "{}{}",
            crate::core::packaging::bootstrap_install_guard(),
            crate::core::packaging::cargo_install_step(name)
        ),
        BuildSystem::CMake => {
            r#"cmake --build build -j$(nproc)
if [ -f build/cmake_install.cmake ]; then DESTDIR="$pkgdir" cmake --install build; fi
if [ -z "$(find "$pkgdir/usr" -type f -perm -u+x -print -quit 2>/dev/null)" ]; then
  echo '[pkg] no install rules - staging built binaries' >&2
  find build -maxdepth 4 -type f -perm -u+x ! -path '*/CMakeFiles/*' -exec sh -c 'case "$1" in *.a|*.o) exit 0;; *.so|*.so.*) install -Dm755 "$1" "$2/$(basename "$1")"; exit 0;; esac; head -c 4 "$1" | grep -q ELF || exit 0; install -Dm755 "$1" "$3/$(basename "$1")"' _ {} "$pkgdir/usr/lib" "$pkgdir/usr/bin" \;
fi"#
            .to_string()
        }
        BuildSystem::Meson => {
            r#"DESTDIR="$pkgdir" ninja -C build install
if [ -z "$(find "$pkgdir/usr" -type f -perm -u+x -print -quit 2>/dev/null)" ]; then
  echo '[pkg] no install rules - staging built binaries' >&2
  find build -maxdepth 4 -type f -perm -u+x ! -path '*/CMakeFiles/*' -exec sh -c 'case "$1" in *.a|*.o) exit 0;; *.so|*.so.*) install -Dm755 "$1" "$2/$(basename "$1")"; exit 0;; esac; head -c 4 "$1" | grep -q ELF || exit 0; install -Dm755 "$1" "$3/$(basename "$1")"' _ {} "$pkgdir/usr/lib" "$pkgdir/usr/bin" \;
fi"#
            .to_string()
        }
        BuildSystem::Go => {
            "for binary in /work/dist/*; do [ -f \"$binary\" ] && install -Dm755 \"$binary\" \"$pkgdir/usr/bin/$(basename \"$binary\")\"; done".to_string()
        }
        BuildSystem::Python => {
            "python3 -m pip install --no-deps --prefix=\"$pkgdir/usr\" .".to_string()
        }
        BuildSystem::Autotools => {
            "if ! make DESTDIR=\"$pkgdir\" install; then echo '[pkg] make install unavailable - staging built binaries' >&2; for binary in /work/dist/*; do [ -f \"$binary\" ] && install -Dm755 \"$binary\" \"$pkgdir/usr/bin/$(basename \"$binary\")\"; done; fi".to_string()
        }
        BuildSystem::Make => {
            "if ! make DESTDIR=\"$pkgdir\" PREFIX=/usr install; then echo '[pkg] make install unavailable - staging built binaries' >&2; for binary in /work/dist/*; do [ -f \"$binary\" ] && install -Dm755 \"$binary\" \"$pkgdir/usr/bin/$(basename \"$binary\")\"; done; fi".to_string()
        }
        BuildSystem::Qmake => "make INSTALL_ROOT=\"$pkgdir\" install".to_string(),
        BuildSystem::Unknown => "true".to_string(),
    };
    format!(
        "mkdir -p \"$pkgdir/usr/bin\" \"$pkgdir/usr/lib\" \"$pkgdir/usr/share\"\n{}\n{}\n{}",
        step,
        crate::core::packaging::strip_binaries_step(),
        dev_artifacts_cleanup_step()
    )
}

/// Anwendungspakete sollen keine Entwicklungsdateien ausliefern. Header,
/// statische Bibliotheken und pkg-config-Dateien stammen bei solchen Projekten
/// fast immer aus mitgebauten Fremdbibliotheken (z. B. einem eingebetteten
/// Test-Framework) und gehoeren in ein separates -dev-Paket.
fn dev_artifacts_cleanup_step() -> &'static str {
    r#"if [ -n "$(find "$pkgdir/usr/bin" -maxdepth 1 -type f -perm -u+x -print -quit 2>/dev/null)" ]; then
  rm -rf "$pkgdir/usr/include"
  find "$pkgdir/usr/lib" "$pkgdir/usr/lib64" -name '*.a' -type f -delete 2>/dev/null || true
  find "$pkgdir/usr/lib" "$pkgdir/usr/lib64" -name '*.pc' -type f -delete 2>/dev/null || true
fi"#
}

fn binary_build_step(system: &BuildSystem) -> String {
    match system {
        BuildSystem::Cargo => crate::core::packaging::cargo_build_step(),
        BuildSystem::CMake => {
            r#"cmake -S . -B build -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr $(g2p_cmake_prefix_flag) -DCMAKE_POLICY_VERSION_MINIMUM=3.5 -DCMAKE_SUPPRESS_DEVELOPER_ERRORS=ON && cmake --build build -j$(nproc) && find build -maxdepth 3 -type f -perm -u+x ! -path '*/CMakeFiles/*' -exec sh -c 'case "$1" in *.so|*.so.*|*.a|*.o) exit 0;; esac; head -c 4 "$1" | grep -q ELF && cp "$1" /work/dist/' _ {} \;"#
                .to_string()
        }
        BuildSystem::Meson => {
            r#"libdir="${GIT2PKG_LIBDIR:-/usr/lib}"; meson setup build --prefix=/usr --libdir="${libdir##*/}" && ninja -C build && find build -maxdepth 3 -type f -perm -u+x ! -path '*/CMakeFiles/*' -exec sh -c 'case "$1" in *.so|*.so.*|*.a|*.o) exit 0;; esac; head -c 4 "$1" | grep -q ELF && cp "$1" /work/dist/' _ {} \;"#
                .to_string()
        }
        BuildSystem::Go => {
            r#"export GOCACHE=/work/go-cache; mkdir -p /work/dist /work/go-cache; found=0; for pkg in $(go list -f '{{if eq .Name "main"}}{{.ImportPath}}{{end}}' ./... 2>/dev/null); do name=$(basename "$pkg"); go build -o "/work/dist/$name" "$pkg" && found=1; done; if [ "$found" = 0 ]; then go build -o /work/dist/app .; fi"#
                .to_string()
        }
        BuildSystem::Python => "python3 -m pip wheel --no-deps -w /work/dist .".to_string(),
        BuildSystem::Autotools => {
            r#"libdir="${GIT2PKG_LIBDIR:-/usr/lib}"; if [ ! -x ./configure ]; then if [ -x ./bootstrap ]; then ./bootstrap || exit 1; elif [ -x ./autogen.sh ]; then ./autogen.sh || exit 1; fi; fi; if [ -x ./configure ]; then ./configure --prefix=/usr --libdir="$libdir"; else autoreconf -fi && ./configure --prefix=/usr --libdir="$libdir"; fi && make -j$(nproc) && find . -maxdepth 3 -type f -perm -u+x -exec sh -c 'if command -v file >/dev/null 2>&1; then file -b "$1" | grep -q ELF && cp "$1" /work/dist/; else cp "$1" /work/dist/; fi' _ {} \;"#
                .to_string()
        }
        BuildSystem::Make => {
            "make && find . -maxdepth 2 -type f -perm -u+x -exec sh -c 'if command -v file >/dev/null 2>&1; then file -b \"$1\" | grep -q ELF && cp \"$1\" /work/dist/; else cp \"$1\" /work/dist/; fi' _ {} \\;".to_string()
        }
        BuildSystem::Qmake => {
            "(qmake6 . 2>/dev/null || qmake .) && make && find . -maxdepth 2 -type f -perm -u+x -exec cp {} /work/dist/ \\;".to_string()
        }
        BuildSystem::Unknown => "true".to_string(),
    }
}

fn binary_build_step_with_flags(system: &BuildSystem, flags: &[String]) -> String {
    if *system == BuildSystem::CMake {
        return format!(
            r#"cmake -S . -B build {} -DCMAKE_INSTALL_PREFIX=/usr $(g2p_cmake_prefix_flag) && cmake --build build -j$(nproc) && find build -maxdepth 3 -type f -perm -u+x ! -path '*/CMakeFiles/*' -exec sh -c 'case "$1" in *.so|*.so.*|*.a|*.o) exit 0;; esac; head -c 4 "$1" | grep -q ELF && cp "$1" /work/dist/' _ {{}} \;"#,
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
# Verzeichnisse entstehen erst beim Ablegen echter Dateien - leere
# applications/- oder hicolor/-Ordner gehoeren nicht in ein Paket.
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
        *badge*|badge*|*social*|*creator*|*made_for*|*third-party*|*logo_creator*) continue ;;
      esac
      case "$(dirname "$candidate" | tr '[:upper:]' '[:lower:]')" in
        *badge*|*badges*|*third-party*|*contrib*) continue ;;
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
  mkdir -p "$pkgdir/usr/share/applications"
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
# Kein Desktop-Eintrag erfinden: CLI-Werkzeuge haben in nativen Paketen
# keinen, und ein geratener `Exec=`-Name hat schon Pakete als "ungueltig"
# abgelehnt. Es wird nur gestagt, was das Projekt selbst mitbringt.
mkdir -p "$pkgdir/usr/share/doc/$PKG_NAME" "$pkgdir/usr/share/licenses/$PKG_NAME"
for f in LICENSE LICENSE-* LICENSE.* NOTICE NOTICE-* README README.* THIRD-PARTY*; do
  [ -f "$f" ] || continue
  base=$(basename "$f")
  install -Dm644 "$f" "$pkgdir/usr/share/doc/$PKG_NAME/$base"
  case "$base" in
    LICENSE*|NOTICE*|THIRD-PARTY*) install -Dm644 "$f" "$pkgdir/usr/share/licenses/$PKG_NAME/$base" ;;
  esac
done
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
                r#"cmake -S . -B build -G "Unix Makefiles" {} -DCMAKE_INSTALL_PREFIX=/usr $(g2p_cmake_prefix_flag) && cmake --build build -j$(nproc) && find build -maxdepth 3 -type f -perm -u+x ! -path '*/CMakeFiles/*' -exec sh -c 'case "$1" in *.so|*.so.*|*.a|*.o) exit 0;; esac; head -c 4 "$1" | grep -q ELF && cp "$1" /work/dist/' _ {{}} \;"#,
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
    let mut cmake_flags: Vec<String> = cmake_flags.to_vec();
    push_unique_string(
        &mut cmake_flags,
        format!(
            "-DCMAKE_INSTALL_LIBDIR={}",
            target_libdir(&target.family)
                .rsplit('/')
                .next()
                .unwrap_or("lib")
        ),
    );
    let extra: Vec<String> = extra
        .iter()
        .map(|p| sanitize_package(p))
        .filter(|p| !p.is_empty())
        .collect();
    let mut lines: Vec<String> = vec![
        "#!/usr/bin/env bash".to_string(),
        "set -e".to_string(),
        "set -o pipefail".to_string(),
        "trap 'chown -R $(stat -c '%u:%g' /work 2>/dev/null || echo 0:0) /work 2>/dev/null || true' EXIT".to_string(),
        "export CMAKE_POLICY_VERSION_MINIMUM=3.5".to_string(),
        "export CARGO_TARGET_DIR=/work/cargo-target".to_string(),
        "export CARGO_HOME=/work/cargo-home".to_string(),
        "export CCACHE_DIR=/work/ccache".to_string(),
        "export PIP_BREAK_SYSTEM_PACKAGES=1".to_string(),
        "mkdir -p /work/cargo-target /work/cargo-home /work/ccache".to_string(),
        echo_line(&texts.log_setup(&target.package_manager)),
        format!("export GIT2PKG_LIBDIR={}", target_libdir(&target.family)),
    ];
    // Ein vorgebautes Builder-Image bringt die Toolchain schon mit; dann
    // entfaellt die Paketinstallation zu Beginn jedes Builds.
    if choose_build_image(config, &target.base_image) != target.base_image {
        lines.push(echo_line(
            "[build] prebuilt builder image - toolchain already installed",
        ));
    } else {
        lines.push(base_dependencies(&target.family, system));
    }
    // Upstream-Bootstrap-Projekte (`x.py`, z. B. der Rust-Compiler) laden ihre
    // Stage0-Toolchain und LLVM selbst herunter. Die Dateien werden erst zur
    // Laufzeit geprueft, damit normale Cargo-Projekte nichts extra installieren.
    let bootstrap_cmd =
        install_packages_command(&target.family, &bootstrap_packages(&target.family));
    if !bootstrap_cmd.is_empty() {
        lines.push(format!(
            "if [ -f /work/src/x.py ] && [ -d /work/src/src/bootstrap ]; then {} || true; fi",
            bootstrap_cmd
        ));
    }
    // Erst nach der Installation von git wirksam.
    lines.push("git config --global --add safe.directory '*' 2>/dev/null || true".to_string());
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
    if !config.include_dirs.is_empty() {
        let flags: Vec<String> = config
            .include_dirs
            .iter()
            .map(|dir| format!("-I{}", shell_quote(dir)))
            .collect();
        let flags = flags.join(" ");
        lines.push(format!("export CFLAGS=\"${{CFLAGS:+$CFLAGS }} {flags}\""));
        lines.push(format!(
            "export CXXFLAGS=\"${{CXXFLAGS:+$CXXFLAGS }} {flags}\""
        ));
        lines.push(format!(
            "export CMAKE_C_FLAGS=\"${{CMAKE_C_FLAGS:+$CMAKE_C_FLAGS }} {flags}\""
        ));
        lines.push(format!(
            "export CMAKE_CXX_FLAGS=\"${{CMAKE_CXX_FLAGS:+$CMAKE_CXX_FLAGS }} {flags}\""
        ));
    }
    if config.submodule_fix {
        lines.push("git -C /work/src submodule update --init --recursive || true".to_string());
    }

    let extra_cmd = install_packages_command(&target.family, &extra);
    if !extra_cmd.is_empty() {
        lines.push(echo_line(&texts.log_extra(&extra.join(" "))));
        // Vorgemerkte Pakete können inzwischen umbenannt sein. Ein Fehlschlag
        // darf den Build nicht verhindern: der Resolver findet sonst nichts.
        lines.push(format!(
            "if ! ( {} ); then echo '[pkg] some extra packages could not be installed - continuing' >&2; fi",
            extra_cmd
        ));
    }
    if config.rustup_fix && *system == BuildSystem::Cargo {
        lines.extend(rustup_bootstrap_step(&target.family));
    }
    if config.newer_gxx && is_compiled_build_system(system.label()) {
        lines.extend(newer_gxx_step(&target.family));
    }

    lines.push("mkdir -p /work/out /work/dist".to_string());

    if has_pkgbuild
        && !matches!(
            (&config.target, &target.family),
            (TargetFormat::NativePackage, DistroFamily::Arch)
        )
    {
        lines.push(echo_line(
            "[pkgbuild] Arch PKGBUILD ignored for this target format - using the universal build system",
        ));
    }

    lines.push(cmake_prefix_flag_function());
    let build_step = build_step_text_with_flags(system, strategy, repo_script, &cmake_flags);
    let install_step = project_install_step(system, &identity.name);
    let asset_step = asset_staging_step(&identity.name);
    let pkg_ctx = crate::core::packaging::PackageContext {
        name: &identity.name,
        version: &identity.version,
        description: &identity.description,
        license: &identity.license,
        url: &identity.url,
        family: &target.family,
        strategy,
        has_pkgbuild,
        arch: crate::core::packaging::PackageArch::detect(),
        build_step: &build_step,
        install_step: &install_step,
        asset_step: &asset_step,
    };

    match &config.target {
        TargetFormat::StandaloneBinary => {
            lines.push(echo_line(texts.log_binary_build()));
            lines.push(crate::core::packaging::run_step(&format!(
                "cd /work/src && {}",
                build_step
            )));
        }
        TargetFormat::NativePackage => {
            if target.family == DistroFamily::Unknown {
                lines.push(crate::core::packaging::run_step(&format!(
                    "cd /work/src && {}",
                    build_step
                )));
            } else {
                lines.extend(crate::core::packaging::generate(&pkg_ctx));
            }
        }
    }

    // Erfolg nur melden, wenn wirklich ein Artefakt im Zielordner liegt.
    let artifact_glob = match &config.target {
        TargetFormat::NativePackage => {
            "artifact=\"\"; for candidate in /work/dist/*.pkg.tar.zst /work/dist/*.deb /work/dist/*.rpm; do [ -f \"$candidate\" ] && artifact=\"$candidate\" && break; done"
        }
        TargetFormat::StandaloneBinary => {
            "artifact=\"\"; for candidate in /work/dist/*; do [ -f \"$candidate\" ] && artifact=\"$candidate\" && break; done"
        }
    };
    lines.push(artifact_glob.to_string());
    lines.push(
        "if [ -z \"$artifact\" ]; then echo '__GIT2PKG_NO_ARTIFACT__' >&2; exit 1; fi".to_string(),
    );
    // Ein bloß vorhandenes Paket ist kein Erfolg: nach einem CMake-/Compiler-
    // Fehler darf kein leeres Artefakt den OK-Marker ausloesen.
    lines.push(
        r#"case "$artifact" in
  *.pkg.tar.zst|*.tar.zst)
    if ! (tar -tf "$artifact" 2>/dev/null || tar --zstd -tf "$artifact" 2>/dev/null) | grep -E '(^|/)usr/(bin|lib|lib64)/[^/]' >/dev/null; then
      echo '__GIT2PKG_EMPTY_PACKAGE__' >&2
      exit 1
    fi
    ;;
  *.deb)
    if ! dpkg-deb -c "$artifact" 2>/dev/null | grep -E 'usr/(bin|lib|lib64)/[^/]' >/dev/null; then
      echo '__GIT2PKG_EMPTY_PACKAGE__' >&2
      exit 1
    fi
    ;;
  *.rpm)
    if ! rpm -qlp "$artifact" 2>/dev/null | grep -E 'usr/(bin|lib|lib64)/[^/]' >/dev/null; then
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
    let src = repo_cache_root().join(repo_cache_dir_name(url));
    has_local_dependencies(&src)
}

/// Ein vorhandener Workspace ist nur gültig, wenn sein Quellbaum noch existiert.
fn reusable_workspace(path: Option<&Path>) -> Option<PathBuf> {
    path.filter(|path| path.join("src").is_dir())
        .map(Path::to_path_buf)
}

/// Eindeutiges, vom Repo-Cache getrenntes Arbeitsverzeichnis pro Build.
fn temp_workdir(_url: &str) -> Result<PathBuf, String> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let dir = build_cache_root().join(build_dir_name(millis, std::process::id()));
    fs::create_dir_all(dir.join("out")).map_err(|e| e.to_string())?;
    fs::create_dir_all(dir.join("dist")).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn find_repo_script(src: &Path, family: &str) -> Option<String> {
    crate::core::detector::find_build_script_for(src, family)
}

fn install_command(config: &BuildConfig, artifacts: &[PathBuf]) -> String {
    let file = artifacts
        .first()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "<artifact>".to_string());
    let target = config.target_distro.info(&config.distro);
    match (&config.target, &target.family) {
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
    archive_listing_command, listing_payload, verify_built_packages, VerifyOutcome,
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
                    // Nur der generische Fallback-Name unseres eigenen
                    // Go-Builds wird durch den Projektnamen ersetzt.
                    if config.target == TargetFormat::StandaloneBinary && final_name == "app" {
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
        .unwrap_or_else(|_| "build.sh in cache".to_string());
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

    #[test]
    fn lange_ausgaben_werden_fuer_fehler_gekuerzt() {
        let text = (0..200)
            .map(|index| format!("line {}", index))
            .collect::<Vec<_>>()
            .join("\n");
        let tail = tail_lines(&text, 80);
        assert!(tail.contains("last 80 of 200 lines"));
        assert!(tail.contains("line 199"));
        assert!(!tail.contains("line 119"));
    }

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
            newer_gxx: false,
            skip_deps: false,
            include_dirs: Vec::new(),
            workspace: None,
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
            ..Default::default()
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
    fn bootstrap_pakete_haengen_von_der_familie_ab() {
        let arch = bootstrap_packages(&DistroFamily::Arch).join(" ");
        assert!(arch.contains("curl"));
        assert!(arch.contains("xz"));
        assert!(arch.contains("zlib"));

        let debian = bootstrap_packages(&DistroFamily::Debian).join(" ");
        assert!(debian.contains("xz-utils"));
        assert!(debian.contains("zlib1g-dev"));

        let fedora = bootstrap_packages(&DistroFamily::Fedora).join(" ");
        assert!(fedora.contains("zlib-devel"));

        let suse = bootstrap_packages(&DistroFamily::Suse).join(" ");
        assert!(suse.contains("libopenssl-devel"));

        assert!(bootstrap_packages(&DistroFamily::Unknown).is_empty());
    }

    #[test]
    fn bootstrap_pakete_werden_nur_bei_x_py_installiert() {
        let identity = ProjectIdentity {
            name: "sample-tool".to_string(),
            version: "1.0.0".to_string(),
            ..Default::default()
        };
        let cfg = config(DistroFamily::Arch, TargetFormat::NativePackage);
        let script = build_script_with(
            &cfg,
            &BuildSystem::Cargo,
            &[],
            None,
            &identity,
            BuildStrategy::Default,
            false,
        );
        assert!(script.contains("if [ -f /work/src/x.py ] && [ -d /work/src/src/bootstrap ]"));
        assert!(script.contains("pacman -S --noconfirm --needed -- curl xz zlib openssl"));
    }

    #[test]
    fn spec_metadaten_werden_gelesen() {
        use std::fs;
        let root = std::env::temp_dir().join(format!(
            "g2p-spec-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("sample-tool.spec"),
            "Name: sample-tool\nVersion: 2.3.4\nRelease: 1\n",
        )
        .unwrap();
        let (name, version) = spec_identity(&root).expect("spec");
        assert_eq!(name, "sample-tool");
        assert_eq!(version, "2.3.4");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn debian_metadaten_werden_gelesen() {
        assert_eq!(
            debian_package_name("Source: example-app\nPackage: example-app\n").as_deref(),
            Some("example-app")
        );
        assert_eq!(
            debian_package_name("Package: sample-tool\n").as_deref(),
            Some("sample-tool")
        );
        assert_eq!(debian_package_name("kein control"), None);
        assert_eq!(
            debian_changelog_version(
                "example-app (1.4.2-1) unstable; urgency=medium\n\n  * test\n"
            )
            .as_deref(),
            Some("1.4.2-1")
        );
        assert_eq!(debian_changelog_version("kaputt"), None);
    }

    #[test]
    fn lizenz_wird_aus_lizenzdateien_erkannt() {
        use std::fs;
        let root = std::env::temp_dir().join(format!(
            "g2p-license-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        // Keine Lizenzdatei -> None
        assert_eq!(license_from_files(&root), None);

        // Duale Lizenz: deterministisch in Dateinamen-Reihenfolge
        fs::write(
            root.join("LICENSE-APACHE"),
            "Apache License\nVersion 2.0, January 2004\n",
        )
        .unwrap();
        fs::write(
            root.join("LICENSE-MIT"),
            "MIT License\nPermission is hereby granted, free of charge, to any person\n",
        )
        .unwrap();
        assert_eq!(
            license_from_files(&root).as_deref(),
            Some("Apache-2.0 OR MIT")
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn spdx_erkennt_die_haeufigsten_lizenzen() {
        assert_eq!(
            spdx_from_text("gnu general public license version 3"),
            Some("GPL-3.0-or-later")
        );
        assert_eq!(
            spdx_from_text("gnu lesser general public license version 3"),
            Some("LGPL-3.0-or-later")
        );
        assert_eq!(
            spdx_from_text("apache license version 2.0"),
            Some("Apache-2.0")
        );
        assert_eq!(
            spdx_from_text("permission is hereby granted, free of charge"),
            Some("MIT")
        );
        assert_eq!(spdx_from_text("voellig unbekannter text"), None);
    }

    #[test]
    fn builder_image_namen_folgen_der_familie() {
        assert_eq!(
            builder_image_name(&DistroFamily::Arch),
            "git2pkg-builder:arch"
        );
        assert_eq!(
            builder_image_name(&DistroFamily::Debian),
            "git2pkg-builder:debian"
        );
        assert_eq!(
            builder_image_name(&DistroFamily::Fedora),
            "git2pkg-builder:fedora"
        );
        assert_eq!(
            builder_image_name(&DistroFamily::Suse),
            "git2pkg-builder:suse"
        );
        assert!(builder_image_name(&DistroFamily::Unknown).is_empty());
    }

    #[test]
    fn neuer_compiler_wird_im_skript_gesetzt() {
        let identity = ProjectIdentity {
            name: "sample-tool".to_string(),
            version: "1.0.0".to_string(),
            ..Default::default()
        };
        let mut cfg = config(DistroFamily::Debian, TargetFormat::NativePackage);
        cfg.newer_gxx = true;
        let script = build_script_with(
            &cfg,
            &BuildSystem::CMake,
            &[],
            None,
            &identity,
            BuildStrategy::Default,
            false,
        );
        assert!(script.contains("looking for a newer C++ compiler"));
        assert!(script.contains("g++-[0-9]*"));
        assert!(script.contains("export CXX="));
        assert!(script.contains("apt-cache search"));

        // Ein reines Rust-Projekt bekommt den Schritt nicht.
        let plain = config(DistroFamily::Debian, TargetFormat::NativePackage);
        let rust = build_script_with(
            &plain,
            &BuildSystem::Cargo,
            &[],
            None,
            &identity,
            BuildStrategy::Default,
            false,
        );
        assert!(!rust.contains("newer C++ compiler"));
    }

    #[test]
    fn heil_runden_sind_konfigurierbar() {
        // Standard ohne Umgebungsvariable
        std::env::remove_var("GIT2PKG_MAX_HEAL_ROUNDS");
        assert_eq!(max_heal_rounds(), 20);
        std::env::set_var("GIT2PKG_MAX_HEAL_ROUNDS", "40");
        assert_eq!(max_heal_rounds(), 40);
        std::env::set_var("GIT2PKG_MAX_HEAL_ROUNDS", "0");
        assert_eq!(max_heal_rounds(), 1);
        std::env::set_var("GIT2PKG_MAX_HEAL_ROUNDS", "9999");
        assert_eq!(max_heal_rounds(), 200);
        std::env::set_var("GIT2PKG_MAX_HEAL_ROUNDS", "unsinn");
        assert_eq!(max_heal_rounds(), 20);
        std::env::remove_var("GIT2PKG_MAX_HEAL_ROUNDS");
    }

    #[test]
    fn autotools_fuehrt_bootstrap_aus() {
        let step = binary_build_step(&BuildSystem::Autotools);
        assert!(step.contains("[ -x ./bootstrap ]"));
        assert!(step.contains("./bootstrap || exit 1"));
        assert!(step.contains("[ -x ./autogen.sh ]"));
        assert!(step.contains("autoreconf -fi"));
    }

    #[test]
    fn besitzerabfrage_kennt_alle_paketmanager() {
        assert!(owner_command("pacman", "/usr/share/x").starts_with("pacman -Qoq"));
        assert!(owner_command("apt", "/usr/share/x").starts_with("dpkg -S"));
        assert!(owner_command("dnf", "/usr/share/x").starts_with("rpm -qf"));
        assert!(owner_command("zypper", "/usr/share/x").starts_with("rpm -qf"));
        assert!(owner_command("unbekannt", "/usr/share/x").is_empty());
    }

    #[test]
    fn installationspruefung_fragt_die_paketdatenbank() {
        // Reine Kommandoerzeugung: die Funktion selbst braucht einen Container.
        for (manager, expected) in [
            ("pacman", "pacman -Q"),
            ("apt", "dpkg -s"),
            ("dnf", "rpm -q"),
            ("zypper", "rpm -q"),
        ] {
            let command = match manager {
                "pacman" => format!("pacman -Q {} 2>/dev/null", "'example'"),
                "apt" => "dpkg -s 'example' 2>/dev/null | grep -q '^Status: install ok installed'"
                    .to_string(),
                _ => format!("rpm -q {} 2>/dev/null", "'example'"),
            };
            assert!(command.contains(expected), "{manager}: {command}");
        }
    }

    #[test]
    fn arbeitsverzeichnis_wird_fuer_selinux_relabelt() {
        let mount = workspace_mount(std::path::Path::new("/home/user/.cache/git2pkg/builds/x"));
        assert!(mount.ends_with(":/work:z"), "{mount}");
    }

    #[test]
    fn anwendungspakete_verlieren_entwicklungsdateien() {
        let step = project_install_step(&BuildSystem::CMake, "example-app");
        assert!(step.contains("rm -rf \"$pkgdir/usr/include\""));
        assert!(step.contains("-name '*.a' -type f -delete"));
        assert!(step.contains("-name '*.pc' -type f -delete"));
        // Nur Pakete mit ausfuehrbaren Dateien werden bereinigt.
        assert!(step.contains("-type f -perm -u+x"));
    }

    #[test]
    fn submodule_werden_flach_und_parallel_geholt() {
        let args = submodule_update_args();
        for expected in ["--init", "--recursive", "--depth=1", "--jobs=4"] {
            assert!(
                args.contains(&expected.to_string()),
                "{} fehlt in {:?}",
                expected,
                args
            );
        }
    }

    #[test]
    fn versionen_bevorzugen_stabile_tags() {
        assert_eq!(
            pick_version_tag("v2.0.0-alpha.2\nv2.0.0\nv1.10.0").as_deref(),
            Some("v2.0.0")
        );
        assert_eq!(pick_version_tag("1.4.5\n1.3.0").as_deref(), Some("1.4.5"));
        assert_eq!(pick_version_tag("nightly\nstable").as_deref(), None);
        assert_eq!(pick_version_tag("").as_deref(), None);
        assert_eq!(
            pick_version_tag("v3.0.0-rc.1").as_deref(),
            Some("v3.0.0-rc.1")
        );
        // Praefixe duerfen eine hoehere Version nicht verdraengen.
        assert_eq!(
            pick_version_tag("release-0.7\n1.102.0\n1.101.0").as_deref(),
            Some("1.102.0")
        );
        assert_eq!(
            pick_version_tag("v1.0.0\nrelease-1.0.0").as_deref(),
            Some("v1.0.0")
        );
    }

    #[test]
    fn cmake_baut_mit_install_prefix_usr() {
        let plain = binary_build_step(&BuildSystem::CMake);
        assert!(plain.contains("-DCMAKE_INSTALL_PREFIX=/usr"));
        let with_flags =
            binary_build_step_with_flags(&BuildSystem::CMake, &["-DEXAMPLE=ON".to_string()]);
        assert!(with_flags.contains("-DCMAKE_INSTALL_PREFIX=/usr"));
    }

    #[test]
    fn rustup_healing_setzt_toolchain_vor_usr_bin() {
        let identity = ProjectIdentity {
            name: "sample-tool".to_string(),
            version: "1.0.0".to_string(),
            ..Default::default()
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
    fn projekt_metadaten_werden_aus_manifesten_gelesen() {
        let root = std::env::temp_dir().join(format!(
            "git2pkg-metadata-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("packaging/linux")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace.package]\nversion = \"1.2.3\"\nlicense = \"MIT OR Apache-2.0\"\nrepository = \"https://example.com/repo\"\n",
        )
        .unwrap();
        fs::write(
            root.join("packaging/linux/nfpm.yaml"),
            "name: example\nhomepage: https://example.com/app\nlicense: Apache-2.0\ndescription: |-\n  A sample app\n",
        )
        .unwrap();

        let identity = project_identity("https://example.com/example", &root);
        assert_eq!(identity.name, "example");
        assert_eq!(identity.version, "1.2.3");
        assert_eq!(identity.license, "Apache-2.0");
        assert_eq!(identity.url, "https://example.com/app");
        assert_eq!(identity.description, "A sample app");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn asset_staging_legt_docs_und_lizenzen_ab() {
        let script = asset_staging_step("example-app");
        assert!(script.contains("usr/share/licenses/$PKG_NAME"));
        assert!(script.contains("THIRD-PARTY*"));
        assert!(script.contains("README.*"));
    }

    #[test]
    fn badges_werden_nicht_als_app_icon_installiert() {
        let root = std::env::temp_dir().join(format!(
            "git2pkg-badge-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let repo = root.join("repo");
        let out = root.join("out");
        fs::create_dir_all(repo.join("assets/badges")).unwrap();
        fs::create_dir_all(repo.join("assets")).unwrap();

        // App-Icon (PNG) und ein Badge (SVG), das frueher gewonnen hat.
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&48u32.to_be_bytes());
        png.extend_from_slice(&48u32.to_be_bytes());
        png.extend_from_slice(&[8, 2, 0, 0, 0, 0, 0, 0, 0]);
        fs::write(repo.join("assets/app_icon.png"), png).unwrap();
        fs::write(
            repo.join("assets/badges/hermes-agent.svg"),
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1024 1024\"></svg>",
        )
        .unwrap();
        // Ohne eigenen Desktop-Eintrag wird bewusst kein Icon gestagt; hier
        // bringt das Projekt einen mit.
        fs::write(
            repo.join("sample-tool.desktop"),
            "[Desktop Entry]\nType=Application\nName=sample-tool\nExec=sample-tool\nIcon=sample-tool\n",
        )
        .unwrap();

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

        let icon = out.join("usr/share/icons/hicolor/48x48/apps/sample-tool.png");
        assert!(icon.is_file(), "App-Icon wurde nicht installiert");
        assert!(
            !out.join("usr/share/icons/hicolor/scalable/apps/sample-tool.svg")
                .exists(),
            "Badge darf nicht als App-Icon landen"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn asset_staging_erfindet_keinen_desktop_eintrag() {
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

        // Ohne eigenes .desktop im Projekt entsteht keiner: ein geratenes
        // `Exec=` hat schon Pakete als ungueltig abgelehnt (CLI-Werkzeuge
        // haben in nativen Paketen ebenfalls keinen Desktop-Eintrag).
        assert!(
            !out.join("usr/share/applications/sample-tool.desktop")
                .exists(),
            "Desktop-Eintrag darf nicht erfunden werden"
        );
        let script = asset_staging_step("sample-tool");
        assert!(!script.contains("Exec=$PKG_NAME"));
        // Mitgelieferte Dateien werden weiterhin gestagt.
        let _ = fs::metadata(repo.join("assets/app_icon.png"))
            .unwrap()
            .permissions()
            .mode();
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn qmake_kette_bricht_bei_konfigurationsfehler_ab() {
        let step = binary_build_step(&BuildSystem::Qmake);
        assert!(step.contains("&& make"), "{step}");
        assert!(step.contains("&& find"), "{step}");
    }

    #[test]
    fn go_projekt_baut_alle_main_pakete() {
        let step = binary_build_step(&BuildSystem::Go);
        assert!(step.contains("go list -f"), "{step}");
        assert!(step.contains("/work/dist/$name"), "{step}");
        let install = project_install_step(&BuildSystem::Go, "example-app");
        assert!(install.contains("for binary in /work/dist/*"), "{install}");
    }

    #[test]
    fn make_und_autotools_haben_binary_fallback() {
        for system in [BuildSystem::Make, BuildSystem::Autotools] {
            let install = project_install_step(&system, "example-app");
            assert!(install.contains("staging built binaries"), "{install}");
        }
    }

    #[test]
    fn cmake_und_meson_stagen_binaries_ohne_install_regeln() {
        for system in [BuildSystem::CMake, BuildSystem::Meson] {
            let install = project_install_step(&system, "example-app");
            assert!(install.contains("no install rules"), "{install}");
            assert!(install.contains("pkgdir/usr/bin"), "{install}");
        }
        // Interne CMake-Testprogramme dürfen nicht als Programm gelten.
        let build = binary_build_step(&BuildSystem::CMake);
        assert!(build.contains("grep -q ELF"), "{build}");
        assert!(!build.contains("-exec cp"), "{build}");
    }

    #[test]
    fn autotools_version_wird_gelesen() {
        assert_eq!(
            autotools_version("AC_INIT([example-app], [1.2.3], [bugs@example.invalid])"),
            Some("1.2.3".to_string())
        );
        assert_eq!(
            autotools_version("PACKAGE_VERSION='4.5.6'"),
            Some("4.5.6".to_string())
        );
        assert_eq!(autotools_version("nothing here"), None);
    }

    #[test]
    fn ziel_libdir_wird_gesetzt() {
        assert_eq!(target_libdir(&DistroFamily::Fedora), "/usr/lib64");
        assert_eq!(target_libdir(&DistroFamily::Suse), "/usr/lib64");
        assert_eq!(target_libdir(&DistroFamily::Arch), "/usr/lib");
        assert_eq!(target_libdir(&DistroFamily::Debian), "/usr/lib");

        let fedora = build_script(
            &config(DistroFamily::Fedora, TargetFormat::NativePackage),
            &BuildSystem::Autotools,
            &[],
        );
        assert!(fedora.contains("export GIT2PKG_LIBDIR=/usr/lib64"));
        assert!(fedora.contains("--libdir=\"$libdir\""));

        let fedora_cmake = build_script(
            &config(DistroFamily::Fedora, TargetFormat::NativePackage),
            &BuildSystem::CMake,
            &[],
        );
        assert!(fedora_cmake.contains("-DCMAKE_INSTALL_LIBDIR=lib64"));

        let arch = build_script(
            &config(DistroFamily::Arch, TargetFormat::NativePackage),
            &BuildSystem::Autotools,
            &[],
        );
        assert!(arch.contains("export GIT2PKG_LIBDIR=/usr/lib"));
        let arch_cmake = build_script(
            &config(DistroFamily::Arch, TargetFormat::NativePackage),
            &BuildSystem::CMake,
            &[],
        );
        assert!(arch_cmake.contains("-DCMAKE_INSTALL_LIBDIR=lib"));
    }

    #[test]
    fn repo_skripte_werden_generisch_erkannt() {
        let root = std::env::temp_dir().join(format!(
            "git2pkg-script-scan-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("install-deps.sh"), "").unwrap();
        fs::write(root.join("compile-everything.sh"), "").unwrap();
        fs::write(root.join("BuildProject.sh"), "").unwrap();
        fs::write(root.join("run-tests.sh"), "").unwrap();
        fs::write(root.join("build-docker.sh"), "").unwrap();
        fs::write(root.join("build-ci.sh"), "").unwrap();
        fs::write(root.join("notes.txt"), "").unwrap();

        // Wortgrenzen statt Teilstrings: `build-docker.sh`/`build-ci.sh` sind
        // keine Bau-Skripte, CamelCase wird aber erkannt.
        assert_eq!(
            find_repo_script(&root, "arch").as_deref(),
            Some("BuildProject.sh")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cmake_modul_verifikation_nutzt_find_package() {
        let probe = cmake_verification_probe("ExampleModule");
        assert!(probe.contains("find_package(ExampleModule"));
        assert!(!probe.contains("command -v"));
    }

    #[test]
    fn nicht_existierender_prefix_pfad_wird_entfernt() {
        let script = build_script(
            &config(DistroFamily::Arch, TargetFormat::NativePackage),
            &BuildSystem::CMake,
            &[],
        );
        assert!(script.contains("g2p_cmake_prefix_flag"), "{script}");
        assert!(script.contains("[ -d \"$dir\" ]"), "{script}");
        assert!(
            !script.contains("-DCMAKE_PREFIX_PATH=/work/deps-install"),
            "{script}"
        );
        assert!(
            !script.contains("export CMAKE_PREFIX_PATH=/work/deps-install"),
            "{script}"
        );
    }

    #[test]
    fn vorhandener_workspace_wird_wiederverwendet() {
        let root = std::env::temp_dir().join(format!(
            "git2pkg-workspace-reuse-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let valid = root.join("valid");
        fs::create_dir_all(valid.join("src")).unwrap();
        let empty = root.join("empty");
        fs::create_dir_all(&empty).unwrap();

        assert_eq!(
            reusable_workspace(Some(valid.as_path())),
            Some(valid.clone())
        );
        assert!(reusable_workspace(Some(empty.as_path())).is_none());
        assert!(reusable_workspace(None).is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn artefaktpruefung_vermeidet_sigpipe() {
        let script = build_script(
            &config(DistroFamily::Arch, TargetFormat::NativePackage),
            &BuildSystem::Cargo,
            &[],
        );
        assert!(
            !script.contains("grep -Eq"),
            "grep -q bricht bei pipefail ab"
        );
        assert!(script.contains("grep -E 'usr/(bin|lib|lib64)/[^/]' >/dev/null"));
    }

    #[test]
    fn include_pfade_landen_im_buildskript() {
        let mut config = config(DistroFamily::Arch, TargetFormat::NativePackage);
        config.include_dirs = vec!["/usr/include/example".to_string()];
        let identity = project_identity("example-app", Path::new(""));
        let script = build_script_with(
            &config,
            &BuildSystem::CMake,
            &[],
            None,
            &identity,
            BuildStrategy::Default,
            false,
        );
        assert!(script.contains("-I'/usr/include/example'"), "{}", script);
        assert!(script.contains("export CXXFLAGS="), "{}", script);
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
            BuildSystem::Autotools,
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
    fn nur_configure_stempel_werden_entfernt() {
        let root = std::env::temp_dir().join(format!(
            "g2p-stamps-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let stamp = root.join("deps-build/dep_X-prefix/src/dep_X-stamp");
        std::fs::create_dir_all(&stamp).unwrap();
        std::fs::write(stamp.join("dep_X-configure"), "").unwrap();
        std::fs::write(stamp.join("dep_X-download"), "").unwrap();
        assert_eq!(reset_dependency_configure_stamps(&root), 1);
        assert!(!stamp.join("dep_X-configure").exists());
        assert!(stamp.join("dep_X-download").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn strategien_werden_nicht_wiederholt() {
        let mut visited: HashSet<&'static str> = HashSet::new();
        visited.insert(BuildStrategy::Default.key());
        visited.insert(BuildStrategy::SystemLibsOnly.key());
        assert_eq!(
            next_unvisited_strategy(BuildStrategy::SystemLibsOnly, true, &visited),
            Some(BuildStrategy::RepoScript)
        );
        visited.insert(BuildStrategy::RepoScript.key());
        assert_eq!(
            next_unvisited_strategy(BuildStrategy::RepoScript, true, &visited),
            Some(BuildStrategy::DirectStagingFallback)
        );
        visited.insert(BuildStrategy::DirectStagingFallback.key());
        assert_eq!(
            next_unvisited_strategy(BuildStrategy::DirectStagingFallback, true, &visited),
            None
        );
        let empty: HashSet<&'static str> = HashSet::new();
        assert_eq!(
            next_unvisited_strategy(BuildStrategy::Default, false, &empty),
            Some(BuildStrategy::RepoScript)
        );
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
            ..Default::default()
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
            ..Default::default()
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
        assert!(arch.contains("pkgver = 1.0.0.13-1"));
        assert!(arch.contains("bsdtar --zstd"));

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
            Some("compile-repo.sh"),
        );
        assert!(script.contains("./compile-repo.sh"));
        assert!(script.contains("bash \"$script\""));
        assert!(script.contains("usage|and then"));
        let default_with_script = build_step_text(
            &BuildSystem::CMake,
            BuildStrategy::Default,
            Some("compile-repo.sh"),
        );
        assert!(!default_with_script.contains("compile-repo.sh"));
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
        assert!(!patched.contains("export CMAKE_PREFIX_PATH=/work/deps-install"));
        assert!(patched.contains("-DCMAKE_INSTALL_PREFIX=/work/deps-install"));
        assert!(patched.contains("rm -f /work/deps-build/CMakeCache.txt"));
        assert!(patched.contains("rm -rf /work/deps-build/CMakeFiles"));
        assert!(!patched.contains("rm -rf /work/deps-build;"));
        assert!(patched.contains("downloaded sources are kept"));
        assert!(patched.contains("resolver takes over"));
        assert!(patched.lines().count() == script.lines().count() + 1);
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
                script.contains("safe.directory"),
                "{:?}: git-Ownership-Ausnahme fehlt",
                family
            );
            assert!(
                !script.contains("yes '' |"),
                "{:?}: SIGPIPE-Pipeline",
                family
            );
            let check = script
                .find("__GIT2PKG_NO_ARTIFACT__")
                .expect("artefaktcheck");
            // Der Marker kann mehrfach vorkommen (nativer Debian-Pfad); der
            // generische Pfad endet mit dem letzten Vorkommen.
            let ok = script.rfind(OK_MARKER).expect("ok-marker");
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
    fn in_tree_pkgbuild_baut_als_unprivilegierter_benutzer() {
        let cfg = config(DistroFamily::Arch, TargetFormat::NativePackage);
        let identity = project_identity("example-app", Path::new(""));
        let script = build_script_with(
            &cfg,
            &BuildSystem::Cargo,
            &[],
            None,
            &identity,
            BuildStrategy::Default,
            true,
        );
        assert!(script.contains("useradd -m -s /bin/bash builder"));
        assert!(script.contains("chown -R builder:builder"));
        assert!(script.contains("cd /work/src && makepkg -s --noconfirm"));
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
            ..Default::default()
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
        assert!(deb.contains("Arch PKGBUILD ignored for this target format"));
        assert!(deb.contains("cmake --install build"));
        assert!(deb.contains("if [ -f build/cmake_install.cmake ]; then DESTDIR="));
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
            ..Default::default()
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
        assert!(generated.contains(".PKGINFO"));
        assert!(generated.contains("bsdtar --zstd"));
    }

    #[test]
    fn script_enthaelt_direkte_arch_paketierung() {
        let script = build_script(
            &config(DistroFamily::Arch, TargetFormat::NativePackage),
            &BuildSystem::Cargo,
            &[],
        );
        assert!(script.contains("bsdtar --zstd"));
        assert!(script.contains("pkgname = example-app"));
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
            "[err] [deps] build failed - resolver takes over",
            false
        ));
        assert!(!deps_fallback_needed(
            "[err] [deps] build failed - resolver takes over",
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
        assert!(!assets.contains("mkdir -p \"$pkgdir/usr/share/icons/hicolor"));
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
    fn build_cache_loeschen_laesst_repo_cache_stehen() {
        let root = std::env::temp_dir().join(format!(
            "git2pkg-builds-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let builds = root.join("builds");
        let repos = root.join("repos");
        fs::create_dir_all(builds.join("build-1/src")).unwrap();
        fs::create_dir_all(&repos).unwrap();
        fs::write(repos.join("keep"), "x").unwrap();

        assert!(cleanup_all_builds_in(&builds) >= 1);
        assert!(!builds.exists());
        assert!(repos.join("keep").is_file());

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
            ..Default::default()
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
    fn install_command_folgt_dem_ziel_bei_cross_builds() {
        let artifact = vec![PathBuf::from("/tmp/dist/app_1.0.0_amd64.deb")];
        let mut cfg = config(DistroFamily::Arch, TargetFormat::NativePackage);
        cfg.target_distro = TargetDistro::Debian;
        assert_eq!(
            install_command(&cfg, &artifact),
            "sudo apt install /tmp/dist/app_1.0.0_amd64.deb"
        );

        let rpm = vec![PathBuf::from("/tmp/dist/app-1.0-1.x86_64.rpm")];
        cfg.target_distro = TargetDistro::Fedora;
        assert_eq!(
            install_command(&cfg, &rpm),
            "sudo dnf install /tmp/dist/app-1.0-1.x86_64.rpm"
        );
        cfg.target_distro = TargetDistro::Suse;
        assert_eq!(
            install_command(&cfg, &rpm),
            "sudo zypper install /tmp/dist/app-1.0-1.x86_64.rpm"
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
    fn cache_praefixe_sind_getrennt() {
        let repo = repo_cache_dir_name("example-app");
        assert!(repo.starts_with("repo-"));
        assert_eq!(
            build_dir_name(1732285620123, 4812),
            "build-1732285620123-4812"
        );
        let work = temp_workdir("example-app").unwrap();
        let name = work.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("build-"));
        assert!(!name.starts_with("repo-"));
        make_tree_writable(&work);
        let _ = fs::remove_dir_all(&work);
    }

    #[test]
    fn vollstaendiger_cache_wipe_laesst_lerndaten() {
        let root = std::env::temp_dir().join(format!(
            "g2p-full-cache-wipe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let cache = root.join("git2pkg");
        let mappings = root.join("learned_mappings.json");
        fs::create_dir_all(cache.join("builds/example-hash/src/.git/objects/aa")).unwrap();
        fs::create_dir_all(cache.join("repos/example-hash/.git/objects/bb")).unwrap();
        fs::write(
            cache.join("builds/example-hash/src/.git/objects/aa/object"),
            "x",
        )
        .unwrap();
        fs::write(cache.join("repos/example-hash/.git/objects/bb/object"), "y").unwrap();
        fs::write(&mappings, "{}").unwrap();

        assert!(cleanup_all_cache_in(&cache));
        assert!(!cache.exists());
        assert!(mappings.is_file());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_script_setzt_chown_trap() {
        let identity = ProjectIdentity {
            name: "example-app".to_string(),
            version: "1.0.0".to_string(),
            ..Default::default()
        };
        let script = build_script_with(
            &config(DistroFamily::Arch, TargetFormat::NativePackage),
            &BuildSystem::CMake,
            &[],
            None,
            &identity,
            BuildStrategy::Default,
            false,
        );
        assert!(script.contains("trap 'chown -R"));
        assert!(script.contains("stat -c '%u:%g' /work"));
    }

    #[test]
    fn container_cleanup_kommando_baut_mount_und_rm() {
        let builds = Path::new("/tmp/example-builds");
        let (program, args) = container_cleanup_args(builds, "example-image:latest");
        assert_eq!(program, "docker");
        assert_eq!(args[0], "run");
        assert_eq!(args[1], "--rm");
        assert_eq!(args[3], "/tmp/example-builds:/cleanup:z");
        assert_eq!(args[4], "example-image:latest");
        assert!(args[7].contains("rm -rf /cleanup/*"));
    }

    #[test]
    fn cleanup_loescht_nur_builds_und_laesst_repos_und_lerndaten() {
        let root = std::env::temp_dir().join(format!(
            "g2p-cache-trennung-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let builds = root.join("builds");
        let work = builds.join("example-hash");
        let repos = root.join("repos");
        let cached_repo = repos.join("example-hash/.git/objects/aa");
        let mappings = root.join("learned_mappings.json");
        fs::create_dir_all(work.join("src/.git/objects/aa")).unwrap();
        fs::create_dir_all(&cached_repo).unwrap();
        fs::write(work.join("src/.git/objects/aa/object"), "build").unwrap();
        fs::write(cached_repo.join("object"), "repo").unwrap();
        fs::write(&mappings, "{}").unwrap();

        assert_eq!(cleanup_all_builds_in(&builds), 1);
        assert!(!builds.exists());
        assert!(cached_repo.join("object").is_file());
        assert!(mappings.is_file());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cmake_retry_flags_aus_fehlertext_und_policy() {
        let output = "CMake Error: Missing backend, or pass -DFOO_BAR=ON\nmirrors: set -DUSE_SYSTEM_FOO=ON\nCompatibility with CMake < 3.5 has been removed.\n";
        let flags = cmake_retry_flags(output, &[]);
        assert!(flags.contains(&"-DFOO_BAR=ON".to_string()));
        assert!(flags.contains(&"-DUSE_SYSTEM_FOO=ON".to_string()));
        assert!(flags.contains(&"-DCMAKE_POLICY_VERSION_MINIMUM=3.5".to_string()));
        assert!(!flags
            .iter()
            .any(|flag| flag.to_ascii_lowercase().contains("eigen")));
        let again = cmake_retry_flags(output, &flags);
        assert!(again.is_empty());
    }

    #[test]
    fn retry_state_erkennt_leerlauf() {
        let before = RetryState {
            packages: vec!["boost".to_string()],
            cmake_flags: vec!["-DFOO=ON".to_string()],
            include_dirs: Vec::new(),
        };
        let same = RetryState {
            packages: vec!["boost".to_string()],
            cmake_flags: vec!["-DFOO=ON".to_string()],
            include_dirs: Vec::new(),
        };
        let changed = RetryState {
            packages: vec!["boost".to_string(), "openssl".to_string()],
            cmake_flags: vec!["-DFOO=ON".to_string()],
            include_dirs: Vec::new(),
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
