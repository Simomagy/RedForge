//! The navmesh's XML, as `Rsc8Navmesh.Read/Write(MetaNodeReader/Writer)` and
//! `YnvFile` do it: polygons with their own vertices, edges and special links,
//! and the special links with world positions.

use super::{
    CONTENT_FLAG_NAMES, EDGE_FLAG_NAMES, Navmesh, NavmeshBuilder, NavmeshEdge, NavmeshPolygon, NavmeshSpecialLink,
    NavmeshVertex, POLY_FLAG_NAMES, format_flags, parse_flags, read,
};
use crate::Result;
use crate::formats::XmlContext;
use crate::xml::{XmlDocument, XmlNode, XmlWriter};

const ROOT: &str = "RDR2Navmesh";

/// A loose ynv file to XML.
pub fn to_xml(file: &[u8], cx: &XmlContext) -> Result<String> {
    let nm = read(file)?;
    Ok(XmlWriter::document(cx.hashes, ROOT, None, |w| nm.write_xml(w)))
}

/// XML to a loose ynv file.
pub fn from_xml(xml: &str, _cx: &XmlContext) -> Result<Vec<u8>> {
    let doc = XmlDocument::new(xml);
    let nm = doc.with_root(None, Navmesh::read_xml)?;
    Ok(nm.write())
}

/// The flat arrays the polygons' XML is written from (the C#'s `Cached*`).
struct Cache {
    vertices: Vec<[f32; 3]>,
    indices: Vec<u16>,
    edges: Vec<NavmeshEdge>,
}

impl NavmeshEdge {
    /// `GetXMLString`: "area ID; polygon; space around vertex; flags".
    pub fn xml_string(&self, adj_area_ids: &[u32]) -> String {
        let ai = self.area_id1() as usize;
        let area = adj_area_ids.get(ai).copied().unwrap_or(16383);
        let flags = format_flags(self.flags() as u64, EDGE_FLAG_NAMES);
        format!("{area}; {}; {}; {flags}", self.poly_id1(), self.space_around_vertex())
    }

    /// `FromXMLString`: the area ID is added to the navmesh's adjacent areas.
    pub fn from_xml_string(s: &str, nm: &mut Navmesh) -> Self {
        let parts: Vec<&str> = s.split(';').map(str::trim).filter(|p| !p.is_empty()).collect();
        if parts.len() != 4 {
            return Self::default();
        }
        let uint = |t: &str| t.strip_prefix('+').unwrap_or(t).parse::<u32>().unwrap_or(0);
        let area_id = uint(parts[0]);
        let poly_id = uint(parts[1]);
        let space_around = uint(parts[2]);
        let flags = parse_flags(Some(parts[3]), EDGE_FLAG_NAMES).unwrap_or(0) as u32;
        let mut e = Self::default();
        e.set_area_id1(nm.ensure_adj_area_id(area_id));
        e.set_area_id2(e.area_id1());
        e.set_poly_id1(poly_id);
        e.set_poly_id2(poly_id);
        e.set_adjacency_type(0);
        e.set_space_around_vertex(space_around);
        e.set_space_beyond_edge(0);
        e.set_flags(flags);
        e
    }
}

impl NavmeshPolygon {
    fn write_xml(&self, w: &mut XmlWriter, nm: &Navmesh, cache: &Cache) {
        let flags = self.flags();
        if flags != 0 {
            w.string("Flags", &format_flags(flags, POLY_FLAG_NAMES));
        }
        let bytes = [
            ("Audio", self.audio_properties()),
            ("WaterDepth", self.water_depth()),
            ("CentroidX", self.centroid_x),
            ("CentroidY", self.centroid_y),
            ("Unk2Cc", self.u2cc()),
            ("Unk30a", self.u30a()),
        ];
        for (id, v) in bytes {
            if v != 0 {
                w.u8(id, v);
            }
        }

        // The corners, cut short (with zero vertices and empty edges) where the
        // indices or edges run out.
        let count = self.index_count() as usize;
        let mut verts = vec![[0.0f32; 3]; count];
        let mut edges = vec![String::new(); count];
        let start = self.index_id;
        let end = start.wrapping_add(self.index_count());
        let mut n = 0;
        let mut i = start;
        while i < end {
            let Some(&ind) = cache.indices.get(i as usize) else { break };
            verts[n] = cache.vertices.get(ind as usize).copied().unwrap_or_default();
            let Some(edge) = cache.edges.get(i as usize) else { break };
            edges[n] = edge.xml_string(&nm.adj_area_ids);
            n += 1;
            i += 1;
        }
        w.vec3_array("Vertices", Some(&verts));
        w.string_array("Edges", Some(&edges));

        let link_count = self.link_count();
        if link_count != 0
            && let Some(nm_links) = &nm.special_links_inds
        {
            let mut links = vec![0u16; link_count as usize];
            for (i, l) in links.iter_mut().enumerate() {
                let id = self.link_id() as usize + i;
                let Some(&v) = nm_links.get(id) else { break };
                *l = v;
            }
            w.u16_array("Links", Some(&links));
        }
    }

