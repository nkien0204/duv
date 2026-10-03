//! In-memory tree of scanned paths and their sizes.
//!
//! Every node lives in one flat `Vec<Node>` and refers to its parent and
//! children by [`NodeId`] (a `u32` index) rather than through nested
//! allocations. Nodes store only their own name; full paths are rebuilt by
//! walking up to the root (see [`Tree::path`]), so no `PathBuf` is kept
//! per node.
//!
//! A directory's entries are only held in memory when its children are
//! [`Children::Loaded`]; otherwise the directory still knows its total
//! size, so totals shown to the user never depend on what's loaded. This
//! is what lets the scanner stay within a memory budget: it can skip
//! storing a directory's entries (leaving it [`Children::Unloaded`]) or
//! drop them later with [`Tree::evict_children`], and load them again on
//! demand.
//!
//! Evicted nodes become [`NodeKind::Free`] slots that later insertions
//! reuse, so the node list stops growing once the budget is reached.
//!
//! Invariant: a loaded directory's `size` is the sum of its children's
//! sizes. [`Tree::add_child`] and [`Tree::add_size`] maintain this by
//! adding every size change to all of the node's ancestors.

use std::{
    num::NonZeroU32,
    path::{Path, PathBuf},
};

/// Index of a [`Node`] within its [`Tree`].
pub type NodeId = u32;

/// A single file or directory in the tree.
#[derive(Debug)]
pub struct Node {
    /// This entry's own file name (not its full path). Empty for the root.
    pub name: Box<str>,
    /// Size in bytes: a file's length, or a directory's recursive total.
    pub size: u64,
    /// Last modification time in seconds since the Unix epoch, or `0` if
    /// unknown. A directory's own time (when its entries last changed), not
    /// that of anything inside it.
    pub modified: u32,
    /// The parent's id plus one, so `None` fits in the same 4 bytes (`None`
    /// only for the root and free slots). Read it with [`Node::parent`].
    parent: Option<NonZeroU32>,
    pub kind: NodeKind,
}

impl Node {
    /// The parent directory's id; `None` for the root.
    pub fn parent(&self) -> Option<NodeId> {
        self.parent.map(|link| link.get() - 1)
    }

    fn free_slot() -> Self {
        Self {
            name: "".into(),
            size: 0,
            modified: 0,
            parent: None,
            kind: NodeKind::Free,
        }
    }

    pub fn is_dir(&self) -> bool {
        matches!(self.kind, NodeKind::Dir(_))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum NodeKind {
    File,
    Dir(Children),
    /// An evicted slot waiting to be reused. Never returned by
    /// [`Tree::get`].
    Free,
}

/// Whether a directory's entries are held in memory, and if not, why.
#[derive(Debug, PartialEq, Eq)]
pub enum Children {
    /// Entries are in memory, in insertion order until sorted.
    Loaded(Vec<NodeId>),
    /// Entries aren't in memory: not listed yet, skipped to stay within
    /// the memory budget, or evicted. Can be loaded into the tree later;
    /// `size` is accurate once the scan that measured it has finished.
    Unloaded,
    /// The directory is the mount point of another filesystem. Its space
    /// isn't counted (`size` is `0`) and it's never loaded into this tree.
    OtherFilesystem,
    /// The directory couldn't be listed (e.g. permission denied).
    Unreadable,
}

/// Where a path sits in a [`Tree`], from [`Tree::lookup`].
#[derive(Debug, PartialEq, Eq)]
pub enum Lookup {
    /// The path is this node.
    Found(NodeId),
    /// The path is somewhere under this unloaded directory, so its size is
    /// included in the directory's total but it has no node of its own.
    InsideUnloaded(NodeId),
    /// The path is under a directory whose space isn't counted (another
    /// filesystem, or unreadable).
    NotCounted,
    /// The path isn't under the root, or isn't in the tree.
    Outside,
}

/// A directory tree rooted at `root_path`.
#[derive(Debug)]
pub struct Tree {
    root_path: PathBuf,
    nodes: Vec<Node>,
    /// Slots in `nodes` freed by eviction, reused before growing `nodes`.
    free: Vec<NodeId>,
    /// Running estimate of the memory used by live nodes.
    approx_bytes: usize,
}

impl Tree {
    /// The root node's id.
    pub const ROOT: NodeId = 0;

    /// Creates a tree containing just an unloaded, zero-sized root
    /// directory for `root_path`.
    pub fn new(root_path: PathBuf) -> Self {
        let root = Node {
            name: "".into(),
            size: 0,
            modified: 0,
            parent: None,
            kind: NodeKind::Dir(Children::Unloaded),
        };
        Self {
            root_path,
            nodes: vec![root],
            free: Vec::new(),
            approx_bytes: std::mem::size_of::<Node>(),
        }
    }

    /// The filesystem path of the root node.
    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    /// The node with this id, or `None` if it doesn't exist or its slot is
    /// free.
    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes
            .get(id as usize)
            .filter(|node| !matches!(node.kind, NodeKind::Free))
    }

    /// Every live node, with its id.
    pub fn iter(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| !matches!(node.kind, NodeKind::Free))
            .map(|(id, node)| (id as NodeId, node))
    }

