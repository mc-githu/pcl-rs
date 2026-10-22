//! Version installation: everything needed before a version can be launched.

pub mod assets;
pub mod installer;
pub mod natives;

pub use installer::{InstallOptions, InstallPlan, InstalledVersion, Installer};
