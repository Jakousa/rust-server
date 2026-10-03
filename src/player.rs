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
        println!(
            "Got a request to create player {:?}",
            name
        );

        {
            let mut players = self.players.lock().map_err(|_| Status::internal("Internal error: player lock poisoned"))?;

            if players.iter().any(|x| x.name == name) {
                return Err(Status::already_exists("Player already exists!"));
            }

            players.push(PlayerEntity {
                name,
                position: (0, 0, 0),
            });
        }

        let reply = CreateReply {
            message: "Player created successfully!".to_string(),
        };
        Ok(Response::new(reply))
    }
    async fn update(
        &self,
        request: Request<UpdateRequest>,
    ) -> Result<Response<UpdateReply>, Status> {
        let req = request.into_inner();
        let name = req.name;
        let x = req.x;
        let y = req.y;
        let z = req.z;
        println!(
            "Got a request to update player {:?}",
            name
        );

        {
            let mut players = self.players.lock().map_err(|_| Status::internal("Internal error: player lock poisoned"))?;

            let player = players.iter_mut().find(|x| x.name == name)
                .ok_or_else(|| Status::not_found("Player not found!"))?;

            player.position = (x, y, z);
        }

        let reply = UpdateReply {
            message: format!("Player updated successfully: x: {x}, y: {y}, z: {z}"),
        };
        Ok(Response::new(reply))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_player_success() {
        let player_service = MyPlayer::default();
        let request = Request::new(CreateRequest {
            name: "test_player".to_string(),
        });

        let response = player_service.create(request).await;
        assert!(response.is_ok());
        let response_inner = response.unwrap().into_inner();
        assert_eq!(response_inner.message, "Player created successfully!");

        // Verify state
        let players = player_service.players.lock().unwrap();
        assert_eq!(players.len(), 1);
        assert_eq!(players[0].name, "test_player");
    }

    #[tokio::test]
    async fn test_create_player_duplicate() {
        let player_service = MyPlayer::default();

        // Add initial player
        let request1 = Request::new(CreateRequest {
            name: "test_player".to_string(),
        });
        let _ = player_service.create(request1).await;

        // Try adding duplicate
        let request2 = Request::new(CreateRequest {
            name: "test_player".to_string(),
        });

        let response = player_service.create(request2).await;
        assert!(response.is_err());
        let status = response.unwrap_err();
        assert_eq!(status.code(), tonic::Code::AlreadyExists);
    }

    #[tokio::test]
    async fn test_update_player_success() {
        let player_service = MyPlayer::default();

        // Setup initial player
        let request_create = Request::new(CreateRequest {
            name: "test_player".to_string(),
        });
        let _ = player_service.create(request_create).await;

        let request_update = Request::new(UpdateRequest {
            name: "test_player".to_string(),
            x: 10,
            y: 20,
            z: 30,
        });

        let response = player_service.update(request_update).await;
        assert!(response.is_ok());
        let response_inner = response.unwrap().into_inner();
        assert_eq!(response_inner.message, "Player updated successfully: x: 10, y: 20, z: 30");

        // Verify updated state
        let players = player_service.players.lock().unwrap();
        assert_eq!(players[0].position, (10, 20, 30));
    }

    #[tokio::test]
    async fn test_update_player_not_found() {
        let player_service = MyPlayer::default();

        let request = Request::new(UpdateRequest {
            name: "unknown_player".to_string(),
            x: 1,
            y: 2,
            z: 3,
        });

        let response = player_service.update(request).await;
        assert!(response.is_err());
        let status = response.unwrap_err();
        assert_eq!(status.code(), tonic::Code::NotFound);
    }
}
