//! Regex-basierte Analyse von Compiler-Ausgaben: fehlende Header,
//! pkg-config-Pakete und CMake-Module.

use crate::core::distro::DistroFamily;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Mitgelieferte Start-Wissensbasis; lokale Learnings ergänzen diese.
const DEFAULT_MAPPINGS: &str = include_str!("../../assets/default_mappings.json");

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MissingDep {
    Header(String),
    PkgConfig(String),
    CMakeModule(String),
    Tool(String),
}

impl MissingDep {
    pub fn label(&self) -> String {
        match self {
            MissingDep::Header(h) => format!("header '{}'", h),
            MissingDep::PkgConfig(p) => format!("pkg-config '{}'", p),
            MissingDep::CMakeModule(m) => format!("CMake module '{}'", m),
            MissingDep::Tool(t) => format!("tool '{}'", t),
        }
    }

    /// Suchbegriff für den Paketmanager: immer kleingeschrieben, da Linux-
    /// Paketnamen praktisch nie Großbuchstaben enthalten (`Boost` -> `boost`).
    /// Ursprünglicher Name aus der Compiler-Ausgabe (mit Großschreibung).
    pub fn raw_name(&self) -> &str {
        match self {
            MissingDep::Header(h) => h,
            MissingDep::PkgConfig(p) | MissingDep::CMakeModule(p) | MissingDep::Tool(p) => p,
        }
    }

    pub fn search_term(&self) -> String {
        match self {
            MissingDep::Header(h) => h.trim_end_matches(".h").to_ascii_lowercase(),
            MissingDep::PkgConfig(p) | MissingDep::CMakeModule(p) => {
                p.trim_end_matches("-config")
                    .trim_end_matches("Config")
                    .to_ascii_lowercase()
            }
            MissingDep::Tool(t) => t.to_ascii_lowercase(),
        }
    }
}

/// Häufige CMake-Module auf die passenden Distribution-Pakete abbilden.
/// Kleingeschriebene Modulnamen fallen auf den Namen selbst zurück.
pub fn distro_package_name(name: &str, package_manager: &str) -> Option<&'static str> {
    match (package_manager, name.to_ascii_lowercase().as_str()) {
        ("pacman", "boost") => Some("boost"),
        ("pacman", "eigen3") | ("pacman", "eigen") => Some("eigen"),
        ("pacman", "openssl") | ("pacman", "ssl") => Some("openssl"),
        ("pacman", "curl") | ("pacman", "libcurl") => Some("curl"),
        ("pacman", "zlib") => Some("zlib"),
        ("pacman", "jpeg") => Some("libjpeg-turbo"),
        ("pacman", "png") | ("pacman", "libpng") => Some("libpng"),
        ("pacman", "opengl") => Some("libglvnd"),
        ("pacman", "wxwidgets") => Some("wxwidgets-gtk3"),

        ("apt", "boost") => Some("libboost-dev"),
        ("apt", "eigen3") | ("apt", "eigen") => Some("libeigen3-dev"),
        ("apt", "openssl") | ("apt", "ssl") => Some("libssl-dev"),
        ("apt", "curl") | ("apt", "libcurl") => Some("libcurl4-openssl-dev"),
        ("apt", "zlib") => Some("zlib1g-dev"),
        ("apt", "jpeg") => Some("libjpeg-dev"),
        ("apt", "png") | ("apt", "libpng") => Some("libpng-dev"),
        ("apt", "opengl") => Some("libgl-dev"),
        ("apt", "wxwidgets") => Some("libwxgtk3.2-dev"),

        ("dnf", "boost") => Some("boost-devel"),
        ("dnf", "eigen3") | ("dnf", "eigen") => Some("eigen3-devel"),
        ("dnf", "openssl") | ("dnf", "ssl") => Some("openssl-devel"),
        ("dnf", "curl") | ("dnf", "libcurl") => Some("libcurl-devel"),
        ("dnf", "zlib") => Some("zlib-devel"),
        ("dnf", "jpeg") => Some("libjpeg-turbo-devel"),
        ("dnf", "png") | ("dnf", "libpng") => Some("libpng-devel"),
        ("dnf", "opengl") => Some("mesa-libGL-devel"),
        ("dnf", "wxwidgets") => Some("wxGTK-devel"),

        ("zypper", "boost") => Some("boost-devel"),
        ("zypper", "eigen3") | ("zypper", "eigen") => Some("eigen3-devel"),
        ("zypper", "openssl") | ("zypper", "ssl") => Some("libopenssl-devel"),
        ("zypper", "zlib") => Some("zlib-devel"),
        ("zypper", "curl") | ("zypper", "libcurl") => Some("libcurl-devel"),
        ("zypper", "jpeg") => Some("libjpeg-devel"),
        ("zypper", "png") | ("zypper", "libpng") => Some("libpng-devel"),
        ("zypper", "opengl") => Some("libGL-devel"),
        ("zypper", "wxwidgets") => Some("wxWidgets-devel"),
        _ => None,
    }
}

