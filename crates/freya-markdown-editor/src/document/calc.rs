//! Numbers as people write them in cells, and the arithmetic behind
//! computed table columns and the inline calculator.

use std::fmt;

/// A number read from text, keeping how it was written so results can be
/// shown the same way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Number {
    pub value: f64,
    /// Digits after the decimal separator.
    pub decimals: usize,
    /// Whether the decimal separator was a comma.
    pub comma: bool,
}

impl Number {
    /// The number in `text`, if it is one: an optional sign, digits with
    /// `.` or `,` as decimal or thousands separators, and a currency sign
    /// or `%` around it. `1.250` and `1,250` read as thousands (a lone
    /// separator before exactly three digits), everything else as a
    /// decimal separator.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let trimmed = text
            .trim()
            .trim_matches(|ch: char| ch.is_whitespace() || is_unit(ch));
        let (negative, digits) = match trimmed.chars().next()? {
            '-' | '−' => (true, &trimmed[trimmed.chars().next()?.len_utf8()..]),
            '+' => (false, &trimmed[1..]),
            _ => (false, trimmed),
        };
        let digits = digits.trim_start();
        if digits.is_empty()
            || !digits
                .chars()
                .all(|ch| ch.is_ascii_digit() || matches!(ch, '.' | ','))
            || !digits.chars().any(|ch| ch.is_ascii_digit())
        {
            return None;
        }
        let dots = digits.matches('.').count();
        let commas = digits.matches(',').count();
        let decimal = match (dots, commas) {
            (0, 0) => None,
            (1, 0) | (0, 1) => {
                let separator = if dots == 1 { '.' } else { ',' };
                let (whole, fraction) = digits.split_once(separator)?;
                let thousands = fraction.len() == 3 && !whole.is_empty() && whole != "0";
                (!thousands).then_some(separator)
            }
            (_, 0) | (0, _) => None,
            _ => digits
                .rfind(['.', ','])
                .and_then(|at| digits[at..].chars().next()),
        };
        let mut normalised = String::with_capacity(digits.len());
        for ch in digits.chars() {
            match ch {
                '.' | ',' if Some(ch) == decimal => normalised.push('.'),
                '.' | ',' => {}
                digit => normalised.push(digit),
            }
        }
        let value: f64 = normalised.parse().ok()?;
        let decimals = normalised
            .split_once('.')
            .map_or(0, |(_, fraction)| fraction.len());
        Some(Self {
            value: if negative { -value } else { value },
            decimals,
            comma: decimal == Some(','),
        })
    }

    /// `value` written with `decimals` digits and the chosen separator.
    #[must_use]
    pub fn format(value: f64, decimals: usize, comma: bool) -> String {
        let mut text = format!("{value:.decimals$}");
        if text.starts_with("-0") && text[1..].chars().all(|ch| ch == '0' || ch == '.') {
            text.remove(0);
        }
        if comma {
            text = text.replace('.', ",");
        }
        text
    }
}

/// Signs people put around an amount that say nothing about its value.
fn is_unit(ch: char) -> bool {
    matches!(ch, '$' | '€' | '£' | '¥' | '%' | '¢')
}

/// How a column is named inside an expression.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ColumnRef {
    /// `[Header text]`.
    Header(String),
    /// `#n`, the n-th column counting from one.
    Index(usize),
}

impl fmt::Display for ColumnRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Header(name) => write!(f, "[{name}]"),
            Self::Index(index) => write!(f, "#{index}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
}

impl Op {
    const fn symbol(self) -> char {
        match self {
            Self::Add => '+',
            Self::Sub => '-',
            Self::Mul => '*',
            Self::Div => '/',
        }
    }

    const fn precedence(self) -> u8 {
        match self {
            Self::Add | Self::Sub => 1,
            Self::Mul | Self::Div => 2,
        }
    }

    fn apply(self, left: f64, right: f64) -> Option<f64> {
        let value = match self {
            Self::Add => left + right,
            Self::Sub => left - right,
            Self::Mul => left * right,
            Self::Div => {
                if right == 0. {
                    return None;
                }
                left / right
            }
        };
        value.is_finite().then_some(value)
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Node {
    Number(f64),
    Column(ColumnRef),
    Negate(Box<Node>),
    Binary(Box<Node>, Op, Box<Node>),
}

/// Arithmetic over numbers and columns: `+ - * /`, parentheses and a
/// leading minus. `×`, `÷` and `−` read as their ASCII operators.
#[derive(Clone, Debug, PartialEq)]
pub struct Expr(Node);

/// Why text is not an [`Expr`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExprError {
    Empty,
    /// A character with no place in an expression, with its char offset.
    Unexpected(char, usize),
    /// An operand or an operator is missing at the char offset.
    Incomplete(usize),
    UnclosedBracket,
    UnclosedParenthesis,
}

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("the expression is empty"),
            Self::Unexpected(ch, at) => write!(f, "unexpected {ch:?} at {at}"),
            Self::Incomplete(at) => write!(f, "something is missing at {at}"),
            Self::UnclosedBracket => f.write_str("a column name is missing its closing ]"),
            Self::UnclosedParenthesis => f.write_str("a parenthesis is never closed"),
        }
    }
}

