//! Source manager for `rccx`.
//!
//! Owns the bytes of every file the compiler reads, and translates between
//! byte offsets, `(line, column)` pairs, and source slices.
//!
//! Spans are deliberately small (`u32` offsets) so they can be embedded in
//! every AST/HIR/MIR node without bloating those structures.

use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Stable identifier for a source file inside a `SourceMap`.
///
/// Wraps a `u32` so it's cheap to copy and embed in spans.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(pub u32);

impl FileId {
    pub const DUMMY: FileId = FileId(u32::MAX);

    pub fn is_dummy(self) -> bool {
        self == Self::DUMMY
    }
}

/// Half-open byte range `[lo, hi)` inside a `FileId`'s text.
///
/// `Span::DUMMY` is a sentinel for "no useful location" and renders as `?`
/// in diagnostics. Constructors enforce `lo <= hi`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub file: FileId,
    pub lo: u32,
    pub hi: u32,
}

impl Span {
    pub const DUMMY: Span = Span {
        file: FileId::DUMMY,
        lo: 0,
        hi: 0,
    };

    pub fn new(file: FileId, lo: u32, hi: u32) -> Self {
        debug_assert!(lo <= hi, "Span::new requires lo <= hi");
        Self { file, lo, hi }
    }

    pub fn is_dummy(self) -> bool {
        self.file.is_dummy()
    }

    pub fn len(self) -> u32 {
        self.hi - self.lo
    }

    pub fn is_empty(self) -> bool {
        self.lo == self.hi
    }

    /// Smallest span covering both endpoints. Both spans must share a file.
    pub fn join(self, other: Span) -> Span {
        debug_assert_eq!(self.file, other.file, "Span::join across files");
        Span {
            file: self.file,
            lo: self.lo.min(other.lo),
            hi: self.hi.max(other.hi),
        }
    }
}

/// 1-based line/column for diagnostics. Columns count Unicode scalar values
/// inside a line, not bytes, so multi-byte UTF-8 sequences don't push the
/// caret off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCol {
    pub line: u32,
    pub column: u32,
}

/// Resolved location of a `Span` against a specific `SourceFile`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub file: FileId,
    pub path: PathBuf,
    pub start: LineCol,
    pub end: LineCol,
}

/// A single source file plus a precomputed table of line-start offsets.
#[derive(Debug)]
pub struct SourceFile {
    id: FileId,
    path: PathBuf,
    text: Arc<str>,
    /// Byte offset of the start of each line. `line_starts[0] == 0`. The
    /// number of lines is `line_starts.len()`.
    line_starts: Vec<u32>,
}

impl SourceFile {
    fn new(id: FileId, path: PathBuf, text: Arc<str>) -> Self {
        let line_starts = compute_line_starts(&text);
        Self { id, path, text, line_starts }
    }

    pub fn id(&self) -> FileId {
        self.id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn line_count(&self) -> u32 {
        self.line_starts.len() as u32
    }

    /// 1-based line/column for a byte offset. Offsets equal to the file
    /// length resolve to the position just past the last byte.
    pub fn line_col(&self, offset: u32) -> LineCol {
        let off = offset.min(self.text.len() as u32);
        let line_idx = match self.line_starts.binary_search(&off) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let line_start = self.line_starts[line_idx];
        // Column counts Unicode scalar values, not bytes.
        let column = self.text[line_start as usize..off as usize].chars().count() as u32;
        LineCol {
            line: line_idx as u32 + 1,
            column: column + 1,
        }
    }

    /// Byte range of the given 1-based line, excluding the trailing newline.
    pub fn line_range(&self, line: u32) -> Option<(u32, u32)> {
        if line == 0 || line as usize > self.line_starts.len() {
            return None;
        }
        let idx = (line - 1) as usize;
        let start = self.line_starts[idx];
        let end = if idx + 1 < self.line_starts.len() {
            // Strip the newline character.
            let next = self.line_starts[idx + 1];
            let bytes = self.text.as_bytes();
            let mut e = next;
            while e > start && (bytes[(e - 1) as usize] == b'\n' || bytes[(e - 1) as usize] == b'\r') {
                e -= 1;
            }
            e
        } else {
            self.text.len() as u32
        };
        Some((start, end))
    }

    /// Source text of the given 1-based line, with no trailing newline.
    pub fn line_text(&self, line: u32) -> Option<&str> {
        let (s, e) = self.line_range(line)?;
        Some(&self.text[s as usize..e as usize])
    }

    /// Source slice covered by a span. Returns `None` if the span isn't in
    /// this file or its range is out of bounds.
    pub fn slice(&self, span: Span) -> Option<&str> {
        if span.file != self.id {
            return None;
        }
        let lo = span.lo as usize;
        let hi = span.hi as usize;
        if hi > self.text.len() || lo > hi {
            return None;
        }
        Some(&self.text[lo..hi])
    }
}

fn compute_line_starts(text: &str) -> Vec<u32> {
    let mut starts = Vec::with_capacity(64);
    starts.push(0);
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                starts.push((i + 1) as u32);
                i += 1;
            }
            b'\r' => {
                let next = (i + 1) as u32;
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    starts.push(next + 1);
                    i += 2;
                } else {
                    starts.push(next);
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    starts
}

/// Owner of every `SourceFile` known to the compiler. Adding a file returns
/// a stable `FileId`; the map never invalidates earlier ids.
#[derive(Debug, Default)]
pub struct SourceMap {
    files: Vec<Arc<SourceFile>>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_file(&mut self, path: impl Into<PathBuf>, text: impl Into<String>) -> FileId {
        let id = FileId(self.files.len() as u32);
        let text: Arc<str> = Arc::from(text.into());
        let file = Arc::new(SourceFile::new(id, path.into(), text));
        self.files.push(file);
        id
    }

