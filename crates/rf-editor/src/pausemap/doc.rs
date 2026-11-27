//! The layer tree over the flat object list: groups are objects too, and
//! members name their group in `group`. The list's order is the layers'
//! (last on top).

use std::collections::{HashMap, HashSet};

use super::project::MapObject;

pub type Index<'a> = HashMap<&'a str, &'a MapObject>;

pub fn index_of(objects: &[MapObject]) -> Index<'_> {
    objects.iter().map(|o| (o.id(), o)).collect()
}

/// The groups above an object, innermost first.
pub fn ancestors<'a>(o: &MapObject, index: &Index<'a>) -> Vec<&'a MapObject> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut g = o.common().group.as_deref().and_then(|g| index.get(g).copied());
    while let Some(group) = g {
        if !seen.insert(group.id()) {
            break;
        }
        out.push(group);
        g = group.common().group.as_deref().and_then(|g| index.get(g).copied());
    }
    out
}

pub fn is_hidden(o: &MapObject, index: &Index) -> bool {
    o.common().hidden || ancestors(o, index).iter().any(|g| g.common().hidden)
}

pub fn is_locked(o: &MapObject, index: &Index) -> bool {
    o.common().locked || ancestors(o, index).iter().any(|g| g.common().locked)
}

/// From the outermost group down to the object.
fn path_to<'a>(o: &'a MapObject, index: &Index<'a>) -> Vec<&'a MapObject> {
    let mut p = ancestors(o, index);
    p.reverse();
    p.push(o);
    p
}

/// What a click on the map picks: the outermost group around the object,
/// unless something in that group is selected, when clicks stay in it and
/// pick its members (as Illustrator and Figma let you work in a group).
pub fn pick_target<'a>(o: &'a MapObject, index: &Index<'a>, selection: &[String]) -> &'a str {
    let path = path_to(o, index);
    let entered = |g: &MapObject| {
        selection.iter().any(|id| {
            index
                .get(id.as_str())
                .is_some_and(|x| x.id() != g.id() && ancestors(x, index).iter().any(|a| a.id() == g.id()))
        })
    };
    let mut i = 0;
    while i + 1 < path.len() && entered(path[i]) {
        i += 1;
    }
    path[i].id()
}

/// A double-click goes one level deeper than the selected group under the
/// pointer.
pub fn drill_target<'a>(o: &'a MapObject, index: &Index<'a>, selection: &[String]) -> &'a str {
    let path = path_to(o, index);
    let at = path.iter().position(|p| selection.iter().any(|s| s == p.id()));
    let i = at.map_or(0, |a| a + 1).min(path.len() - 1);
    path[i].id()
}

/// The group right around a layer, to step back out with Esc.
pub fn parent_of(id: &str, index: &Index) -> Option<String> {
    let g = index.get(id)?.common().group.as_deref()?;
    index.contains_key(g).then(|| g.to_owned())
}

/// The ids and everything in any group among them.
pub fn with_descendants<'a>(ids: impl IntoIterator<Item = &'a str>, objects: &[MapObject]) -> HashSet<String> {
    let mut out: HashSet<String> = ids.into_iter().map(str::to_owned).collect();
    loop {
        let mut grew = false;
        for o in objects {
            if let Some(g) = &o.common().group
                && out.contains(g)
                && !out.contains(o.id())
            {
                out.insert(o.id().to_owned());
                grew = true;
            }
        }
        if !grew {
            return out;
        }
    }
}

/// The drawable objects of a selection (groups made their members).
pub fn drawables_in<'a>(ids: &[String], objects: &'a [MapObject]) -> Vec<&'a MapObject> {
    let all = with_descendants(ids.iter().map(String::as_str), objects);
    objects.iter().filter(|o| !o.is_group() && all.contains(o.id())).collect()
}

/// What reaches the tiles: the drawables not hidden, themselves or by a
/// group.
pub fn renderable(objects: &[MapObject]) -> Vec<&MapObject> {
    let index = index_of(objects);
    objects.iter().filter(|o| !o.is_group() && !is_hidden(o, &index)).collect()
}

/// A group's children (the root's for `None`), in the list's order.
pub fn children_of<'a>(id: Option<&str>, objects: &'a [MapObject]) -> Vec<&'a MapObject> {
    let index = index_of(objects);
    objects.iter().filter(|o| o.common().group.as_deref().filter(|g| index.contains_key(g)) == id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pausemap::project::{Common, GroupObject, LabelObject};

    fn group(id: &str, parent: Option<&str>) -> MapObject {
        MapObject::Group(GroupObject {
            common: Common { id: id.into(), group: parent.map(Into::into), ..Default::default() },
            collapsed: false,
        })
    }

    fn label(id: &str, parent: Option<&str>) -> MapObject {
        MapObject::Label(LabelObject {
            common: Common { id: id.into(), group: parent.map(Into::into), ..Default::default() },
            text: id.into(),
            font: String::new(),
            size: 10.0,
            x: 0.0,
            y: 0.0,
            rotation: 0.0,
            tracking: 0.0,
            curve: 0.0,
            weight: 0.0,
            caps: false,
        })
    }

    // The web app's doc tests.
    #[test]
    fn clicks_pick_the_outermost_group_until_one_is_entered() {
        let objects = vec![group("g", None), group("h", Some("g")), label("a", Some("h")), label("b", None)];
        let index = index_of(&objects);
        let a = index["a"];
        assert_eq!(pick_target(a, &index, &[]), "g");
        assert_eq!(pick_target(a, &index, &["h".into()]), "h");
        assert_eq!(pick_target(a, &index, &["a".into()]), "a");
        assert_eq!(drill_target(a, &index, &["g".into()]), "h");
        assert_eq!(drill_target(a, &index, &["h".into()]), "a");
        assert_eq!(parent_of("a", &index).as_deref(), Some("h"));
    }

    #[test]
    fn hidden_groups_hide_their_members() {
        let mut objects = vec![group("g", None), label("a", Some("g")), label("b", None)];
        objects[0].common_mut().hidden = true;
        let ids: Vec<&str> = renderable(&objects).iter().map(|o| o.id()).collect();
        assert_eq!(ids, vec!["b"]);
        let all = with_descendants(["g"], &objects);
        assert!(all.contains("a") && !all.contains("b"));
        assert_eq!(children_of(Some("g"), &objects).len(), 1);
        assert_eq!(children_of(None, &objects).len(), 2);
    }
}
