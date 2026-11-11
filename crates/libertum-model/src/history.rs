//! Undo/redo over transactions, with typing coalescing (PLAN §4.1).

use std::time::{Duration, Instant};

use crate::document::{ChangeSet, Document, ModelError, Section};
use crate::header_footer::{HfKind, HfVariant};
use crate::ops::Operation;
use crate::paragraph::Run;

/// How long a run of typing may pause before it becomes a separate undo step.
pub const TYPING_PAUSE: Duration = Duration::from_secs(1);

/// Whether an entry may be coalesced with the previous one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    /// A single-character insertion or deletion on a collapsed selection.
    Typing,
    /// Anything else (paste, Enter, delete selection, formatting).
    Other,
}

/// One undoable step: the operations to redo and the operations to undo.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryEntry {
    pub forward: Vec<Operation>,
    pub inverse: Vec<Operation>,
    pub kind: EditKind,
}

impl HistoryEntry {
    pub fn new(forward: Vec<Operation>, inverse: Vec<Operation>, kind: EditKind) -> Self {
        Self {
            forward,
            inverse,
            kind,
        }
    }

    pub fn change_set(&self) -> ChangeSet {
        let mut set = ChangeSet::default();
        for op in &self.forward {
            set.merge(ChangeSet::from_operation(op));
        }
        set
    }
}

/// Bounded undo and redo stacks.
#[derive(Debug)]
pub struct History {
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    cap: usize,
    typing: Option<TypingRun>,
}

#[derive(Clone, Copy, Debug)]
struct TypingRun {
    at: Instant,
    last_char_whitespace: bool,
}

impl Default for History {
    fn default() -> Self {
        Self::new(10_000)
    }
}

impl History {
    pub fn new(cap: usize) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            cap: cap.max(1),
            typing: None,
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    /// Records an entry. Consecutive typing on the same block coalesces into one step until a
    /// whitespace character, a pause longer than [`TYPING_PAUSE`], or a different edit breaks
    /// the run.
    pub fn record(&mut self, entry: HistoryEntry, now: Instant) {
        self.redo.clear();
        if entry.forward.is_empty() {
            return;
        }
        if entry.kind == EditKind::Typing {
            if let Some(run) = self.typing {
                let within = now.saturating_duration_since(run.at) <= TYPING_PAUSE;
                if within {
                    if let Some(updated) = self.try_coalesce(run, &entry) {
                        self.typing = Some(updated);
                        return;
                    }
                    if let Some(updated) = self.try_coalesce_section(run, &entry) {
                        self.typing = Some(updated);
                        return;
                    }
                }
            }
        }
        self.typing = typing_run_after(&entry, now);
        self.undo.push(entry);
        if self.undo.len() > self.cap {
            self.undo.remove(0);
        }
    }

    fn try_coalesce(&mut self, run: TypingRun, entry: &HistoryEntry) -> Option<TypingRun> {
        if run.last_char_whitespace {
            return None;
        }
        let last = self.undo.last_mut()?;

        // Insert: delete the previous range, then insert the bigger range.
        if let (
            [
                Operation::InsertText {
                    block: b0,
                    offset: o0,
                    text: t0,
                    format: None,
                },
            ],
            [
                Operation::DeleteRange {
                    block: b1,
                    range: r1,
                },
            ],
            [
                Operation::InsertText {
                    block: b2,
                    offset: o2,
                    text: t2,
                    format: None,
                },
            ],
        ) = (
            last.forward.as_mut_slice(),
            last.inverse.as_mut_slice(),
            entry.forward.as_slice(),
        ) {
            if b0 == b1
                && b0 == b2
                && *o0 + t0.len() == *o2
                && !t2.contains(['\n', '\r'])
                && !t2.chars().any(char::is_whitespace)
            {
                t0.push_str(t2);
                *r1 = *o0..*o0 + t0.len();
                return Some(TypingRun {
                    at: run.at,
                    last_char_whitespace: t2.chars().next_back().is_some_and(char::is_whitespace),
                });
            }
            return None;
        }

        // Delete: extend the previous deletion range and its inverse insertion.
        if let (
            [
                Operation::DeleteRange {
                    block: b0,
                    range: r0,
                },
            ],
            [
                Operation::InsertText {
                    block: b1,
                    offset: o1,
                    text: t1,
                    format: Some(f1),
                },
            ],
            [
                Operation::DeleteRange {
                    block: b2,
                    range: r2,
                },
            ],
            [
                Operation::InsertText {
                    text: t2,
                    format: Some(f2),
                    ..
                },
            ],
        ) = (
            last.forward.as_mut_slice(),
            last.inverse.as_mut_slice(),
            entry.forward.as_slice(),
            entry.inverse.as_slice(),
        ) {
            if b0 == b1 && b0 == b2 {
                if t2.chars().any(char::is_whitespace) || t2.contains(['\n', '\r']) {
                    return None;
                }
                let backspace = r2.end == r0.start;
                let forward = r2.start == r0.end;
                // The undo re-inserts the merged text with the formatting of both parts.
                let shift = |runs: &[Run], by: usize| -> Vec<Run> {
                    runs.iter()
                        .map(|r| Run {
                            range: r.range.start + by..r.range.end + by,
                            format: r.format.clone(),
                        })
                        .collect()
                };
                if backspace {
                    *r0 = r2.start..r0.end;
                    *o1 = r0.start;
                    let mut runs = f2.clone();
                    runs.extend(shift(f1, t2.len()));
                    *f1 = runs;
                    t1.insert_str(0, t2);
                } else if forward {
                    *r0 = r0.start..r2.end;
                    *o1 = r0.start;
                    let extra = shift(f2, t1.len());
                    f1.extend(extra);
                    t1.push_str(t2);
                } else {
                    return None;
                }
                return Some(TypingRun {
                    at: run.at,
                    last_char_whitespace: false,
                });
            }
        }
        None
    }

