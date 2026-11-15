use std::time::Duration;

use eframe::egui::{self, RichText, Ui};
use serde_json::Value;

use crate::shell;
use crate::timeutil::age;
use crate::ui::{self, Cell, Cx, OutputWindow, TableSpec, Task, Tone};

pub fn pod_status(pod: &Value) -> String {
    if pod
        .pointer("/metadata/deletionTimestamp")
        .is_some_and(|v| !v.is_null())
    {
        return "Terminating".into();
    }
    let status = pod.get("status").cloned().unwrap_or(Value::Null);
    let gs = |v: &Value, p: &str| v.pointer(p).and_then(Value::as_str).map(str::to_string);
    let mut reason = gs(&status, "/reason")
        .or_else(|| gs(&status, "/phase"))
        .unwrap_or_else(|| "Unknown".into());
    for c in status
        .get("initContainerStatuses")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(w) = gs(c, "/state/waiting/reason").filter(|s| !s.is_empty()) {
            return format!("Init:{w}");
        }
        if let Some(t) = c.pointer("/state/terminated")
            && t.get("exitCode").and_then(Value::as_i64).unwrap_or(0) != 0
        {
            return format!(
                "Init:{}",
                t.get("reason")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Error")
            );
        }
    }
    let phase = gs(&status, "/phase");
    for c in status
        .get("containerStatuses")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(w) = gs(c, "/state/waiting/reason").filter(|s| !s.is_empty()) {
            reason = w;
        } else if let Some(t) = gs(c, "/state/terminated/reason").filter(|s| !s.is_empty())
            && phase.as_deref() != Some("Succeeded")
        {
            reason = t;
        }
    }
    reason
}

pub fn status_tone(status: &str, ready: bool) -> Tone {
    match status {
        "Running" => {
            if ready {
                Tone::Ok
            } else {
                Tone::Warn
            }
        }
        "Completed" | "Succeeded" => Tone::Muted,
        "Pending" | "ContainerCreating" | "Terminating" | "PodInitializing" => Tone::Warn,
        s if s.starts_with("Init:") => Tone::Warn,
        _ => Tone::Bad,
    }
}

#[derive(Clone)]
pub struct Pod {
    pub namespace: String,
    pub name: String,
    pub ready: String,
    pub status: String,
    pub tone: Tone,
    pub restarts: i64,
    pub age: String,
    pub node: String,
}

