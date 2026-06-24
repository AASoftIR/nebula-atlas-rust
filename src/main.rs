use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const MAX_FILE_BYTES: u64 = 512 * 1024;

#[derive(Clone, Debug)]
struct FileInfo {
    id: usize,
    path: String,
    lang: String,
    ext: String,
    stem: String,
    size: u64,
    lines: usize,
    blank: usize,
    comment: usize,
    code: usize,
    todos: Vec<String>,
    warnings: Vec<String>,
    imports: Vec<String>,
    risk: usize,
    modified: String,
}

#[derive(Clone, Debug)]
struct Edge {
    from: usize,
    to: usize,
    label: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return Ok(());
    }

    let root_arg = PathBuf::from(args.get(1).map(|s| s.as_str()).unwrap_or("."));
    let output = PathBuf::from(args.get(2).map(|s| s.as_str()).unwrap_or("atlas.html"));

    let root = match fs::canonicalize(&root_arg) {
        Ok(path) => path,
        Err(_) => root_arg,
    };

    if !root.exists() {
        return Err(format!("Path does not exist: {}", root.display()).into());
    }

    println!("🦀 Scanning: {}", root.display());

    let paths = collect_files(&root)?;
    let mut files = Vec::new();

    for path in paths {
        match analyze_file(&root, &path, files.len()) {
            Ok(Some(info)) => files.push(info),
            Ok(None) => {}
            Err(err) => eprintln!("Skipped {}: {}", path.display(), err),
        }
    }

    let edges = build_edges(&files);
    let html = make_html(&root, &files, &edges);

    fs::write(&output, html)?;

    let total_lines: usize = files.iter().map(|f| f.lines).sum();
    let total_warnings: usize = files.iter().map(|f| f.warnings.len()).sum();

    println!("✅ Atlas created: {}", output.display());
    println!("📁 Files analyzed: {}", files.len());
    println!("📏 Lines scanned: {}", total_lines);
    println!("⚠️  Warnings found: {}", total_warnings);
    println!("🌌 Open the HTML file in your browser.");

    Ok(())
}

fn print_help() {
    println!(
        r#"Nebula Code Atlas

Usage:
  nebula-code-atlas [project_path] [output_html]

Examples:
  cargo run --release
  cargo run --release -- . atlas.html
  cargo run --release -- ../my-project my-project-atlas.html

What it does:
  - Scans source/config/text files
  - Builds a visual offline HTML report
  - Detects TODOs, suspicious secrets, unsafe Rust, unwrap(), eval(), and more
  - Creates a searchable project galaxy
"#
    );
}

fn collect_files(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    visit_dir(root, &mut files)?;
    files.sort_by_key(|p| p.to_string_lossy().to_lowercase());
    Ok(files)
}

fn visit_dir(dir: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    let mut entries = Vec::new();

    for entry in fs::read_dir(dir)? {
        if let Ok(entry) = entry {
            entries.push(entry.path());
        }
    }

    entries.sort_by_key(|p| p.to_string_lossy().to_lowercase());

    for path in entries {
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(_) => continue,
        };

        if meta.file_type().is_symlink() {
            continue;
        }

        if meta.is_dir() {
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();

            if should_skip_dir(&name) {
                continue;
            }

            visit_dir(&path, files)?;
        } else if meta.is_file() && meta.len() <= MAX_FILE_BYTES && is_probably_text(&path) {
            files.push(path);
        }
    }

    Ok(())
}

fn should_skip_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".hg"
            | ".svn"
            | "target"
            | "node_modules"
            | "dist"
            | "build"
            | ".next"
            | ".nuxt"
            | "vendor"
            | "venv"
            | ".venv"
            | "__pycache__"
            | ".idea"
            | ".vscode"
            | "coverage"
            | ".cache"
            | ".cargo"
            | ".gradle"
            | "bin"
            | "obj"
    )
}

fn is_probably_text(path: &Path) -> bool {
    language_from_path(path).is_some()
}