    /// Consecutive typed edits of one section (a header being typed in the inspector) fold
    /// into one undo step: the latest forward operation replaces the last one and the first
    /// inverse is kept, so undo returns to the text before the run. Only a run that changes
    /// the *same* header or footer text field coalesces, so typing two different variants in
    /// quick succession is two undo steps and redo restores both.
    fn try_coalesce_section(&mut self, run: TypingRun, entry: &HistoryEntry) -> Option<TypingRun> {
        let last = self.undo.last_mut()?;
        let (
            [
                Operation::SetSection {
                    index: a,
                    section: mid,
                },
            ],
            [
                Operation::SetSection {
                    index: old_index,
                    section: old,
                },
            ],
            [
                Operation::SetSection {
                    index: b,
                    section: new,
                },
            ],
        ) = (
            last.forward.as_slice(),
            last.inverse.as_slice(),
            entry.forward.as_slice(),
        )
        else {
            return None;
        };
        if a != b || a != old_index {
            return None;
        }
        let slot = sole_header_text_change(old, mid)?;
        if sole_header_text_change(mid, new) != Some(slot) {
            return None;
        }
        last.forward = entry.forward.clone();
        Some(TypingRun {
            at: run.at,
            last_char_whitespace: false,
        })
    }

    /// Undoes the last step, applying its inverse operations in reverse order.
    pub fn undo(&mut self, doc: &mut Document) -> Result<Option<ChangeSet>, ModelError> {
        self.typing = None;
        let Some(entry) = self.undo.pop() else {
            return Ok(None);
        };
        for op in entry.inverse.iter().rev() {
            doc.apply(op.clone())?;
        }
        let set = entry.change_set();
        self.redo.push(entry);
        Ok(Some(set))
    }

    /// Redoes the last undone step, applying its forward operations in order.
    pub fn redo(&mut self, doc: &mut Document) -> Result<Option<ChangeSet>, ModelError> {
        self.typing = None;
        let Some(entry) = self.redo.pop() else {
            return Ok(None);
        };
        for op in &entry.forward {
            doc.apply(op.clone())?;
        }
        let set = entry.change_set();
        self.undo.push(entry);
        Ok(Some(set))
    }
}

/// The header or footer text field that differs between two sections, when that is the only
/// difference. `None` for any other change, so unrelated section edits never coalesce.
///
/// Adding or removing the variant (typing the first character of a header creates it, an
/// empty text field) counts as a text change of that field, so that first keystroke coalesces
/// with the rest of the run and one undo removes the whole header.
fn sole_header_text_change(a: &Section, b: &Section) -> Option<(HfKind, HfVariant)> {
    if a == b {
        return None;
    }
    for kind in [HfKind::Header, HfKind::Footer] {
        for variant in HfVariant::ALL {
            let before = a.header_footer(kind).get(variant);
            let after = b.header_footer(kind).get(variant);
            let mut probe = a.clone();
            match (before, after) {
                (None, None) => continue,
                (Some(before_hf), Some(after_hf)) => {
                    if before_hf == after_hf {
                        continue;
                    }
                    if let Some(hf) = probe.header_footer_mut(kind).get_mut(variant) {
                        *hf = after_hf.clone();
                    }
                }
                _ => {
                    *probe.header_footer_mut(kind).get_mut(variant) = after.cloned();
                }
            }
            if probe == *b {
                return Some((kind, variant));
            }
        }
    }
    None
}

fn typing_run_after(entry: &HistoryEntry, now: Instant) -> Option<TypingRun> {
    if entry.kind != EditKind::Typing {
        return None;
    }
    let last_char_whitespace = matches!(
        entry.forward.as_slice(),
        [Operation::InsertText { text, .. }] if text.chars().next_back().is_some_and(char::is_whitespace)
    );
    Some(TypingRun {
        at: now,
        last_char_whitespace,
    })
}