pub fn parse_pods(data: &Value) -> Vec<Pod> {
    let mut items: Vec<&Value> = data
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    let key = |p: &Value, k: &str| {
        p.pointer(&format!("/metadata/{k}"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    items.sort_by_key(|p| (key(p, "namespace"), key(p, "name")));
    items
        .into_iter()
        .map(|p| {
            let statuses = p
                .pointer("/status/containerStatuses")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let total = p
                .pointer("/spec/containers")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let ready = statuses
                .iter()
                .filter(|c| c.get("ready").and_then(Value::as_bool).unwrap_or(false))
                .count();
            let status = pod_status(p);
            Pod {
                namespace: key(p, "namespace"),
                name: key(p, "name"),
                ready: format!("{ready}/{total}"),
                tone: status_tone(&status, ready == total),
                status,
                restarts: statuses
                    .iter()
                    .filter_map(|c| c.get("restartCount").and_then(Value::as_i64))
                    .sum(),
                age: age(&key(p, "creationTimestamp")),
                node: p
                    .pointer("/spec/nodeName")
                    .and_then(Value::as_str)
                    .unwrap_or("-")
                    .into(),
            }
        })
        .collect()
}

#[derive(Clone)]
pub struct Node {
    pub name: String,
    pub ready: bool,
    pub roles: String,
    pub version: String,
    pub ip: String,
    pub age: String,
}

pub fn parse_nodes(data: &Value) -> Vec<Node> {
    data.get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|n| {
            let ready = n
                .pointer("/status/conditions")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .any(|c| {
                    c.get("type").and_then(Value::as_str) == Some("Ready")
                        && c.get("status").and_then(Value::as_str) == Some("True")
                });
            let prefix = "node-role.kubernetes.io/";
            let roles: Vec<String> = n
                .pointer("/metadata/labels")
                .and_then(Value::as_object)
                .map(|l| {
                    l.keys()
                        .filter_map(|k| k.strip_prefix(prefix).map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let ip = n
                .pointer("/status/addresses")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|a| a.get("type").and_then(Value::as_str) == Some("InternalIP"))
                .and_then(|a| a.get("address").and_then(Value::as_str))
                .unwrap_or("-");
            Node {
                name: n
                    .pointer("/metadata/name")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .into(),
                ready,
                roles: if roles.is_empty() {
                    "-".into()
                } else {
                    roles.join(", ")
                },
                version: n
                    .pointer("/status/nodeInfo/kubeletVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("-")
                    .into(),
                ip: ip.into(),
                age: age(n
                    .pointer("/metadata/creationTimestamp")
                    .and_then(Value::as_str)
                    .unwrap_or_default()),
            }
        })
        .collect()
}

async fn query(args: &[&str]) -> String {
    match shell::capture(args, 10).await {
        Ok(o) if o.ok() => o.stdout.trim().to_string(),
        _ => String::new(),
    }
}

#[derive(Clone, Default)]
struct Env {
    context: String,
    namespace: String,
    contexts: Vec<String>,
    namespaces: Vec<String>,
}

async fn load_env() -> anyhow::Result<Env> {
    let ns = query(&[
        "kubectl",
        "config",
        "view",
        "--minify",
        "-o",
        "jsonpath={..namespace}",
    ])
    .await;
    let ctx = query(&["kubectl", "config", "current-context"]).await;
    let contexts = query(&["kubectl", "config", "get-contexts", "-o", "name"])
        .await
        .lines()
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .collect();
    let namespaces =
        match shell::capture_json(&["kubectl", "get", "namespaces", "-o", "json"], 15).await {
            Ok(v) => {
                let mut n: Vec<String> = v
                    .get("items")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|i| {
                        i.pointer("/metadata/name")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .collect();
                n.sort();
                n
            }
            Err(_) => vec![],
        };
    Ok(Env {
        context: if ctx.is_empty() { "-".into() } else { ctx },
        namespace: if ns.is_empty() { "default".into() } else { ns },
        contexts,
        namespaces,
    })
}

#[derive(PartialEq, Eq, Clone, Copy, Default)]
enum Tab {
    #[default]
    Pods,
    Nodes,
    Events,
}

#[derive(PartialEq, Eq, Clone)]
enum Pending {
    Delete(String, String),
}

#[derive(Default)]
pub struct Kube {
    tab: Tab,
    env: Task<Env>,
    switch: Task<()>,
    all_ns: bool,
    pods: Task<Vec<Pod>>,
    pods_scope: Option<String>,
    nodes: Task<Vec<Node>>,
    sel_pod: Option<usize>,
    sel_node: Option<usize>,
    out: Option<OutputWindow>,
    confirm: Option<Pending>,
    error: Option<String>,
}

impl Kube {
    fn pod_actions(&mut self, ui: &mut Ui, cx: &Cx, p: &Pod) {
        ui::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&p.name).strong());
                ui.label(RichText::new(&p.namespace).color(ui::theme::MUTED));
                ui::pill(ui, &p.status, p.tone);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (ns, name) = (p.namespace.clone(), p.name.clone());
                    if ui.add(ui::danger_button("Löschen")).clicked() {
                        self.confirm = Some(Pending::Delete(ns.clone(), name.clone()));
                    }
                    if ui.button("Shell").clicked() {
                        self.error = cx
                            .terminal(
                                [
                                    "kubectl",
                                    "exec",
                                    "-it",
                                    "-n",
                                    &ns,
                                    &name,
                                    "--",
                                    "sh",
                                    "-c",
                                    "command -v bash >/dev/null 2>&1 && exec bash || exec sh",
                                ]
                                .iter()
                                .map(|s| s.to_string())
                                .collect(),
                            )
                            .err();
                    }
                    if ui.button("Describe").clicked() {
                        self.out = Some(OutputWindow::new(
                            cx,
                            format!("Describe · {name}"),
                            "",
                            sv(&["kubectl", "describe", "pod", "-n", &ns, &name]),
                            30,
                        ));
                    }
                    if ui.button("Logs folgen").clicked() {
                        self.error = cx
                            .terminal(sv(&[
                                "kubectl",
                                "logs",
                                "-f",
                                "-n",
                                &ns,
                                &name,
                                "--all-containers=true",
                                "--tail=100",
                            ]))
                            .err();
                    }
                    if ui.button("Logs").clicked() {
                        self.out = Some(OutputWindow::new(
                            cx,
                            format!("Logs · {name}"),
                            "Keine Logs",
                            sv(&[
                                "kubectl",
                                "logs",
                                "-n",
                                &ns,
                                &name,
                                "--all-containers=true",
                                "--tail=300",
                            ]),
                            60,
                        ));
                    }
                });
            });
        });
        ui.add_space(6.0);
    }
}

