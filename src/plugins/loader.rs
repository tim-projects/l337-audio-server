use crate::plugins::base::Plugin;
use libloading::{Library, Symbol};
use std::path::Path;
use std::sync::Arc;
use walkdir::WalkDir;

pub struct PluginLoader {
    pub plugins: Vec<Arc<dyn Plugin>>,
}

impl PluginLoader {
    pub fn load_from_dir(dir: &str) -> Self {
        let mut plugins = Vec::new();
        for entry in WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            if is_plugin_file(path) {
                unsafe {
                    if let Ok(lib) = Library::new(path) {
                        if let Ok(constructor) = lib.get::<crate::plugins::base::PluginCreate>(b"_create_plugin") {
                            let plugin_ptr = constructor();
                            let plugin = Box::from_raw(plugin_ptr);
                            plugins.push(Arc::from(plugin));
                            // We need to keep the library open, this is a simplified example
                            std::mem::forget(lib); 
                        }
                    }
                }
            }
        }
        Self { plugins }
    }
}

fn is_plugin_file(path: &Path) -> bool {
    #[cfg(target_os = "windows")]
    return path.extension().map_or(false, |ext| ext == "dll");
    #[cfg(target_os = "macos")]
    return path.extension().map_or(false, |ext| ext == "dylib");
    #[cfg(target_os = "linux")]
    return path.extension().map_or(false, |ext| ext == "so");
}
