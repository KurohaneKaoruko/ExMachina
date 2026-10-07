//! 纯 Rust 嵌入式文档存储（FsDb）—— 零 C 依赖、零外部服务
//!
//! 为什么不用 SQLite：本构建环境无 MSVC / mingw（无任何 C 编译器），
//! `rusqlite --features bundled` 无法编译。存储层本就位于 repository 接口之后，
//! 因此以纯 Rust 实现替代，语义等价、可随时换回 SQLite/redb（见 docs/架构与设计.md）。
//!
//! 设计：
//!   - 单条文档：`{root}/{collection}/{id}.json`，原子写（临时文件 + rename）
//!   - 追加日志：`{root}/{collection}/{key}.jsonl`（事件、消息、审计），读时解析
//!   - 分桶索引：`{root}/{collection}/{bucket}.json`（倒排词项等大映射），按哈希分桶，
//!     写入只重写受影响的分桶 → 避免整表重写

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const TERM_BUCKETS: usize = 64;

pub struct FsDb {
    root: PathBuf,
    /// 保护分桶文件的读改写（单进程内串行化即可）
    lock: Mutex<()>,
}

impl FsDb {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        Ok(FsDb { root, lock: Mutex::new(()) })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn coll_dir(&self, collection: &str) -> Result<PathBuf> {
        let dir = self.root.join(collection);
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    fn doc_path(&self, collection: &str, id: &str) -> Result<PathBuf> {
        Ok(self.coll_dir(collection)?.join(format!("{}.json", sanitize(id))))
    }

    // ---------------- 文档读写 ----------------

    pub fn put<T: Serialize>(&self, collection: &str, id: &str, value: &T) -> Result<()> {
        let path = self.doc_path(collection, id)?;
        let data = serde_json::to_vec_pretty(value)?;
        atomic_write(&path, &data)
            .with_context(|| format!("写入失败: {}", path.display()))
    }

    pub fn get<T: DeserializeOwned>(&self, collection: &str, id: &str) -> Result<Option<T>> {
        let path = self.doc_path(collection, id)?;
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes).with_context(|| {
                format!("解析失败: {}", path.display())
            })?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn delete(&self, collection: &str, id: &str) -> Result<()> {
        let path = self.doc_path(collection, id)?;
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    pub fn exists(&self, collection: &str, id: &str) -> Result<bool> {
        Ok(self.doc_path(collection, id)?.exists())
    }

    /// 列出集合内全部文档（按 id 排序，保证确定性）
    pub fn list<T: DeserializeOwned>(&self, collection: &str) -> Result<Vec<T>> {
        let dir = self.coll_dir(collection)?;
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
            .collect();
        files.sort();
        let mut out = Vec::with_capacity(files.len());
        for f in files {
            let bytes = std::fs::read(&f)?;
            match serde_json::from_slice::<T>(&bytes) {
                Ok(v) => out.push(v),
                Err(e) => eprintln!("[fsdb] 跳过损坏文档 {}: {e}", f.display()),
            }
        }
        Ok(out)
    }

    pub fn count(&self, collection: &str) -> Result<usize> {
        let dir = self.coll_dir(collection)?;
        Ok(std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
            .count())
    }

    // ---------------- 追加日志（JSONL） ----------------

    pub fn append_line<T: Serialize>(&self, collection: &str, key: &str, value: &T) -> Result<()> {
        use std::io::Write;
        let path = self.coll_dir(collection)?.join(format!("{}.jsonl", sanitize(key)));
        let mut line = serde_json::to_vec(value)?;
        line.push(b'\n');
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        f.write_all(&line)?;
        Ok(())
    }

    /// 读取追加日志；`tail` 为 0 表示全部
    pub fn read_lines<T: DeserializeOwned>(
        &self,
        collection: &str,
        key: &str,
        tail: usize,
    ) -> Result<Vec<T>> {
        let path = self.coll_dir(collection)?.join(format!("{}.jsonl", sanitize(key)));
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(e.into()),
        };
        let mut items: Vec<T> = Vec::new();
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<T>(line) {
                Ok(v) => items.push(v),
                Err(_) => continue, // 容忍尾部半行（崩溃残留）
            }
        }
        if tail > 0 && items.len() > tail {
            items = items.split_off(items.len() - tail);
        }
        Ok(items)
    }

