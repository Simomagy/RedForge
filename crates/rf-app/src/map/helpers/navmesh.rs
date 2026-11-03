//! "Navmesh": the navigation meshes (ynv) around the camera.

use super::shapes::Shapes;
use super::{Collect, HelperPart};

#[derive(Default)]
pub struct Navmesh {}

impl Navmesh {
    pub fn collect(&mut self, _c: &Collect, _shapes: &mut Shapes, _out: &mut Vec<HelperPart>) {}
}
