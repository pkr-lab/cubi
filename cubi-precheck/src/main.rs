use std::path::PathBuf;

use cubi_precheck::{app::App, config};

const USAGE: &str = "Aufruf: cubi-precheck [--config <ordner>] [--cli] [<repo-pfad>]\n  --cli  Checks ohne Oberfläche ausführen (Exit-Code 0 = alle Pflicht-Checks grün)";

fn main() -> eframe::Result {
    let mut config_dir: Option<PathBuf> = None;
    let mut repo: Option<PathBuf> = None;
    let mut cli = false;
    let mut args = std::env::args_os().skip(1);
    while let Some(a) = args.next() {
        match a.to_str() {
            Some("--config") => config_dir = args.next().map(PathBuf::from),
            Some("--cli") => cli = true,
            Some("-h" | "--help") => {
                println!("{USAGE}");
                return Ok(());
            }
            _ if repo.is_none() => repo = Some(PathBuf::from(a)),
            _ => {
                eprintln!("{USAGE}");
                std::process::exit(2);
            }
        }
    }
    let dir = config_dir.unwrap_or_else(config::default_dir);
    let cfg = config::Config::load(&dir).map_err(|e| format!("{e:#}"));
    let repo = repo.or_else(|| {
        let cwd = std::env::current_dir().ok()?;
        cwd.join(".git").exists().then_some(cwd)
    });
    if cli {
        let result = cfg.map_err(anyhow::Error::msg).and_then(|cfg| {
            let repo = repo.clone().unwrap_or_else(|| PathBuf::from("."));
            cubi_precheck::cli::run(&cfg, &repo)
        });
        std::process::exit(match result {
            Ok(true) => 0,
            Ok(false) => 1,
            Err(e) => {
                eprintln!("Fehler: {e:#}");
                2
            }
        });
    }
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("cubi-precheck")
            .with_inner_size([1280.0, 820.0]),
        ..Default::default()
    };
    eframe::run_native(
        "cubi-precheck",
        options,
        Box::new(move |_cc| Ok(Box::new(App::new(cfg, repo)))),
    )
}
