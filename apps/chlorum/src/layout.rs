//! The two Chlorum templates. The screen shows either one app or two apps
//! side by side; every other open app waits on a hidden "shelf" workspace.
//! Nothing is placed by hand: the shell works out where windows must be and
//! asks niri to move them there.

use crate::niri::{Action, SizeChange, Window, Workspace, WorkspaceReferenceArg};
use std::collections::{BTreeMap, HashMap};

pub const STAGE: &str = "stage";
pub const SHELF: &str = "shelf";
pub const HOME: &str = "home";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stage {
    Empty,
    One(u64),
    Two { left: u64, right: u64, ratio: f32, right_focused: bool },
}

#[derive(Default)]
pub struct Layout {
    pub stage: Option<Stage>,
    pub windows: BTreeMap<u64, Window>,
    stage_ws: Option<u64>,
    shelf_ws: Option<u64>,
    /// most recently used first
    pub recent: Vec<u64>,
    widths: HashMap<u64, i32>,
    /// the next new window opens next to the current app
    pub beside_next: bool,
    /// how many new windows still form the start-up pair
    pub startup_left: usize,
    /// the template the last plan put on screen
    last_planned: Option<Stage>,
}

impl Layout {
    pub fn stage(&self) -> Stage {
        self.stage.unwrap_or(Stage::Empty)
    }

    pub fn on_stage(&self) -> Vec<u64> {
        match self.stage() {
            Stage::Empty => vec![],
            Stage::One(a) => vec![a],
            Stage::Two { left, right, .. } => vec![left, right],
        }
    }

    /// The app that keys go to now.
    pub fn focused(&self) -> Option<u64> {
        match self.stage() {
            Stage::Empty => None,
            Stage::One(a) => Some(a),
            Stage::Two { left, right, right_focused, .. } => Some(if right_focused { right } else { left }),
        }
    }

    /// Open apps in the order they were last used.
    pub fn apps(&self) -> Vec<&Window> {
        let mut v: Vec<&Window> = self.recent.iter().filter_map(|id| self.windows.get(id)).collect();
        for w in self.windows.values() {
            if !w.is_floating && !self.recent.contains(&w.id) {
                v.push(w);
            }
        }
        v
    }

    fn touch(&mut self, id: u64) {
        self.recent.retain(|x| *x != id);
        self.recent.insert(0, id);
    }

    fn tiled(&self, id: u64) -> bool {
        self.windows.get(&id).is_some_and(|w| !w.is_floating)
    }

    // ------------------------------------------------------------ niri events

    pub fn set_workspaces(&mut self, ws: &[Workspace]) {
        self.stage_ws = ws.iter().find(|w| w.name.as_deref() == Some(STAGE)).map(|w| w.id);
        self.shelf_ws = ws.iter().find(|w| w.name.as_deref() == Some(SHELF)).map(|w| w.id);
    }

    pub fn set_windows(&mut self, list: Vec<Window>) {
        let first = self.stage.is_none();
        self.windows = list.into_iter().map(|w| (w.id, w)).collect();
        if first {
            // The shell (re)started: keep what is on screen.
            let mut on: Vec<&Window> = self
                .windows
                .values()
                .filter(|w| !w.is_floating && w.workspace_id.is_some() && w.workspace_id == self.stage_ws)
                .collect();
            on.sort_by_key(|w| w.layout.pos_in_scrolling_layout.map(|p| p.0).unwrap_or(0));
            self.stage = Some(match on.as_slice() {
                [] => Stage::Empty,
                [a] => Stage::One(a.id),
                [a, b, ..] => Stage::Two { left: a.id, right: b.id, ratio: 0.6, right_focused: b.is_focused },
            });
        } else {
            self.forget_missing();
        }
    }

    /// A window appeared or changed. Returns true when it is a new app window.
    pub fn window_changed(&mut self, w: Window) -> bool {
        let new = !self.windows.contains_key(&w.id);
        let id = w.id;
        let floating = w.is_floating;
        self.windows.insert(id, w);
        if new && !floating {
            self.opened(id);
            return true;
        }
        if !new && floating {
            // turned into a floating window: no longer part of the template
            self.closed(id);
        }
        false
    }

    /// Column positions changed (a column moved or was resized).
    pub fn layouts_changed(&mut self, changes: Vec<(u64, niri_ipc::WindowLayout)>) {
        for (id, l) in changes {
            if let Some(w) = self.windows.get_mut(&id) {
                w.layout = l;
            }
        }
    }