pub fn sv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

impl super::View for Kube {
    fn id(&self) -> &'static str {
        "kubernetes"
    }
    fn title(&self) -> &'static str {
        "Kubernetes"
    }
    fn icon(&self) -> &'static str {
        "☸"
    }
    fn hint(&self) -> &'static str {
        "Pods, Nodes, Warnungen, Kontext"
    }

    fn ui(&mut self, ui: &mut Ui, cx: &Cx) {
        self.env.poll();
        if self.switch.poll() {
            if let Some(e) = self.switch.error() {
                self.error = Some(e.to_string());
            }
            self.env.clear();
            self.pods.clear();
            self.nodes.clear();
        }
        if self.env.result.is_none() && !self.env.busy() {
            self.env.spawn(cx, load_env());
        }
        let env = self.env.value().cloned().unwrap_or_default();
        ui::page_header(ui, "Kubernetes", "", |ui| {
            if ui.button("⟳ Neu laden").clicked() {
                self.pods.at = None;
                self.nodes.at = None;
                self.env.clear();
            }
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("Kontext").color(ui::theme::MUTED));
            let mut pick = None;
            egui::ComboBox::from_id_salt("kctx")
                .selected_text(&env.context)
                .show_ui(ui, |ui| {
                    for c in &env.contexts {
                        if ui.selectable_label(*c == env.context, c).clicked() {
                            pick = Some(c.clone());
                        }
                    }
                });
            if let Some(c) = pick.filter(|c| *c != env.context) {
                self.switch.spawn(cx, async move {
                    shell::capture_ok(
                        &["kubectl", "config", "use-context", &c],
                        10,
                        "Kontext konnte nicht gesetzt werden",
                    )
                    .await?;
                    Ok(())
                });
            }
            ui.add_space(12.0);
            ui.label(RichText::new("Namespace").color(ui::theme::MUTED));
            let mut pick = None;
            egui::ComboBox::from_id_salt("kns")
                .selected_text(&env.namespace)
                .height(400.0)
                .show_ui(ui, |ui| {
                    for n in &env.namespaces {
                        if ui.selectable_label(*n == env.namespace, n).clicked() {
                            pick = Some(n.clone());
                        }
                    }
                });
            if let Some(n) = pick.filter(|n| *n != env.namespace) {
                self.switch.spawn(cx, async move {
                    let arg = format!("--namespace={n}");
                    shell::capture_ok(
                        &["kubectl", "config", "set-context", "--current", &arg],
                        10,
                        "Namespace konnte nicht gesetzt werden",
                    )
                    .await?;
                    Ok(())
                });
            }
            if self.switch.busy() || self.env.busy() {
                ui.spinner();
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            for (t, label) in [
                (Tab::Pods, "Pods"),
                (Tab::Nodes, "Nodes"),
                (Tab::Events, "Warnungen"),
            ] {
                if ui
                    .selectable_label(self.tab == t, RichText::new(label).strong())
                    .clicked()
                {
                    if t == Tab::Events {
                        self.out = Some(OutputWindow::new(
                            cx,
                            "Warnungen (Events vom Typ Warning)",
                            "Keine Warnungen",
                            sv(&[
                                "kubectl",
                                "get",
                                "events",
                                "-A",
                                "--field-selector",
                                "type=Warning",
                                "--sort-by=.lastTimestamp",
                            ]),
                            30,
                        ));
                    } else {
                        self.tab = t;
                    }
                }
            }
        });
        ui.separator();
        if let Some(e) = &self.error {
            ui::error_box(ui, e);
        }
        match self.tab {
            Tab::Pods => {
                let scope = if self.all_ns {
                    None
                } else {
                    Some(env.namespace.clone())
                };
                let key = scope.clone().unwrap_or_else(|| "*".into());
                if self.pods_scope.as_deref() != Some(key.as_str()) {
                    self.pods.clear();
                    self.sel_pod = None;
                    self.pods_scope = Some(key);
                }
                let c = cx.clone();
                if self.env.value().is_some() {
                    ui::auto_refresh(ui, &mut self.pods, Duration::from_secs(10), |t| {
                        t.spawn(&c, async move {
                            let mut args = vec!["kubectl", "get", "pods"];
                            match &scope {
                                Some(ns) => args.extend(["-n", ns.as_str()]),
                                None => args.push("-A"),
                            }
                            args.extend(["-o", "json"]);
                            Ok(parse_pods(&shell::capture_json(&args, 30).await?))
                        })
                    });
                }
                let pods = self.pods.value().cloned().unwrap_or_default();
                let problems = pods
                    .iter()
                    .filter(|p| matches!(p.tone, Tone::Bad | Tone::Warn))
                    .count();
                ui.horizontal(|ui| {
                    if self.pods.error().is_none() && self.pods.value().is_some() {
                        ui::pill(ui, format!("{} Pods", pods.len()), Tone::Muted);
                        ui::pill(
                            ui,
                            format!("{problems} mit Auffälligkeiten"),
                            if problems > 0 { Tone::Warn } else { Tone::Ok },
                        );
                    }
                    ui.checkbox(&mut self.all_ns, "alle Namespaces");
                });
                ui.add_space(4.0);
                if let Some(p) = self.sel_pod.and_then(|i| pods.get(i)).cloned() {
                    self.pod_actions(ui, cx, &p);
                }
                if ui::task_status(ui, &self.pods, "Lade Pods …") {
                    let rows: Vec<Vec<Cell>> = pods
                        .iter()
                        .map(|p| {
                            let mut r = Vec::new();
                            if self.all_ns {
                                r.push(Cell::muted(&p.namespace));
                            }
                            r.extend([
                                Cell::plain(&p.name),
                                Cell::muted(&p.ready),
                                Cell::pill(&p.status, p.tone),
                                Cell::tone(
                                    p.restarts.to_string(),
                                    if p.restarts > 0 {
                                        Tone::Warn
                                    } else {
                                        Tone::Muted
                                    },
                                ),
                                Cell::muted(&p.age),
                                Cell::muted(&p.node),
                            ]);
                            r
                        })
                        .collect();
                    let headers: &[&str] = if self.all_ns {
                        &[
                            "Namespace",
                            "Pod",
                            "Ready",
                            "Status",
                            "Restarts",
                            "Alter",
                            "Node",
                        ]
                    } else {
                        &["Pod", "Ready", "Status", "Restarts", "Alter", "Node"]
                    };
                    ui::card(ui, |ui| {
                        ui::table(
                            ui,
                            TableSpec {
                                id: "pods",
                                headers,
                                checks: None,
                            },
                            &rows,
                            &mut self.sel_pod,
                        );
                    });
                }
            }
            Tab::Nodes | Tab::Events => {
                let c = cx.clone();
                ui::auto_refresh(ui, &mut self.nodes, Duration::from_secs(15), |t| {
                    t.spawn(&c, async {
                        Ok(parse_nodes(
                            &shell::capture_json(&["kubectl", "get", "nodes", "-o", "json"], 30)
                                .await?,
                        ))
                    })
                });
                if let Some(n) = self
                    .sel_node
                    .and_then(|i| self.nodes.value().and_then(|v| v.get(i)))
                    .cloned()
                {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&n.name).strong());
                        if ui.button("Describe").clicked() {
                            self.out = Some(OutputWindow::new(
                                cx,
                                format!("Node · {}", n.name),
                                "",
                                sv(&["kubectl", "describe", "node", &n.name]),
                                30,
                            ));
                        }
                    });
                }
                if ui::task_status(ui, &self.nodes, "Lade Nodes …") {
                    let rows: Vec<Vec<Cell>> = self
                        .nodes
                        .value()
                        .unwrap()
                        .iter()
                        .map(|n| {
                            vec![
                                Cell::plain(&n.name),
                                if n.ready {
                                    Cell::pill("Ready", Tone::Ok)
                                } else {
                                    Cell::pill("NotReady", Tone::Bad)
                                },
                                Cell::muted(&n.roles),
                                Cell::muted(&n.version),
                                Cell::mono(&n.ip, Tone::Muted),
                                Cell::muted(&n.age),
                            ]
                        })
                        .collect();
                    ui::card(ui, |ui| {
                        ui::table(
                            ui,
                            TableSpec {
                                id: "nodes",
                                headers: &["Node", "Status", "Rollen", "Version", "IP", "Alter"],
                                checks: None,
                            },
                            &rows,
                            &mut self.sel_node,
                        );
                    });
                }
            }
        }
        if let Some(Pending::Delete(ns, name)) = self.confirm.clone() {
            match ui::confirm(
                ui.ctx(),
                "pod-del",
                &format!("Pod {name} löschen?"),
                &[format!("Namespace {ns}")],
                "Löschen",
                true,
            ) {
                Some(true) => {
                    self.confirm = None;
                    self.out = Some(OutputWindow::new(
                        cx,
                        format!("Löschen · {name}"),
                        "",
                        sv(&["kubectl", "delete", "pod", "-n", &ns, &name]),
                        60,
                    ));
                    self.pods.at = None;
                }
                Some(false) => self.confirm = None,
                None => {}
            }
        }
        ui::show_output(ui.ctx(), &mut self.out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pod_states() {
        let p: Value = serde_json::from_str(r#"{"metadata":{"name":"a"},"status":{"phase":"Running","containerStatuses":[{"ready":false,"restartCount":4,"state":{"waiting":{"reason":"CrashLoopBackOff"}}}]},"spec":{"containers":[{}]}}"#).unwrap();
        assert_eq!(pod_status(&p), "CrashLoopBackOff");
        assert_eq!(status_tone("CrashLoopBackOff", false), Tone::Bad);
        let i: Value = serde_json::from_str(r#"{"metadata":{},"status":{"phase":"Pending","initContainerStatuses":[{"state":{"terminated":{"exitCode":1}}}]}}"#).unwrap();
        assert_eq!(pod_status(&i), "Init:Error");
        let d: Value = serde_json::from_str(r#"{"metadata":{"deletionTimestamp":"x"}}"#).unwrap();
        assert_eq!(pod_status(&d), "Terminating");
        let list = serde_json::json!({"items": [p]});
        let pods = parse_pods(&list);
        assert_eq!((pods[0].ready.as_str(), pods[0].restarts), ("0/1", 4));
        assert_eq!(status_tone("Running", false), Tone::Warn);
    }

    #[test]
    fn nodes() {
        let v: Value = serde_json::from_str(r#"{"items":[{"metadata":{"name":"n","labels":{"node-role.kubernetes.io/control-plane":"true"}},"status":{"conditions":[{"type":"Ready","status":"True"}],"addresses":[{"type":"InternalIP","address":"10.0.0.1"}],"nodeInfo":{"kubeletVersion":"v1.33"}}}]}"#).unwrap();
        let n = parse_nodes(&v);
        assert!(n[0].ready);
        assert_eq!(n[0].roles, "control-plane");
        assert_eq!(n[0].ip, "10.0.0.1");
    }
}
