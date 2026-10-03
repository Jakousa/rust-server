#![allow(dead_code)]
pub mod entity;
pub mod player;
pub mod slice;
pub mod world;
pub mod proto {
    tonic::include_proto!("game");

    #[allow(dead_code)]
pub(crate) const FILE_DESCRIPTOR_SET: &[u8] =
        tonic::include_file_descriptor_set!("game_descriptor");
}
