//! ProArt core: DDC/CI protocol, per-OS display backends, the ProArt OSD
//! feature registry, learning of ASUS-specific opcodes, settings and the
//! display manager shared by the CLI and the GUI.

pub mod automation;
pub mod autostart;
pub mod backend;
pub mod config;
pub mod ddc;
pub mod features;
pub mod learn;
pub mod manager;
pub mod tint;

pub use backend::{Backend, DisplayInfo, Session};
pub use config::Settings;
pub use ddc::VcpReply;
pub use manager::{Event, Manager};

pub const APP_NAME: &str = "ProArt";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