impl std::error::Error for ExprError {}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Number(f64),
    Column(ColumnRef),
    Op(Op),
    Open,
    Close,
}

struct Lexer<'a> {
    chars: std::iter::Peekable<std::str::CharIndices<'a>>,
    source: &'a str,
}

impl Lexer<'_> {
    #[allow(clippy::too_many_lines, reason = "one arm per token kind")]
    fn next_token(&mut self) -> Result<Option<(Token, usize)>, ExprError> {
        while let Some((at, ch)) = self.chars.next() {
            let token = match ch {
                ch if ch.is_whitespace() => continue,
                '+' => Token::Op(Op::Add),
                '-' | '−' => Token::Op(Op::Sub),
                '*' | '×' | '·' => Token::Op(Op::Mul),
                '/' | '÷' => Token::Op(Op::Div),
                '(' => Token::Open,
                ')' => Token::Close,
                '[' => {
                    let start = at + 1;
                    let mut end = None;
                    for (index, ch) in self.chars.by_ref() {
                        if ch == ']' {
                            end = Some(index);
                            break;
                        }
                    }
                    let end = end.ok_or(ExprError::UnclosedBracket)?;
                    let name = self.source[start..end].trim();
                    if name.is_empty() {
                        return Err(ExprError::Incomplete(at));
                    }
                    Token::Column(ColumnRef::Header(name.to_owned()))
                }
                '#' => {
                    let mut digits = String::new();
                    while let Some((_, ch)) = self.chars.peek() {
                        if ch.is_ascii_digit() {
                            digits.push(*ch);
                            self.chars.next();
                        } else {
                            break;
                        }
                    }
                    let index: usize = digits
                        .parse()
                        .ok()
                        .filter(|index| *index > 0)
                        .ok_or(ExprError::Incomplete(at))?;
                    Token::Column(ColumnRef::Index(index))
                }
                ch if ch.is_ascii_digit() || ch == '.' || ch == ',' => {
                    let mut text = String::from(ch);
                    while let Some((_, ch)) = self.chars.peek() {
                        if ch.is_ascii_digit() || matches!(ch, '.' | ',') {
                            text.push(*ch);
                            self.chars.next();
                        } else {
                            break;
                        }
                    }
                    let number = Number::parse(&text).ok_or(ExprError::Unexpected(ch, at))?;
                    Token::Number(number.value)
                }
                ch => return Err(ExprError::Unexpected(ch, at)),
            };
            return Ok(Some((token, at)));
        }
        Ok(None)
    }
}