/// Aus der Ausgabe eines Paket-Suchbefehls den ersten brauchbaren Paketnamen ziehen.
pub fn package_from_search(output: &str, term: &str, package_manager: &str) -> Option<String> {
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let name = match package_manager {
            "apt" => line.split(':').next().unwrap_or(""),
            "pacman" => {
                if let Some((head, _)) = line.split_once(':') {
                    head.rsplit('/').next().unwrap_or(head)
                } else if line.contains('/') {
                    continue; // reiner Dateipfad aus pacman -Fq
                } else {
                    line
                }
            }
            _ => line.split_whitespace().next().unwrap_or(""),
        };
        let name = name.trim().to_ascii_lowercase();
        if !valid_package_name(&name) {
            continue;
        }
        if name.contains(term) || term.contains(&name) {
            return Some(name);
        }
    }
    None
}

/// Kurzschlüssel für die Wissensdatenbank, z. B. `arch` oder `debian`.
pub fn family_key(family: &DistroFamily) -> &'static str {
    match family {
        DistroFamily::Arch => "arch",
        DistroFamily::Debian => "debian",
        DistroFamily::Fedora => "fedora",
        DistroFamily::Suse => "suse",
        DistroFamily::Unknown => "unknown",
    }
}

/// Lokale, dauerhafte Paket-Zuordnungen ("Live-Lernen" ohne KI).
#[derive(Debug, Clone)]
#[derive(Serialize, Deserialize, Default)]
struct LearnedFile {
    #[serde(default)]
    packages: HashMap<String, String>,
    #[serde(default)]
    negative_cache: Vec<String>,
    #[serde(default)]
    build_fixes: HashMap<String, String>,
}

pub struct MappingStore {
    path: PathBuf,
    learned: HashMap<String, String>,
    negative_cache: Vec<String>,
    build_fixes: HashMap<String, String>,
    defaults: HashMap<String, String>,
}

impl MappingStore {
    pub fn load() -> Self {
        let path = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".config/git2pkg/learned_mappings.json");
        Self::from_path(path)
    }

    fn from_path(path: PathBuf) -> Self {
        let defaults: HashMap<String, String> =
            serde_json::from_str(DEFAULT_MAPPINGS).unwrap_or_default();
        let text = fs::read_to_string(&path).unwrap_or_default();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();

        // Neue datei mit Sektionen; alte flache Dateien werden übernommen.
        let (learned, negative_cache, build_fixes) = if value.get("packages").is_some()
            || value.get("build_fixes").is_some()
            || value.get("negative_cache").is_some()
        {
            let packages = value
                .get("packages")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            let negative = value
                .get("negative_cache")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            let fixes = value
                .get("build_fixes")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            (packages, negative, fixes)
        } else {
            let flat = serde_json::from_value::<HashMap<String, String>>(value).unwrap_or_default();
            (flat, Vec::new(), HashMap::new())
        };

        let store = Self {
            path,
            learned,
            negative_cache,
            build_fixes,
            defaults,
        };
        store.save();
        store
    }

    fn save(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let file = LearnedFile {
            packages: self.learned.clone(),
            negative_cache: self.negative_cache.clone(),
            build_fixes: self.build_fixes.clone(),
        };
        if let Ok(json) = serde_json::to_string_pretty(&file) {
            let _ = fs::write(&self.path, json);
        }
    }

    /// Erst die gelernten Einträge prüfen, dann die mitgelieferte Basis.
    pub fn get(&self, family: &str, name: &str) -> Option<String> {
        let key = format!("{}:{}", family, name);
        self.learned
            .get(&key)
            .or_else(|| self.defaults.get(&key))
            .cloned()
    }

    pub fn learn(&mut self, family: &str, name: &str, package: &str) {
        let key = format!("{}:{}", family, name);
        if self.learned.get(&key).map(String::as_str) == Some(package) {
            return;
        }
        self.learned.insert(key, package.to_string());
        self.negative_cache.retain(|item| item != package);
        self.save();
    }

    /// Merkt sich Paketnamen, die auf keiner Distro installierbar waren.
    pub fn learn_negative(&mut self, package: &str) {
        if package.is_empty() || self.negative_cache.iter().any(|p| p == package) {
            return;
        }
        self.negative_cache.push(package.to_string());
        self.save();
    }

    pub fn is_negative(&self, package: &str) -> bool {
        self.negative_cache.iter().any(|p| p == package)
    }

    /// Merkt sich einen allgemeinen Build-Fix (z. B. `cstdint`, `cmake_policy`).
    pub fn learn_build_fix(&mut self, key: &str, value: &str) -> bool {
        if self.build_fixes.get(key).map(String::as_str) == Some(value) {
            return false;
        }
        self.build_fixes.insert(key.to_string(), value.to_string());
        self.save();
        true
    }

    pub fn build_fix(&self, key: &str) -> Option<String> {
        self.build_fixes.get(key).cloned()
    }

    pub fn learned_rule_count(&self) -> usize {
        self.learned.len() + self.negative_cache.len() + self.build_fixes.len()
    }
}

