use std::path::PathBuf;

use cubi_desktop::{app::DesktopApp, cubi_config::CubiConfig, precheck, views::precheck::Precheck};

const USAGE: &str =
    "Aufruf: cubi-desktop [--config <ordner>] [--bereich <name>] [--cli] [<repo-pfad>]
  --bereich  Startbereich, z. B. argocd, kubernetes, github, tokens, login, precheck
  --cli      Pre-Push-Checks ohne Oberfläche ausführen (Exit-Code 0 = alle Pflicht-Checks grün)";

const APP_ICON: &[u8] = include_bytes!("../assets/icon-256.png");

#[derive(serde::Deserialize, Default)]
struct DesktopConfig {
    #[serde(default)]
    terminal: Vec<String>,
    #[serde(default)]
    env: std::collections::BTreeMap<String, String>,
}

fn prepare_env(desktop: &DesktopConfig) {
    let shell = cubi_desktop::shell::shell_env();
    let home = dirs::home_dir().unwrap_or_default();
    let mut updates = cubi_desktop::shell::env_updates(&shell, |k| std::env::var(k).ok(), &home);
    for (k, v) in &desktop.env {
        let v = precheck::config::expand_home(v).display().to_string();
        updates.push((k.clone(), v));
    }
    for (k, v) in updates {
        // SAFETY: Läuft vor dem Start der Runtime und des Fensters, noch ohne weitere Threads.
        unsafe { std::env::set_var(k, v) };
    }
}

fn main() -> eframe::Result {
    let mut config_dir: Option<PathBuf> = None;
    let mut repo: Option<PathBuf> = None;
    let mut start: Option<String> = None;
    let mut cli = false;
    let mut args = std::env::args_os().skip(1);
    while let Some(a) = args.next() {
        match a.to_str() {
            Some("--config") => config_dir = args.next().map(PathBuf::from),
            Some("--bereich") => start = args.next().map(|s| s.to_string_lossy().into_owned()),
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
    let dir = config_dir.unwrap_or_else(precheck::config::default_dir);
    let desktop: DesktopConfig = std::fs::read_to_string(dir.join("desktop.toml"))
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default();
    prepare_env(&desktop);
    let pc_cfg =
        precheck::config::Config::load(&dir.join("precheck")).map_err(|e| format!("{e:#}"));
    if cli {
        let result = pc_cfg.map_err(anyhow::Error::msg).and_then(|cfg| {
            let repo = repo.clone().unwrap_or_else(|| PathBuf::from("."));
            precheck::cli::run(&cfg, &repo)
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
    let repo = repo.or_else(|| {
        let cwd = std::env::current_dir().ok()?;
        cwd.join(".git").exists().then_some(cwd)
    });
    if repo.is_some() && start.is_none() {
        start = Some("precheck".into());
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio-Runtime");
    let cubi = CubiConfig::load().map_err(|e| format!("{e:#}"));
    let precheck = Precheck::new(pc_cfg, repo);
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title("cubi")
        .with_app_id("cubi-desktop")
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size([980.0, 620.0]);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(APP_ICON) {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "cubi-desktop",
        options,
        Box::new(move |cc| {
            Ok(Box::new(DesktopApp::new(
                cc,
                rt,
                cubi,
                desktop.terminal,
                precheck,
                start,
            )))
        }),
    )
}
