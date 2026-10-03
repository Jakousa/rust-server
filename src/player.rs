use tonic::{Request, Response, Status};
use std::sync::{Arc, Mutex};

use crate::proto::player_server::Player;
use crate::proto::{CreateReply, CreateRequest, UpdateReply, UpdateRequest};

#[derive(PartialEq)]
struct PlayerEntity {
    pub name: String,
    pub position: (i32, i32, i32),
}

#[derive(Default, Clone)]
pub struct MyPlayer {
    players: Arc<Mutex<Vec<PlayerEntity>>>,
}

#[tonic::async_trait]
impl Player for MyPlayer {
    async fn create(
        &self,
        request: Request<CreateRequest>,
    ) -> Result<Response<CreateReply>, Status> {
        let name = request.into_inner().name;

        if name.is_empty() {
            return Err(Status::invalid_argument("Player name is empty"));
        }
        if name.len() > 32 {
            return Err(Status::invalid_argument("Player name is too long"));
        }

        println!(
            "Got a request to create player {:?}",
            name
        );

        {
            let mut players = self.players.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

            if players.iter().any(|x| x.name == name) {
                return Err(Status::already_exists("Player already exists!"));
            }

            players.push(PlayerEntity {
                name: name.clone(),
                position: (0, 0, 0),
            });
        }

        let reply = CreateReply {
            message: format!("Player created successfully!"),
        };
        Ok(Response::new(reply))
    }
    async fn update(
        &self,
        request: Request<UpdateRequest>,
    ) -> Result<Response<UpdateReply>, Status> {
        let req = request.into_inner();
        let name = req.name;

        if name.is_empty() {
            return Err(Status::invalid_argument("Player name is empty"));
        }
        if name.len() > 32 {
            return Err(Status::invalid_argument("Player name is too long"));
        }

        let x = req.x;
        let y = req.y;
        let z = req.z;
        println!(
            "Got a request to update player {:?}",
            name
        );

        {
            let mut players = self.players.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

            let player = players.iter_mut().find(|x| x.name == name);

            if player.is_none() {
                return Err(Status::not_found("Player not found!"));
            }

            let player = player.unwrap();
            player.position = (x, y, z);
        }

        let reply = UpdateReply {
            message: format!("Player updated successfully!, x: {}, y: {}, z: {}", x, y, z),
        };
        Ok(Response::new(reply))
    }
}
