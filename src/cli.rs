//! Headless-Modus: `git2pkg --cli <url> [--format=package|binary] [--target=auto|arch|debian|fedora|opensuse]`.

use crate::core::container::{
    default_output_dir, detect_engine, run_build, BuildConfig, BuildEvent, TargetFormat,
};
use crate::core::distro::{detect_distro, TargetDistro};
use crate::core::resolver::MappingStore;
use crate::gui::i18n::{Language, Texts};
use std::sync::mpsc;

pub fn run_cli(url: &str) {
    let _ = MappingStore::load();
    let language = Language::detect();
    let texts = Texts::new(language);

    let target_distro = std::env::args()
        .find_map(|arg| arg.strip_prefix("--target=").map(TargetDistro::from_key))
        .unwrap_or(TargetDistro::Auto);
    let target = std::env::args()
        .find_map(|arg| arg.strip_prefix("--format=").map(TargetFormat::from_key))
        .unwrap_or_else(|| {
            if std::env::args().any(|arg| arg == "--binary") {
                TargetFormat::StandaloneBinary
            } else {
                TargetFormat::NativePackage
            }
        });

    let config = BuildConfig {
        url: url.to_string(),
        target,
        target_distro,
        distro: detect_distro(),
        engine: detect_engine(),
        output_dir: default_output_dir(),
        language,
        strategy: crate::core::container::BuildStrategy::Default,
        repo_script: None,
        cmake_compat: false,
        cstdint_fix: false,
        submodule_fix: false,
        rustup_fix: false,
        skip_deps: false,
        cancel_flag: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        container_id: std::sync::Arc::new(std::sync::Mutex::new(None)),
    };

    println!(
        "{}",
        texts.cli_header(config.engine.label(), &config.distro.name)
    );

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || run_build(config, tx));

    for event in rx {
        match event {
            BuildEvent::Step(step) => println!("==> {}", step),
            BuildEvent::Log(line) => println!("{}", line),
            BuildEvent::Finished(outcome) => {
                println!("{}", texts.cli_done());
                for artifact in &outcome.artifacts {
                    println!("    {}", artifact.to_string_lossy());
                }
                println!("{}", texts.cli_install(&outcome.install_command));
                return;
            }
            BuildEvent::Failed(error) => {
                eprintln!("{}", texts.cli_failed(&error));
                std::process::exit(1);
            }
            BuildEvent::ContainerStarted(id) => println!("==> {}", id),
            BuildEvent::Cancelled => {
                eprintln!("[warn] Build vom Benutzer abgebrochen.");
                std::process::exit(130);
            }
        }
    }
}
