pub mod player;
pub mod world;
pub mod entity;
pub mod slice;
pub mod proto {
    tonic::include_proto!("game");

    pub(crate) const FILE_DESCRIPTOR_SET: &[u8] =
        tonic::include_file_descriptor_set!("game_descriptor");
}