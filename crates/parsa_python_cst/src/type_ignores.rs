use parsa_python::CodeIndex;

use crate::Tree;

#[derive(Debug, PartialEq, Eq)]
pub enum TypeIgnoreComment<'db> {
    WithCodes {
        codes: &'db str,
        kind: &'static str,
        codes_start_at_index: CodeIndex,
        codes_of_later_type_ignores: Vec<&'db str>,
    },
    WithoutCode,
}

/// All `# type: ignore` / `# zuban: ignore` comments of a file, scanned once and ordered by
/// position. Only actual comments are scanned, a `#` within e.g. a string literal is never
/// treated as a comment.
#[derive(Debug, Clone, Default)]
pub struct IgnoreDirectives {
    entries: Vec<IgnoreDirective>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoreDirective {
    /// The offset of the `#` that starts the comment containing this directive
    pub hash_start: CodeIndex,
    /// "type" or "zuban"
    pub kind: &'static str,
    /// The span of the raw text within the brackets of e.g. `# type: ignore[a, b]`.
    /// `None` for a bare ignore like `# type: ignore`.
    pub codes_span: Option<(CodeIndex, CodeIndex)>,
}

impl IgnoreDirectives {
    pub fn scan(tree: &Tree) -> Self {
        let code = tree.code();
        let mut entries = vec![];
        let mut pos = 0;
        while let Some(found) = code[pos..].find('#') {
            let hash_start = pos + found;
            let leaf = tree.0.leaf_by_position(hash_start as CodeIndex);
            if (leaf.start() as usize) <= hash_start && hash_start < leaf.end() as usize {
                // The '#' is part of a token (e.g. within a string literal) and is therefore
                // not a comment
                pos = hash_start + 1;
                continue;
            }
            // A '#' outside of tokens starts a comment that runs to the end of the line
            let comment_end = code[hash_start..]
                .find(['\n', '\r'])
                .map(|newline| hash_start + newline)
                .unwrap_or(code.len());
            scan_comment(
                &code[hash_start..comment_end],
                hash_start as CodeIndex,
                &mut entries,
            );
            pos = comment_end;
        }
        Self { entries }
    }

    pub fn entries(&self) -> &[IgnoreDirective] {
        &self.entries
    }

    /// Returns the merged ignore comment relevant for an issue with the given span, i.e. all
    /// ignore comments between `start` and the end of the line that contains `end`.
    pub fn type_ignore_comment_for<'code>(
        &self,
        code: &'code str,
        start: CodeIndex,
        end: CodeIndex,
    ) -> Option<TypeIgnoreComment<'code>> {
        // Returns Some(WithoutCode) when there is a type: ignore
        // Returns Some(WithCodes{codes: "foo", ..}) when there is a type: ignore[foo]
        let end_of_last_line = match code[end as usize..].find(['\n', '\r']) {
            Some(newline) => end + newline as CodeIndex,
            None => code.len() as CodeIndex,
        };
        self.fold_in_range(code, start, end_of_last_line)
    }

    /// Merges all directives whose comments start within `start..end`, with the same semantics
    /// the previous per-issue text scan had: multiple coded ignores accumulate their codes, while
    /// any bare ignore makes the result a bare ignore.
    pub(crate) fn fold_in_range<'code>(
        &self,
        code: &'code str,
        start: CodeIndex,
        end: CodeIndex,
    ) -> Option<TypeIgnoreComment<'code>> {
        let first_index = self
            .entries
            .partition_point(|entry| entry.hash_start < start);
        let mut result = None;
        for entry in &self.entries[first_index..] {
            if entry.hash_start >= end {
                break;
            }
            let new = entry.as_type_ignore_comment(code);
            if let Some(old) = &mut result {
                match (old, new) {
                    (
                        TypeIgnoreComment::WithCodes {
                            codes_of_later_type_ignores,
                            ..
                        },
                        TypeIgnoreComment::WithCodes {
                            codes: new_codes, ..
                        },
                    ) => codes_of_later_type_ignores.push(new_codes),
                    (old, _) => *old = TypeIgnoreComment::WithoutCode,
                }
            } else {
                result = Some(new);
            }
        }
        result
    }
}

impl IgnoreDirective {
    pub fn is_bare(&self) -> bool {
        self.codes_span.is_none()
    }

    pub fn codes<'code>(&self, code: &'code str) -> Option<&'code str> {
        self.codes_span
            .map(|(start, end)| &code[start as usize..end as usize])
    }

    fn as_type_ignore_comment<'code>(&self, code: &'code str) -> TypeIgnoreComment<'code> {
        match self.codes_span {
            Some((start, _)) => TypeIgnoreComment::WithCodes {
                codes: self.codes(code).unwrap(),
                kind: self.kind,
                codes_start_at_index: start,
                codes_of_later_type_ignores: vec![],
            },
            None => TypeIgnoreComment::WithoutCode,
        }
    }
}

