//! Rebuild English full forms from a pinned ESDB checkout; see docs/english-spelling.md.
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

const REVISION: &str = "1e5b7d3a72f47a71da5d28686c1dd4b397178485";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let [_, source, python, out] = args.as_slice() else {
        return Err("usage: build_en <ESDB checkout> <python executable> <out.txt>".into());
    };
    let source = Path::new(source).canonicalize()?;
    let revision = Command::new("git").current_dir(&source).args(["rev-parse", "HEAD"]).output()?;
    if !revision.status.success() || String::from_utf8(revision.stdout)?.trim() != REVISION {
        return Err(format!("expected ESDB revision {REVISION}").into());
    }
    if !Command::new("git").current_dir(&source).args(["diff", "--quiet", "HEAD", "--"]).status()?.success() {
        return Err("ESDB tracked sources must be unmodified".into());
    }
    let output = Command::new(python)
        .current_dir(&source)
        .args(["-X", "utf8", "-m", "libscowl", "word-list", "60", "A,B,Z,C", "1", "--hyphen", "--nosuggest"])
        .output()?;
    if !output.status.success() {
        return Err(format!("ESDB extraction failed: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    let text = String::from_utf8(output.stdout)?;
    let mut forms = BTreeMap::new();
    for line in text.lines() {
        let (word, suggest) = line.strip_suffix("/!").map_or((line, true), |w| (w, false));
        if word.is_empty() || word.contains(['\t', ' ', '\r']) {
            return Err(format!("invalid full form: {word:?}").into());
        }
        *forms.entry(word).or_insert(true) &= suggest;
    }
    if forms.len() < 100_000 {
        return Err("unexpectedly small English lexicon".into());
    }
    let count = forms.len();
    let mut result = String::new();
    for (word, suggest) in forms {
        result.push_str(word);
        if !suggest {
            result.push_str("\t!");
        }
        result.push('\n');
    }
    std::fs::write(out, result)?;
    println!("{count} forms from ESDB {REVISION}");
    Ok(())
}
