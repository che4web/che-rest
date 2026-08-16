pub use crate::app_channels::AppChannels as Channels;

pub fn module() -> Channels {
    Channels::new()
}
