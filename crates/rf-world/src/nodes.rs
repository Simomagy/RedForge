//! The map nodes (one per ymap, from the map data cache), the C#'s
//! `StreamingBVH` over their streaming boxes, the map data cull boxes and the
//! map states file.

use rf_formats::formats::databag::{DataBag, Value};
use rf_formats::hash::joaat;
use rf_formats::rpf8::FileRef;
use rf_game::MapDataCacheEntry;

use crate::math::Aabb;

/// `RDR2MapNode`: a ymap the world can stream.
#[derive(Clone, Debug)]
pub(crate) struct Node {
    pub name: u32,
    pub parent: u32,
    /// The parent's index, when there's a node for it.
    pub parent_index: Option<u32>,
    /// `None` for a ymap made in the editor.
    pub file: Option<FileRef>,
    /// The file's name, lowercase, without folder or extension.
    pub file_name: String,
    pub streaming: Aabb,
    /// Off for ymaps the map states disable (and their parents, as the C#'s
    /// `EnableIPL` does), or that turn out to be scripted when loaded.
    pub enabled: bool,
    /// Turned on by `EnableIPL`: shown even when scripted.
    pub requested: bool,
    /// The ymap's `flags & 1`, once loaded.
    pub scripted: Option<bool>,
    /// How many parents up to the root, to load parents first.
    pub depth: u32,
}

impl Node {
    /// From the map data cache: enabled, as all cached nodes are in the C#.
    pub fn cached(file: FileRef, file_name: String, c: &MapDataCacheEntry) -> Self {
        Self {
            name: c.name_hash,
            parent: c.parent_name_hash,
            parent_index: None,
            file: Some(file),
            file_name,
            streaming: Aabb::new(c.streaming_min, c.streaming_max),
            enabled: true,
            requested: false,
            scripted: None,
            depth: 0,
        }
    }

    /// Whether the C# puts it in the streaming BVH: not when its box is the
    /// cache's "none" (-1, -1, -1).
    pub fn has_streaming_box(&self) -> bool {
        self.streaming.min != [-1.0; 3]
    }

    /// Whether to stream it.
    pub fn streamable(&self, hide_scripted: bool) -> bool {
        self.enabled && !(hide_scripted && self.scripted == Some(true) && !self.requested)
    }
}

// ---------------------------------------------------------------------------
// StreamingBVH

/// `StreamingBVH`: a quadtree of boxes by their XY size and centre, queried by
/// point. Items too big for a node's quarters stay in the node.
#[derive(Default, Debug)]
pub(crate) struct Bvh {
    nodes: Vec<BvhNode>,
    boxes: Vec<Aabb>,
}

#[derive(Debug)]
struct BvhNode {
    bounds: Aabb,
    items: Vec<u32>,
    children: [Option<u32>; 4],
}

impl BvhNode {
    fn new() -> Self {
        Self { bounds: Aabb::EMPTY, items: Vec::new(), children: [None; 4] }
    }
}

/// `StreamingBVH.SplitThreshold`.
const SPLIT_THRESHOLD: usize = 10;

impl Bvh {
    /// `Build`: `boxes[i]` is item `i`; only `items` go in.
    pub fn build(boxes: Vec<Aabb>, items: impl IntoIterator<Item = u32>) -> Self {
        let mut bvh = Bvh { nodes: vec![BvhNode::new()], boxes };
        for i in items {
            bvh.add(0, i);
        }
        bvh.try_split(0);
        bvh
    }

    fn add(&mut self, node: usize, item: u32) {
        let b = self.boxes[item as usize];
        let n = &mut self.nodes[node];
        n.bounds.extend(b.min);
        n.bounds.extend(b.max);
        n.items.push(item);
    }