    /// Keeps track of which window niri says has the keys.
    pub fn set_focused(&mut self, id: Option<u64>) {
        for w in self.windows.values_mut() {
            w.is_focused = Some(w.id) == id;
        }
    }

    pub fn window_closed(&mut self, id: u64) {
        self.windows.remove(&id);
        self.widths.remove(&id);
        self.recent.retain(|x| *x != id);
        self.closed(id);
    }

    /// Focus moved by itself: an app asked for attention, or the person clicked.
    pub fn focus_changed(&mut self, id: Option<u64>) {
        let Some(id) = id else { return };
        if !self.tiled(id) {
            return;
        }
        match self.stage() {
            Stage::Two { left, right, ratio, .. } if id == left || id == right => {
                self.stage = Some(Stage::Two { left, right, ratio, right_focused: id == right });
                self.touch(id);
            }
            Stage::One(a) if a == id => self.touch(id),
            _ => {
                // An app on the shelf took focus (a link opened in the browser,
                // for example): bring it forward the normal way.
                self.bring(id);
            }
        }
    }

    fn forget_missing(&mut self) {
        for id in self.on_stage() {
            if !self.tiled(id) {
                self.closed(id);
            }
        }
    }

    fn opened(&mut self, id: u64) {
        if self.startup_left > 0 {
            self.startup_left -= 1;
            match self.stage() {
                Stage::One(a) => {
                    self.stage = Some(Stage::Two { left: a, right: id, ratio: 0.6, right_focused: false });
                    self.touch(id);
                    return;
                }
                Stage::Empty => {
                    self.stage = Some(Stage::One(id));
                    self.touch(id);
                    return;
                }
                _ => {}
            }
        }
        if self.beside_next {
            self.beside_next = false;
            self.beside(id);
        } else {
            self.bring(id);
        }
    }

    fn closed(&mut self, id: u64) {
        self.stage = Some(match self.stage() {
            Stage::One(a) if a == id => Stage::Empty,
            Stage::Two { left, right, .. } if left == id => Stage::One(right),
            Stage::Two { left, right, .. } if right == id => Stage::One(left),
            s => s,
        });
    }

    // ------------------------------------------------------------ what people do

    /// Show an app: alone, or in place of the app that has the keys.
    pub fn bring(&mut self, id: u64) {
        if !self.tiled(id) {
            return;
        }
        self.stage = Some(match self.stage() {
            Stage::Empty | Stage::One(_) => Stage::One(id),
            Stage::Two { left, right, ratio, right_focused } => {
                if id == left || id == right {
                    Stage::Two { left, right, ratio, right_focused: id == right }
                } else if right_focused {
                    Stage::Two { left, right: id, ratio, right_focused }
                } else {
                    Stage::Two { left: id, right, ratio, right_focused }
                }
            }
        });
        self.touch(id);
    }

    /// Show an app next to the current one.
    pub fn beside(&mut self, id: u64) {
        if !self.tiled(id) {
            return;
        }
        self.stage = Some(match self.stage() {
            Stage::Empty => Stage::One(id),
            Stage::One(a) if a == id => Stage::One(a),
            Stage::One(a) => Stage::Two { left: a, right: id, ratio: 0.6, right_focused: true },
            Stage::Two { left, right, ratio, right_focused } => {
                if id == left || id == right {
                    Stage::Two { left, right, ratio, right_focused: id == right }
                } else if right_focused {
                    Stage::Two { left, right: id, ratio, right_focused }
                } else {
                    Stage::Two { left: id, right, ratio, right_focused }
                }
            }
        });
        self.touch(id);
    }

    /// One app ⇄ two apps. Returns false when there is no second app to show.
    pub fn toggle_two(&mut self) -> bool {
        match self.stage() {
            Stage::One(a) => {
                let other = self.recent.iter().copied().find(|x| *x != a && self.tiled(*x));
                match other {
                    Some(b) => {
                        self.stage = Some(Stage::Two { left: a, right: b, ratio: 0.6, right_focused: false });
                        true
                    }
                    None => false,
                }
            }
            Stage::Two { .. } => {
                let f = self.focused().unwrap();
                self.stage = Some(Stage::One(f));
                true
            }
            Stage::Empty => false,
        }
    }

    pub fn focus_side(&mut self, right: bool) {
        if let Stage::Two { left, right: r, ratio, .. } = self.stage() {
            self.stage = Some(Stage::Two { left, right: r, ratio, right_focused: right });
            self.touch(if right { r } else { left });
        }
    }

