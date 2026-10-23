use super::{Part, Word};

const MAX_TOKENS: usize = 16_384;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Token {
    Word(Word),
    Semicolon,
    Newline,
    Pipe,
    AndIf,
    OrIf,
    Redirection {
        descriptor: u32,
        kind: super::recipe::RecipeRedirectionKind,
    },
}

pub(super) struct Lexer {
    chars: Vec<char>,
    cursor: usize,
    token_count: usize,
    recipe_syntax: bool,
}

impl Lexer {
    pub(super) fn new(input: &str) -> Self {
        Self {
            chars: input.chars().collect(),
            cursor: 0,
            token_count: 0,
            recipe_syntax: false,
        }
    }

    pub(super) fn lex_recipe(mut self) -> Result<Vec<Token>, String> {
        self.recipe_syntax = true;
        self.lex()
    }

    pub(super) fn lex(mut self) -> Result<Vec<Token>, String> {
        let mut tokens = Vec::new();
        while self.cursor < self.chars.len() {
            match self.chars[self.cursor] {
                ' ' | '\t' => self.cursor += 1,
                '\n' => {
                    self.cursor += 1;
                    self.push(&mut tokens, Token::Newline)?;
                }
                ';' => {
                    self.cursor += 1;
                    self.push(&mut tokens, Token::Semicolon)?;
                }
                '\\' if self.chars.get(self.cursor + 1) == Some(&'\n') => self.cursor += 2,
                '#' => self.skip_comment(),
                '&' if self.recipe_syntax && self.chars.get(self.cursor + 1) == Some(&'&') => {
                    self.cursor += 2;
                    self.push(&mut tokens, Token::AndIf)?;
                }
                '|' if self.recipe_syntax => {
                    self.cursor += 1;
                    let token = if self.chars.get(self.cursor) == Some(&'|') {
                        self.cursor += 1;
                        Token::OrIf
                    } else {
                        Token::Pipe
                    };
                    self.push(&mut tokens, token)?;
                }
                '<' | '>' if self.recipe_syntax => {
                    let token = self.lex_redirection(None)?;
                    self.push(&mut tokens, token)?;
                }
                ch if ch.is_ascii_digit() && self.recipe_syntax && self.io_number().is_some() => {
                    let end = self.io_number().ok_or("invalid shell descriptor")?;
                    let text: String = self.chars[self.cursor..end]
                        .iter()
                        .filter(|ch| ch.is_ascii_digit())
                        .collect();
                    let descriptor = text
                        .parse()
                        .map_err(|_| "shell descriptor exceeds admitted range")?;
                    self.cursor = end;
                    let token = self.lex_redirection(Some(descriptor))?;
                    self.push(&mut tokens, token)?;
                }
                '|' | '&' | '<' | '>' | '(' | ')' | '{' | '}' => {
                    return Err("unsupported shell syntax".into());
                }
                _ => {
                    if let Some(word) = self.lex_word()? {
                        self.push(&mut tokens, Token::Word(word))?;
                    }
                }
            }
        }
        Ok(tokens)
    }

    fn io_number(&self) -> Option<usize> {
        let mut end = self.cursor;
        loop {
            if self.chars.get(end).is_some_and(char::is_ascii_digit) {
                end += 1;
            } else if self.chars.get(end) == Some(&'\\') && self.chars.get(end + 1) == Some(&'\n') {
                end += 2;
            } else {
                break;
            }
        }
        if !self
            .chars
            .get(end)
            .is_some_and(|ch| matches!(ch, '<' | '>'))
        {
            return None;
        }
        Some(end)
    }

    fn lex_redirection(&mut self, descriptor: Option<u32>) -> Result<Token, String> {
        use super::recipe::RecipeRedirectionKind as Kind;
        let output = self.chars[self.cursor] == '>';
        self.cursor += 1;
        let kind = match self.chars.get(self.cursor) {
            Some('>') if output => {
                self.cursor += 1;
                Kind::Append
            }
            Some('&') => {
                self.cursor += 1;
                if output {
                    Kind::DuplicateOutput
                } else {
                    Kind::DuplicateInput
                }
            }
            Some('<' | '>' | '|') => return Err("unsupported shell redirection operator".into()),
            _ => {
                if output {
                    Kind::Output
                } else {
                    Kind::Input
                }
            }
        };
        let descriptor = descriptor.unwrap_or_else(|| u32::from(output));
        Ok(Token::Redirection { descriptor, kind })
    }

    fn push(&mut self, tokens: &mut Vec<Token>, token: Token) -> Result<(), String> {
        self.token_count += 1;
        if self.token_count > MAX_TOKENS {
            return Err("shell-data command has too many tokens".into());
        }
        tokens.push(token);
        Ok(())
    }

    fn skip_comment(&mut self) {
        while self.cursor < self.chars.len() && self.chars[self.cursor] != '\n' {
            self.cursor += 1;
        }
    }