/// Kandidaten der Such-Kaskade Stufe 1: kleingeschriebener Name und die
/// üblichen Dev-Paket-Suffixe der Distribution.
pub fn candidate_names(dep: &MissingDep, package_manager: &str) -> Vec<String> {
    let base = dep.search_term().to_lowercase();
    let mut names = Vec::new();
    if let MissingDep::Tool(_) = dep {
        if let Some(mapped) = distro_package_name(&base, package_manager) {
            names.push(mapped.to_string());
        }
        names.push(base);
        return names;
    }
    if let Some(mapped) = distro_package_name(&base, package_manager) {
        names.push(mapped.to_string());
    }
    names.push(base.clone());
    match package_manager {
        "apt" => {
            names.push(format!("lib{}-dev", base));
            names.push(format!("{}-dev", base));
            names.push(format!("lib{}", base));
        }
        "dnf" | "zypper" => {
            names.push(format!("{}-devel", base));
            names.push(format!("lib{}-devel", base));
        }
        "pacman" => names.push(format!("lib{}", base)),
        _ => {}
    }
    let mut unique: Vec<String> = Vec::new();
    for name in names {
        if !name.is_empty() && !unique.contains(&name) {
            unique.push(name);
        }
    }
    unique
}

/// Stufe 2 und 3: gezielte Suche nach CMake-Konfigurationsdateien,
/// `.pc`-Dateien und Headern in der Paketdatenbank des Containers.
pub fn probe_commands(package_manager: &str, dep: &MissingDep) -> Vec<String> {
    let mut files: Vec<(String, bool)> = Vec::new();
    match dep {
        MissingDep::CMakeModule(module) => {
            files.push((format!("{}Config.cmake", module), false));
            files.push((format!("{}-config.cmake", module.to_lowercase()), false));
        }
        MissingDep::Header(header) => {
            files.push((header.rsplit('/').next().unwrap_or(header).to_string(), true));
        }
        MissingDep::PkgConfig(pkg) => files.push((format!("{}.pc", pkg.to_lowercase()), false)),
        MissingDep::Tool(tool) => {
            let name = tool.to_lowercase();
            files.push((format!("bin/{}", name), true));
            files.push((format!("usr/bin/{}", name), true));
        }
    }
    files.retain(|(file, _)| {
        !file.is_empty()
            && file
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_+./".contains(c))
    });
    files
        .into_iter()
        .map(|(file, is_header)| match package_manager {
            // `-Fx` = exakte Zeichenkette, kein Regex - verhindert Fehltreffer.
            "pacman" => {
                if is_header {
                    format!("pacman -Fx '/{}'", file)
                } else {
                    format!("pacman -Fx '{}'", file)
                }
            }
            "apt" => {
                if is_header {
                    format!("apt-file search '/{}'", file)
                } else {
                    format!("apt-file search '{}'", file)
                }
            }
            "dnf" => {
                if is_header {
                    format!("dnf provides '*/{}'", file)
                } else {
                    format!("dnf provides '*{}'", file)
                }
            }
            "zypper" => {
                if is_header {
                    format!("zypper search --provides --match-exact '{}'", file)
                } else {
                    format!("zypper search --provides '{}'", file)
                }
            }
            _ => String::new(),
        })
        .filter(|command| !command.is_empty())
        .collect()
}

