//! Deterministische Erkennung des Build-Systems anhand von Marker-Dateien.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildSystem {
    Cargo,
    CMake,
    Meson,
    Qmake,
    Go,
    Python,
    Autotools,
    Make,
    Unknown,
}

impl BuildSystem {
    pub fn label(&self) -> &'static str {
        match self {
            BuildSystem::Cargo => "Cargo (Rust)",
            BuildSystem::CMake => "CMake",
            BuildSystem::Meson => "Meson",
            BuildSystem::Qmake => "qmake (.pro)",
            BuildSystem::Go => "Go Modules",
            BuildSystem::Python => "Python (pyproject.toml)",
            BuildSystem::Autotools => "Autotools (configure)",
            BuildSystem::Make => "Makefile",
            BuildSystem::Unknown => "Unknown",
        }
    }

    #[allow(dead_code)]
    pub fn build_command(&self) -> &'static str {
        match self {
            BuildSystem::Cargo => "cargo build --release",
            BuildSystem::CMake => "cmake -S . -B build -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr && cmake --build build",
            BuildSystem::Meson => "meson setup build --prefix=/usr && ninja -C build",
            BuildSystem::Qmake => "qmake6 . 2>/dev/null || qmake .; make",
            BuildSystem::Go => "go build -o app .",
            BuildSystem::Python => "python3 -m pip wheel --no-deps -w dist .",
            BuildSystem::Autotools => "./configure --prefix=/usr && make",
            BuildSystem::Make => "make",
            BuildSystem::Unknown => "true",
        }
    }
}

/// Allgemeine, reproduzierbare Struktur-Signatur eines Repositories.
/// Enthält niemals den Projekt- oder Repository-Namen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProjectFingerprint {
    pub signature: String,
    pub build_system: String,
    pub features: Vec<String>,
}

impl ProjectFingerprint {
    pub fn from_root(root: &Path) -> Self {
        let build_system = fingerprint_build_system(root).to_string();
        let mut features = Vec::new();
        if root.join("deps/CMakeLists.txt").is_file() {
            features.push("deps_dir".to_string());
        }
        if root.join("third_party/CMakeLists.txt").is_file() {
            features.push("third_party".to_string());
        }
        if has_repo_script(root) {
            features.push("repo_script".to_string());
        }
        if root.join("PKGBUILD").is_file() {
            features.push("pkgbuild".to_string());
        }
        if is_workspace(root) {
            features.push("workspace".to_string());
        }
        if root.join(".gitmodules").is_file() {
            features.push("submodules".to_string());
        }
        if has_desktop_assets(root) {
            features.push("desktop_assets".to_string());
        }
        let text = fingerprint_text(root);
        if uses_toolkit(&text, &["qt5", "qt6", "find_package(qt", "qmake"]) {
            features.push("qt".to_string());
        }
        if uses_toolkit(&text, &["gtk", "gtkmm", "gtk4"]) {
            features.push("gtk".to_string());
        }
        let signature = if features.is_empty() {
            build_system.clone()
        } else {
            format!("{}+{}", build_system, features.join("+"))
        };
        Self {
            signature,
            build_system,
            features,
        }
    }
}

fn fingerprint_build_system(root: &Path) -> &'static str {
    if root.join("configure.ac").is_file() || root.join("configure").is_file() {
        return "autotools";
    }
    match detect_build_system(root) {
        BuildSystem::Cargo => "cargo",
        BuildSystem::CMake => "cmake",
        BuildSystem::Meson => "meson",
        BuildSystem::Qmake => "qmake",
        BuildSystem::Go => "go",
        BuildSystem::Python => "python",
        BuildSystem::Autotools => "autotools",
        BuildSystem::Make => "make",
        BuildSystem::Unknown => "unknown",
    }
}

fn has_repo_script(root: &Path) -> bool {
    [
        "BuildLinux.sh",
        "build.sh",
        "build_linux.sh",
        "build-linux.sh",
        "compile.sh",
    ]
    .iter()
    .any(|name| root.join(name).is_file())
}

fn is_workspace(root: &Path) -> bool {
    if let Ok(text) = std::fs::read_to_string(root.join("Cargo.toml")) {
        if text.lines().any(|line| line.trim() == "[workspace]") {
            return true;
        }
    }
    root.join("crates").is_dir()
}

fn has_desktop_assets(root: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(root) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry
            .path()
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("desktop"))
            .unwrap_or(false)
    })
}

fn fingerprint_text(root: &Path) -> String {
    let mut text = String::new();
    for name in ["CMakeLists.txt", "meson.build"] {
        if let Ok(content) = std::fs::read_to_string(root.join(name)) {
            text.push_str(&content);
            text.push('\n');
        }
    }
    text.to_ascii_lowercase()
}