    /// `StreamingBVHNode.TrySplit`.
    fn try_split(&mut self, node: usize) {
        if self.nodes[node].items.len() <= SPLIT_THRESHOLD {
            return;
        }
        let bounds = self.nodes[node].bounds;
        let ncen = bounds.centre();
        let next = [0, 1].map(|i| (bounds.max[i] - bounds.min[i]) * 0.5);
        let nsizh = next[0].max(next[1]) * 0.5;
        let items = std::mem::take(&mut self.nodes[node].items);
        let mut kept = Vec::new();
        for &item in &items {
            let b = self.boxes[item as usize];
            let icen = b.centre();
            let iext = [0, 1].map(|i| (b.max[i] - b.min[i]) * 0.5);
            if iext[0].max(iext[1]) >= nsizh {
                kept.push(item);
                continue;
            }
            let c = (icen[0] > ncen[0]) as usize + 2 * (icen[1] > ncen[1]) as usize;
            let child = match self.nodes[node].children[c] {
                Some(child) => child as usize,
                None => {
                    self.nodes.push(BvhNode::new());
                    let child = self.nodes.len() - 1;
                    self.nodes[node].children[c] = Some(child as u32);
                    child
                }
            };
            self.add(child, item);
        }
        for c in self.nodes[node].children.into_iter().flatten() {
            // Not when every item went to one child: that wouldn't end.
            if self.nodes[c as usize].items.len() != items.len() {
                self.try_split(c as usize);
            }
        }
        self.nodes[node].items = kept;
    }

    /// `GetItems`: the items whose box contains `p`.
    pub fn query(&self, p: [f32; 3], out: &mut Vec<u32>) {
        if !self.nodes.is_empty() {
            self.query_node(0, p, out);
        }
    }

    fn query_node(&self, node: usize, p: [f32; 3], out: &mut Vec<u32>) {
        let n = &self.nodes[node];
        if !n.bounds.contains(p) {
            return;
        }
        out.extend(n.items.iter().copied().filter(|&i| self.boxes[i as usize].contains(p)));
        for c in n.children.into_iter().flatten() {
            self.query_node(c as usize, p, out);
        }
    }
}

// ---------------------------------------------------------------------------
// Cull boxes

/// `RDR2MapDataCullBox`, from `common:/data/mapdatacullboxes_new.meta`: while
/// the camera is in an enabled box, the ymaps it lists (and their children)
/// aren't streamed.
#[derive(Clone, Debug, PartialEq)]
pub struct CullBox {
    pub name: String,
    pub min: [f32; 3],
    pub max: [f32; 3],
    /// The ymaps culled.
    pub culled: Vec<u32>,
    pub enabled: bool,
}

impl CullBox {
    /// `RDR2MapDataCullBoxFile`'s entries.
    pub(crate) fn read_file(bag: &DataBag) -> Vec<CullBox> {
        let entries = bag.bags(0x37B8_D303).unwrap_or_default(); // "entries"
        entries.iter().flatten().map(CullBox::read).collect()
    }

    fn read(item: &DataBag) -> CullBox {
        let (mut min, mut max) = ([0.0; 3], [0.0; 3]);
        if let Some(aabb) = item.bag(0x1180_F0B8) {
            // "aabb"
            let [x, y, z, _] = aabb.vec4(0xFE30_E6F3); // "min"
            min = [x, y, z];
            let [x, y, z, _] = aabb.vec4(0x6060_68C4); // "max"
            max = [x, y, z];
        }
        let culled = match item.object(0xEFC0_E923) {
            // "culledContainerHashes"
            Some(Value::U32s(v) | Value::Hashes(v)) => v.clone(),
            _ => Vec::new(),
        };
        CullBox {
            name: item.string(0xE6E1_7F14).unwrap_or_default().to_string(), // "name"
            min,
            max,
            culled,
            enabled: item.bool(0x5403_564B), // "bEnabled"
        }
    }

    pub fn contains(&self, p: [f32; 3]) -> bool {
        Aabb::new(self.min, self.max).contains(p)
    }

    /// The C#'s `nameHash`: the name as written (not lowercased).
    pub fn name_hash(&self) -> u32 {
        joaat(&self.name)
    }
}

