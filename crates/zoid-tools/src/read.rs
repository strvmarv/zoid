use crate::{str_arg, Tool, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

/// Read a UTF-8 text file relative to the working directory.
pub struct Read;

impl Tool for Read {
    fn name(&self) -> &str {
        "Read"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: "Read a UTF-8 text file. Output is line-numbered. Use offset/limit to page through large files.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path":   { "type": "string", "description": "File path relative to the working directory." },
                    "offset": { "type": "integer", "description": "1-indexed line to start from (default 1)." },
                    "limit":  { "type": "integer", "description": "Max lines to return (default 2000)." }
                },
                "required": ["path"]
            }),
        }
    }
    fn run(&self, args: &Value, cwd: &Path) -> ToolOutput {
        const DEFAULT_LIMIT: usize = 2000;
        const MAX_LINE: usize = 2000; // per-line char cap (CC parity) — stops a
                                      // single giant line from blowing context.
        let path = match str_arg(args, "path") {
            Ok(p) => p,
            Err(e) => return e,
        };
        let contents = match std::fs::read_to_string(crate::resolve(cwd, &path)) {
            Ok(c) => c,
            Err(e) => return ToolOutput::err(format!("Read({path}): {e}")),
        };
        let offset = args
            .get("offset")
            .and_then(|v| v.as_u64())
            .map(|n| n.max(1) as usize)
            .unwrap_or(1);
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_LIMIT);
        let lines: Vec<&str> = contents.lines().collect();
        let total = lines.len();
        let start = offset.saturating_sub(1).min(total);
        let end = start.saturating_add(limit).min(total);
        let mut out = String::new();
        for (i, line) in lines[start..end].iter().enumerate() {
            let shown = if line.chars().count() > MAX_LINE {
                let head: String = line.chars().take(MAX_LINE).collect();
                format!("{head}… (line truncated)")
            } else {
                (*line).to_string()
            };
            out.push_str(&format!("{}\t{}\n", offset + i, shown));
        }
        if end < total {
            out.push_str(&format!(
                "… truncated; {} more lines, continue with offset={}\n",
                total - end,
                end + 1
            ));
        }
        ToolOutput::ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    #[test]
    fn reads_existing_file() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "hello tools").unwrap();
        let out = Read.run(
            &json!({ "path": f.path().to_str().unwrap() }),
            std::path::Path::new("."),
        );
        assert!(!out.is_error);
        assert_eq!(out.text, "1\thello tools\n");
    }

    #[test]
    fn missing_file_is_error() {
        let out = Read.run(
            &json!({ "path": "/no/such/zoid/file" }),
            std::path::Path::new("."),
        );
        assert!(out.is_error);
    }

    #[test]
    fn missing_arg_is_error() {
        let out = Read.run(&json!({}), std::path::Path::new("."));
        assert!(out.is_error);
        assert!(out.text.contains("path"));
    }

    #[test]
    fn reads_with_line_numbers() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "alpha\nbeta\ngamma").unwrap();
        let out = Read.run(
            &json!({ "path": f.path().to_str().unwrap() }),
            std::path::Path::new("."),
        );
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(out.text, "1\talpha\n2\tbeta\n3\tgamma\n");
    }

    #[test]
    fn offset_and_limit_page_the_file() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "l1\nl2\nl3\nl4\nl5").unwrap();
        let out = Read.run(
            &json!({ "path": f.path().to_str().unwrap(), "offset": 2, "limit": 2 }),
            std::path::Path::new("."),
        );
        // `end (3) < total (5)`, so a "there's more" notice follows the two
        // requested lines — assert the prefix, not exact equality.
        assert!(out.text.starts_with("2\tl2\n3\tl3\n"), "got: {}", out.text);
        assert!(out.text.contains("offset=4"));
    }

    #[test]
    fn over_long_line_is_truncated() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "{}", "x".repeat(5000)).unwrap();
        let out = Read.run(
            &json!({ "path": f.path().to_str().unwrap() }),
            std::path::Path::new("."),
        );
        assert!(out.text.contains("(line truncated)"));
        assert!(out.text.len() < 4000, "a 5000-char line must not pass through whole");
    }

    #[test]
    fn non_utf8_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bin");
        std::fs::write(&p, [0xff, 0xfe, 0x00]).unwrap();
        let out = Read.run(
            &json!({ "path": p.to_str().unwrap() }),
            std::path::Path::new("."),
        );
        assert!(out.is_error);
    }

    #[test]
    fn over_cap_appends_truncation_notice() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        let body: String = (1..=2100).map(|n| format!("line{n}\n")).collect();
        write!(f, "{body}").unwrap();
        let out = Read.run(
            &json!({ "path": f.path().to_str().unwrap() }),
            std::path::Path::new("."),
        );
        assert!(out.text.starts_with("1\tline1\n"));
        assert!(out.text.contains("truncated"));
        assert!(out.text.contains("offset=2001"));
    }
}
