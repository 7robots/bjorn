//! Handing a note to `$EDITOR`: the temp file, the command, the round trip.
//! The editor itself runs in a pty inside the reader pane; see `pty`.

use std::path::PathBuf;

use crate::bear::{Note, NoteContent};
use crate::export::safe_filename;

/// One editing session: the note as it was read, and where it was written for the editor.
#[derive(Debug, Clone)]
pub struct EditorJob {
    pub note: Note,
    pub before: NoteContent,
    pub tmp: PathBuf,
    pub command: Vec<String>,
}

/// Write the note to `<tmpdir>/bjorn-XXXX/<title>.md` and build the command.
pub fn prepare(note: &Note, before: &NoteContent, editor: &str) -> std::io::Result<EditorJob> {
    let dir = tempfile::Builder::new().prefix("bjorn-").tempdir()?.keep();
    let tmp = dir.join(format!("{}.md", safe_filename(&note.title)));
    std::fs::write(&tmp, &before.content)?;
    let mut command = shlex::split(editor)
        .unwrap_or_else(|| editor.split_whitespace().map(str::to_string).collect());
    command.push(tmp.to_string_lossy().into_owned());
    Ok(EditorJob {
        note: note.clone(),
        before: before.clone(),
        tmp,
        command,
    })
}

/// The edited text, or the error reading it.
pub fn result(job: &EditorJob) -> std::io::Result<String> {
    std::fs::read_to_string(&job.tmp)
}

/// Remove the temp file and its directory; nothing to report if that fails.
pub fn cleanup(job: &EditorJob) {
    let _ = std::fs::remove_file(&job.tmp);
    if let Some(dir) = job.tmp.parent() {
        let _ = std::fs::remove_dir(dir);
    }
}
