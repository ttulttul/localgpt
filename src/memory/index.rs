use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::debug;

use super::search::MemoryChunk;

#[derive(Clone)]
pub struct MemoryIndex {
    conn: Arc<Mutex<Connection>>,
    workspace: PathBuf,
}

#[derive(Debug)]
pub struct ReindexStats {
    pub files_processed: usize,
    pub files_updated: usize,
    pub chunks_indexed: usize,
    pub duration: Duration,
}

impl MemoryIndex {
    pub fn new(workspace: &Path) -> Result<Self> {
        let db_path = workspace.join("memory.sqlite");
        let conn = Connection::open(&db_path)?;

        // Initialize schema
        conn.execute_batch(
            r#"
            -- File tracking
            CREATE TABLE IF NOT EXISTS files (
                path TEXT PRIMARY KEY,
                hash TEXT NOT NULL,
                mtime INTEGER NOT NULL,
                size INTEGER NOT NULL
            );

            -- Chunked content
            CREATE TABLE IF NOT EXISTS chunks (
                id INTEGER PRIMARY KEY,
                file_path TEXT NOT NULL,
                line_start INTEGER NOT NULL,
                line_end INTEGER NOT NULL,
                content TEXT NOT NULL,
                FOREIGN KEY (file_path) REFERENCES files(path) ON DELETE CASCADE
            );

            -- Full-text search index
            CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
                content,
                content='chunks',
                content_rowid='id'
            );

            -- Triggers to keep FTS in sync
            CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
                INSERT INTO chunks_fts(rowid, content) VALUES (new.id, new.content);
            END;

            CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
                INSERT INTO chunks_fts(chunks_fts, rowid, content) VALUES('delete', old.id, old.content);
            END;

            CREATE TRIGGER IF NOT EXISTS chunks_au AFTER UPDATE ON chunks BEGIN
                INSERT INTO chunks_fts(chunks_fts, rowid, content) VALUES('delete', old.id, old.content);
                INSERT INTO chunks_fts(rowid, content) VALUES (new.id, new.content);
            END;
            "#,
        )?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            workspace: workspace.to_path_buf(),
        })
    }

    /// Index a file, returning true if it was updated
    pub fn index_file(&self, path: &Path, force: bool) -> Result<bool> {
        let content = fs::read_to_string(path)?;
        let hash = hash_content(&content);
        let metadata = fs::metadata(path)?;
        let mtime = metadata
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;
        let size = metadata.len() as i64;

        let relative_path = path
            .strip_prefix(&self.workspace)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow!("Lock poisoned: {}", e))?;

        // Check if file has changed
        if !force {
            let existing: Option<String> = conn
                .query_row(
                    "SELECT hash FROM files WHERE path = ?1",
                    params![&relative_path],
                    |row| row.get(0),
                )
                .ok();

            if existing.as_deref() == Some(&hash) {
                debug!("File unchanged, skipping: {}", relative_path);
                return Ok(false);
            }
        }

        debug!("Indexing file: {}", relative_path);

        // Update file record
        conn.execute(
            "INSERT OR REPLACE INTO files (path, hash, mtime, size) VALUES (?1, ?2, ?3, ?4)",
            params![&relative_path, &hash, mtime, size],
        )?;

        // Delete existing chunks
        conn.execute(
            "DELETE FROM chunks WHERE file_path = ?1",
            params![&relative_path],
        )?;

        // Create new chunks
        let chunks = chunk_text(&content, 400, 80);

        for (_i, chunk) in chunks.iter().enumerate() {
            conn.execute(
                "INSERT INTO chunks (file_path, line_start, line_end, content) VALUES (?1, ?2, ?3, ?4)",
                params![&relative_path, chunk.line_start, chunk.line_end, &chunk.content],
            )?;
        }

        Ok(true)
    }

    /// Search using FTS5
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryChunk>> {
        // Escape special FTS5 characters
        let escaped_query = escape_fts_query(query);

        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow!("Lock poisoned: {}", e))?;

        let mut stmt = conn.prepare(
            r#"
            SELECT c.file_path, c.line_start, c.line_end, c.content, bm25(chunks_fts) as score
            FROM chunks_fts fts
            JOIN chunks c ON fts.rowid = c.id
            WHERE chunks_fts MATCH ?1
            ORDER BY score
            LIMIT ?2
            "#,
        )?;

        let rows = stmt.query_map(params![&escaped_query, limit as i64], |row| {
            Ok(MemoryChunk {
                file: row.get(0)?,
                line_start: row.get(1)?,
                line_end: row.get(2)?,
                content: row.get(3)?,
                score: row.get::<_, f64>(4)?.abs(), // BM25 returns negative scores
            })
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }

        Ok(results)
    }

    /// Get total chunk count
    pub fn chunk_count(&self) -> Result<usize> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow!("Lock poisoned: {}", e))?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    /// Get chunk count for a specific file
    pub fn file_chunk_count(&self, path: &Path) -> Result<usize> {
        let relative_path = path
            .strip_prefix(&self.workspace)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow!("Lock poisoned: {}", e))?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM chunks WHERE file_path = ?1",
            params![&relative_path],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// Get database size in bytes
    pub fn size_bytes(&self) -> Result<u64> {
        let db_path = self.workspace.join("memory.sqlite");
        if db_path.exists() {
            Ok(fs::metadata(&db_path)?.len())
        } else {
            Ok(0)
        }
    }
}

