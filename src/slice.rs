use uuid::Uuid;
use crate::entity::Entity;
pub struct Slice {
    id: Uuid,
    size: (i32, i32, i32),
    position: (i32, i32, i32),

    entities: Vec<Entity>,
}

impl Slice {
    pub fn new(id: Option<Uuid>, size: (i32, i32, i32), position: (i32, i32, i32)) -> Self {
        Slice {
            id: id.unwrap_or(Uuid::new_v4()),
            size,
            position,
            entities: Vec::new(),
        }
    }

    pub fn get_id(&self) -> Uuid {
        self.id
    }

    pub fn add_entity(&mut self, entity: Entity) {
        self.entities.push(entity);
    }

    pub fn remove_entity(&mut self, entity_id: Uuid) {
        self.entities.retain(|entity| entity.id != entity_id);
    }

    pub fn get_entities(&self) -> &Vec<Entity> {
        &self.entities
    }

    // If two slices are near to each other, they are considered neighbors
    pub fn near_to(&self, other: &Slice) -> bool {
        let (x1, y1, z1) = self.position;
        let (s1x, s1y, s1z) = self.size;
        let (x2, y2, z2) = other.position;
        let (s2x, s2y, s2z) = other.size;

        let x = x1 == x2 || x2 == x1 + s1x || x1 == x2 + s2x;
        let y = y1 == y2 || y2 == y1 + s1y || y1 == y2 + s2y;
        let z = z1 == z2 || z2 == z1 + s1z || z1 == z2 + s2z;

        return (x && y) || (x && z) || (y && z);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::Entity;
    use uuid::Uuid;

    #[test]
    fn test_add_entity() {
        let mut slice = Slice::new(None, (10, 10, 10), (0, 0, 0));
        let entity_id = Uuid::new_v4();
        let entity = Entity::new(Some(entity_id), (5, 5, 5));

        slice.add_entity(entity);

        assert_eq!(slice.get_entities().len(), 1);
        assert_eq!(slice.get_entities()[0].id, entity_id);
    }

    #[test]
    fn test_add_multiple_entities() {
        let mut slice = Slice::new(None, (10, 10, 10), (0, 0, 0));
        let entity1_id = Uuid::new_v4();
        let entity1 = Entity::new(Some(entity1_id), (1, 1, 1));

        let entity2_id = Uuid::new_v4();
        let entity2 = Entity::new(Some(entity2_id), (2, 2, 2));

        slice.add_entity(entity1);
        slice.add_entity(entity2);

        let entities = slice.get_entities();
        assert_eq!(entities.len(), 2);
        assert_eq!(entities[0].id, entity1_id);
        assert_eq!(entities[1].id, entity2_id);
    }
}