    pub fn swap(&mut self) {
        if let Stage::Two { left, right, ratio, right_focused } = self.stage() {
            self.stage = Some(Stage::Two { left: right, right: left, ratio: 1.0 - ratio, right_focused: !right_focused });
        }
    }

    /// Give the app with the keys more (or less) room.
    pub fn resize(&mut self, grow: bool) {
        if let Stage::Two { left, right, ratio, right_focused } = self.stage() {
            let step = if grow == right_focused { -0.1 } else { 0.1 };
            let ratio = ((ratio + step) * 10.0).round() / 10.0;
            self.stage = Some(Stage::Two { left, right, ratio: ratio.clamp(0.3, 0.7), right_focused });
        }
    }

    // ------------------------------------------------------------ the plan

    /// What niri must do so the screen matches the template. Empty when it
    /// already does, so calling this after every event settles quickly.
    pub fn plan(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        let (Some(stage_ws), Some(_)) = (self.stage_ws, self.shelf_ws) else { return out };
        let wanted = self.on_stage();
        let mut moved = false;

        for w in self.windows.values() {
            if w.is_floating {
                continue;
            }
            let here = w.workspace_id == Some(stage_ws);
            if wanted.contains(&w.id) && !here {
                moved = true;
                out.push(Action::MoveWindowToWorkspace {
                    window_id: Some(w.id),
                    reference: WorkspaceReferenceArg::Name(STAGE.into()),
                    focus: false,
                });
            } else if !wanted.contains(&w.id) && here {
                moved = true;
                out.push(Action::MoveWindowToWorkspace {
                    window_id: Some(w.id),
                    reference: WorkspaceReferenceArg::Name(SHELF.into()),
                    focus: false,
                });
            }
        }

        let widths: Vec<(u64, i32)> = match self.stage() {
            Stage::Empty => vec![],
            Stage::One(a) => vec![(a, 100)],
            Stage::Two { left, right, ratio, .. } => {
                let l = (ratio * 100.0).round() as i32;
                vec![(left, l), (right, 100 - l)]
            }
        };
        for (id, pct) in widths {
            if self.widths.get(&id) != Some(&pct) {
                self.widths.insert(id, pct);
                out.push(Action::SetWindowWidth { id: Some(id), change: SizeChange::SetProportion(pct as f64) });
            }
        }

        if let Stage::Two { left, right, .. } = self.stage() {
            let col = |id: u64| {
                self.windows
                    .get(&id)
                    .filter(|w| w.workspace_id == Some(stage_ws))
                    .and_then(|w| w.layout.pos_in_scrolling_layout)
                    .map(|p| p.0)
            };
            if let (Some(l), Some(r)) = (col(left), col(right)) {
                if l > r {
                    moved = true;
                    out.push(Action::FocusWindow { id: left });
                    out.push(Action::MoveColumnToIndex { index: 1 });
                }
            }
        }

        // Give the keys to the right app only when the template changed or
        // windows moved; a dialog that has the keys keeps them.
        let changed = self.last_planned != Some(self.stage()) || moved;
        let dialog_has_keys = self.windows.values().any(|w| w.is_focused && w.is_floating);
        if let Some(f) = self.focused() {
            if changed && !(dialog_has_keys && !moved) {
                // Focusing the left app first puts niri's view at the start,
                // so both apps are fully on screen; then the one with the keys.
                if let Stage::Two { left, .. } = self.stage() {
                    if left != f {
                        out.push(Action::FocusWindow { id: left });
                    }
                }
                out.push(Action::FocusWindow { id: f });
            }
        }
        self.last_planned = Some(self.stage());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use niri_ipc::WindowLayout;

    fn win(id: u64, ws: u64, col: usize) -> Window {
        Window {
            id,
            title: None,
            app_id: Some(format!("app{id}")),
            pid: None,
            workspace_id: Some(ws),
            is_focused: false,
            is_floating: false,
            is_urgent: false,
            layout: WindowLayout {
                pos_in_scrolling_layout: Some((col, 1)),
                tile_size: (100.0, 100.0),
                window_size: (100, 100),
                tile_pos_in_workspace_view: None,
                window_offset_in_tile: (0.0, 0.0),
            },
            focus_timestamp: None,
        }
    }

    fn layout() -> Layout {
        let mut l = Layout::default();
        l.stage_ws = Some(1);
        l.shelf_ws = Some(2);
        l.set_windows(vec![]);
        l
    }

    fn moves(plan: &[Action]) -> Vec<(u64, String)> {
        plan.iter()
            .filter_map(|a| match a {
                Action::MoveWindowToWorkspace { window_id: Some(id), reference: WorkspaceReferenceArg::Name(n), .. } => {
                    Some((*id, n.clone()))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn new_app_replaces_the_one_on_screen() {
        let mut l = layout();
        assert!(l.window_changed(win(1, 1, 1)));
        assert_eq!(l.stage(), Stage::One(1));
        l.plan();
        assert!(l.window_changed(win(2, 1, 2)));
        assert_eq!(l.stage(), Stage::One(2));
        assert_eq!(moves(&l.plan()), vec![(1, SHELF.to_string())]);
    }

    #[test]
    fn beside_makes_two_and_closing_returns_to_one() {
        let mut l = layout();
        l.window_changed(win(1, 1, 1));
        l.beside_next = true;
        l.window_changed(win(2, 1, 2));
        assert!(matches!(l.stage(), Stage::Two { left: 1, right: 2, .. }));
        let plan = l.plan();
        assert!(moves(&plan).is_empty());
        assert!(plan.iter().any(|a| matches!(a, Action::SetWindowWidth { id: Some(1), change: SizeChange::SetProportion(p) } if *p == 60.0)));
        l.window_closed(2);
        assert_eq!(l.stage(), Stage::One(1));
        assert!(l.plan().iter().any(|a| matches!(a, Action::SetWindowWidth { id: Some(1), change: SizeChange::SetProportion(p) } if *p == 100.0)));
        l.window_closed(1);
        assert_eq!(l.stage(), Stage::Empty);
    }

    #[test]
    fn startup_pair_and_order() {
        let mut l = layout();
        l.startup_left = 2;
        l.window_changed(win(1, 1, 2));
        l.window_changed(win(2, 1, 1));
        assert!(matches!(l.stage(), Stage::Two { left: 1, right: 2, .. }));
        let plan = l.plan();
        // window 1 must become the left column
        assert!(plan.iter().any(|a| matches!(a, Action::MoveColumnToIndex { index: 1 })));
    }

    #[test]
    fn plan_settles() {
        let mut l = layout();
        let mut w = win(1, 1, 1);
        w.is_focused = true;
        l.window_changed(w);
        assert!(!l.plan().is_empty());
        assert!(l.plan().is_empty());
    }

    #[test]
    fn resize_and_swap() {
        let mut l = layout();
        l.window_changed(win(1, 1, 1));
        l.beside_next = true;
        l.window_changed(win(2, 1, 2)); // right has the keys
        l.resize(true);
        assert!(matches!(l.stage(), Stage::Two { ratio, .. } if (ratio - 0.5).abs() < 1e-6));
        l.swap();
        assert!(matches!(l.stage(), Stage::Two { left: 2, right: 1, right_focused: false, .. }));
        assert!(l.toggle_two());
        assert_eq!(l.stage(), Stage::One(2));
        assert!(l.toggle_two());
        assert!(matches!(l.stage(), Stage::Two { left: 2, right: 1, .. }));
    }

    #[test]
    fn swap_settles_once_niri_reports_new_columns() {
        let mut l = layout();
        l.window_changed(win(1, 1, 1));
        l.beside_next = true;
        l.window_changed(win(2, 1, 2));
        l.plan();
        l.swap(); // now 2 must be left
        let p = l.plan();
        assert!(p.iter().any(|a| matches!(a, Action::MoveColumnToIndex { index: 1 })));
        // niri moved the columns and focused the right one
        l.layouts_changed(vec![(2, win(2, 1, 1).layout), (1, win(1, 1, 2).layout)]);
        l.set_focused(l.focused());
        assert!(l.plan().is_empty());
        assert!(l.plan().is_empty());
    }

    #[test]
    fn a_dialog_keeps_the_keys() {
        let mut l = layout();
        l.window_changed(win(1, 1, 1));
        l.set_focused(Some(1));
        l.plan();
        let mut d = win(9, 1, 1);
        d.is_floating = true;
        assert!(!l.window_changed(d));
        l.set_focused(Some(9));
        assert!(l.plan().is_empty());
    }

    #[test]
    fn shelf_app_taking_focus_comes_forward() {
        let mut l = layout();
        l.window_changed(win(1, 1, 1));
        l.window_changed(win(2, 1, 2));
        assert_eq!(l.stage(), Stage::One(2));
        l.focus_changed(Some(1));
        assert_eq!(l.stage(), Stage::One(1));
    }
}
