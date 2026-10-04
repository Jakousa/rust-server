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

    pub fn get_neighbours(&self, id: Uuid) -> Option<&[Uuid]> {
        self.slice_neighbours.get(&id).map(|v| v.as_slice())
    }

    pub fn get_slice_and_neighbour_entities(&self, id: Uuid) -> Option<Vec<&[Entity]>> {
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
