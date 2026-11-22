//! Gemeinsame Inhaltsprüfung für `.pkg.tar.zst`, `.deb` und `.rpm`.

use crate::core::container::{BuildConfig, BuildContainer};
use crate::core::resolver::MappingStore;
use std::sync::mpsc::Sender;

pub fn archive_listing_command(kind: &str, path: &str) -> String {
    match kind {
        "pkg" => format!("tar -tvf '{0}' 2>/dev/null || tar --zstd -tvf '{0}'", path),
        "deb" => format!("dpkg-deb -c '{}'", path),
        "rpm" => format!("rpm -qlp '{}'", path),
        _ => String::new(),
    }
}

/// `rpm -qlp` liefert keine Größen - die kommen aus der Paketabfrage.
pub fn rpm_size_command(path: &str) -> String {
    format!("rpm -qp --qf '%{{SIZE}}\\n' '{}'", path)
}

/// Zählt echte Nutzdateien im Paket und ignoriert reine Metadaten.
#[allow(dead_code)]
pub fn listing_payload(text: &str, kind: &str) -> (usize, u64, bool) {
    listing_payload_with_size(text, kind, None)
}

pub fn listing_payload_with_size(
    text: &str,
    kind: &str,
    forced_bytes: Option<u64>,
) -> (usize, u64, bool) {
    let prefixes = [
        "usr/bin/",
        "usr/sbin/",
        "usr/lib/",
        "usr/lib64/",
        "usr/share/",
        "opt/",
        "bin/",
        "lib/",
    ];
    let metadata = [
        ".PKGINFO",
        ".BUILDINFO",
        ".MTREE",
        ".INSTALL",
        ".Changelog",
        ".FILELIST",
        "DEBIAN",
    ];
    let mut files = 0usize;
    let mut bytes = 0u64;
    let mut has_size = false;

    for line in text.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.is_empty() {
            continue;
        }
        if kind != "rpm" {
            let flags = tokens[0];
            if !(flags.starts_with('-') || flags.starts_with('l')) {
                continue;
            }
        }
        let raw_path = if kind == "rpm" {
            tokens[0]
        } else {
            *tokens.last().unwrap()
        };
        let path = raw_path.trim_start_matches("./").trim_start_matches('/');
        if path.ends_with('/')
            || metadata
                .iter()
                .any(|m| path.split('/').any(|seg| seg == *m))
        {
            continue;
        }
        if !prefixes.iter().any(|prefix| path.starts_with(prefix)) {
            continue;
        }
        let size = match kind {
            "deb" => tokens.get(2).and_then(|value| value.parse::<u64>().ok()),
            "pkg" => tokens.get(3).and_then(|value| value.parse::<u64>().ok()),
            _ => None,
        };
        if size.is_some() {
            has_size = true;
        }
        bytes += size.unwrap_or(0);
        files += 1;
    }

    if let Some(total) = forced_bytes {
        bytes = total;
        has_size = true;
    }
    (files, bytes, has_size)
}

/// Zählt echte Programmdateien in den ausführbaren Standardpfaden.
pub fn listing_program_files(text: &str, kind: &str) -> usize {
    let prefixes = ["usr/bin/", "usr/local/bin/", "bin/", "usr/sbin/", "opt/"];
    let mut count = 0usize;
    for line in text.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.is_empty() {
            continue;
        }
        if kind != "rpm" {
            let flags = tokens[0];
            if !(flags.starts_with('-') || flags.starts_with('l')) {
                continue;
            }
        }
        let raw = if kind == "rpm" {
            tokens[0]
        } else {
            *tokens.last().unwrap()
        };
        let path = raw.trim_start_matches("./").trim_start_matches('/');
        if prefixes.iter().any(|prefix| path.starts_with(prefix)) && !path.ends_with('/') {
            count += 1;
        }
    }
    count
}

/// Ergebnis der Paketprüfung. Der Aufrufer entscheidet anhand dieses
/// Ergebnisses, ob die nächste Strategiestufe versucht wird.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyOutcome {
    Ok,
    /// Kompiliertes Projekt, aber das Paket enthält keine Programmdateien.
    NoProgramFiles,
    /// Paket ist leer oder besteht nur aus Metadaten.
    Invalid,
}

