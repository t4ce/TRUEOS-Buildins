//! Browser/DOM dependencies required by translated collection algorithms.
//! Backends must provide actual semantics. Defaults report absent capabilities.
use crate::Value;
use alloc::{string::String, vec::Vec};
use core::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Unsupported(&'static str),
    InvalidInput(&'static str),
    Host(String),
    ForeignDocument,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(name) => write!(f, "unsupported host operation: {name}"),
            Self::InvalidInput(message) => f.write_str(message),
            Self::Host(message) => f.write_str(message),
            Self::ForeignDocument => f.write_str("selections belong to different document hosts"),
        }
    }
}
impl core::error::Error for Error {}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Element,
    Text,
    Comment,
    Document,
    Fragment,
    Other,
}
pub trait Dom {
    type Node: Clone + Eq;
    fn query(&self, context: Option<&Self::Node>, selector: &str)
    -> Result<Vec<Self::Node>, Error>;
    fn matches(&self, node: &Self::Node, selector: &str) -> Result<bool, Error>;
    /// Match a selector against an ordered seed, retaining its order and
    /// duplicate occurrences. Backends implementing jQuery positional selector
    /// extensions must override this; per-element matches cannot provide their
    /// set-based semantics. The default supports ordinary selector matching.
    fn filter(&self, nodes: &[Self::Node], selector: &str) -> Result<Vec<Self::Node>, Error> {
        if crate::utilities::needs_context(selector) {
            return Err(Error::Unsupported("seed selector matching"));
        }
        let mut result = Vec::new();
        for node in nodes {
            if self.kind(node) == NodeKind::Element && self.matches(node, selector)? {
                result.push(node.clone());
            }
        }
        Ok(result)
    }
    fn kind(&self, node: &Self::Node) -> NodeKind;
    /// HTML is the default host contract. XML adapters must override this so
    /// jQuery's HTML property-name fixes are not applied to XML elements.
    fn is_xml(&self, _: &Self::Node) -> bool {
        false
    }
    fn parent(&self, node: &Self::Node) -> Option<Self::Node>;
    /// All child nodes, including text/comments, in tree order.
    fn children(&self, node: &Self::Node) -> Vec<Self::Node>;
    /// Stable document order for connected nodes; retain input order for
    /// otherwise unordered disconnected peers. Also remove duplicate handles.
    fn unique_sort(&self, nodes: &mut Vec<Self::Node>);
    fn text(&self, node: &Self::Node) -> String;
    fn set_text(&mut self, _: &Self::Node, _: &str) -> Result<(), Error> {
        Err(Error::Unsupported("text setter"))
    }
    fn attribute(&self, _: &Self::Node, _: &str) -> Result<Option<String>, Error> {
        Err(Error::Unsupported("attribute getter"))
    }
    fn set_attribute(&mut self, _: &Self::Node, _: &str, _: &str) -> Result<(), Error> {
        Err(Error::Unsupported("attribute setter"))
    }
    fn remove_attribute(&mut self, _: &Self::Node, _: &str) -> Result<(), Error> {
        Err(Error::Unsupported("attribute removal"))
    }
    fn property(&self, _: &Self::Node, _: &str) -> Result<Value, Error> {
        Err(Error::Unsupported("property getter"))
    }
    fn set_property(&mut self, _: &Self::Node, _: &str, _: Value) -> Result<(), Error> {
        Err(Error::Unsupported("property setter"))
    }
    fn remove_property(&mut self, _: &Self::Node, _: &str) -> Result<(), Error> {
        Err(Error::Unsupported("property removal"))
    }
    fn html(&self, _: &Self::Node) -> Result<String, Error> {
        Err(Error::Unsupported("HTML serialization"))
    }
    fn set_html(&mut self, _: &Self::Node, _: &str) -> Result<(), Error> {
        Err(Error::Unsupported("HTML parsing/insertion"))
    }
    fn append(&mut self, _: &Self::Node, _: &Self::Node) -> Result<(), Error> {
        Err(Error::Unsupported("node insertion"))
    }
    fn prepend(&mut self, _: &Self::Node, _: &Self::Node) -> Result<(), Error> {
        Err(Error::Unsupported("node insertion"))
    }
    fn insert_before(&mut self, _: &Self::Node, _: &Self::Node) -> Result<(), Error> {
        Err(Error::Unsupported("node insertion"))
    }
    fn insert_after(&mut self, _: &Self::Node, _: &Self::Node) -> Result<(), Error> {
        Err(Error::Unsupported("node insertion"))
    }
    fn detach(&mut self, _: &Self::Node) -> Result<(), Error> {
        Err(Error::Unsupported("node detachment"))
    }
    /// Remove jQuery private/user data and event handlers on this node and all
    /// element descendants. Mutation helpers call this before discarding nodes.
    fn clean_data(&mut self, _: &Self::Node) -> Result<(), Error> {
        Err(Error::Unsupported("event/data cleanup"))
    }
    fn clone_node(&mut self, _: &Self::Node, _: bool, _: bool) -> Result<Self::Node, Error> {
        Err(Error::Unsupported("node cloning"))
    }
    /// jQuery contents semantics include template.content and the accessible
    /// contentDocument of frames/objects. Ordinary child nodes alone do not
    /// implement that contract, so hosts must opt in explicitly.
    fn contents(&self, _: &Self::Node) -> Result<Vec<Self::Node>, Error> {
        Err(Error::Unsupported("contents traversal"))
    }
}
