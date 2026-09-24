#![forbid(unsafe_code)]

//! The `XPath` path technology — a technology of `xmip-core-path`.
//!
//! Three things, because a path language is nothing without content to address:
//! [`XPathEngine`], the [`PathEngine`] for the language `xpath`;
//! [`XmlStructure`], a [`StructureReader`] over an XML Stream; and
//! [`XmlRewrite`], a [`StructureWriter`] that produces a new Stream with one or
//! more values replaced, as ADR-0013 asks of anything that changes content.
//! Promote reads through the first two; demote writes through the first and
//! third; route and process read.
//!
//! `XPath` is 1.0. An expression that selects nodes yields the first node's
//! string value; one that computes yields its number, boolean or string.
//! Namespaces bind through the prefixes the document itself declares. A write
//! replaces an element's text or an attribute's value; it does not create
//! either, because demote names a place rather than inventing structure.

use contract::{
    ContractDescriptor, ContractError, ContractId, StructureReader, StructureWriter,
    StructuredValue,
};
use path::{Path, PathCost, PathEngine};
use stream::Stream;
use sxd_document::Package;
use sxd_xpath::nodeset::Node;
use sxd_xpath::{Context, Factory, Value};
use xcore::StreamId;

/// The `xpath` engine. The reader speaks `XPath` already, so the engine adds
/// no traversal of its own.
pub struct XPathEngine;

impl PathEngine for XPathEngine {
    fn language(&self) -> &'static str {
        "xpath"
    }

    fn read(
        &self,
        reader: &dyn StructureReader,
        path: &Path,
    ) -> Result<Option<StructuredValue>, ContractError> {
        reader.read(&path.expression)
    }

    fn write(
        &self,
        writer: &mut dyn StructureWriter,
        path: &Path,
        value: StructuredValue,
    ) -> Result<(), ContractError> {
        writer.write(&path.expression, value)
    }

    /// A document is parsed whole before any axis can be walked.
    fn cost(&self, _path: &Path) -> PathCost {
        PathCost::Materialized
    }
}

fn descriptor() -> ContractDescriptor {
    ContractDescriptor {
        id: ContractId("xml-schema".to_string()),
        version: "1".to_string(),
        representation: "application/xml".to_string(),
    }
}

/// The DOM is not thread-safe and a structure reader is, so the text is what
/// is kept and the DOM is built per operation. `XPath` is `Materialized`
/// anyway; this is the same parse, done where the answer is needed.
fn text_of(stream: &Stream) -> Result<String, ContractError> {
    let text = std::str::from_utf8(stream.bytes()).map_err(|error| ContractError {
        message: format!("not UTF-8 text: {error}"),
    })?;
    parse(text)?;
    Ok(text.to_string())
}

fn parse(text: &str) -> Result<Package, ContractError> {
    sxd_document::parser::parse(text).map_err(|error| ContractError {
        message: format!("not well-formed XML: {error}"),
    })
}

/// Evaluate `expression` over the document with the document's own prefixes.
fn evaluate<'d>(package: &'d Package, expression: &str) -> Result<Value<'d>, ContractError> {
    let document = package.as_document();
    let compiled = Factory::new()
        .build(expression)
        .map_err(|error| ContractError {
            message: format!("XPath {expression:?}: {error}"),
        })?
        .ok_or_else(|| ContractError {
            message: format!("XPath {expression:?} is empty"),
        })?;
    let mut context = Context::new();
    if let Some(root) = document
        .root()
        .children()
        .into_iter()
        .find_map(sxd_document::dom::ChildOfRoot::element)
    {
        for namespace in root.namespaces_in_scope() {
            context.set_namespace(namespace.prefix(), namespace.uri());
        }
    }
    compiled
        .evaluate(&context, document.root())
        .map_err(|error| ContractError {
            message: format!("XPath {expression:?}: {error}"),
        })
}

/// An XML Stream, read by `XPath`.
pub struct XmlStructure {
    descriptor: ContractDescriptor,
    text: String,
}

impl XmlStructure {
    /// Parse `stream` once; every read is an evaluation after that.
    ///
    /// # Errors
    /// The Stream is not well-formed XML.
    pub fn parse(stream: &Stream) -> Result<Self, ContractError> {
        Ok(Self {
            descriptor: descriptor(),
            text: text_of(stream)?,
        })
    }
}

impl StructureReader for XmlStructure {
    fn contract(&self) -> &ContractDescriptor {
        &self.descriptor
    }

    fn read(&self, path: &str) -> Result<Option<StructuredValue>, ContractError> {
        let package = parse(&self.text)?;
        Ok(match evaluate(&package, path)? {
            Value::Nodeset(nodes) => nodes
                .document_order_first()
                .map(|node| StructuredValue::Text(node.string_value())),
            Value::Boolean(flag) => Some(StructuredValue::Bool(flag)),
            // Whole and within i64: the guard makes the cast exact.
            #[allow(clippy::cast_possible_truncation)]
            Value::Number(number) if number.fract() == 0.0 && number.abs() < 9.0e15 => {
                Some(StructuredValue::Integer(number as i64))
            }
            Value::Number(number) => Some(StructuredValue::Decimal(number)),
            Value::String(text) => Some(StructuredValue::Text(text)),
        })
    }
}

/// An XML Stream being rewritten into a new one.
pub struct XmlRewrite {
    descriptor: ContractDescriptor,
    id: StreamId,
    text: String,
}

impl XmlRewrite {
    /// Start from `stream`; the Stream `finish` produces carries `id`.
    ///
    /// # Errors
    /// The Stream is not well-formed XML.
    pub fn of(stream: &Stream, id: StreamId) -> Result<Self, ContractError> {
        Ok(Self {
            descriptor: descriptor(),
            id,
            text: text_of(stream)?,
        })
    }
}

