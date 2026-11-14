//! Handwritten shared Rcore lexer. Input traversal is retained; syntax is separate.
use super::{SyntaxDiagnostic, Token, TokenKind};
use crate::definitions::{
    bundle::{DefinitionSource, SourceKind, SourceSpan, SourceValidation, input_chunk_end},
    conversion::InvocationContext,
};

#[derive(Clone, Copy)]
enum OwnedKind {
    Word,
    Number,
    Name,
    String,
}
#[derive(Clone, Copy)]
enum Phase {
    Kind,
    Scan,
    Word,
    Integer,
    Fraction,
    ExponentSign,
    ExponentDigits(bool),
    LineComment,
    BlockComment(usize),
    String(bool),
    Name,
    Allocate(OwnedKind),
    Copy(OwnedKind),
    DecodeName,
    DecodeString,
    Emit(OwnedKind),
    End,
    Complete,
}

/// Complete lexical information, without document or executable authority.
pub struct LexicalOutput<'source> {
    source: &'source DefinitionSource,
    tokens: Vec<Token>,
    diagnostics: Vec<SyntaxDiagnostic>,
}
impl<'source> LexicalOutput<'source> {
    pub fn source(&self) -> &DefinitionSource {
        self.source
    }
    pub fn tokens(&self) -> &[Token] {
        &self.tokens
    }
    pub fn diagnostics(&self) -> &[SyntaxDiagnostic] {
        &self.diagnostics
    }
    pub(crate) fn into_parts(self) -> (Vec<Token>, Vec<SyntaxDiagnostic>) {
        (self.tokens, self.diagnostics)
    }
    pub(crate) fn into_input(
        self,
    ) -> (&'source DefinitionSource, Vec<Token>, Vec<SyntaxDiagnostic>) {
        (self.source, self.tokens, self.diagnostics)
    }
}

