//! Grainy's application in egui: the window of the Python Grainy (luma/app.py `MainWindow`). See
//! `ARCHITECTURE.md`: commands ([`commands`]), panels ([`panels`]), canvas tools ([`tools`]), dialogs
//! ([`dialogs`]), background jobs ([`jobs`]) and the document model ([`document`]: an in-memory
//! [`session::Session`] or a library on disk, [`document::catalog::CatalogDocument`]) all work on the
//! [`context::AppContext`]. The background [`worker::Worker`] develops pictures with `grainy-engine`.

pub mod adjustments;
pub mod ai;
pub mod app;
mod basic_preview;
pub mod commands;
pub mod context;
pub mod dialogs;
pub mod display;
pub mod document;
pub mod export;
pub mod folder_watch;
pub mod folders;
pub mod fonts;
pub mod gpu_worker;
pub mod hdr;
pub mod histogram;
pub mod imagespace;
pub mod jobs;
mod library;
pub mod logger;
pub mod masks;
mod menus;
pub mod menus_photo;
mod monitor_preview;
#[cfg(any(windows, target_os = "macos"))]
#[allow(unsafe_code)]
mod native_hdr;
pub mod network;
pub mod panels;
pub mod paths;
pub mod render;
pub mod session;
pub mod session_state;
pub mod shortcuts;
pub mod smoke;
pub mod startup;
pub mod state;
pub mod system_open;
pub mod text;
pub mod theme;
pub mod thumbjpeg;
pub mod tools;
pub mod video;
mod view;
pub mod views;
pub mod widgets;
pub mod window_state;
pub mod worker;

pub use app::GrainyApp;
pub use context::{AppContext, Mode, Zoom};
pub use menus::TITLES as MENU_TITLES;
pub use panels::basic::{drt_look, DRT_LOOKS};
pub use panels::crop::centred_crop;