impl StructureWriter for XmlRewrite {
    fn contract(&self) -> &ContractDescriptor {
        &self.descriptor
    }

    /// Replace the text of the first element `path` selects, or the value of
    /// the first attribute. Anything else selected, or nothing, is refused.
    fn write(&mut self, path: &str, value: StructuredValue) -> Result<(), ContractError> {
        let text = lexical(value)?;
        let package = parse(&self.text)?;
        let Value::Nodeset(nodes) = evaluate(&package, path)? else {
            return Err(ContractError {
                message: format!("{path:?} computes a value, it does not select a place"),
            });
        };
        match nodes.document_order_first() {
            Some(Node::Element(element)) => {
                element.set_text(&text);
            }
            Some(Node::Attribute(attribute)) => {
                let owner = attribute.parent().ok_or_else(|| ContractError {
                    message: format!("{path:?} selects an orphaned attribute"),
                })?;
                owner.set_attribute_value(attribute.name(), &text);
            }
            Some(_) => {
                return Err(ContractError {
                    message: format!("{path:?} selects neither an element nor an attribute"),
                });
            }
            None => {
                return Err(ContractError {
                    message: format!("{path:?} selects nothing to write"),
                });
            }
        }
        self.text = serialise(&package)?;
        Ok(())
    }

    fn finish(self: Box<Self>) -> Result<Stream, ContractError> {
        Ok(Stream::new(
            self.id,
            self.text.into_bytes(),
            Some(self.descriptor.representation),
        ))
    }
}

fn serialise(package: &Package) -> Result<String, ContractError> {
    let mut bytes = Vec::new();
    sxd_document::writer::format_document(&package.as_document(), &mut bytes).map_err(|error| {
        ContractError {
            message: format!("cannot serialise XML: {error}"),
        }
    })?;
    String::from_utf8(bytes).map_err(|error| ContractError {
        message: format!("cannot serialise XML: {error}"),
    })
}

/// The text a value is written as. Binary has no lexical form here.
fn lexical(value: StructuredValue) -> Result<String, ContractError> {
    Ok(match value {
        StructuredValue::Null => String::new(),
        StructuredValue::Bool(flag) => flag.to_string(),
        StructuredValue::Integer(integer) => integer.to_string(),
        StructuredValue::Decimal(decimal) => decimal.to_string(),
        StructuredValue::Text(text) => text,
        StructuredValue::Binary(_) => {
            return Err(ContractError {
                message: "binary has no XML text form here".to_string(),
            });
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use path::fixture::stream;

    const ORDER: &str = r#"<o:order xmlns:o="urn:example:order" currency="SEK">
  <o:id>A1</o:id><o:line><o:sku>X</o:sku><o:qty>2</o:qty></o:line>
  <o:line><o:sku>Y</o:sku><o:qty>3</o:qty></o:line></o:order>"#;

    #[test]
    fn reads_the_first_node_or_the_computed_value() {
        let structure = XmlStructure::parse(&stream(ORDER)).expect("parses");
        let engine = XPathEngine;
        let read = |p: &str| engine.read(&structure, &Path::new("xpath", p));
        assert_eq!(
            read("/o:order/o:id").expect("reads"),
            Some(StructuredValue::Text("A1".into()))
        );
        assert_eq!(
            read("/o:order/@currency").expect("reads"),
            Some(StructuredValue::Text("SEK".into()))
        );
        assert_eq!(
            read("count(//o:line)").expect("reads"),
            Some(StructuredValue::Integer(2))
        );
        assert_eq!(
            read("sum(//o:qty) div 4").expect("reads"),
            Some(StructuredValue::Decimal(1.25))
        );
        assert_eq!(
            read("//o:qty > 1").expect("reads"),
            Some(StructuredValue::Bool(true))
        );
        assert_eq!(read("//o:nowhere").expect("reads"), None);
        assert!(read("count(").is_err());
        assert_eq!(
            engine.cost(&Path::new("xpath", "/a")),
            PathCost::Materialized
        );
    }

    #[test]
    fn rewrites_element_text_and_attribute_values_into_a_new_stream() {
        let mut rewrite = XmlRewrite::of(&stream(ORDER), StreamId::new(2)).expect("parses");
        let engine = XPathEngine;
        engine
            .write(
                &mut rewrite,
                &Path::new("xpath", "//o:line[2]/o:qty"),
                StructuredValue::Integer(9),
            )
            .expect("writes");
        engine
            .write(
                &mut rewrite,
                &Path::new("xpath", "/o:order/@currency"),
                StructuredValue::Text("EUR".into()),
            )
            .expect("writes");
        assert!(rewrite.write("//o:nowhere", StructuredValue::Null).is_err());
        assert!(
            rewrite
                .write("count(//o:line)", StructuredValue::Null)
                .is_err()
        );
        let out = Box::new(rewrite).finish().expect("finishes");
        assert_eq!(out.id(), StreamId::new(2));
        let back = XmlStructure::parse(&out).expect("parses");
        assert_eq!(
            back.read("//o:line[2]/o:qty").expect("reads"),
            Some(StructuredValue::Text("9".into()))
        );
        assert_eq!(
            back.read("/o:order/@currency").expect("reads"),
            Some(StructuredValue::Text("EUR".into()))
        );
        assert_eq!(
            back.read("/o:order/o:id").expect("reads"),
            Some(StructuredValue::Text("A1".into()))
        );
    }

    #[test]
    fn a_stream_that_is_not_xml_is_refused_up_front() {
        assert!(XmlStructure::parse(&stream("<a><b></a>")).is_err());
        assert!(XmlRewrite::of(&stream("<a><b></a>"), StreamId::new(1)).is_err());
    }
}
