//! Shell-specific quoting for the cmd and PowerShell launch grammars.

use std::{iter::Peekable, str::CharIndices};

use super::Shell;

pub(super) struct Token {
    pub(super) text: String,
    pub(super) raw: String,
    pub(super) literal: bool,
    pub(super) boundary: bool,
    pub(super) subexpressions: Vec<String>,
}

impl Token {
    pub(super) fn argument(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            raw: text.to_owned(),
            literal: false,
            boundary: false,
            subexpressions: Vec::new(),
        }
    }

    pub(super) fn name(&self) -> String {
        self.text.trim_start_matches('@').to_ascii_lowercase()
    }
}

pub(super) fn command_tokens(script: &str, shell: Shell) -> Result<Vec<Token>, &'static str> {
    let mut tokens = Vec::new();
    let mut chars = script.char_indices().peekable();
    while let Some(&(start, ch)) = chars.peek() {
        if boundary(ch, shell) {
            chars.next();
            let mut text = ch.to_string();
            if matches!(ch, '&' | '|') && chars.peek().is_some_and(|(_, next)| *next == ch) {
                chars.next();
                text.push(ch);
            }
            tokens.push(Token {
                raw: text.clone(),
                text,
                literal: false,
                boundary: true,
                subexpressions: Vec::new(),
            });
        } else if ch.is_whitespace() {
            chars.next();
        } else {
            tokens.push(word(script, start, &mut chars, shell)?);
        }
    }
    Ok(tokens)
}

fn word(
    script: &str,
    start: usize,
    chars: &mut Peekable<CharIndices<'_>>,
    shell: Shell,
) -> Result<Token, &'static str> {
    let mut token = Token::argument("");
    read_word(script, chars, shell, &mut token)?;
    let end = chars.peek().map_or(script.len(), |(offset, _)| *offset);
    token.raw = script[start..end].to_owned();
    Ok(token)
}

fn read_word(
    script: &str,
    chars: &mut Peekable<CharIndices<'_>>,
    shell: Shell,
    token: &mut Token,
) -> Result<(), &'static str> {
    let (mut quote, mut quoted, mut plain) = (None, false, false);
    while let Some(&(offset, ch)) = chars.peek() {
        let separator = boundary(ch, shell) && (ch != '=' || token.text.starts_with('$'));
        if quote.is_none() && (ch.is_whitespace() || separator) {
            break;
        }
        chars.next();
        if shell.is_powershell()
            && quote == Some('"')
            && ch == '$'
            && chars.peek().is_some_and(|(_, next)| *next == '(')
        {
            token.subexpressions.push(subexpression(script, chars)?);
            let end = chars.peek().map_or(script.len(), |(index, _)| *index);
            token.text.push_str(&script[offset..end]);
        } else if quote.is_some() {
            quoted_character(ch, shell, &mut quote, chars, &mut token.text)?;
        } else if ch == '"' || (shell.is_powershell() && ch == '\'') {
            quote = Some(ch);
            quoted = true;
        } else {
            plain = true;
            if ch == shell.escape() {
                escaped_character(chars, &mut token.text)?;
            } else {
                token.text.push(ch);
            }
        }
    }
    if quote.is_some() {
        return Err("unterminated interpreter payload quote");
    }
    token.literal = quoted && !plain;
    Ok(())
}

fn quoted_character(
    ch: char,
    shell: Shell,
    quote: &mut Option<char>,
    chars: &mut Peekable<CharIndices<'_>>,
    text: &mut String,
) -> Result<(), &'static str> {
    if Some(ch) == *quote {
        if shell.is_powershell() && chars.peek().is_some_and(|(_, next)| *next == ch) {
            chars.next();
            text.push(ch);
        } else {
            *quote = None;
        }
    } else if shell.is_powershell() && *quote == Some('"') && ch == '`' {
        escaped_character(chars, text)?;
    } else {
        text.push(ch);
    }
    Ok(())
}

fn escaped_character(
    chars: &mut Peekable<CharIndices<'_>>,
    text: &mut String,
) -> Result<(), &'static str> {
    let (_, next) = chars
        .next()
        .ok_or("unfinished interpreter payload escape")?;
    text.push(next);
    Ok(())
}

/// Read a balanced PowerShell subexpression without decoding its script text.
fn subexpression(
    script: &str,
    chars: &mut Peekable<CharIndices<'_>>,
) -> Result<String, &'static str> {
    let (open, _) = chars.next().ok_or("missing PowerShell subexpression")?;
    let mut contexts = vec![None];
    while let Some((offset, ch)) = chars.next() {
        let Some(quote) = contexts.last_mut() else {
            break;
        };
        match (*quote, ch) {
            (None | Some('"'), '`') => {
                chars.next();
            }
            (Some(delimiter), ch) if ch == delimiter => {
                if chars.peek().is_some_and(|(_, next)| *next == delimiter) {
                    chars.next();
                } else {
                    *quote = None;
                }
            }
            (Some('"'), '$') if chars.peek().is_some_and(|(_, next)| *next == '(') => {
                chars.next();
                contexts.push(None);
            }
            (None, '\'' | '"') => *quote = Some(ch),
            (None, '(') => contexts.push(None),
            (None, ')') => {
                contexts.pop();
                if contexts.is_empty() {
                    return Ok(script[open + 1..offset].to_owned());
                }
            }
            _ => {}
        }
    }
    Err("unterminated PowerShell subexpression")
}

const fn boundary(ch: char, shell: Shell) -> bool {
    // Both grammars are inspected conservatively, including unreachable blocks.
    matches!(ch, '&' | ';' | '|' | '{' | '}' | '(' | ')' | '\r' | '\n')
        || (shell.is_powershell() && ch == '=')
}
