use uuid::Uuid;

#[derive(Debug)]
pub struct Entity {
    pub id: Uuid,
    pub position: (i32, i32, i32),
}

impl Entity {
    pub fn new(id: Option<Uuid>, position: (i32, i32, i32)) -> Self {
        Entity {
            id: id.unwrap_or(Uuid::new_v4()),
            position,
        }
    }
}