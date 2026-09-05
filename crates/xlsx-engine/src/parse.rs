//! Small formula tokenizer + recursive-descent parser (seed coverage only).

use xlsx_types::{CellAddr, CellRef, EvalError, ExcelError, RangeRef};

#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Number(f64),
    String(String),
    Ident(String),
    Quoted(String),
    Error(ExcelError),
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    Amp,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    LParen,
    RParen,
    LBrace,
    RBrace,
    Comma,
    Semicolon,
    Colon,
    Bang,
    Percent,
    Eof,
}

pub fn tokenize(input: &str) -> Result<Vec<Token>, EvalError> {
    let mut chars: Vec<char> = input.chars().collect();
    // Strip a single leading `=`.
    if chars.first().copied() == Some('=') {
        chars.remove(0);
    }
    let mut i = 0;
    let mut out = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        match c {
            '+' => {
                out.push(Token::Plus);
                i += 1;
            }
            '-' => {
                out.push(Token::Minus);
                i += 1;
            }
            '*' => {
                out.push(Token::Star);
                i += 1;
            }
            '/' => {
                out.push(Token::Slash);
                i += 1;
            }
            '^' => {
                out.push(Token::Caret);
                i += 1;
            }
            '&' => {
                out.push(Token::Amp);
                i += 1;
            }
            '(' => {
                out.push(Token::LParen);
                i += 1;
            }
            ')' => {
                out.push(Token::RParen);
                i += 1;
            }
            '{' => {
                out.push(Token::LBrace);
                i += 1;
            }
            '}' => {
                out.push(Token::RBrace);
                i += 1;
            }
            ',' => {
                out.push(Token::Comma);
                i += 1;
            }
            ';' => {
                out.push(Token::Semicolon);
                i += 1;
            }
            ':' => {
                out.push(Token::Colon);
                i += 1;
            }
            '!' => {
                out.push(Token::Bang);
                i += 1;
            }
            '%' => {
                out.push(Token::Percent);
                i += 1;
            }
            '=' => {
                out.push(Token::Eq);
                i += 1;
            }
            '<' => {
                if chars.get(i + 1) == Some(&'>') {
                    out.push(Token::Ne);
                    i += 2;
                } else if chars.get(i + 1) == Some(&'=') {
                    out.push(Token::Le);
                    i += 2;
                } else {
                    out.push(Token::Lt);
                    i += 1;
                }
            }
            '>' => {
                if chars.get(i + 1) == Some(&'=') {
                    out.push(Token::Ge);
                    i += 2;
                } else {
                    out.push(Token::Gt);
                    i += 1;
                }
            }
            '"' => {
                i += 1;
                let mut s = String::new();
                loop {
                    if i >= chars.len() {
                        return Err(EvalError::Parse("unterminated string".into()));
                    }
                    let ch = chars[i];
                    if ch == '"' {
                        if chars.get(i + 1) == Some(&'"') {
                            s.push('"');
                            i += 2;
                        } else {
                            i += 1;
                            break;
                        }
                    } else {
                        s.push(ch);
                        i += 1;
                    }
                }
                out.push(Token::String(s));
            }
            '\'' => {
                i += 1;
                let mut s = String::new();
                loop {
                    if i >= chars.len() {
                        return Err(EvalError::Parse("unterminated sheet name".into()));
                    }
                    let ch = chars[i];
                    if ch == '\'' {
                        if chars.get(i + 1) == Some(&'\'') {
                            s.push('\'');
                            i += 2;
                        } else {
                            i += 1;
                            break;
                        }
                    } else {
                        s.push(ch);
                        i += 1;
                    }
                }
                out.push(Token::Quoted(s));
            }
            '#' => {
                let start = i;
                i += 1;
                while i < chars.len() {
                    let ch = chars[i];
                    if ch.is_ascii_alphanumeric()
                        || ch == '/'
                        || ch == '_'
                        || ch == '?'
                        || ch == '!'
                    {
                        i += 1;
                        if ch == '!' || ch == '?' {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                let raw: String = chars[start..i].iter().collect();
                let err = ExcelError::parse(&raw)
                    .ok_or_else(|| EvalError::Parse(format!("unknown error literal {raw}")))?;
                out.push(Token::Error(err));
            }
            c if c.is_ascii_digit()
                || (c == '.' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit())) =>
            {
                let start = i;
                i += 1;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                    i += 1;
                    if i < chars.len() && (chars[i] == '+' || chars[i] == '-') {
                        i += 1;
                    }
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                let raw: String = chars[start..i].iter().collect();
                let n: f64 = raw
                    .parse()
                    .map_err(|_| EvalError::Parse(format!("bad number {raw}")))?;
                out.push(Token::Number(n));
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                i += 1;
                while i < chars.len()
                    && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '.')
                {
                    i += 1;
                }
                let raw: String = chars[start..i].iter().collect();
                out.push(Token::Ident(raw));
            }
            other => {
                return Err(EvalError::Parse(format!("unexpected character {other:?}")));
            }
        }
    }
    out.push(Token::Eof);
    Ok(out)
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Number(f64),
    Text(String),
    Bool(bool),
    Error(ExcelError),
    Cell(CellRef),
    Range(RangeRef),
    Name(String),
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Call {
        name: String,
        args: Vec<Expr>,
    },
    Array(Vec<Vec<Expr>>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Plus,
    Minus,
    Percent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Concat,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
}

pub fn parse(input: &str) -> Result<Expr, EvalError> {
    let tokens = tokenize(input)?;
    let mut p = Parser { tokens, i: 0 };
    let expr = p.parse_comparison()?;
    p.expect_eof()?;
    Ok(expr)
}

struct Parser {
    tokens: Vec<Token>,
    i: usize,
}

impl Parser {
    fn peek(&self) -> &Token {
        self.tokens.get(self.i).unwrap_or(&Token::Eof)
    }

    fn bump(&mut self) -> Token {
        let t = self.tokens.get(self.i).cloned().unwrap_or(Token::Eof);
        if self.i < self.tokens.len() {
            self.i += 1;
        }
        t
    }

    fn expect_eof(&self) -> Result<(), EvalError> {
        match self.peek() {
            Token::Eof => Ok(()),
            other => Err(EvalError::Parse(format!("trailing token {other:?}"))),
        }
    }

    fn parse_comparison(&mut self) -> Result<Expr, EvalError> {
        let mut left = self.parse_concat()?;
        loop {
            let op = match self.peek() {
                Token::Eq => BinOp::Eq,
                Token::Ne => BinOp::Ne,
                Token::Lt => BinOp::Lt,
                Token::Gt => BinOp::Gt,
                Token::Le => BinOp::Le,
                Token::Ge => BinOp::Ge,
                _ => break,
            };
            self.bump();
            let right = self.parse_concat()?;
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_concat(&mut self) -> Result<Expr, EvalError> {
        let mut left = self.parse_add()?;
        while matches!(self.peek(), Token::Amp) {
            self.bump();
            let right = self.parse_add()?;
            left = Expr::Binary {
                op: BinOp::Concat,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_add(&mut self) -> Result<Expr, EvalError> {
        let mut left = self.parse_mul()?;
        loop {
            let op = match self.peek() {
                Token::Plus => BinOp::Add,
                Token::Minus => BinOp::Sub,
                _ => break,
            };
            self.bump();
            let right = self.parse_mul()?;
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_mul(&mut self) -> Result<Expr, EvalError> {
        let mut left = self.parse_pow()?;
        loop {
            let op = match self.peek() {
                Token::Star => BinOp::Mul,
                Token::Slash => BinOp::Div,
                _ => break,
            };
            self.bump();
            let right = self.parse_pow()?;
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_pow(&mut self) -> Result<Expr, EvalError> {
        let left = self.parse_unary()?;
        if matches!(self.peek(), Token::Caret) {
            self.bump();
            let right = self.parse_pow()?; // right-assoc is fine for seed (`2^8`)
            Ok(Expr::Binary {
                op: BinOp::Pow,
                left: Box::new(left),
                right: Box::new(right),
            })
        } else {
            Ok(left)
        }
    }

    fn parse_unary(&mut self) -> Result<Expr, EvalError> {
        match self.peek() {
            Token::Plus => {
                self.bump();
                Ok(Expr::Unary {
                    op: UnaryOp::Plus,
                    expr: Box::new(self.parse_unary()?),
                })
            }
            Token::Minus => {
                self.bump();
                Ok(Expr::Unary {
                    op: UnaryOp::Minus,
                    expr: Box::new(self.parse_unary()?),
                })
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, EvalError> {
        let mut expr = self.parse_primary()?;
        while matches!(self.peek(), Token::Percent) {
            self.bump();
            expr = Expr::Unary {
                op: UnaryOp::Percent,
                expr: Box::new(expr),
            };
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expr, EvalError> {
        match self.bump() {
            Token::Number(n) => Ok(Expr::Number(n)),
            Token::String(s) => Ok(Expr::Text(s)),
            Token::Error(e) => Ok(Expr::Error(e)),
            Token::LParen => {
                let inner = self.parse_comparison()?;
                match self.bump() {
                    Token::RParen => Ok(inner),
                    other => Err(EvalError::Parse(format!("expected ')', got {other:?}"))),
                }
            }
            Token::LBrace => self.parse_array(),
            Token::Ident(id) => self.parse_identish(id, false),
            Token::Quoted(q) => self.parse_identish(q, true),
            other => Err(EvalError::Parse(format!("unexpected token {other:?}"))),
        }
    }

    fn parse_identish(&mut self, id: String, quoted: bool) -> Result<Expr, EvalError> {
        if matches!(self.peek(), Token::LParen) && !quoted {
            self.bump();
            let args = self.parse_args()?;
            return Ok(Expr::Call { name: id, args });
        }
        if matches!(self.peek(), Token::Bang) {
            self.bump();
            return self.parse_cell_or_range(Some(id));
        }
        if !quoted && is_bool(&id) {
            return Ok(Expr::Bool(id.eq_ignore_ascii_case("TRUE")));
        }
        if !quoted && looks_like_cell(&id) {
            return self.finish_cell_or_range(None, id);
        }
        if quoted {
            return Err(EvalError::Parse(format!(
                "quoted identifier {id:?} must be followed by !A1"
            )));
        }
        Ok(Expr::Name(id))
    }

    fn parse_cell_or_range(&mut self, sheet: Option<String>) -> Result<Expr, EvalError> {
        match self.bump() {
            Token::Ident(id) if looks_like_cell(&id) => self.finish_cell_or_range(sheet, id),
            other => Err(EvalError::Parse(format!(
                "expected cell address after sheet, got {other:?}"
            ))),
        }
    }

    fn finish_cell_or_range(
        &mut self,
        sheet: Option<String>,
        start_id: String,
    ) -> Result<Expr, EvalError> {
        let start = CellAddr::parse(&start_id).map_err(|e| EvalError::Parse(e.to_string()))?;
        if matches!(self.peek(), Token::Colon) {
            self.bump();
            let end_id = match self.bump() {
                Token::Ident(id) => id,
                other => {
                    return Err(EvalError::Parse(format!(
                        "expected end of range, got {other:?}"
                    )))
                }
            };
            let end = CellAddr::parse(&end_id).map_err(|e| EvalError::Parse(e.to_string()))?;
            Ok(Expr::Range(RangeRef::new(sheet, start, end)))
        } else {
            Ok(Expr::Cell(CellRef { sheet, addr: start }))
        }
    }

    fn parse_args(&mut self) -> Result<Vec<Expr>, EvalError> {
        let mut args = Vec::new();
        if matches!(self.peek(), Token::RParen) {
            self.bump();
            return Ok(args);
        }
        loop {
            args.push(self.parse_comparison()?);
            match self.bump() {
                Token::Comma => continue,
                Token::RParen => break,
                other => {
                    return Err(EvalError::Parse(format!(
                        "expected ',' or ')', got {other:?}"
                    )))
                }
            }
        }
        Ok(args)
    }

    fn parse_array(&mut self) -> Result<Expr, EvalError> {
        let mut rows: Vec<Vec<Expr>> = Vec::new();
        let mut row: Vec<Expr> = Vec::new();
        if matches!(self.peek(), Token::RBrace) {
            self.bump();
            return Ok(Expr::Array(rows));
        }
        loop {
            row.push(self.parse_comparison()?);
            match self.bump() {
                Token::Comma => continue,
                Token::Semicolon => {
                    rows.push(std::mem::take(&mut row));
                }
                Token::RBrace => {
                    rows.push(row);
                    break;
                }
                other => {
                    return Err(EvalError::Parse(format!(
                        "expected ',' ';' or '}}', got {other:?}"
                    )))
                }
            }
        }
        Ok(Expr::Array(rows))
    }
}

fn is_bool(s: &str) -> bool {
    s.eq_ignore_ascii_case("TRUE") || s.eq_ignore_ascii_case("FALSE")
}

fn looks_like_cell(s: &str) -> bool {
    CellAddr::parse(s).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic() {
        assert!(matches!(parse("=1+2").unwrap(), Expr::Binary { .. }));
        assert!(matches!(parse("SUM(A1:A3)").unwrap(), Expr::Call { .. }));
        match parse("=Sheet1!B2").unwrap() {
            Expr::Cell(c) => {
                assert_eq!(c.sheet.as_deref(), Some("Sheet1"));
                assert_eq!(c.addr.a1(), "B2");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_error_literal() {
        match parse("=#DIV/0!").unwrap() {
            Expr::Error(ExcelError::Div0) => {}
            other => panic!("{other:?}"),
        }
    }
}