    pub fn file(&self, id: FileId) -> Option<&SourceFile> {
        if id.is_dummy() {
            return None;
        }
        self.files.get(id.0 as usize).map(|f| f.as_ref())
    }

    pub fn files(&self) -> impl Iterator<Item = &SourceFile> {
        self.files.iter().map(|f| f.as_ref())
    }

    /// Resolve a span to a full `Location`, or `None` if the span is dummy
    /// or points to an unknown file.
    pub fn location(&self, span: Span) -> Option<Location> {
        let file = self.file(span.file)?;
        Some(Location {
            file: file.id,
            path: file.path.to_path_buf(),
            start: file.line_col(span.lo),
            end: file.line_col(span.hi),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_has_one_line_start() {
        let mut sm = SourceMap::new();
        let id = sm.add_file("empty.c", "");
        let f = sm.file(id).unwrap();
        assert_eq!(f.line_count(), 1);
        assert_eq!(f.line_col(0), LineCol { line: 1, column: 1 });
        assert_eq!(f.line_text(1), Some(""));
    }

    #[test]
    fn single_line_no_newline() {
        let mut sm = SourceMap::new();
        let id = sm.add_file("a.c", "hello");
        let f = sm.file(id).unwrap();
        assert_eq!(f.line_count(), 1);
        assert_eq!(f.line_col(0), LineCol { line: 1, column: 1 });
        assert_eq!(f.line_col(5), LineCol { line: 1, column: 6 });
        assert_eq!(f.line_text(1), Some("hello"));
    }

    #[test]
    fn multi_line_lf() {
        let mut sm = SourceMap::new();
        let id = sm.add_file("a.c", "ab\ncd\nef");
        let f = sm.file(id).unwrap();
        assert_eq!(f.line_count(), 3);
        assert_eq!(f.line_col(0), LineCol { line: 1, column: 1 });
        assert_eq!(f.line_col(3), LineCol { line: 2, column: 1 });
        assert_eq!(f.line_col(6), LineCol { line: 3, column: 1 });
        assert_eq!(f.line_text(2), Some("cd"));
    }

    #[test]
    fn multi_line_crlf() {
        let mut sm = SourceMap::new();
        let id = sm.add_file("a.c", "ab\r\ncd\r\nef");
        let f = sm.file(id).unwrap();
        assert_eq!(f.line_count(), 3);
        assert_eq!(f.line_text(1), Some("ab"));
        assert_eq!(f.line_text(2), Some("cd"));
        assert_eq!(f.line_text(3), Some("ef"));
    }

    #[test]
    fn trailing_newline() {
        let mut sm = SourceMap::new();
        let id = sm.add_file("a.c", "ab\n");
        let f = sm.file(id).unwrap();
        // "ab\n" produces two line starts: 0 and 3 (past the newline).
        assert_eq!(f.line_count(), 2);
        assert_eq!(f.line_text(1), Some("ab"));
        assert_eq!(f.line_text(2), Some(""));
    }

    #[test]
    fn span_slice_and_location() {
        let mut sm = SourceMap::new();
        let id = sm.add_file("a.c", "int x = 1;\nreturn x;");
        let span = Span::new(id, 4, 5);
        let f = sm.file(id).unwrap();
        assert_eq!(f.slice(span), Some("x"));
        let loc = sm.location(span).unwrap();
        assert_eq!(loc.start, LineCol { line: 1, column: 5 });
        assert_eq!(loc.end, LineCol { line: 1, column: 6 });
    }

    #[test]
    fn dummy_span_resolves_to_none() {
        let sm = SourceMap::new();
        assert!(sm.location(Span::DUMMY).is_none());
    }

    #[test]
    fn unicode_column_counts_chars_not_bytes() {
        let mut sm = SourceMap::new();
        // 'あ' is three bytes in UTF-8. The column after it must be 2, not 4.
        let id = sm.add_file("u.c", "あx");
        let f = sm.file(id).unwrap();
        assert_eq!(f.line_col(3), LineCol { line: 1, column: 2 });
        assert_eq!(f.line_col(4), LineCol { line: 1, column: 3 });
    }

    #[test]
    fn span_join_extends_endpoints() {
        let mut sm = SourceMap::new();
        let id = sm.add_file("a.c", "abcdef");
        let a = Span::new(id, 1, 3);
        let b = Span::new(id, 2, 5);
        assert_eq!(a.join(b), Span::new(id, 1, 5));
    }
}