    /// Number of live nodes in the tree, including the root.
    pub fn node_count(&self) -> usize {
        self.nodes.len() - self.free.len()
    }

    /// Number of node slots the tree can hold before its node list
    /// reallocates. Spare capacity is real memory not counted by
    /// [`Tree::approx_bytes`].
    pub fn node_capacity(&self) -> usize {
        self.nodes.capacity()
    }

    /// Estimated memory used by live nodes, in bytes: each node's inline
    /// size, its name, and its slot in its parent's child list.
    pub fn approx_bytes(&self) -> usize {
        self.approx_bytes
    }

    /// The loaded children of `id`, or `None` if it's a file, a directory
    /// whose entries aren't loaded, or doesn't exist.
    pub fn children(&self, id: NodeId) -> Option<&[NodeId]> {
        match &self.get(id)?.kind {
            NodeKind::Dir(Children::Loaded(children)) => Some(children),
            _ => None,
        }
    }

    /// Adds a child under directory `parent` and returns its id, reusing a
    /// free slot if there is one. The child's `size` is added to every
    /// ancestor, and `parent` becomes loaded if it wasn't already.
    ///
    /// Returns `None` if `parent` doesn't exist or is a file.
    pub fn add_child(
        &mut self,
        parent: NodeId,
        name: impl Into<Box<str>>,
        size: u64,
        kind: NodeKind,
    ) -> Option<NodeId> {
        let id = match self.free.last() {
            Some(&id) => id,
            // `u32::MAX` is left unused so every id's parent link fits.
            None => NodeId::try_from(self.nodes.len())
                .ok()
                .filter(|&id| id < NodeId::MAX)?,
        };
        let children = match &mut self.get_mut(parent)?.kind {
            NodeKind::Dir(children) => children,
            _ => return None,
        };
        match children {
            Children::Loaded(ids) => ids.push(id),
            other => *other = Children::Loaded(vec![id]),
        }

        let name = name.into();
        self.approx_bytes += node_bytes(&name);
        let node = Node {
            name,
            size: 0,
            modified: 0,
            parent: NonZeroU32::new(parent + 1),
            kind,
        };
        if self.free.pop().is_some() {
            self.nodes[id as usize] = node;
        } else {
            self.nodes.push(node);
        }
        self.add_size(id, size);
        Some(id)
    }

    /// Adds `size` bytes to node `id` and all of its ancestors.
    pub fn add_size(&mut self, id: NodeId, size: u64) {
        let mut current = Some(id);
        while let Some(node) = current.and_then(|id| self.get_mut(id)) {
            node.size += size;
            current = node.parent();
        }
    }

    /// Sets directory `id`'s size to `0`, subtracting its old size from all
    /// of its ancestors. Used before (re)loading its entries, which add
    /// their sizes back as they arrive.
    pub fn clear_size(&mut self, id: NodeId) {
        if let Some(size) = self.get(id).map(|node| node.size) {
            self.subtract_size(id, size);
        }
    }

