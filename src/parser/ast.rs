use crate::lexer::type_def::Span;

pub trait NodeData: Clone {
    fn default() -> Self;
}

#[derive(Default, Clone)]
pub struct Node<T: NodeData> {
    data: T,
    //该节点覆盖的源码字节区间。叶子用词法器给的原样；内部节点是
    //[最左孩子.start, 最右孩子.end)；ε 节点是归约那一刻 lookahead 处的零长度区间
    span: Span,
    children: Vec<usize>,
    parent: Option<usize>,
}

impl<T: NodeData> Node<T> {
    pub fn new(data: T, span: Span) -> Node<T> {
        Node {
            data,
            span,
            children: vec![],
            parent: None,
        }
    }

    pub fn get_data(&self) -> &T {
        &self.data
    }
    pub fn set_data(&mut self, data: T) {
        self.data = data;
    }

    pub fn get_span(&self) -> Span {
        self.span
    }
    pub fn set_span(&mut self, span: Span) {
        self.span = span;
    }
}

pub struct Tree<T: NodeData> {
    current: usize,
    nodes: Vec<Node<T>>,
    root: i32,
}

impl<T: NodeData> Tree<T> {
    pub fn new() -> Tree<T> {
        Tree {
            current: 0,
            nodes: vec![Node { data: T::default(), span: Span::default(), children: vec![], parent: None };300],
            root: -1,
        }
    }

    pub fn alloc_node(&mut self) -> (&mut Node<T>, usize) {
        if self.current >= self.nodes.len() {
            //自动扩容
            self.nodes.extend(vec![Node { data: T::default(), span: Span::default(), children: vec![], parent: None };300]);
        }
        let node = &mut self.nodes[self.current];
        self.current += 1;
        (node, self.current - 1)
    }

    /// 取某个节点的 span。归约时算父节点区间要用，O(1)
    pub fn span_of(&self, node: usize) -> Span {
        self.nodes[node].span
    }

    pub fn set_root(&mut self, node: usize) {
        self.root = node as i32;
    }

    pub fn get_root(&self) -> Option<usize> {
        if self.root == -1 {
            None
        } else {
            Some(self.root as usize)
        }
    }

    pub fn relation(&mut self, parent: usize,child: usize) {
        self.nodes[child].parent = Some(parent);
        self.nodes[parent].children.push(child);
    }

    pub fn append_child(&mut self, parent: Option<usize>, data: T, span: Span) -> usize {
        let (node,index) = self.alloc_node();
        node.data = data;
        node.span = span;
        node.parent = parent;
        if let Some(parent) = parent {
            self.nodes[parent].children.push(index);
        }
        index
    }
}