/// Ein Paketname darf nur Buchstaben, Ziffern, `-`, `_`, `+` und `.` enthalten
/// und niemals mit einem Bindestrich beginnen. Alles andere wird verworfen,
/// bevor es an pacman/apt/dnf übergeben wird.
pub fn valid_package_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_+.".contains(c))
}


/// Allgemeiner, dauerhaft lernbarer Build-Fix.
#[derive(Debug, Clone)]
pub struct BuildFix {
    pub key: String,
    pub description: String,
    pub value: String,
}

/// Erkennt wiederkehrende Compiler-/CMake-Fehler und liefert den passenden Fix.
pub fn detect_build_fixes(output: &str) -> Vec<BuildFix> {
    let lower = output.to_ascii_lowercase();
    let mut fixes = Vec::new();

    let missing_int_type = ["uint8_t", "uint16_t", "uint32_t", "int8_t", "int16_t", "int32_t"]
        .iter()
        .any(|name| lower.contains(name));
    if missing_int_type
        && (lower.contains("does not name a type") || lower.contains("was not declared in this scope"))
    {
        fixes.push(BuildFix {
            key: "cstdint".to_string(),
            description: "Fehlender Standard-Header <cstdint>".to_string(),
            value: "-include cstdint".to_string(),
        });
    }

    if is_cmake_policy_error(output) {
        fixes.push(BuildFix {
            key: "cmake_policy".to_string(),
            description: "CMake-Policy-/Prefix-Fehler".to_string(),
            value: "-DCMAKE_POLICY_VERSION_MINIMUM=3.5 -DCMAKE_SUPPRESS_DEVELOPER_ERRORS=ON"
                .to_string(),
        });
    }

    if lower.contains("no such file or directory")
        && ["third_party/", "extern/", "libs/"].iter().any(|dir| lower.contains(dir))
    {
        fixes.push(BuildFix {
            key: "submodules".to_string(),
            description: "Fehlende Git-Submodule im Quellcode".to_string(),
            value: "git submodule update --init --recursive".to_string(),
        });
    }

    fixes
}


/// Spezifischer CMake-4.x-Fehler: source_group ist kein Prefix des Dateipfads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceGroupError {
    pub file: String,
    pub line: usize,
    /// Absoluter Dateipfad aus der Fehlermeldung, falls vorhanden.
    pub hint: Option<String>,
}

/// Erkennt `CMake Error at <datei>:<zeile> (source_group): ... is not a prefix of file`.
pub fn detect_source_group_error(output: &str) -> Option<SourceGroupError> {
    // Zeilenbasiert: funktioniert mit rohem Log genauso wie mit `[err]`/`[out]`
    // präfigierten Zeilen und mit über mehrere Zeilen verteilten Meldungen.
    let lines: Vec<&str> = output.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        let Some(position) = line.find("CMake Error at ") else {
            continue;
        };
        if !line[position..].contains("(source_group)") {
            continue;
        }
        let rest = &line[position + "CMake Error at ".len()..];
        let token = rest.split_whitespace().next()?;
        let (file, line_text) = token.split_once(':')?;
        let line_number: usize = line_text.parse().ok()?;

        let end = (index + 6).min(lines.len());
        let window = &lines[index..end];
        if !window
            .iter()
            .any(|candidate| candidate.to_ascii_lowercase().contains("is not a prefix of file"))
        {
            continue;
        }
        let window_text = window.join("\n");
        let hint_re = Regex::new(r#"is not a prefix of file[^"]*"([^"]+)""#).ok();
        let hint = hint_re
            .and_then(|regex| regex.captures(&window_text))
            .map(|found| found[1].to_string());
        return Some(SourceGroupError {
            file: file.to_string(),
            line: line_number,
            hint,
        });
    }
    None
}

/// Bekannte CMake-4.x-Policy-Fehler inkl. verschachtelter ExternalProjects.
pub fn is_cmake_policy_error(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    (lower.contains("source_group") && lower.contains("is not a prefix of file"))
        || lower.contains("compatibility with cmake < 3.10")
        || lower.contains("compatibility with cmake <3.10")
}

