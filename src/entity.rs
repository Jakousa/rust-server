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
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entity_new_with_id() {
        let expected_id = Uuid::new_v4();
        let position = (10, 20, 30);
        let entity = Entity::new(Some(expected_id), position);

        assert_eq!(entity.id, expected_id);
        assert_eq!(entity.position, position);
    }

    #[test]
    fn test_entity_new_without_id() {
        let position = (10, 20, 30);
        let entity = Entity::new(None, position);

        assert!(!entity.id.is_nil(), "Generated UUID should not be nil");
        assert_eq!(entity.position, position);
    }
}
