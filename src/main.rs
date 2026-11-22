mod cli;
mod core;
mod gui;

use std::env;
use std::process::Command;

/// Eingebettetes Programm-Icon für Titelleiste, Taskleiste und Alt-Tab.
fn app_icon() -> Option<egui::IconData> {
    const ICON: &[u8] = include_bytes!("../assets/app_icon.png");
    let image = image::load_from_memory(ICON).ok()?.to_rgba8();
    let (width, height) = image.dimensions();
    Some(egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    })
}

/// Position des zweiten angeschlossenen Monitors aus `xrandr` lesen.
/// Fällt auf `None` zurück, wenn nur ein Monitor vorhanden ist oder xrandr fehlt.
fn second_monitor_position() -> Option<egui::Pos2> {
    let output = Command::new("xrandr").arg("--query").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut offsets = Vec::new();
    for line in text.lines() {
        if !line.contains(" connected") {
            continue;
        }
        for token in line.split_whitespace() {
            // Geometrie sieht z. B. aus wie `1920x1080+1920+0`.
            if token.contains('x') && (token.contains('+') || token.contains('-')) {
                if let Some(offset) = parse_geometry(token) {
                    offsets.push(offset);
                }
                break;
            }
        }
    }
    let (x, y) = *offsets.get(1)?;
    Some(egui::pos2(x as f32, y as f32))
}

fn parse_geometry(token: &str) -> Option<(i32, i32)> {
    let start = token.find(['+', '-'])?;
    let mut numbers = Vec::new();
    let mut current = String::new();
    let mut sign = 1i32;
    for c in token[start..].chars() {
        match c {
            '+' => {
                if !current.is_empty() {
                    numbers.push(sign * current.parse::<i32>().ok()?);
                    current.clear();
                }
                sign = 1;
            }
            '-' => {
                if !current.is_empty() {
                    numbers.push(sign * current.parse::<i32>().ok()?);
                    current.clear();
                }
                sign = -1;
            }
            d if d.is_ascii_digit() => current.push(d),
            _ => break,
        }
    }
    if !current.is_empty() {
        numbers.push(sign * current.parse::<i32>().ok()?);
    }
    Some((*numbers.first()?, *numbers.get(1)?))
}

fn print_usage() {
    println!("git2pkg - isolated package builder from Git repositories");
    println!();
    println!("Usage:");
    println!("  git2pkg                         Start the graphical interface");
    println!("  git2pkg --cli <URL> [options]   Build without the GUI");
    println!();
    println!("CLI options:");
    println!("  --format=package|binary         Package type (default: package)");
    println!("  --target=auto|arch|debian|fedora|opensuse");
    println!("                                  Target distribution (default: auto)");
    println!("  --binary                        Alias for --format=binary");
    println!("  --reset-learning                Delete learned mappings and exit");
    println!("  -V, --version                   Show the version");
    println!("  -h, --help                      Show this help");
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = env::args().collect();
    if args
        .iter()
        .skip(1)
        .any(|arg| arg == "-V" || arg == "--version")
    {
        println!("git2pkg {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args
        .iter()
        .skip(1)
        .any(|arg| arg == "-h" || arg == "--help")
    {
        print_usage();
        return Ok(());
    }
    if args.iter().skip(1).any(|arg| arg == "--reset-learning") {
        cli::reset_learning();
        return Ok(());
    }
    if args.len() >= 2 && args[1] == "--cli" && args.len() < 3 {
        eprintln!("git2pkg: --cli requires a repository URL");
        print_usage();
        return Ok(());
    }
    if args.len() >= 3 && args[1] == "--cli" {
        cli::run_cli(&args[2]);
        return Ok(());
    }

    let monitor_position = second_monitor_position();
    // Wayland erlaubt Clients nicht, ihre Fenster selbst zu positionieren.
    // Wenn ein zweiter Monitor vorhanden ist, daher X11/XWayland nutzen.
    if monitor_position.is_some() && env::var_os("WAYLAND_DISPLAY").is_some() {
        env::remove_var("WAYLAND_DISPLAY");
    }

    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([1145.0, 780.0])
        .with_min_inner_size([785.0, 520.0])
        .with_title(format!("git2pkg v{}", env!("CARGO_PKG_VERSION")));
    let viewport = match app_icon() {
        Some(icon) => viewport.with_icon(icon),
        None => viewport,
    };
    let viewport = match monitor_position {
        Some(position) => viewport.with_position(position),
        None => viewport,
    };

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "git2pkg",
        options,
        Box::new(|cc| {
            gui::fonts::install(&cc.egui_ctx);
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(gui::app::Git2PkgApp::new()))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_geometry_liest_monitor_offsets() {
        assert_eq!(parse_geometry("1920x1080+1920+0"), Some((1920, 0)));
        assert_eq!(parse_geometry("1920x1080+0+0"), Some((0, 0)));
        assert_eq!(parse_geometry("2560x1440+1920-40"), Some((1920, -40)));
    }
}
