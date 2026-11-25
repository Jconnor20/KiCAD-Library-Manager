//! Headless GUI tests: the real `App` rendered offscreen by egui_kittest (wgpu), never in a window on the desktop.
//! Everything runs in one sandbox config folder (`KLM_CONFIG_DIR`) with a freshly seeded three-part library,
//! all distributor sources off, so no test touches the real library, keys or network.
//! PNGs go to `target/ui-snapshots/`.

mod bugs;
mod harness;
mod kit;
mod library;
mod other_tabs;
mod project_check;
mod screens;
mod search;
mod settings;
mod shell;