    /// Subtracts `size` bytes from node `id` and all of its ancestors,
    /// capped at the node's own size so no total goes below what's left.
    pub fn subtract_size(&mut self, id: NodeId, size: u64) {
        let Some(size) = self.get(id).map(|node| size.min(node.size)) else {
            return;
        };
        let mut current = Some(id);
        while let Some(node) = current.and_then(|id| self.get_mut(id)) {
            node.size = node.size.saturating_sub(size);
            current = node.parent();
        }
    }

    /// Removes node `id` and everything under it (e.g. after it was deleted
    /// from disk): its size is subtracted from its ancestors, it's dropped
    /// from its parent's children, and all its slots are freed for reuse.
    /// Calls `on_free` with each freed id. Returns the removed size, or
    /// `None` for the root or a missing node.
    pub fn remove(&mut self, id: NodeId, mut on_free: impl FnMut(NodeId)) -> Option<u64> {
        let node = self.get(id)?;
        let parent = node.parent()?;
        let size = node.size;

        self.subtract_size(parent, size);
        if let Some(Node {
            kind: NodeKind::Dir(Children::Loaded(siblings)),
            ..
        }) = self.get_mut(parent)
        {
            siblings.retain(|&sibling| sibling != id);
        }
        self.evict_children(id, &mut on_free);
        let node = std::mem::replace(&mut self.nodes[id as usize], Node::free_slot());
        self.approx_bytes -= node_bytes(&node.name);
        self.free.push(id);
        on_free(id);
        Some(size)
    }

    /// Finds where `path` sits in the tree, following loaded directories
    /// down from the root by name.
    pub fn lookup(&self, path: &Path) -> Lookup {
        let Ok(relative) = path.strip_prefix(&self.root_path) else {
            return Lookup::Outside;
        };
        let mut current = Self::ROOT;
        for component in relative.components() {
            let Some(node) = self.get(current) else {
                return Lookup::Outside;
            };
            match &node.kind {
                NodeKind::Dir(Children::Loaded(children)) => {
                    let name = component.as_os_str().to_string_lossy();
                    match children
                        .iter()
                        .copied()
                        .find(|&child| self.get(child).is_some_and(|c| *c.name == *name))
                    {
                        Some(child) => current = child,
                        None => return Lookup::Outside,
                    }
                }
                NodeKind::Dir(Children::Unloaded) => return Lookup::InsideUnloaded(current),
                NodeKind::Dir(Children::OtherFilesystem | Children::Unreadable) => {
                    return Lookup::NotCounted;
                }
                NodeKind::File | NodeKind::Free => return Lookup::Outside,
            }
        }
        Lookup::Found(current)
    }

    /// Records node `id`'s last modification time (seconds since the Unix
    /// epoch).
    pub fn set_modified(&mut self, id: NodeId, modified: u32) {
        if let Some(node) = self.get_mut(id) {
            node.modified = modified;
        }
    }

    /// Sets directory `id`'s children state, e.g. to mark it
    /// [`Children::Unreadable`]. Only valid for a directory whose entries
    /// aren't loaded (use [`Tree::evict_children`] to drop loaded ones);
    /// no-op otherwise.
    pub fn set_children(&mut self, id: NodeId, children: Children) {
        if let Some(Node {
            kind: NodeKind::Dir(current),
            ..
        }) = self.get_mut(id)
            && !matches!(current, Children::Loaded(_))
        {
            *current = children;
        }
    }

    /// Marks directory `id` as loaded with no entries yet, e.g. after
    /// listing an empty directory. No-op if it's already loaded or isn't
    /// a directory.
    pub fn mark_loaded(&mut self, id: NodeId) {
        self.set_children(id, Children::Loaded(Vec::new()));
    }

