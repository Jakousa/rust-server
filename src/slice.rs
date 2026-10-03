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

    fn create_slice(position: (i32, i32, i32), size: (i32, i32, i32)) -> Slice {
        Slice::new(None, size, position)
    }

    #[test]
    fn test_near_to_identical_position() {
        let s1 = create_slice((0, 0, 0), (10, 10, 10));
        let s2 = create_slice((0, 0, 0), (10, 10, 10));
        assert!(s1.near_to(&s2), "Slices with identical positions should be considered near");
    }

    #[test]
    fn test_near_to_adjacent_on_two_axes() {
        let s1 = create_slice((0, 0, 0), (10, 10, 10));
        let s2 = create_slice((10, 0, 0), (10, 10, 10));
        assert!(s1.near_to(&s2), "Slices touching on X boundary and identical on Y/Z should be near");
    }

    #[test]
    fn test_near_to_adjacent_on_one_axis() {
        let s1 = create_slice((0, 0, 0), (10, 10, 10));
        let s2 = create_slice((10, 10, 0), (10, 10, 10));
        assert!(s1.near_to(&s2), "Slices touching on X and Y and identical on Z should be near");
    }

    #[test]
    fn test_near_to_completely_separate_but_aligned() {
        let s1 = create_slice((0, 0, 0), (10, 10, 10));
        let s2 = create_slice((100, 0, 0), (10, 10, 10));
        assert!(s1.near_to(&s2), "Current logic considers slices near if they align on 2 axes, regardless of distance on the 3rd");
    }

    #[test]
    fn test_near_to_completely_separate_unaligned() {
        let s1 = create_slice((0, 0, 0), (10, 10, 10));
        let s2 = create_slice((100, 100, 0), (10, 10, 10));
        assert!(!s1.near_to(&s2), "Slices completely separate and unaligned on 2 axes should not be near");
    }

    #[test]
    fn test_near_to_corner_touch() {
        let s1 = create_slice((0, 0, 0), (10, 10, 10));
        let s2 = create_slice((10, 10, 10), (10, 10, 10));
        assert!(s1.near_to(&s2), "Slices touching at a corner should be near");
    }
}
