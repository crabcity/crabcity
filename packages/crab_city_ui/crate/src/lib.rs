//! Embedded SvelteKit UI assets for Crab City.
//! v5 - with assets
//!
//! This crate embeds the built SvelteKit SPA at compile time.
//!
//! The folder path is set via CRAB_CITY_UI_PATH env var:
//! - Cargo builds: CRAB_CITY_UI_PATH=../build cargo build
//! - Bazel builds: set via rustc_env in BUILD.bazel

pub use rust_embed::Embed;

/// Embedded UI assets from the SvelteKit build.
#[derive(Embed)]
#[folder = "$CRAB_CITY_UI_PATH"]
pub struct Assets;
