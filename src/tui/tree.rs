use crate::knowledge::{ObjectId, ObjectKind, ObjectMetadata};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
pub struct Row {
    pub id: ObjectId,
    pub path: Vec<ObjectId>,
    pub depth: usize,
    pub branch: bool,
    pub open: bool,
}
#[derive(Default)]
pub struct Tree {
    pub objects: Vec<ObjectMetadata>,
    pub expanded: BTreeSet<ObjectId>,
    pub selected: usize,
    pub filter: String,
}
impl Tree {
    pub fn get(&self, id: &ObjectId) -> Option<&ObjectMetadata> {
        self.objects.iter().find(|o| &o.id == id)
    }
    pub fn update(&mut self, mut objects: Vec<ObjectMetadata>) {
        let previous = self.rows().get(self.selected).map(|r| r.path.clone());
        if self.objects.is_empty() {
            self.expanded.extend(
                objects
                    .iter()
                    .filter(|o| matches!(o.kind, ObjectKind::Domain | ObjectKind::Topic))
                    .map(|o| o.id.clone()),
            );
        }
        objects.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.title.cmp(&b.title)));
        self.objects = objects;
        let rows = self.rows();
        self.selected = previous
            .and_then(|p| rows.iter().position(|r| r.path == p))
            .unwrap_or(self.selected)
            .min(rows.len().saturating_sub(1));
    }
    pub fn rows(&self) -> Vec<Row> {
        let objects = self
            .objects
            .iter()
            .map(|o| (&o.id, o))
            .collect::<BTreeMap<_, _>>();
        let mut children: BTreeMap<&ObjectId, Vec<&ObjectMetadata>> = BTreeMap::new();
        for object in &self.objects {
            for parent in &object.member_of {
                children.entry(parent).or_default().push(object);
            }
        }
        let mut rows = vec![];
        for object in self
            .objects
            .iter()
            .filter(|o| !o.member_of.iter().any(|id| objects.contains_key(id)))
        {
            self.visit(&object.id, &children, &mut vec![], &mut rows);
        }
        if !self.filter.is_empty() {
            let query = self.filter.to_lowercase();
            let paths = rows
                .iter()
                .filter(|r| {
                    objects.get(&r.id).is_some_and(|o| {
                        o.title.to_lowercase().contains(&query)
                            || o.aliases.iter().any(|a| a.to_lowercase().contains(&query))
                    })
                })
                .map(|r| r.path.clone())
                .collect::<Vec<_>>();
            rows.retain(|r| paths.iter().any(|p| p.starts_with(&r.path)));
        }
        rows
    }
    fn visit(
        &self,
        id: &ObjectId,
        children: &BTreeMap<&ObjectId, Vec<&ObjectMetadata>>,
        path: &mut Vec<ObjectId>,
        rows: &mut Vec<Row>,
    ) {
        if path.contains(id) {
            return;
        }
        let members = children.get(id).map(Vec::as_slice).unwrap_or(&[]);
        path.push(id.clone());
        let open = self.expanded.contains(id) || !self.filter.is_empty();
        rows.push(Row {
            id: id.clone(),
            path: path.clone(),
            depth: path.len() - 1,
            branch: !members.is_empty(),
            open,
        });
        if open {
            for member in members {
                self.visit(&member.id, children, path, rows);
            }
        }
        path.pop();
    }
    pub fn expand(&mut self, open: bool) {
        let rows = self.rows();
        if let Some(row) = rows.get(self.selected) {
            if open {
                self.expanded.insert(row.id.clone());
            } else if row.branch && row.open {
                self.expanded.remove(&row.id);
            } else if row.path.len() > 1 {
                self.selected = rows
                    .iter()
                    .position(|r| r.path == row.path[..row.path.len() - 1])
                    .unwrap_or(0);
            }
        }
    }
}
pub fn code(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Domain => "D",
        ObjectKind::Topic => "T",
        ObjectKind::Concept => "C",
        ObjectKind::Unit => "U",
        ObjectKind::Question => "Q",
        ObjectKind::Resource => "R",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multi_parent_hierarchy_and_filter_keep_distinct_paths() {
        let a = ObjectMetadata::new(ObjectKind::Domain, "A");
        let b = ObjectMetadata::new(ObjectKind::Domain, "B");
        let mut topic = ObjectMetadata::new(ObjectKind::Topic, "数据库");
        topic.member_of = vec![a.id.clone(), b.id.clone()];
        let mut c = ObjectMetadata::new(ObjectKind::Concept, "事务");
        c.member_of.push(topic.id.clone());
        let mut tree = Tree::default();
        tree.update(vec![a, b, topic, c.clone()]);
        let rows = tree
            .rows()
            .into_iter()
            .filter(|r| r.id == c.id)
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 2);
        assert_ne!(rows[0].path, rows[1].path);
        tree.expanded.clear();
        tree.filter = "事务".into();
        assert_eq!(tree.rows().len(), 6);
    }
}