/// Retain exact-source validation, scanning and token spelling/JSON decoding.
/// Every advance uses a fresh observer; cancellation is terminal.
/// Validation/copy/decoder payload chunks contain at most 256 bytes; escape
/// classification has at most twelve bytes of lookahead. Output vectors, span
/// identities, allocation and syntax parsing retain separate cooperative obligations.
pub struct LexicalPreparation<'source> {
    source: &'source DefinitionSource,
    state: InputState<'source>,
    failure: Option<String>,
}
enum InputState<'source> {
    Validation(SourceValidation<'source>),
    Scanning(Scanner<'source>),
}
struct Scanner<'source> {
    source: &'source DefinitionSource,
    phase: Phase,
    offset: usize,
    start: usize,
    cursor: usize,
    end: usize,
    value: String,
    tokens: Vec<Token>,
    diagnostics: Vec<SyntaxDiagnostic>,
}
impl<'source> LexicalPreparation<'source> {
    /// Borrow the immutable input without traversing or copying it.
    pub fn new(source: &'source DefinitionSource) -> Self {
        Self {
            source,
            state: InputState::Validation(SourceValidation::new(source)),
            failure: None,
        }
    }
    pub fn advance(&mut self, context: &InvocationContext<'_>) -> Result<bool, String> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        if let Err(error) = context.checkpoint() {
            self.failure = Some(error.clone());
            return Err(error);
        }
        match &mut self.state {
            InputState::Validation(validation) => match validation.advance() {
                Ok(false) => {}
                outcome => {
                    let mut scanner = Scanner::new(self.source);
                    if let Err(error) = outcome {
                        scanner.error(0, error);
                        scanner.phase = Phase::End;
                    }
                    self.state = InputState::Scanning(scanner);
                }
            },
            InputState::Scanning(scanner) => scanner.step(),
        }
        Ok(
            matches!(&self.state, InputState::Scanning(scanner) if matches!(scanner.phase, Phase::Complete)),
        )
    }
    pub fn finish(self) -> Result<LexicalOutput<'source>, String> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let InputState::Scanning(scanner) = self.state else {
            return Err("Rcore lexical preparation is incomplete".into());
        };
        if !matches!(scanner.phase, Phase::Complete) {
            return Err("Rcore lexical preparation is incomplete".into());
        }
        Ok(LexicalOutput {
            source: self.source,
            tokens: scanner.tokens,
            diagnostics: scanner.diagnostics,
        })
    }
}
impl<'source> Scanner<'source> {
    fn new(source: &'source DefinitionSource) -> Self {
        Self {
            source,
            phase: Phase::Kind,
            offset: 0,
            start: 0,
            cursor: 0,
            end: 0,
            value: String::new(),
            tokens: Vec::new(),
            diagnostics: Vec::new(),
        }
    }
    fn rest(&self) -> &str {
        &self.source.text[self.offset..]
    }
    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }
    fn bump(&mut self) {
        if let Some(c) = self.peek() {
            self.offset += c.len_utf8();
        }
    }
    fn span(&self, start: usize) -> SourceSpan {
        SourceSpan {
            source: self.source.id.clone(),
            start,
            end: self.offset,
        }
    }
    fn emit(&mut self, start: usize, kind: TokenKind) {
        self.tokens.push(Token {
            kind,
            span: self.span(start),
        });
    }
    fn error(&mut self, start: usize, message: impl Into<String>) {
        self.diagnostics.push(SyntaxDiagnostic {
            span: self.span(start),
            message: message.into(),
        });
        self.emit(start, TokenKind::Error);
    }
    fn allocate(&mut self, kind: OwnedKind) {
        self.phase = Phase::Allocate(kind);
    }
    fn exponent_or_finish(&mut self) {
        if matches!(self.peek(), Some('e' | 'E')) {
            self.bump();
            self.phase = Phase::ExponentSign;
        } else {
            self.allocate(OwnedKind::Number);
        }
    }
    fn step(&mut self) {
        match self.phase {
            Phase::Kind => {
                if matches!(
                    self.source.kind,
                    SourceKind::Rcore | SourceKind::RcoreBehavior
                ) {
                    self.phase = Phase::Scan;
                } else {
                    self.error(0, "Expected a Rcore source");
                    self.phase = Phase::End;
                }
            }
            Phase::Scan => self.scan(),
            Phase::Word => {
                if self
                    .peek()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    self.bump();
                } else {
                    self.allocate(OwnedKind::Word);
                }
            }
            Phase::Integer => {
                if self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    self.bump();
                } else if self.rest().starts_with('.')
                    && self
                        .rest()
                        .as_bytes()
                        .get(1)
                        .is_some_and(u8::is_ascii_digit)
                {
                    self.bump();
                    self.phase = Phase::Fraction;
                } else {
                    self.exponent_or_finish();
                }
            }
            Phase::Fraction => {
                if self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    self.bump();
                } else {
                    self.exponent_or_finish();
                }
            }
            Phase::ExponentSign => {
                if matches!(self.peek(), Some('+' | '-')) {
                    self.bump();
                }
                self.phase = Phase::ExponentDigits(false);
            }
            Phase::ExponentDigits(seen) => {
                if self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    self.bump();
                    self.phase = Phase::ExponentDigits(true);
                } else if seen {
                    self.allocate(OwnedKind::Number);
                } else {
                    self.error(self.start, "Missing exponent digits");
                    self.phase = Phase::Scan;
                }
            }
            Phase::LineComment => {
                if self.peek().is_none_or(|c| c == '\n' || c == '\r') {
                    self.phase = Phase::Scan;
                } else {
                    self.bump();
                }
            }
            Phase::BlockComment(depth) => {
                if self.offset == self.source.text.len() {
                    self.error(self.start, "Unterminated block comment");
                    self.phase = Phase::Scan;
                } else if self.rest().starts_with("/*") {
                    self.offset += 2;
                    self.phase = Phase::BlockComment(depth + 1);
                } else if self.rest().starts_with("*/") {
                    self.offset += 2;
                    self.phase = if depth == 1 {
                        Phase::Scan
                    } else {
                        Phase::BlockComment(depth - 1)
                    };
                } else {
                    self.bump();
                }
            }
            Phase::String(escaped) => {
                if self.peek().is_none_or(|c| c == '\n' || c == '\r') {
                    self.error(self.start, "Unterminated string");
                    self.phase = Phase::Scan;
                } else {
                    let c = self.peek();
                    self.bump();
                    if !escaped && c == Some('"') {
                        self.allocate(OwnedKind::String);
                    } else {
                        self.phase = Phase::String(!escaped && c == Some('\\'));
                    }
                }
            }
            Phase::Name => {
                if self.peek().is_none_or(|c| c == '\n' || c == '\r') {
                    self.error(self.start, "Unterminated quoted name");
                    self.phase = Phase::Scan;
                } else if self.peek() == Some('`') {
                    self.bump();
                    if self.peek() == Some('`') {
                        self.bump();
                    } else {
                        self.allocate(OwnedKind::Name);
                    }
                } else {
                    self.bump();
                }
            }
            Phase::Allocate(kind) => {
                self.value.clear();
                let quoted = matches!(kind, OwnedKind::Name | OwnedKind::String);
                self.cursor = self.start + usize::from(quoted);
                self.end = self.offset - usize::from(quoted);
                self.value.reserve_exact(self.end - self.cursor);
                self.phase = match kind {
                    OwnedKind::Name => Phase::DecodeName,
                    OwnedKind::String => Phase::DecodeString,
                    _ => Phase::Copy(kind),
                };
            }
            Phase::Copy(kind) => {
                let end = input_chunk_end(&self.source.text, self.cursor).min(self.end);
                self.value.push_str(&self.source.text[self.cursor..end]);
                self.cursor = end;
                if end == self.end {
                    self.phase = Phase::Emit(kind);
                }
            }
            Phase::DecodeName => {
                if self.cursor == self.end {
                    self.phase = Phase::Emit(OwnedKind::Name);
                } else if let Some(c) = self.source.text[self.cursor..self.end].chars().next() {
                    self.cursor += c.len_utf8();
                    if c == '`' {
                        self.cursor += 1;
                    }
                    self.value.push(c);
                }
            }
            Phase::DecodeString => self.decode_string(),
            Phase::Emit(kind) => {
                let value = std::mem::take(&mut self.value);
                if matches!(kind, OwnedKind::Name) && value.is_empty() {
                    self.error(self.start, "Empty quoted name");
                } else {
                    self.emit(
                        self.start,
                        match kind {
                            OwnedKind::Word => TokenKind::Word(value),
                            OwnedKind::Number => TokenKind::Number(value),
                            OwnedKind::Name => TokenKind::QuotedName(value),
                            OwnedKind::String => TokenKind::String(value),
                        },
                    );
                }
                self.phase = Phase::Scan;
            }
            Phase::End => {
                self.emit(self.offset, TokenKind::End);
                self.phase = Phase::Complete;
            }
            Phase::Complete => {}
        }
    }
    fn scan(&mut self) {
        self.start = self.offset;
        let Some(c) = self.peek() else {
            self.phase = Phase::End;
            return;
        };
        if c.is_whitespace() {
            self.bump();
        } else if self.rest().starts_with("//") {
            self.offset += 2;
            self.phase = Phase::LineComment;
        } else if self.rest().starts_with("/*") {
            self.offset += 2;
            self.phase = Phase::BlockComment(1);
        } else if c == '"' {
            self.bump();
            self.phase = Phase::String(false);
        } else if c == '`' {
            self.bump();
            self.phase = Phase::Name;
        } else if c.is_ascii_alphabetic() || c == '_' {
            self.phase = Phase::Word;
        } else if c.is_ascii_digit() {
            self.phase = Phase::Integer;
        } else {
            for symbol in [
                "::", "->", "=>", "?.", "??", "==", "!=", "<=", ">=", "&&", "||",
            ] {
                if self.rest().starts_with(symbol) {
                    self.offset += symbol.len();
                    self.emit(self.start, TokenKind::Symbol(symbol.into()));
                    return;
                }
            }
            self.bump();
            if "(){}[]<>,;:.=+-*/%!|?&".contains(c) {
                self.emit(self.start, TokenKind::Symbol(c.to_string()));
            } else {
                self.error(self.start, format!("Unexpected character {c:?}"));
            }
        }
    }
    fn decode_string(&mut self) {
        if self.cursor == self.end {
            self.phase = Phase::Emit(OwnedKind::String);
            return;
        }
        let mut end = self.cursor;
        while end < self.end {
            let unit = json_unit(&self.source.text[end..self.end]);
            if end - self.cursor + unit > 256 {
                break;
            }
            end += unit;
        }
        let length = end - self.cursor;
        let mut encoded = [0u8; 258];
        encoded[0] = b'"';
        encoded[1..length + 1].copy_from_slice(&self.source.text.as_bytes()[self.cursor..end]);
        encoded[length + 1] = b'"';
        match serde_json::from_slice::<String>(&encoded[..length + 2]) {
            Ok(value) => {
                self.value.push_str(&value);
                self.cursor = end;
            }
            Err(_) => {
                self.value = String::new();
                self.error(self.start, "Invalid string escape or control character");
                self.phase = Phase::Scan;
            }
        }
    }
}
// Determine only an indivisible raw JSON unit. Serde remains the decoder and
// validity authority. A high-surrogate escape and following Unicode escape
// stay together, including invalid pairs, so chunking cannot change rejection.
fn json_unit(text: &str) -> usize {
    let bytes = text.as_bytes();
    if bytes.first() != Some(&b'\\') {
        return text.chars().next().map_or(0, char::len_utf8);
    }
    let unicode = |part: &[u8]| -> Option<u16> {
        if part.len() < 6 || &part[..2] != b"\\u" || !part[2..6].iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        std::str::from_utf8(&part[2..6])
            .ok()
            .and_then(|s| u16::from_str_radix(s, 16).ok())
    };
    if let Some(value) = unicode(bytes) {
        if (0xd800..=0xdbff).contains(&value) && unicode(&bytes[6..]).is_some() {
            12
        } else {
            6
        }
    } else {
        1 + text[1..].chars().next().map_or(0, char::len_utf8)
    }
}