    /// Drops every node under directory `id`, freeing their slots for
    /// reuse, and marks `id` [`Children::Unloaded`]. Sizes are unchanged.
    /// Calls `on_free` with each freed id, so callers can forget anything
    /// they keyed by those ids. Returns the number of nodes freed.
    pub fn evict_children(&mut self, id: NodeId, mut on_free: impl FnMut(NodeId)) -> usize {
        let Some(Node {
            kind: NodeKind::Dir(children @ Children::Loaded(_)),
            ..
        }) = self.get_mut(id)
        else {
            return 0;
        };
        let Children::Loaded(mut stack) = std::mem::replace(children, Children::Unloaded) else {
            return 0;
        };
        let mut freed = 0;
        while let Some(child) = stack.pop() {
            let node = std::mem::replace(&mut self.nodes[child as usize], Node::free_slot());
            if let NodeKind::Dir(Children::Loaded(grandchildren)) = node.kind {
                stack.extend(grandchildren);
            }
            self.approx_bytes -= node_bytes(&node.name);
            self.free.push(child);
            on_free(child);
            freed += 1;
        }
        freed
    }

    /// Sorts the children of `id` and of every loaded directory under it by
    /// size, largest first.
    pub fn sort_subtree_by_size(&mut self, id: NodeId) {
        let mut stack = vec![id];
        while let Some(dir) = stack.pop() {
            let Some(NodeKind::Dir(Children::Loaded(children))) =
                self.nodes.get_mut(dir as usize).map(|node| &mut node.kind)
            else {
                continue;
            };
            let mut children = std::mem::take(children);
            children.sort_by_key(|&child| std::cmp::Reverse(self.nodes[child as usize].size));
            stack.extend(&children);
            if let NodeKind::Dir(Children::Loaded(slot)) = &mut self.nodes[dir as usize].kind {
                *slot = children;
            }
        }
    }

    /// Releases spare capacity in the node and free lists.
    pub fn shrink_to_fit(&mut self) {
        self.nodes.shrink_to_fit();
        self.free.shrink_to_fit();
    }

    /// Rebuilds the full filesystem path of `id` from its ancestors' names.
    pub fn path(&self, id: NodeId) -> Option<PathBuf> {
        let mut names = Vec::new();
        let mut current = self.get(id)?;
        while let Some(parent) = current.parent() {
            names.push(&*current.name);
            current = self.get(parent)?;
        }
        let mut path = self.root_path.clone();
        path.extend(names.iter().rev());
        Some(path)
    }

    fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes
            .get_mut(id as usize)
            .filter(|node| !matches!(node.kind, NodeKind::Free))
    }
}

