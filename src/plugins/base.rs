use crate::api::models::Track;
use async_trait::async_trait;

#[async_trait]
pub trait Plugin: Send + Sync {
    async fn search(&self, query: &str) -> Vec<Track>;
}

pub type PluginCreate = unsafe fn() -> *mut dyn Plugin;
