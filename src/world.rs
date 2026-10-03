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
        self.slices.iter().find(|slice| slice.get_id() == id)
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
    use uuid::Uuid;

    #[test]
    fn test_world_new_empty() {
        let world = World::new(vec![]);
        assert_eq!(world.slices.len(), 0);
        assert_eq!(world.slice_neighbours.len(), 0);
    }

    #[test]
    fn test_world_new_single_slice() {
        let slice = Slice::new(None, (10, 10, 10), (0, 0, 0));
        let slice_id = slice.get_id();
        let world = World::new(vec![slice]);
        assert_eq!(world.slices.len(), 1);
        assert_eq!(world.slice_neighbours.len(), 1);
        assert_eq!(world.slice_neighbours.get(&slice_id).unwrap().len(), 1);
        assert_eq!(world.slice_neighbours.get(&slice_id).unwrap()[0], slice_id);
    }

    #[test]
    fn test_world_new_multiple_slices() {
        let slice1 = Slice::new(None, (10, 10, 10), (0, 0, 0));
        let slice2 = Slice::new(None, (10, 10, 10), (10, 0, 0)); // Neighbour to slice1
        let slice3 = Slice::new(None, (10, 10, 10), (100, 100, 100)); // Far away

        let id1 = slice1.get_id();
        let id2 = slice2.get_id();
        let id3 = slice3.get_id();

        let world = World::new(vec![slice1, slice2, slice3]);

        assert_eq!(world.slices.len(), 3);
        assert_eq!(world.slice_neighbours.len(), 3);

        let neighbours1 = world.slice_neighbours.get(&id1).unwrap();
        assert_eq!(neighbours1.len(), 2);
        assert!(neighbours1.contains(&id1));
        assert!(neighbours1.contains(&id2));

        let neighbours2 = world.slice_neighbours.get(&id2).unwrap();
        assert_eq!(neighbours2.len(), 2);
        assert!(neighbours2.contains(&id2));
        assert!(neighbours2.contains(&id1));

        let neighbours3 = world.slice_neighbours.get(&id3).unwrap();
        assert_eq!(neighbours3.len(), 1);
        assert!(neighbours3.contains(&id3));
    }
}