    fn read_xml(n: XmlNode, nm: &mut Navmesh, builder: &mut NavmeshBuilder) -> Self {
        let mut p = Self::default();
        p.set_flags(parse_flags(n.string("Flags").as_deref(), POLY_FLAG_NAMES).unwrap_or(0));
        p.set_audio_properties(n.u8("Audio", 0));
        p.set_water_depth(n.u8("WaterDepth", 0));
        p.centroid_x = n.u8("CentroidX", 0);
        p.centroid_y = n.u8("CentroidY", 0);
        p.set_u2cc(n.u8("Unk2Cc", 0));
        p.set_u30a(n.u8("Unk30a", 0));
        p.set_area_id(nm.area_id as u16);

        let verts = n.vec3_array("Vertices");
        let links = n.u16_array("Links");
        let edges: Vec<NavmeshEdge> =
            n.string_array("Edges").unwrap_or_default().iter().map(|s| NavmeshEdge::from_xml_string(s, nm)).collect();
        builder.build_polygon(&mut p, verts.as_deref(), &edges, links.as_deref());
        p
    }
}

impl NavmeshSpecialLink {
    fn write_xml(&self, w: &mut XmlWriter, nm: &Navmesh) {
        let (min, size) = (nm.aabb_min, nm.extents3());
        w.u8("Type", self.link_type);
        w.u8("Angle", self.angle);
        w.u16("AreaID1", self.area_id1);
        w.u16("AreaID2", self.area_id2);
        w.u16("PolyID1", self.poly_id1);
        w.u16("PolyID2", self.poly_id2);
        w.vec3("Position1", self.position1.to_vec3(min, size));
        w.vec3("Position2", self.position2.to_vec3(min, size));
    }

    fn read_xml(n: XmlNode, nm: &Navmesh) -> Self {
        let (min, size) = (nm.aabb_min, nm.extents3());
        let poly_id1 = n.u16("PolyID1", 0);
        let poly_id2 = n.u16("PolyID2", 0);
        Self {
            link_type: n.u8("Type", 0),
            angle: n.u8("Angle", 0),
            area_id1: n.u16("AreaID1", 0),
            area_id2: n.u16("AreaID2", 0),
            poly_id1,
            poly_id1a: poly_id1,
            poly_id2,
            poly_id2a: poly_id2,
            position1: NavmeshVertex::new(n.vec3("Position1"), min, size),
            position2: NavmeshVertex::new(n.vec3("Position2"), min, size),
            ..Default::default()
        }
    }
}

