//! "Ambient Mask Volume": the ambient mask volumes.

use super::shapes::Shapes;
use super::{Collect, HelperPart};

#[derive(Default)]
pub struct AmbientMasks {}

impl AmbientMasks {
    pub fn collect(&mut self, _c: &Collect, _shapes: &mut Shapes, _out: &mut Vec<HelperPart>) {}
}
