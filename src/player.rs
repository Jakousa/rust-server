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

const MAX_PLAYER_NAME_CHARS: usize = 32;

fn validate_player_name(name: &str) -> Result<&str, Status> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(Status::invalid_argument("Player name cannot be empty or whitespace"));
    }
    if trimmed.chars().count() > MAX_PLAYER_NAME_CHARS {
        return Err(Status::invalid_argument(format!(
            "Player name cannot exceed {} characters",
            MAX_PLAYER_NAME_CHARS
        )));
    }
    Ok(trimmed)
}

#[tonic::async_trait]
impl Player for MyPlayer {
    async fn create(
        &self,
        request: Request<CreateRequest>,
    ) -> Result<Response<CreateReply>, Status> {
        let name = request.into_inner().name;
        validate_player_name(&name)?;

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
        validate_player_name(&name)?;

        let x = req.x;
        let y = req.y;
        let z = req.z;
        println!(
            "Got a request to update player {:?}",
            name
        );

        {
            let mut players = self.players.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

            let player = players
                .iter_mut()
                .find(|x| x.name == name)
                .ok_or_else(|| Status::not_found("Player not found!"))?;

            player.position = (x, y, z);
        }

        let reply = UpdateReply {
            message: format!("Player updated successfully!, x: {}, y: {}, z: {}", x, y, z),
        };
        Ok(Response::new(reply))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_player_name_valid() {
        assert!(validate_player_name("validname").is_ok());
        assert!(validate_player_name("   trimmedname   ").is_ok());
        assert_eq!(validate_player_name("   trimmedname   ").unwrap(), "trimmedname");
        assert!(validate_player_name("一二三四五").is_ok()); // 5 characters, > 5 bytes
    }

    #[test]
    fn test_validate_player_name_empty_or_whitespace() {
        assert_eq!(
            validate_player_name("").unwrap_err().code(),
            tonic::Code::InvalidArgument
        );
        assert_eq!(
            validate_player_name("   ").unwrap_err().code(),
            tonic::Code::InvalidArgument
        );
        assert_eq!(
            validate_player_name("\n\t").unwrap_err().code(),
            tonic::Code::InvalidArgument
        );
    }

    #[test]
    fn test_validate_player_name_too_long() {
        let long_name = "a".repeat(33);
        assert_eq!(
            validate_player_name(&long_name).unwrap_err().code(),
            tonic::Code::InvalidArgument
        );

        let exactly_32 = "a".repeat(32);
        assert!(validate_player_name(&exactly_32).is_ok());

        let long_unicode = "一".repeat(33);
        assert_eq!(
            validate_player_name(&long_unicode).unwrap_err().code(),
            tonic::Code::InvalidArgument
        );
    }
}