/// Scans the text of a single comment (starting at its leading `#` and ending at the end of the
/// line) for ignore directives. Directives after unrelated comment text are also honored, so
/// that suppressions of multiple tools can be stacked, e.g. `# noqa # type: ignore` or
/// `# ty: ignore[x]  # zuban: ignore[y]`.
fn scan_comment(
    comment_text: &str,
    comment_hash_start: CodeIndex,
    entries: &mut Vec<IgnoreDirective>,
) {
    let mut iterator = comment_text.split('#');
    // The first part is the empty text before the comment's leading `#`
    iterator.next();
    let mut segment_start = comment_hash_start + 1;
    for segment in iterator {
        if let Some(directive) = maybe_ignore_directive_in_comment(segment, segment_start) {
            entries.push(directive);
        }
        segment_start += segment.len() as CodeIndex + 1;
    }
}

fn maybe_ignore_directive_in_comment(
    comment: &str,
    comment_start: CodeIndex,
) -> Option<IgnoreDirective> {
    let rest = comment.trim_start_matches(' ');
    let mut kind = "type";
    let ignore = rest.strip_prefix("type:").or_else(|| {
        kind = "zuban";
        rest.strip_prefix("zuban:")
    })?;
    let ignore = ignore.trim_start_matches(' ');
    let type_ignore = maybe_type_ignore(
        kind,
        comment_start + (comment.len() - ignore.len()) as CodeIndex,
        ignore,
    )?;
    Some(IgnoreDirective {
        hash_start: comment_start - 1,
        kind,
        codes_span: match type_ignore {
            TypeIgnoreComment::WithCodes {
                codes,
                codes_start_at_index,
                ..
            } => Some((
                codes_start_at_index,
                codes_start_at_index + codes.len() as CodeIndex,
            )),
            TypeIgnoreComment::WithoutCode => None,
        },
    })
}