    /// 重写追加日志（用于容量裁剪）
    pub fn rewrite_lines<T: Serialize>(
        &self,
        collection: &str,
        key: &str,
        items: &[T],
    ) -> Result<()> {
        let path = self.coll_dir(collection)?.join(format!("{}.jsonl", sanitize(key)));
        let mut buf = Vec::new();
        for it in items {
            buf.extend_from_slice(&serde_json::to_vec(it)?);
            buf.push(b'\n');
        }
        atomic_write(&path, &buf)
    }

    // ---------------- 分桶大映射（倒排索引等） ----------------

    pub fn bucket_id(term: &str) -> usize {
        let mut h: u64 = 1469598103934665603;
        for b in term.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(1099511628211);
        }
        (h as usize) % TERM_BUCKETS
    }

    /// 读-改-写一个分桶映射（回调内完成修改）。
    /// `key` 同时用于定位分桶与作为映射键传给回调，避免借用冲突。
    pub fn with_bucket_map<V, F>(&self, collection: &str, key: &str, mut f: F) -> Result<()>
    where
        V: DeserializeOwned + Serialize + Clone,
        F: FnMut(&str, &mut std::collections::HashMap<String, V>),
    {
        let _guard = self.lock.lock().unwrap();
        let dir = self.coll_dir(collection)?;
        let path = dir.join(format!("bucket-{:02}.json", Self::bucket_id(key)));
        let mut map: std::collections::HashMap<String, V> = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => std::collections::HashMap::new(),
        };
        f(key, &mut map);
        atomic_write(&path, &serde_json::to_vec(&map)?)
    }

    /// 读取包含指定键的分桶映射
    pub fn read_bucket_map<V: DeserializeOwned>(
        &self,
        collection: &str,
        term: &str,
    ) -> Result<std::collections::HashMap<String, V>> {
        let dir = self.coll_dir(collection)?;
        let path = dir.join(format!("bucket-{:02}.json", Self::bucket_id(term)));
        match std::fs::read(&path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes).unwrap_or_default()),
            Err(_) => Ok(std::collections::HashMap::new()),
        }
    }

    /// 清空分桶目录（索引重建用）
    pub fn clear_buckets(&self, collection: &str) -> Result<()> {
        let dir = self.coll_dir(collection)?;
        for e in std::fs::read_dir(&dir)?.filter_map(|e| e.ok()) {
            if e.path().file_name().map(|n| n.to_string_lossy().starts_with("bucket-")).unwrap_or(false) {
                let _ = std::fs::remove_file(e.path());
            }
        }
        Ok(())
    }

    /// 分桶映射中的键总数（倒排词项数统计）
    pub fn bucket_key_count(&self, collection: &str) -> Result<usize> {
        let dir = self.coll_dir(collection)?;
        let mut total = 0;
        for e in std::fs::read_dir(&dir)?.filter_map(|e| e.ok()) {
            let name = e.path().file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if !name.starts_with("bucket-") {
                continue;
            }
            if let Ok(bytes) = std::fs::read(e.path()) {
                let map: std::collections::HashMap<String, serde_json::Value> =
                    serde_json::from_slice(&bytes).unwrap_or_default();
                total += map.len();
            }
        }
        Ok(total)
    }
}

/// 原子写：同目录临时文件 + rename（Windows 上 rename 到已存在目标需先删除）
fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// 文件名安全化：路径分隔符与保留字符替换
fn sanitize(id: &str) -> String {
    id.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\n' | '\r' => '_',
            c => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 文档读写与追加日志() {
        let dir = std::env::temp_dir().join(format!("fsdb-{}", uuid::Uuid::new_v4()));
        let db = FsDb::open(&dir).unwrap();
        db.put("sessions", "s1", &serde_json::json!({"id":"s1","n":1})).unwrap();
        let got: Option<serde_json::Value> = db.get("sessions", "s1").unwrap();
        assert_eq!(got.unwrap()["n"], 1);
        assert_eq!(db.list::<serde_json::Value>("sessions").unwrap().len(), 1);

        for i in 0..5 {
            db.append_line("events", "s1", &serde_json::json!({"i": i})).unwrap();
        }
        let tail: Vec<serde_json::Value> = db.read_lines("events", "s1", 2).unwrap();
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[1]["i"], 4);

        db.with_bucket_map::<Vec<String>, _>("terms", "记忆", |key, m| {
            m.entry(key.to_string()).or_default().push("e1".into());
        })
        .unwrap();
        let m: std::collections::HashMap<String, Vec<String>> = db.read_bucket_map("terms", "记忆").unwrap();
        assert_eq!(m["记忆"], vec!["e1".to_string()]);

        db.delete("sessions", "s1").unwrap();
        assert!(db.get::<serde_json::Value>("sessions", "s1").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
