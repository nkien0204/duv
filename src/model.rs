//! In-memory tree of scanned paths and their sizes.
//!
//! Every node lives in one flat `Vec<Node>` and refers to its parent and
//! children by [`NodeId`] (a `u32` index) rather than through nested
//! allocations. Nodes store only their own name; full paths are rebuilt by
//! walking up to the root (see [`Tree::path`]), so no `PathBuf` is kept
//! per node.
//!
//! A directory's children are either [`Children::Loaded`] or
//! [`Children::Unloaded`]. An unloaded directory still knows its total
//! size, it just doesn't hold its entries in memory. This lets the tree
//! skip or later evict parts of a large scan (to stay within a memory
//! budget) without changing any totals shown to the user.
//!
//! Invariant: a loaded directory's `size` is the sum of its children's
//! sizes. [`Tree::add_child`] and [`Tree::attach`] maintain this by adding
//! each new node's size to all of its ancestors.
//!
//! [`Subtree`] is a temporary, nested form of a branch, built by the
//! scanner's parallel walk (where threads can't share one `Tree`) and
//! then flattened into the tree in one pass with [`Tree::attach`].

use std::path::{Path, PathBuf};

/// Index of a [`Node`] within its [`Tree`].
pub type NodeId = u32;

/// A single file or directory in the tree.
#[derive(Debug)]
pub struct Node {
    /// This entry's own file name (not its full path). Empty for the root.
    pub name: Box<str>,
    /// Size in bytes: a file's length, or a directory's recursive total.
    pub size: u64,
    /// `None` only for the root.
    pub parent: Option<NodeId>,
    pub kind: NodeKind,
}

impl Node {
    pub fn is_dir(&self) -> bool {
        matches!(self.kind, NodeKind::Dir(_))
    }
}

#[derive(Debug)]
pub enum NodeKind {
    File,
    Dir(Children),
}

/// Whether a directory's entries are held in memory.
#[derive(Debug)]
pub enum Children {
    /// Entries are in memory, in insertion order until
    /// [`Tree::sort_children_by_size`] is called.
    Loaded(Vec<NodeId>),
    /// Entries aren't in memory (never scanned, or evicted); the
    /// directory's `size` is still accurate.
    Unloaded,
}

/// A branch built outside a [`Tree`], to be flattened into one with
/// [`Tree::attach`].
#[derive(Debug)]
pub struct Subtree {
    pub name: Box<str>,
    pub size: u64,
    pub kind: SubtreeKind,
}

#[derive(Debug)]
pub enum SubtreeKind {
    File,
    Loaded(Vec<Subtree>),
    Unloaded,
}

impl Subtree {
    pub fn file(name: impl Into<Box<str>>, size: u64) -> Self {
        Self {
            name: name.into(),
            size,
            kind: SubtreeKind::File,
        }
    }

    /// A directory whose entries are in `children`; its size is their sum.
    pub fn dir(name: impl Into<Box<str>>, children: Vec<Subtree>) -> Self {
        Self {
            name: name.into(),
            size: children.iter().map(|child| child.size).sum(),
            kind: SubtreeKind::Loaded(children),
        }
    }

    /// A directory whose entries weren't collected, with a known `size`.
    pub fn unloaded_dir(name: impl Into<Box<str>>, size: u64) -> Self {
        Self {
            name: name.into(),
            size,
            kind: SubtreeKind::Unloaded,
        }
    }
}