impl Navmesh {
    /// Writes the navmesh's XML into the current node (the root).
    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.string("ContentFlags", &format_flags(self.content_flags as u64, CONTENT_FLAG_NAMES));
        w.u32("AreaID", self.area_id);
        w.u32("BuildID", self.build_id);
        w.vec3("AABBMin", self.aabb_min);
        w.vec3("AABBMax", self.aabb_max);
        w.vec3("Extents", self.extents3());
        w.u32_array("AdjAreaIDs", self.adj_area_ids_trimmed().as_deref());
        let cache = Cache {
            vertices: self.vertex_positions().unwrap_or_default(),
            indices: self.indices().unwrap_or_default(),
            edges: self.edges().unwrap_or_default(),
        };
        let polys = self.polygons();
        w.node_array_with("Polygons", polys.as_deref(), |_, _| None, |w, _, p| p.write_xml(w, self, &cache), true);
        w.node_array_with(
            "SpecialLinks",
            self.special_links.as_deref(),
            |_, _| None,
            |w, _, l| l.write_xml(w, self),
            true,
        );
    }

    /// Reads a navmesh from its XML, building its lists and quadtree as the C#
    /// does.
    pub fn read_xml(n: XmlNode) -> Self {
        let mut nm = Navmesh {
            transform: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            ..Default::default()
        };
        nm.content_flags = parse_flags(n.string("ContentFlags").as_deref(), CONTENT_FLAG_NAMES).unwrap_or(0) as u32;
        nm.area_id = n.u32("AreaID", 0);
        nm.build_id = n.u32("BuildID", 0);
        nm.aabb_min = n.vec3("AABBMin");
        nm.aabb_max = n.vec3("AABBMax");
        let [x, y, z] = n.vec3("Extents");
        nm.extents = [x, y, z, 0.0];
        nm.set_adj_area_ids_trimmed(n.u32_array("AdjAreaIDs").as_deref());

        let mut builder = NavmeshBuilder::new();
        // `type="null"` items are default polygons, not built.
        let polygons: Option<Vec<NavmeshPolygon>> = n.node_array("Polygons").map(|items| {
            items
                .into_iter()
                .map(|i| i.map(|i| NavmeshPolygon::read_xml(i, &mut nm, &mut builder)).unwrap_or_default())
                .collect()
        });
        nm.special_links = n.node_array("SpecialLinks").map(|items| {
            items.into_iter().map(|i| i.map(|i| NavmeshSpecialLink::read_xml(i, &nm)).unwrap_or_default()).collect()
        });
        nm.special_links_inds = Some(std::mem::take(&mut builder.links_inds));
        nm.rebuild_lists(&builder.vertices, &builder.indices, &builder.edges, polygons.as_deref());
        nm.rebuild_quad_tree(polygons.as_deref().unwrap_or_default(), &builder.vertices, &builder.indices);
        nm.update_total_bytes();
        nm
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::ynv::NavmeshAabb;
    use crate::hash::HashIndex;

    /// A small navmesh as the C# writes it: two triangles sharing an edge, and a
    /// special link between them. The vertices are on the corners of the box, so
    /// they survive quantisation exactly.
    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<RDR2Navmesh>
 <ContentFlags>Polygons, SpecialLinks</ContentFlags>
 <AreaID value="12345" />
 <BuildID value="67385349" />
 <AABBMin x="-2" y="-6" z="0" />
 <AABBMax x="2" y="6" z="3" />
 <Extents x="4" y="12" z="3" />
 <AdjAreaIDs>12345 65535</AdjAreaIDs>
 <Polygons>
  <Item i="0">
   <Flags>Small, Pavement</Flags>
   <Audio value="3" />
   <CentroidX value="128" />
   <CentroidY value="64" />
   <Unk2Cc value="1" />
   <Vertices>
    <Item x="-2" y="-6" z="0" />
    <Item x="2" y="-6" z="0" />
    <Item x="2" y="6" z="3" />
   </Vertices>
   <Edges>
    <Item>65535; 32767; 0; 0</Item>
    <Item>65535; 32767; 0; ExternalEdge</Item>
    <Item>12345; 1; 5; EdgeProvidesCover, Unknown16</Item>
   </Edges>
   <Links>0</Links>
  </Item>
  <Item i="1">
   <Flags>Water, Shelter4</Flags>
   <WaterDepth value="2" />
   <Vertices>
    <Item x="-2" y="-6" z="0" />
    <Item x="2" y="6" z="3" />
    <Item x="-2" y="6" z="3" />
   </Vertices>
   <Edges>
    <Item>12345; 0; 5; 34</Item>
    <Item>65535; 32767; 0; 0</Item>
    <Item>65535; 32767; 0; 0</Item>
   </Edges>
  </Item>
 </Polygons>
 <SpecialLinks>
  <Item i="0">
   <Type value="1" />
   <Angle value="64" />
   <AreaID1 value="12345" />
   <AreaID2 value="12345" />
   <PolyID1 value="0" />
   <PolyID2 value="1" />
   <Position1 x="-2" y="-6" z="0" />
   <Position2 x="2" y="6" z="3" />
  </Item>
 </SpecialLinks>
</RDR2Navmesh>
"#;

    #[test]
    fn xml_round_trips_through_the_game_format() {
        let hashes = HashIndex::new();
        let cx = XmlContext { hashes: &hashes, folder: None };
        let file = from_xml(XML, &cx).unwrap();
        let nm = read(&file).unwrap();

        // Shared vertices are written once.
        assert_eq!(nm.vertex_positions().unwrap().len(), 4);
        assert_eq!(nm.indices().unwrap(), [0, 1, 2, 0, 2, 3]);
        assert_eq!((nm.size_of_pools, nm.vertices_count, nm.polygons_count), (6, 4, 2));
        assert_eq!((nm.special_links_count, nm.special_links_inds_count), (1, 1));
        assert_eq!(nm.adj_area_ids_trimmed().unwrap(), [12345, 65535]);
        let polys = nm.polygons().unwrap();
        assert_eq!((polys[1].index_id, polys[1].index_count(), polys[1].area_id()), (3, 3, 12345));
        let aabb = NavmeshAabb { min_x: -4, max_x: 4, min_y: -12, max_y: 12, min_z: 0, max_z: 6 };
        assert_eq!(polys[0].cell_aabb, aabb);
        // One leaf with both polygons.
        let tree = nm.quad_tree.as_ref().unwrap();
        assert!(tree.sub_trees.iter().all(Option::is_none));
        assert_eq!(tree.data.as_ref().unwrap().polygon_ids.as_deref(), Some(&[0u16, 1][..]));
        // Like the C#, the size leaves out the special links: their count is
        // only set when the file is written.
        assert_eq!(nm.total_bytes, 96 + 32 + 2 * 2 + 4 * 6 + 6 * 2 + 6 * 8 + 2 * 56);

        assert_eq!(to_xml(&file, &cx).unwrap(), XML);
    }
}