fn uses_toolkit(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

/// CMake-Projekte mit einem Unterverzeichnis `deps/` enthalten lokale Abhängigkeiten
/// in `deps/CMakeLists.txt`, die vor dem Hauptprojekt gebaut werden müssen.
pub fn has_local_deps(root: &Path) -> bool {
    root.join("deps/CMakeLists.txt").is_file() || root.join("third_party/CMakeLists.txt").is_file()
}

/// Generische Erkennung von qmake-Projekten über `.pro`-Dateien.
pub fn has_qmake_project(root: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(root) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry.path().is_file()
            && entry
                .path()
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("pro"))
                .unwrap_or(false)
    })
}

pub fn detect_build_system(root: &Path) -> BuildSystem {
    if root.join("Cargo.toml").is_file() {
        return BuildSystem::Cargo;
    }
    if root.join("CMakeLists.txt").is_file() {
        return BuildSystem::CMake;
    }
    if root.join("meson.build").is_file() {
        return BuildSystem::Meson;
    }
    if has_qmake_project(root) {
        return BuildSystem::Qmake;
    }
    if root.join("configure.ac").is_file() || root.join("configure").is_file() {
        return BuildSystem::Autotools;
    }

    let markers: [(&str, BuildSystem); 5] = [
        ("go.mod", BuildSystem::Go),
        ("pyproject.toml", BuildSystem::Python),
        ("Makefile", BuildSystem::Make),
        ("makefile", BuildSystem::Make),
        ("GNUmakefile", BuildSystem::Make),
    ];

    for (file, system) in markers {
        if root.join(file).is_file() {
            return system;
        }
    }
    BuildSystem::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root(name: &str, files: &[&str]) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("git2pkg-ai-detect-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for f in files {
            let path = dir.join(f);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, "").unwrap();
        }
        dir
    }

    #[test]
    fn fingerabdruck_ist_generisch_und_stabil() {
        let root = temp_root(
            "fingerprint",
            &[
                "CMakeLists.txt",
                "deps/CMakeLists.txt",
                "build.sh",
                ".gitmodules",
                "app.desktop",
            ],
        );
        fs::write(root.join("CMakeLists.txt"), "find_package(Qt6 REQUIRED)\n").unwrap();
        let first = ProjectFingerprint::from_root(&root);
        let second = ProjectFingerprint::from_root(&root);
        assert_eq!(first, second);
        assert!(first.signature.starts_with("cmake+"));
        assert!(first.features.contains(&"deps_dir".to_string()));
        assert!(first.features.contains(&"repo_script".to_string()));
        assert!(first.features.contains(&"qt".to_string()));
        assert!(!first.signature.contains("fingerprint"));
    }

    #[test]
    fn erkennt_cargo() {
        let root = temp_root("cargo", &["Cargo.toml", "Makefile"]);
        assert_eq!(detect_build_system(&root), BuildSystem::Cargo);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn erkennt_autotools() {
        let root = temp_root("autotools", &["configure.ac", "Makefile.am"]);
        assert_eq!(detect_build_system(&root), BuildSystem::Autotools);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn erkennt_cmake_und_meson() {
        let cmake = temp_root("cmake", &["CMakeLists.txt"]);
        assert_eq!(detect_build_system(&cmake), BuildSystem::CMake);
        fs::remove_dir_all(cmake).unwrap();

        let meson = temp_root("meson", &["meson.build"]);
        assert_eq!(detect_build_system(&meson), BuildSystem::Meson);
        fs::remove_dir_all(meson).unwrap();
    }

    #[test]
    fn erkennt_go_python_make() {
        let go = temp_root("go", &["go.mod"]);
        assert_eq!(detect_build_system(&go), BuildSystem::Go);
        fs::remove_dir_all(go).unwrap();

        let py = temp_root("py", &["pyproject.toml"]);
        assert_eq!(detect_build_system(&py), BuildSystem::Python);
        fs::remove_dir_all(py).unwrap();

        let mk = temp_root("mk", &["Makefile"]);
        assert_eq!(detect_build_system(&mk), BuildSystem::Make);
        fs::remove_dir_all(mk).unwrap();
    }

    #[test]
    fn erkennt_qmake_nur_ohne_cmake() {
        let pro = temp_root("qmake", &["src.pro"]);
        assert_eq!(detect_build_system(&pro), BuildSystem::Qmake);
        fs::remove_dir_all(pro).unwrap();

        let both = temp_root("qmake-cmake", &["src.pro", "CMakeLists.txt"]);
        assert_eq!(detect_build_system(&both), BuildSystem::CMake);
        fs::remove_dir_all(both).unwrap();
    }

    #[test]
    fn erkennt_deps_cmake() {
        let root = temp_root("deps", &["CMakeLists.txt", "deps/CMakeLists.txt"]);
        assert!(has_local_deps(&root));
        fs::remove_dir_all(root).unwrap();

        let third = temp_root("third", &["CMakeLists.txt", "third_party/CMakeLists.txt"]);
        assert!(has_local_deps(&third));
        fs::remove_dir_all(third).unwrap();
    }

    #[test]
    fn unbekannt_ohne_marker() {
        let root = temp_root("none", &["README.md"]);
        assert_eq!(detect_build_system(&root), BuildSystem::Unknown);
        fs::remove_dir_all(root).unwrap();
    }
}
