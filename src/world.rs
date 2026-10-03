use crate::entity::Entity;
use crate::slice::Slice;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Default)]
pub struct World {
    slices: Vec<Slice>,
    slice_neighbours: HashMap<Uuid, Vec<Uuid>>,
}

impl World {
    pub fn new(slices: Vec<Slice>) -> Self {
        let mut slice_neighbours = HashMap::new();

        for slice in slices.iter() {
            let mut neighbours = Vec::new();
            for other_slice in slices.iter() {
                if slice.near_to(other_slice) {
                    neighbours.push(other_slice.get_id());
                }
            }
            slice_neighbours.insert(slice.get_id(), neighbours);
        }

        World {
            slices: slices,
            slice_neighbours: slice_neighbours,
        }
    }

    pub fn get_slice(&self, id: Uuid) -> Option<&Slice> {
        self.slices.iter().find(|slice: &&Slice| slice.get_id() == id)
    }

    pub fn get_neighbours(&self, id: Uuid) -> Option<&Vec<Uuid>> {
        self.slice_neighbours.get(&id)
    }

    pub fn get_slice_and_neighbour_entities(&self, id: Uuid) -> Option<Vec<&Vec<Entity>>> {
        let slice = self.get_slice(id)?;
        let neighbours = self.get_neighbours(id)?;

        let mut entities = Vec::new();
        entities.push(slice.get_entities());

        for neighbour_id in neighbours.iter() {
            let neighbour = self.get_slice(*neighbour_id)?;
            entities.push(neighbour.get_entities());
        }

        Some(entities)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::Entity;
    use crate::slice::Slice;
    use uuid::Uuid;

    #[test]
    fn test_get_slice_and_neighbour_entities_success() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let id3 = Uuid::new_v4();

        let mut slice1 = Slice::new(Some(id1), (10, 10, 10), (0, 0, 0));
        let mut slice2 = Slice::new(Some(id2), (10, 10, 10), (10, 0, 0)); // Neighbor to slice1
        let mut slice3 = Slice::new(Some(id3), (10, 10, 10), (30, 30, 30)); // Not a neighbor to slice1

        let entity1 = Entity::new(None, (1, 1, 1));
        let entity2 = Entity::new(None, (11, 1, 1));
        let entity3 = Entity::new(None, (31, 1, 1));

        let entity1_id = entity1.id;
        let entity2_id = entity2.id;
        let entity3_id = entity3.id;

        slice1.add_entity(entity1);
        slice2.add_entity(entity2);
        slice3.add_entity(entity3);

        let world = World::new(vec![slice1, slice2, slice3]);

        let entities = world.get_slice_and_neighbour_entities(id1).expect("Expected Some entities");

        // We expect 3 lists of entities: slice1 (added directly), and 2 neighbours (slice1 and slice2).
        // Since `near_to` returns true for identical slices, a slice is considered its own neighbor here!
        assert_eq!(entities.len(), 3);

        let has_entity1 = entities.iter().any(|e_list: &&Vec<Entity>| e_list.iter().any(|e| e.id == entity1_id));
        let has_entity2 = entities.iter().any(|e_list: &&Vec<Entity>| e_list.iter().any(|e| e.id == entity2_id));
        let has_entity3 = entities.iter().any(|e_list: &&Vec<Entity>| e_list.iter().any(|e| e.id == entity3_id));

        assert!(has_entity1);
        assert!(has_entity2);
        assert!(!has_entity3);
    }

    #[test]
    fn test_get_slice_and_neighbour_entities_not_found() {
        let world = World::new(vec![]);
        let result = world.get_slice_and_neighbour_entities(Uuid::new_v4());
        assert!(result.is_none());
    }
}
