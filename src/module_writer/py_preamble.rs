/// Find the byte position in a Python file where injected code should be inserted.
///
/// Returns the byte offset right after the module preamble, which includes:
/// 1. UTF-8 BOM (if present)
/// 2. Shebang line (if present)
/// 3. Encoding declaration (first or second line, per PEP 263)
/// 4. Module docstring (if present)
/// 5. All `from __future__` import statements
///
/// This ensures injected code (e.g., DLL loader patches) doesn't break:
/// - File encoding detection
/// - The module's `__doc__` attribute
/// - Python's requirement that `from __future__` imports precede other statements
///
/// This uses a line-oriented byte heuristic rather than parsing Python syntax.
pub(super) fn find_python_insertion_point(content: &[u8]) -> usize {
    let mut pos = 0;

    // Skip UTF-8 BOM if present
    if content.starts_with(b"\xef\xbb\xbf") {
        pos = 3;
    }

    // Helper to find end of current line
    let find_line_end = |start: usize| -> usize {
        content[start..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|i| start + i + 1)
            .unwrap_or(content.len())
    };

    // Helper to check if a line is a comment or blank
    let is_comment_or_blank = |line: &[u8]| -> bool {
        let trimmed = line
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .map(|i| &line[i..])
            .unwrap_or(&[]);
        trimmed.is_empty() || trimmed.starts_with(b"#")
    };

    // Helper to check for encoding declaration (PEP 263)
    // Must be a comment line containing "coding" followed by ":" or "="
    let is_encoding_line = |line: &[u8]| -> bool {
        let trimmed = line
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .map(|i| &line[i..])
            .unwrap_or(&[]);
        if !trimmed.starts_with(b"#") {
            return false;
        }
        let comment = &trimmed[1..];
        // Match PEP 263 pattern: "coding" followed by ":" or "="
        comment
            .windows(7)
            .any(|w| &w[..6] == b"coding" && (w[6] == b':' || w[6] == b'='))
    };

    // Skip shebang (must be first line)
    let pos_after_bom = pos;
    if pos < content.len() {
        let line_end = find_line_end(pos);
        let line = &content[pos..line_end];
        if line.starts_with(b"#!") {
            pos = line_end;
        }
    }

    // Skip encoding declaration if present in the first two physical lines (PEP 263).
    // If we consumed a shebang above, we only need to check the next line.
    // Otherwise check both the current and next line.
    let lines_to_check: usize = if pos > pos_after_bom { 1 } else { 2 };
    for _ in 0..lines_to_check {
        if pos >= content.len() {
            break;
        }
        let line_end = find_line_end(pos);
        let line = &content[pos..line_end];
        if is_encoding_line(line) {
            pos = line_end;
            break;
        }
    }

    // Track minimum insertion point (after BOM/shebang/encoding)
    let mut insertion_point = pos;

    // Skip blank lines and comments before docstring
    while pos < content.len() {
        let line_end = find_line_end(pos);
        let line = &content[pos..line_end];
        if is_comment_or_blank(line) {
            pos = line_end;
            insertion_point = pos;
        } else {
            break;
        }
    }

    // Check for module docstring (triple-quoted string as first statement)
    if pos < content.len() {
        let trimmed_start = content[pos..]
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .map(|i| pos + i)
            .unwrap_or(pos);

        let rest = &content[trimmed_start..];
        let quote = if rest.starts_with(b"\"\"\"") {
            Some(b"\"\"\"".as_slice())
        } else if rest.starts_with(b"'''") {
            Some(b"'''".as_slice())
        } else if rest.starts_with(b"r\"\"\"")
            || rest.starts_with(b"R\"\"\"")
            || rest.starts_with(b"u\"\"\"")
            || rest.starts_with(b"U\"\"\"")
        {
            Some(b"\"\"\"".as_slice())
        } else if rest.starts_with(b"r'''")
            || rest.starts_with(b"R'''")
            || rest.starts_with(b"u'''")
            || rest.starts_with(b"U'''")
        {
            Some(b"'''".as_slice())
        } else {
            None
        };

        if let Some(q) = quote {
            // Find the closing triple quote
            let start_offset = if rest.starts_with(b"r")
                || rest.starts_with(b"R")
                || rest.starts_with(b"u")
                || rest.starts_with(b"U")
            {
                4
            } else {
                3
            };
            if let Some(end_idx) = rest[start_offset..]
                .windows(3)
                .position(|w| w == q)
                .map(|i| trimmed_start + start_offset + i + 3)
            {
                // Move past the docstring and any trailing newline
                pos = end_idx;
                if pos < content.len() && content[pos] == b'\n' {
                    pos += 1;
                }
                insertion_point = pos;
            }
        }
    }

    // Now scan for `from __future__` imports
    let mut in_future_import = false;
    let mut paren_depth: usize = 0;

    while pos < content.len() {
        let line_end = find_line_end(pos);
        let line = &content[pos..line_end];

        if in_future_import {
            // Count parentheses in this continuation line
            for &b in line {
                match b {
                    b'(' => paren_depth += 1,
                    b')' => paren_depth = paren_depth.saturating_sub(1),
                    _ => {}
                }
            }
            let ends_with_backslash = !line.is_empty()
                && line
                    .iter()
                    .rposition(|b| !b.is_ascii_whitespace())
                    .map(|i| line[i] == b'\\')
                    .unwrap_or(false);
            if paren_depth == 0 && !ends_with_backslash {
                in_future_import = false;
                insertion_point = line_end;
            }
        } else {
            let trimmed_start = line
                .iter()
                .position(|b| !b.is_ascii_whitespace())
                .unwrap_or(0);

            if line[trimmed_start..].starts_with(b"from __future__") {
                // Check if this is a multi-line import
                paren_depth = 0;
                for &b in line {
                    match b {
                        b'(' => paren_depth += 1,
                        b')' => paren_depth = paren_depth.saturating_sub(1),
                        _ => {}
                    }
                }
                let ends_with_backslash = !line.is_empty()
                    && line
                        .iter()
                        .rposition(|b| !b.is_ascii_whitespace())
                        .map(|i| line[i] == b'\\')
                        .unwrap_or(false);
                if paren_depth > 0 || ends_with_backslash {
                    in_future_import = true;
                } else {
                    insertion_point = line_end;
                }
            } else if !is_comment_or_blank(line) {
                // Non-future import statement found, stop scanning
                break;
            }
        }

        pos = line_end;
    }

    insertion_point
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_find_python_insertion_point() {
        use super::find_python_insertion_point;

        // No __future__ imports, no preamble
        assert_eq!(find_python_insertion_point(b"import os\n"), 0);

        // With __future__ import
        let content = b"from __future__ import annotations\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 35); // after the newline

        // Multiple __future__ imports
        let content =
            b"from __future__ import annotations\nfrom __future__ import division\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 67);

        // Docstring then __future__ - insertion after __future__, not after docstring
        let content = b"\"\"\"Docstring.\"\"\"\nfrom __future__ import annotations\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 52);

        // Docstring only (no __future__) - insertion after docstring
        let content = b"\"\"\"Module docstring.\"\"\"\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 24);

        // Empty content
        assert_eq!(find_python_insertion_point(b""), 0);

        // Multi-line parenthesized __future__ import
        let content = b"from __future__ import (\n    annotations,\n)\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 44); // after closing paren line

        // Backslash-continued __future__ import
        let content = b"from __future__ import annotations, \\\n    division\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 51); // after continuation line

        // UTF-8 BOM should be skipped
        let content = b"\xef\xbb\xbfimport os\n";
        assert_eq!(find_python_insertion_point(content), 3); // after BOM

        // UTF-8 BOM with __future__
        let content = b"\xef\xbb\xbffrom __future__ import annotations\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 38); // BOM(3) + future line(35)

        // Shebang line
        let content = b"#!/usr/bin/env python\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 22); // after shebang

        // Shebang + encoding
        let content = b"#!/usr/bin/env python\n# -*- coding: utf-8 -*-\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 46); // after encoding line

        // Shebang + encoding + __future__
        let content =
            b"#!/usr/bin/env python\n# -*- coding: utf-8 -*-\nfrom __future__ import annotations\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 81);

        // Encoding only (no shebang)
        let content = b"# coding: utf-8\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 16);

        // Raw docstring
        let content = b"r\"\"\"Raw docstring.\"\"\"\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 22);

        // Single-quoted docstring
        let content = b"'''Single quoted docstring.'''\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 31);

        // Multi-line docstring
        let content = b"\"\"\"Multi-line\ndocstring.\"\"\"\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 28);
    }
}
