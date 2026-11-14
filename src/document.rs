//! An XML document as the `XPath` engine evaluates it: read once by the
//! estate's one XML reader (`xmip-core-library-codec`'s tree), then built
//! into the engine's own model, `sxd-document`'s, which the engine is
//! written over and nothing else in the estate holds. The model is mutable,
//! so a rewrite is a write and one serialization at the end.
//!
//! What the reader refuses the engine never sees: a document type
//! declaration, an entity XML does not predefine, a prefix bound to no
//! namespace. Each prefix a document writes is kept as its preferred one,
//! and each namespace it declares where it declares it, so a document
//! written back reads as it was written.

use codec::xml::tree::{self, Node};
use contract::ContractError;
use path::{Editable, Form};
use stream::Stream;
use sxd_document::{Package, QName, dom};

/// An XML document, parsed once, for expressions to read and write. Its
/// own prefixes bind nothing an expression reads.
pub struct Document {
    package: Package,
}

impl Document {
    /// `text` read and built for the engine.
    ///
    /// # Errors
    /// The text is not a well-formed XML document the estate reads.
    pub fn parse(text: &str) -> Result<Self, ContractError> {
        Self::built(text, false)
    }

    /// `text` read and built; `written` where it will be written back, so
    /// each prefix and each declaration is kept where the text has it.
    /// Reading needs neither: an expression's prefixes bind through its
    /// own namespaces.
    fn built(text: &str, written: bool) -> Result<Self, ContractError> {
        let read = tree::Document::parse(text)
            .map_err(|refused| ContractError::new(format!("not well-formed XML: {refused}")))?;
        let package = Package::new();
        {
            let build = Build {
                into: package.as_document(),
                written,
            };
            let root = build.into.root();
            for child in read.root().children() {
                if child.is_element() {
                    root.append_child(build.element(child, true));
                } else if child.is_comment() {
                    root.append_child(build.into.create_comment(child.text().unwrap_or("")));
                } else if child.is_instruction() {
                    root.append_child(build.instruction(child));
                }
            }
        }
        Ok(Self { package })
    }

    /// The document node, where an absolute expression starts.
    pub(crate) fn root(&self) -> sxd_xpath::nodeset::Node<'_> {
        self.package.as_document().root().into()
    }
}

/// The tree read, built into the engine's model.
struct Build<'d> {
    into: dom::Document<'d>,
    /// Whether the prefixes and declarations as written are kept.
    written: bool,
}

impl<'d> Build<'d> {
    /// `node`, an element of the tree read, built with everything inside
    /// it; `top` where it is the root element.
    fn element(&self, node: Node<'_>, top: bool) -> dom::Element<'d> {
        let local = node.local();
        let made = self
            .into
            .create_element(QName::with_namespace_uri(node.namespace(), local));
        if self.written {
            Self::as_written(made, node, top);
        }
        for attribute in node.attributes() {
            let name = QName::with_namespace_uri(attribute.namespace(), attribute.local());
            let value = made.set_attribute_value(name, attribute.value());
            if self.written && !attribute.prefix().is_empty() {
                value.set_preferred_prefix(Some(attribute.prefix()));
            }
        }
        for child in node.children() {
            if child.is_element() {
                made.append_child(self.element(child, false));
            } else if child.is_text() {
                made.append_child(self.into.create_text(child.text().unwrap_or("")));
            } else if child.is_comment() {
                made.append_child(self.into.create_comment(child.text().unwrap_or("")));
            } else if child.is_instruction() {
                made.append_child(self.instruction(child));
            }
        }
        made
    }

    /// `made` given the prefix `node` is written with and the namespaces
    /// it declares, as the engine's own parser gives them.
    fn as_written(made: dom::Element<'d>, node: Node<'_>, top: bool) {
        if !node.prefix().is_empty() {
            made.set_preferred_prefix(Some(node.prefix()));
        }
        for (prefix, uri) in node.declarations() {
            if prefix.is_empty() {
                made.set_default_namespace_uri(Some(uri).filter(|uri| !uri.is_empty()));
            } else {
                made.register_prefix(prefix, uri);
            }
        }
        if top {
            made.register_prefix("xml", tree::XML_NAMESPACE);
        }
    }

    fn instruction(&self, node: Node<'_>) -> dom::ProcessingInstruction<'d> {
        let data = node.text().filter(|data| !data.is_empty());
        self.into.create_processing_instruction(node.name(), data)
    }
}

impl Form for Document {
    fn parse(stream: &Stream) -> Result<Self, ContractError> {
        Self::parse(path::text(stream)?)
    }
}

impl Editable for Document {
    const MEDIA_TYPE: &'static str = "application/xml";

    fn open(bytes: &[u8]) -> Result<Self, ContractError> {
        let text = std::str::from_utf8(bytes)
            .map_err(|error| ContractError::new(format!("not UTF-8 text: {error}")))?;
        Self::built(text, true)
    }

    fn into_bytes(self) -> Result<Vec<u8>, ContractError> {
        let mut bytes = Vec::new();
        sxd_document::writer::format_document(&self.package.as_document(), &mut bytes)
            .map_err(|error| ContractError::new(format!("cannot serialize XML: {error}")))?;
        Ok(bytes)
    }
}

impl Document {
    /// The element or attribute `node` selected in this document, written.
    pub(crate) fn write(
        node: sxd_xpath::nodeset::Node<'_>,
        text: &str,
        expression: &str,
    ) -> Result<(), ContractError> {
        use sxd_xpath::nodeset::Node as Selected;
        match node {
            Selected::Element(element) => {
                element.set_text(text);
                Ok(())
            }
            Selected::Attribute(attribute) => {
                let owner = attribute.parent().ok_or_else(|| {
                    ContractError::new(format!("{expression:?} selects an orphaned attribute"))
                })?;
                owner.set_attribute_value(attribute.name(), text);
                Ok(())
            }
            _ => Err(ContractError::new(format!(
                "{expression:?} selects neither an element nor an attribute"
            ))),
        }
    }
}
