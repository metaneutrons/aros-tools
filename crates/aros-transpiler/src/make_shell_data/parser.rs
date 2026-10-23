mod lexer;
pub(super) mod recipe;
#[cfg(test)]
mod tests;

use lexer::{Lexer, Token};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Statement {
    Simple(Vec<Word>),
    For {
        name: String,
        words: Vec<Word>,
        body: Vec<Self>,
    },
    If {
        condition: Vec<Self>,
        then_body: Vec<Self>,
        else_body: Vec<Self>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Word {
    pub(super) parts: Vec<Part>,
}

impl Word {
    pub(super) fn keyword(&self, expected: &str) -> bool {
        matches!(self.parts.as_slice(), [Part::Literal { text, quoted: false }] if text.as_str() == expected)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Part {
    Literal { text: String, quoted: bool },
    Variable { name: String, quoted: bool },
}

const MAX_COMMAND_BYTES: usize = 1024 * 1024;
const MAX_NESTING: usize = 32;
const MAX_STATEMENTS: usize = 4096;

pub(super) fn parse(command: &str) -> Result<Vec<Statement>, String> {
    if command.len() > MAX_COMMAND_BYTES {
        return Err("shell-data command is too large".into());
    }
    if command
        .chars()
        .any(|ch| ch.is_control() && !matches!(ch, '\t' | '\n'))
    {
        return Err("unsupported shell control character".into());
    }

    let tokens = Lexer::new(command).lex()?;
    let mut parser = Parser {
        tokens,
        cursor: 0,
        statement_count: 0,
    };
    let statements = parser.parse_list(&[], 0)?;
    if parser.cursor != parser.tokens.len() {
        return Err("unexpected shell syntax".into());
    }
    Ok(statements)
}

struct Parser {
    tokens: Vec<Token>,
    cursor: usize,
    statement_count: usize,
}

impl Parser {
    fn parse_list(&mut self, stop_words: &[&str], depth: usize) -> Result<Vec<Statement>, String> {
        let mut statements = Vec::new();
        loop {
            self.skip_newlines();
            if self.at_end() || self.at_keyword_in(stop_words) {
                return Ok(statements);
            }
            if self.at_semicolon() {
                return Err("unexpected shell separator".into());
            }

            statements.push(self.parse_statement(depth)?);
            match self.tokens.get(self.cursor) {
                None => return Ok(statements),
                Some(Token::Semicolon) => {
                    self.cursor += 1;
                    self.skip_newlines();
                    if self.at_semicolon() {
                        return Err("repeated shell separator".into());
                    }
                    if self.at_end() || self.at_keyword_in(stop_words) {
                        return Ok(statements);
                    }
                }
                Some(Token::Newline) => {
                    self.skip_newlines();
                    if self.at_semicolon() {
                        return Err("unexpected shell separator".into());
                    }
                    if self.at_end() || self.at_keyword_in(stop_words) {
                        return Ok(statements);
                    }
                }
                _ => return Err("expected shell command separator".into()),
            }
        }
    }

    fn parse_statement(&mut self, depth: usize) -> Result<Statement, String> {
        self.statement_count += 1;
        if self.statement_count > MAX_STATEMENTS {
            return Err("shell-data command has too many statements".into());
        }
        let Some(Token::Word(word)) = self.tokens.get(self.cursor) else {
            return Err("expected shell statement".into());
        };
        if word.keyword("for") {
            return self.parse_for(depth);
        }
        if word.keyword("if") {
            return self.parse_if(depth);
        }
        if is_control_word(word) {
            return Err("unsupported or unexpected shell control syntax".into());
        }
        self.parse_simple()
    }

    fn parse_for(&mut self, depth: usize) -> Result<Statement, String> {
        Self::check_nesting(depth)?;
        self.cursor += 1;
        let name_word = self.take_word("expected for-loop variable")?;
        let name = literal_identifier(&name_word)
            .ok_or_else(|| "invalid for-loop variable".to_string())?;
        if !self
            .take_word("expected for-loop in keyword")?
            .keyword("in")
        {
            return Err("expected for-loop in keyword".into());
        }

        let mut words = Vec::new();
        while let Some(Token::Word(word)) = self.tokens.get(self.cursor) {
            words.push(word.clone());
            self.cursor += 1;
        }
        self.consume_for_separator()?;
        if !self.take_word("expected do keyword")?.keyword("do") {
            return Err("expected do keyword".into());
        }

        let body = self.parse_list(&["done"], depth + 1)?;
        if body.is_empty() {
            return Err("expected nonempty for-loop body".into());
        }
        self.expect_keyword("done", "expected done keyword")?;
        Ok(Statement::For { name, words, body })
    }

    fn parse_if(&mut self, depth: usize) -> Result<Statement, String> {
        Self::check_nesting(depth)?;
        self.cursor += 1;
        let condition = self.parse_list(&["then"], depth + 1)?;
        if condition.is_empty() {
            return Err("expected if condition".into());
        }
        self.expect_keyword("then", "expected then keyword")?;

        let then_body = self.parse_list(&["else", "fi"], depth + 1)?;
        if then_body.is_empty() {
            return Err("expected nonempty then body".into());
        }
        let has_else = self.at_keyword("else");
        let else_body = if has_else {
            self.cursor += 1;
            let body = self.parse_list(&["fi"], depth + 1)?;
            if body.is_empty() {
                return Err("expected nonempty else body".into());
            }
            body
        } else {
            Vec::new()
        };
        self.expect_keyword("fi", "expected fi keyword")?;

        Ok(Statement::If {
            condition,
            then_body,
            else_body,
        })
    }

    fn parse_simple(&mut self) -> Result<Statement, String> {
        let mut words = Vec::new();
        while let Some(Token::Word(word)) = self.tokens.get(self.cursor) {
            words.push(word.clone());
            self.cursor += 1;
        }
        if words.is_empty() {
            return Err("expected shell statement".into());
        }
        Ok(Statement::Simple(words))
    }

    fn consume_for_separator(&mut self) -> Result<(), String> {
        match self.tokens.get(self.cursor) {
            Some(Token::Semicolon) => self.cursor += 1,
            Some(Token::Newline) => self.skip_newlines(),
            _ => return Err("expected separator before for-loop body".into()),
        }
        self.skip_newlines();
        if self.at_semicolon() {
            return Err("repeated shell separator".into());
        }
        Ok(())
    }

    fn take_word(&mut self, error: &str) -> Result<Word, String> {
        let Some(Token::Word(word)) = self.tokens.get(self.cursor) else {
            return Err(error.into());
        };
        self.cursor += 1;
        Ok(word.clone())
    }

    fn expect_keyword(&mut self, keyword: &str, error: &str) -> Result<(), String> {
        if self.take_word(error)?.keyword(keyword) {
            Ok(())
        } else {
            Err(error.into())
        }
    }

    fn check_nesting(depth: usize) -> Result<(), String> {
        if depth >= MAX_NESTING {
            return Err("shell-data command is nested too deeply".into());
        }
        Ok(())
    }

    fn skip_newlines(&mut self) {
        while matches!(self.tokens.get(self.cursor), Some(Token::Newline)) {
            self.cursor += 1;
        }
    }

    fn at_semicolon(&self) -> bool {
        matches!(self.tokens.get(self.cursor), Some(Token::Semicolon))
    }

    const fn at_end(&self) -> bool {
        self.cursor == self.tokens.len()
    }

    fn at_keyword(&self, keyword: &str) -> bool {
        matches!(self.tokens.get(self.cursor), Some(Token::Word(word)) if word.keyword(keyword))
    }

    fn at_keyword_in(&self, keywords: &[&str]) -> bool {
        keywords.iter().any(|keyword| self.at_keyword(keyword))
    }
}

fn literal_identifier(word: &Word) -> Option<String> {
    match word.parts.as_slice() {
        [Part::Literal {
            text,
            quoted: false,
        }] if valid_identifier(text) => Some(text.clone()),
        _ => None,
    }
}

fn is_control_word(word: &Word) -> bool {
    [
        "then", "else", "elif", "fi", "do", "done", "while", "until", "case", "esac", "function",
        "select", "coproc", "!", "[[", "]]",
    ]
    .iter()
    .any(|keyword| word.keyword(keyword))
}

fn valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(is_identifier_start) && chars.all(is_identifier_continue)
}

const fn is_identifier_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

const fn is_identifier_continue(ch: char) -> bool {
    is_identifier_start(ch) || ch.is_ascii_digit()
}
