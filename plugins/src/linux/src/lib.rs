use l337_audio_server::api::models::Track;
use l337_audio_server::plugins::base::Plugin;
use async_trait::async_trait;

pub struct LinuxFsPlugin;

#[async_trait]
impl Plugin for LinuxFsPlugin {
    async fn search(&self, query: &str) -> Vec<Track> {
        // Implementation for Linux...
        vec![]
    }
}

#[no_mangle]
pub extern "C" fn _create_plugin() -> *mut dyn Plugin {
    Box::into_raw(Box::new(LinuxFsPlugin))
}
