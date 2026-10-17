//! The panel for screen readers: the bar, the dock, the pop-ups and the notification list as an AT-SPI tree (through AccessKit),
//! so Orca (or any AT-SPI reader) can read them and press their buttons. The panel draws its own pixels, so none of this exists
//! unless we describe it: this module turns what the panel knows (its model and the clickable areas of each surface) into nodes.
//!
//! `tree()` is a pure function of the model and the hit areas, tested without a display. `A11y` owns the connection: it hands
//! the tree to the accessibility bus and turns a reader's "click" back into the same `shell::Action` a real click would send.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use accesskit::{Action as AkAction, ActionHandler, ActionRequest, ActivationHandler, DeactivationHandler, Live, Node, NodeId, Role, Tree, TreeId, TreeUpdate};

use crate::shell::{self, Action, Hit, Model};

/// Which surface a list of hit areas belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Bar,
    Dock,
    Toasts,
}

const ROOT: NodeId = NodeId(1);
const BAR: NodeId = NodeId(2);
const DOCK: NodeId = NodeId(3);
const TOASTS: NodeId = NodeId(4);
const LIST: NodeId = NodeId(5);

/// What a reader says for an area: (role, name, extra description). None = not worth exposing.
pub fn describe(a: &Action, m: &Model) -> Option<(Role, String, String)> {
    let button = |name: String| Some((Role::Button, name, String::new()));
    match a {
        Action::Logo => button("Start menu".into()),
        Action::Clock => Some((Role::Button, format!("Clock, {}", m.clock.replace('\n', " ")), "Opens the calendar".into())),
        Action::Workspace(i) => button(format!("Desktop {}", i + 1)),
        Action::NewWorkspace => button("New desktop".into()),
        Action::Notch => button("Fylgja".into()),
        Action::NotchSection(i) => button(crate::fylgja::SECTIONS.get(*i).map(|s| format!("Fylgja, {s}")).unwrap_or_else(|| "Fylgja section".into())),
        Action::Network => button("Network".into()),
        Action::Bluetooth => button("Bluetooth".into()),
        Action::Dock(id) => {
            let items = shell::dock_items(m);
            let it = items.iter().find(|i| &i.app_id == id)?;
            let state = if it.active { ", focused" } else if !it.wins.is_empty() { ", running" } else { "" };
            button(format!("{}{}", it.name, state))
        }
        Action::DockWin(_, wi) => {
            let w = m.wins.get(*wi)?;
            button(format!("Window: {}{}", if w.title.is_empty() { &w.app_id } else { &w.title }, if w.active() { ", focused" } else { "" }))
        }
        Action::ToastOpen(id) => {
            let n = m.notifs.get(*id)?;
            Some((Role::ListItem, format!("{}: {}", n.app, n.summary), n.body.clone()))
        }
        Action::ToastClose(_) => button("Dismiss notification".into()),
        Action::ToastAction(id, i) => {
            let n = m.notifs.get(*id)?;
            let label = shell_toast_label(n, *i)?;
            button(label)
        }
        Action::Notifications => {
            let n = m.notifs.unread_count();
            button(if n == 0 { "Notifications, none unread".to_string() } else { format!("Notifications, {n} unread") })
        }
        Action::Tray(key) => {
            let t = m.tray.iter().find(|t| &t.key == key)?;
            button(if t.title.is_empty() { t.id.clone() } else { t.title.clone() })
        }
        Action::TrayMore => button("More notification area icons".into()),
    }
}

fn shell_toast_label(n: &crate::notify::Notif, i: usize) -> Option<String> {
    crate::notify::toast_actions(n).get(i).map(|(_, label)| label.to_string())
}

/// A stable id for an area, from what it does (the same area keeps its id while the panel redraws).
fn id_of(a: &Action) -> NodeId {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in format!("{a:?}").bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    NodeId(h | (1 << 40))
}