pub fn maybe_type_ignore<'db>(
    kind: &'static str,
    start_at: CodeIndex,
    text: &'db str,
) -> Option<TypeIgnoreComment<'db>> {
    if let Some(after) = text.strip_prefix("ignore") {
        let trimmed = after.trim_start_matches(' ');
        let start_at = start_at + (text.len() - trimmed.len()) as CodeIndex;
        let trimmed = trimmed.trim_end_matches(' ');
        if let Some(trimmed) = trimmed.strip_prefix('[')
            && let Some(trimmed) = trimmed.strip_suffix(']')
            && !trimmed.is_empty()
        {
            return Some(TypeIgnoreComment::WithCodes {
                kind,
                codes: trimmed,
                codes_start_at_index: start_at + 1,
                codes_of_later_type_ignores: vec![],
            });
        }

        if after.is_empty() || after.starts_with([' ', '\t']) {
            return Some(TypeIgnoreComment::WithoutCode);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(code: &str) -> Vec<(CodeIndex, &'static str, Option<String>)> {
        let tree = Tree::parse(code.into());
        IgnoreDirectives::scan(&tree)
            .entries()
            .iter()
            .map(|entry| {
                (
                    entry.hash_start,
                    entry.kind,
                    entry.codes(tree.code()).map(str::to_string),
                )
            })
            .collect()
    }

    fn expect(
        entries: &[(CodeIndex, &'static str, Option<&str>)],
    ) -> Vec<(CodeIndex, &'static str, Option<String>)> {
        entries
            .iter()
            .map(|&(start, kind, codes)| (start, kind, codes.map(str::to_string)))
            .collect()
    }

    #[test]
    fn scan_bare_and_coded() {
        assert_eq!(
            spans("x = 1  # type: ignore\n"),
            expect(&[(7, "type", None)])
        );
        assert_eq!(
            spans("x = 1  # type: ignore[assignment]\n"),
            expect(&[(7, "type", Some("assignment"))])
        );
        // Multiple codes with weird spacing keep the raw bracket interior
        assert_eq!(
            spans("x = 1  # type: ignore   [ a , b ]\n"),
            expect(&[(7, "type", Some(" a , b "))])
        );
        // Tolerates missing trailing newline
        assert_eq!(spans("x = 1  # type: ignore"), expect(&[(7, "type", None)]));
    }

    #[test]
    fn scan_non_matches() {
        assert_eq!(spans("x = 1  # type: ignored\n"), []);
        assert_eq!(spans("x = 1  # type: ignore_foo\n"), []);
        assert_eq!(spans("x = 1  # type: ignore[]\n"), []);
        assert_eq!(spans("x = 1  # type: ignore[a] trailing\n"), []);
        assert_eq!(spans("x = 1  # types: ignore\n"), []);
        // `ignore` directly followed by a comment end or whitespace is fine though
        assert_eq!(
            spans("x = 1  # type: ignore more\n"),
            expect(&[(7, "type", None)])
        );
    }

    #[test]
    fn scan_only_finds_actual_comments() {
        // A '#' within a string is not a comment
        assert_eq!(spans("x = '# type: ignore '\n"), []);
        assert_eq!(spans("x = 'foo # type: ignore[a]'\n"), []);
        assert_eq!(spans("x = '''\n# type: ignore\n'''\n"), []);
        assert_eq!(spans("x = f'{1} # type: ignore '\n"), []);
        // An actual comment after a string containing a '#' is found
        assert_eq!(
            spans("url = 'http://x#y'  # type: ignore[a]\n"),
            expect(&[(20, "type", Some("a"))])
        );
        assert_eq!(
            spans("x = '# type: ignore '  # type: ignore[a]\n"),
            expect(&[(23, "type", Some("a"))])
        );
    }

    #[test]
    fn scan_kinds_and_multiple_comments_per_line() {
        assert_eq!(
            spans("x = 1  # zuban: ignore[foo]\n"),
            expect(&[(7, "zuban", Some("foo"))])
        );
        // Note that the conventional way to ignore multiple error codes is the comma syntax
        // (`# type: ignore[a, b]`, a single directive). Multiple ignore directives within a
        // comment are nevertheless scanned separately, and their codes accumulate on lookup.
        // A directive after unrelated comment text is honored as well
        assert_eq!(
            spans("x = 1  # a comment # type: ignore[a]\n"),
            expect(&[(19, "type", Some("a"))])
        );
        assert_eq!(
            spans("x = 1  # type: ignore[a] # zuban: ignore[b]\n"),
            expect(&[(7, "type", Some("a")), (25, "zuban", Some("b"))])
        );
        assert_eq!(
            spans("x = 1  # type: ignore[a] # type: ignore\n"),
            expect(&[(7, "type", Some("a")), (25, "type", None)])
        );
    }

    #[test]
    fn scan_offsets_across_lines() {
        let code = "x = 1\ny = 2  # type: ignore[a]\nz = 3\na = 4  # zuban: ignore\n";
        assert_eq!(
            spans(code),
            expect(&[(13, "type", Some("a")), (44, "zuban", None)])
        );
        // Windows line endings
        let code = "x = 1\r\ny = 2  # type: ignore[a]\r\n";
        assert_eq!(spans(code), expect(&[(14, "type", Some("a"))]));
    }

    #[test]
    fn lookup_same_line() {
        let tree = Tree::parse("x = 1  # type: ignore[a]\ny = 2\n".into());
        let code = tree.code();
        let directives = IgnoreDirectives::scan(&tree);
        let expected = || {
            Some(TypeIgnoreComment::WithCodes {
                codes: "a",
                kind: "type",
                codes_start_at_index: 22,
                codes_of_later_type_ignores: vec![],
            })
        };
        assert_eq!(directives.type_ignore_comment_for(code, 0, 5), expected());
        // Lookups never look at previous lines
        assert_eq!(directives.type_ignore_comment_for(code, 26, 31), None);
        // Comments before the start of the lookup are not considered
        assert_eq!(directives.type_ignore_comment_for(code, 8, 8), None);
    }

    #[test]
    fn lookup_over_multiple_lines() {
        let tree = Tree::parse(
            "foo(  # type: ignore[a]\n    1,  # type: ignore[b]\n    2,\n)  # type: ignore\n"
                .into(),
        );
        let code = tree.code();
        let directives = IgnoreDirectives::scan(&tree);
        // A lookup that only spans the first line
        assert_eq!(
            directives.type_ignore_comment_for(code, 0, 3),
            Some(TypeIgnoreComment::WithCodes {
                codes: "a",
                kind: "type",
                codes_start_at_index: 21,
                codes_of_later_type_ignores: vec![],
            })
        );
        // A lookup over the first two lines merges the coded ignores
        assert_eq!(
            directives.type_ignore_comment_for(code, 0, 28),
            Some(TypeIgnoreComment::WithCodes {
                codes: "a",
                kind: "type",
                codes_start_at_index: 21,
                codes_of_later_type_ignores: vec!["b"],
            })
        );
        // A lookup over all lines includes the bare ignore, which wins
        assert_eq!(
            directives.type_ignore_comment_for(code, 0, 59),
            Some(TypeIgnoreComment::WithoutCode)
        );
    }

    #[test]
    fn lookup_bare_ignore_wins_in_both_directions() {
        let tree = Tree::parse("foo(  # type: ignore\n    1,  # type: ignore[b]\n)\n".into());
        let code = tree.code();
        let directives = IgnoreDirectives::scan(&tree);
        assert_eq!(
            directives.type_ignore_comment_for(code, 0, 26),
            Some(TypeIgnoreComment::WithoutCode)
        );
    }
}