// ---------------------------------------------------------------------------
// Map states

/// `RDR2MapStateGroup`: a group of the map states file, which turns ymaps
/// (IPLs), a cull box and interior entity sets on or off by named states.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapStateGroup {
    pub name: String,
    pub cull_box: Option<String>,
    pub mp: bool,
    pub enabled: bool,
    /// State name → its current value (`None`: the group has no states).
    pub states: Option<Vec<(String, String)>>,
    /// (ymap name, state name, state value): on while the state has that value.
    pub ipls: Vec<(String, Option<String>, Option<String>)>,
    pub entity_sets: Vec<MapStateEntitySet>,
}

/// A map state group's entity set: (its name, its interior's position, state
/// name, state value).
pub type MapStateEntitySet = (String, [f32; 3], Option<String>, Option<String>);

impl MapStateGroup {
    /// `IsStateActive`.
    pub fn is_state_active(&self, name: Option<&str>, value: Option<&str>) -> bool {
        if !self.enabled {
            return false;
        }
        let Some(name) = name.filter(|n| !n.is_empty()) else { return true };
        let Some(states) = &self.states else { return true };
        match states.iter().find(|(n, _)| n == name) {
            Some((_, current)) => Some(current.as_str()) == value,
            None => true,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Section {
    None,
    Ipls,
    States,
    EntitySets,
}

/// `RDR2MapStatesFile`: the groups of a map states file
/// (`RedForge.Games.RDR2.mapstates.txt`).
pub fn parse_map_states(text: &str) -> Vec<MapStateGroup> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut groups: Vec<MapStateGroup> = Vec::new();
    let mut section = Section::None;
    for line in text.lines() {
        let line = line.find("//").map_or(line, |c| &line[..c]);
        let parts: Vec<&str> = line.split([' ', '\t']).map(str::trim).filter(|p| !p.is_empty()).collect();
        let Some(&first) = parts.first() else { continue };
        if let Some(name) = first.strip_prefix("group:") {
            section = Section::None;
            let mut g = MapStateGroup { name: name.to_string(), enabled: true, ..Default::default() };
            for p in &parts[1..] {
                let sub: Vec<&str> = p.split(':').map(str::trim).filter(|s| !s.is_empty()).collect();
                if sub.len() < 2 {
                    continue;
                }
                match sub[0] {
                    "mp" => g.mp = sub[1] == "true",
                    "enabled" => g.enabled = sub[1] == "true",
                    "cullbox" => g.cull_box = Some(sub[1].to_string()),
                    _ => {}
                }
            }
            groups.push(g);
            continue;
        }
        match first {
            "ipls:" => section = Section::Ipls,
            "states:" => section = Section::States,
            "entitysets:" => section = Section::EntitySets,
            _ => {
                let Some(g) = groups.last_mut() else { continue };
                let name = first.to_lowercase();
                let state = |p: Option<&&str>| {
                    let mut s = p.map(|p| p.split(':'));
                    let s0 = s.as_mut().and_then(|s| s.next()).map(str::to_string);
                    let s1 = s.as_mut().and_then(|s| s.next()).map(str::to_string);
                    (s0, s1)
                };
                match section {
                    Section::Ipls => {
                        let (n, v) = state(parts.get(1));
                        g.ipls.push((name, n, v));
                    }
                    Section::States => {
                        let value = parts.get(1).unwrap_or(&"").to_string();
                        let states = g.states.get_or_insert_with(Vec::new);
                        match states.iter_mut().find(|(n, _)| *n == name) {
                            Some(s) => s.1 = value,
                            None => states.push((name, value)),
                        }
                    }
                    Section::EntitySets => {
                        // `FloatUtil.Parse`: 0 when it isn't a number.
                        let f = |i: usize| parts.get(i).and_then(|p| p.replace(',', "").parse().ok()).unwrap_or(0.0);
                        let (n, v) = state(parts.get(4));
                        g.entity_sets.push((name, [f(1), f(2), f(3)], n, v));
                    }
                    Section::None => {}
                }
            }
        }
    }
    groups
}

/// The map states file the C# ships, `data/rdr2/mapstates.txt`.
pub const MAP_STATES: &str = include_str!("../../../data/rdr2/mapstates.txt");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bvh_finds_the_boxes_around_a_point() {
        // A grid of small boxes and a few big ones.
        let mut boxes = Vec::new();
        for x in 0..20 {
            for y in 0..20 {
                let (x, y) = (x as f32 * 100.0, y as f32 * 100.0);
                boxes.push(Aabb::new([x, y, -10.0], [x + 150.0, y + 150.0, 10.0]));
            }
        }
        boxes.push(Aabb::new([-5000.0, -5000.0, -100.0], [5000.0, 5000.0, 1000.0]));
        boxes.push(Aabb::new([0.0, 0.0, 500.0], [3000.0, 3000.0, 600.0]));
        let n = boxes.len() as u32;
        let bvh = Bvh::build(boxes.clone(), 0..n);
        for p in [[0.0, 0.0, 0.0], [120.0, 260.0, 5.0], [1999.0, 1999.0, 0.0], [700.0, 700.0, 550.0], [-1.0, 3.0, 0.0]]
        {
            let mut got = Vec::new();
            bvh.query(p, &mut got);
            got.sort();
            let want: Vec<u32> = (0..n).filter(|&i| boxes[i as usize].contains(p)).collect();
            assert_eq!(got, want, "at {p:?}");
        }
        // Only the items given go in.
        let bvh = Bvh::build(boxes, [400]);
        let mut got = Vec::new();
        bvh.query([0.0; 3], &mut got);
        assert_eq!(got, [400]);
    }

    #[test]
    fn map_states_file() {
        let text = "\u{FEFF}\ngroup:a mp:true cullbox:box_a\nstates:\n  s1  on // comment\n  s2\nipls:\n  IPL_One s1:on\n  ipl_two s1:off\n  ipl_three\n\
                    entitysets:\n  set_a  1,000.5 -2 3 s2:x\ngroup:b enabled:false\nipls:\n  ipl_four\n";
        let g = parse_map_states(text);
        assert_eq!(g.len(), 2);
        let a = &g[0];
        assert_eq!((a.name.as_str(), a.mp, a.enabled, a.cull_box.as_deref()), ("a", true, true, Some("box_a")));
        assert_eq!(a.states, Some(vec![("s1".into(), "on".into()), ("s2".into(), String::new())]));
        assert_eq!(a.ipls[0], ("ipl_one".into(), Some("s1".into()), Some("on".into())));
        assert_eq!(a.ipls[2], ("ipl_three".into(), None, None));
        assert_eq!(a.entity_sets[0], ("set_a".into(), [1000.5, -2.0, 3.0], Some("s2".into()), Some("x".into())));
        let active = |i: usize| a.is_state_active(a.ipls[i].1.as_deref(), a.ipls[i].2.as_deref());
        assert!(active(0) && !active(1) && active(2));
        // A state set to nothing only matches nothing given as a value.
        assert!(!a.is_state_active(Some("s2"), Some("x")) && !a.is_state_active(Some("s2"), None));
        assert!(a.is_state_active(Some("unknown"), Some("x")));
        assert!(!g[1].enabled && !g[1].is_state_active(None, None));
    }

    #[test]
    fn shipped_map_states() {
        let g = parse_map_states(MAP_STATES);
        assert_eq!(g.len(), 1);
        // The defaults turn every IPL listed off.
        assert_eq!(g[0].ipls.len(), 20);
        assert!(g[0].ipls.iter().all(|(_, n, v)| !g[0].is_state_active(n.as_deref(), v.as_deref())));
        // The made-up names stand for their hash.
        assert!(g[0].ipls.iter().any(|(n, ..)| joaat(n) == 0x4070_17EA));
    }
}
