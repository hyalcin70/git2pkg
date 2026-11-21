//! Erkennung der Host-Distribution anhand von /etc/os-release.
//! Reine, deterministische Logik ohne Netzwerk und externe Prozesse.

use std::fs;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DistroFamily {
    Arch,
    Debian,
    Fedora,
    Suse,
    Unknown,
}

/// Vom Nutzer wählbares Paketziel. `Auto` bedeutet immer die Host-Distribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetDistro {
    Auto,
    Arch,
    Debian,
    Fedora,
    Suse,
}

impl TargetDistro {
    pub fn from_key(key: &str) -> Self {
        match key.to_ascii_lowercase().as_str() {
            "arch" | "archlinux" | "cachyos" => Self::Arch,
            "debian" | "ubuntu" | "mint" | "deb" => Self::Debian,
            "fedora" | "rhel" => Self::Fedora,
            "suse" | "opensuse" => Self::Suse,
            _ => Self::Auto,
        }
    }

    #[allow(dead_code)]
    pub fn key(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Arch => "arch",
            Self::Debian => "debian",
            Self::Fedora => "fedora",
            Self::Suse => "opensuse",
        }
    }

    pub fn family(&self, host: &DistroFamily) -> DistroFamily {
        match self {
            Self::Auto => host.clone(),
            Self::Arch => DistroFamily::Arch,
            Self::Debian => DistroFamily::Debian,
            Self::Fedora => DistroFamily::Fedora,
            Self::Suse => DistroFamily::Suse,
        }
    }

    /// Effektive Container-Distro inklusive Basis-Image und Paketmanager.
    pub fn info(&self, host: &DistroInfo) -> DistroInfo {
        match self {
            Self::Auto => host.clone(),
            other => DistroInfo::for_family(other.family(&host.family)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DistroInfo {
    pub name: String,
    pub id: String,
    pub family: DistroFamily,
    pub base_image: String,
    pub package_manager: String,
    pub search_command: String,
}

impl DistroInfo {
    pub fn detect() -> Self {
        let content = fs::read_to_string("/etc/os-release")
            .or_else(|_| fs::read_to_string("/usr/lib/os-release"))
            .unwrap_or_default();
        Self::from_os_release(&content)
    }

    /// Synthetische Distro-Info für ein explizites Ziel (Cross-Build).
    pub fn for_family(family: DistroFamily) -> Self {
        let os_release = match family {
            DistroFamily::Arch => "ID=arch\nNAME=\"Arch Linux\"\n",
            DistroFamily::Debian => "ID=debian\nNAME=\"Debian\"\nID_LIKE=debian\n",
            DistroFamily::Fedora => "ID=fedora\nNAME=\"Fedora\"\n",
            DistroFamily::Suse => "ID=opensuse\nNAME=\"openSUSE\"\n",
            DistroFamily::Unknown => "",
        };
        let mut info = Self::from_os_release(os_release);
        if info.family != family && family != DistroFamily::Unknown {
            info.family = family.clone();
        }
        info
    }

    pub fn from_os_release(content: &str) -> Self {
        let mut id = String::new();
        let mut name = String::new();
        let mut id_like: Vec<String> = Vec::new();

        for line in content.lines() {
            let line = line.trim();
            if let Some(v) = line.strip_prefix("ID=") {
                id = unquote(v);
            } else if let Some(v) = line.strip_prefix("NAME=") {
                name = unquote(v);
            } else if let Some(v) = line.strip_prefix("ID_LIKE=") {
                id_like = unquote(v).split_whitespace().map(str::to_string).collect();
            }
        }

        let family = classify(&id, &id_like);
        let (base_image, package_manager, search_command) = match family {
            DistroFamily::Arch => ("archlinux:latest", "pacman", "pacman -Fq"),
            DistroFamily::Debian => (
                if id == "debian" {
                    "debian:stable-slim"
                } else {
                    "ubuntu:latest"
                },
                "apt",
                "apt-file search",
            ),
            DistroFamily::Fedora => ("fedora:latest", "dnf", "dnf provides"),
            DistroFamily::Suse => ("opensuse/leap:latest", "zypper", "zypper se --provides"),
            DistroFamily::Unknown => ("archlinux:latest", "pacman", "pacman -Fq"),
        };

        Self {
            name: if name.is_empty() { id.clone() } else { name },
            id,
            family,
            base_image: base_image.to_string(),
            package_manager: package_manager.to_string(),
            search_command: search_command.to_string(),
        }
    }
}

/// Kurzform für `DistroInfo::detect`.
pub fn detect_distro() -> DistroInfo {
    DistroInfo::detect()
}

fn unquote(value: &str) -> String {
    value
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .to_string()
}

fn classify(id: &str, id_like: &[String]) -> DistroFamily {
    let id = id.to_lowercase();
    let likes: Vec<String> = id_like.iter().map(|s| s.to_lowercase()).collect();

    if id == "arch" || id == "cachyos" || id == "manjaro" || likes.iter().any(|x| x == "arch") {
        DistroFamily::Arch
    } else if id == "ubuntu"
        || id == "debian"
        || id == "linuxmint"
        || id == "pop"
        || likes.iter().any(|x| x == "debian" || x == "ubuntu")
    {
        DistroFamily::Debian
    } else if id == "opensuse"
        || id == "opensuse-leap"
        || id == "opensuse-tumbleweed"
        || id == "sles"
        || id == "sles_sap"
        || likes.iter().any(|x| x == "suse" || x == "opensuse")
    {
        DistroFamily::Suse
    } else if id == "fedora"
        || id == "rhel"
        || id == "centos"
        || id == "rocky"
        || id == "almalinux"
        || likes.iter().any(|x| x == "fedora" || x == "rhel")
    {
        DistroFamily::Fedora
    } else {
        DistroFamily::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arch_und_cachyos_werden_erkannt() {
        let info = DistroInfo::from_os_release("ID=cachyos\nID_LIKE=\"arch\"\nNAME=\"CachyOS\"\n");
        assert_eq!(info.family, DistroFamily::Arch);
        assert_eq!(info.base_image, "archlinux:latest");
        assert_eq!(info.search_command, "pacman -Fq");
    }

    #[test]
    fn debian_und_ubuntu_werden_erkannt() {
        let info = DistroInfo::from_os_release("ID=ubuntu\nID_LIKE=debian\nNAME=\"Ubuntu\"\n");
        assert_eq!(info.family, DistroFamily::Debian);
        assert_eq!(info.base_image, "ubuntu:latest");
        assert_eq!(info.search_command, "apt-file search");
    }

    #[test]
    fn target_keys_werden_geparst() {
        assert_eq!(TargetDistro::from_key("auto"), TargetDistro::Auto);
        assert_eq!(TargetDistro::from_key("arch"), TargetDistro::Arch);
        assert_eq!(TargetDistro::from_key("ubuntu"), TargetDistro::Debian);
        assert_eq!(TargetDistro::from_key("rhel"), TargetDistro::Fedora);
        assert_eq!(TargetDistro::from_key("opensuse"), TargetDistro::Suse);
    }

    #[test]
    fn ziel_distro_waehlt_passendes_image() {
        let host = DistroInfo::for_family(DistroFamily::Arch);
        assert_eq!(TargetDistro::Auto.info(&host).family, DistroFamily::Arch);
        assert_eq!(
            TargetDistro::Arch.info(&host).base_image,
            "archlinux:latest"
        );
        assert_eq!(
            TargetDistro::Debian.info(&host).base_image,
            "debian:stable-slim"
        );
        assert_eq!(TargetDistro::Fedora.info(&host).base_image, "fedora:latest");
        assert_eq!(
            TargetDistro::Suse.info(&host).base_image,
            "opensuse/leap:latest"
        );
    }

    #[test]
    fn debian_nutzt_debian_image() {
        let info = DistroInfo::from_os_release("ID=debian\nID_LIKE=debian\nNAME=\"Debian\"\n");
        assert_eq!(info.base_image, "debian:stable-slim");
    }

    #[test]
    fn ubuntu_nutzt_ubuntu_image() {
        let info = DistroInfo::from_os_release("ID=ubuntu\nID_LIKE=debian\nNAME=\"Ubuntu\"\n");
        assert_eq!(info.base_image, "ubuntu:latest");
    }

    #[test]
    fn fedora_wird_erkannt() {
        let info = DistroInfo::from_os_release("ID=fedora\nNAME=\"Fedora Linux\"\n");
        assert_eq!(info.family, DistroFamily::Fedora);
        assert_eq!(info.search_command, "dnf provides");
    }

    #[test]
    fn opensuse_wird_erkannt() {
        let info = DistroInfo::from_os_release(
            "ID=opensuse-leap\nID_LIKE=\"suse opensuse\"\nNAME=\"openSUSE Leap\"\n",
        );
        assert_eq!(info.family, DistroFamily::Suse);
        assert_eq!(info.base_image, "opensuse/leap:latest");
        assert_eq!(info.package_manager, "zypper");
    }

    #[test]
    fn unbekannte_distro_faellt_auf_arch_zurueck() {
        let info = DistroInfo::from_os_release("ID=gensomething\nNAME=\"Gen\"\n");
        assert_eq!(info.family, DistroFamily::Unknown);
        assert_eq!(info.base_image, "archlinux:latest");
    }
}
