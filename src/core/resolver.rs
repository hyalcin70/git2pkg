//! Regex-basierte Analyse von Compiler-Ausgaben: fehlende Header,
//! pkg-config-Pakete und CMake-Module.

use crate::core::distro::DistroFamily;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Mitgelieferte Start-Wissensbasis; lokale Learnings ergänzen diese.
const DEFAULT_MAPPINGS: &str = include_str!("../../assets/default_mappings.json");

/// Obergrenzen für die Lerndatei. Sie verhindern unbegrenztes Wachstum; bei
/// Überschreitung werden die am längsten nicht mehr benutzten Einträge entfernt.
const MAX_LEARNED_PACKAGES: usize = 2000;
const MAX_NEGATIVE_ENTRIES: usize = 512;
const MAX_BUILD_FIXES: usize = 128;
const MAX_REPO_PROFILES: usize = 512;
const MAX_BUILD_RECIPES: usize = 512;

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
            MissingDep::PkgConfig(p) | MissingDep::CMakeModule(p) => p
                .trim_end_matches("-config")
                .trim_end_matches("Config")
                .to_ascii_lowercase(),
            MissingDep::Tool(t) => t.to_ascii_lowercase(),
        }
    }
}

/// Häufige CMake-Module auf die passenden Distribution-Pakete abbilden.
/// Kleingeschriebene Modulnamen fallen auf den Namen selbst zurück.
pub fn distro_package_name(name: &str, package_manager: &str) -> Option<&'static str> {
    match (package_manager, name.to_ascii_lowercase().as_str()) {
        ("pacman", "boost") | ("pacman", "boost_include_dir") => Some("boost"),
        ("pacman", "glew") => Some("glew"),
        ("pacman", "glfw") | ("pacman", "glfw3") => Some("glfw"),
        ("pacman", "tbb") => Some("tbb"),
        ("pacman", "opencv") => Some("opencv"),
        ("pacman", "freetype") => Some("freetype2"),
        ("pacman", "expat") => Some("expat"),
        ("pacman", "bzip2") => Some("bzip2"),
        ("pacman", "liblzma") | ("pacman", "lzma") => Some("xz"),
        ("pacman", "gtk2") => Some("gtk2"),
        ("pacman", "gtk3") => Some("gtk3"),
        ("pacman", "webkit2gtk") => Some("webkit2gtk"),
        ("pacman", "cereal") => Some("cereal"),
        ("pacman", "qhull") => Some("qhull"),
        ("pacman", "nlopt") => Some("nlopt"),
        ("pacman", "assimp") => Some("assimp"),
        ("pacman", "ffmpeg") => Some("ffmpeg"),
        ("pacman", "eigen3") | ("pacman", "eigen") => Some("eigen"),
        ("pacman", "openssl") | ("pacman", "ssl") => Some("openssl"),
        ("pacman", "curl") | ("pacman", "libcurl") => Some("curl"),
        ("pacman", "zlib") => Some("zlib"),
        ("pacman", "jpeg") => Some("libjpeg-turbo"),
        ("pacman", "png") | ("pacman", "libpng") => Some("libpng"),
        ("pacman", "opengl") => Some("libglvnd"),
        ("pacman", "wxwidgets") => Some("wxwidgets-gtk3"),

        ("apt", "boost") | ("apt", "boost_include_dir") => Some("libboost-all-dev"),
        ("apt", "glew") => Some("libglew-dev"),
        ("apt", "glfw") | ("apt", "glfw3") => Some("libglfw3-dev"),
        ("apt", "tbb") => Some("libtbb-dev"),
        ("apt", "opencv") => Some("libopencv-dev"),
        ("apt", "freetype") => Some("libfreetype-dev"),
        ("apt", "expat") => Some("libexpat1-dev"),
        ("apt", "bzip2") => Some("libbz2-dev"),
        ("apt", "liblzma") | ("apt", "lzma") => Some("liblzma-dev"),
        ("apt", "gtk2") => Some("libgtk2.0-dev"),
        ("apt", "gtk3") => Some("libgtk-3-dev"),
        ("apt", "webkit2gtk") => Some("libwebkit2gtk-4.0-dev"),
        ("apt", "cereal") => Some("libcereal-dev"),
        ("apt", "qhull") => Some("libqhull-dev"),
        ("apt", "nlopt") => Some("libnlopt-dev"),
        ("apt", "assimp") => Some("libassimp-dev"),
        ("apt", "ffmpeg") => Some("libavformat-dev"),
        ("apt", "eigen3") | ("apt", "eigen") => Some("libeigen3-dev"),
        ("apt", "openssl") | ("apt", "ssl") => Some("libssl-dev"),
        ("apt", "curl") | ("apt", "libcurl") => Some("libcurl4-openssl-dev"),
        ("apt", "zlib") => Some("zlib1g-dev"),
        ("apt", "jpeg") => Some("libjpeg-dev"),
        ("apt", "png") | ("apt", "libpng") => Some("libpng-dev"),
        ("apt", "opengl") => Some("libgl-dev"),
        ("apt", "wxwidgets") => Some("libwxgtk3.2-dev"),

        ("dnf", "boost") | ("dnf", "boost_include_dir") => Some("boost-devel"),
        ("dnf", "glew") => Some("glew-devel"),
        ("dnf", "glfw") | ("dnf", "glfw3") => Some("glfw-devel"),
        ("dnf", "tbb") => Some("tbb-devel"),
        ("dnf", "opencv") => Some("opencv-devel"),
        ("dnf", "freetype") => Some("freetype-devel"),
        ("dnf", "expat") => Some("expat-devel"),
        ("dnf", "bzip2") => Some("bzip2-devel"),
        ("dnf", "liblzma") | ("dnf", "lzma") => Some("xz-devel"),
        ("dnf", "gtk2") => Some("gtk2-devel"),
        ("dnf", "gtk3") => Some("gtk3-devel"),
        ("dnf", "webkit2gtk") => Some("webkit2gtk3-devel"),
        ("dnf", "cereal") => Some("cereal-devel"),
        ("dnf", "qhull") => Some("qhull-devel"),
        ("dnf", "nlopt") => Some("nlopt-devel"),
        ("dnf", "assimp") => Some("assimp-devel"),
        ("dnf", "ffmpeg") => Some("ffmpeg-devel"),
        ("dnf", "eigen3") | ("dnf", "eigen") => Some("eigen3-devel"),
        ("dnf", "openssl") | ("dnf", "ssl") => Some("openssl-devel"),
        ("dnf", "curl") | ("dnf", "libcurl") => Some("libcurl-devel"),
        ("dnf", "zlib") => Some("zlib-devel"),
        ("dnf", "jpeg") => Some("libjpeg-turbo-devel"),
        ("dnf", "png") | ("dnf", "libpng") => Some("libpng-devel"),
        ("dnf", "opengl") => Some("mesa-libGL-devel"),
        ("dnf", "wxwidgets") => Some("wxGTK-devel"),

        ("zypper", "boost") | ("zypper", "boost_include_dir") => Some("boost-devel"),
        ("zypper", "glew") => Some("glew-devel"),
        ("zypper", "glfw") | ("zypper", "glfw3") => Some("libglfw3-devel"),
        ("zypper", "tbb") => Some("tbb-devel"),
        ("zypper", "opencv") => Some("opencv-devel"),
        ("zypper", "freetype") => Some("freetype2-devel"),
        ("zypper", "expat") => Some("libexpat-devel"),
        ("zypper", "bzip2") => Some("libbz2-devel"),
        ("zypper", "liblzma") | ("zypper", "lzma") => Some("liblzma-devel"),
        ("zypper", "gtk2") => Some("gtk2-devel"),
        ("zypper", "gtk3") => Some("gtk3-devel"),
        ("zypper", "webkit2gtk") => Some("webkit2gtk3-devel"),
        ("zypper", "cereal") => Some("cereal-devel"),
        ("zypper", "qhull") => Some("qhull-devel"),
        ("zypper", "nlopt") => Some("nlopt-devel"),
        ("zypper", "assimp") => Some("assimp-devel"),
        ("zypper", "ffmpeg") => Some("ffmpeg-devel"),
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
        if package_name_matches(&name, term) {
            return Some(name);
        }
    }
    None
}

