//! yggdrasil-player's libVLC layer as a library, so other Yggdrasil apps play video with the same code (Huginn plays inside its own
//! window with it). The `vlc` module and the stream `relay` (for servers that hand out a stream in pieces): no window, no D-Bus.

pub mod relay;
pub mod vlc;
