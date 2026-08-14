//! 语义分块器。

use crate::corpus::SourceFile;

/// 语义块。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Chunk {
    pub crate_name: String,
    pub file_path: String,
    pub line_start: u32,
    pub line_end: u32,
    pub symbol_type: SymbolType,
    pub text: String,
}

/// 符号类型。
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SymbolType {
    Function,
    Struct,
    Trait,
    Mod,
    Impl,
    Enum,
    Const,
    Static,
    Other,
}

/// 语义分块器：按 Rust item 边界分块，超长块按行边界二次切分。
pub struct SemanticChunker {
    max_chars: usize,
}

impl SemanticChunker {
    pub fn new(max_chars: usize) -> Self {
        Self {
            max_chars: max_chars.max(200),
        }
    }

    /// 按语义单元分块，超长块按行边界二次切分。
    pub fn chunk(&self, file: &SourceFile) -> Vec<Chunk> {
        let lines: Vec<&str> = file.content.lines().collect();
        let mut chunks = Vec::new();
        let mut current_start = 0u32;
        let mut current_text = String::new();
        let mut current_symbol = SymbolType::Other;

        for (idx, line) in lines.iter().enumerate() {
            let line_no = (idx + 1) as u32;
            let trimmed = line.trim_start();

            if let Some(sym) = detect_symbol(trimmed) {
                if !current_text.is_empty() {
                    self.flush_chunk(
                        file,
                        current_start,
                        line_no - 1,
                        current_symbol,
                        &mut current_text,
                        &mut chunks,
                    );
                }
                current_start = line_no;
                current_symbol = sym;
            }

            current_text.push_str(line);
            current_text.push('\n');

            if current_text.len() >= self.max_chars {
                self.flush_chunk(
                    file,
                    current_start,
                    line_no,
                    current_symbol,
                    &mut current_text,
                    &mut chunks,
                );
                current_start = line_no + 1;
                current_symbol = SymbolType::Other;
            }
        }

        if !current_text.is_empty() {
            self.flush_chunk(
                file,
                current_start,
                lines.len() as u32,
                current_symbol,
                &mut current_text,
                &mut chunks,
            );
        }

        chunks
    }

    fn flush_chunk(
        &self,
        file: &SourceFile,
        start: u32,
        end: u32,
        symbol: SymbolType,
        text: &mut String,
        chunks: &mut Vec<Chunk>,
    ) {
        let chunk_text = std::mem::take(text);
        let trimmed = chunk_text.trim();
        if trimmed.is_empty() {
            return;
        }
        chunks.push(Chunk {
            crate_name: file.crate_name.clone(),
            file_path: file.path.to_string_lossy().to_string(),
            line_start: start,
            line_end: end,
            symbol_type: symbol,
            text: chunk_text,
        });
    }
}

fn detect_symbol(line: &str) -> Option<SymbolType> {
    if line.starts_with("fn ")
        || line.starts_with("pub fn ")
        || line.starts_with("async fn ")
        || line.starts_with("pub async fn ")
        || line.starts_with("pub(crate) fn ")
    {
        Some(SymbolType::Function)
    } else if line.starts_with("struct ")
        || line.starts_with("pub struct ")
        || line.starts_with("pub(crate) struct ")
    {
        Some(SymbolType::Struct)
    } else if line.starts_with("trait ") || line.starts_with("pub trait ") {
        Some(SymbolType::Trait)
    } else if line.starts_with("mod ") || line.starts_with("pub mod ") {
        Some(SymbolType::Mod)
    } else if line.starts_with("impl ") || line.starts_with("impl<") {
        Some(SymbolType::Impl)
    } else if line.starts_with("enum ") || line.starts_with("pub enum ") {
        Some(SymbolType::Enum)
    } else if line.starts_with("const ") || line.starts_with("pub const ") {
        Some(SymbolType::Const)
    } else if line.starts_with("static ") || line.starts_with("pub static ") {
        Some(SymbolType::Static)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_file(content: &str) -> SourceFile {
        SourceFile {
            crate_name: "test".into(),
            path: PathBuf::from("test.rs"),
            content: content.into(),
        }
    }

    #[test]
    fn chunk_empty() {
        let chunker = SemanticChunker::new(800);
        let file = make_file("");
        assert!(chunker.chunk(&file).is_empty());
    }

    #[test]
    fn chunk_single_function() {
        let chunker = SemanticChunker::new(800);
        let file = make_file("fn foo() {\n    println!(\"hello\");\n}\n");
        let chunks = chunker.chunk(&file);
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].symbol_type, SymbolType::Function);
    }

    #[test]
    fn chunk_multiple_symbols() {
        let chunker = SemanticChunker::new(800);
        let file = make_file("struct A { x: i32 }\nfn b() {}\ntrait C { fn c(&self); }\n");
        let chunks = chunker.chunk(&file);
        assert!(chunks.len() >= 2);
    }

    #[test]
    fn chunk_oversize_split() {
        let chunker = SemanticChunker::new(100);
        let long_line = format!("fn big() {{\n    {}\n}}\n", "x".repeat(200));
        let file = make_file(&long_line);
        let chunks = chunker.chunk(&file);
        assert!(chunks.len() >= 2);
    }
}
