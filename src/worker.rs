//! All DDC/CI traffic runs on one background thread. DDC calls take tens of milliseconds each,
//! so the UI never waits on them: it sends commands and gets a message back when values change.

use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::ddc::{self, PhysicalMonitor, FEATURES, VCP_INPUT};

#[derive(Clone, Copy, Debug)]
pub struct FeatureValue {
    pub cur: u32,
    pub max: u32,
}

#[derive(Clone, Debug)]
pub struct MonitorState {
    pub name: String,
    /// Indexed like `ddc::FEATURES`. `None` means the monitor does not support it.
    pub features: [Option<FeatureValue>; 3],
    pub input: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub monitors: Vec<MonitorState>,
    pub loaded: bool,
}

pub type Shared = Arc<Mutex<Snapshot>>;

#[derive(Clone, Copy, Debug)]
pub enum Cmd {
    /// Re-detect monitors and read every value.
    Rescan,
    /// Read every value again for the monitors already known.
    Reread,
    Set { mon: usize, feature: usize, value: u32 },
    SetAllPercent { feature: usize, pct: u32 },
    AdjustAllPercent { feature: usize, delta: i32 },
    Raw { mon: usize, code: u8, value: u32 },
    Quit,
}

pub fn percent(fv: &FeatureValue) -> u32 {
    if fv.max == 0 {
        return 0;
    }
    ((fv.cur.min(fv.max) as u64 * 100 + fv.max as u64 / 2) / fv.max as u64) as u32
}

pub fn from_percent(pct: u32, max: u32) -> u32 {
    ((pct.min(100) as u64 * max as u64 + 50) / 100) as u32
}

pub fn spawn(shared: Shared, notify_hwnd: isize, notify_msg: u32) -> Sender<Cmd> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("ddc".into())
        .spawn(move || run(rx, shared, notify_hwnd, notify_msg))
        .expect("failed to start the DDC worker thread");
    tx
}

fn run(rx: Receiver<Cmd>, shared: Shared, notify_hwnd: isize, notify_msg: u32) {
    let mut monitors: Vec<PhysicalMonitor> = Vec::new();

    while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        while let Ok(cmd) = rx.try_recv() {
            batch.push(cmd);
        }
        if batch.iter().any(|c| matches!(c, Cmd::Quit)) {
            break;
        }

        let mut changed = false;
        for cmd in coalesce(batch) {
            match cmd {
                Cmd::Rescan => {
                    monitors.clear();
                    monitors = ddc::enumerate();
                    let states = read_all(&monitors);
                    let mut snap = shared.lock().unwrap();
                    snap.monitors = states;
                    snap.loaded = true;
                    changed = true;
                }
                Cmd::Reread => {
                    let states = read_all(&monitors);
                    let mut snap = shared.lock().unwrap();
                    snap.monitors = states;
                    snap.loaded = true;
                    changed = true;
                }
                Cmd::Set { mon, feature, value } => {
                    if let Some(m) = monitors.get(mon) {
                        m.set(FEATURES[feature].0, value);
                        store(&shared, mon, feature, value);
                        changed = true;
                    }
                }
                Cmd::SetAllPercent { feature, pct } => {
                    for (mon, fv) in feature_values(&shared, feature) {
                        if let Some(m) = monitors.get(mon) {
                            let value = from_percent(pct, fv.max);
                            m.set(FEATURES[feature].0, value);
                            store(&shared, mon, feature, value);
                            changed = true;
                        }
                    }
                }
                Cmd::AdjustAllPercent { feature, delta } => {
                    for (mon, fv) in feature_values(&shared, feature) {
                        if let Some(m) = monitors.get(mon) {
                            let pct = (percent(&fv) as i32 + delta).clamp(0, 100) as u32;
                            let value = from_percent(pct, fv.max);
                            m.set(FEATURES[feature].0, value);
                            store(&shared, mon, feature, value);
                            changed = true;
                        }
                    }
                }
                Cmd::Raw { mon, code, value } => {
                    if let Some(m) = monitors.get(mon) {
                        if m.set(code, value) && code == VCP_INPUT {
                            if let Some(state) = shared.lock().unwrap().monitors.get_mut(mon) {
                                state.input = Some(value);
                            }
                            changed = true;
                        }
                    }
                }
                Cmd::Quit => {}
            }
        }

        if changed {
            unsafe {
                PostMessageW(notify_hwnd as HWND, notify_msg, 0, 0);
            }
        }
    }
}

/// Drops slider updates that a later update in the same batch overrides, and sums hotkey steps,
/// so dragging a slider or holding a hotkey never queues up a backlog of slow DDC writes.
fn coalesce(batch: Vec<Cmd>) -> Vec<Cmd> {
    let mut out = Vec::new();
    let mut seen_set = HashSet::new();
    let mut seen_all = HashSet::new();
    let (mut rescan, mut reread) = (false, false);
    let mut adjust = [0i32; FEATURES.len()];

    for cmd in batch.into_iter().rev() {
        match cmd {
            Cmd::Set { mon, feature, .. } => {
                if seen_set.insert((mon, feature)) {
                    out.push(cmd);
                }
            }
            Cmd::SetAllPercent { feature, .. } => {
                if seen_all.insert(feature) {
                    out.push(cmd);
                }
            }
            Cmd::AdjustAllPercent { feature, delta } => adjust[feature] += delta,
            Cmd::Rescan => {
                if !rescan {
                    rescan = true;
                    out.push(cmd);
                }
            }
            Cmd::Reread => {
                if !reread {
                    reread = true;
                    out.push(cmd);
                }
            }
            other => out.push(other),
        }
    }
    out.reverse();
    for (feature, delta) in adjust.into_iter().enumerate() {
        if delta != 0 {
            out.push(Cmd::AdjustAllPercent { feature, delta });
        }
    }
    out
}

fn read_all(monitors: &[PhysicalMonitor]) -> Vec<MonitorState> {
    monitors
        .iter()
        .map(|m| MonitorState {
            name: m.name.clone(),
            features: FEATURES.map(|(code, _)| {
                m.get(code)
                    .filter(|&(_, max)| max > 0)
                    .map(|(cur, max)| FeatureValue { cur: cur.min(max), max })
            }),
            input: m.get(VCP_INPUT).map(|(cur, _)| cur & 0xFF),
        })
        .collect()
}

fn feature_values(shared: &Shared, feature: usize) -> Vec<(usize, FeatureValue)> {
    let snap = shared.lock().unwrap();
    snap.monitors
        .iter()
        .enumerate()
        .filter_map(|(i, m)| m.features[feature].map(|fv| (i, fv)))
        .collect()
}

fn store(shared: &Shared, mon: usize, feature: usize, value: u32) {
    let mut snap = shared.lock().unwrap();
    if let Some(fv) = snap.monitors.get_mut(mon).and_then(|m| m.features[feature].as_mut()) {
        fv.cur = value;
    }
}
