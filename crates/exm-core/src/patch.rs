//! 统一 diff（unified diff）补丁解析与应用 —— `patch` 工具的纯计算核心。
//!
//! 契约（openspec capability-completion / coding-workbench）：
//! - 逐文件上下文校验：任一文件校验失败 → 整体拒绝，调用方不得落任何写入
//! - 上下文行匹配允许行尾空白容差（trailing whitespace 差异不算失配）
//! - 本模块只做纯计算（解析 + 内存应用），检查点与落盘由调用方（ToolGateway）统一执行
//!
//! 支持的输入形态：多文件段；git 扩展头（`diff --git` / `new file mode` / `deleted file mode`）；
//! 新建（`--- /dev/null`）与删除（`+++ /dev/null`）；`\ No newline at end of file` 标记（跳过，
//! 末尾换行沿用旧文件惯例）。不支持：二进制补丁（GIT binary patch）——解析报错。

use anyhow::{bail, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Add,
    Del,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkLine {
    pub kind: LineKind,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Hunk {
    /// 旧文件起始行（1 起；0 = 新文件）
    pub old_start: usize,
    /// 新文件起始行（1 起）
    pub new_start: usize,
    /// 头声明的旧文件行数（上下文 + 删除）
    pub old_count: usize,
    /// 头声明的新文件行数（上下文 + 新增）
    pub new_count: usize,
    pub lines: Vec<HunkLine>,
}

/// hunk 行消费进度（判定 hunk 何时结束：两侧行数均满足）
#[derive(Default, Clone, Copy)]
struct HunkSeen {
    old_seen: usize,
    new_seen: usize,
}

#[derive(Debug, Clone, Default)]
pub struct FilePatch {
    /// 应用目标路径（+++ 侧，去掉 `b/` 前缀；相对工作区）
    pub path: String,
    pub new_file: bool,
    pub delete_file: bool,
    pub hunks: Vec<Hunk>,
}

const NO_NEWLINE_MARK: &str = "\\ No newline at end of file";

/// `a/foo` / `b/foo` / `foo` → `foo`；`/dev/null` → None
fn strip_prefix_path(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw == "/dev/null" {
        return None;
    }
    let cleaned = raw
        .strip_prefix("a/")
        .or_else(|| raw.strip_prefix("b/"))
        .unwrap_or(raw);
    Some(cleaned.trim_end().to_string())
}

fn parse_hunk_header(line: &str) -> Option<Hunk> {
    let inner = line.trim().strip_prefix("@@")?;
    let inner = inner.trim_start();
    let mut old_start = 1usize;
    let mut new_start = 1usize;
    let mut old_count = 1usize;
    let mut new_count = 1usize;
    for tok in inner.split_whitespace().take(2) {
        let (sign, num) = (tok.as_bytes()[0], &tok[1..]);
        let mut parts = num.split(',');
        let value: usize = parts.next().unwrap_or("").parse().unwrap_or(1);
        let count: usize = parts.next().and_then(|c| c.parse().ok()).unwrap_or(1);
        match sign {
            b'-' => {
                old_start = value;
                old_count = count;
            }
            b'+' => {
                new_start = value;
                new_count = count;
            }
            _ => return None,
        }
    }
    Some(Hunk { old_start, new_start, old_count, new_count, lines: Vec::new() })
}

/// 解析统一 diff 文本为逐文件补丁；无法识别任何文件段时报错
pub fn parse_unified_diff(text: &str) -> Result<Vec<FilePatch>> {
    let mut patches: Vec<FilePatch> = Vec::new();
    let mut cur: Option<FilePatch> = None;
    let mut cur_hunk: Option<Hunk> = None;
    let mut seen = HunkSeen::default();
    let mut in_hunk = false;

    let flush_hunk = |cur: &mut Option<FilePatch>, hunk: &mut Option<Hunk>, seen: &mut HunkSeen| {
        if let Some(h) = hunk.take() {
            if let Some(p) = cur.as_mut() {
                p.hunks.push(h);
            }
        }
        *seen = HunkSeen::default();
    };

    for raw in text.lines() {
        if raw.starts_with('\\') && raw.trim() == NO_NEWLINE_MARK {
            continue; // 换行标记：跳过（末尾换行沿用旧文件惯例）
        }
        if in_hunk {
            // hunk 完整性判定：头声明的两侧行数均已满足 → hunk 结束。
            // 否则下一文件的 "--- a/x" 会被当作删除行吞进当前 hunk。
            let complete = {
                let h = cur_hunk.as_ref().expect("in_hunk 但无 hunk");
                !h.lines.is_empty() && seen.old_seen >= h.old_count && seen.new_seen >= h.new_count
            };
            if complete {
                flush_hunk(&mut cur, &mut cur_hunk, &mut seen);
                in_hunk = false;
                // 落出 hunk 后按头部行继续处理（不得 return）
            } else {
                let kind = match raw.as_bytes().first() {
                    Some(b' ') => Some((LineKind::Context, 1usize, 1usize)),
                    Some(b'+') => Some((LineKind::Add, 0usize, 1usize)),
                    Some(b'-') => Some((LineKind::Del, 1usize, 0usize)),
                    _ => None,
                };
                match kind {
                    Some((kind, old_step, new_step)) => {
                        if let Some(h) = cur_hunk.as_mut() {
                            h.lines.push(HunkLine { kind, text: raw[1..].to_string() });
                        }
                        seen.old_seen += old_step;
                        seen.new_seen += new_step;
                        continue;
                    }
                    None => {
                        // 空行/异常行：hunk 就此截断，落出后按头部行处理
                        flush_hunk(&mut cur, &mut cur_hunk, &mut seen);
                        in_hunk = false;
                    }
                }
            }
        }

        if let Some(rest) = raw.strip_prefix("@@ ") {
            flush_hunk(&mut cur, &mut cur_hunk, &mut seen);
            match parse_hunk_header(&format!("@@ {rest}")) {
                Some(h) => {
                    cur_hunk = Some(h);
                    in_hunk = true;
                }
                None => bail!("无法解析 hunk 头: {raw}"),
            }
            continue;
        }
        if let Some(rest) = raw.strip_prefix("--- ") {
            flush_hunk(&mut cur, &mut cur_hunk, &mut seen);
            let old_path = strip_prefix_path(rest);
            let is_new = old_path.is_none();
            // 归档前一段（有完整路径与 hunk 才算有效段）；随后开启新段。
            // 旧路径先行落入 path：删除文件段（+++ /dev/null）没有新路径，目标取旧路径。
            if let Some(p) = cur.take() {
                if !p.path.is_empty() && !p.hunks.is_empty() {
                    patches.push(p);
                }
            }
            cur = Some(FilePatch {
                path: old_path.unwrap_or_default(),
                new_file: is_new,
                ..Default::default()
            });
            continue;
        }
        if let Some(rest) = raw.strip_prefix("+++ ") {
            match cur.as_mut() {
                Some(p) => {
                    if let Some(path) = strip_prefix_path(rest) {
                        p.path = path;
                    } else {
                        p.delete_file = true;
                    }
                }
                None => bail!("补丁格式错误：+++ 出现在 --- 之前"),
            }
            continue;
        }
        if raw.starts_with("diff --git ") {
            // git 扩展头：开启新文件段（路径由后续 ---/+++ 补齐）
            flush_hunk(&mut cur, &mut cur_hunk, &mut seen);
            if cur.is_some() && cur.as_ref().map(|p| !p.hunks.is_empty()).unwrap_or(false) {
                patches.push(cur.take().unwrap());
            } else {
                cur = None;
            }
            continue;
        }
        if raw.starts_with("new file mode") {
            if let Some(p) = cur.as_mut() {
                p.new_file = true;
            }
            continue;
        }
        if raw.starts_with("deleted file mode") {
            if let Some(p) = cur.as_mut() {
                p.delete_file = true;
            }
            continue;
        }
        // 其余行（index/similarity/空行等）忽略
    }
    flush_hunk(&mut cur, &mut cur_hunk, &mut seen);
    if let Some(p) = cur.take() {
        if !p.path.is_empty() && !p.hunks.is_empty() {
            patches.push(p);
        }
    }
    if patches.is_empty() {
        bail!("未解析到任何补丁文件段");
    }
    for p in &patches {
        if p.path.is_empty() {
            bail!("补丁缺少 +++ 目标路径");
        }
    }
    Ok(patches)
}

fn lines_match(expected: &str, actual: &str) -> bool {
    expected.trim_end() == actual.trim_end()
}

/// 极简统一 diff 生成器：掐头去尾找公共前后缀，中间整体替换为一个 hunk。
/// 不是最小 LCS diff，但格式合法、可被 `patch` 工具回放，适用于检查点对比等
/// "两份已知内容的差异展示"场景（git 口径的精确 diff 由 git 自身产出）。
pub fn simple_unified_diff(old: &str, new: &str, path: &str, context: usize) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    if a == b {
        return String::new();
    }
    // 公共前缀
    let mut prefix = 0usize;
    while prefix < a.len() && prefix < b.len() && a[prefix] == b[prefix] {
        prefix += 1;
    }
    // 公共后缀（不与前缀重叠）
    let mut suffix = 0usize;
    while suffix < a.len() - prefix && suffix < b.len() - prefix
        && a[a.len() - 1 - suffix] == b[b.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let old_mid = &a[prefix..a.len() - suffix];
    let new_mid = &b[prefix..b.len() - suffix];
    let ctx = context.min(prefix.min(suffix));

    let old_start = prefix + 1 - ctx; // 1 起
    let new_start = prefix + 1 - ctx;
    let old_count = ctx + old_mid.len() + ctx;
    let new_count = ctx + new_mid.len() + ctx;

    let mut out = format!("--- a/{path}\n+++ b/{path}\n@@ -{old_start},{old_count} +{new_start},{new_count} @@\n");
    for line in &a[prefix - ctx..prefix] {
        out.push_str(&format!(" {line}\n"));
    }
    for line in old_mid {
        out.push_str(&format!("-{line}\n"));
    }
    for line in new_mid {
        out.push_str(&format!("+{line}\n"));
    }
    for line in &b[b.len() - suffix..b.len() - suffix + ctx] {
        out.push_str(&format!(" {line}\n"));
    }
    out
}

/// 把单个文件补丁应用到旧内容上（纯计算；失败返回带行号上下文的错误描述）
pub fn apply_file(old: &str, fp: &FilePatch) -> std::result::Result<String, String> {
    if fp.new_file && !old.is_empty() {
        return Err(format!("{} 标记为新文件但已存在内容", fp.path));
    }
    let had_trailing_newline = old.ends_with('\n');
    let lines: Vec<String> = if old.is_empty() {
        Vec::new()
    } else {
        old.lines().map(|s| s.to_string()).collect()
    };
    let mut result: Vec<String> = Vec::new();
    let mut cursor = 0usize; // 0-based 下一个未消费行

    for hunk in &fp.hunks {
        let target = hunk.old_start.saturating_sub(1);
        if target > lines.len() {
            return Err(format!(
                "{} hunk 目标行 {} 超出文件行数 {}",
                fp.path,
                hunk.old_start,
                lines.len()
            ));
        }
        while cursor < target {
            result.push(lines[cursor].clone());
            cursor += 1;
        }
        for hl in &hunk.lines {
            match hl.kind {
                LineKind::Context | LineKind::Del => {
                    match lines.get(cursor) {
                        Some(actual) if lines_match(&hl.text, actual) => {
                            if hl.kind == LineKind::Context {
                                result.push(actual.clone());
                            }
                            cursor += 1;
                        }
                        Some(actual) => {
                            return Err(format!(
                                "{} 第 {} 行上下文不匹配：补丁期望 {:?}，实际 {:?}",
                                fp.path,
                                cursor + 1,
                                hl.text,
                                actual
                            ));
                        }
                        None => {
                            return Err(format!(
                                "{} 上下文行超出文件末尾（文件共 {} 行）",
                                fp.path,
                                lines.len()
                            ));
                        }
                    }
                }
                LineKind::Add => result.push(hl.text.clone()),
            }
        }
    }
    while cursor < lines.len() {
        result.push(lines[cursor].clone());
        cursor += 1;
    }

    let mut out = result.join("\n");
    if had_trailing_newline && !out.is_empty() {
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod patch_tests {
    use super::*;

    const MULTI_DIFF: &str = "\
diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,3 +1,4 @@
 fn main() {
-    println!(\"a\");
+    println!(\"a changed\");
+    println!(\"a added\");
 }
diff --git a/src/b.rs b/src/b.rs
--- a/src/b.rs
+++ b/src/b.rs
@@ -1,2 +1,2 @@
 // b header
-const B: u8 = 1;
+const B: u8 = 2;
";

    #[test]
    fn 解析_多文件段与hunk() {
        let patches = parse_unified_diff(MULTI_DIFF).unwrap();
        assert_eq!(patches.len(), 2);
        assert_eq!(patches[0].path, "src/a.rs");
        assert_eq!(patches[0].hunks.len(), 1);
        assert_eq!(patches[0].hunks[0].old_start, 1);
        // 1 上下文 + 1 删除 + 2 新增 + 1 上下文 = 5 行
        assert_eq!(patches[0].hunks[0].lines.len(), 5);
        assert_eq!(patches[1].path, "src/b.rs");
    }

    #[test]
    fn 应用_多文件顺序应用() {
        let patches = parse_unified_diff(MULTI_DIFF).unwrap();
        let a_old = "fn main() {\n    println!(\"a\");\n}\n";
        let b_old = "// b header\nconst B: u8 = 1;\n";
        let a_new = apply_file(a_old, &patches[0]).unwrap();
        let b_new = apply_file(b_old, &patches[1]).unwrap();
        assert_eq!(a_new, "fn main() {\n    println!(\"a changed\");\n    println!(\"a added\");\n}\n");
        assert_eq!(b_new, "// b header\nconst B: u8 = 2;\n");
        // 顺序应用：第二个文件不受第一个文件影响（互相独立）
        let b_again = apply_file(b_old, &patches[1]).unwrap();
        assert_eq!(b_again, b_new);
    }

    #[test]
    fn 应用_上下文不匹配整体报错带行号() {
        let patches = parse_unified_diff(MULTI_DIFF).unwrap();
        let b_bad = "// 不同的头部\nconst B: u8 = 1;\n";
        let err = apply_file(b_bad, &patches[1]).unwrap_err();
        assert!(err.contains("src/b.rs"), "错误需带文件路径: {err}");
        assert!(err.contains("上下文不匹配"), "错误需说明原因: {err}");
        // 调用方契约：任一文件失败 → 不产出任何新内容（这里第一个文件即使可应用也作废）
        let a_old = "fn main() {\n    println!(\"a\");\n}\n";
        let _a_new = apply_file(a_old, &patches[0]).unwrap(); // 单独可应用，但整体拒绝由调用方执行
        let b_broken = "// b header\nconst B: u8 = 3;\n";
        assert!(apply_file(b_broken, &patches[1]).is_err());
    }

    #[test]
    fn 应用_行尾空白容差() {
        let patches = parse_unified_diff(MULTI_DIFF).unwrap();
        // 上下文/被删行带行尾空格（文件里常见脏数据）也应匹配
        let a_dirty = "fn main() {   \n    println!(\"a\");  \n}\n";
        let a_new = apply_file(a_dirty, &patches[0]).unwrap();
        assert!(a_new.contains("a changed"));
        // 补丁行带尾随空格同样容差（去掉文件末尾换行不影响应用）
        assert!(apply_file(a_dirty.trim_end(), &patches[0]).is_ok());
    }

    #[test]
    fn 解析_新建与删除文件() {
        let diff = "\
diff --git a/new.txt b/new.txt
new file mode 100644
--- /dev/null
+++ b/new.txt
@@ -0,0 +1,2 @@
+hello
+world
diff --git a/old.txt b/old.txt
deleted file mode 100644
--- a/old.txt
+++ /dev/null
@@ -1,1 +0,0 @@
-goodbye
";
        let patches = parse_unified_diff(diff).unwrap();
        assert_eq!(patches.len(), 2);
        assert!(patches[0].new_file);
        assert_eq!(patches[0].path, "new.txt");
        let out = apply_file("", &patches[0]).unwrap();
        assert_eq!(out, "hello\nworld");
        assert!(patches[1].delete_file);
        let out = apply_file("goodbye\n", &patches[1]).unwrap();
        assert_eq!(out, "");
    }

    #[test]
    fn 解析_非git风格多文件() {
        let diff = "\
--- a/x.txt
+++ b/x.txt
@@ -1,1 +1,1 @@
-old
+new
--- a/y.txt
+++ b/y.txt
@@ -1,1 +1,1 @@
-alpha
+beta
";
        let patches = parse_unified_diff(diff).unwrap();
        assert_eq!(patches.len(), 2, "无 diff --git 分隔也要拆成两段");
        assert_eq!(patches[0].path, "x.txt");
        assert_eq!(patches[1].path, "y.txt");
        assert_eq!(apply_file("old\n", &patches[0]).unwrap(), "new\n");
        assert_eq!(apply_file("alpha\n", &patches[1]).unwrap(), "beta\n");
    }

    #[test]
    fn 生成_极简统一diff可回放() {
        let old = "fn main() {\n    println!(\"a\");\n    let x = 1;\n    let y = 2;\n    let z = 3;\n}\n";
        let new = "fn main() {\n    println!(\"a\");\n    let x = 1;\n    let y = 20;\n    let z = 3;\n}\n";
        let diff = simple_unified_diff(old, new, "src/m.rs", 3);
        assert!(diff.contains("--- a/src/m.rs"));
        assert!(diff.contains("-    let y = 2;"));
        assert!(diff.contains("+    let y = 20;"));
        // 生成的 diff 必须能被自家解析器应用回放（新内容往返一致）
        let patches = parse_unified_diff(&diff).unwrap();
        assert_eq!(patches.len(), 1);
        let applied = apply_file(old, &patches[0]).unwrap();
        assert_eq!(applied, new);
        // 相同内容 → 空 diff
        assert_eq!(simple_unified_diff(old, old, "f", 3), "");
    }

    #[test]
    fn 解析_垃圾输入报错() {
        assert!(parse_unified_diff("这不是 diff").is_err());
        assert!(parse_unified_diff("").is_err());
    }

    #[test]
    fn 应用_上下文超出文件末尾报错() {
        let diff = "\
--- a/f.txt
+++ b/f.txt
@@ -5,2 +5,2 @@
-x
-y
";
        let mut fp = parse_unified_diff(diff).unwrap();
        let err = apply_file("only\none\n", &fp.remove(0)).unwrap_err();
        assert!(err.contains("超出") || err.contains("不匹配"), "{err}");
    }
}
