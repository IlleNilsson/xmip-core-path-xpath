//! An `XPath` 1.0 expression: checked once where configuration gives it,
//! built once per thread that evaluates it and kept there, because the
//! engine's compiled form and its evaluation context cannot move between
//! threads. The one implementation every evaluation in the estate goes
//! through: promote's and demote's paths (`crate::XPathLanguage`) and a
//! Schematron's rules.
//!
//! A prefix binds through the namespaces it was compiled with and never
//! through the document (ADR-0066, amendment 2026-11-08), and one the
//! namespaces do not bind is refused as it compiles.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use contract::ContractError;
use path::Namespaces;
use sxd_xpath::nodeset::Node;
use sxd_xpath::{Context, Factory, Value, XPath};

use crate::document::Document;
use crate::prefix;

/// An expression checked, with the namespaces its prefixes bind through.
#[derive(Clone, Debug)]
pub struct Expression {
    text: String,
    namespaces: Namespaces,
}

#[cfg(test)]
thread_local! {
    /// How often this thread built an expression, for the test that holds
    /// the once.
    pub(crate) static BUILT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn build(text: &str) -> Result<XPath, ContractError> {
    #[cfg(test)]
    BUILT.with(|built| built.set(built.get() + 1));
    Factory::new()
        .build(text)
        .map_err(|error| ContractError::new(format!("XPath {text:?}: {error}")))?
        .ok_or_else(|| ContractError::new(format!("XPath {text:?} is empty")))
}

/// An expression built, and the evaluation context its namespaces bind.
pub(crate) struct Built {
    xpath: XPath,
    context: Context<'static>,
}

/// One expression's builds, one per namespaces it was compiled with.
pub(crate) type Builds = Vec<(Namespaces, Rc<Built>)>;

thread_local! {
    /// Every expression this thread has evaluated, built once per
    /// namespaces it was compiled with.
    pub(crate) static BUILT_HERE: RefCell<HashMap<String, Builds>> =
        RefCell::new(HashMap::new());
}

impl Expression {
    /// `text` checked: it compiles, and every prefix it uses `namespaces`
    /// binds.
    ///
    /// # Errors
    /// It does not compile, or uses a prefix `namespaces` does not bind.
    pub fn compile(text: &str, namespaces: &Namespaces) -> Result<Self, ContractError> {
        build(text)?;
        let unbound: Vec<&str> = prefix::used(text)
            .into_iter()
            .filter(|prefix| namespaces.uri(prefix).is_none())
            .collect();
        if !unbound.is_empty() {
            return Err(ContractError::new(format!(
                "XPath {text:?} uses the prefix {}, which the path's namespaces do not bind",
                unbound.join(", ")
            )));
        }
        Ok(Self {
            text: text.to_string(),
            namespaces: namespaces.clone(),
        })
    }

    /// The expression as written.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The expression built on this thread, built here the first time.
    ///
    /// # Errors
    /// It no longer compiles, which an expression [`Expression::compile`]
    /// checked never does.
    pub fn ready(&self) -> Result<Ready<'_>, ContractError> {
        BUILT_HERE.with(|here| {
            let found = here.borrow().get(&self.text).and_then(|builds| {
                builds
                    .iter()
                    .find(|(namespaces, _)| *namespaces == self.namespaces)
                    .map(|(_, built)| Rc::clone(built))
            });
            if let Some(built) = found {
                return Ok(Ready {
                    built,
                    expression: self,
                });
            }
            let mut context = Context::new();
            for (prefix, uri) in self.namespaces.iter() {
                context.set_namespace(prefix, uri);
            }
            let built = Rc::new(Built {
                xpath: build(&self.text)?,
                context,
            });
            here.borrow_mut()
                .entry(self.text.clone())
                .or_default()
                .push((self.namespaces.clone(), Rc::clone(&built)));
            Ok(Ready {
                built,
                expression: self,
            })
        })
    }
}

/// An expression built on this thread, ready to evaluate; it stays on the
/// thread that built it.
pub struct Ready<'e> {
    built: Rc<Built>,
    expression: &'e Expression,
}

impl Ready<'_> {
    /// Its value at `at`.
    pub(crate) fn evaluate<'d>(&self, at: Node<'d>) -> Result<Value<'d>, ContractError> {
        self.built
            .xpath
            .evaluate(&self.built.context, at)
            .map_err(|error| {
                ContractError::new(format!("XPath {:?}: {error}", self.expression.text))
            })
    }

    /// The nodes it selects from `document`'s root, in document order;
    /// none where it computes a value.
    ///
    /// # Errors
    /// It cannot be evaluated over this document.
    pub fn select<'d>(&self, document: &'d Document) -> Result<Vec<Selected<'d>>, ContractError> {
        Ok(match self.evaluate(document.root())? {
            Value::Nodeset(nodes) => nodes.document_order().into_iter().map(Selected).collect(),
            _ => Vec::new(),
        })
    }

    /// Whether it holds at `at`: its value as `XPath`'s `boolean()` reads
    /// it.
    ///
    /// # Errors
    /// It cannot be evaluated there.
    pub fn holds(&self, at: Selected<'_>) -> Result<bool, ContractError> {
        Ok(self.evaluate(at.0)?.boolean())
    }
}

/// A node an expression selected in a [`Document`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Selected<'d>(Node<'d>);

impl Selected<'_> {
    /// Where it is, as an `XPath` an operator can paste: `/Invoice/Line[2]`,
    /// `/Invoice/@currency`; an ordinal only where a name repeats.
    #[must_use]
    pub fn location(self) -> String {
        let mut steps = Vec::new();
        let mut current = Some(self.0);
        while let Some(here) = current {
            match here {
                Node::Element(element) => steps.push(step(element)),
                Node::Attribute(attribute) => {
                    steps.push(format!("@{}", attribute.name().local_part()));
                }
                Node::Root(_) => break,
                _ => {}
            }
            current = here.parent();
        }
        steps.reverse();
        format!("/{}", steps.join("/"))
    }
}

fn step(element: sxd_document::dom::Element<'_>) -> String {
    let name = element.name().local_part();
    let Some(parent) = element
        .parent()
        .and_then(sxd_document::dom::ParentOfChild::element)
    else {
        return name.to_string();
    };
    let siblings: Vec<sxd_document::dom::Element<'_>> = parent
        .children()
        .into_iter()
        .filter_map(sxd_document::dom::ChildOfElement::element)
        .filter(|sibling| sibling.name().local_part() == name)
        .collect();
    if siblings.len() == 1 {
        return name.to_string();
    }
    let ordinal = siblings
        .iter()
        .position(|sibling| *sibling == element)
        .map_or(1, |index| index + 1);
    format!("{name}[{ordinal}]")
}
