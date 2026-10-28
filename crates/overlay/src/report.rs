//! The generation report (step 4 PRD, Report): what the overlay did with each source document
//! and page, and every warning, each one the output is demonstrably correct with.
//!
//! [`LayeredOutput`](crate::LayeredOutput) builds it page by page; an error leaves it as it was,
//! like the document. Its text never says the output is signed or that redactions were applied:
//! the overlay is neither.

use std::fmt::Write as _;

use crate::Side;
use crate::annotations::{AppearanceSource, Outcome, SourceWarning};
use crate::layers::{RegistrationNote, Revision};
use crate::oc::eval::Note;
use crate::oc::prepare::PageReport;

/// One source document as the overlay used it.
#[derive(Clone, Debug, PartialEq)]
pub struct DocumentReport {
    pub side: Side,
    pub role: Revision,
    /// Signed signature fields and redaction marks not applied.
    pub warnings: Vec<SourceWarning>,
    /// What the evaluator noted about the document's layers, as text.
    pub notes: Vec<String>,
}

/// One output page: its registration and what each source page contributed.
#[derive(Clone, Debug, PartialEq)]
pub struct PageEntry {
    pub registration: RegistrationNote,
    pub a: Option<PageReport>,
    pub b: Option<PageReport>,
}

/// The report of a layered overlay.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OverlayReport {
    pub documents: Vec<DocumentReport>,
    pub pages: Vec<PageEntry>,
}

/// A layer note as the report states it.
pub(crate) fn note_text(note: &Note, name: &dyn Fn(pdfcraft_cos::ObjRef) -> String) -> String {
    match note {
        Note::PrintExportNotApplied { groups } => format!(
            "automatic Print and Export layer states aren't applied, as they change what the document prints or exports, not what it shows (layers {})",
            groups.iter().map(|g| name(*g)).collect::<Vec<_>>().join(", ")
        ),
        Note::AutoStateNoChange { group } => {
            format!("an automatic View state of layer {} leaves it as the default configuration has it", name(*group))
        }
        Note::ExpressionIgnoredByDisplay { membership, with_policy, display_differs } => {
            let object = membership.map(|m| format!(" (object {} {})", m.num, m.generation)).unwrap_or_default();
            if *with_policy {
                format!(
                    "a layer membership{object} has a visibility expression and a layer list that give the same result; pdfcraft's own display reads only the list"
                )
            } else {
                format!(
                    "a layer visibility expression{object} decides what shows (ISO 32000-1 §8.11.2.2 recommends expressions over layer lists in an informative note, not a requirement); pdfcraft's own display ignores it{}",
                    match display_differs {
                        Some(true) => " and shows that content differently",
                        Some(false) => ", with the same result here",
                        None => "",
                    }
                )
            }
        }
        Note::MembershipNoEffect { membership } => format!(
            "a layer membership{} names no layer, so it has no effect on visibility and its content is shown (ISO 32000-1 Table 99)",
            membership.map(|m| format!(" (object {} {})", m.num, m.generation)).unwrap_or_default()
        ),
        Note::ConfigurationIntent { intents } => format!(
            "the default layer configuration's intent is {}, but the standard says it shall be View (ISO 32000-1 Table 101); layers are matched with it as written, and any it leaves out are reported as unresolved",
            intents.join(", ")
        ),
        Note::UndefinedMarker => {
            "some content is marked as a layer with a name the page doesn't define; that isn't a layer (ISO 32000-1 §8.11.3.2), so the content is shown, as PDF Overlay and other viewers show it"
                .into()
        }
        Note::BaseStateOff => {
            "the default layer configuration's base state is OFF, which the standard doesn't allow; it is followed as viewers do: layers not listed as on are hidden"
                .into()
        }
        Note::UndeclaredShown { groups, configured } => {
            let names: Vec<String> = groups.iter().take(UNDECLARED_NAMES).map(|g| name(*g)).collect();
            let more = groups.len().saturating_sub(UNDECLARED_NAMES);
            let more = if more > 0 { format!(" and {more} more") } else { String::new() };
            if *configured {
                format!(
                    "{} layer(s) aren't declared in the document's layer list, which the standard requires; they are shown, as PDF Overlay and other viewers show them (layers {}{more})",
                    groups.len(),
                    names.join(", ")
                )
            } else {
                format!(
                    "the document has no layer configuration, so its {} layer(s) have no effect on what shows (ISO 32000-1 §8.11.4.2); they are shown, as PDF Overlay and other viewers show them (layers {}{more})",
                    groups.len(),
                    names.join(", ")
                )
            }
        }
    }
}

