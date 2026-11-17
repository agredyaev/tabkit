//! Bounded source-preserving text patch application.
//!
//! Patches carry an expected source slice and are admitted only when ranges do
//! not overlap and the resulting candidate remains within configured limits.

use crate::{Error, Result, require, xml::Span};
use serde::{Deserialize, Serialize};

/// One expected-source replacement over a byte span.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    pub span: Span,
    pub expected: String,
    pub replacement: String,
    pub reason: String,
}

/// Applies admitted non-overlapping patches and verifies unchanged regions.
pub fn apply(input: &str, patches: &[Patch], max_bytes: u64) -> Result<String> {
    let (sorted, output_len) = admitted_layout(input, patches, max_bytes)?;
    let mut output = String::new();
    output
        .try_reserve_exact(output_len)
        .map_err(|_| Error::new("LIMIT", "Cannot reserve candidate buffer"))?;

    let mut cursor = 0usize;
    for patch in &sorted {
        let range = patch.span.range();
        output.push_str(&input[cursor..range.start]);
        output.push_str(&patch.replacement);
        cursor = range.end;
    }
    output.push_str(&input[cursor..]);

    verify_preservation_sorted(input, &output, &sorted)?;
    Ok(output)
}

fn verify_preservation_sorted(input: &str, output: &str, sorted: &[&Patch]) -> Result<()> {
    let mut input_cursor = 0usize;
    let mut output_cursor = 0usize;
    for patch in sorted {
        let range = patch.span.range();
        require(
            range.start >= input_cursor && range.end <= input.len(),
            "PRESERVATION",
            "Invalid patch range",
        )?;
        let unchanged = &input[input_cursor..range.start];
        let unchanged_end = output_cursor + unchanged.len();
        require(
            output.get(output_cursor..unchanged_end) == Some(unchanged),
            "PRESERVATION",
            "Unrelated XML bytes changed",
        )?;
        output_cursor = unchanged_end + patch.replacement.len();
        input_cursor = range.end;
    }

    require(
        output.get(output_cursor..) == Some(&input[input_cursor..]),
        "PRESERVATION",
        "Trailing unrelated XML bytes changed",
    )
}

fn admitted_layout<'a>(
    input: &str,
    patches: &'a [Patch],
    max_bytes: u64,
) -> Result<(Vec<&'a Patch>, usize)> {
    let mut sorted: Vec<&Patch> = patches.iter().collect();
    sorted.sort_by_key(|patch| (patch.span.start, patch.span.end));

    let mut cursor = 0usize;
    let mut output_len = input.len() as u64;
    for patch in &sorted {
        let range = patch.span.range();
        require(
            range.start >= cursor && range.end <= input.len(),
            "PATCH_OVERLAP",
            "Overlapping or invalid patch ranges",
        )?;
        let original = input
            .get(range.clone())
            .ok_or_else(|| Error::new("PATCH_BOUNDARY", "Patch splits a UTF-8 character"))?;
        require(
            original == patch.expected,
            "STALE_PRECONDITION",
            "Expected patch bytes changed",
        )?;
        output_len = output_len
            .checked_sub((range.end - range.start) as u64)
            .and_then(|size| size.checked_add(patch.replacement.len() as u64))
            .ok_or_else(|| Error::new("LIMIT", "Patched XML size overflow"))?;
        cursor = range.end;
    }

    require(
        output_len <= max_bytes && output_len <= usize::MAX as u64,
        "LIMIT",
        "Patched XML exceeds configured limit",
    )?;
    Ok((sorted, output_len as usize))
}