/// The whole tree and, for every clickable node, the action a click on it means.
pub fn tree(m: &Model, parts: &[(Part, Vec<Hit>)]) -> (TreeUpdate, HashMap<u64, Action>) {
    let mut nodes: Vec<(NodeId, Node)> = vec![];
    let mut table = HashMap::new();
    let mut groups: HashMap<u8, Vec<NodeId>> = HashMap::new();
    for (part, hits) in parts {
        // reading order: left to right along a bar, top to bottom down a side dock
        let mut hits: Vec<&Hit> = hits.iter().collect();
        hits.sort_by(|a, b| (a.rect.1, a.rect.0).partial_cmp(&(b.rect.1, b.rect.0)).unwrap_or(std::cmp::Ordering::Equal));
        for h in hits {
            let Some((role, name, desc)) = describe(&h.action, m) else { continue };
            let id = id_of(&h.action);
            if table.contains_key(&id.0) || nodes.iter().any(|(i, _)| *i == id) {
                continue; // the same area on a second screen
            }
            let mut n = Node::new(role);
            n.set_label(name);
            if !desc.is_empty() {
                n.set_description(desc);
            }
            if role == Role::Button || role == Role::ListItem {
                n.add_action(AkAction::Click);
                table.insert(id.0, h.action.clone());
            }
            nodes.push((id, n));
            groups.entry(*part as u8).or_default().push(id);
        }
    }
    let part_node = |role: Role, name: &str, kids: Vec<NodeId>, live: bool| {
        let mut n = Node::new(role);
        n.set_label(name);
        n.set_children(kids);
        if live {
            n.set_live(Live::Polite);
        }
        n
    };
    let bar = groups.remove(&(Part::Bar as u8)).unwrap_or_default();
    let dock = groups.remove(&(Part::Dock as u8)).unwrap_or_default();
    let toasts = groups.remove(&(Part::Toasts as u8)).unwrap_or_default();
    // the kept notifications, newest first (what the list shows when it is open)
    let mut list_kids = vec![];
    for g in m.notifs.groups().into_iter().take(10) {
        for n in g.items.iter().take(3) {
            let id = NodeId(0x2_0000_0000 + u64::from(n.id));
            let mut node = Node::new(Role::ListItem);
            node.set_label(format!("{}: {}", n.app, n.summary));
            if !n.body.is_empty() {
                node.set_description(n.body.clone());
            }
            nodes.push((id, node));
            list_kids.push(id);
        }
    }
    nodes.push((BAR, part_node(Role::Toolbar, "Top bar", bar, false)));
    nodes.push((DOCK, part_node(Role::Toolbar, "Dock", dock, false)));
    nodes.push((TOASTS, part_node(Role::Log, "New notifications", toasts, true)));
    nodes.push((LIST, part_node(Role::List, "Kept notifications", list_kids, false)));
    let mut root = Node::new(Role::Window);
    root.set_label("Labyrinth desktop");
    root.set_children(vec![BAR, DOCK, TOASTS, LIST]);
    nodes.push((ROOT, root));
    let mut t = Tree::new(ROOT);
    t.toolkit_name = Some("labyrinth-panel".into());
    (TreeUpdate { nodes, tree: Some(t), tree_id: TreeId::ROOT, focus: ROOT }, table)
}

struct Activation(Arc<Mutex<Option<TreeUpdate>>>);

impl ActivationHandler for Activation {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        self.0.lock().ok().and_then(|g| g.clone())
    }
}

struct Actions {
    table: Arc<Mutex<HashMap<u64, Action>>>,
    tx: calloop::channel::Sender<Action>,
}

impl ActionHandler for Actions {
    fn do_action(&mut self, req: ActionRequest) {
        if req.action != AkAction::Click {
            return;
        }
        let action = self.table.lock().ok().and_then(|t| t.get(&req.target_node.0).cloned());
        if let Some(a) = action {
            let _ = self.tx.send(a);
        }
    }
}

struct Deactivation;

impl DeactivationHandler for Deactivation {
    fn deactivate_accessibility(&mut self) {}
}

/// The connection to the accessibility bus.
pub struct A11y {
    adapter: accesskit_unix::Adapter,
    latest: Arc<Mutex<Option<TreeUpdate>>>,
    table: Arc<Mutex<HashMap<u64, Action>>>,
    last_key: String,
}

impl A11y {
    pub fn new(tx: calloop::channel::Sender<Action>) -> A11y {
        let latest = Arc::new(Mutex::new(None));
        let table = Arc::new(Mutex::new(HashMap::new()));
        let adapter = accesskit_unix::Adapter::new(Activation(latest.clone()), Actions { table: table.clone(), tx }, Deactivation);
        A11y { adapter, latest, table, last_key: String::new() }
    }

    /// Send the current tree if it changed since the last call.
    pub fn sync(&mut self, m: &Model, parts: &[(Part, Vec<Hit>)]) {
        let (update, actions) = tree(m, parts);
        // what a reader can see: names, descriptions and structure. Skip the update when none of it changed.
        let key = format!("{:?}", update.nodes.iter().map(|(i, n)| (i.0, n.label().map(str::to_owned), n.description().map(str::to_owned), n.children().len())).collect::<Vec<_>>());
        if key == self.last_key {
            return;
        }
        self.last_key = key;
        if let Ok(mut t) = self.table.lock() {
            *t = actions;
        }
        if let Ok(mut l) = self.latest.lock() {
            *l = Some(update.clone());
        }
        self.adapter.update_if_active(|| update);
    }
}
