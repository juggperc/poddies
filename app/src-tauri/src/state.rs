use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use poddies_plugin_host::{LoadReport, PluginHost};

use crate::services::SharedLibrary;

pub struct AppState {
    pub library: Arc<SharedLibrary>,
    pub plugins: Mutex<PluginHost>,
    pub plugin_reports: Vec<LoadReport>,
    pub plugins_dir: PathBuf,
    pub data_dir: PathBuf,
}

impl AppState {
    pub fn library_path(&self) -> PathBuf {
        self.data_dir.join("library.json")
    }
}

/// Where plugins are loaded from, in priority order:
///
/// 1. `PODDIES_PLUGINS_DIR` — development and tests.
/// 2. `plugins/` next to the executable — the portable layout.
/// 3. `%APPDATA%\Poddies\plugins` — where users drop new plugins.
pub fn plugin_search_paths(exe_dir: Option<&Path>, data_dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Ok(from_env) = std::env::var("PODDIES_PLUGINS_DIR") {
        if !from_env.trim().is_empty() {
            paths.push(PathBuf::from(from_env));
        }
    }

    if let Some(exe_dir) = exe_dir {
        paths.push(exe_dir.join("plugins"));
    }

    paths.push(data_dir.join("plugins"));
    paths
}