/// A directory tree rooted at `root_path`.
#[derive(Debug)]
pub struct Tree {
    root_path: PathBuf,
    nodes: Vec<Node>,
    /// Running estimate of the heap and inline memory used by `nodes`.
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
            parent: None,
            kind: NodeKind::Dir(Children::Unloaded),
        };
        Self {
            root_path,
            nodes: vec![root],
            approx_bytes: std::mem::size_of::<Node>(),
        }
    }

    /// The filesystem path of the root node.
    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id as usize)
    }

    /// Number of nodes in the tree, including the root.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Estimated memory used by the tree's nodes, in bytes: each node's
    /// inline size, its name, and its slot in its parent's child list.
    pub fn approx_bytes(&self) -> usize {
        self.approx_bytes
    }

    /// The loaded children of `id`, or `None` if it's a file, an unloaded
    /// directory, or doesn't exist.
    pub fn children(&self, id: NodeId) -> Option<&[NodeId]> {
        match &self.get(id)?.kind {
            NodeKind::Dir(Children::Loaded(children)) => Some(children),
            _ => None,
        }
    }

    /// Adds a child under directory `parent` and returns its id.
    ///
    /// A directory child starts [`Children::Unloaded`] with the given
    /// `size` (pass `0` if its entries will be added afterwards). The
    /// child's `size` is added to every ancestor, and `parent` becomes
    /// loaded if it wasn't already.
    ///
    /// Returns `None` if `parent` doesn't exist or is a file.
    pub fn add_child(
        &mut self,
        parent: NodeId,
        name: &str,
        size: u64,
        is_dir: bool,
    ) -> Option<NodeId> {
        let id = NodeId::try_from(self.nodes.len()).ok()?;
        self.link_child(parent, id)?;
        self.nodes.push(Node {
            name: name.into(),
            size,
            parent: Some(parent),
            kind: if is_dir {
                NodeKind::Dir(Children::Unloaded)
            } else {
                NodeKind::File
            },
        });
        self.approx_bytes += node_bytes(name);
        self.add_size_to_ancestors(parent, size);
        Some(id)
    }

    /// Flattens `subtree` into the tree as a child of directory `parent`
    /// and returns the id of its top node. Sizes inside `subtree` are
    /// taken as given; its total is added to every ancestor.
    ///
    /// Returns `None` if `parent` doesn't exist or is a file.
    pub fn attach(&mut self, parent: NodeId, subtree: Subtree) -> Option<NodeId> {
        let size = subtree.size;
        let id = self.push_subtree(parent, subtree)?;
        self.add_size_to_ancestors(parent, size);
        Some(id)
    }

    fn push_subtree(&mut self, parent: NodeId, subtree: Subtree) -> Option<NodeId> {
        let id = NodeId::try_from(self.nodes.len()).ok()?;
        self.link_child(parent, id)?;
        let (kind, children) = match subtree.kind {
            SubtreeKind::File => (NodeKind::File, Vec::new()),
            SubtreeKind::Unloaded => (NodeKind::Dir(Children::Unloaded), Vec::new()),
            SubtreeKind::Loaded(children) => (
                NodeKind::Dir(Children::Loaded(Vec::with_capacity(children.len()))),
                children,
            ),
        };
        self.approx_bytes += node_bytes(&subtree.name);
        self.nodes.push(Node {
            name: subtree.name,
            size: subtree.size,
            parent: Some(parent),
            kind,
        });
        for child in children {
            self.push_subtree(id, child)?;
        }
        Some(id)
    }

    /// Appends `child` to directory `parent`'s child list, making it
    /// loaded if it wasn't. `None` if `parent` is missing or a file.
    fn link_child(&mut self, parent: NodeId, child: NodeId) -> Option<()> {
        let children = match &mut self.nodes.get_mut(parent as usize)?.kind {
            NodeKind::File => return None,
            NodeKind::Dir(children) => children,
        };
        match children {
            Children::Loaded(ids) => ids.push(child),
            Children::Unloaded => *children = Children::Loaded(vec![child]),
        }
        Some(())
    }

    fn add_size_to_ancestors(&mut self, from: NodeId, size: u64) {
        let mut ancestor = Some(from);
        while let Some(current) = ancestor {
            let node = &mut self.nodes[current as usize];
            node.size += size;
            ancestor = node.parent;
        }
    }

    /// Marks directory `id` as loaded with no entries yet, e.g. after
    /// scanning an empty directory. No-op if it's already loaded or isn't
    /// a directory.
    pub fn mark_loaded(&mut self, id: NodeId) {
        if let Some(Node {
            kind: NodeKind::Dir(children @ Children::Unloaded),
            ..
        }) = self.nodes.get_mut(id as usize)
        {
            *children = Children::Loaded(Vec::new());
        }
    }

    /// Sorts every loaded directory's children by size, largest first.
    pub fn sort_children_by_size(&mut self) {
        let sizes: Vec<u64> = self.nodes.iter().map(|node| node.size).collect();
        for node in &mut self.nodes {
            if let NodeKind::Dir(Children::Loaded(children)) = &mut node.kind {
                children.sort_by_key(|&id| std::cmp::Reverse(sizes[id as usize]));
            }
        }
    }

    /// Rebuilds the full filesystem path of `id` from its ancestors' names.
    pub fn path(&self, id: NodeId) -> Option<PathBuf> {
        let mut names = Vec::new();
        let mut current = self.get(id)?;
        while let Some(parent) = current.parent {
            names.push(&*current.name);
            current = &self.nodes[parent as usize];
        }
        let mut path = self.root_path.clone();
        path.extend(names.iter().rev());
        Some(path)
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

    /// root/
    /// ├── a.txt (10)
    /// └── sub/
    ///     ├── b.txt (100)
    ///     └── deep/ (unloaded, 5)
    fn sample() -> (Tree, NodeId, NodeId, NodeId) {
        let mut tree = Tree::new(PathBuf::from("/data"));
        let a = tree.add_child(Tree::ROOT, "a.txt", 10, false).unwrap();
        let sub = tree.add_child(Tree::ROOT, "sub", 0, true).unwrap();
        tree.add_child(sub, "b.txt", 100, false).unwrap();
        let deep = tree.add_child(sub, "deep", 5, true).unwrap();
        (tree, a, sub, deep)
    }

    #[test]
    fn new_tree_is_an_unloaded_root() {
        let tree = Tree::new(PathBuf::from("/data"));
        assert_eq!(tree.node_count(), 1);
        assert_eq!(tree.get(Tree::ROOT).unwrap().size, 0);
        assert!(tree.children(Tree::ROOT).is_none());
    }

    #[test]
    fn sizes_propagate_to_ancestors() {
        let (tree, _, sub, deep) = sample();
        assert_eq!(tree.get(deep).unwrap().size, 5);
        assert_eq!(tree.get(sub).unwrap().size, 105);
        assert_eq!(tree.get(Tree::ROOT).unwrap().size, 115);
    }

    #[test]
    fn unloaded_directory_keeps_its_size_but_has_no_children() {
        let (tree, _, _, deep) = sample();
        assert!(matches!(
            tree.get(deep).unwrap().kind,
            NodeKind::Dir(Children::Unloaded)
        ));
        assert!(tree.children(deep).is_none());
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
        assert!(tree.add_child(a, "x", 1, false).is_none());
        assert_eq!(tree.node_count(), 5);
    }

    #[test]
    fn sort_orders_children_largest_first() {
        let (mut tree, a, sub, _) = sample();
        assert_eq!(tree.children(Tree::ROOT), Some(&[a, sub][..]));
        tree.sort_children_by_size();
        assert_eq!(tree.children(Tree::ROOT), Some(&[sub, a][..]));
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
        tree.add_child(Tree::ROOT, "name", 1, false).unwrap();
        let per_node = std::mem::size_of::<Node>() + std::mem::size_of::<NodeId>();
        assert_eq!(tree.approx_bytes(), before + per_node + "name".len());
    }

    #[test]
    fn attach_flattens_subtree_and_propagates_its_total() {
        let (mut tree, _, sub, _) = sample();
        let branch = Subtree::dir(
            "new",
            vec![
                Subtree::file("c.txt", 7),
                Subtree::dir("inner", vec![Subtree::file("d.txt", 3)]),
                Subtree::unloaded_dir("mnt", 0),
            ],
        );
        let before = tree.approx_bytes();
        let new = tree.attach(sub, branch).unwrap();

        assert_eq!(tree.get(new).unwrap().size, 10);
        assert_eq!(tree.get(sub).unwrap().size, 115);
        assert_eq!(tree.get(Tree::ROOT).unwrap().size, 125);
        assert_eq!(tree.node_count(), 5 + 5);

        let children = tree.children(new).unwrap();
        assert_eq!(children.len(), 3);
        let inner = children[1];
        let d = tree.children(inner).unwrap()[0];
        assert_eq!(
            tree.path(d).unwrap(),
            PathBuf::from("/data/sub/new/inner/d.txt")
        );
        assert!(tree.children(children[2]).is_none()); // "mnt" stays unloaded

        let names = "newc.txtinnerd.txtmnt".len();
        let per_node = std::mem::size_of::<Node>() + std::mem::size_of::<NodeId>();
        assert_eq!(tree.approx_bytes(), before + 5 * per_node + names);
    }

    #[test]
    fn attach_to_file_is_rejected() {
        let (mut tree, a, _, _) = sample();
        assert!(tree.attach(a, Subtree::file("x", 1)).is_none());
        assert_eq!(tree.node_count(), 5);
    }

    #[test]
    fn node_stays_small() {
        // Guards against accidentally bloating every node in large scans.
        assert!(
            std::mem::size_of::<Node>() <= 64,
            "{}",
            std::mem::size_of::<Node>()
        );
    }
}