fn analyze_file(root: &Path, abs: &Path, id: usize) -> io::Result<Option<FileInfo>> {
    let lang = match language_from_path(abs) {
        Some(lang) => lang.to_string(),
        None => return Ok(None),
    };

    let bytes = fs::read(abs)?;

    if bytes.contains(&0) {
        return Ok(None);
    }

    let content = String::from_utf8_lossy(&bytes);
    let meta = fs::metadata(abs)?;

    let rel = abs
        .strip_prefix(root)
        .unwrap_or(abs)
        .to_string_lossy()
        .replace('\\', "/");

    let ext = abs
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    let stem = abs
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    let mut lines = 0;
    let mut blank = 0;
    let mut comment = 0;
    let mut code = 0;
    let mut todos = Vec::new();
    let mut warnings = Vec::new();
    let mut imports = Vec::new();

    for (index, line) in content.lines().enumerate() {
        let line_no = index + 1;
        lines += 1;

        let trimmed = line.trim();

        if trimmed.is_empty() {
            blank += 1;
            continue;
        }

        if is_comment_line(trimmed, &lang) {
            comment += 1;
        } else {
            code += 1;
        }

        let lower = trimmed.to_lowercase();

        if contains_any(&lower, &["todo", "fixme", "hack", "bug", "xxx"]) {
            if todos.len() < 80 {
                todos.push(format!("L{}: {}", line_no, preview(trimmed)));
            }
        }

        if looks_like_secret(&lower, trimmed) {
            push_warning(
                &mut warnings,
                line_no,
                "Possible hard-coded secret",
                trimmed,
            );
        }

        if lang == "Rust" {
            if lower.contains("unsafe") {
                push_warning(&mut warnings, line_no, "Rust unsafe block/keyword", trimmed);
            }

            if trimmed.contains(".unwrap()") || trimmed.contains(".expect(") {
                push_warning(
                    &mut warnings,
                    line_no,
                    "Rust panic-prone unwrap()/expect()",
                    trimmed,
                );
            }
        }

        if matches!(lang.as_str(), "JavaScript" | "TypeScript") {
            if lower.contains("eval(") {
                push_warning(&mut warnings, line_no, "JavaScript eval()", trimmed);
            }

            if lower.contains("innerhtml") {
                push_warning(&mut warnings, line_no, "Possible unsafe innerHTML usage", trimmed);
            }
        }

        if lang == "Python" && lower.contains("pickle.loads") {
            push_warning(&mut warnings, line_no, "Python pickle.loads()", trimmed);
        }

        if lang == "SQL" && lower.contains("select *") {
            push_warning(&mut warnings, line_no, "SQL SELECT *", trimmed);
        }

        imports.extend(extract_imports(trimmed, &lang));
    }

    imports = dedup_vec(imports);

    let mut risk = warnings.len() * 2 + todos.len();

    if lines > 400 {
        risk += 3;
        warnings.push(format!("File is long: {} lines", lines));
    }

    if meta.len() > 200 * 1024 {
        risk += 2;
        warnings.push(format!("File is large: {} KB", meta.len() / 1024));
    }

    if lines > 80 && comment == 0 {
        risk += 1;
        warnings.push("No comment lines detected in a non-trivial file".to_string());
    }

    if imports.len() > 25 {
        risk += 1;
        warnings.push(format!("Many imports detected: {}", imports.len()));
    }

    Ok(Some(FileInfo {
        id,
        path: rel,
        lang,
        ext,
        stem,
        size: meta.len(),
        lines,
        blank,
        comment,
        code,
        todos,
        warnings,
        imports,
        risk,
        modified: age_label(meta.modified().ok()),
    }))
}

fn push_warning(warnings: &mut Vec<String>, line_no: usize, title: &str, line: &str) {
    if warnings.len() < 120 {
        warnings.push(format!("L{}: {} → {}", line_no, title, preview(line)));
    }
}

fn language_from_path(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();

    match name.as_str() {
        "dockerfile" => return Some("Docker"),
        "makefile" => return Some("Make"),
        "cargo.toml" | "cargo.lock" => return Some("TOML"),
        "package.json" | "package-lock.json" | "tsconfig.json" => return Some("JSON"),
        ".gitignore" | ".env.example" => return Some("Text"),
        "readme" => return Some("Markdown"),
        "license" => return Some("Text"),
        _ => {}
    }

    let ext = path.extension()?.to_string_lossy().to_lowercase();

    match ext.as_str() {
        "rs" => Some("Rust"),
        "js" | "jsx" => Some("JavaScript"),
        "ts" | "tsx" => Some("TypeScript"),
        "py" => Some("Python"),
        "java" => Some("Java"),
        "c" | "h" => Some("C"),
        "cpp" | "cc" | "cxx" | "hpp" => Some("C++"),
        "cs" => Some("C#"),
        "go" => Some("Go"),
        "php" => Some("PHP"),
        "rb" => Some("Ruby"),
        "ex" | "exs" => Some("Elixir"),
        "erl" | "hrl" => Some("Erlang"),
        "lua" => Some("Lua"),
        "html" | "htm" => Some("HTML"),
        "css" => Some("CSS"),
        "scss" | "sass" => Some("SCSS"),
        "json" => Some("JSON"),
        "toml" => Some("TOML"),
        "yaml" | "yml" => Some("YAML"),
        "md" | "markdown" => Some("Markdown"),
        "sql" => Some("SQL"),
        "sh" | "bash" | "zsh" | "fish" => Some("Shell"),
        "ps1" => Some("PowerShell"),
        "xml" => Some("XML"),
        "kt" | "kts" => Some("Kotlin"),
        "swift" => Some("Swift"),
        "dart" => Some("Dart"),
        "txt" => Some("Text"),
        _ => None,
    }
}

