//! A small arena tree for html5ever to build into.
//!
//! html5ever's own `markup5ever_rcdom` describes itself as an unsupported test DOM, so the
//! reader keeps its own: nodes in a `Vec`, handles are indices, and only what the importer
//! reads is kept (element names, attributes, text). Comments, doctypes and processing
//! instructions become inert nodes that are never visited.

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::{Attribute, ParseOpts, QualName, parse_document};

/// Index of a node in [`Dom::nodes`].
pub type NodeId = usize;

#[derive(Debug)]
pub enum NodeKind {
    Document,
    /// The name (its local part is lower case for HTML) and the attributes as
    /// (local name, value).
    Element {
        name: Rc<QualName>,
        attrs: Vec<(String, String)>,
    },
    Text(String),
    /// Comments, doctypes, processing instructions: kept only so handles stay valid.
    Other,
}

#[derive(Debug)]
pub struct Node {
    pub kind: NodeKind,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
}

/// The parsed tree.
#[derive(Debug)]
pub struct Dom {
    pub nodes: Vec<Node>,
}

impl Dom {
    pub const ROOT: NodeId = 0;

    /// Parses a whole document (a fragment is wrapped in `html`/`body` by the HTML rules).
    pub fn parse(source: &str) -> Dom {
        let sink = Sink::default();
        parse_document(sink, ParseOpts::default())
            .from_utf8()
            .one(source.as_bytes())
    }

    /// The element's local name, lower case (`""` for anything that is not an element).
    pub fn name(&self, id: NodeId) -> &str {
        match &self.nodes[id].kind {
            NodeKind::Element { name, .. } => &name.local,
            _ => "",
        }
    }

    pub fn attr(&self, id: NodeId, key: &str) -> Option<&str> {
        match &self.nodes[id].kind {
            NodeKind::Element { attrs, .. } => attrs
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v.as_str()),
            _ => None,
        }
    }

    pub fn has_class(&self, id: NodeId, class: &str) -> bool {
        self.attr(id, "class")
            .is_some_and(|value| value.split_ascii_whitespace().any(|c| c == class))
    }

    pub fn children(&self, id: NodeId) -> &[NodeId] {
        &self.nodes[id].children
    }

    pub fn is_element(&self, id: NodeId) -> bool {
        matches!(self.nodes[id].kind, NodeKind::Element { .. })
    }

    /// The first descendant (depth first, `id` included) for which `pred` holds.
    pub fn find(&self, id: NodeId, pred: &dyn Fn(&Dom, NodeId) -> bool) -> Option<NodeId> {
        let mut stack = vec![id];
        while let Some(at) = stack.pop() {
            if pred(self, at) {
                return Some(at);
            }
            stack.extend(self.nodes[at].children.iter().rev());
        }
        None
    }

    /// Every descendant element named `name`, in document order.
    pub fn find_all(&self, id: NodeId, name: &str) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = vec![id];
        while let Some(at) = stack.pop() {
            if self.name(at) == name {
                out.push(at);
            }
            stack.extend(self.nodes[at].children.iter().rev());
        }
        out
    }

    /// All text below `id`, concatenated.
    pub fn text(&self, id: NodeId) -> String {
        let mut out = String::new();
        let mut stack = vec![id];
        while let Some(at) = stack.pop() {
            if let NodeKind::Text(text) = &self.nodes[at].kind {
                out.push_str(text);
            }
            stack.extend(self.nodes[at].children.iter().rev());
        }
        out
    }

    /// The `body` element, or the root when there is none.
    pub fn body(&self) -> NodeId {
        self.find(Self::ROOT, &|dom, id| dom.name(id) == "body")
            .unwrap_or(Self::ROOT)
    }
}

/// The html5ever sink. Interior mutability because html5ever calls it through `&self`.
#[derive(Default)]
struct Sink {
    nodes: RefCell<Vec<Node>>,
    /// Template contents: a template element's children go into a detached node.
    templates: RefCell<Vec<(NodeId, NodeId)>>,
}

impl Sink {
    fn push(&self, kind: NodeKind) -> NodeId {
        let mut nodes = self.nodes.borrow_mut();
        if nodes.is_empty() {
            nodes.push(Node {
                kind: NodeKind::Document,
                parent: None,
                children: Vec::new(),
            });
        }
        nodes.push(Node {
            kind,
            parent: None,
            children: Vec::new(),
        });
        nodes.len() - 1
    }

    fn ensure_root(&self) {
        let mut nodes = self.nodes.borrow_mut();
        if nodes.is_empty() {
            nodes.push(Node {
                kind: NodeKind::Document,
                parent: None,
                children: Vec::new(),
            });
        }
    }

    fn detach(nodes: &mut [Node], id: NodeId) {
        if let Some(parent) = nodes[id].parent.take() {
            nodes[parent].children.retain(|&c| c != id);
        }
    }

    /// Appends text to `parent`, merging with a text node that is already last.
    fn append_text(nodes: &mut Vec<Node>, parent: NodeId, text: &str) {
        if let Some(&last) = nodes[parent].children.last()
            && let NodeKind::Text(existing) = &mut nodes[last].kind
        {
            existing.push_str(text);
            return;
        }
        nodes.push(Node {
            kind: NodeKind::Text(text.to_owned()),
            parent: Some(parent),
            children: Vec::new(),
        });
        let id = nodes.len() - 1;
        nodes[parent].children.push(id);
    }
}

