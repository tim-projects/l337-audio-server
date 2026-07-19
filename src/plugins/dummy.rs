use async_trait::async_trait;
use crate::plugins::base::{SearchPlugin, StreamPlugin};
use crate::api::models::Track;

pub struct DummyPlugin;

#[async_trait]
impl SearchPlugin for DummyPlugin {
    async fn search(&self, query: &str) -> Vec<Track> {
        vec![Track {
            track_id: "dummy-1".into(),
            stream_url: "http://example.com/dummy.mp3".into(),
            title: Some(format!("Dummy Result for {}", query)),
            artist: Some("Dummy Artist".into()),
            duration: Some(120),
        }]
    }
}

#[async_trait]
impl StreamPlugin for DummyPlugin {
    async fn resolve_stream_url(&self, _track_id: &str) -> Option<String> {
        Some("http://example.com/dummy.mp3".into())
    }
}
