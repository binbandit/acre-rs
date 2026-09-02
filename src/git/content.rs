//! Reading files at a commit without checking them out.

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::Result;
use crate::git::runner::{RunOptions, run_git_with};

pub fn read_files_at_ref(cwd: &Path, reference: &str, files: &[&str]) -> Result<BTreeMap<String, Vec<u8>>> {
    if files.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut input = String::new();
    for file in files {
        // One `ref:path` request per line; git answers in the same order, which parse_batch relies on.
        input.push_str(reference);
        input.push(':');
        input.push_str(file);
        input.push('\n');
    }
    let result = run_git_with(
        cwd,
        &["cat-file", "--batch"],
        RunOptions {
            stdin: Some(input.into_bytes()),
            ..RunOptions::default()
        },
    )?;
    Ok(parse_batch(&result.stdout, files))
}

fn parse_batch(output: &[u8], files: &[&str]) -> BTreeMap<String, Vec<u8>> {
    let mut values = BTreeMap::new();
    let mut offset = 0;
    for file in files {
        let Some(relative_newline) = output[offset..].iter().position(|byte| *byte == b'\n') else {
            break;
        };
        let newline = offset + relative_newline;
        // Header is `<oid> <type> <size>` or `<request> missing`.
        let header = String::from_utf8_lossy(&output[offset..newline]);
        offset = newline + 1;
        // A missing file gets a header but no body, so there is nothing to skip past.
        if header.ends_with(" missing") {
            continue;
        }
        let size = header
            .split_whitespace()
            .next_back()
            .and_then(|value| value.parse::<usize>().ok());
        let Some(size) = size else {
            continue;
        };
        // Truncated output; trust nothing past this point.
        if offset + size > output.len() {
            break;
        }
        values.insert((*file).to_owned(), output[offset..offset + size].to_vec());
        // Skip the newline git appends after each body.
        offset = (offset + size + 1).min(output.len());
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_present_blobs_and_skips_missing_ones() {
        let output = b"abc123 blob 5\nhello\nHEAD:package.json missing\n";
        let files = parse_batch(output, &["README.md", "package.json"]);
        assert_eq!(files.get("README.md").map(Vec::as_slice), Some(&b"hello"[..]));
        assert!(!files.contains_key("package.json"));
    }
}