fn is_comment_line(trimmed: &str, lang: &str) -> bool {
    if matches!(lang, "Markdown" | "JSON" | "Text") {
        return false;
    }

    let prefixes: &[&str] = match lang {
        "HTML" | "XML" => &["<!--"],
        "CSS" | "SCSS" => &["/*", "*"],
        "SQL" => &["--", "/*"],
        "Python" | "Shell" | "PowerShell" | "Ruby" | "YAML" | "TOML" | "Docker" | "Make" => {
            &["#"]
        }
        "Lua" => &["--"],
        _ => &["//", "/*", "*", "#", "--"],
    };

    prefixes.iter().any(|prefix| trimmed.starts_with(prefix))
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn looks_like_secret(lower: &str, original: &str) -> bool {
    let keys = [
        "api_key",
        "apikey",
        "secret",
        "password",
        "passwd",
        "token",
        "private_key",
        "access_key",
        "auth_key",
    ];

    contains_any(lower, &keys)
        && (original.contains('=') || original.contains(':'))
        && !contains_any(lower, &["example", "placeholder", "dummy", "your_", "todo"])
}

fn preview(line: &str) -> String {
    let compact = line.split_whitespace().collect::<Vec<_>>().join(" ");

    if compact.chars().count() > 120 {
        compact.chars().take(120).collect::<String>() + "…"
    } else {
        compact
    }
}

fn extract_imports(line: &str, lang: &str) -> Vec<String> {
    let t = line.trim();
    let mut imports = Vec::new();

    match lang {
        "Rust" => {
            for prefix in ["pub mod ", "mod "] {
                if let Some(rest) = t.strip_prefix(prefix) {
                    add_import(&mut imports, &clean_token(rest));
                }
            }

            if let Some(rest) = t.strip_prefix("use ") {
                add_import(&mut imports, &clean_token(rest));
            }
        }

        "JavaScript" | "TypeScript" => {
            if t.starts_with("import ")
                || t.starts_with("export ")
                || t.contains("require(")
                || t.contains(" from ")
            {
                if let Some(q) = first_quoted(t) {
                    add_import(&mut imports, &q);
                }
            }
        }

        "Python" => {
            if let Some(rest) = t.strip_prefix("import ") {
                for part in rest.split(',') {
                    let token = part.trim().split_whitespace().next().unwrap_or("");
                    add_import(&mut imports, token);
                }
            }

            if let Some(rest) = t.strip_prefix("from ") {
                let token = rest.split_whitespace().next().unwrap_or("");
                add_import(&mut imports, token);
            }
        }

        "C" | "C++" => {
            if t.starts_with("#include") {
                if let Some(q) = first_quoted(t) {
                    add_import(&mut imports, &q);
                } else if let Some(angle) = first_angle(t) {
                    add_import(&mut imports, &angle);
                }
            }
        }

        "Java" | "Kotlin" | "Dart" | "Swift" | "Go" => {
            if let Some(rest) = t.strip_prefix("import ") {
                if let Some(q) = first_quoted(rest) {
                    add_import(&mut imports, &q);
                } else {
                    add_import(&mut imports, &clean_token(rest));
                }
            }
        }

        "PHP" => {
            for prefix in ["include ", "include_once ", "require ", "require_once ", "use "] {
                if let Some(rest) = t.strip_prefix(prefix) {
                    if let Some(q) = first_quoted(rest) {
                        add_import(&mut imports, &q);
                    } else {
                        add_import(&mut imports, &clean_token(rest));
                    }
                }
            }
        }

        "Ruby" => {
            for prefix in ["require ", "require_relative ", "load "] {
                if let Some(rest) = t.strip_prefix(prefix) {
                    if let Some(q) = first_quoted(rest) {
                        add_import(&mut imports, &q);
                    }
                }
            }
        }

        "Elixir" | "Erlang" => {
            for prefix in ["alias ", "import ", "use ", "require "] {
                if let Some(rest) = t.strip_prefix(prefix) {
                    add_import(&mut imports, &clean_token(rest));
                }
            }
        }

        "Lua" => {
            if let Some(rest) = t.strip_prefix("require") {
                if let Some(q) = first_quoted(rest) {
                    add_import(&mut imports, &q);
                }
            }
        }

        "HTML" => {
            if t.contains("src=") || t.contains("href=") {
                if let Some(q) = first_quoted(t) {
                    add_import(&mut imports, &q);
                }
            }
        }

        "CSS" | "SCSS" => {
            if t.contains("@import") || t.contains("url(") {
                if let Some(q) = first_quoted(t) {
                    add_import(&mut imports, &q);
                }
            }
        }

        _ => {}
    }

    dedup_vec(imports)
}

fn add_import(imports: &mut Vec<String>, raw: &str) {
    let normalized = normalize_import(raw);

    if normalized.is_empty() {
        return;
    }

    imports.push(normalized.clone());

    let no_ext = strip_known_ext(&normalized);

    if no_ext != normalized {
        imports.push(no_ext.clone());
    }

    let parts: Vec<&str> = no_ext.split('/').filter(|p| !p.is_empty()).collect();

    if let Some(first) = parts.first() {
        imports.push((*first).to_string());
    }

    if let Some(last) = parts.last() {
        imports.push((*last).to_string());
    }
}

fn clean_token(rest: &str) -> String {
    rest.chars()
        .take_while(|c| {
            c.is_ascii_alphanumeric()
                || matches!(*c, '_' | '-' | '.' | '/' | ':' | '\\')
        })
        .collect::<String>()
}

fn normalize_import(raw: &str) -> String {
    let mut s = raw
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_matches(';')
        .replace('\\', "/")
        .replace("::", "/");

    for prefix in ["crate/", "self/", "super/"] {
        while s.starts_with(prefix) {
            s = s[prefix.len()..].to_string();
        }
    }

    while s.starts_with("./") {
        s = s[2..].to_string();
    }

    while s.starts_with("../") {
        s = s[3..].to_string();
    }

    for stop in ['{', ';', ',', ' ', '\t', '('] {
        if let Some(pos) = s.find(stop) {
            s.truncate(pos);
        }
    }

    s.trim_matches('/').trim().to_string()
}

fn first_quoted(s: &str) -> Option<String> {
    for quote in ['"', '\''] {
        if let Some(start) = s.find(quote) {
            let tail = &s[start + quote.len_utf8()..];

            if let Some(end) = tail.find(quote) {
                return Some(tail[..end].to_string());
            }
        }
    }

    None
}

fn first_angle(s: &str) -> Option<String> {
    let start = s.find('<')?;
    let tail = &s[start + 1..];
    let end = tail.find('>')?;
    Some(tail[..end].to_string())
}

fn dedup_vec(items: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();

    for item in items {
        let item = item.trim().trim_matches('/').to_string();

        if item.is_empty() {
            continue;
        }

        let key = item.to_lowercase();

        if seen.insert(key) {
            out.push(item);
        }
    }

    out
}

fn build_edges(files: &[FileInfo]) -> Vec<Edge> {
    let mut index: HashMap<String, Vec<usize>> = HashMap::new();

    for file in files {
        let lower_path = file.path.to_lowercase();
        let path_no_ext = strip_known_ext(&lower_path);

        add_index_key(&mut index, &file.stem, file.id);
        add_index_key(&mut index, &path_no_ext, file.id);

        if let Some(without_src) = path_no_ext.strip_prefix("src/") {
            add_index_key(&mut index, without_src, file.id);
        }

        if let Some(name) = path_no_ext.rsplit('/').next() {
            add_index_key(&mut index, name, file.id);
        }
    }

    let mut edges = Vec::new();
    let mut seen = HashSet::new();

    for file in files {
        for import in &file.imports {
            let keys = import_keys(import);
            let mut connected = false;

            for key in keys {
                if let Some(targets) = index.get(&key) {
                    for &to in targets {
                        if to == file.id {
                            continue;
                        }

                        let dedupe_key = format!("{}>{}>{}", file.id, to, import.to_lowercase());

                        if seen.insert(dedupe_key) {
                            edges.push(Edge {
                                from: file.id,
                                to,
                                label: import.clone(),
                            });
                        }

                        connected = true;
                        break;
                    }
                }

                if connected {
                    break;
                }
            }
        }
    }

    edges.sort_by_key(|e| (e.from, e.to));
    edges
}

fn add_index_key(index: &mut HashMap<String, Vec<usize>>, key: &str, id: usize) {
    let key = key.to_lowercase();

    if !key.is_empty() {
        index.entry(key).or_default().push(id);
    }
}

fn import_keys(import: &str) -> Vec<String> {
    let clean = normalize_import(import).to_lowercase();
    let no_ext = strip_known_ext(&clean);
    let mut keys = vec![clean.clone(), no_ext.clone()];

    if let Some(last) = no_ext.rsplit('/').next() {
        keys.push(last.to_string());
    }

    if let Some(without_src) = no_ext.strip_prefix("src/") {
        keys.push(without_src.to_string());
    }

    dedup_vec(keys)
}

fn strip_known_ext(s: &str) -> String {
    let known = [
        ".rs",
        ".js",
        ".jsx",
        ".ts",
        ".tsx",
        ".py",
        ".java",
        ".c",
        ".h",
        ".cpp",
        ".cc",
        ".cxx",
        ".hpp",
        ".cs",
        ".go",
        ".php",
        ".rb",
        ".ex",
        ".exs",
        ".erl",
        ".hrl",
        ".lua",
        ".html",
        ".htm",
        ".css",
        ".scss",
        ".sass",
        ".json",
        ".toml",
        ".yaml",
        ".yml",
        ".md",
        ".markdown",
        ".sql",
        ".sh",
        ".bash",
        ".zsh",
        ".fish",
        ".ps1",
        ".xml",
        ".kt",
        ".kts",
        ".swift",
        ".dart",
        ".txt",
    ];

    for ext in known {
        if let Some(stripped) = s.strip_suffix(ext) {
            return stripped.to_string();
        }
    }

    s.to_string()
}

fn age_label(modified: Option<SystemTime>) -> String {
    let Some(modified) = modified else {
        return "unknown".to_string();
    };

    match SystemTime::now().duration_since(modified) {
        Ok(duration) => {
            let days = duration.as_secs() / 86_400;

            match days {
                0 => "today".to_string(),
                1 => "1 day ago".to_string(),
                2..=30 => format!("{} days ago", days),
                31..=365 => format!("{} months ago", days / 30),
                _ => format!("{} years ago", days / 365),
            }
        }
        Err(_) => "future".to_string(),
    }
}

fn make_html(root: &Path, files: &[FileInfo], edges: &[Edge]) -> String {
    let mut lang_counts: BTreeMap<String, usize> = BTreeMap::new();

    for file in files {
        *lang_counts.entry(file.lang.clone()).or_insert(0) += 1;
    }

    let mut html = String::new();

    html.push_str(
        r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>Nebula Code Atlas</title>
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <style>
    :root {
      --bg: #08111f;
      --panel: rgba(255, 255, 255, 0.075);
      --panel-strong: rgba(255, 255, 255, 0.13);
      --text: #eaf2ff;
      --muted: #9fb1cf;
      --line: rgba(255, 255, 255, 0.14);
      --good: #61f2a4;
      --warn: #ffd166;
      --hot: #ff5c8a;
      --shadow: 0 18px 60px rgba(0, 0, 0, 0.32);
    }

    * {
      box-sizing: border-box;
    }

    body {
      margin: 0;
      min-height: 100vh;
      color: var(--text);
      font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      background:
        radial-gradient(circle at 18% 8%, rgba(97, 242, 164, 0.18), transparent 28rem),
        radial-gradient(circle at 82% 18%, rgba(116, 144, 255, 0.2), transparent 30rem),
        radial-gradient(circle at 50% 82%, rgba(255, 92, 138, 0.16), transparent 26rem),
        linear-gradient(135deg, #060914, #0b1a31 44%, #060914);
    }

    body::before {
      content: "";
      position: fixed;
      inset: 0;
      pointer-events: none;
      background-image:
        radial-gradient(circle, rgba(255,255,255,.22) 1px, transparent 1px),
        radial-gradient(circle, rgba(255,255,255,.13) 1px, transparent 1px);
      background-size: 46px 46px, 89px 89px;
      mask-image: linear-gradient(to bottom, rgba(0,0,0,0.8), rgba(0,0,0,0.12));
    }

    .wrap {
      width: min(1440px, calc(100% - 32px));
      margin: 0 auto;
      padding: 32px 0 48px;
      position: relative;
      z-index: 1;
    }

    .hero {
      padding: 28px;
      border: 1px solid var(--line);
      border-radius: 30px;
      background: linear-gradient(135deg, rgba(255,255,255,.14), rgba(255,255,255,.055));
      box-shadow: var(--shadow);
      backdrop-filter: blur(18px);
      overflow: hidden;
      position: relative;
    }

    .hero::after {
      content: "";
      position: absolute;
      width: 340px;
      height: 340px;
      right: -130px;
      top: -170px;
      border-radius: 50%;
      background: radial-gradient(circle, rgba(97,242,164,.35), transparent 68%);
      filter: blur(2px);
    }

    .hero h1 {
      margin: 0 0 10px;
      font-size: clamp(2.1rem, 5vw, 4.7rem);
      letter-spacing: -0.07em;
      line-height: 0.92;
    }

    .hero p {
      margin: 0;
      max-width: 880px;
      color: var(--muted);
      font-size: 1.04rem;
      line-height: 1.7;
    }

    .root {
      margin-top: 16px;
      color: #c8d7f7;
      font-family: ui-monospace, SFMono-Regular, Consolas, monospace;
      padding: 12px 14px;
      border-radius: 16px;
      border: 1px solid var(--line);
      background: rgba(0,0,0,.22);
      overflow-wrap: anywhere;
    }

    .controls {
      display: grid;
      grid-template-columns: 1.4fr .75fr .75fr;
      gap: 14px;
      margin: 18px 0;
    }

    .control {
      padding: 14px;
      border-radius: 20px;
      background: var(--panel);
      border: 1px solid var(--line);
      backdrop-filter: blur(14px);
    }

    label {
      display: block;
      color: var(--muted);
      font-size: .82rem;
      margin-bottom: 8px;
    }

    input,
    select {
      width: 100%;
      border: 0;
      outline: 0;
      color: var(--text);
      background: rgba(0,0,0,.27);
      border: 1px solid var(--line);
      border-radius: 14px;
      padding: 12px 13px;
      font: inherit;
    }

    input[type="range"] {
      padding: 10px 0;
    }

    .cards {
      display: grid;
      grid-template-columns: repeat(5, 1fr);
      gap: 14px;
      margin-bottom: 18px;
    }

    .card {
      border: 1px solid var(--line);
      background: var(--panel);
      border-radius: 22px;
      padding: 18px;
      backdrop-filter: blur(14px);
      box-shadow: 0 10px 28px rgba(0,0,0,.16);
    }

    .card b {
      display: block;
      font-size: 1.7rem;
      letter-spacing: -0.04em;
    }

    .card span {
      color: var(--muted);
      font-size: .86rem;
    }

    .main {
      display: grid;
      grid-template-columns: minmax(0, 1fr) 380px;
      gap: 18px;
      align-items: start;
    }

    .panel {
      border: 1px solid var(--line);
      background: var(--panel);
      border-radius: 28px;
      backdrop-filter: blur(16px);
      box-shadow: var(--shadow);
      overflow: hidden;
    }

    .galaxy-panel {
      min-height: 640px;
    }

    svg {
      display: block;
      width: 100%;
      height: 640px;
      background:
        radial-gradient(circle at 50% 50%, rgba(255,255,255,.08), transparent 35%),
        rgba(0,0,0,.12);
    }

    .edge {
      stroke: rgba(255,255,255,.18);
      stroke-width: 1;
    }

    .ring {
      fill: none;
      stroke: rgba(255,255,255,.055);
      stroke-width: 1;
    }

    .node {
      cursor: pointer;
    }

    .node circle {
      stroke: rgba(255,255,255,.75);
      stroke-width: 1.2;
      transition: transform .18s ease, filter .18s ease;
      filter: drop-shadow(0 0 12px rgba(255,255,255,.11));
    }

    .node:hover circle,
    .node:focus circle {
      transform: scale(1.25);
      filter: drop-shadow(0 0 18px rgba(255,255,255,.42));
    }

    .node text {
      pointer-events: none;
      fill: rgba(255,255,255,.82);
      font-size: 13px;
      text-shadow: 0 1px 8px rgba(0,0,0,.8);
    }

    .details {
      padding: 20px;
      min-height: 640px;
    }

    .details h2 {
      margin: 0 0 6px;
      font-size: 1.45rem;
      letter-spacing: -.04em;
      overflow-wrap: anywhere;
    }

    .muted {
      color: var(--muted);
    }

    .chips {
      display: flex;
      flex-wrap: wrap;
      gap: 8px;
      margin: 12px 0;
    }

    .chip {
      display: inline-flex;
      gap: 6px;
      align-items: center;
      border-radius: 999px;
      padding: 7px 10px;
      border: 1px solid var(--line);
      background: rgba(0,0,0,.22);
      color: #dce8ff;
      font-size: .82rem;
    }

    .risk {
      font-weight: 800;
    }

    .risk.cool {
      color: var(--good);
    }

    .risk.warm {
      color: var(--warn);
    }

    .risk.hot {
      color: var(--hot);
    }

    .table-panel {
      margin-top: 18px;
      padding: 18px;
    }

    .table-panel h2 {
      margin: 0 0 12px;
      letter-spacing: -.04em;
    }

    .table-scroll {
      overflow: auto;
      border-radius: 18px;
      border: 1px solid var(--line);
    }

    table {
      width: 100%;
      border-collapse: collapse;
      min-width: 900px;
      background: rgba(0,0,0,.16);
    }

    th,
    td {
      padding: 12px 14px;
      text-align: left;
      border-bottom: 1px solid rgba(255,255,255,.08);
      font-size: .91rem;
    }

    th {
      color: var(--muted);
      font-weight: 700;
      background: rgba(255,255,255,.04);
      position: sticky;
      top: 0;
      backdrop-filter: blur(8px);
    }

    tr {
      cursor: pointer;
    }

    tr:hover {
      background: rgba(255,255,255,.06);
    }

    .path {
      font-family: ui-monospace, SFMono-Regular, Consolas, monospace;
      overflow-wrap: anywhere;
    }

    ul {
      margin: 10px 0;
      padding-left: 19px;
    }

    li {
      margin: 7px 0;
      color: #d8e4fb;
      overflow-wrap: anywhere;
    }

    .footer {
      margin-top: 18px;
      color: var(--muted);
      text-align: center;
      font-size: .9rem;
    }

    @media (max-width: 1050px) {
      .controls,
      .main,
      .cards {
        grid-template-columns: 1fr;
      }

      .details {
        min-height: auto;
      }
    }
  </style>
</head>
<body>
  <div class="wrap">
    <header class="hero">
      <h1>🦀 Nebula Code Atlas</h1>
      <p>
        An offline software galaxy generated by a tiny Rust scanner.
        Search files, inspect risk hotspots, and explore how your project is shaped.
      </p>
      <div class="root" id="rootPath"></div>
    </header>

    <section class="controls">
      <div class="control">
        <label for="searchBox">Search path, language, warning, TODO</label>
        <input id="searchBox" placeholder="Try: rust, auth, todo, unwrap, api_key...">
      </div>

      <div class="control">
        <label for="langFilter">Language</label>
        <select id="langFilter">
          <option value="">All languages</option>
        </select>
      </div>

      <div class="control">
        <label for="riskFilter">Minimum risk: <b id="riskValue">0</b></label>
        <input id="riskFilter" type="range" min="0" max="20" value="0">
      </div>
    </section>

    <section class="cards">
      <div class="card"><b id="statFiles">0</b><span>visible files</span></div>
      <div class="card"><b id="statLines">0</b><span>visible lines</span></div>
      <div class="card"><b id="statEdges">0</b><span>visible links</span></div>
      <div class="card"><b id="statWarnings">0</b><span>warnings</span></div>
      <div class="card"><b id="statTopLang">—</b><span>top language</span></div>
    </section>

    <main class="main">
      <section class="panel galaxy-panel">
        <svg id="galaxy" viewBox="0 0 1600 900" role="img" aria-label="Codebase galaxy"></svg>
      </section>

      <aside class="panel details" id="details">
        <h2>Select a star</h2>
        <p class="muted">
          Each star is a file. Bigger stars have more lines. Hotter stars have more TODOs,
          suspicious patterns, or complexity signals.
        </p>
      </aside>
    </main>

    <section class="panel table-panel">
      <h2>File Control Deck</h2>
      <div class="table-scroll">
        <table>
          <thead>
            <tr>
              <th>Risk</th>
              <th>File</th>
              <th>Language</th>
              <th>Lines</th>
              <th>Code</th>
              <th>Comments</th>
              <th>TODOs</th>
              <th>Warnings</th>
              <th>Modified</th>
            </tr>
          </thead>
          <tbody id="fileRows"></tbody>
        </table>
      </div>
    </section>

    <div class="footer">
      Generated locally. No uploads. No tracking. Just Rust and one HTML file.
    </div>
  </div>

<script>
"##,
    );

    let root_display = root.to_string_lossy().replace('\\', "/");
    let _ = writeln!(&mut html, "const PROJECT_ROOT = {};", js_str(&root_display));

    html.push_str("const files = [\n");

    for file in files {
        let _ = writeln!(
            &mut html,
            "{{id:{},path:{},lang:{},ext:{},size:{},lines:{},code:{},comment:{},blank:{},risk:{},modified:{},todos:{},warnings:{},imports:{}}},",
            file.id,
            js_str(&file.path),
            js_str(&file.lang),
            js_str(&file.ext),
            file.size,
            file.lines,
            file.code,
            file.comment,
            file.blank,
            file.risk,
            js_str(&file.modified),
            js_array(&file.todos),
            js_array(&file.warnings),
            js_array(&file.imports),
        );
    }

    html.push_str("];\nconst edges = [\n");

    for edge in edges {
        let _ = writeln!(
            &mut html,
            "{{from:{},to:{},label:{}}},",
            edge.from,
            edge.to,
            js_str(&edge.label)
        );
    }

    html.push_str(
        r##"];
const rootPath = document.getElementById("rootPath");
const searchBox = document.getElementById("searchBox");
const langFilter = document.getElementById("langFilter");
const riskFilter = document.getElementById("riskFilter");
const riskValue = document.getElementById("riskValue");
const galaxy = document.getElementById("galaxy");
const fileRows = document.getElementById("fileRows");
const details = document.getElementById("details");

rootPath.textContent = PROJECT_ROOT;

riskFilter.max = Math.max(20, ...files.map(file => file.risk));

const languages = [...new Set(files.map(file => file.lang))].sort();

for (const lang of languages) {
  const option = document.createElement("option");
  option.value = lang;
  option.textContent = lang;
  langFilter.appendChild(option);
}

const positions = makePositions();

function makePositions() {
  const sorted = [...files].sort((a, b) => b.lines - a.lines || a.path.localeCompare(b.path));
  const map = {};
  const width = 1600;
  const height = 900;
  const cx = width / 2;
  const cy = height / 2;
  const goldenAngle = 2.399963229728653;

  sorted.forEach((file, index) => {
    const radius = 26 + 34 * Math.sqrt(index + 1);
    const angle = index * goldenAngle;
    map[file.id] = {
      x: cx + Math.cos(angle) * radius,
      y: cy + Math.sin(angle) * radius
    };
  });

  return map;
}

function render() {
  const query = searchBox.value.trim().toLowerCase();
  const lang = langFilter.value;
  const minRisk = Number(riskFilter.value);

  riskValue.textContent = minRisk;

  const visible = files.filter(file => {
    const haystack = [
      file.path,
      file.lang,
      file.ext,
      file.todos.join(" "),
      file.warnings.join(" "),
      file.imports.join(" ")
    ].join(" ").toLowerCase();

    return (!query || haystack.includes(query))
      && (!lang || file.lang === lang)
      && file.risk >= minRisk;
  });

  const visibleIds = new Set(visible.map(file => file.id));
  const visibleEdges = edges.filter(edge => visibleIds.has(edge.from) && visibleIds.has(edge.to));

  document.getElementById("statFiles").textContent = visible.length.toLocaleString();
  document.getElementById("statLines").textContent = sum(visible, "lines").toLocaleString();
  document.getElementById("statEdges").textContent = visibleEdges.length.toLocaleString();
  document.getElementById("statWarnings").textContent =
    visible.reduce((total, file) => total + file.warnings.length, 0).toLocaleString();
  document.getElementById("statTopLang").textContent = topLanguage(visible);

  renderGalaxy(visible, visibleEdges);
  renderTable(visible);
}

function renderGalaxy(visible, visibleEdges) {
  galaxy.replaceChildren();

  for (const radius of [120, 220, 340, 470, 620]) {
    galaxy.appendChild(svgEl("circle", {
      cx: 800,
      cy: 450,
      r: radius,
      class: "ring"
    }));
  }

  for (const edge of visibleEdges) {
    const a = positions[edge.from];
    const b = positions[edge.to];

    if (!a || !b) continue;

    const line = svgEl("line", {
      x1: a.x,
      y1: a.y,
      x2: b.x,
      y2: b.y,
      class: "edge"
    });

    const title = svgEl("title", {});
    title.textContent = edge.label;
    line.appendChild(title);
    galaxy.appendChild(line);
  }

  for (const file of visible) {
    const pos = positions[file.id];
    if (!pos) continue;

    const radius = Math.max(7, Math.min(32, 5 + Math.sqrt(file.lines)));
    const group = svgEl("g", {
      class: "node",
      transform: `translate(${pos.x}, ${pos.y})`,
      tabindex: "0"
    });

    const circle = svgEl("circle", {
      r: radius,
      fill: colorFor(file.lang),
      opacity: file.risk >= 10 ? "1" : file.risk >= 5 ? ".9" : ".78"
    });

    if (file.risk >= 10) {
      circle.setAttribute("stroke", "#ff5c8a");
      circle.setAttribute("stroke-width", "3");
    }

    const title = svgEl("title", {});
    title.textContent = `${file.path} | ${file.lines} lines | risk ${file.risk}`;

    circle.appendChild(title);
    group.appendChild(circle);

    if (visible.length <= 160 || file.risk >= 8 || file.lines > 250) {
      const text = svgEl("text", {
        x: radius + 6,
        y: 4
      });
      text.textContent = basename(file.path);
      group.appendChild(text);
    }

    group.addEventListener("click", () => showDetails(file));
    group.addEventListener("keydown", event => {
      if (event.key === "Enter" || event.key === " ") {
        showDetails(file);
      }
    });

    galaxy.appendChild(group);
  }
}

function renderTable(visible) {
  const rows = [...visible]
    .sort((a, b) => b.risk - a.risk || b.lines - a.lines || a.path.localeCompare(b.path))
    .slice(0, 400);

  fileRows.innerHTML = rows.map(file => `
    <tr onclick="showDetailsById(${file.id})">
      <td><span class="risk ${riskClass(file.risk)}">${file.risk}</span></td>
      <td class="path">${esc(file.path)}</td>
      <td>${esc(file.lang)}</td>
      <td>${file.lines}</td>
      <td>${file.code}</td>
      <td>${file.comment}</td>
      <td>${file.todos.length}</td>
      <td>${file.warnings.length}</td>
      <td>${esc(file.modified)}</td>
    </tr>
  `).join("");
}

function showDetailsById(id) {
  const file = files.find(file => file.id === id);
  if (file) showDetails(file);
}

function showDetails(file) {
  const density = file.lines ? Math.round((file.code / file.lines) * 100) : 0;
  const commentRatio = file.lines ? Math.round((file.comment / file.lines) * 100) : 0;

  details.innerHTML = `
    <h2>${esc(basename(file.path))}</h2>
    <p class="muted path">${esc(file.path)}</p>

    <div class="chips">
      <span class="chip">Language: <b>${esc(file.lang)}</b></span>
      <span class="chip">Risk: <b class="risk ${riskClass(file.risk)}">${file.risk}</b></span>
      <span class="chip">Lines: <b>${file.lines}</b></span>
      <span class="chip">Code density: <b>${density}%</b></span>
      <span class="chip">Comments: <b>${commentRatio}%</b></span>
      <span class="chip">Modified: <b>${esc(file.modified)}</b></span>
    </div>

    <h3>Warnings</h3>
    ${file.warnings.length
      ? `<ul>${file.warnings.slice(0, 35).map(w => `<li>${esc(w)}</li>`).join("")}</ul>`
      : `<p class="muted">No warnings flagged.</p>`}

    <h3>TODO / FIXME / HACK</h3>
    ${file.todos.length
      ? `<ul>${file.todos.slice(0, 35).map(t => `<li>${esc(t)}</li>`).join("")}</ul>`
      : `<p class="muted">No TODO-style notes found.</p>`}

    <h3>Detected imports / links</h3>
    ${file.imports.length
      ? `<div class="chips">${file.imports.slice(0, 35).map(i => `<span class="chip">${esc(i)}</span>`).join("")}</div>`
      : `<p class="muted">No imports detected.</p>`}
  `;
}

function svgEl(name, attrs) {
  const el = document.createElementNS("http://www.w3.org/2000/svg", name);

  for (const [key, value] of Object.entries(attrs)) {
    el.setAttribute(key, value);
  }

  return el;
}

function colorFor(text) {
  let hash = 0;

  for (const char of text) {
    hash = (hash * 31 + char.charCodeAt(0)) % 360;
  }

  return `hsl(${hash} 78% 62%)`;
}

function riskClass(risk) {
  if (risk >= 10) return "hot";
  if (risk >= 5) return "warm";
  return "cool";
}

function topLanguage(list) {
  const counts = new Map();

  for (const file of list) {
    counts.set(file.lang, (counts.get(file.lang) || 0) + 1);
  }

  let best = "—";
  let score = 0;

  for (const [lang, count] of counts) {
    if (count > score) {
      best = lang;
      score = count;
    }
  }

  return best;
}

function sum(list, key) {
  return list.reduce((total, item) => total + item[key], 0);
}

function basename(path) {
  return path.split("/").pop() || path;
}

function esc(value) {
  return String(value).replace(/[&<>"']/g, char => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;"
  }[char]));
}

searchBox.addEventListener("input", render);
langFilter.addEventListener("change", render);
riskFilter.addEventListener("input", render);

render();
</script>
</body>
</html>
"##,
    );

    html
}

fn js_str(s: &str) -> String {
    let mut out = String::new();
    out.push('"');

    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '<' => out.push_str("\\u003C"),
            '>' => out.push_str("\\u003E"),
            '&' => out.push_str("\\u0026"),
            _ => out.push(ch),
        }
    }

    out.push('"');
    out
}

fn js_array(items: &[String]) -> String {
    let mut out = String::from("[");

    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }

        out.push_str(&js_str(item));
    }

    out.push(']');
    out
}
