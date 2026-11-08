#![forbid(unsafe_code)]

//! The `XPath` path technology — a technology of `xmip-core-path`.
//!
//! [`XPathLanguage`] is the [`PathLanguage`] `xpath`: it compiles an
//! expression once, and the compiled expression reads one value from an XML
//! Stream and writes one into a rewrite of it (ADR-0013). Promote reads
//! through it; demote writes through it; route and process read.
//!
//! `XPath` is 1.0. An expression that selects nodes yields the first node's
//! string value; one that computes yields its number, boolean or string.
//! A prefix binds through the Path's `namespaces` and never through the
//! document (ADR-0066, amendment 2026-11-08): `/o:order` reads the element
//! `order` in the URI the configuration gave `o`, whatever prefix a
//! document writes for that URI and wherever in it the declaration is, and
//! a prefix the table does not bind is refused as the expression compiles.
//! A write replaces an element's text or an attribute's value; it does not
//! create either, because demote names a place rather than inventing
//! structure.
//!
//! **Once, and where.** The document is parsed once per Message, and every
//! expression reading that Message evaluates over it; a rewrite parses once
//! and serializes once, at the end. The DOM and a compiled expression are
//! not thread-safe, so the expression is checked when configuration
//! compiles it and built, with the evaluation context its namespaces bind,
//! once per thread that evaluates it, then kept there.

mod prefix;

use contract::ContractError;
use path::{CompiledExpression, Content, Editable, Form, Namespaces, PathLanguage, Rewriting};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use stream::Stream;
use sxd_document::Package;
use sxd_xpath::nodeset::Node;
use sxd_xpath::{Context, Factory, Value, XPath};
use xcore::ScalarValue;

/// The language `xpath`.
pub struct XPathLanguage;

impl PathLanguage for XPathLanguage {
    fn language(&self) -> &'static str {
        "xpath"
    }

    /// Checked now, so a configuration naming an expression that does not
    /// compile, or a prefix its namespaces do not bind, is refused as it is
    /// read.
    fn compile(
        &self,
        expression: &str,
        namespaces: &Namespaces,
    ) -> Result<Box<dyn CompiledExpression>, ContractError> {
        build(expression)?;
        let unbound: Vec<&str> = prefix::used(expression)
            .into_iter()
            .filter(|prefix| namespaces.uri(prefix).is_none())
            .collect();
        if !unbound.is_empty() {
            return Err(ContractError::new(format!(
                "XPath {expression:?} uses the prefix {}, which the path's namespaces do not bind",
                unbound.join(", ")
            )));
        }
        Ok(Box::new(Compiled {
            expression: expression.to_string(),
            namespaces: namespaces.clone(),
        }))
    }
}

#[cfg(test)]
thread_local! {
    /// How often this thread built an expression, for the test that holds
    /// the once.
    static BUILT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn build(expression: &str) -> Result<XPath, ContractError> {
    #[cfg(test)]
    BUILT.with(|built| built.set(built.get() + 1));
    Factory::new()
        .build(expression)
        .map_err(|error| ContractError::new(format!("XPath {expression:?}: {error}")))?
        .ok_or_else(|| ContractError::new(format!("XPath {expression:?} is empty")))
}

/// An expression built, and the evaluation context its namespaces bind.
struct Built {
    xpath: XPath,
    context: Context<'static>,
}

/// One expression's builds, one per namespaces it was compiled with.
type Builds = Vec<(Namespaces, Rc<Built>)>;

thread_local! {
    /// Every expression this thread has evaluated, built once per
    /// namespaces it was compiled with.
    static BUILT_HERE: RefCell<HashMap<String, Builds>> = RefCell::new(HashMap::new());
}

/// An expression checked at configuration, built on first use per thread.
struct Compiled {
    expression: String,
    namespaces: Namespaces,
}

impl Compiled {
    fn built(&self) -> Result<Rc<Built>, ContractError> {
        BUILT_HERE.with(|here| {
            if let Some((_, found)) = here.borrow().get(&self.expression).and_then(|built| {
                built
                    .iter()
                    .find(|(namespaces, _)| *namespaces == self.namespaces)
            }) {
                return Ok(Rc::clone(found));
            }
            let mut context = Context::new();
            for (prefix, uri) in self.namespaces.iter() {
                context.set_namespace(prefix, uri);
            }
            let built = Rc::new(Built {
                xpath: build(&self.expression)?,
                context,
            });
            here.borrow_mut()
                .entry(self.expression.clone())
                .or_default()
                .push((self.namespaces.clone(), Rc::clone(&built)));
            Ok(built)
        })
    }

    fn evaluate<'d>(&self, xml: &'d Xml) -> Result<Value<'d>, ContractError> {
        let built = self.built()?;
        built
            .xpath
            .evaluate(&built.context, xml.package.as_document().root())
            .map_err(|error| ContractError::new(format!("XPath {:?}: {error}", self.expression)))
    }
}