struct Parser {
    tokens: Vec<(Token, usize)>,
    at: usize,
    end: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at).map(|(token, _)| token)
    }

    fn offset(&self) -> usize {
        self.tokens.get(self.at).map_or(self.end, |(_, at)| *at)
    }

    fn expression(&mut self, min_precedence: u8) -> Result<Node, ExprError> {
        let mut left = self.operand()?;
        while let Some(Token::Op(op)) = self.peek().cloned() {
            if op.precedence() < min_precedence {
                break;
            }
            self.at += 1;
            let right = self.expression(op.precedence() + 1)?;
            left = Node::Binary(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn operand(&mut self) -> Result<Node, ExprError> {
        let offset = self.offset();
        let Some(token) = self.peek().cloned() else {
            return Err(ExprError::Incomplete(offset));
        };
        self.at += 1;
        match token {
            Token::Number(value) => Ok(Node::Number(value)),
            Token::Column(column) => Ok(Node::Column(column)),
            Token::Op(Op::Sub) => Ok(Node::Negate(Box::new(self.operand()?))),
            Token::Op(Op::Add) => self.operand(),
            Token::Open => {
                let inner = self.expression(1)?;
                if self.peek() != Some(&Token::Close) {
                    return Err(ExprError::UnclosedParenthesis);
                }
                self.at += 1;
                Ok(inner)
            }
            Token::Op(_) | Token::Close => Err(ExprError::Incomplete(offset)),
        }
    }
}

impl Expr {
    /// Parse `source`.
    ///
    /// # Errors
    ///
    /// When `source` is not an expression, saying where it stops being one.
    pub fn parse(source: &str) -> Result<Self, ExprError> {
        let mut lexer = Lexer {
            chars: source.char_indices().peekable(),
            source,
        };
        let mut tokens = Vec::new();
        while let Some(token) = lexer.next_token()? {
            tokens.push(token);
        }
        if tokens.is_empty() {
            return Err(ExprError::Empty);
        }
        let mut parser = Parser {
            tokens,
            at: 0,
            end: source.chars().count(),
        };
        let node = parser.expression(1)?;
        match parser.peek() {
            None => Ok(Self(node)),
            Some(Token::Close) => Err(ExprError::Unexpected(')', parser.offset())),
            Some(_) => Err(ExprError::Incomplete(parser.offset())),
        }
    }

    /// The value with every column looked up through `column`; `None` when
    /// a column has no number or a division hits zero.
    #[must_use]
    pub fn eval(&self, column: &dyn Fn(&ColumnRef) -> Option<f64>) -> Option<f64> {
        fn walk(node: &Node, column: &dyn Fn(&ColumnRef) -> Option<f64>) -> Option<f64> {
            match node {
                Node::Number(value) => Some(*value),
                Node::Column(reference) => column(reference),
                Node::Negate(inner) => walk(inner, column).map(|value| -value),
                Node::Binary(left, op, right) => {
                    op.apply(walk(left, column)?, walk(right, column)?)
                }
            }
        }
        walk(&self.0, column)
    }

    /// Every column the expression reads, in order of first use.
    #[must_use]
    pub fn columns(&self) -> Vec<&ColumnRef> {
        fn walk<'n>(node: &'n Node, out: &mut Vec<&'n ColumnRef>) {
            match node {
                Node::Number(_) => {}
                Node::Column(reference) => {
                    if !out.contains(&reference) {
                        out.push(reference);
                    }
                }
                Node::Negate(inner) => walk(inner, out),
                Node::Binary(left, _, right) => {
                    walk(left, out);
                    walk(right, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.0, &mut out);
        out
    }

    /// Point every use of column `from` at `to`.
    pub fn rename_column(&mut self, from: &ColumnRef, to: ColumnRef) {
        fn walk(node: &mut Node, from: &ColumnRef, to: &ColumnRef) {
            match node {
                Node::Number(_) => {}
                Node::Column(reference) => {
                    if reference == from {
                        *reference = to.clone();
                    }
                }
                Node::Negate(inner) => walk(inner, from, to),
                Node::Binary(left, _, right) => {
                    walk(left, from, to);
                    walk(right, from, to);
                }
            }
        }
        walk(&mut self.0, from, &to);
    }

    /// Whether the expression combines at least two values.
    #[must_use]
    pub fn is_compound(&self) -> bool {
        matches!(self.0, Node::Binary(..))
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn write(node: &Node, parent: u8, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match node {
                Node::Number(value) => write!(f, "{value}"),
                Node::Column(reference) => write!(f, "{reference}"),
                Node::Negate(inner) => {
                    f.write_str("-")?;
                    write(inner, 3, f)
                }
                Node::Binary(left, op, right) => {
                    let wrap = op.precedence() < parent;
                    if wrap {
                        f.write_str("(")?;
                    }
                    write(left, op.precedence(), f)?;
                    write!(f, " {} ", op.symbol())?;
                    write(right, op.precedence() + 1, f)?;
                    if wrap {
                        f.write_str(")")?;
                    }
                    Ok(())
                }
            }
        }
        write(&self.0, 0, f)
    }
}

/// An arithmetic expression someone just finished typing: the text of one
/// line ends in it.
#[derive(Clone, Debug, PartialEq)]
pub struct TrailingCalculation {
    /// Byte offset of the expression in the line.
    pub start: usize,
    pub value: f64,
    /// The result written like the operands.
    pub result: String,
}

impl TrailingCalculation {
    /// The longest expression `line` ends with that combines at least two
    /// numbers, starting at a word boundary. `1 + 2` qualifies, `12` or
    /// `x1 + 2` do not.
    #[must_use]
    pub fn at_end_of(line: &str) -> Option<Self> {
        let allowed = |ch: char| {
            ch.is_ascii_digit()
                || ch.is_whitespace()
                || matches!(
                    ch,
                    '.' | ',' | '+' | '-' | '−' | '*' | '×' | '·' | '/' | '÷' | '(' | ')'
                )
        };
        let tail_start = line
            .char_indices()
            .rev()
            .take_while(|(_, ch)| allowed(*ch))
            .last()
            .map(|(at, _)| at)?;
        let candidates = line[tail_start..]
            .char_indices()
            .filter(|(_, ch)| !ch.is_whitespace())
            .map(|(at, _)| tail_start + at);
        for start in candidates {
            let source = &line[start..];
            let boundary = line[..start]
                .chars()
                .next_back()
                .is_none_or(|ch| !ch.is_alphanumeric());
            if !boundary {
                continue;
            }
            let Ok(expr) = Expr::parse(source) else {
                continue;
            };
            if !expr.is_compound() {
                continue;
            }
            let Some(value) = expr.eval(&|_| None) else {
                continue;
            };
            let numbers: Vec<Number> = source
                .split(|ch: char| !(ch.is_ascii_digit() || matches!(ch, '.' | ',')))
                .filter_map(Number::parse)
                .collect();
            let decimals = numbers.iter().map(|n| n.decimals).max().unwrap_or(0);
            let comma = numbers.iter().any(|n| n.comma);
            return Some(Self {
                start,
                value,
                result: Number::format(value, decimals, comma),
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_read_separators_and_units() {
        let cases = [
            ("12", 12., 0, false),
            ("12.5", 12.5, 1, false),
            ("12,50", 12.5, 2, true),
            ("1.250", 1250., 0, false),
            ("1,250", 1250., 0, false),
            ("0.125", 0.125, 3, false),
            ("1.234.567", 1_234_567., 0, false),
            ("1.234,56", 1234.56, 2, true),
            ("1,234.56", 1234.56, 2, false),
            ("$ 20", 20., 0, false),
            ("20 €", 20., 0, false),
            ("-3", -3., 0, false),
            ("−3,5", -3.5, 1, true),
            ("15%", 15., 0, false),
        ];
        for (text, value, decimals, comma) in cases {
            let number = Number::parse(text).unwrap_or_else(|| panic!("{text}"));
            assert!((number.value - value).abs() < 1e-9, "{text}: {number:?}");
            assert_eq!((number.decimals, number.comma), (decimals, comma), "{text}");
        }
        for text in ["", "abc", "12 apples", "1-2", ".", "Bs"] {
            assert!(Number::parse(text).is_none(), "{text}");
        }
    }

    #[test]
    fn formatting_follows_the_inputs() {
        assert_eq!(Number::format(2.5, 2, true), "2,50");
        assert_eq!(Number::format(-0.0, 0, false), "0");
        assert_eq!(Number::format(1234.5, 1, false), "1234.5");
    }

    #[test]
    fn expressions_parse_and_print() {
        let expr = Expr::parse("[Costo] - [Presupuesto]").expect("parse");
        assert_eq!(expr.to_string(), "[Costo] - [Presupuesto]");
        let expr = Expr::parse("(#1 + 2) × 3 ÷ -4").expect("parse");
        assert_eq!(expr.to_string(), "(#1 + 2) * 3 / -4");
        let value = expr.eval(&|column| match column {
            ColumnRef::Index(1) => Some(2.),
            _ => None,
        });
        assert_eq!(value, Some(-3.));
        assert_eq!(Expr::parse("1 + ").unwrap_err(), ExprError::Incomplete(4));
        assert_eq!(
            Expr::parse("[Costo").unwrap_err(),
            ExprError::UnclosedBracket
        );
        assert_eq!(
            Expr::parse("1 ) 2").unwrap_err(),
            ExprError::Unexpected(')', 2)
        );
        assert_eq!(Expr::parse("").unwrap_err(), ExprError::Empty);
    }

    #[test]
    fn renaming_columns_rewrites_the_source() {
        let mut expr = Expr::parse("[A] * 2 + [A]").expect("parse");
        expr.rename_column(
            &ColumnRef::Header("A".into()),
            ColumnRef::Header("B".into()),
        );
        assert_eq!(expr.to_string(), "[B] * 2 + [B]");
        assert_eq!(expr.columns(), vec![&ColumnRef::Header("B".into())]);
    }

    #[test]
    fn a_line_ending_in_arithmetic_is_a_calculation() {
        let calc = TrailingCalculation::at_end_of("Total: 123 + 87").expect("calc");
        assert_eq!((calc.start, calc.result.as_str()), (7, "210"));
        let calc = TrailingCalculation::at_end_of("2,5 * 4").expect("calc");
        assert_eq!(calc.result, "10,0");
        let calc = TrailingCalculation::at_end_of("x = (1 + 2) * 3").expect("calc");
        assert_eq!((calc.start, calc.result.as_str()), (4, "9"));
        for line in ["123", "1 +", "x1 + 2", "", "3 / 0", "hello 2024"] {
            assert!(TrailingCalculation::at_end_of(line).is_none(), "{line}");
        }
    }
}
