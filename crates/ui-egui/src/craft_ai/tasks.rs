//! What Craft AI does inside WordCraft. Craft AI is one assistant across the ArtCraft family
//! — the model, the runtime and the shared folder belong to the family — but its *work* is the
//! app's own: the system prompt tells the model which app it is inside and what that app does,
//! and every ready-made task is framed for that app's world, so the answer that comes back is
//! about headings and styles and margins, not generic advice.
//!
//! Each app of the family carries its own `tasks.rs` — VectorCraft's asks about paths and
//! Pathfinder steps, SoundCraft's about tracks and buses, and so on — same model, same shared
//! folder, different craft. The labels and hints are catalog keys: the panel shows them
//! translated.

/// The app Craft AI is embedded in, as the shared folder's records name it.
pub const APP: &str = "WordCraft";

/// One ready-made task of the Assistant tab.
pub struct Task {
    /// The label the picker shows (a catalog key).
    pub label: &'static str,
    /// The input field's hint (a catalog key).
    pub hint: &'static str,
}

/// The tasks, in the picker's order.
pub const TASKS: &[Task] = &[
    Task { label: "Ask anything", hint: "Ask in any language" },
    Task { label: "Polish this text", hint: "Paste the text to tighten and correct" },
    Task { label: "Summarize", hint: "Paste the text, say how short" },
    Task { label: "Check grammar and style", hint: "Paste the text to check" },
    Task { label: "Structure a document", hint: "What the document must cover" },
    Task { label: "Change the tone", hint: "Paste the text, name the tone" },
];

/// Tell the model where it is and how to answer: briefly, practically, in the language the user
/// wrote in, about this app's own world — the words of its interface, the files it opens and
/// writes. The last line is the honesty clause: a local model can be wrong, and it is told to
/// say so rather than invent.
pub fn system_prompt() -> String {
    "You are Craft AI, the assistant built into WordCraft, a desktop word processor. \
     WordCraft's world, in its own words: documents of pages, with page setup and margins; \
     headings and styles, and a table of contents; paragraphs, lists and tables; find and \
     replace; .docx and .pdf files opened and written. Answer in the language the user wrote \
     in, Arabic included. Be brief and practical: name the ribbon tab or the pane, give the \
     steps as a short list, say what to watch out for, and when a rewrite is asked for, hand \
     back a clean block to copy — the rewritten text only. When a question is not about \
     WordCraft or writing, answer it anyway, briefly. Never invent ribbon tabs, commands or \
     pane names: if you are not sure WordCraft has a feature, say so plainly."
        .to_string()
}

/// Frame the user's input for the task they picked.
pub fn user_prompt(task: usize, input: &str) -> String {
    let input = input.trim();
    match TASKS[task.min(TASKS.len() - 1)].label {
        "Polish this text" => format!(
            "In WordCraft, polish this text for clarity and correctness, keeping the writer's \
             voice, and hand back the rewritten text only: {input}"
        ),
        "Summarize" => format!("Summarize this text, as short as the request asks, and hand back the summary only: {input}"),
        "Check grammar and style" => format!(
            "Check the grammar and style of this text: list each problem and its fix briefly, \
             then hand back the corrected text: {input}"
        ),
        "Structure a document" => format!(
            "Plan the structure of a document that covers this: its headings and the order of \
             its parts, as a short outline: {input}"
        ),
        "Change the tone" => format!(
            "Rewrite this text in the tone the request names, keeping its meaning, and hand \
             back the rewritten text only: {input}"
        ),
        _ => input.to_string(),
    }
}
