//! Deterministische Erkennung des Build-Systems anhand von Marker-Dateien.

use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildSystem {
    Cargo,
    CMake,
    Meson,
    Qmake,
    Go,
    Python,
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
            BuildSystem::Make => "make",
            BuildSystem::Unknown => "true",
        }
    }
}

/// CMake-Projekte mit einem Unterverzeichnis `deps/` enthalten lokale Abhängigkeiten
/// in `deps/CMakeLists.txt`, die vor dem Hauptprojekt gebaut werden müssen.
pub fn has_local_deps(root: &Path) -> bool {
    root.join("deps/CMakeLists.txt").is_file()
        || root.join("third_party/CMakeLists.txt").is_file()
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
        let dir = std::env::temp_dir().join(format!("git2pkg-detect-{}-{}", name, std::process::id()));
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
    fn erkennt_cargo() {
        let root = temp_root("cargo", &["Cargo.toml", "Makefile"]);
        assert_eq!(detect_build_system(&root), BuildSystem::Cargo);
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
