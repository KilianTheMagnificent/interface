//! Quote-aware shell tokenizer for the agent approval policy.
//!
//! This is not a shell. It splits a command line into simple-command segments
//! so the policy can look at each program, its words and its redirect targets.
//! Anything it cannot model faithfully (heredocs, subshell parentheses,
//! unterminated quotes, dangling operators) is a parse error, and the policy
//! turns every parse error into a request to the owner. `#` is an ordinary
//! character: treating it as a comment could hide code from the policy in a
//! shell that does not honour comments.

/// One simple command between control operators.
#[derive(Debug, PartialEq, Eq)]
pub struct Segment {
    /// Words with quotes and escapes removed, assignments and wrappers kept.
    pub words: Vec<String>,
    /// Output redirect targets (`>`, `>>`, `>|`, `2>`, `&>`, `<>`, `>&file`).
    pub redirects: Vec<String>,
    /// Input redirect sources (`<`). Kept apart so reads are not scope-checked.
    pub inputs: Vec<String>,
    /// `$(`, backticks, `<(` or `>(` appeared anywhere in the segment.
    pub substitution: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ShellParseError;

/// Splits on unquoted `&&`, `||`, `;`, `|`, `&` and newlines. Quotes are removed from words.
/// `$(`, backticks and `<(`/`>(` set `substitution`. `>`, `>>`, `2>`, `&>`, `>|` capture the next word as a redirect target.
#[allow(clippy::too_many_lines)] // One state machine reads best as one match.
pub fn parse(command: &str) -> Result<Vec<Segment>, ShellParseError> {
    let chars: Vec<char> = command.chars().collect();
    let mut builder = Builder::default();
    let mut quote = Quote::None;
    // Open `$(`, `<(` and `>(` groups. Inside one, operators are literal text:
    // the segment is already flagged, so its exact shape no longer matters.
    let mut depth = 0_usize;
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        let next = chars.get(index + 1).copied();
        match quote {
            Quote::Single => {
                if c == '\'' {
                    quote = Quote::None;
                } else {
                    builder.push(c);
                }
            }
            Quote::Double => match (c, next) {
                ('"', _) => quote = Quote::None,
                ('\\', Some(escaped @ ('$' | '`' | '"' | '\\'))) => {
                    builder.push(escaped);
                    index += 1;
                }
                ('\\', Some('\n')) => index += 1,
                ('`', _) | ('$', Some('(')) => {
                    builder.substitution = true;
                    builder.push(c);
                }
                _ => builder.push(c),
            },
            Quote::Backtick => {
                builder.push(c);
                if c == '`' {
                    quote = Quote::None;
                } else if let ('\\', Some(escaped)) = (c, next) {
                    builder.push(escaped);
                    index += 1;
                }
            }
            Quote::None => match (c, next) {
                ('\\', Some('\n')) => index += 1,
                ('\\', Some(escaped)) => {
                    builder.push_quoted(escaped);
                    index += 1;
                }
                ('\\', None) => return Err(ShellParseError),
                ('\'', _) => {
                    builder.start_quoted();
                    quote = Quote::Single;
                }
                ('"', _) => {
                    builder.start_quoted();
                    quote = Quote::Double;
                }
                ('`', _) => {
                    builder.substitution = true;
                    builder.push(c);
                    quote = Quote::Backtick;
                }
                ('$' | '<' | '>', Some('(')) => {
                    builder.substitution = true;
                    builder.push(c);
                    builder.push('(');
                    depth += 1;
                    index += 1;
                }
                _ if depth > 0 => {
                    match c {
                        '(' => depth += 1,
                        ')' => depth -= 1,
                        _ => {}
                    }
                    builder.push(c);
                }
                (' ' | '\t', _) => builder.finish_word(),
                ('\n' | ';', _) => builder.end_segment()?,
                ('&', Some('&')) => {
                    builder.end_segment()?;
                    index += 1;
                }
                ('&', Some('>')) => {
                    builder.begin_redirect(Pending::Output)?;
                    index += 1 + redirect_suffix(&chars, index + 1);
                }
                ('&', _) => builder.end_segment()?,
                ('|', _) => {
                    if matches!(next, Some('|' | '&')) {
                        index += 1;
                    }
                    builder.end_segment()?;
                }
                ('>', Some('&')) => {
                    builder.begin_redirect(Pending::OutputOrDuplicate)?;
                    index += 1;
                }
                ('>', _) => {
                    builder.begin_redirect(Pending::Output)?;
                    index += redirect_suffix(&chars, index);
                }
                ('<', Some('>')) => {
                    builder.begin_redirect(Pending::Output)?;
                    index += 1;
                }
                // Heredocs and herestrings carry text the policy cannot see.
                ('<', Some('<')) | ('(' | ')', _) => return Err(ShellParseError),
                ('<', Some('&')) => {
                    builder.begin_redirect(Pending::InputOrDuplicate)?;
                    index += 1;
                }
                ('<', _) => builder.begin_redirect(Pending::Input)?,
                _ => builder.push(c),
            },
        }
        index += 1;
    }
    if quote != Quote::None || depth > 0 {
        return Err(ShellParseError);
    }
    builder.end_segment()?;
    Ok(builder.segments)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Quote {
    None,
    Single,
    Double,
    Backtick,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Pending {
    Output,
    OutputOrDuplicate,
    Input,
    InputOrDuplicate,
}

#[derive(Default)]
struct Builder {
    segments: Vec<Segment>,
    words: Vec<String>,
    redirects: Vec<String>,
    inputs: Vec<String>,
    substitution: bool,
    word: Option<String>,
    word_quoted: bool,
    pending: Option<Pending>,
}

impl Builder {
    fn push(&mut self, c: char) {
        self.word.get_or_insert_with(String::new).push(c);
    }

    fn push_quoted(&mut self, c: char) {
        self.push(c);
        self.word_quoted = true;
    }

    fn start_quoted(&mut self) {
        self.word.get_or_insert_with(String::new);
        self.word_quoted = true;
    }

    fn finish_word(&mut self) {
        let Some(word) = self.word.take() else {
            return;
        };
        self.word_quoted = false;
        match self.pending.take() {
            None => self.words.push(word),
            Some(Pending::Output) => self.redirects.push(word),
            Some(Pending::Input) => self.inputs.push(word),
            Some(Pending::OutputOrDuplicate) if !is_descriptor(&word) => self.redirects.push(word),
            Some(Pending::InputOrDuplicate) if !is_descriptor(&word) => self.inputs.push(word),
            Some(Pending::OutputOrDuplicate | Pending::InputOrDuplicate) => {}
        }
    }

    fn begin_redirect(&mut self, kind: Pending) -> Result<(), ShellParseError> {
        // An unquoted number written directly before the operator names a descriptor.
        let descriptor = !self.word_quoted
            && self
                .word
                .as_deref()
                .is_some_and(|word| !word.is_empty() && word.bytes().all(|b| b.is_ascii_digit()));
        if descriptor {
            self.word = None;
        } else {
            self.finish_word();
        }
        if self.pending.is_some() {
            return Err(ShellParseError);
        }
        self.pending = Some(kind);
        Ok(())
    }

    fn end_segment(&mut self) -> Result<(), ShellParseError> {
        self.finish_word();
        if self.pending.is_some() {
            return Err(ShellParseError);
        }
        if self.words.is_empty()
            && self.redirects.is_empty()
            && self.inputs.is_empty()
            && !self.substitution
        {
            return Ok(());
        }
        self.segments.push(Segment {
            words: std::mem::take(&mut self.words),
            redirects: std::mem::take(&mut self.redirects),
            inputs: std::mem::take(&mut self.inputs),
            substitution: std::mem::take(&mut self.substitution),
        });
        Ok(())
    }
}

/// Extra characters of an output operator whose first `>` is at `at`: the
/// second `>` of `>>`, then the `|` or zsh `!` that forces clobbering.
fn redirect_suffix(chars: &[char], at: usize) -> usize {
    let appended = usize::from(chars.get(at + 1) == Some(&'>'));
    let forced = usize::from(matches!(chars.get(at + 1 + appended), Some('|' | '!')));
    appended + forced
}

/// `>&2`, `<&0`, `>&-` and `>&2-` duplicate or close descriptors and name no file.
fn is_descriptor(word: &str) -> bool {
    let digits = word.strip_suffix('-').unwrap_or(word);
    word == "-" || (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::{Segment, ShellParseError, parse};

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn words(values: &[&str]) -> Segment {
        Segment {
            words: strings(values),
            redirects: Vec::new(),
            inputs: Vec::new(),
            substitution: false,
        }
    }

    fn with_redirects(values: &[&str], redirects: &[&str]) -> Segment {
        Segment {
            redirects: strings(redirects),
            ..words(values)
        }
    }

    fn substituted(values: &[&str]) -> Segment {
        Segment {
            substitution: true,
            ..words(values)
        }
    }

    #[test]
    fn simple_command_is_one_segment() {
        assert_eq!(parse("ls -la"), Ok(vec![words(&["ls", "-la"])]));
    }

    #[test]
    fn control_operators_split_segments() {
        assert_eq!(
            parse("cd a && cargo test | tee log.txt"),
            Ok(vec![
                words(&["cd", "a"]),
                words(&["cargo", "test"]),
                words(&["tee", "log.txt"]),
            ])
        );
        assert_eq!(
            parse("a; b || c & d\ne |& f"),
            Ok(vec![
                words(&["a"]),
                words(&["b"]),
                words(&["c"]),
                words(&["d"]),
                words(&["e"]),
                words(&["f"]),
            ])
        );
        assert_eq!(
            parse("nohup git push &"),
            Ok(vec![words(&["nohup", "git", "push"])])
        );
        assert_eq!(parse("a;;b"), Ok(vec![words(&["a"]), words(&["b"])]));
    }

    #[test]
    fn quotes_protect_operators_and_are_removed() {
        assert_eq!(
            parse("echo \"a && b\""),
            Ok(vec![words(&["echo", "a && b"])])
        );
        assert_eq!(parse("echo 'x;y'"), Ok(vec![words(&["echo", "x;y"])]));
        assert_eq!(
            parse("git commit -m \"it's\""),
            Ok(vec![words(&["git", "commit", "-m", "it's"])])
        );
        assert_eq!(parse("g\"i\"t p'u'sh"), Ok(vec![words(&["git", "push"])]));
        assert_eq!(parse("echo '' \"\""), Ok(vec![words(&["echo", "", ""])]));
        assert_eq!(
            parse("echo \"a\\\"b\\\\c\\d\""),
            Ok(vec![words(&["echo", "a\"b\\c\\d"])])
        );
        assert_eq!(parse("echo 'a\\b'"), Ok(vec![words(&["echo", "a\\b"])]));
        assert_eq!(parse("echo \"a\nb\""), Ok(vec![words(&["echo", "a\nb"])]));
    }

    #[test]
    fn backslash_escapes_the_next_character() {
        assert_eq!(parse("a\\ b"), Ok(vec![words(&["a b"])]));
        assert_eq!(
            parse("echo \\;x \\&\\&"),
            Ok(vec![words(&["echo", ";x", "&&"])])
        );
        assert_eq!(parse("\\git push"), Ok(vec![words(&["git", "push"])]));
        assert_eq!(parse("git \\\npush"), Ok(vec![words(&["git", "push"])]));
    }

    #[test]
    fn heredocs_and_herestrings_are_unsupported() {
        assert_eq!(parse("cat <<EOF"), Err(ShellParseError));
        assert_eq!(parse("cat <<-EOF"), Err(ShellParseError));
        assert_eq!(parse("cat <<< hi"), Err(ShellParseError));
    }

    #[test]
    fn substitutions_are_flagged() {
        assert_eq!(
            parse("echo $(whoami)"),
            Ok(vec![substituted(&["echo", "$(whoami)"])])
        );
        assert_eq!(parse("echo `id`"), Ok(vec![substituted(&["echo", "`id`"])]));
        assert_eq!(
            parse("echo `a b` c"),
            Ok(vec![substituted(&["echo", "`a b`", "c"])])
        );
        assert_eq!(
            parse("echo $(a; b && c) d"),
            Ok(vec![substituted(&["echo", "$(a; b && c)", "d"])])
        );
        assert_eq!(
            parse("diff <(ls a) >(cat)"),
            Ok(vec![substituted(&["diff", "<(ls a)", ">(cat)"])])
        );
        assert_eq!(
            parse("echo \"$(id)\""),
            Ok(vec![substituted(&["echo", "$(id)"])])
        );
        assert_eq!(
            parse("echo \"`id`\""),
            Ok(vec![substituted(&["echo", "`id`"])])
        );
        assert_eq!(
            parse("echo $((1+2))"),
            Ok(vec![substituted(&["echo", "$((1+2))"])])
        );
        // A single-quoted `$(` is literal text.
        assert_eq!(parse("echo '$(id)'"), Ok(vec![words(&["echo", "$(id)"])]));
        // Only the segment that holds the substitution is flagged.
        assert_eq!(
            parse("ls && echo $(id)"),
            Ok(vec![words(&["ls"]), substituted(&["echo", "$(id)"])])
        );
    }

    #[test]
    fn variables_stay_literal() {
        assert_eq!(
            parse("rm -rf $DIR"),
            Ok(vec![words(&["rm", "-rf", "$DIR"])])
        );
        assert_eq!(parse("eval \"$CMD\""), Ok(vec![words(&["eval", "$CMD"])]));
    }

    #[test]
    fn output_redirects_capture_their_target() {
        assert_eq!(
            parse("printf x > out.txt 2> err.txt"),
            Ok(vec![with_redirects(
                &["printf", "x"],
                &["out.txt", "err.txt"]
            )])
        );
        assert_eq!(
            parse("cmd &> /dev/null"),
            Ok(vec![with_redirects(&["cmd"], &["/dev/null"])])
        );
        assert_eq!(
            parse("a >>log >|clobber &>>both 2>>e <>rw"),
            Ok(vec![with_redirects(
                &["a"],
                &["log", "clobber", "both", "e", "rw"]
            )])
        );
        assert_eq!(
            parse("echo hi>out"),
            Ok(vec![with_redirects(&["echo", "hi"], &["out"])])
        );
        assert_eq!(
            parse("echo hi >\"my file\""),
            Ok(vec![with_redirects(&["echo", "hi"], &["my file"])])
        );
        // A quoted digit before `>` is a word, not a file descriptor.
        assert_eq!(
            parse("echo '2'>x"),
            Ok(vec![with_redirects(&["echo", "2"], &["x"])])
        );
    }

    #[test]
    fn descriptor_duplication_has_no_target() {
        assert_eq!(parse("make 2>&1"), Ok(vec![words(&["make"])]));
        assert_eq!(parse("make >&2 <&0 2>&-"), Ok(vec![words(&["make"])]));
        assert_eq!(
            parse("make >&build.log"),
            Ok(vec![with_redirects(&["make"], &["build.log"])])
        );
        assert_eq!(
            parse("make > out 2>&1 | tail -5"),
            Ok(vec![
                with_redirects(&["make"], &["out"]),
                words(&["tail", "-5"])
            ])
        );
    }

    #[test]
    fn input_redirects_are_kept_apart() {
        assert_eq!(
            parse("xargs git push < list"),
            Ok(vec![Segment {
                inputs: strings(&["list"]),
                ..words(&["xargs", "git", "push"])
            }])
        );
        assert_eq!(
            parse("< list xargs git push"),
            Ok(vec![Segment {
                inputs: strings(&["list"]),
                ..words(&["xargs", "git", "push"])
            }])
        );
    }

    #[test]
    fn hash_is_an_ordinary_word() {
        // A shell without interactive comments runs what follows `#`, so the
        // policy must see it too.
        assert_eq!(
            parse("ls # rm -rf /"),
            Ok(vec![words(&["ls", "#", "rm", "-rf", "/"])])
        );
        assert_eq!(
            parse("echo # ; git push"),
            Ok(vec![words(&["echo", "#"]), words(&["git", "push"])])
        );
    }

    #[test]
    fn zsh_clobber_redirects_capture_their_target() {
        assert_eq!(
            parse("a >!x >>!y >>|z &>!w &>|v"),
            Ok(vec![with_redirects(&["a"], &["x", "y", "z", "w", "v"])])
        );
        assert_eq!(parse("a >>& x"), Err(ShellParseError));
    }

    #[test]
    fn malformed_input_is_an_error() {
        for command in [
            "\"abc",
            "echo 'abc",
            "echo `id",
            "echo $(id",
            "echo trailing\\",
            "echo >",
            "echo > && ls",
            "echo > > x",
            "(cd a && ls)",
            "f() { ls; }",
            "echo )",
            "cat <",
        ] {
            assert_eq!(parse(command), Err(ShellParseError), "{command:?}");
        }
    }

    #[test]
    fn empty_input_has_no_segments() {
        assert_eq!(parse(""), Ok(Vec::new()));
        assert_eq!(parse("  \t\n "), Ok(Vec::new()));
        assert_eq!(parse(" ; && "), Ok(Vec::new()));
    }

    #[test]
    fn assignments_are_kept_as_words() {
        assert_eq!(
            parse("FOO=1 BAR=2 npm test"),
            Ok(vec![words(&["FOO=1", "BAR=2", "npm", "test"])])
        );
        assert_eq!(
            parse("FOO=\"a b\" cmd"),
            Ok(vec![words(&["FOO=a b", "cmd"])])
        );
    }
}