/// An element's name as html5ever asks for it. It shares the node's `Rc`, so it does not
/// borrow the sink's `RefCell`.
#[derive(Debug)]
pub struct Name(Rc<QualName>);

impl html5ever::tree_builder::ElemName for Name {
    fn ns(&self) -> &html5ever::Namespace {
        &self.0.ns
    }
    fn local_name(&self) -> &html5ever::LocalName {
        &self.0.local
    }
}

impl TreeSink for Sink {
    type Handle = NodeId;
    type Output = Dom;
    type ElemName<'a> = Name;

    fn finish(self) -> Dom {
        self.ensure_root();
        Dom {
            nodes: self.nodes.into_inner(),
        }
    }

    fn parse_error(&self, _msg: Cow<'static, str>) {}

    fn get_document(&self) -> NodeId {
        self.ensure_root();
        Dom::ROOT
    }

    fn elem_name<'a>(&'a self, target: &'a NodeId) -> Name {
        match &self.nodes.borrow()[*target].kind {
            NodeKind::Element { name, .. } => Name(Rc::clone(name)),
            // html5ever only asks about elements; a blank name is a safe answer otherwise.
            _ => Name(Rc::new(QualName::new(
                None,
                html5ever::ns!(html),
                html5ever::local_name!(""),
            ))),
        }
    }

    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, flags: ElementFlags) -> NodeId {
        let attrs = attrs
            .into_iter()
            .map(|a| (a.name.local.to_string(), a.value.to_string()))
            .collect();
        let id = self.push(NodeKind::Element {
            name: Rc::new(name),
            attrs,
        });
        if flags.template {
            let contents = self.push(NodeKind::Other);
            self.templates.borrow_mut().push((id, contents));
        }
        id
    }

    fn create_comment(&self, _text: StrTendril) -> NodeId {
        self.push(NodeKind::Other)
    }

    fn create_pi(&self, _target: StrTendril, _data: StrTendril) -> NodeId {
        self.push(NodeKind::Other)
    }

    fn append(&self, parent: &NodeId, child: NodeOrText<NodeId>) {
        let mut nodes = self.nodes.borrow_mut();
        match child {
            NodeOrText::AppendText(text) => Self::append_text(&mut nodes, *parent, &text),
            NodeOrText::AppendNode(node) => {
                Self::detach(&mut nodes, node);
                nodes[node].parent = Some(*parent);
                nodes[*parent].children.push(node);
            }
        }
    }

    fn append_based_on_parent_node(
        &self,
        element: &NodeId,
        prev_element: &NodeId,
        child: NodeOrText<NodeId>,
    ) {
        let has_parent = self.nodes.borrow()[*element].parent.is_some();
        if has_parent {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(&self, _: StrTendril, _: StrTendril, _: StrTendril) {}

    fn get_template_contents(&self, target: &NodeId) -> NodeId {
        self.templates
            .borrow()
            .iter()
            .find(|(t, _)| t == target)
            .map(|(_, c)| *c)
            .unwrap_or(*target)
    }

    fn same_node(&self, x: &NodeId, y: &NodeId) -> bool {
        x == y
    }

    fn set_quirks_mode(&self, _mode: QuirksMode) {}

    fn append_before_sibling(&self, sibling: &NodeId, new_node: NodeOrText<NodeId>) {
        let mut nodes = self.nodes.borrow_mut();
        let Some(parent) = nodes[*sibling].parent else {
            return;
        };
        let index = nodes[parent]
            .children
            .iter()
            .position(|c| c == sibling)
            .unwrap_or(0);
        match new_node {
            NodeOrText::AppendText(text) => {
                // Merge with a text node just before the sibling.
                if index > 0 {
                    let before = nodes[parent].children[index - 1];
                    if let NodeKind::Text(existing) = &mut nodes[before].kind {
                        existing.push_str(&text);
                        return;
                    }
                }
                nodes.push(Node {
                    kind: NodeKind::Text(text.to_string()),
                    parent: Some(parent),
                    children: Vec::new(),
                });
                let id = nodes.len() - 1;
                nodes[parent].children.insert(index, id);
            }
            NodeOrText::AppendNode(node) => {
                Self::detach(&mut nodes, node);
                let index = nodes[parent]
                    .children
                    .iter()
                    .position(|c| c == sibling)
                    .unwrap_or(0);
                nodes[node].parent = Some(parent);
                nodes[parent].children.insert(index, node);
            }
        }
    }

    fn add_attrs_if_missing(&self, target: &NodeId, attrs: Vec<Attribute>) {
        let mut nodes = self.nodes.borrow_mut();
        if let NodeKind::Element {
            attrs: existing, ..
        } = &mut nodes[*target].kind
        {
            for attr in attrs {
                let key = attr.name.local.to_string();
                if !existing.iter().any(|(k, _)| *k == key) {
                    existing.push((key, attr.value.to_string()));
                }
            }
        }
    }

    fn remove_from_parent(&self, target: &NodeId) {
        Self::detach(&mut self.nodes.borrow_mut(), *target);
    }

    fn reparent_children(&self, node: &NodeId, new_parent: &NodeId) {
        let mut nodes = self.nodes.borrow_mut();
        let children = std::mem::take(&mut nodes[*node].children);
        for &child in &children {
            nodes[child].parent = Some(*new_parent);
        }
        nodes[*new_parent].children.extend(children);
    }
}