/// Prüft, ob ein Paketname wirklich zum Suchbegriff gehört. Reine
/// Teilstring-Treffer sind zu unscharf (`ecm` steckt auch in `doublecmd`).
/// Prüft, ob ein Paketname wirklich zum Suchbegriff gehört. Der Begriff muss
/// am Anfang oder Ende stehen (inkl. `lib`-Präfix); Treffer mitten im Namen
/// sind zu unscharf (`ecm` steckt auch in `doublecmd`).
fn package_name_matches(name: &str, term: &str) -> bool {
    if name == term {
        return true;
    }
    if name.starts_with(term) || name.starts_with(&format!("lib{}", term)) {
        return true;
    }
    if name.ends_with(term) {
        return true;
    }
    // Suchbegriff enthält den Paketnamen, z. B. `libpng` -> `png`.
    term.ends_with(name) && name.len() >= 3
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

/// Normalisiert eine Git-URL, damit der Profil-Hash stabil bleibt.
fn normalize_repo_url(url: &str) -> String {
    let mut value = url.trim().to_ascii_lowercase();
    for prefix in ["https://", "http://", "git://", "ssh://"] {
        if let Some(rest) = value.strip_prefix(prefix) {
            value = rest.to_string();
            break;
        }
    }
    if let Some(at) = value.find('@') {
        value = value[at + 1..].to_string();
    }
    while value.ends_with('/') {
        value.pop();
    }
    if value.ends_with(".git") {
        value.truncate(value.len() - 4);
    }
    value
}

/// SHA-256 des normalisierten Repository-Namens - ohne weitere Abhängigkeit.
fn sha256_hex(input: &str) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut message = normalize_repo_url(input).into_bytes();
    let bit_len = (message.len() as u64).saturating_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    for chunk in message.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (index, word) in w[..16].iter_mut().enumerate() {
            *word = u32::from_be_bytes([
                chunk[index * 4],
                chunk[index * 4 + 1],
                chunk[index * 4 + 2],
                chunk[index * 4 + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d) = (h[0], h[1], h[2], h[3]);
        let (mut e, mut f, mut g, mut hh) = (h[4], h[5], h[6], h[7]);
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(choice)
                .wrapping_add(K[index])
                .wrapping_add(w[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(majority);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    h.iter().map(|word| format!("{word:08x}")).collect()
}

fn repo_profile_key(url: &str) -> String {
    sha256_hex(url)
}

/// Anonymer, stabiler Schlüssel für ein Repository (ohne Klartextnamen).
pub fn repo_hash(url: &str) -> String {
    repo_profile_key(url)
}

/// Nur kompilierte Build-Systeme werden für unvollständige Pakete profiliert.
pub fn is_compiled_build_system(label: &str) -> bool {
    let value = label.to_ascii_lowercase();
    [
        "cmake",
        "cargo",
        "rust",
        "meson",
        "ninja",
        "make",
        "qmake",
        "go",
        "autotools",
    ]
    .iter()
    .any(|name| value.contains(name))
}

/// Erfolgreiche, generische Bauweise für ein Repo oder eine Strukturklasse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BuildRecipe {
    #[serde(default)]
    pub signature: String,
    #[serde(default)]
    pub strategy: String,
    #[serde(default)]
    pub skip_deps: bool,
    #[serde(default)]
    pub cmake_compat: bool,
    #[serde(default)]
    pub cstdint_fix: bool,
    #[serde(default)]
    pub submodule_fix: bool,
    #[serde(default)]
    pub packages: Vec<String>,
    /// Nur vollständig verifizierte Rezepte duerfen spaeter wiederverwendet werden.
    #[serde(default)]
    pub verified: bool,
}

/// Lokale, dauerhafte Paket-Zuordnungen ("Live-Lernen" ohne KI).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct LearnedFile {
    #[serde(default)]
    packages: HashMap<String, String>,
    #[serde(default)]
    negative_cache: Vec<String>,
    #[serde(default)]
    build_fixes: HashMap<String, String>,
    #[serde(default)]
    repo_profiles: HashMap<String, Vec<String>>,
    #[serde(default)]
    build_recipes: HashMap<String, BuildRecipe>,
    /// Unix-Zeitstempel der letzten Nutzung je Sektion (für die LRU-Eviction).
    #[serde(default)]
    last_used: HashMap<String, u64>,
}

/// Veraltete "unvollständig"-Marker aus älteren Lerndateien ignorieren.
/// Ältere Versionen haben Anzeige-Labels statt Strategie-Keys gespeichert.
fn is_obsolete_incomplete_strategy(strategy: &str) -> bool {
    matches!(
        strategy,
        "Standard"
            | "System-Bibliotheken ohne deps/-Vorbau"
            | "Direkter Build-System-Fallback"
            | "Repository-Buildskript"
    )
}

pub struct MappingStore {
    path: PathBuf,
    learned: HashMap<String, String>,
    negative_cache: Vec<String>,
    build_fixes: HashMap<String, String>,
    repo_profiles: HashMap<String, Vec<String>>,
    build_recipes: HashMap<String, BuildRecipe>,
    last_used: HashMap<String, u64>,
    /// Profil-Keys, die dieser Prozess bewusst geleert hat. Das Merge darf sie
    /// nicht aus dem älteren Dateistand wiederherstellen.
    cleared_profiles: HashSet<String>,
    defaults: HashMap<String, String>,
}

impl MappingStore {
    pub fn mappings_path() -> PathBuf {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".config/git2pkg-ai/learned_mappings.json")
    }

    pub fn load() -> Self {
        Self::from_path(Self::mappings_path())
    }

    /// Loescht die gelernte Zuordnungsdatei. Rueckgabe: wurde eine Datei entfernt.
    pub fn reset() -> bool {
        Self::reset_at(&Self::mappings_path())
    }

    pub fn reset_at(path: &Path) -> bool {
        fs::remove_file(path).is_ok()
    }

    fn from_path(path: PathBuf) -> Self {
        let defaults: HashMap<String, String> =
            serde_json::from_str(DEFAULT_MAPPINGS).unwrap_or_default();
        let text = fs::read_to_string(&path).unwrap_or_default();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();

        // Neue datei mit Sektionen; alte flache Dateien werden übernommen.
        let (learned, negative_cache, build_fixes, repo_profiles, build_recipes, last_used) =
            if value.get("packages").is_some()
                || value.get("build_fixes").is_some()
                || value.get("negative_cache").is_some()
                || value.get("repo_profiles").is_some()
                || value.get("build_recipes").is_some()
                || value.get("last_used").is_some()
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
                let profiles: HashMap<String, Vec<String>> = value
                    .get("repo_profiles")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                let profiles = profiles
                    .into_iter()
                    .map(|(key, list)| {
                        let list = list
                            .into_iter()
                            .filter(|entry| !is_obsolete_incomplete_strategy(entry))
                            .collect();
                        (key, list)
                    })
                    .collect();
                let recipes = value
                    .get("build_recipes")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                let last_used = value
                    .get("last_used")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                (packages, negative, fixes, profiles, recipes, last_used)
            } else {
                let flat =
                    serde_json::from_value::<HashMap<String, String>>(value).unwrap_or_default();
                (
                    flat,
                    Vec::new(),
                    HashMap::new(),
                    HashMap::new(),
                    HashMap::new(),
                    HashMap::new(),
                )
            };

        let mut store = Self {
            path,
            learned,
            negative_cache,
            build_fixes,
            repo_profiles,
            build_recipes,
            last_used,
            cleared_profiles: HashSet::new(),
            defaults,
        };
        store.save();
        store
    }

    /// Persistiert den Lernstand. Die Datei wird unter einer Lock-Datei erneut
    /// gelesen, mit dem vorhandenen Stand zusammengeführt und atomar ersetzt.
    /// Dadurch gehen parallele Learnings (GUI + CLI) nicht verloren.
    fn save(&mut self) {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _lock = FileLock::acquire(&lock_path_for(&self.path));
        self.merge_from_disk();
        self.prune();
        let file = LearnedFile {
            packages: self.learned.clone(),
            negative_cache: self.negative_cache.clone(),
            build_fixes: self.build_fixes.clone(),
            repo_profiles: self.repo_profiles.clone(),
            build_recipes: self.build_recipes.clone(),
            last_used: self.last_used.clone(),
        };
        if let Ok(json) = serde_json::to_string_pretty(&file) {
            let tmp = temp_path_for(&self.path);
            if fs::write(&tmp, json).is_ok() && fs::rename(&tmp, &self.path).is_err() {
                let _ = fs::remove_file(&tmp);
            }
        }
    }

    /// Führt Einträge zusammen, die eine andere Instanz seit dem Laden
    /// geschrieben hat. Lokal bekannte Werte haben Vorrang.
    fn merge_from_disk(&mut self) {
        let Ok(text) = fs::read_to_string(&self.path) else {
            return;
        };
        let Ok(disk) = serde_json::from_str::<LearnedFile>(&text) else {
            return;
        };
        for (key, value) in disk.packages {
            self.learned.entry(key).or_insert(value);
        }
        for package in disk.negative_cache {
            // Ein zwischenzeitlich gelerntes Mapping hebt den Negativ-Eintrag auf.
            let learned_now = self.learned.values().any(|value| value == &package);
            if !learned_now && !self.negative_cache.iter().any(|item| item == &package) {
                self.negative_cache.push(package);
            }
        }
        for (key, value) in disk.build_fixes {
            self.build_fixes.entry(key).or_insert(value);
        }
        for (key, list) in disk.repo_profiles {
            if self.cleared_profiles.contains(&key) {
                continue;
            }
            let entry = self.repo_profiles.entry(key).or_default();
            for strategy in list {
                if !is_obsolete_incomplete_strategy(&strategy) && !entry.contains(&strategy) {
                    entry.push(strategy);
                }
            }
        }
        for (key, recipe) in disk.build_recipes {
            self.build_recipes.entry(key).or_insert(recipe);
        }
        for (key, stamp) in disk.last_used {
            let entry = self.last_used.entry(key).or_insert(0);
            *entry = (*entry).max(stamp);
        }
    }

    /// Begrenzt die Lerndatei deterministisch über die Nutzungszeitpunkte.
    fn prune(&mut self) {
        prune_map(
            &mut self.learned,
            "packages",
            MAX_LEARNED_PACKAGES,
            &self.last_used,
        );
        prune_map(
            &mut self.build_fixes,
            "fixes",
            MAX_BUILD_FIXES,
            &self.last_used,
        );
        prune_map(
            &mut self.repo_profiles,
            "profiles",
            MAX_REPO_PROFILES,
            &self.last_used,
        );
        prune_map(
            &mut self.build_recipes,
            "recipes",
            MAX_BUILD_RECIPES,
            &self.last_used,
        );
        prune_negative(
            &mut self.negative_cache,
            MAX_NEGATIVE_ENTRIES,
            &self.last_used,
        );
        let stale: Vec<String> = self
            .last_used
            .keys()
            .filter(|key| !self.tracks_key(key))
            .cloned()
            .collect();
        for key in stale {
            self.last_used.remove(&key);
        }
    }

    fn tracks_key(&self, key: &str) -> bool {
        if let Some(rest) = key.strip_prefix("packages:") {
            return self.learned.contains_key(rest);
        }
        if let Some(rest) = key.strip_prefix("negative:") {
            return self.negative_cache.iter().any(|item| item == rest);
        }
        if let Some(rest) = key.strip_prefix("fixes:") {
            return self.build_fixes.contains_key(rest);
        }
        if let Some(rest) = key.strip_prefix("profiles:") {
            return self.repo_profiles.contains_key(rest);
        }
        if let Some(rest) = key.strip_prefix("recipes:") {
            return self.build_recipes.contains_key(rest);
        }
        false
    }

    fn touch(&mut self, key: String) {
        self.last_used.insert(key, now_secs());
    }

    /// Erst die gelernten Einträge prüfen, dann die mitgelieferte Basis.
    pub fn get(&mut self, family: &str, name: &str) -> Option<String> {
        let key = format!("{}:{}", family, name);
        if let Some(value) = self.learned.get(&key).cloned() {
            self.touch(format!("packages:{}", key));
            return Some(value);
        }
        self.defaults.get(&key).cloned()
    }

    pub fn learn(&mut self, family: &str, name: &str, package: &str) {
        let key = format!("{}:{}", family, name);
        if self.learned.get(&key).map(String::as_str) == Some(package) {
            self.touch(format!("packages:{}", key));
            return;
        }
        self.learned.insert(key.clone(), package.to_string());
        self.negative_cache.retain(|item| item != package);
        self.touch(format!("packages:{}", key));
        self.save();
    }

    /// Merkt sich Paketnamen, die auf keiner Distro installierbar waren.
    pub fn learn_negative(&mut self, package: &str) {
        if package.is_empty() || self.negative_cache.iter().any(|p| p == package) {
            return;
        }
        self.negative_cache.push(package.to_string());
        self.touch(format!("negative:{}", package));
        self.save();
    }

    pub fn is_negative(&self, package: &str) -> bool {
        self.negative_cache.iter().any(|p| p == package)
    }

    /// Merkt sich einen allgemeinen Build-Fix (z. B. `cstdint`, `cmake_policy`).
    pub fn learn_build_fix(&mut self, key: &str, value: &str) -> bool {
        if self.build_fixes.get(key).map(String::as_str) == Some(value) {
            self.touch(format!("fixes:{}", key));
            return false;
        }
        self.build_fixes.insert(key.to_string(), value.to_string());
        self.touch(format!("fixes:{}", key));
        self.save();
        true
    }

    pub fn build_fix(&self, key: &str) -> Option<String> {
        self.build_fixes.get(key).cloned()
    }

    /// Merkt sich, dass eine Strategie für dieses Repository nur ein unvollständiges
    /// Paket erzeugt hat. Der Repository-Name wird nie im Klartext gespeichert.
    pub fn mark_incomplete_strategy(&mut self, url: &str, strategy: &str) {
        let key = repo_profile_key(url);
        let list = self.repo_profiles.entry(key.clone()).or_default();
        if !list.iter().any(|entry| entry == strategy) {
            list.push(strategy.to_string());
            self.cleared_profiles.remove(&key);
            self.touch(format!("profiles:{}", key));
            self.save();
        }
    }

    pub fn has_incomplete_default(&self, url: &str) -> bool {
        self.is_incomplete_strategy(url, "Default")
    }

    pub fn is_incomplete_strategy(&self, url: &str, strategy: &str) -> bool {
        self.repo_profiles
            .get(&repo_profile_key(url))
            .map(|list| list.iter().any(|entry| entry == strategy))
            .unwrap_or(false)
    }

    #[allow(dead_code)]
    pub fn incomplete_strategies(&self, url: &str) -> Vec<String> {
        self.repo_profiles
            .get(&repo_profile_key(url))
            .cloned()
            .unwrap_or_default()
    }

    /// Speichert eine erfolgreiche Bauweise exakt und strukturbasiert.
    pub fn record_build_recipe(&mut self, url: &str, mut recipe: BuildRecipe) {
        let repo_key = format!("repo:{}", repo_hash(url));
        let signature_key = format!("sig:{}", recipe.signature);
        recipe.verified = true;
        let changed = self.build_recipes.get(&repo_key) != Some(&recipe)
            || self.build_recipes.get(&signature_key) != Some(&recipe);
        self.build_recipes.insert(repo_key.clone(), recipe.clone());
        self.build_recipes.insert(signature_key.clone(), recipe);
        self.touch(format!("recipes:{}", repo_key));
        self.touch(format!("recipes:{}", signature_key));
        if changed {
            self.save();
        }
    }

    /// Entfernt eine frueher als unvollstaendig markierte Strategie wieder.
    pub fn clear_incomplete_strategy(&mut self, url: &str, strategy: &str) {
        let key = repo_profile_key(url);
        let changed = self.repo_profiles.get_mut(&key).is_some_and(|list| {
            let before = list.len();
            list.retain(|entry| entry != strategy);
            list.len() != before
        });
        if changed {
            self.cleared_profiles.insert(key);
            self.save();
        }
    }

    /// Merkt sich frisch gelernte Pakete pro Repository, damit sie im naechsten
    /// Anlauf bereits im initialen Setup installiert werden koennen.
    pub fn record_repo_packages(&mut self, url: &str, signature: &str, packages: &[String]) {
        let key = format!("repo:{}", repo_hash(url));
        let verified = self
            .build_recipes
            .get(&key)
            .map(|recipe| recipe.verified)
            .unwrap_or(false);
        let mut recipe = self
            .build_recipes
            .get(&key)
            .cloned()
            .unwrap_or_else(|| BuildRecipe {
                signature: signature.to_string(),
                ..BuildRecipe::default()
            });
        recipe.packages = packages.to_vec();
        recipe.verified = verified;
        self.build_recipes.insert(key.clone(), recipe);
        self.touch(format!("recipes:{}", key));
        self.save();
    }

    /// Erst der exakte Repo-Treffer, dann die allgemeine Struktur-Signatur.
    pub fn apply_learned_recipe(&self, url: &str, signature: &str) -> Option<(BuildRecipe, bool)> {
        let verified = |recipe: Option<&BuildRecipe>| recipe.filter(|r| r.verified).cloned();
        verified(self.build_recipes.get(&format!("repo:{}", repo_hash(url))))
            .map(|recipe| (recipe, true))
            .or_else(|| {
                verified(self.build_recipes.get(&format!("sig:{}", signature)))
                    .map(|recipe| (recipe, false))
            })
    }

    pub fn build_recipe_count(&self) -> usize {
        self.build_recipes
            .iter()
            .filter(|(key, _)| key.starts_with("sig:"))
            .count()
    }

    pub fn repo_profile_count(&self) -> usize {
        self.repo_profiles.values().map(Vec::len).sum()
    }

    pub fn learned_rule_count(&self) -> usize {
        self.learned.len()
            + self.negative_cache.len()
            + self.build_fixes.len()
            + self.repo_profile_count()
            + self.build_recipe_count()
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn lock_path_for(path: &Path) -> PathBuf {
    path.with_extension("json.lock")
}

fn temp_path_for(path: &Path) -> PathBuf {
    path.with_extension("json.tmp")
}

/// Einfache, prozessübergreifende Sperre über eine Lock-Datei. Sie ist nur
/// best-effort: Schlägt das Sperren fehl, wird trotzdem zusammengeführt und
/// atomar geschrieben, damit kein Learning verloren geht.
struct FileLock {
    path: PathBuf,
}

impl FileLock {
    fn acquire(path: &Path) -> Option<Self> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(_) => {
                    return Some(Self {
                        path: path.to_path_buf(),
                    })
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    if is_stale_lock(path) {
                        let _ = fs::remove_file(path);
                        continue;
                    }
                    if Instant::now() >= deadline {
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(_) => return None,
            }
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn is_stale_lock(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .map(|age| age > Duration::from_secs(10))
        .unwrap_or(false)
}

fn prune_map<V>(
    map: &mut HashMap<String, V>,
    prefix: &str,
    limit: usize,
    last_used: &HashMap<String, u64>,
) {
    if map.len() <= limit {
        return;
    }
    let mut keys: Vec<String> = map.keys().cloned().collect();
    keys.sort_by(|a, b| {
        let stamp = |key: &str| {
            last_used
                .get(&format!("{}:{}", prefix, key))
                .copied()
                .unwrap_or(0)
        };
        stamp(a).cmp(&stamp(b)).then_with(|| a.cmp(b))
    });
    for key in keys.into_iter().take(map.len() - limit) {
        map.remove(&key);
    }
}

fn prune_negative(list: &mut Vec<String>, limit: usize, last_used: &HashMap<String, u64>) {
    if list.len() <= limit {
        return;
    }
    let stamp = |package: &str| {
        last_used
            .get(&format!("negative:{}", package))
            .copied()
            .unwrap_or(0)
    };
    let mut order: Vec<usize> = (0..list.len()).collect();
    order.sort_by(|&a, &b| {
        stamp(&list[a])
            .cmp(&stamp(&list[b]))
            .then_with(|| list[a].cmp(&list[b]))
    });
    let mut drop_flags = vec![false; list.len()];
    for index in order.into_iter().take(list.len() - limit) {
        drop_flags[index] = true;
    }
    let mut index = 0;
    list.retain(|_| {
        let keep = !drop_flags[index];
        index += 1;
        keep
    });
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
fn safe_tool_name(tool: &str) -> Option<String> {
    let name = tool.trim();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_+.".contains(c))
    {
        return None;
    }
    Some(name.to_string())
}

/// Paketmanager-Dateidatenbank nach dem Binary eines fehlenden Tools fragen.
pub fn tool_file_database_commands(package_manager: &str, tool: &str) -> Vec<String> {
    let Some(name) = safe_tool_name(tool) else {
        return Vec::new();
    };
    let path = format!("/usr/bin/{name}");
    match package_manager {
        "pacman" => vec![
            "pacman -Fy --noconfirm".to_string(),
            format!("pacman -Fq '{path}'"),
        ],
        "apt" => vec![format!("apt-file search '{path}'")],
        "dnf" => vec![format!("dnf provides '{path}'")],
        "zypper" => vec![format!("zypper se --provides '{path}'")],
        _ => Vec::new(),
    }
}

/// Parst die Paketnamen aus der Ausgabe der Dateidatenbank.
pub fn packages_from_file_database(output: &str, tool: &str) -> Vec<String> {
    if safe_tool_name(tool).is_none() {
        return Vec::new();
    }
    let mut packages: Vec<String> = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let candidate = if let Some((_, rest)) = line.split_once("is owned by ") {
            rest.split_whitespace().next().unwrap_or("")
        } else if let Some((head, _)) = line.split_once(':') {
            head.rsplit('/').next().unwrap_or(head)
        } else {
            let token = line.split_whitespace().next().unwrap_or("");
            if token.starts_with('/') || looks_like_path(token) {
                continue;
            }
            token
        };
        let candidate = candidate
            .rsplit('/')
            .next()
            .unwrap_or(candidate)
            .trim()
            .to_ascii_lowercase();
        if valid_package_name(&candidate) && !packages.contains(&candidate) {
            packages.push(candidate);
        }
    }
    packages
}

/// Dateipfade aus Paketmanager-Ausgaben erkennen (z. B. die Pfadzeile unter
/// einem `pacman -Fx`-Treffer).
fn looks_like_path(token: &str) -> bool {
    let first = token.split('/').next().unwrap_or(token);
    matches!(
        first,
        "usr"
            | "etc"
            | "opt"
            | "var"
            | "lib"
            | "lib64"
            | "bin"
            | "sbin"
            | "boot"
            | "dev"
            | "proc"
            | "sys"
            | "run"
            | "home"
            | "root"
            | "tmp"
    )
}

pub fn probe_commands(package_manager: &str, dep: &MissingDep) -> Vec<String> {
    let mut files: Vec<(String, bool)> = Vec::new();
    match dep {
        MissingDep::CMakeModule(module) => {
            files.push((format!("{}Config.cmake", module), false));
            files.push((format!("{}-config.cmake", module.to_lowercase()), false));
        }
        MissingDep::Header(header) => {
            files.push((
                header.rsplit('/').next().unwrap_or(header).to_string(),
                true,
            ));
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

/// Erkennt transiente Netzwerkfehler (z. B. HTTP 5xx beim Dependency-Download).
/// Solche Fehler rechtfertigen einen erneuten Versuch derselben Strategie.
pub fn is_transient_network_error(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    let patterns = [
        "http/2 500",
        "http/2 502",
        "http/2 503",
        "http/2 504",
        "http/1.1 500",
        "http/1.1 502",
        "http/1.1 503",
        "http/1.1 504",
        "could not resolve host",
        "temporary failure in name resolution",
        "connection timed out",
        "operation timed out",
        "connection reset",
        "could not connect to server",
        "failed to connect to",
        "ssl connect error",
    ];
    patterns.iter().any(|pattern| lower.contains(pattern))
}

/// Erkennt, dass ein Repository-Buildskript die Linux-Distribution ablehnt.
/// Der Runner soll dann auf das generische Build-System zurueckfallen.
pub fn detect_repo_script_distro_rejection(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    [
        "distribution does not appear to be currently supported",
        "unsupported distribution",
        "unsupported distro",
        "not supported on this os",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern))
}

/// Erkennt Fehler, bei denen die System-`rustc` zu alt für Crate-Anforderungen ist.
///
/// Deckt beide gaengigen Formulierungen ab:
/// `rustc 1.85.1 is not supported by the following packages:` und
/// `package ... cannot be built because it requires rustc 1.89.0`.
pub fn detect_outdated_rustc(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    if !lower.contains("rustc") {
        return false;
    }
    let unsupported = lower.contains("is not supported by the following packages")
        || lower.contains("is not supported by the package");
    let requires = lower.contains("requires rustc")
        || (lower.contains("cannot be built") && lower.contains("rustc"));
    unsupported || requires
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

    let missing_int_type = [
        "uint8_t", "uint16_t", "uint32_t", "int8_t", "int16_t", "int32_t",
    ]
    .iter()
    .any(|name| lower.contains(name));
    if missing_int_type
        && (lower.contains("does not name a type")
            || lower.contains("was not declared in this scope"))
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

    if detect_outdated_rustc(output) {
        fixes.push(BuildFix {
            key: "rustup_toolchain".to_string(),
            description: "Veraltete rustc-Toolchain der Distribution".to_string(),
            value: "install stable toolchain via rustup".to_string(),
        });
    }

    if lower.contains("no such file or directory")
        && ["third_party/", "extern/", "libs/"]
            .iter()
            .any(|dir| lower.contains(dir))
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
    let hint_re = Regex::new(r#"is not a prefix of file[^"]*"([^"]+)""#).ok();
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
        if !window.iter().any(|candidate| {
            candidate
                .to_ascii_lowercase()
                .contains("is not a prefix of file")
        }) {
            continue;
        }
        let window_text = window.join("\n");
        let hint = hint_re
            .as_ref()
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
        || (lower.contains("compatibility with cmake <")
            && (lower.contains("3.5") || lower.contains("3.10")))
}

/// Liest `option(<NAME> "<doc>" <DEFAULT>)` aus CMake-Dateien und liefert
/// die passenden `-D`-Flags fuer system-nahe bzw. geteilte Bibliotheken.
pub fn cmake_option_flags_from_text(
    text: &str,
    prefer_system: bool,
    prefer_shared: bool,
) -> Vec<String> {
    let Ok(regex) =
        Regex::new(r#"(?m)^\s*option\s*\(\s*([A-Za-z0-9_]+)\s+"[^"]*"\s+(ON|OFF)\s*\)"#)
    else {
        return Vec::new();
    };
    let mut flags: Vec<String> = Vec::new();
    for found in regex.captures_iter(text) {
        let name = &found[1];
        let upper = name.to_ascii_uppercase();
        let default_on = &found[2] == "ON";
        if prefer_system && upper.contains("SYSTEM") && !default_on {
            let flag = format!("-D{name}=ON");
            if !flags.contains(&flag) {
                flags.push(flag);
            }
        }
        if prefer_shared && upper.ends_with("_STATIC") && default_on {
            let flag = format!("-D{name}=OFF");
            if !flags.contains(&flag) {
                flags.push(flag);
            }
        }
    }
    flags
}

/// Parst generische CMake-Hinweise wie `or pass -DFOO_BAR=ON`.
pub fn cmake_alternative_flags(output: &str) -> Vec<String> {
    let Ok(regex) = Regex::new(r"(?:or\s+)?(?:pass|set)\s+(-D[A-Za-z0-9_]+=[A-Za-z0-9_./-]+)")
    else {
        return Vec::new();
    };
    let mut flags: Vec<String> = Vec::new();
    for found in regex.captures_iter(output) {
        // Satzzeichen aus Fließtext gehören nicht zum Flag-Wert.
        let flag = found[1].trim_end_matches(['.', ',', ';', ':']).to_string();
        if !flag.ends_with('=') && !flags.contains(&flag) {
            flags.push(flag);
        }
    }
    flags
}

/// Alter `FindEigen3.cmake` parst `Macros.h` statt `Version.h`. Ein Retry mit
/// expliziten Version-Flags laesst `find_package(Eigen3)` das Systempaket akzeptieren.
pub fn has_eigen_version_mismatch(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    lower.contains("eigen3 version")
        && lower.contains("but at least version")
        && lower.contains("is required")
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
            | "libraries"
            | "packages"
            | "files"
            | "modules"
            | "headers"
            | "components"
            | "missing"
            | "available"
            | "compiler"
            | "compilers"
            | "linker"
            | "target"
            // Häufige englische Füllwörter aus Statusmeldungen
            // (z. B. "libraries were not found").
            | "were"
            | "was"
            | "are"
            | "is"
            | "be"
            | "been"
            | "being"
            | "have"
            | "has"
            | "had"
            | "do"
            | "does"
            | "did"
            | "can"
            | "could"
            | "may"
            | "might"
            | "must"
            | "shall"
            | "should"
            | "will"
            | "would"
            | "some"
            | "all"
            | "any"
            | "no"
            | "this"
            | "that"
            | "these"
            | "those"
            | "they"
            | "them"
            | "their"
            | "there"
            | "which"
            | "who"
            | "what"
            | "when"
            | "where"
            | "and"
            | "but"
            | "if"
            | "then"
            | "so"
            | "such"
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
    let shell_cmd_re = Regex::new(r"(?i)([A-Za-z0-9_.\-]+):\s*command not found").unwrap();
    let tool_quoted_re =
        Regex::new(r#"(?i)(?:tool|executable|program)\s+['\"]([A-Za-z0-9_+.\-]+)['\"]"#).unwrap();
    // CMake-Module melden fehlende Datenpakete z. B. als
    // "Missing wayland-protocols pkgdatadir".
    let pkgdata_re = Regex::new(r"(?i)missing\s+([A-Za-z0-9_+.\-]+)\s+pkgdatadir").unwrap();

    let mut deps: Vec<MissingDep> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        // Erfolgsmeldungen wie `-- Found EXPAT: ... (found version "2.10.0")`
        // sind keine Fehler und dürfen nie in den Resolver gelangen.
        if trimmed.to_ascii_lowercase().starts_with("-- found ") {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        // Reine Statusmeldungen wie `-- XYZ NOT found, using bundled version...`
        // sind keine fehlenden System-Dependencies.
        if lower.starts_with("-- ")
            && (lower.contains("using bundled")
                || lower.contains("using internal")
                || lower.contains("optional"))
        {
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
        for caps in tool_quoted_re.captures_iter(trimmed) {
            let name = &caps[1];
            if !is_generic_tool_word(name) {
                push_unique(&mut deps, MissingDep::Tool(name.to_string()));
            }
        }
        for caps in pkgdata_re.captures_iter(trimmed) {
            push_unique(&mut deps, MissingDep::PkgConfig(caps[1].to_string()));
        }
    }

    // Tool-Meldungen, die nur ein bereits erkanntes CMake-/pkg-config-Modul
    // wiederholen (`webkit` neben `WEBKIT`), sind redundant.
    let modules: Vec<String> = deps
        .iter()
        .filter_map(|dep| match dep {
            MissingDep::CMakeModule(name) | MissingDep::PkgConfig(name) => {
                Some(name.to_ascii_lowercase())
            }
            _ => None,
        })
        .collect();
    deps.retain(|dep| match dep {
        MissingDep::Tool(name) => !modules.contains(&name.to_ascii_lowercase()),
        _ => true,
    });

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
        assert_eq!(
            analyze_output(out),
            vec![MissingDep::Header("openssl/sha.h".into())]
        );
    }

    #[test]
    fn statusmeldungen_erzeugen_keine_falschen_tools() {
        let out =
            "-- Some or all of the gtk libraries were not found. (missing: GTK2_GTK_LIBRARY)\n";
        assert!(!analyze_output(out)
            .iter()
            .any(|dep| matches!(dep, MissingDep::Tool(name) if name == "were")));
    }

    #[test]
    fn erkennt_fehlendes_pkgdatadir() {
        let out = "CMake Error at CMake/modules/FindWaylandProtocols.cmake:9 (message):\n  Missing wayland-protocols pkgdatadir\n";
        assert!(analyze_output(out)
            .iter()
            .any(|dep| matches!(dep, MissingDep::PkgConfig(name) if name == "wayland-protocols")));
    }

    #[test]
    fn erkennt_pkgconfig_fehler() {
        let out = "Package 'libcurl', required by 'virtual:world', not found\n";
        assert_eq!(
            analyze_output(out),
            vec![MissingDep::PkgConfig("libcurl".into())]
        );
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
        assert_eq!(
            MissingDep::CMakeModule("Boost".into()).search_term(),
            "boost"
        );
        assert_eq!(
            MissingDep::CMakeModule("OpenSSL".into()).search_term(),
            "openssl"
        );
        assert_eq!(MissingDep::PkgConfig("CURL".into()).search_term(), "curl");
        assert_eq!(
            MissingDep::CMakeModule("Eigen3Config".into()).search_term(),
            "eigen3"
        );
    }

    #[test]
    fn cmake_module_werden_auf_distro_pakte_abgebildet() {
        assert_eq!(distro_package_name("boost", "pacman"), Some("boost"));
        assert_eq!(
            distro_package_name("boost", "apt"),
            Some("libboost-all-dev")
        );
        assert_eq!(distro_package_name("boost", "dnf"), Some("boost-devel"));
        assert_eq!(distro_package_name("eigen3", "pacman"), Some("eigen"));
        assert_eq!(distro_package_name("eigen3", "apt"), Some("libeigen3-dev"));
        assert_eq!(distro_package_name("eigen3", "dnf"), Some("eigen3-devel"));
        assert_eq!(
            distro_package_name("wxwidgets", "pacman"),
            Some("wxwidgets-gtk3")
        );
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
    fn unscharfe_teilstring_treffer_werden_abgelehnt() {
        // "ecm" steckt zufaellig in "doublecmd".
        assert_eq!(
            package_from_search("doublecmd-qt5\n", "ecm", "pacman"),
            None
        );
        assert_eq!(
            package_from_search("extra-cmake-modules\n", "ecm", "pacman"),
            None
        );
        assert_eq!(
            package_from_search("gmp-ecm\n", "ecm", "pacman"),
            Some("gmp-ecm".to_string())
        );
        assert_eq!(
            package_from_search("ecm-tools\n", "ecm", "pacman"),
            Some("ecm-tools".to_string())
        );
        assert_eq!(
            package_from_search("libsoup3\nlibsoup3-docs\n", "libsoup", "pacman"),
            Some("libsoup3".to_string())
        );
        assert_eq!(
            package_from_search(
                "lightdm-webkit-theme-litarvan\nwebkit2gtk-4.1\n",
                "webkit",
                "pacman"
            ),
            Some("webkit2gtk-4.1".to_string())
        );
        assert_eq!(
            package_from_search("freeglut\n", "glut", "pacman"),
            Some("freeglut".to_string())
        );
    }

    #[test]
    fn transiente_netzwerkfehler_werden_erkannt() {
        assert!(is_transient_network_error(
            "curl: (22) The requested URL returned error: 502\n< HTTP/2 502"
        ));
        assert!(is_transient_network_error(
            "Could not resolve host: example.invalid"
        ));
        assert!(is_transient_network_error(
            "Connection timed out after 30000 ms"
        ));
        assert!(!is_transient_network_error(
            "CMake Error at CMakeLists.txt:10 (message): foo"
        ));
    }

    #[test]
    fn tools_wiederholen_keine_erkannten_module() {
        let out = "Could NOT find WEBKIT (missing: WEBKIT_LIBRARIES)\nwebkit not found\n";
        let deps = analyze_output(out);
        assert!(deps
            .iter()
            .any(|dep| matches!(dep, MissingDep::CMakeModule(name) if name == "WEBKIT")));
        assert!(!deps.iter().any(
            |dep| matches!(dep, MissingDep::Tool(name) if name.eq_ignore_ascii_case("webkit"))
        ));
    }

    #[test]
    fn dateidatenbank_ignoriert_pfadzeilen_und_erkennt_repo_pakete() {
        let output =
            "extra/extra-cmake-modules 6.32.0-1 (kf6)\n    usr/share/ECM/cmake/ECMConfig.cmake\n";
        assert_eq!(
            packages_from_file_database(output, "ECM"),
            vec!["extra-cmake-modules".to_string()]
        );
        let owned = "usr/bin/vdb_print is owned by extra/openvdb 13.0.0-3\n";
        assert_eq!(
            packages_from_file_database(owned, "vdb_print"),
            vec!["openvdb".to_string()]
        );
    }

    #[test]
    fn standard_wissensbasis_enthlt_klassiker() {
        let defaults: HashMap<String, String> = serde_json::from_str(DEFAULT_MAPPINGS).unwrap();
        assert_eq!(
            defaults.get("arch:Boost").map(String::as_str),
            Some("boost")
        );
        assert_eq!(
            defaults.get("debian:Eigen3").map(String::as_str),
            Some("libeigen3-dev")
        );
        assert_eq!(
            defaults.get("fedora:wxWidgets").map(String::as_str),
            Some("wxGTK-devel")
        );
        assert_eq!(
            defaults.get("arch:OpenGL").map(String::as_str),
            Some("libglvnd")
        );
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
        assert!(
            detect_build_fixes("source_group \"x\" is not a prefix of file \"y\"")
                .iter()
                .any(|fix| fix.key == "cmake_policy")
        );
        assert!(
            detect_build_fixes("third_party/foo.h: No such file or directory")
                .iter()
                .any(|fix| fix.key == "submodules")
        );
        assert!(detect_build_fixes("all good").is_empty());
    }

    #[test]
    fn erkennt_veraltete_rustc_toolchain() {
        let unsupported = concat!(
            "error: rustc 1.85.1 is not supported by the following packages:\n",
            "  some_crate@1.2.3 requires rustc 1.89.0\n"
        );
        assert!(detect_outdated_rustc(unsupported));
        assert!(detect_build_fixes(unsupported)
            .iter()
            .any(|fix| fix.key == "rustup_toolchain"));

        let cannot_build = "error: some_crate cannot be built because it requires rustc 1.89.0";
        assert!(detect_outdated_rustc(cannot_build));

        assert!(!detect_outdated_rustc(
            "error: could not compile some_crate"
        ));
        assert!(!detect_outdated_rustc("warning: rustc version is fine"));
    }

    #[test]
    fn mapping_store_lernt_und_speichert() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "git2pkg-ai-map-{}-{}.json",
            std::process::id(),
            nanos
        ));
        let mut store = MappingStore::from_path(path.clone());
        assert!(path.exists(), "Datei muss automatisch angelegt werden");
        assert_eq!(store.get("arch", "Boost").as_deref(), Some("boost"));
        store.learn("arch", "MyLib", "mylib-dev");
        assert_eq!(store.get("arch", "MyLib").as_deref(), Some("mylib-dev"));
        store.learn_negative("bogus-pkg");
        assert!(store.is_negative("bogus-pkg"));
        assert!(store.learn_build_fix("cstdint", "-include cstdint"));
        assert_eq!(
            store.build_fix("cstdint").as_deref(),
            Some("-include cstdint")
        );
        assert!(store.learned_rule_count() >= 3);
        let json = fs::read_to_string(&path).unwrap();
        assert!(json.contains("packages"));
        assert!(json.contains("negative_cache"));
        assert!(json.contains("build_fixes"));
        let mut reloaded = MappingStore::from_path(path.clone());
        assert_eq!(reloaded.get("arch", "MyLib").as_deref(), Some("mylib-dev"));
        assert!(reloaded.is_negative("bogus-pkg"));
        assert_eq!(
            reloaded.build_fix("cstdint").as_deref(),
            Some("-include cstdint")
        );
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
        assert!(!deps
            .iter()
            .any(|dep| matches!(dep, MissingDep::Tool(t) if is_generic_tool_word(t))));

        let shell = analyze_output("/work/build.sh: line 11: cargo: command not found");
        assert!(shell.contains(&MissingDep::Tool("cargo".to_string())));
        let tool = MissingDep::Tool("nasm".to_string());
        assert_eq!(tool.search_term(), "nasm");
        assert!(probe_commands("pacman", &tool).contains(&"pacman -Fx '/bin/nasm'".to_string()));
        assert!(probe_commands("apt", &tool).contains(&"apt-file search '/bin/nasm'".to_string()));
        assert!(probe_commands("dnf", &tool).contains(&"dnf provides '*/bin/nasm'".to_string()));
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
    #[test]
    fn sha256_ist_korrekt() {
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn build_recipes_werden_gelernt_und_wiedergefunden() {
        let dir = std::env::temp_dir().join(format!(
            "git2pkg-ai-recipes-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let path = dir.join("learned_mappings.json");
        let mut store = MappingStore::from_path(path.clone());
        let recipe = BuildRecipe {
            signature: "cmake+deps_dir+repo_script".to_string(),
            strategy: "SystemLibsOnly".to_string(),
            skip_deps: true,
            packages: vec!["pkgconf".to_string()],
            ..Default::default()
        };
        store.record_build_recipe("example-app", recipe);

        let reloaded = MappingStore::from_path(path);
        let (exact, is_exact) = reloaded
            .apply_learned_recipe("example-app", "cmake+deps_dir+repo_script")
            .unwrap();
        assert!(is_exact);
        assert!(exact.skip_deps);
        let (similar, by_signature) = reloaded
            .apply_learned_recipe("sample-tool", "cmake+deps_dir+repo_script")
            .unwrap();
        assert!(!by_signature);
        assert_eq!(similar.strategy, "SystemLibsOnly");
        assert_eq!(reloaded.build_recipe_count(), 1);
        assert!(reloaded.learned_rule_count() >= 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn repo_profiles_werden_gelernt_und_uebersprungen() {
        let dir = std::env::temp_dir().join(format!(
            "git2pkg-ai-profiles-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let path = dir.join("learned_mappings.json");
        let url = "example-app";

        let mut store = MappingStore::from_path(path.clone());
        assert!(!store.is_incomplete_strategy(url, "Default"));
        store.mark_incomplete_strategy(url, "Default");
        assert!(store.is_incomplete_strategy(url, "Default"));
        assert!(store.has_incomplete_default(url));
        assert_eq!(
            store.incomplete_strategies(url),
            vec!["Default".to_string()]
        );

        let reloaded = MappingStore::from_path(path);
        assert!(reloaded.is_incomplete_strategy(url, "Default"));
        assert!(reloaded.has_incomplete_default(url));
        assert_eq!(reloaded.learned_rule_count(), 1);
        assert_eq!(repo_profile_key(url), sha256_hex(&normalize_repo_url(url)));

        assert!(is_compiled_build_system("Cargo (Rust)"));
        assert!(!is_compiled_build_system("Unknown"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn distro_ablehnung_von_repo_skripten_wird_erkannt() {
        assert!(detect_repo_script_distro_rejection(
            "Your distribution does not appear to be currently supported by these build scripts"
        ));
        assert!(detect_repo_script_distro_rejection(
            "unsupported distribution"
        ));
        assert!(detect_repo_script_distro_rejection(
            "Unsupported distro: example"
        ));
        assert!(detect_repo_script_distro_rejection(
            "not supported on this os"
        ));
        assert!(!detect_repo_script_distro_rejection(
            "error: could not compile"
        ));
    }

    #[test]
    fn gelernte_repo_pakete_werden_sofort_wiedergefunden() {
        let dir = std::env::temp_dir().join(format!(
            "git2pkg-ai-repo-packages-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let path = dir.join("learned_mappings.json");
        let mut store = MappingStore::from_path(path.clone());
        let packages = vec!["libpng".to_string(), "pkgconf".to_string()];
        store.record_build_recipe(
            "example-app",
            BuildRecipe {
                signature: "cmake".to_string(),
                strategy: "Default".to_string(),
                ..BuildRecipe::default()
            },
        );
        store.record_repo_packages("example-app", "cmake", &packages);

        let reloaded = MappingStore::from_path(path);
        let (recipe, exact) = reloaded
            .apply_learned_recipe("example-app", "cmake")
            .unwrap();
        assert!(exact);
        assert_eq!(recipe.signature, "cmake");
        assert_eq!(recipe.packages, packages);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn cmake_findmodule_mappings_sind_vollstaendig() {
        for module in [
            "Boost",
            "GLEW",
            "GLFW",
            "TBB",
            "OpenCV",
            "Eigen3",
            "CURL",
            "OpenSSL",
            "Freetype",
            "EXPAT",
            "ZLIB",
            "BZip2",
            "LibLZMA",
            "GTK2",
            "GTK3",
            "wxWidgets",
            "WebKit2GTK",
            "Cereal",
            "Qhull",
            "NLopt",
            "Assimp",
            "FFmpeg",
        ] {
            for manager in ["pacman", "apt", "dnf", "zypper"] {
                assert!(
                    distro_package_name(module, manager).is_some(),
                    "{module} fehlt für {manager}"
                );
            }
        }
        assert_eq!(
            distro_package_name("Boost", "apt"),
            Some("libboost-all-dev")
        );
    }

    #[test]
    fn veraltete_unvollstaendige_profile_werden_verworfen() {
        let dir = std::env::temp_dir().join(format!(
            "g2p-obsolete-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("mappings.json");
        let json = r#"{"repo_profiles":{"repo:abc":["Standard","System-Bibliotheken ohne deps/-Vorbau","Direkter Build-System-Fallback"]}}"#;
        fs::write(&path, json).unwrap();
        let store = MappingStore::from_path(path.clone());
        let list = store.repo_profiles.get("repo:abc").unwrap();
        assert!(list.is_empty(), "veraltete Labels muessen verworfen werden");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn cmake_alternativ_flags_werden_aus_fehlermeldung_extrahiert() {
        let output = "CMake Error at src/CMakeLists.txt:10 (find_package):\n  Some backend missing. Alternatively pass -DFOO_BAR=ON or pass -DENABLE_XYZ=OFF\nmirrors: set -DUSE_SYSTEM_FOO=ON\n";
        let flags = cmake_alternative_flags(output);
        assert_eq!(
            flags,
            vec![
                "-DFOO_BAR=ON".to_string(),
                "-DENABLE_XYZ=OFF".to_string(),
                "-DUSE_SYSTEM_FOO=ON".to_string(),
            ]
        );
        // Satzpunkt am Satzende gehört nicht zum Flag.
        assert_eq!(
            cmake_alternative_flags("Alternatively pass -DFOO=ON."),
            vec!["-DFOO=ON".to_string()]
        );
        assert!(cmake_alternative_flags("no hint here").is_empty());
    }

    #[test]
    fn cmake_status_mit_using_bundled_ist_keine_fehlende_dependency() {
        let output = "-- IGL NOT found, using bundled version from third_party\n-- Optional plugin NOT found, using internal copy\n";
        assert!(analyze_output(output).is_empty());
        assert!(is_cmake_policy_error(
            "Compatibility with CMake < 3.5 has been removed."
        ));
        assert!(!is_cmake_policy_error("Could NOT find PNG"));
        assert!(has_eigen_version_mismatch(
            "Eigen3 version 5.0.0 found in /usr/include/eigen3, but at least version 3.3 is required"
        ));
    }

    #[test]
    fn bauweise_nur_nach_verifiziertem_erfolg_gespeichert() {
        let dir = std::env::temp_dir().join(format!(
            "g2p-verified-recipe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let path = dir.join("learned_mappings.json");
        let mut store = MappingStore::from_path(path.clone());
        let packages = vec!["boost".to_string()];
        store.record_repo_packages("example-app", "cmake-signature", &packages);

        // Ein Programmeintrag allein ist kein verifiziertes Rezept.
        let unverified = MappingStore::from_path(path.clone());
        assert!(unverified
            .apply_learned_recipe("example-app", "cmake-signature")
            .is_none());

        store.record_build_recipe(
            "example-app",
            BuildRecipe {
                signature: "cmake-signature".to_string(),
                strategy: "Default".to_string(),
                packages: packages.clone(),
                ..BuildRecipe::default()
            },
        );
        // Ein spaeterer Paket-Eintrag darf die Verifizierung nicht zuruecknehmen.
        store.record_repo_packages("example-app", "cmake-signature", &packages);
        let reloaded = MappingStore::from_path(path);
        let (recipe, exact) = reloaded
            .apply_learned_recipe("example-app", "cmake-signature")
            .unwrap();
        assert!(exact);
        assert!(recipe.verified);
        assert_eq!(recipe.packages, packages);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn cmake_option_flags_werden_generisch_erkannt() {
        let text = r#"
option(EXAMPLE_USE_SYSTEM_FOO "Use the system foo" OFF)
option(EXAMPLE_STATIC "Link foo statically" ON)
option(EXAMPLE_OTHER "Unrelated" OFF)
"#;
        let flags = cmake_option_flags_from_text(text, true, true);
        assert_eq!(
            flags,
            vec![
                "-DEXAMPLE_USE_SYSTEM_FOO=ON".to_string(),
                "-DEXAMPLE_STATIC=OFF".to_string(),
            ]
        );
        assert!(cmake_option_flags_from_text(text, false, false).is_empty());
    }

    #[test]
    fn pacman_dateidatenbank_paketname_wird_geparst() {
        let commands = tool_file_database_commands("pacman", "example-tool");
        assert_eq!(commands[0], "pacman -Fy --noconfirm");
        assert_eq!(commands[1], "pacman -Fq '/usr/bin/example-tool'");
        let output = "usr/bin/example-tool is owned by extra/example-package 1.2.3-1\n";
        assert_eq!(
            packages_from_file_database(output, "example-tool"),
            vec!["example-package".to_string()]
        );
        assert!(tool_file_database_commands("pacman", "../bad").is_empty());
    }

    #[test]
    fn reset_entfernt_lerndatei() {
        let dir = std::env::temp_dir().join(format!(
            "g2p-reset-learning-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let path = dir.join("learned_mappings.json");
        fs::create_dir_all(&dir).unwrap();
        fs::write(&path, "{}").unwrap();
        assert!(MappingStore::reset_at(&path));
        assert!(!path.exists());
        assert!(!MappingStore::reset_at(&path));
        let _ = fs::remove_dir_all(dir);
    }

    fn temp_mapping_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "g2p-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir.join("learned_mappings.json")
    }

    #[test]
    fn parallele_stores_verlieren_keine_learnings() {
        let path = temp_mapping_path("merge");
        let mut first = MappingStore::from_path(path.clone());
        let mut second = MappingStore::from_path(path.clone());

        first.learn("arch", "Alpha", "alpha-pkg");
        second.learn("arch", "Beta", "beta-pkg");

        let mut merged = MappingStore::from_path(path.clone());
        assert_eq!(merged.get("arch", "Alpha").as_deref(), Some("alpha-pkg"));
        assert_eq!(merged.get("arch", "Beta").as_deref(), Some("beta-pkg"));

        // Ein gelerntes Mapping hebt einen zwischenzeitlichen Negativ-Eintrag auf.
        first.learn_negative("gamma-pkg");
        second.learn("arch", "Gamma", "gamma-pkg");
        let mut merged = MappingStore::from_path(path.clone());
        assert_eq!(merged.get("arch", "Gamma").as_deref(), Some("gamma-pkg"));
        assert!(!merged.is_negative("gamma-pkg"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn geleertes_profil_wird_nicht_wiederhergestellt() {
        let path = temp_mapping_path("clear");
        let url = "https://example.invalid/example-app";
        let mut store = MappingStore::from_path(path.clone());
        store.mark_incomplete_strategy(url, "Default");
        assert!(store.is_incomplete_strategy(url, "Default"));
        store.clear_incomplete_strategy(url, "Default");
        assert!(!store.is_incomplete_strategy(url, "Default"));

        let reloaded = MappingStore::from_path(path.clone());
        assert!(!reloaded.is_incomplete_strategy(url, "Default"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn lernspeicher_wird_deterministisch_begrenzt() {
        let path = temp_mapping_path("prune");
        let mut store = MappingStore::from_path(path.clone());
        let total = MAX_NEGATIVE_ENTRIES + 10;
        for index in 0..total {
            let package = format!("pkg-{:04}", index);
            store.negative_cache.push(package.clone());
            store
                .last_used
                .insert(format!("negative:{}", package), index as u64);
        }
        store.save();

        assert_eq!(store.negative_cache.len(), MAX_NEGATIVE_ENTRIES);
        assert!(!store.is_negative("pkg-0000"));
        assert!(store.is_negative(&format!("pkg-{:04}", total - 1)));
        assert!(
            !lock_path_for(&path).exists(),
            "Lock-Datei muss entfernt sein"
        );
        assert!(
            !temp_path_for(&path).exists(),
            "Temp-Datei muss entfernt sein"
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn lernspeicher_bleibt_abwaertskompatibel() {
        let path = temp_mapping_path("legacy");
        fs::write(&path, "{\"arch:Legacy\":\"legacy-pkg\"}").unwrap();
        let mut store = MappingStore::from_path(path.clone());
        assert_eq!(store.get("arch", "Legacy").as_deref(), Some("legacy-pkg"));
        let json = fs::read_to_string(&path).unwrap();
        assert!(json.contains("\"packages\""));
        let _ = fs::remove_file(path);
    }
}
