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
        let header = String::from_utf8_lossy(&output[offset..newline]);
        offset = newline + 1;
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
        if offset + size > output.len() {
            break;
        }
        values.insert((*file).to_owned(), output[offset..offset + size].to_vec());
        offset = (offset + size + 1).min(output.len());
    }
    values
}