fn hash_content(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn escape_fts_query(query: &str) -> String {
    // Wrap in quotes to treat as phrase, escape internal quotes
    let escaped = query.replace('"', "\"\"");
    format!("\"{}\"", escaped)
}

struct ChunkInfo {
    line_start: i32,
    line_end: i32,
    content: String,
}

fn chunk_text(text: &str, target_tokens: usize, overlap_tokens: usize) -> Vec<ChunkInfo> {
    let lines: Vec<&str> = text.lines().collect();
    let mut chunks = Vec::new();

    if lines.is_empty() {
        return chunks;
    }

    // Rough estimate: 4 chars per token
    let target_chars = target_tokens * 4;
    let overlap_chars = overlap_tokens * 4;

    let mut start_line = 0;
    let mut current_chars = 0;
    let mut chunk_lines = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        chunk_lines.push(*line);
        current_chars += line.len() + 1; // +1 for newline

        if current_chars >= target_chars || i == lines.len() - 1 {
            // Create chunk
            chunks.push(ChunkInfo {
                line_start: (start_line + 1) as i32,
                line_end: (i + 1) as i32,
                content: chunk_lines.join("\n"),
            });

            // Calculate overlap for next chunk
            let mut overlap_len = 0;
            let mut overlap_start = chunk_lines.len();

            for (j, line) in chunk_lines.iter().enumerate().rev() {
                overlap_len += line.len() + 1;
                if overlap_len >= overlap_chars {
                    overlap_start = j;
                    break;
                }
            }

            // Prepare for next chunk
            if overlap_start < chunk_lines.len() {
                start_line = start_line + overlap_start;
                chunk_lines = chunk_lines[overlap_start..].to_vec();
                current_chars = chunk_lines.iter().map(|l| l.len() + 1).sum();
            } else {
                start_line = i + 1;
                chunk_lines.clear();
                current_chars = 0;
            }
        }
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_chunk_text() {
        let text = "Line 1\nLine 2\nLine 3\nLine 4\nLine 5";
        let chunks = chunk_text(text, 10, 2); // Small chunks for testing

        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].line_start, 1);
    }

    #[test]
    fn test_memory_index() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let workspace = temp_dir.path();

        // Create a test file
        let test_file = workspace.join("test.md");
        fs::write(
            &test_file,
            "# Test\n\nThis is a test document.\n\nWith multiple lines.",
        )?;

        let index = MemoryIndex::new(workspace)?;
        index.index_file(&test_file, false)?;

        assert!(index.chunk_count()? > 0);

        let results = index.search("test document", 10)?;
        assert!(!results.is_empty());

        Ok(())
    }
}
