//! Gemeinsame Inhaltsprüfung für `.pkg.tar.zst`, `.deb` und `.rpm`.

use crate::core::container::{BuildContainer, BuildConfig};
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
    ];
    let mut files = 0usize;
    let mut bytes = 0u64;
    let mut has_size = false;

    for line in text.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.is_empty() {
            continue;
        }
        let raw_path = if kind == "rpm" {
            tokens[0]
        } else {
            *tokens.last().unwrap()
        };
        let path = raw_path
            .trim_start_matches("./")
            .trim_start_matches('/');
        if path.ends_with('/') || metadata.iter().any(|m| path.split('/').any(|seg| seg == *m)) {
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

/// Prüft alle gebauten Pakete im Container. Ein Paket ohne Nutzdateien wird
/// entfernt und führt zum automatischen Wechsel auf das direkte Build-System.
pub fn verify_built_packages(
    container: &BuildContainer,
    _config: &BuildConfig,
    tx: &Sender<crate::core::container::BuildEvent>,
) -> bool {
    let Ok(listing) = container.exec("find /work/dist -maxdepth 1 -type f", tx) else {
        return true;
    };
    let mut checked = 0usize;
    let mut invalid: Vec<String> = Vec::new();

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
                    if total > 0 { Some(total) } else { None }
                })
        } else {
            None
        };
        let (files, bytes, has_size) =
            listing_payload_with_size(&output.text, kind, forced_bytes);
        let plausible = files > 0 && (!has_size || bytes >= 256 || files >= 2);
        if !plausible {
            invalid.push(path.to_string());
        }
    }

    if checked == 0 || invalid.is_empty() {
        return true;
    }

    let _ = tx.send(crate::core::container::BuildEvent::Log(
        "[warn] Erzeugtes Paket enthält keine Programmdateien – wechsle auf direktes Build-System..."
            .to_string(),
    ));
    for path in invalid {
        let _ = container.exec(&format!("rm -f '{}'", path), tx);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpm_size_abfrage_ist_enthalten() {
        assert!(rpm_size_command("/work/dist/app.rpm").contains("%{SIZE}"));
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
