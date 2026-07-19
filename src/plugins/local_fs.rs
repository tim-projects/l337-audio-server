use async_trait::async_trait;
use crate::plugins::base::SearchPlugin;
use crate::api::models::Track;
use walkdir::WalkDir;
use std::path::Path;

pub struct LocalFileSystemPlugin {
    pub base_path: String,
}

#[async_trait]
impl SearchPlugin for LocalFileSystemPlugin {
    async fn search(&self, query: &str) -> Vec<Track> {
        let mut results = Vec::new();
        for entry in WalkDir::new(&self.base_path).into_iter().filter_map(|e| e.ok()) {
            if entry.file_type().is_file() {
                let path = entry.path();
                if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                    if filename.contains(query) {
                        results.push(Track {
                            track_id: path.to_string_lossy().to_string(),
                            stream_url: format!("file://{}", path.to_string_lossy()),
                            title: Some(filename.to_string()),
                            artist: Some("Local Filesystem".to_string()),
                            duration: None,
                        });
                    }
                }
            }
        }
        results
    }
}
