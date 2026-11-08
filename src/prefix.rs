//! The prefixes an `XPath` 1.0 expression uses, read from its text.
//!
//! The engine resolves a prefix only as it tests a node, and stops the
//! thread on one it cannot resolve, so every prefix is checked against the
//! Path's namespaces when the expression is compiled (ADR-0066, amendment
//! 2026-11-08). A prefix is the name before a single `:` in a name test, a
//! function's name or a variable's; an axis is followed by `::`, and a
//! literal holds no names.

use std::collections::BTreeSet;

/// Every prefix `expression` uses, once each, in order.
pub fn used(expression: &str) -> BTreeSet<&str> {
    let mut prefixes = BTreeSet::new();
    let mut rest = expression;
    while let Some(first) = rest.chars().next() {
        if first == '\'' || first == '"' {
            rest = rest[1..].find(first).map_or("", |end| &rest[end + 2..]);
        } else if starts_name(first) {
            let length = rest.find(|c: char| !in_name(c)).unwrap_or(rest.len());
            let (name, after) = rest.split_at(length);
            if after.starts_with(':') && !after.starts_with("::") {
                prefixes.insert(name);
                rest = &after[1..];
            } else {
                rest = after;
            }
        } else {
            rest = &rest[first.len_utf8()..];
        }
    }
    prefixes
}

/// Whether a name begins with `c` (XML's `NCName`, its first character).
fn starts_name(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

/// Whether `c` continues a name (XML's `NCName`, its other characters).
fn in_name(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '\u{b7}')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefixes(expression: &str) -> Vec<&str> {
        used(expression).into_iter().collect()
    }

    #[test]
    fn names_tests_functions_and_variables_carry_a_prefix_and_axes_and_literals_do_not() {
        assert_eq!(prefixes("/o:order/o:line[2]/p:sku"), ["o", "p"]);
        assert_eq!(prefixes("child::o:order/attribute::id"), ["o"]);
        assert_eq!(prefixes("//o:*[@x:id = 'a:b']"), ["o", "x"]);
        assert_eq!(prefixes("f:total($v:rate) div 2"), ["f", "v"]);
        assert_eq!(
            prefixes("count(//line) > 1.5 and \"q:r\" != ''"),
            [] as [&str; 0]
        );
        assert_eq!(prefixes("/ö-1:a"), ["ö-1"]);
        assert_eq!(prefixes("'unterminated a:b"), [] as [&str; 0]);
    }
}
