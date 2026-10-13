//! TeX source pane: .tex files open as opaque byte buffers and compile only
//! on explicit request (`tex.compile`) or through the opt-in auto-compile
//! poll (`tex.tick` once `tex.autoCompile` enabled it for the open document).
//! Nothing here parses TeX or touches the document model: file.open keeps its
//! import behaviour. Edits (`tex.edit`) replace the opaque bytes verbatim.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::{CmdError, CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("tex.open", "Open TeX Source", "TeX", open).params(r#"{"path": string}"#).pure(),
        CommandSpec::new("tex.edit", "Edit TeX Source", "TeX", edit).params(r#"{"text": string}"#).pure(),
        CommandSpec::new("tex.compile", "Compile TeX", "TeX", compile)
            .params(r#"{"engine"?: string, "kind"?: "tectonic|pdflatex|xelatex", "timeoutMs"?: n, "rasterizer"?: string}"#)
            .pure(),
        CommandSpec::new("tex.autoCompile", "Auto-Compile TeX", "TeX", auto_compile).params(r#"{"enabled": boolean}"#).pure(),
        CommandSpec::new("tex.tick", "Poll TeX Auto-Compile", "TeX", tick)
            .params(r#"{"engine"?: string, "kind"?: "tectonic|pdflatex|xelatex", "timeoutMs"?: n, "rasterizer"?: string}"#)
            .pure(),
        CommandSpec::new("tex.diagnostics", "TeX Diagnostics", "TeX", diagnostics).params("{}").pure(),
        CommandSpec::new("tex.close", "Close TeX Source", "TeX", close).params("{}").pure(),
        CommandSpec::new("tex.status", "TeX Status", "TeX", status).params("{}").pure(),
        CommandSpec::new("tex.forwardSearch", "TeX Forward Search", "TeX", forward_search).params(r#"{"line": n, "column"?: n}"#).pure(),
        CommandSpec::new("tex.inverseSearch", "TeX Inverse Search", "TeX", inverse_search).params(r#"{"page": n, "x": n, "y": n}"#).pure(),
    ]
}

/// Open a .tex file as an opaque byte buffer. The bytes are kept verbatim:
/// no parsing, no import into the document, no compile kicked off. Opening
/// resets the item-5 state too: revision restarts, auto-compile is off, and
/// nothing is stale or pending.
#[cfg(not(target_arch = "wasm32"))]
fn open(s: &mut Session, v: &Value) -> CmdResult {
    let path = p::req_str(v, "path")?;
    let bytes = crate::io::read_file_capped(path, crate::tex::MAX_SOURCE_BYTES as u64, "TeX source")
        .map_err(|e| CmdError::Failed(format!("could not read TeX source: {e}")))?;
    let n = bytes.len();
    s.tex = crate::tex::TexState { source: Some(crate::tex::TexSource { path: PathBuf::from(path), bytes }), revision: 1, ..Default::default() };
    Ok(json!({"path": path, "bytes": n}))
}

#[cfg(target_arch = "wasm32")]
fn open(_s: &mut Session, _v: &Value) -> CmdResult {
    Err(CmdError::Disabled("TeX sources are not available on this platform".to_string()))
}

/// Replace the open source with new text, still as opaque bytes (no parsing).
/// The revision bumps; a successful compile's preview is badged stale; when
/// auto-compile is opted in, the edit arms the debounce window instead of
/// compiling on the keystroke.
fn edit(s: &mut Session, v: &Value) -> CmdResult {
    let text = p::req_str(v, "text")?;
    if text.len() > crate::tex::MAX_SOURCE_BYTES {
        return Err(CmdError::Failed(format!("TeX source is over the {n} byte limit", n = crate::tex::MAX_SOURCE_BYTES)));
    }
    {
        let Some(src) = s.tex.source.as_mut() else {
            return Err(CmdError::Params("no TeX source open (run tex.open first)".to_string()));
        };
        src.bytes = text.as_bytes().to_vec();
    }
    s.tex.note_edit(std::time::Instant::now());
    Ok(json!({
        "bytes": s.tex.source.as_ref().map(|x| x.bytes.len()).unwrap_or(0),
        "revision": s.tex.revision,
        "stale": s.tex.preview_stale,
        "pendingRevision": s.tex.pending_revision,
    }))
}

/// Compile the open TeX source exactly once, on explicit request. The source
/// is copied to a scratch dir (the tree beside the file is never written to);
/// the worker runs with a wall clock and is killed past it.
#[cfg(not(target_arch = "wasm32"))]
fn compile(s: &mut Session, v: &Value) -> CmdResult {
    let rev = s.tex.revision;
    run_compile(s, v, rev)
}

#[cfg(target_arch = "wasm32")]
fn compile(_s: &mut Session, _v: &Value) -> CmdResult {
    Err(CmdError::Disabled("TeX workers are not available on this platform".to_string()))
}

/// Stage the current source, run the worker once, and publish the outcome
/// under the generation guard so a superseded compile can never show.
#[cfg(not(target_arch = "wasm32"))]
fn run_compile(s: &mut Session, v: &Value, rev: u64) -> CmdResult {
    let Some(source) = s.tex.source.clone() else {
        return Err(CmdError::Params("no TeX source open (run tex.open first)".to_string()));
    };
    let engine = match p::str(v, "engine") {
        Some(path) => crate::tex::engine_for(path, p::str(v, "kind")).map_err(CmdError::Params)?,
        None => crate::tex::detect_from_path(),
    };
    if engine == crate::tex::TexEngine::Missing {
        return Err(CmdError::Failed("no TeX engine found (pass an explicit engine path)".to_string()));
    }
    let timeout = std::time::Duration::from_millis(p::u64(v, "timeoutMs").unwrap_or(120_000).clamp(50, 3_600_000));
    let rasterizer = p::str(v, "rasterizer").map(PathBuf::from);
    let scratch = crate::tex::new_scratch("wctexside").map_err(CmdError::Failed)?;
    let stem = source.path.file_stem().and_then(|x| x.to_str()).unwrap_or("main");
    let src_file = scratch.join(format!("{stem}.tex"));
    if let Err(e) = crate::io::write_file(&src_file.to_string_lossy(), &source.bytes) {
        crate::io::remove_scratch_dir(&scratch);
        return Err(CmdError::Failed(format!("could not stage TeX source: {e}")));
    }
    let generation = s.tex.begin_compile(rev);
    let mut done = crate::tex::compile_tex(&src_file, &scratch, &engine, rasterizer.as_deref(), timeout);
    crate::io::remove_scratch_dir(&scratch);
    done.revision = rev;
    s.tex.finish_compile(generation, done);
    match &s.tex.last {
        Some(c) => Ok(json!({
            "success": c.success,
            "timedOut": c.timed_out,
            "engine": c.engine,
            "log": c.log,
            "pdfBytes": c.pdf.as_ref().map(Vec::len).unwrap_or(0),
            "previewBytes": c.preview_png.as_ref().map(Vec::len).unwrap_or(0),
            "revision": c.revision,
            "stale": s.tex.preview_stale,
            "diagnostics": s.tex.diagnostics.len(),
        })),
        None => Err(CmdError::Failed("compile result was superseded by a newer edit".to_string())),
    }
}

/// Opt auto-compile in or out for the open document. It is always off unless
/// the user turns it on here; turning it off clears everything pending and
/// retires any in-flight compile so it can never publish.
fn auto_compile(s: &mut Session, v: &Value) -> CmdResult {
    if s.tex.source.is_none() {
        return Err(CmdError::Params("no TeX source open (run tex.open first)".to_string()));
    }
    let Some(enabled) = p::bool(v, "enabled") else {
        return Err(CmdError::Params("`enabled` (boolean) is required".to_string()));
    };
    s.tex.set_auto(enabled, std::time::Instant::now());
    Ok(json!({
        "enabled": s.tex.auto_compile,
        "revision": s.tex.revision,
        "pendingRevision": s.tex.pending_revision,
        "stale": s.tex.preview_stale,
    }))
}

/// Poll the auto-compile scheduler: when auto-compile is on, an edit is
/// pending and the quiet window has elapsed, perform the one compile for the
/// newest revision. Otherwise report without starting any worker.
#[cfg(not(target_arch = "wasm32"))]
fn tick(s: &mut Session, v: &Value) -> CmdResult {
    let Some(rev) = s.tex.due_revision(std::time::Instant::now()) else {
        return Ok(json!({
            "compiled": false,
            "autoCompile": s.tex.auto_compile,
            "revision": s.tex.revision,
            "pendingRevision": s.tex.pending_revision,
            "stale": s.tex.preview_stale,
        }));
    };
    let mut out = run_compile(s, v, rev)?;
    if let Value::Object(ref mut map) = out {
        map.insert("compiled".to_string(), Value::Bool(true));
    }
    Ok(out)
}

#[cfg(target_arch = "wasm32")]
fn tick(_s: &mut Session, _v: &Value) -> CmdResult {
    Err(CmdError::Disabled("TeX workers are not available on this platform".to_string()))
}

/// Structured entries parsed from the last published compile log. Entries
/// from an older source revision (or behind a newer edit) are badged stale
/// and must never be presented as current.
fn diagnostics(s: &mut Session, _v: &Value) -> CmdResult {
    let rev = s.tex.revision;
    let stale = s.tex.preview_stale;
    let entries: Vec<Value> = s
        .tex
        .diagnostics
        .iter()
        .map(|d| {
            json!({
                "file": d.file,
                "line": d.line,
                "severity": d.severity.as_str(),
                "message": d.message,
                "revision": d.revision,
                "stale": stale || d.revision != rev,
            })
        })
        .collect();
    Ok(json!({"open": s.tex.source.is_some(), "revision": rev, "stale": stale, "entries": entries}))
}

/// Forget the open source and everything around it: compile result, revision,
/// auto-compile opt-in, pending work, stale badge and diagnostics.
fn close(s: &mut Session, _v: &Value) -> CmdResult {
    s.tex = Default::default();
    Ok(json!({"open": false}))
}

/// What the source pane holds: open file, byte size, last compile summary,
/// plus the item-5 scheduler state (revision, opt-in, stale badge, pending).
fn status(s: &mut Session, _v: &Value) -> CmdResult {
    let (path, bytes) = match &s.tex.source {
        Some(x) => (Some(x.path.to_string_lossy().into_owned()), Some(x.bytes.len())),
        None => (None, None),
    };
    let result = s.tex.last.as_ref().map(|c| {
        json!({
            "success": c.success,
            "timedOut": c.timed_out,
            "engine": c.engine,
            "logBytes": c.log.len(),
            "pdfBytes": c.pdf.as_ref().map(Vec::len).unwrap_or(0),
            "previewBytes": c.preview_png.as_ref().map(Vec::len).unwrap_or(0),
            "log": c.log,
            "revision": c.revision,
        })
    });
    Ok(json!({
        "open": s.tex.source.is_some(),
        "path": path,
        "bytes": bytes,
        "result": result,
        "revision": s.tex.revision,
        "autoCompile": s.tex.auto_compile,
        "stale": s.tex.preview_stale,
        "pendingRevision": s.tex.pending_revision,
        "compiledRevision": s.tex.compiled_revision,
        "diagnostics": s.tex.diagnostics.len(),
    }))
}

/// Forward search: where does one source line land in the last successful
/// compile. Returns the page plus rectangle in PDF points with a top-left
/// origin (x right, y down), tagged with the compiled revision. The optional
/// column is accepted and ignored because SyncTeX records carry lines. A
/// stale preview, a missing compile, or a line with no mapping is a clean
/// error, never wrong coordinates.
fn forward_search(s: &mut Session, v: &Value) -> CmdResult {
    let Some(line64) = p::u64(v, "line") else {
        return Err(CmdError::Params("`line` (positive integer) is required".to_string()));
    };
    if line64 == 0 || line64 > u32::MAX as u64 {
        return Err(CmdError::Params("`line` must be at least 1".to_string()));
    }
    let line = line64 as u32;
    match crate::tex::forward_jump(&s.tex, line) {
        Ok(hit) => Ok(json!({
            "page": hit.page,
            "x": hit.x,
            "y": hit.y,
            "w": hit.w,
            "h": hit.h,
            "line": hit.line,
            "column": 0,
            "file": hit.file,
            "revision": hit.revision,
            "stale": false,
        })),
        Err(e) => Err(CmdError::Failed(e)),
    }
}

/// Inverse search: which source line owns one preview point. Takes the page
/// plus x and y in PDF points with a top-left origin (the same space forward
/// jumps return, preview pixels map via `pixel = pt * dpi / 72`). Returns the
/// source file plus line and the compiled revision. Stale previews and empty
/// positions are clean errors.
fn inverse_search(s: &mut Session, v: &Value) -> CmdResult {
    let Some(page64) = p::u64(v, "page") else {
        return Err(CmdError::Params("`page` (positive integer) is required".to_string()));
    };
    if page64 == 0 || page64 > u32::MAX as u64 {
        return Err(CmdError::Params("`page` must be at least 1".to_string()));
    }
    let page = page64 as u32;
    let Some(x) = v.get("x").and_then(Value::as_f64).filter(|f| f.is_finite()) else {
        return Err(CmdError::Params("`x` (finite number) is required".to_string()));
    };
    let Some(y) = v.get("y").and_then(Value::as_f64).filter(|f| f.is_finite()) else {
        return Err(CmdError::Params("`y` (finite number) is required".to_string()));
    };
    match crate::tex::inverse_jump(&s.tex, page, x, y) {
        Ok(hit) => Ok(json!({
            "file": hit.file,
            "line": hit.line,
            "column": hit.column,
            "page": hit.page,
            "revision": hit.revision,
            "stale": false,
        })),
        Err(e) => Err(CmdError::Failed(e)),
    }
}