/// Most layer names the `UndeclaredShown` note lists; it counts the rest.
const UNDECLARED_NAMES: usize = 10;

impl OverlayReport {
    /// Whether the overlay was made without any warning.
    pub fn clean(&self) -> bool {
        self.warnings().is_empty()
    }

    /// Every warning, each saying which kind it is.
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        for d in &self.documents {
            let side = d.side.letter();
            out.extend(d.warnings.iter().map(|w| w.text(side)));
            out.extend(d.notes.iter().map(|n| format!("Document {side}: {n}.")));
        }
        out
    }

    /// The whole report as text.
    pub fn text(&self) -> String {
        let mut s = String::new();
        let warnings = self.warnings();
        let _ = writeln!(s, "Result: {}", if warnings.is_empty() { "clean".to_string() } else { format!("with {} warning(s)", warnings.len()) });
        for w in &warnings {
            let _ = writeln!(s, "  Warning: {w}");
        }
        for d in &self.documents {
            let _ = writeln!(s, "Document {}: {}", d.side.letter(), d.role.name());
        }
        for (i, p) in self.pages.iter().enumerate() {
            let _ = writeln!(s, "Page {}: {}", i.saturating_add(1), p.registration.text());
            for (side, report) in [('A', &p.a), ('B', &p.b)] {
                let Some(r) = report else {
                    let _ = writeln!(s, "  Document {side}: blank");
                    continue;
                };
                let _ = writeln!(s, "  Document {side}, page {}:", r.page.saturating_add(1));
                if r.optional {
                    let layers: Vec<String> = r
                        .layers
                        .iter()
                        .map(|(g, n, v)| {
                            let name = n.clone().unwrap_or_else(|| format!("object {} {}", g.num, g.generation));
                            format!("{name} {}", if *v == Some(true) { "shown" } else { "hidden" })
                        })
                        .collect();
                    let _ = writeln!(
                        s,
                        "    Layers: {}; {} hidden block(s), {} hidden XObject(s), {} marker pair(s) removed, {} state operator(s) and {} clip(s) kept from hidden blocks",
                        layers.join(", "),
                        r.hidden_blocks,
                        r.hidden_xobjects,
                        r.markers_removed,
                        r.kept_state,
                        r.kept_clips
                    );
                }
                let count = |f: &dyn Fn(&Outcome) -> bool| r.annotations.iter().filter(|a| f(&a.outcome)).count();
                if !r.annotations.is_empty() {
                    let _ = writeln!(
                        s,
                        "    Annotations: {} drawn ({} with appearances generated, {} form fields redrawn from their values)",
                        count(&|o| matches!(o, Outcome::Drawn(_))),
                        count(&|o| *o == Outcome::Drawn(AppearanceSource::Generated)),
                        count(&|o| *o == Outcome::Drawn(AppearanceSource::Regenerated))
                    );
                }
                for a in &r.annotations {
                    let what =
                        format!("{} annotation {}", if a.subtype.is_empty() { "an untyped" } else { a.subtype.as_str() }, a.index.saturating_add(1));
                    match &a.outcome {
                        Outcome::Drawn(_) => {}
                        Outcome::Excluded(e) => {
                            let _ = writeln!(s, "    Not drawn: {what}: {}", e.describe());
                        }
                        Outcome::Omitted(rule) => {
                            let _ = writeln!(s, "    Omitted by your decision: {what}{}", rule.map(|r| format!(" ({r})")).unwrap_or_default());
                        }
                        Outcome::Unresolved(rule) => {
                            let _ = writeln!(s, "    Unresolved: {what}: {rule}");
                        }
                    }
                }
            }
        }
        s
    }
}