impl CompiledExpression for Compiled {
    fn read(&self, content: &Content<'_>) -> Result<Option<ScalarValue>, ContractError> {
        let xml = content.form::<Xml>()?;
        Ok(match self.evaluate(&xml)? {
            Value::Nodeset(nodes) => nodes
                .document_order_first()
                .map(|node| ScalarValue::Text(node.string_value())),
            Value::Boolean(flag) => Some(ScalarValue::Bool(flag)),
            // Whole and within i64: the guard makes the cast exact.
            #[allow(clippy::cast_possible_truncation)]
            Value::Number(number) if number.fract() == 0.0 && number.abs() < 9.0e15 => {
                Some(ScalarValue::Integer(number as i64))
            }
            Value::Number(number) => Some(ScalarValue::Decimal(number)),
            Value::String(text) => Some(ScalarValue::Text(text)),
        })
    }

    /// Replace the text of the first element the expression selects, or the
    /// value of the first attribute; a Null writes empty text. Anything else
    /// selected, or nothing, is refused.
    fn write(&self, rewriting: &mut Rewriting, value: ScalarValue) -> Result<(), ContractError> {
        let path = &self.expression;
        let text = match &value {
            ScalarValue::Null => String::new(),
            other => other
                .text()
                .ok_or_else(|| ContractError::new("binary has no XML text form here"))?
                .into_owned(),
        };
        let xml = rewriting.form_mut::<Xml>()?;
        let Value::Nodeset(nodes) = self.evaluate(xml)? else {
            return Err(ContractError::new(format!(
                "{path:?} computes a value, it does not select a place"
            )));
        };
        match nodes.document_order_first() {
            Some(Node::Element(element)) => {
                element.set_text(&text);
            }
            Some(Node::Attribute(attribute)) => {
                let owner = attribute.parent().ok_or_else(|| {
                    ContractError::new(format!("{path:?} selects an orphaned attribute"))
                })?;
                owner.set_attribute_value(attribute.name(), &text);
            }
            Some(_) => {
                return Err(ContractError::new(format!(
                    "{path:?} selects neither an element nor an attribute"
                )));
            }
            None => {
                return Err(ContractError::new(format!(
                    "{path:?} selects nothing to write"
                )));
            }
        }
        Ok(())
    }
}

/// An XML document, parsed once. Its own prefixes bind nothing an
/// expression reads.
struct Xml {
    package: Package,
}

impl Xml {
    fn parse(text: &str) -> Result<Self, ContractError> {
        let package = sxd_document::parser::parse(text)
            .map_err(|error| ContractError::new(format!("not well-formed XML: {error}")))?;
        Ok(Self { package })
    }
}

impl Form for Xml {
    fn parse(stream: &Stream) -> Result<Self, ContractError> {
        let text = stream
            .text()
            .map_err(|error| ContractError::new(format!("not UTF-8 text: {error}")))?;
        Self::parse(text)
    }
}

impl Editable for Xml {
    const MEDIA_TYPE: &'static str = "application/xml";

