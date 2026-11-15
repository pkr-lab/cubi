use eframe::egui::Ui;

use crate::ui::Cx;

pub mod argocd;
pub mod connect;
pub mod github;
pub mod helm;
pub mod kube;
pub mod login;
pub mod precheck;
pub mod tailscale;
pub mod tokens;

pub trait View {
    fn id(&self) -> &'static str;
    fn title(&self) -> &'static str;
    fn icon(&self) -> &'static str;
    fn hint(&self) -> &'static str;
    fn ui(&mut self, ui: &mut Ui, cx: &Cx);
    fn open(&mut self, _arg: &str) {}
}

pub fn all(cx: &Cx, precheck: precheck::Precheck) -> Vec<Box<dyn View>> {
    let _ = cx;
    vec![
        Box::new(precheck),
        Box::new(connect::Connect::default()),
        Box::new(argocd::ArgoCd::default()),
        Box::new(tailscale::Tailscale::default()),
        Box::new(github::GitHub::default()),
        Box::new(kube::Kube::default()),
        Box::new(helm::Helm::default()),
        Box::new(tokens::Tokens::default()),
        Box::new(login::Login::default()),
    ]
}

pub fn alias(name: &str) -> &str {
    match name {
        "argo" | "sync" | "autosync" | "branch" | "ft" | "feature-testing" | "scale" => "argocd",
        "ts" => "tailscale",
        "gh" => "github",
        "k8s" | "kubectl" => "kubernetes",
        "auth" => "login",
        "token" => "tokens",
        "checks" | "push" => "precheck",
        other => other,
    }
}

pub fn argocd_tab(name: &str) -> Option<argocd::Tab> {
    match name {
        "sync" | "autosync" => Some(argocd::Tab::AutoSync),
        "branch" | "ft" | "feature-testing" => Some(argocd::Tab::Feature),
        "scale" => Some(argocd::Tab::Scale),
        _ => None,
    }
}
