//! "Scenario": the scenario points of the regions around the camera.

use super::shapes::Shapes;
use super::{Collect, HelperPart};

#[derive(Default)]
pub struct Scenario {}

impl Scenario {
    pub fn collect(&mut self, _c: &Collect, _shapes: &mut Shapes, _out: &mut Vec<HelperPart>) {}
}