/// Drive the existing lexical cursor without retaining an operation observer.
/// Resource/compiler admission owns its observation policy; lexical traversal is pure.
pub(crate) async fn parts_retained(
    source: &DefinitionSource,
) -> (Vec<Token>, Vec<SyntaxDiagnostic>) {
    let mut preparation = LexicalPreparation::new(source);
    let result = loop {
        let step = preparation.advance(&InvocationContext::default());
        match step {
            Err(error) => break Err(error),
            Ok(complete) => {
                crate::definitions::continuation::pause().await;
                if complete {
                    break preparation.finish();
                }
            }
        }
    };
    let (tokens, diagnostics) = match result {
        Ok(output) => output.into_parts(),
        Err(message) => {
            let span = SourceSpan {
                source: source.id.clone(),
                start: 0,
                end: 0,
            };
            (
                vec![
                    Token {
                        kind: TokenKind::Error,
                        span: span.clone(),
                    },
                    Token {
                        kind: TokenKind::End,
                        span: span.clone(),
                    },
                ],
                vec![SyntaxDiagnostic { span, message }],
            )
        }
    };
    (tokens, diagnostics)
}

#[cfg(test)]
mod traversal_controls {
    use super::*;
    #[test]
    fn lexical_steps_bound_source_and_decoding_work_without_recopying_prefixes() {
        for (text, expected) in [
            (
                "identifier".repeat(1000),
                TokenKind::Word("identifier".repeat(1000)),
            ),
            (
                format!(
                    "{}.{}e-{}",
                    "1".repeat(500),
                    "2".repeat(500),
                    "3".repeat(500)
                ),
                TokenKind::Number(format!(
                    "{}.{}e-{}",
                    "1".repeat(500),
                    "2".repeat(500),
                    "3".repeat(500)
                )),
            ),
            (
                format!("`{}`", "λ``🚀".repeat(1000)),
                TokenKind::QuotedName("λ`🚀".repeat(1000)),
            ),
            (
                format!("\"{}\\uD83D\\uDE80\"", "λ🚀".repeat(1000)),
                TokenKind::String(format!("{}🚀", "λ🚀".repeat(1000))),
            ),
        ] {
            let source = DefinitionSource::new(
                format!("memory:{}", "α".repeat(400)),
                SourceKind::Rcore,
                text,
            );
            let pointer = source.text.as_ptr();
            let mut preparation = LexicalPreparation::new(&source);
            let mut scanning_steps = 0;
            let mut spelling_steps = 0;
            loop {
                let before = match &preparation.state {
                    InputState::Validation(_) => None,
                    InputState::Scanning(scanner) => Some((
                        scanner.offset,
                        scanner.cursor,
                        scanner.value.as_ptr(),
                        scanner.value.len(),
                        matches!(
                            scanner.phase,
                            Phase::Copy(_) | Phase::DecodeName | Phase::DecodeString
                        ),
                    )),
                };
                let complete = preparation.advance(&InvocationContext::default()).unwrap();
                if let (
                    Some((offset, cursor, value_pointer, value_length, copying)),
                    InputState::Scanning(scanner),
                ) = (before, &preparation.state)
                {
                    assert!(scanner.offset - offset <= 4);
                    scanning_steps += 1;
                    if copying {
                        assert!(scanner.cursor - cursor <= 256);
                        assert!(scanner.value.len() - value_length <= 256);
                        assert_eq!(scanner.value.as_ptr(), value_pointer);
                        spelling_steps += 1;
                    }
                }
                assert_eq!(source.text.as_ptr(), pointer);
                if complete {
                    break;
                }
            }
            let output = preparation.finish().unwrap();
            assert!(scanning_steps > source.text.len() / 256);
            assert!(spelling_steps > 1);
            assert!(output.diagnostics().is_empty());
            assert_eq!(output.tokens().len(), 2);
            assert_eq!(output.tokens()[0].kind, expected);
            assert_eq!(output.tokens()[0].span.start, 0);
            assert_eq!(output.tokens()[0].span.end, source.text.len());
            assert_eq!(output.tokens()[0].span.source, source.id);
            assert_eq!(output.tokens()[1].kind, TokenKind::End);
        }
    }
    #[test]
    fn comments_and_unterminated_tokens_yield_without_publishing_partial_spellings() {
        for (text, message) in [
            (
                format!("/*{}", "λ🚀".repeat(1000)),
                "Unterminated block comment",
            ),
            (
                format!("\"{}\\x", "λ🚀".repeat(1000)),
                "Unterminated string",
            ),
            (
                format!("`{}", "λ🚀".repeat(1000)),
                "Unterminated quoted name",
            ),
        ] {
            let source = DefinitionSource::new("memory:unterminated", SourceKind::Rcore, text);
            let mut preparation = LexicalPreparation::new(&source);
            let mut advances = 0;
            while !preparation.advance(&InvocationContext::default()).unwrap() {
                advances += 1;
                if let InputState::Scanning(scanner) = &preparation.state {
                    assert!(scanner.value.is_empty());
                }
            }
            assert!(advances > source.text.len() / 256);
            let output = preparation.finish().unwrap();
            assert_eq!(output.diagnostics().len(), 1);
            assert_eq!(output.diagnostics()[0].message, message);
            assert_eq!(output.diagnostics()[0].span.end, source.text.len());
            assert_eq!(output.tokens().len(), 2);
            assert_eq!(output.tokens()[0].kind, TokenKind::Error);
            assert_eq!(output.tokens()[1].kind, TokenKind::End);
        }
    }
}