    fn open(bytes: &[u8]) -> Result<Self, ContractError> {
        let text = std::str::from_utf8(bytes)
            .map_err(|error| ContractError::new(format!("not UTF-8 text: {error}")))?;
        Self::parse(text)
    }

    fn into_bytes(self) -> Result<Vec<u8>, ContractError> {
        let mut bytes = Vec::new();
        sxd_document::writer::format_document(&self.package.as_document(), &mut bytes)
            .map_err(|error| ContractError::new(format!("cannot serialize XML: {error}")))?;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use contract::fixture::stream;
    use xcore::StreamId;

    const ORDER: &str = r#"<o:order xmlns:o="urn:example:order" currency="SEK">
  <o:id>A1</o:id><o:line><o:sku>X</o:sku><o:qty>2</o:qty></o:line>
  <o:line><o:sku>Y</o:sku><o:qty>3</o:qty></o:line></o:order>"#;

    fn order_namespaces() -> Namespaces {
        Namespaces::new().bind("o", "urn:example:order")
    }

    fn compiled(expression: &str) -> Result<Box<dyn CompiledExpression>, ContractError> {
        XPathLanguage.compile(expression, &order_namespaces())
    }

    fn id_in(document: &str) -> Option<ScalarValue> {
        let order = stream(document);
        compiled("/o:order/o:id")
            .expect("compiles")
            .read(&Content::of(&order))
            .expect("reads")
    }

    #[test]
    fn a_prefix_means_the_uri_the_configuration_binds_it_to_never_the_documents() {
        // Another prefix for the right URI matches.
        let other_prefix = ORDER
            .replace("o:", "ord:")
            .replace("xmlns:o=", "xmlns:ord=");
        assert_eq!(id_in(&other_prefix), Some(ScalarValue::Text("A1".into())));
        // The default namespace for the right URI matches.
        let default = r#"<order xmlns="urn:example:order"><id>A1</id></order>"#;
        assert_eq!(id_in(default), Some(ScalarValue::Text("A1".into())));
        // The same prefix bound to another URI does not.
        let other_uri = ORDER.replace("urn:example:order", "urn:example:invoice");
        assert_eq!(id_in(&other_uri), None);
        // And no namespace at all does not either.
        assert_eq!(id_in("<order><id>A1</id></order>"), None);
    }

    #[test]
    fn a_namespace_declared_below_the_root_is_found() {
        let below = r#"<envelope><body>
  <o:order xmlns:o="urn:example:order"><o:id>A1</o:id></o:order></body></envelope>"#;
        let order = stream(below);
        assert_eq!(
            compiled("/envelope/body/o:order/o:id")
                .expect("compiles")
                .read(&Content::of(&order))
                .expect("reads"),
            Some(ScalarValue::Text("A1".into()))
        );
    }

    #[test]
    fn a_prefix_the_namespaces_do_not_bind_is_refused_as_it_compiles() {
        let refused = compiled("/o:order/p:id[@q:x = 'r:s']")
            .map(drop)
            .expect_err("refused");
        assert_eq!(
            refused.message,
            "XPath \"/o:order/p:id[@q:x = 'r:s']\" uses the prefix p, q, which the \
             path's namespaces do not bind"
        );
        let refused = XPathLanguage
            .compile("/o:order", &Namespaces::new())
            .map(drop)
            .expect_err("refused");
        assert!(
            refused.message.contains("the prefix o,"),
            "{}",
            refused.message
        );
        assert!(XPathLanguage.compile("/order", &Namespaces::new()).is_ok());
    }

    #[test]
    fn one_expression_under_two_bindings_is_built_once_for_each() {
        let order = stream(ORDER);
        let content = Content::of(&order);
        let invoice = Namespaces::new().bind("o", "urn:example:invoice");
        let as_order = compiled("/o:order/o:id").expect("compiles");
        let as_invoice = XPathLanguage
            .compile("/o:order/o:id", &invoice)
            .expect("compiles");
        assert_eq!(
            as_order.read(&content).expect("reads"),
            Some(ScalarValue::Text("A1".into()))
        );
        assert_eq!(as_invoice.read(&content).expect("reads"), None);
        assert!(BUILT_HERE.with(|here| here.borrow()["/o:order/o:id"].len() == 2));
    }

    #[test]
    fn reads_the_first_node_or_the_computed_value() {
        let order = stream(ORDER);
        let content = Content::of(&order);
        let read = |p: &str| compiled(p)?.read(&content);
        assert_eq!(
            read("/o:order/o:id").expect("reads"),
            Some(ScalarValue::Text("A1".into()))
        );
        assert_eq!(
            read("/o:order/@currency").expect("reads"),
            Some(ScalarValue::Text("SEK".into()))
        );
        assert_eq!(
            read("count(//o:line)").expect("reads"),
            Some(ScalarValue::Integer(2))
        );
        assert_eq!(
            read("sum(//o:qty) div 4").expect("reads"),
            Some(ScalarValue::Decimal(1.25))
        );
        assert_eq!(
            read("//o:qty > 1").expect("reads"),
            Some(ScalarValue::Bool(true))
        );
        assert_eq!(read("//o:nowhere").expect("reads"), None);
        assert!(read("count(").is_err());
    }

    #[test]
    fn an_expression_is_built_once_per_thread_however_many_messages_it_reads() {
        let expression = "/o:order/o:line[2]/o:sku";
        let sku = compiled(expression).expect("compiles");
        let before = BUILT.with(std::cell::Cell::get);
        for _ in 0..1000 {
            let order = stream(ORDER);
            assert_eq!(
                sku.read(&Content::of(&order)).expect("reads"),
                Some(ScalarValue::Text("Y".into()))
            );
        }
        // Checked when compiled, built once more by the first read on this
        // thread, and never again.
        assert_eq!(BUILT.with(std::cell::Cell::get) - before, 1);
        assert!(BUILT_HERE.with(|here| here.borrow()[expression].len() == 1));
    }

    #[test]
    fn rewrites_element_text_and_attribute_values_into_a_new_stream() {
        let mut rewriting = Rewriting::of(&stream(ORDER), StreamId::new(2));
        let mut write =
            |p: &str, v: ScalarValue| compiled(p).and_then(|c| c.write(&mut rewriting, v));
        write("//o:line[2]/o:qty", ScalarValue::Integer(9)).expect("writes");
        write("/o:order/@currency", ScalarValue::Text("EUR".into())).expect("writes");
        assert!(write("//o:nowhere", ScalarValue::Null).is_err());
        assert!(write("count(//o:line)", ScalarValue::Null).is_err());
        assert!(write("/o:order/o:id", ScalarValue::Binary(vec![1])).is_err());
        let out = rewriting.finish().expect("finishes");
        assert_eq!(out.id(), StreamId::new(2));
        assert_eq!(out.media_type(), Some("application/xml"));
        let back = Content::of(&out);
        let read = |p: &str| compiled(p)?.read(&back);
        assert_eq!(
            read("//o:line[2]/o:qty").expect("reads"),
            Some(ScalarValue::Text("9".into()))
        );
        assert_eq!(
            read("/o:order/@currency").expect("reads"),
            Some(ScalarValue::Text("EUR".into()))
        );
        assert_eq!(
            read("/o:order/o:id").expect("reads"),
            Some(ScalarValue::Text("A1".into()))
        );
    }

    #[test]
    fn a_stream_that_is_not_xml_and_an_expression_that_does_not_compile_are_refused() {
        let broken = stream("<a><b></a>");
        let id = compiled("/a").expect("compiles");
        assert!(id.read(&Content::of(&broken)).is_err());
        assert!(
            id.write(
                &mut Rewriting::of(&broken, StreamId::new(1)),
                ScalarValue::Null
            )
            .is_err()
        );
        assert!(compiled("count(").is_err());
        assert!(compiled("").is_err());
    }
}
