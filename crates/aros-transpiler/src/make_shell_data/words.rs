use super::{
    parser::{Part, Word},
    Evaluator,
};

const MAX_WORDS: usize = 16_384;

impl Evaluator<'_, '_> {
    pub(super) fn expand_words(&mut self, words: &[Word]) -> Result<Vec<String>, String> {
        let mut result = Vec::new();
        for word in words {
            self.charge(1)?;
            let mut value = String::new();
            let mut active_glob = false;
            let mut quoted_glob = false;
            let mut preserve_empty = false;
            for part in &word.parts {
                let (text, quoted) = match part {
                    Part::Literal { text, quoted } => (text.clone(), *quoted),
                    Part::Variable { name, quoted } => {
                        let text = self
                            .variables
                            .get(name)
                            .ok_or("unbound shell data parameter")?
                            .clone();
                        if !quoted && !self.context.default_shell_controls {
                            return Err(
                                "shell parameter splitting needs a bound IFS context".into()
                            );
                        }
                        (text, *quoted)
                    }
                };
                self.charge(text.len())?;
                preserve_empty |= quoted;
                if text.contains(['*', '?', '[']) {
                    if quoted {
                        quoted_glob = true;
                    } else {
                        active_glob = true;
                    }
                }
                if matches!(part, Part::Variable { quoted: false, .. })
                    && text.chars().any(|c| matches!(c, ' ' | '\t' | '\n'))
                {
                    if word.parts.len() != 1 {
                        return Err(
                            "split shell parameters with adjacent word parts are unsupported"
                                .into(),
                        );
                    }
                    for field in text
                        .split([' ', '\t', '\n'])
                        .filter(|field| !field.is_empty())
                    {
                        self.expand_field(
                            field.into(),
                            field.contains(['*', '?', '[']),
                            quoted_glob,
                            &mut result,
                        )?;
                    }
                    value.clear();
                    preserve_empty = false;
                    break;
                }
                if value
                    .len()
                    .checked_add(text.len())
                    .is_none_or(|size| size > super::MAX_OUTPUT)
                {
                    return Err("shell data word exceeds its limit".into());
                }
                value.push_str(&text);
            }
            if !value.is_empty() || preserve_empty {
                self.expand_field(value, active_glob, quoted_glob, &mut result)?;
            }
        }
        Ok(result)
    }

    fn expand_field(
        &mut self,
        value: String,
        active_glob: bool,
        quoted_glob: bool,
        result: &mut Vec<String>,
    ) -> Result<(), String> {
        // An unmatched opening bracket is literal shell text, including the
        // ordinary `[` command. It does not request filesystem expansion.
        let active_glob = active_glob
            && (value.contains(['*', '?'])
                || value
                    .split_once('[')
                    .is_some_and(|(_, tail)| tail.contains(']')));
        if active_glob && quoted_glob {
            return Err(
                "mixed protected and active shell pattern characters are unsupported".into(),
            );
        }
        let values = if active_glob {
            let prefix = format!("{}/", self.cwd);
            let absolute = std::path::Path::new(&value).is_absolute();
            let query = if absolute {
                value.clone()
            } else {
                format!("{prefix}{value}")
            };
            let matches = (self.context.glob)(&query)?;
            if matches.is_empty() {
                vec![value]
            } else {
                matches
                    .into_iter()
                    .map(|path| {
                        if absolute {
                            Ok(path)
                        } else {
                            path.strip_prefix(&prefix)
                                .map(str::to_owned)
                                .ok_or("shell glob result leaves its requested base")
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?
            }
        } else {
            vec![value]
        };
        for value in values {
            if result.len() >= MAX_WORDS {
                return Err("shell data word count exceeds its limit".into());
            }
            self.charge(value.len())?;
            result.push(value);
        }
        Ok(())
    }
}