    fn lex_word(&mut self) -> Result<Option<Word>, String> {
        let mut parts = Vec::new();
        let mut started = false;
        while self.cursor < self.chars.len() {
            match self.chars[self.cursor] {
                ' ' | '\t' | '\n' | ';' => break,
                '<' | '>' | '|' | '&' if self.recipe_syntax => break,
                '|' | '&' | '<' | '>' | '(' | ')' | '{' | '}' => {
                    return Err("unsupported shell syntax".into());
                }
                '\'' => {
                    started = true;
                    self.cursor += 1;
                    self.lex_single_quoted(&mut parts)?;
                }
                '"' => {
                    started = true;
                    self.cursor += 1;
                    self.lex_double_quoted(&mut parts)?;
                }
                '\\' => {
                    self.cursor += 1;
                    let Some(escaped) = self.take_char() else {
                        return Err("trailing shell escape".into());
                    };
                    if escaped != '\n' {
                        started = true;
                        push_literal(&mut parts, escaped, true);
                    }
                }
                '#' if !started => {
                    self.skip_comment();
                    break;
                }
                '$' => {
                    started = true;
                    self.cursor += 1;
                    parts.push(self.lex_variable(false)?);
                }
                '`' => return Err("unsupported shell substitution".into()),
                '~' if is_tilde_expansion_position(&parts) => {
                    return Err("unsupported tilde expansion".into());
                }
                _ => {
                    started = true;
                    let ch = self.chars[self.cursor];
                    self.cursor += 1;
                    push_literal(&mut parts, ch, false);
                }
            }
        }
        if started {
            Ok(Some(Word { parts }))
        } else {
            Ok(None)
        }
    }

    fn lex_single_quoted(&mut self, parts: &mut Vec<Part>) -> Result<(), String> {
        let initial_parts = parts.len();
        while let Some(ch) = self.take_char() {
            if ch == '\'' {
                if parts.len() == initial_parts {
                    push_empty_quoted(parts);
                }
                return Ok(());
            }
            push_literal(parts, ch, true);
        }
        Err("unterminated shell quote".into())
    }

    fn lex_double_quoted(&mut self, parts: &mut Vec<Part>) -> Result<(), String> {
        let initial_parts = parts.len();
        while self.cursor < self.chars.len() {
            match self.chars[self.cursor] {
                '"' => {
                    self.cursor += 1;
                    if parts.len() == initial_parts {
                        push_empty_quoted(parts);
                    }
                    return Ok(());
                }
                '\\' => {
                    let Some(&next) = self.chars.get(self.cursor + 1) else {
                        return Err("trailing shell escape".into());
                    };
                    match next {
                        '\n' => self.cursor += 2,
                        '$' | '`' | '"' | '\\' => {
                            self.cursor += 2;
                            push_literal(parts, next, true);
                        }
                        _ => {
                            self.cursor += 1;
                            push_literal(parts, '\\', true);
                        }
                    }
                }
                '$' => {
                    self.cursor += 1;
                    parts.push(self.lex_variable(true)?);
                }
                '`' => return Err("unsupported shell substitution".into()),
                _ => {
                    let ch = self.chars[self.cursor];
                    self.cursor += 1;
                    push_literal(parts, ch, true);
                }
            }
        }
        Err("unterminated shell quote".into())
    }

    fn lex_variable(&mut self, quoted: bool) -> Result<Part, String> {
        let Some(&next) = self.chars.get(self.cursor) else {
            return Err("unsupported shell parameter expansion".into());
        };
        if next == '(' {
            return Err("unsupported shell substitution".into());
        }
        if next == '{' {
            self.cursor += 1;
            let start = self.cursor;
            while self.cursor < self.chars.len() && self.chars[self.cursor] != '}' {
                self.cursor += 1;
            }
            if self.cursor == self.chars.len() {
                return Err("unterminated shell parameter expansion".into());
            }
            let name: String = self.chars[start..self.cursor].iter().collect();
            self.cursor += 1;
            if !valid_identifier(&name) {
                return Err("unsupported shell parameter expansion".into());
            }
            return Ok(Part::Variable { name, quoted });
        }
        if is_identifier_start(next) {
            let start = self.cursor;
            self.cursor += 1;
            while self.cursor < self.chars.len() && is_identifier_continue(self.chars[self.cursor])
            {
                self.cursor += 1;
            }
            let name: String = self.chars[start..self.cursor].iter().collect();
            return Ok(Part::Variable { name, quoted });
        }
        Err("unsupported shell parameter expansion".into())
    }

    fn take_char(&mut self) -> Option<char> {
        let ch = self.chars.get(self.cursor).copied()?;
        self.cursor += 1;
        Some(ch)
    }
}

fn push_literal(parts: &mut Vec<Part>, ch: char, quoted: bool) {
    if let Some(Part::Literal {
        text,
        quoted: previous,
    }) = parts.last_mut()
    {
        if *previous == quoted {
            text.push(ch);
            return;
        }
    }
    parts.push(Part::Literal {
        text: ch.to_string(),
        quoted,
    });
}

fn push_empty_quoted(parts: &mut Vec<Part>) {
    if !matches!(parts.last(), Some(Part::Literal { quoted: true, .. })) {
        parts.push(Part::Literal {
            text: String::new(),
            quoted: true,
        });
    }
}

fn is_tilde_expansion_position(parts: &[Part]) -> bool {
    if parts.iter().all(|part| match part {
        Part::Literal { text, .. } => text.is_empty(),
        Part::Variable { .. } => false,
    }) {
        return true;
    }
    let Some(Part::Literal {
        text: prefix,
        quoted: false,
    }) = parts.first()
    else {
        return false;
    };
    let Some((name, value)) = prefix.split_once('=') else {
        return false;
    };
    if !valid_identifier(name) {
        return false;
    }
    let mut last_value_char = value.chars().last();
    for part in &parts[1..] {
        match part {
            Part::Literal { text, .. } => {
                if let Some(ch) = text.chars().last() {
                    last_value_char = Some(ch);
                }
            }
            Part::Variable { .. } => last_value_char = None,
        }
    }
    matches!(last_value_char, None | Some('=' | ':'))
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