/// Prüft alle gebauten Pakete im Container. Ein Paket ohne Nutzdateien wird
/// entfernt und führt zum automatischen Wechsel auf die nächste Strategie.
pub fn verify_built_packages(
    container: &BuildContainer,
    config: &BuildConfig,
    tx: &Sender<crate::core::container::BuildEvent>,
    compiled: bool,
    strategy_label: &str,
    mappings: &mut MappingStore,
) -> VerifyOutcome {
    let Ok(listing) = container.exec("find /work/dist -maxdepth 1 -type f", tx) else {
        return VerifyOutcome::Ok;
    };
    let mut checked = 0usize;
    let mut invalid: Vec<String> = Vec::new();
    let mut compiled_without_program = false;

    for raw in listing.text.lines() {
        let path = raw.trim();
        if !path.starts_with("/work/dist/")
            || !path
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c))
        {
            continue;
        }
        let kind = if path.ends_with(".pkg.tar.zst") {
            "pkg"
        } else if path.ends_with(".deb") {
            "deb"
        } else if path.ends_with(".rpm") {
            "rpm"
        } else {
            continue;
        };
        checked += 1;
        let Ok(output) = container.exec(&archive_listing_command(kind, path), tx) else {
            invalid.push(path.to_string());
            continue;
        };
        let forced_bytes = if kind == "rpm" {
            container
                .exec(&rpm_size_command(path), tx)
                .ok()
                .and_then(|query| {
                    let total: u64 = query
                        .text
                        .lines()
                        .filter_map(|line| line.trim().parse::<u64>().ok())
                        .sum();
                    if total > 0 {
                        Some(total)
                    } else {
                        None
                    }
                })
        } else {
            None
        };
        let (files, bytes, has_size) = listing_payload_with_size(&output.text, kind, forced_bytes);
        let program_files = listing_program_files(&output.text, kind);
        let plausible = if compiled {
            program_files > 0
        } else {
            files > 0 && (!has_size || bytes >= 256 || files >= 2)
        };
        if !plausible {
            if compiled && program_files == 0 {
                compiled_without_program = true;
            }
            invalid.push(path.to_string());
        }
    }

    if checked == 0 || invalid.is_empty() {
        return VerifyOutcome::Ok;
    }

    let outcome = if compiled_without_program {
        mappings.mark_incomplete_strategy(&config.url, strategy_label);
        let _ = tx.send(crate::core::container::BuildEvent::Log(
            "[warn] produced package contains no program files - marking strategy as incomplete"
                .to_string(),
        ));
        VerifyOutcome::NoProgramFiles
    } else {
        let _ = tx.send(crate::core::container::BuildEvent::Log(
            "[warn] produced package is empty or invalid - switching to the next strategy"
                .to_string(),
        ));
        VerifyOutcome::Invalid
    };
    for path in invalid {
        let _ = container.exec(&format!("rm -f '{}'", path), tx);
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpm_size_abfrage_ist_enthalten() {
        assert!(rpm_size_command("/work/dist/app.rpm").contains("%{SIZE}"));
    }

    #[test]
    fn metadaten_und_verzeichnisse_zaehlen_nicht_als_inhalt() {
        let listing = "drwxr-xr-x 0 0 0 ./\n\
-rw-r--r-- 0 0 120 .PKGINFO\n\
-rw-r--r-- 0 0 40 .BUILDINFO\n\
-rw-r--r-- 0 0 60 .MTREE\n\
-rw-r--r-- 0 0 20 DEBIAN/control\n";
        let (files, _, _) = listing_payload_with_size(listing, "pkg", Some(6075));
        assert_eq!(files, 0);
        assert_eq!(listing_program_files(listing, "pkg"), 0);
    }

    #[test]
    fn echte_programmdatei_wird_als_solche_erkannt() {
        let listing = "drwxr-xr-x 0 0 0 usr/bin/\n\
-rwxr-xr-x 0 0 4096 usr/bin/app\n\
-rw-r--r-- 0 0 12 usr/share/app/data.txt\n";
        assert_eq!(listing_program_files(listing, "pkg"), 1);
        let rpm_listing = "/usr/bin/app\n/usr/share/app/data.txt\n";
        assert_eq!(listing_program_files(rpm_listing, "rpm"), 1);
    }

    #[test]
    fn deb_metadaten_alleine_sind_unvollstaendig() {
        let meta = "-rw-r--r-- root/root 100 2026-01-01 00:00 DEBIAN/control\n\
-rw-r--r-- root/root 40 2026-01-01 00:00 usr/share/doc/repo/copyright\n";
        assert_eq!(listing_program_files(meta, "deb"), 0);
        let with_binary = "-rwxr-xr-x root/root 4096 2026-01-01 00:00 usr/bin/repo\n";
        assert_eq!(listing_program_files(with_binary, "deb"), 1);
    }

    #[test]
    fn rpm_programmdatei_wird_erkannt() {
        let listing = "/usr/bin/repo\n/usr/share/repo/data.txt\n";
        assert_eq!(listing_program_files(listing, "rpm"), 1);
        assert_eq!(
            listing_program_files("/usr/share/repo/data.txt\n", "rpm"),
            0
        );
    }

    #[test]
    fn rpm_groesse_wird_uebernommen() {
        let listing = "/usr/bin/app\n/usr/share/app/data.txt\n";
        let (files, bytes, has_size) = listing_payload_with_size(listing, "rpm", Some(4096));
        assert_eq!(files, 2);
        assert_eq!(bytes, 4096);
        assert!(has_size);
    }
}
