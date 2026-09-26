use crate::{
    error::{
        Error,
        Result,
        require
    },
    fs::sha256,
    xml::Span
};
use serde::{
    Deserialize,
    Serialize
};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    pub span: Span,
    pub expected: String,
    pub replacement: String,
    pub reason: String
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delta {
    pub object: String,
    pub property: String,
    pub before: serde_json::Value,
    pub after: serde_json::Value,
}
/// Sorted, non-overlapping spans; copy unchanged runs once (linear, not repeated inserts).
pub fn apply(input: &str, expected_hash: &str, patches: &[Patch], max: u64) -> Result<String> {
    require(sha256(input.as_bytes()) == expected_hash, "STALE_BASE", "TWB hash changed")?;
    let mut sorted: Vec<&Patch> = patches.iter().collect();
    sorted.sort_by_key(|p| (p.span.start, p.span.end));
    let mut cursor = 0usize;
    let mut length = input.len() as u64;
    for p in &sorted {
        let r = p.span.range();
        require(r.start >= cursor && r.start <= r.end && r.end <= input.len(), "PATCH_OVERLAP", "Overlapping or invalid byte ranges")?;
        let original = input.get(r.clone()).ok_or_else(|| Error::new("PATCH_BOUNDARY", "Patch splits UTF-8 character"))?;
        require(original == p.expected, "STALE_PRECONDITION", "Expected patch bytes changed")?;
        length = length.checked_sub((r.end-r.start) as u64)
            .and_then(|v|v.checked_add(p.replacement.len() as u64))
            .ok_or_else(||Error::new("LIMIT","Patched XML size overflow"))?;
        cursor = r.end;
    }
    require(length <= max && length <= usize::MAX as u64,"LIMIT","Patched XML exceeds limit")?;
    let mut out = String::new();
    out.try_reserve_exact(length as usize).map_err(|_|Error::new("LIMIT","Cannot reserve candidate buffer"))?;
    cursor = 0;
    for p in sorted {
        let r=p.span.range();
        out.push_str(&input[cursor..r.start]);
        out.push_str(&p.replacement);
        cursor = r.end;
    }
    out.push_str(&input[cursor..]);
    Ok(out)
}
/// Independent preservation walk over input/output offsets, not a second serializer.
pub fn verify_preservation(input: &str, output: &str, patches: &[Patch]) -> Result<()> {
    let mut patches: Vec<&Patch> = patches.iter().collect();
    patches.sort_by_key(|p| (p.span.start, p.span.end));
    let (mut i, mut o) = (0usize, 0usize);
    for p in patches {
        let start = p.span.start as usize;
        let end = p.span.end as usize;
        require(start >= i && start <= end && end <= input.len(), "PRESERVATION", "Invalid or overlapping source ranges")?;
        require(input.get(start..end) == Some(p.expected.as_str()), "PRESERVATION", "Source range or expected bytes are invalid")?;
        let len = start - i;
        let keep_end = o.checked_add(len).ok_or_else(|| Error::new("PRESERVATION", "Output offset overflow"))?;
        require(output.as_bytes().get(o..keep_end) == Some(&input.as_bytes()[i..start]), "PRESERVATION", "Unrelated XML bytes changed")?;
        let replace_end = keep_end.checked_add(p.replacement.len()).ok_or_else(|| Error::new("PRESERVATION", "Output offset overflow"))?;
        require(output.as_bytes().get(keep_end..replace_end) == Some(p.replacement.as_bytes()), "PRESERVATION", "Patch output mismatch")?;
        i = end;
        o = replace_end;
    }
    require(input.as_bytes().get(i..) == output.as_bytes().get(o..), "PRESERVATION", "Trailing XML changed")
}