/// Estimated memory for one non-root node: its inline size, its name, and
/// its slot in its parent's child list.
fn node_bytes(name: &str) -> usize {
    std::mem::size_of::<Node>() + name.len() + std::mem::size_of::<NodeId>()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> NodeKind {
        NodeKind::Dir(Children::Unloaded)
    }

    /// root/
    /// ├── a.txt (10)
    /// └── sub/
    ///     ├── b.txt (100)
    ///     └── deep/ (unloaded, 5)
    fn sample() -> (Tree, NodeId, NodeId, NodeId) {
        let mut tree = Tree::new(PathBuf::from("/data"));
        let a = tree
            .add_child(Tree::ROOT, "a.txt", 10, NodeKind::File)
            .unwrap();
        let sub = tree.add_child(Tree::ROOT, "sub", 0, dir()).unwrap();
        tree.add_child(sub, "b.txt", 100, NodeKind::File).unwrap();
        let deep = tree.add_child(sub, "deep", 5, dir()).unwrap();
        (tree, a, sub, deep)
    }

    fn size(tree: &Tree, id: NodeId) -> u64 {
        tree.get(id).unwrap().size
    }

    #[test]
    fn new_tree_is_an_unloaded_root() {
        let tree = Tree::new(PathBuf::from("/data"));
        assert_eq!(tree.node_count(), 1);
        assert_eq!(size(&tree, Tree::ROOT), 0);
        assert!(tree.children(Tree::ROOT).is_none());
    }

    #[test]
    fn sizes_propagate_to_ancestors() {
        let (tree, _, sub, deep) = sample();
        assert_eq!(size(&tree, deep), 5);
        assert_eq!(size(&tree, sub), 105);
        assert_eq!(size(&tree, Tree::ROOT), 115);
    }

    #[test]
    fn add_size_and_clear_size_keep_ancestors_consistent() {
        let (mut tree, _, sub, deep) = sample();
        tree.add_size(deep, 20);
        assert_eq!(size(&tree, deep), 25);
        assert_eq!(size(&tree, Tree::ROOT), 135);

        tree.clear_size(sub);
        assert_eq!(size(&tree, sub), 0);
        assert_eq!(size(&tree, Tree::ROOT), 10);
    }

    #[test]
    fn unloaded_directory_keeps_its_size_but_has_no_children() {
        let (tree, _, _, deep) = sample();
        assert_eq!(
            tree.get(deep).unwrap().kind,
            NodeKind::Dir(Children::Unloaded)
        );
        assert!(tree.children(deep).is_none());
    }

    #[test]
    fn set_children_marks_unloaded_directories_only() {
        let (mut tree, _, sub, deep) = sample();
        tree.set_children(deep, Children::Unreadable);
        assert_eq!(
            tree.get(deep).unwrap().kind,
            NodeKind::Dir(Children::Unreadable)
        );
        tree.set_children(sub, Children::Unreadable);
        assert!(tree.children(sub).is_some(), "loaded dirs are untouched");
    }

    #[test]
    fn mark_loaded_makes_empty_directory_loaded() {
        let (mut tree, _, _, deep) = sample();
        tree.mark_loaded(deep);
        assert_eq!(tree.children(deep), Some(&[][..]));
    }

    #[test]
    fn cannot_add_child_to_file() {
        let (mut tree, a, _, _) = sample();
        assert!(tree.add_child(a, "x", 1, NodeKind::File).is_none());
        assert_eq!(tree.node_count(), 5);
    }

    #[test]
    fn sort_orders_only_the_given_subtree_largest_first() {
        let (mut tree, a, sub, deep) = sample();
        let b = tree.children(sub).unwrap()[0];
        tree.add_size(deep, 200);
        tree.sort_subtree_by_size(sub);
        assert_eq!(tree.children(sub), Some(&[deep, b][..]));
        // The root's children weren't part of the sorted subtree.
        assert_eq!(tree.children(Tree::ROOT), Some(&[a, sub][..]));
    }

    #[test]
    fn path_is_rebuilt_from_ancestors() {
        let (tree, a, _, deep) = sample();
        assert_eq!(tree.path(Tree::ROOT).unwrap(), PathBuf::from("/data"));
        assert_eq!(tree.path(a).unwrap(), PathBuf::from("/data/a.txt"));
        assert_eq!(tree.path(deep).unwrap(), PathBuf::from("/data/sub/deep"));
    }

    #[test]
    fn approx_bytes_grows_with_nodes_and_names() {
        let mut tree = Tree::new(PathBuf::from("/data"));
        let before = tree.approx_bytes();
        tree.add_child(Tree::ROOT, "name", 1, NodeKind::File)
            .unwrap();
        let per_node = std::mem::size_of::<Node>() + std::mem::size_of::<NodeId>();
        assert_eq!(tree.approx_bytes(), before + per_node + "name".len());
    }

    #[test]
    fn evict_children_frees_the_subtree_but_keeps_sizes() {
        let (mut tree, _, sub, deep) = sample();
        let b = tree.children(sub).unwrap()[0];
        let before = tree.approx_bytes();

        let mut freed = Vec::new();
        assert_eq!(tree.evict_children(sub, |id| freed.push(id)), 2);
        freed.sort();
        assert_eq!(freed, [b, deep]);

        assert_eq!(
            tree.get(sub).unwrap().kind,
            NodeKind::Dir(Children::Unloaded)
        );
        assert_eq!(size(&tree, sub), 105);
        assert_eq!(size(&tree, Tree::ROOT), 115);
        assert!(tree.get(b).is_none());
        assert_eq!(tree.node_count(), 3);
        assert_eq!(tree.iter().count(), 3);
        assert!(tree.approx_bytes() < before);
    }

    #[test]
    fn freed_slots_are_reused() {
        let (mut tree, a, sub, _) = sample();
        tree.evict_children(sub, |_| {});
        let len_before = tree.nodes.len();

        let x = tree.add_child(a, "x", 1, NodeKind::File);
        let y = tree.add_child(Tree::ROOT, "y", 1, NodeKind::File).unwrap();
        let z = tree.add_child(Tree::ROOT, "z", 1, NodeKind::File).unwrap();
        assert!(x.is_none(), "a.txt is a file");
        assert_eq!(
            tree.nodes.len(),
            len_before,
            "no growth while slots are free"
        );
        assert_eq!(tree.path(z).unwrap(), PathBuf::from("/data/z"));
        assert_eq!(tree.path(y).unwrap(), PathBuf::from("/data/y"));
        assert_eq!(tree.node_count(), 5);

        tree.add_child(Tree::ROOT, "w", 1, NodeKind::File).unwrap();
        assert_eq!(tree.nodes.len(), len_before + 1);
    }

    #[test]
    fn remove_drops_the_subtree_and_its_size() {
        let (mut tree, a, sub, deep) = sample();
        let b = tree.children(sub).unwrap()[0];
        let before = tree.approx_bytes();

        let mut freed = Vec::new();
        assert_eq!(tree.remove(sub, |id| freed.push(id)), Some(105));
        freed.sort();
        assert_eq!(freed, [sub, b, deep]);

        assert_eq!(tree.children(Tree::ROOT), Some(&[a][..]));
        assert_eq!(size(&tree, Tree::ROOT), 10);
        assert_eq!(tree.node_count(), 2);
        assert!(tree.approx_bytes() < before);
        assert!(tree.get(sub).is_none());

        // Freed slots are reused.
        let len = tree.nodes.len();
        tree.add_child(Tree::ROOT, "new", 1, NodeKind::File)
            .unwrap();
        assert_eq!(tree.nodes.len(), len);
    }

    #[test]
    fn remove_rejects_the_root() {
        let (mut tree, _, _, _) = sample();
        assert_eq!(tree.remove(Tree::ROOT, |_| {}), None);
        assert_eq!(tree.node_count(), 5);
    }

    #[test]
    fn subtract_size_saturates_at_zero() {
        let (mut tree, _, sub, deep) = sample();
        tree.subtract_size(deep, 1_000);
        assert_eq!(size(&tree, deep), 0);
        assert_eq!(size(&tree, sub), 100);
        assert_eq!(size(&tree, Tree::ROOT), 110);
    }

    #[test]
    fn lookup_follows_loaded_directories() {
        let (mut tree, a, _, deep) = sample();
        tree.add_child(
            Tree::ROOT,
            "mnt",
            0,
            NodeKind::Dir(Children::OtherFilesystem),
        )
        .unwrap();

        assert_eq!(tree.lookup(Path::new("/data")), Lookup::Found(Tree::ROOT));
        assert_eq!(tree.lookup(Path::new("/data/a.txt")), Lookup::Found(a));
        assert_eq!(
            tree.lookup(Path::new("/data/sub/deep")),
            Lookup::Found(deep)
        );
        assert_eq!(
            tree.lookup(Path::new("/data/sub/deep/x/y")),
            Lookup::InsideUnloaded(deep)
        );
        assert_eq!(tree.lookup(Path::new("/data/mnt/x")), Lookup::NotCounted);
        assert_eq!(tree.lookup(Path::new("/data/missing")), Lookup::Outside);
        assert_eq!(tree.lookup(Path::new("/data/a.txt/x")), Lookup::Outside);
        assert_eq!(tree.lookup(Path::new("/elsewhere")), Lookup::Outside);
    }

    #[test]
    fn node_stays_small() {
        // Guards against accidentally bloating every node in large scans.
        // Adding the modification time kept nodes at 56 bytes by packing
        // the parent link into 4.
        assert!(
            std::mem::size_of::<Node>() <= 56,
            "{}",
            std::mem::size_of::<Node>()
        );
    }
}
