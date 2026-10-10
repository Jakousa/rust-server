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
    use uuid::Uuid;

    #[test]
    fn test_entity_new_with_id() {
        let id = Uuid::new_v4();
        let entity = Entity::new(Some(id), (1, 2, 3));
        assert_eq!(entity.id, id);
        assert_eq!(entity.position, (1, 2, 3));
    }

    #[test]
    fn test_entity_new_without_id() {
        let entity = Entity::new(None, (4, 5, 6));
        assert!(!entity.id.is_nil());
        assert_eq!(entity.position, (4, 5, 6));
    }

    #[test]
    fn test_entity_new_without_id_generates_unique_ids() {
        let entity1 = Entity::new(None, (0, 0, 0));
        let entity2 = Entity::new(None, (0, 0, 0));
        assert_ne!(entity1.id, entity2.id);
    }

    #[test]
    fn test_entity_debug() {
        let id = Uuid::new_v4();
        let entity = Entity::new(Some(id), (10, 20, 30));
        let debug_str = format!("{:?}", entity);
        assert!(debug_str.contains("Entity"));
        assert!(debug_str.contains(&id.to_string()));
        assert!(debug_str.contains("position: (10, 20, 30)"));
    }
}