/// Allgemeine Wörter sind keine Kommandozeilen-Werkzeuge.
fn is_generic_tool_word(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "tool"
            | "tools"
            | "command"
            | "program"
            | "package"
            | "library"
            | "file"
            | "module"
            | "dependency"
            | "dependencies"
            | "required"
            | "found"
            | "not"
            | "or"
            | "too"
            | "old"
            | "in"
            | "on"
            | "the"
            | "a"
            | "an"
            | "version"
            | "path"
            | "installed"
            | "binary"
            | "header"
            | "cmake"
            | "pkgconfig"
            | "configure"
    )
}

pub fn analyze_output(output: &str) -> Vec<MissingDep> {
    let header_re = Regex::new(r"fatal error:\s*([^\s:]+):\s*No such file or directory").unwrap();
    let pkg_re = Regex::new(r"No package '([^']+)' found").unwrap();
    let pkg2_re = Regex::new(r"Package '([^']+)'[^\n]*not found").unwrap();
    let cmake_find_re = Regex::new(r"(?i)Could\s+NOT\s+find\s+([A-Za-z0-9_+\-]+)").unwrap();
    let cmake_config_re = Regex::new(
        r#"(?i)Could not find a package configuration file provided by "([A-Za-z0-9_+\-]+)""#,
    )
    .unwrap();
    // `nasm/yasm not found or too old`, `<tool> not found`,
    // `configure: error: ... <tool> not found`, `command not found: <tool>`.
    let tool_re = Regex::new(
        r"(?i)\b([A-Za-z0-9_+\-]+)(?:/[A-Za-z0-9_+\-]+)?(?:\s+(?:or|too|old|in|on|the|PATH|path|version|installed))*\s+not found(?:\s+(?:in|on|the|PATH|path))?",
    )
    .unwrap();
    let cmd_tool_re = Regex::new(r"(?i)command not found:\s*([A-Za-z0-9_+\-]+)").unwrap();
    let shell_cmd_re = Regex::new(r"(?i)([A-Za-z0-9_+.\-]+):\s*command not found").unwrap();

    let mut deps: Vec<MissingDep> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        // Erfolgsmeldungen wie `-- Found EXPAT: ... (found version "2.10.0")`
        // sind keine Fehler und dürfen nie in den Resolver gelangen.
        if trimmed.to_ascii_lowercase().starts_with("-- found ") {
            continue;
        }

        for caps in header_re.captures_iter(trimmed) {
            push_unique(&mut deps, MissingDep::Header(caps[1].to_string()));
        }
        for caps in pkg_re.captures_iter(trimmed) {
            push_unique(&mut deps, MissingDep::PkgConfig(caps[1].to_string()));
        }
        for caps in pkg2_re.captures_iter(trimmed) {
            push_unique(&mut deps, MissingDep::PkgConfig(caps[1].to_string()));
        }
        for caps in cmake_find_re.captures_iter(trimmed) {
            let name = &caps[1];
            // `Could not find a package configuration file ...` liefert `a`.
            if name.eq_ignore_ascii_case("a") {
                continue;
            }
            push_unique(&mut deps, MissingDep::CMakeModule(name.to_string()));
        }
        for caps in cmake_config_re.captures_iter(trimmed) {
            push_unique(&mut deps, MissingDep::CMakeModule(caps[1].to_string()));
        }
        for caps in tool_re.captures_iter(trimmed) {
            let name = &caps[1];
            if !is_generic_tool_word(name) {
                push_unique(&mut deps, MissingDep::Tool(name.to_string()));
            }
        }
        for caps in cmd_tool_re.captures_iter(trimmed) {
            let name = &caps[1];
            if !is_generic_tool_word(name) {
                push_unique(&mut deps, MissingDep::Tool(name.to_string()));
            }
        }
        for caps in shell_cmd_re.captures_iter(trimmed) {
            let name = &caps[1];
            if !is_generic_tool_word(name) {
                push_unique(&mut deps, MissingDep::Tool(name.to_string()));
            }
        }
    }

    deps
}

fn push_unique(list: &mut Vec<MissingDep>, dep: MissingDep) {
    if !list.contains(&dep) {
        list.push(dep);
    }
}

