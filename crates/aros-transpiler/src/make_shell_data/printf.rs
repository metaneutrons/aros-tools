use super::Evaluator;

enum FormatPart {
    Text(String),
    Value,
}

impl Evaluator<'_, '_> {
    pub(super) fn printf(&mut self, args: &[String]) -> Result<u8, String> {
        if args[0].starts_with('-') {
            return Err("shell printf options are unsupported".into());
        }
        let mut format = Vec::new();
        let mut literal = String::new();
        let mut chars = args[0].chars();
        let mut slots = 0usize;
        while let Some(character) = chars.next() {
            match character {
                '%' => match chars.next() {
                    Some('%') => literal.push('%'),
                    Some('s') => {
                        format.push(FormatPart::Text(std::mem::take(&mut literal)));
                        format.push(FormatPart::Value);
                        slots += 1;
                    }
                    _ => return Err("unsupported shell printf conversion".into()),
                },
                '\\' => literal.push(match chars.next() {
                    Some('n') => '\n',
                    Some('t') => '\t',
                    Some('r') => '\r',
                    Some('b') => '\u{8}',
                    Some('a') => '\u{7}',
                    Some('v') => '\u{b}',
                    Some('f') => '\u{c}',
                    Some('\\') => '\\',
                    _ => return Err("unsupported shell printf escape".into()),
                }),
                _ => literal.push(character),
            }
        }
        format.push(FormatPart::Text(literal));
        let values = &args[1..];
        let repeats = if slots == 0 {
            1
        } else {
            values.len().div_ceil(slots).max(1)
        };
        let mut index = 0;
        for _ in 0..repeats {
            self.charge(1)?;
            for part in &format {
                match part {
                    FormatPart::Text(text) => self.append(text)?,
                    FormatPart::Value => {
                        self.append(values.get(index).map_or("", String::as_str))?;
                        index += 1;
                    }
                }
            }
        }
        Ok(0)
    }
}
