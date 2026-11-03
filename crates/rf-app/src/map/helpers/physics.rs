//! "Physics": the world's static collisions (ybn) and the entities' own.

use super::shapes::Shapes;
use super::{Collect, HelperPart};

#[derive(Default)]
pub struct Physics {}

impl Physics {
    pub fn collect(&mut self, _c: &Collect, _shapes: &mut Shapes, _out: &mut Vec<HelperPart>) {}
}