pub fn search_command(package_manager: &str, dep: &MissingDep) -> String {
    let term = dep.search_term();
    match package_manager {
        "pacman" => format!(
            "pacman -Ssq '^{}$' || pacman -Ssq '{}' || pacman -Fx '{}Config.cmake' || pacman -Fx '{}'",
            term, term, term, term
        ),
        "apt" => format!(
            "apt-cache search '{}' || apt-file search '{}'",
            term, term
        ),
        "dnf" => format!("dnf provides '*/{}' || dnf search '{}'", term, term),
        "zypper" => format!(
            "zypper search --provides --match-exact '{}' || zypper search '{}'",
            term, term
        ),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erkennt_fehlenden_header() {
        let out = "src/main.c:3:10: fatal error: openssl/sha.h: No such file or directory\ncompilation terminated.\n";
        assert_eq!(analyze_output(out), vec![MissingDep::Header("openssl/sha.h".into())]);
    }

    #[test]
    fn erkennt_pkgconfig_fehler() {
        let out = "Package 'libcurl', required by 'virtual:world', not found\n";
        assert_eq!(analyze_output(out), vec![MissingDep::PkgConfig("libcurl".into())]);
    }

    #[test]
    fn erkennt_cmake_modul() {
        let out = "CMake Error: Could NOT find ZLIB (missing: ZLIB_LIBRARY)\n";
        assert!(analyze_output(out)
            .iter()
            .any(|d| matches!(d, MissingDep::CMakeModule(m) if m == "ZLIB")));
    }

    #[test]
    fn suchbegriffe_sind_kleingeschrieben() {
        assert_eq!(MissingDep::CMakeModule("Boost".into()).search_term(), "boost");
        assert_eq!(MissingDep::CMakeModule("OpenSSL".into()).search_term(), "openssl");
        assert_eq!(MissingDep::PkgConfig("CURL".into()).search_term(), "curl");
        assert_eq!(MissingDep::CMakeModule("Eigen3Config".into()).search_term(), "eigen3");
    }

    #[test]
    fn cmake_module_werden_auf_distro_pakte_abgebildet() {
        assert_eq!(distro_package_name("boost", "pacman"), Some("boost"));
        assert_eq!(distro_package_name("boost", "apt"), Some("libboost-dev"));
        assert_eq!(distro_package_name("boost", "dnf"), Some("boost-devel"));
        assert_eq!(distro_package_name("eigen3", "pacman"), Some("eigen"));
        assert_eq!(distro_package_name("eigen3", "apt"), Some("libeigen3-dev"));
        assert_eq!(distro_package_name("eigen3", "dnf"), Some("eigen3-devel"));
        assert_eq!(distro_package_name("wxwidgets", "pacman"), Some("wxwidgets-gtk3"));
        assert_eq!(distro_package_name("opengl", "apt"), Some("libgl-dev"));
        assert_eq!(distro_package_name("opengl", "pacman"), Some("libglvnd"));
    }

    #[test]
    fn suchbefehle_nutzen_kleinbuchstaben() {
        let dep = MissingDep::CMakeModule("Boost".into());
        let pacman = search_command("pacman", &dep);
        assert!(pacman.contains("pacman -Ssq '^boost$'"), "{}", pacman);
        assert!(!pacman.contains("Boost"));
        assert!(search_command("apt", &dep).contains("apt-cache search 'boost'"));
        assert!(search_command("zypper", &dep).contains("zypper search 'boost'"));
    }

    #[test]
    fn paketname_aus_suchergebnis_wird_gezogen() {
        let apt = "libboost1.74-dev: /usr/include/boost/version.hpp\n";
        assert_eq!(
            package_from_search(apt, "boost", "apt"),
            Some("libboost1.74-dev".to_string())
        );
        let pacman = "extra/boost: /usr/include/boost/version.hpp\n";
        assert_eq!(
            package_from_search(pacman, "boost", "pacman"),
            Some("boost".to_string())
        );
        let plain = "usr/include/boost/version.hpp\nboost\n";
        assert_eq!(
            package_from_search(plain, "boost", "pacman"),
            Some("boost".to_string())
        );
    }

    #[test]
    fn standard_wissensbasis_enthlt_klassiker() {
        let defaults: HashMap<String, String> = serde_json::from_str(DEFAULT_MAPPINGS).unwrap();
        assert_eq!(defaults.get("arch:Boost").map(String::as_str), Some("boost"));
        assert_eq!(defaults.get("debian:Eigen3").map(String::as_str), Some("libeigen3-dev"));
        assert_eq!(defaults.get("fedora:wxWidgets").map(String::as_str), Some("wxGTK-devel"));
        assert_eq!(defaults.get("arch:OpenGL").map(String::as_str), Some("libglvnd"));
    }

    #[test]
    fn erkennt_source_group_konflikt() {
        let log = concat!(
            "[err] CMake Error at libs/locale/CMakeLists.txt:231 (source_group):\n",
            "[out]   source_group\n",
            "[out]   \"/work/deps-build/dep_example/libs/locale/include/example/locale\"\n",
            "[out]   is not a prefix of file\n",
            "[out]   \"/work/deps-build/dep_example/libs/locale/include/example/locale.hpp\"\n"
        );
        let error = detect_source_group_error(log).expect("source_group Fehler");
        assert_eq!(error.file, "libs/locale/CMakeLists.txt");
        assert_eq!(error.line, 231);
        assert_eq!(
            error.hint.as_deref(),
            Some("/work/deps-build/dep_example/libs/locale/include/example/locale.hpp")
        );
        let raw = concat!(
            "CMake Error at libs/locale/CMakeLists.txt:231 (source_group):\n",
            "  source_group\n",
            "  \"/work/deps-build/dep_example/libs/locale/include/example/locale\"\n",
            "  is not a prefix of file\n",
            "  \"/work/deps-build/dep_example/libs/locale/include/example/locale.hpp\"\n"
        );
        let raw_error = detect_source_group_error(raw).expect("roher Block");
        assert_eq!(raw_error.file, "libs/locale/CMakeLists.txt");
        assert_eq!(raw_error.line, 231);
        assert!(detect_source_group_error("Could NOT find PNG").is_none());
    }

    #[test]
    fn erkennt_allgemeine_build_fixes() {
        let cstdint = "error: 'uint8_t' does not name a type";
        let fixes = detect_build_fixes(cstdint);
        assert!(fixes.iter().any(|fix| fix.key == "cstdint"));
        assert!(detect_build_fixes("source_group \"x\" is not a prefix of file \"y\"")
            .iter()
            .any(|fix| fix.key == "cmake_policy"));
        assert!(detect_build_fixes("third_party/foo.h: No such file or directory")
            .iter()
            .any(|fix| fix.key == "submodules"));
        assert!(detect_build_fixes("all good").is_empty());
    }

    #[test]
    fn mapping_store_lernt_und_speichert() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("git2pkg-map-{}-{}.json", std::process::id(), nanos));
        let mut store = MappingStore::from_path(path.clone());
        assert!(path.exists(), "Datei muss automatisch angelegt werden");
        assert_eq!(store.get("arch", "Boost").as_deref(), Some("boost"));
        store.learn("arch", "MyLib", "mylib-dev");
        assert_eq!(store.get("arch", "MyLib").as_deref(), Some("mylib-dev"));
        store.learn_negative("bogus-pkg");
        assert!(store.is_negative("bogus-pkg"));
        assert!(store.learn_build_fix("cstdint", "-include cstdint"));
        assert_eq!(store.build_fix("cstdint").as_deref(), Some("-include cstdint"));
        assert!(store.learned_rule_count() >= 3);
        let json = fs::read_to_string(&path).unwrap();
        assert!(json.contains("packages"));
        assert!(json.contains("negative_cache"));
        assert!(json.contains("build_fixes"));
        let reloaded = MappingStore::from_path(path.clone());
        assert_eq!(reloaded.get("arch", "MyLib").as_deref(), Some("mylib-dev"));
        assert!(reloaded.is_negative("bogus-pkg"));
        assert_eq!(reloaded.build_fix("cstdint").as_deref(), Some("-include cstdint"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn kaskade_baut_kandidaten_und_suchbefehle() {
        let dep = MissingDep::CMakeModule("PNG".into());
        assert_eq!(family_key(&DistroFamily::Arch), "arch");
        assert_eq!(family_key(&DistroFamily::Debian), "debian");
        assert_eq!(family_key(&DistroFamily::Fedora), "fedora");
        assert_eq!(family_key(&DistroFamily::Suse), "suse");

        let arch = candidate_names(&dep, "pacman");
        assert!(arch.contains(&"libpng".to_string()));

        let debian = candidate_names(&dep, "apt");
        assert!(debian.contains(&"libpng-dev".to_string()));
        assert!(debian.contains(&"png-dev".to_string()));

        let fedora = candidate_names(&dep, "dnf");
        assert!(fedora.contains(&"png-devel".to_string()));
        assert!(fedora.contains(&"libpng-devel".to_string()));

        let suse = candidate_names(&dep, "zypper");
        assert!(suse.contains(&"png-devel".to_string()));
        assert!(suse.contains(&"libpng-devel".to_string()));

        let header = MissingDep::Header("openssl/sha.h".into());
        assert!(probe_commands("pacman", &header)
            .iter()
            .any(|c| c == "pacman -Fx '/sha.h'"));
        assert!(probe_commands("apt", &header)
            .iter()
            .any(|c| c == "apt-file search '/sha.h'"));
        assert!(probe_commands("dnf", &header)
            .iter()
            .any(|c| c == "dnf provides '*/sha.h'"));
        assert!(probe_commands("zypper", &header)
            .iter()
            .any(|c| c == "zypper search --provides --match-exact 'sha.h'"));

        let commands = probe_commands("pacman", &dep);
        assert!(commands.iter().any(|c| c.contains("PNGConfig.cmake")));
        let pc = MissingDep::PkgConfig("ssl".into());
        assert!(probe_commands("dnf", &pc)
            .iter()
            .any(|c| c.contains("ssl.pc")));
    }

    #[test]
    fn cmake_policy_fehler_werden_erkannt() {
        assert!(is_cmake_policy_error(
            "CMake Error at libs/locale/CMakeLists.txt:231 (source_group): source_group \"x\" is not a prefix of file \"y\""
        ));
        assert!(is_cmake_policy_error(
            "Compatibility with CMake < 3.10 will be removed"
        ));
        assert!(!is_cmake_policy_error("Could NOT find PNG"));
    }

    #[test]
    fn configure_tool_fehler_werden_erkannt() {
        let out = "nasm/yasm not found or too old\nconfigure: error: yasm not found\nbash: nasm: command not found: nasm\n";
        let deps = analyze_output(out);
        assert!(deps.contains(&MissingDep::Tool("nasm".to_string())));
        assert!(deps.contains(&MissingDep::Tool("yasm".to_string())));
        assert!(!deps.iter().any(|dep| matches!(dep, MissingDep::Tool(t) if is_generic_tool_word(t))));

        let shell = analyze_output("/work/build.sh: line 11: cargo: command not found");
    assert!(shell.contains(&MissingDep::Tool("cargo".to_string())));
    let tool = MissingDep::Tool("nasm".to_string());
        assert_eq!(tool.search_term(), "nasm");
        assert!(probe_commands("pacman", &tool)
            .contains(&"pacman -Fx '/bin/nasm'".to_string()));
        assert!(probe_commands("apt", &tool)
            .contains(&"apt-file search '/bin/nasm'".to_string()));
        assert!(probe_commands("dnf", &tool)
            .contains(&"dnf provides '*/bin/nasm'".to_string()));
        assert!(probe_commands("zypper", &tool)
            .contains(&"zypper search --provides --match-exact 'bin/nasm'".to_string()));
    }

    #[test]
    fn positive_found_zeilen_werden_nicht_als_fehler_geparst() {
        let out = "-- Found EXPAT: /usr/lib/libexpat.so (found version \"2.10.0\")\n-- Found ZLIB: /usr/lib/libz.so (found version \"1.3.1\")\n";
        assert!(analyze_output(out).is_empty());
    }

    #[test]
    fn alle_cmake_fehler_eines_laufs_werden_erfasst() {
        let out = "CMake Error at cmake/deps.cmake:10 (find_package):\n  Could NOT find PNG (missing: PNG_LIBRARY)\n-- Could NOT find Eigen3 (missing: EIGEN3_INCLUDE_DIR)\nCould not find a package configuration file provided by \"Foo\" with any of\n";
        let deps = analyze_output(out);
        let modules: Vec<&str> = deps
            .iter()
            .filter_map(|dep| match dep {
                MissingDep::CMakeModule(m) => Some(m.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(modules, vec!["PNG", "Eigen3", "Foo"]);
    }

    #[test]
    fn ungueltige_paketnamen_werden_abgelehnt() {
        assert!(valid_package_name("libpng-dev"));
        assert!(valid_package_name("libjpeg-turbo"));
        assert!(!valid_package_name(""));
        assert!(!valid_package_name("-foundexpat"));
        assert!(!valid_package_name("--foundexpat:/usr/lib/libexpat.so"));
        assert!(!valid_package_name("foo bar"));
        assert!(!valid_package_name("foo(bar)"));
        assert!(!valid_package_name("extra/foo"));
        assert!(!valid_package_name("libfoo:amd64"));
    }

    #[test]
    fn duplicate_werden_entfernt() {
        let out = "No package 'ssl' found\nNo package 'ssl' found\n";
        assert_eq!(analyze_output(out).len(), 1);
    }
}
