//! Handwritten: Rcore's reserved and contextual words, and the single rule for
//! writing a name so that both the structure and behavior parsers read it back
//! as that exact name.
//!
//! A bare word is a name unless a parser reserves it or recognizes it at that
//! position; a backtick-quoted name is always a name and is never resolved as a
//! built-in scalar. Generated Rcore therefore quotes a name only when the bare
//! word could be read differently.

/// Words the structure parser rejects as unquoted identifiers.
pub(crate) const STRUCTURE_RESERVED: &[&str] = &[
    "rcore",
    "package",
    "version",
    "metamodel",
    "prefix",
    "class",
    "enum",
    "abstract",
    "extends",
    "attr",
    "ref",
    "opposite",
    "unordered",
    "nonunique",
    "containment",
    "op",
    "default",
    "derived",
];

/// Words the behavior parser rejects as unquoted identifiers.
pub(crate) const BEHAVIOR_RESERVED: &[&str] = &[
    "rcore",
    "package",
    "ecore",
    "version",
    "import",
    "model",
    "as",
    "pub",
    "private",
    "fn",
    "derive",
    "validate",
    "require",
    "code",
    "message",
    "let",
    "if",
    "else",
    "match",
    "true",
    "false",
    "none",
    "for",
    "in",
    "where",
    "yield",
    "initialize",
    "projection",
    "convert",
    "extern",
];

/// Words a parser recognizes by position without reserving them. A bare name
/// spelled like one of these could be read as syntax, so it is quoted.
/// `contextual_words_cover_every_parser_keyword` keeps this list complete.
const CONTEXTUAL: &[&str] = &[
    "_",
    "annotation",
    "behavior",
    "bind",
    "count",
    "datatype",
    "detail",
    "external",
    "from",
    "generic",
    "id",
    "identity",
    "implicit",
    "instanceClass",
    "instanceType",
    "interface",
    "literal",
    "namespace",
    "new",
    "noIncoming",
    "noresolve",
    "nonserializable",
    "ordered",
    "parameter",
    "readonly",
    "reference",
    "resource",
    "rule",
    "scope",
    "self",
    "sha256",
    "span",
    "super",
    "throws",
    "to",
    "transient",
    "unique",
    "unsettable",
    "using",
    "void",
    "volatile",
    "writable",
];

/// Whether `name` would lex as one Rcore word.
fn is_word(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Whether a bare `name` is read back as exactly that name in every position.
pub(crate) fn is_plain_name(name: &str) -> bool {
    is_word(name)
        && !STRUCTURE_RESERVED.contains(&name)
        && !BEHAVIOR_RESERVED.contains(&name)
        && !CONTEXTUAL.contains(&name)
        && super::rcore::scalar(name).is_none()
}

/// Rcore spelling of `name`: bare when that is unambiguous, otherwise
/// backtick-quoted with each backtick doubled.
pub(crate) fn spell(name: &str) -> String {
    if is_plain_name(name) {
        name.into()
    } else {
        format!("`{}`", name.replace('`', "``"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every word a parser matches by position must be quoted when spelled
    /// as a name, so generated names can never be read as syntax.
    #[test]
    fn contextual_words_cover_every_parser_keyword() {
        let sources = [
            include_str!("rcore.rs"),
            include_str!("rcore_structure_syntax.rs"),
            include_str!("rcore_behavior_parser.rs"),
            include_str!("rcore_behavior_parser_sugar.rs"),
        ];
        let mut missing = Vec::new();
        for source in sources {
            for helper in ["self.eat(\"", "self.need(\"", "self.at(\""] {
                for (at, _) in source.match_indices(helper) {
                    let rest = &source[at + helper.len()..];
                    let Some(end) = rest.find('"') else { continue };
                    let word = &rest[..end];
                    if is_word(word) && is_plain_name(word) {
                        missing.push(word.to_string());
                    }
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(missing.is_empty(), "unquoted parser keywords: {missing:?}");
    }

    #[test]
    fn names_are_quoted_only_when_needed() {
        for plain in [
            "Type",
            "Feature",
            "excludedTypes",
            "_private1",
            "type",
            "source",
        ] {
            assert_eq!(spell(plain), plain);
        }
        for (name, spelled) in [
            ("class", "`class`"),
            ("in", "`in`"),
            ("opposite", "`opposite`"),
            ("readonly", "`readonly`"),
            ("String", "`String`"),
            ("EBoolean", "`EBoolean`"),
            ("_", "`_`"),
            ("1st", "`1st`"),
            ("has space", "`has space`"),
            ("a`b", "`a``b`"),
            ("", "``"),
        ] {
            assert_eq!(spell(name), spelled);
        }
    }
}
