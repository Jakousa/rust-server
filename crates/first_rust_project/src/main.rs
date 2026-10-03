#![allow(dead_code)]
use tonic::transport::Server;

use proto::player_server::PlayerServer;
use world::World;

mod entity;
mod player;
mod slice;
mod world;

mod proto {
    tonic::include_proto!("game");

    pub(crate) const FILE_DESCRIPTOR_SET: &[u8] =
        tonic::include_file_descriptor_set!("game_descriptor");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(proto::FILE_DESCRIPTOR_SET)
        .build_v1alpha()
        .unwrap();

    let addr = "[::1]:50051".parse().unwrap();

    println!("GameServer listening on {}", addr);

    let slices = vec![
        slice::Slice::new(None, (10, 10, 10), (0, 0, 0)),
        slice::Slice::new(None, (10, 10, 10), (10, 0, 0)),
        slice::Slice::new(None, (10, 10, 10), (0, 10, 0)),
        slice::Slice::new(None, (10, 10, 10), (10, 10, 0)),
    ];
    let _world = World::new(slices);

    Server::builder()
        .add_service(reflection_service)
        .add_service(PlayerServer::new(player::MyPlayer::default()))
        .serve(addr)
        .await?;

    Ok(())
}
