#![forbid(unsafe_code)]

//! The `XPath` path technology — a technology of `xmip-core-path`.
//!
//! [`XPathLanguage`] is the [`PathLanguage`] `xpath`: it compiles an
//! expression once, and the compiled expression reads one value from an XML
//! Stream and writes one into a rewrite of it (ADR-0013). Promote reads
//! through it; demote writes through it; route and process read. The
//! Schematron contract evaluates its rules through [`Expression`] and
//! [`Document`], so the estate evaluates `XPath` in one place.
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
//! **Once, and where.** The document is read once per Message by the
//! estate's one XML reader and built into the engine's model
//! ([`Document`]), and every expression reading that Message evaluates over
//! it; a rewrite reads once and serializes once, at the end. The model and
//! a compiled expression are not thread-safe, so an expression is checked
//! when configuration compiles it and built, with the evaluation context
//! its namespaces bind, once per thread that evaluates it, then kept there
//! ([`Expression::ready`]).

mod document;
mod expression;
mod prefix;

use contract::ContractError;
use path::{CompiledExpression, Content, PathLanguage, Rewriting};
use sxd_xpath::Value;
use xcore::ScalarValue;

pub use document::Document;
pub use expression::{Expression, Ready, Selected};
pub use path::Namespaces;

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
        Ok(Box::new(Expression::compile(expression, namespaces)?))
    }
}

impl CompiledExpression for Expression {
    fn read(&self, content: &Content<'_>) -> Result<Option<ScalarValue>, ContractError> {
        let document = content.form::<Document>()?;
        Ok(match self.ready()?.evaluate(document.root())? {
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
        let path = self.text();
        let text = match &value {
            ScalarValue::Null => String::new(),
            other => other
                .text()
                .ok_or_else(|| ContractError::new("binary has no XML text form here"))?
                .into_owned(),
        };
        let document = rewriting.form_mut::<Document>()?;
        let Value::Nodeset(nodes) = self.ready()?.evaluate(document.root())? else {
            return Err(ContractError::new(format!(
                "{path:?} computes a value, it does not select a place"
            )));
        };
        let node = nodes
            .document_order_first()
            .ok_or_else(|| ContractError::new(format!("{path:?} selects nothing to write")))?;
        Document::write(node, &text, path)
    }
}

#[cfg(test)]
mod tests {
    use super::expression::{BUILT, BUILT_HERE};
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
        let doctype = stream("<!DOCTYPE a [<!ENTITY x \"y\">]><a>&x;</a>");
        assert!(id.read(&Content::of(&doctype)).is_err());
    }

    #[test]
    fn selected_nodes_are_located_and_a_test_holds_at_each() {
        let document = Document::parse(ORDER).expect("parses");
        let ready = |text: &str| Expression::compile(text, &order_namespaces()).expect("compiles");
        let lines = ready("//o:line");
        let found = lines
            .ready()
            .expect("ready")
            .select(&document)
            .expect("selects");
        let at: Vec<String> = found.iter().map(|node| node.location()).collect();
        assert_eq!(at, ["/order/line[1]", "/order/line[2]"]);
        let more = ready("o:qty > 2");
        let more = more.ready().expect("ready");
        let held: Vec<bool> = found
            .iter()
            .map(|node| more.holds(*node).expect("holds"))
            .collect();
        assert_eq!(held, [false, true]);
        let currency = ready("/o:order/@currency");
        let currency = currency.ready().expect("ready").select(&document);
        assert_eq!(currency.expect("selects")[0].location(), "/order/@currency");
        let counted = ready("count(//o:line)");
        let selected = counted
            .ready()
            .expect("ready")
            .select(&document)
            .expect("selects");
        assert_eq!(selected.len(), 0, "a computed value selects nothing");
    }

    #[test]
    fn a_document_written_back_keeps_its_prefixes_namespaces_comments_and_instructions() {
        use codec::xml::tree::Node;
        let text = "<?xml version=\"1.0\"?><!-- head --><o:order xmlns:o=\"urn:example:order\" \
                    xmlns=\"urn:d\" xmlns:x=\"urn:x\" x:flag=\"1\"><line>t&amp;u</line>\
                    <?keep me?></o:order>";
        let bytes = <Document as path::Editable>::open(text.as_bytes())
            .and_then(path::Editable::into_bytes)
            .expect("writes");
        let back = String::from_utf8(bytes).expect("UTF-8");
        let read = codec::xml::tree::Document::parse(&back).expect("reads back");
        let root = read.root_element();
        assert_eq!(
            (root.name(), root.namespace()),
            ("o:order", Some("urn:example:order"))
        );
        assert_eq!(root.attribute_in("urn:x", "flag"), Some("1"), "{back}");
        let line = root.elements().next().expect("line");
        assert_eq!(
            (line.namespace(), line.text()),
            (Some("urn:d"), Some("t&u"))
        );
        assert!(read.root().children().any(Node::is_comment), "{back}");
        assert!(root.children().any(|node| node.name() == "keep"), "{back}");
    }
}
