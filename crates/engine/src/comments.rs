// Modified by the PdfCraft Overlay fork (sefr-14/pdfcraft); changes are recorded in its Git history.
//! The comment list and its summary.

/// The comment list, read from the object graph (as `inspect` lists comments).
pub(crate) fn comment_list(doc: &pdfcraft_cos::Document) -> Vec<pdfcraft_render::Annotation> {
    let mut list: Vec<pdfcraft_render::Annotation> = pdfcraft_annot::summaries(doc)
        .into_iter()
        .map(|s| pdfcraft_render::Annotation {
            page: s.page,
            subtype: s.subtype,
            author: s.author,
            contents: s.contents,
            modified: s.modified.map(|m| pdfcraft_render::pretty_date(&m)),
            name: s.name,
            in_reply_to: s.in_reply_to,
            parent: s.parent,
            grouped: s.grouped,
            rect: s.rect,
            color: s.color,
            index: s.index,
            object: s.object,
            state: s.state,
            quads: s.quads,
            locked: s.locked,
            intent: s.intent,
            fill_sign: s.fill_sign,
            no_rotate: s.no_rotate,
            points: s.points,
        })
        .collect();
    pdfcraft_render::link_replies(&mut list);
    list
}

/// Order of Summarize Comments.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SummarySort {
    #[default]
    Page,
    Author,
    Date,
    Type,
}

impl SummarySort {
    pub const ALL: [SummarySort; 4] = [SummarySort::Page, SummarySort::Author, SummarySort::Date, SummarySort::Type];
    pub fn name(self) -> &'static str {
        match self {
            SummarySort::Page => "Page",
            SummarySort::Author => "Author",
            SummarySort::Date => "Date",
            SummarySort::Type => "Type",
        }
    }
    pub fn parse(s: &str) -> Option<SummarySort> {
        Self::ALL.into_iter().find(|k| k.name().eq_ignore_ascii_case(s))
    }
}

/// Acrobat's name for a comment type, as comment lists show it.
pub fn comment_type_name(subtype: &str) -> &str {
    match subtype {
        "Text" => "Sticky Note",
        "FreeText" => "Text Box",
        "Highlight" => "Highlight",
        "Underline" => "Underline",
        "StrikeOut" => "Strikethrough",
        "Squiggly" => "Squiggly",
        "Square" => "Rectangle",
        "Circle" => "Oval",
        "Line" => "Line",
        "Ink" => "Pencil",
        "Polygon" => "Polygon",
        "PolyLine" => "Polygonal Line",
        "Stamp" => "Stamp",
        "Caret" => "Inserted Text",
        "FileAttachment" => "File Attachment",
        "Sound" => "Sound",
        "Redact" => "Redaction",
        other => other,
    }
}

/// [`comment_type_name`], telling callouts, clouds and typewriter text apart by `/IT`.
pub fn comment_kind(a: &pdfcraft_render::Annotation) -> &str {
    match a.intent.as_deref() {
        Some("FreehandHighlight") if a.subtype == "Ink" => "Freehand Highlight",
        Some("FreeTextCallout") => "Callout",
        Some("PolygonCloud") => "Cloud",
        Some("FreeTextTypeWriter") => "Typewriter",
        _ => comment_type_name(&a.subtype),
    }
}

/// The text of a comment summary: one block per comment (replies indented under it), with a
/// "Page N" heading when sorted by page. Checkmarks and status replies are left out, as are
/// pop-ups; the number is the comment's position on its page.
pub fn comment_summary(name: &str, all: &[pdfcraft_render::Annotation], sort: SummarySort) -> String {
    use std::fmt::Write;
    let top: Vec<&pdfcraft_render::Annotation> = all.iter().filter(|a| a.is_comment()).collect();
    // Each reply under the annotation its `/IRT` points at, in document order, gathered once.
    type Children<'a> = std::collections::HashMap<(usize, usize), Vec<&'a pdfcraft_render::Annotation>>;
    let mut children = Children::new();
    for r in all.iter().filter(|r| r.state.is_none()) {
        if let Some(parent) = r.parent {
            children.entry(parent).or_default().push(r);
        }
    }
    // Write a comment's replies, indenting each level. A reply to a reply keeps its `/IRT` on the
    // direct parent, so following one level would leave the deeper replies out of the summary.
    fn write_replies(out: &mut String, children: &Children<'_>, parent: &pdfcraft_render::Annotation, depth: usize) {
        use std::fmt::Write;
        if depth > 64 {
            return;
        }
        for r in children.get(&(parent.page, parent.index)).into_iter().flatten() {
            let indent = "    ".repeat(depth + 1);
            let _ =
                writeln!(out, "{indent}Author: {}  Subject: Reply  Date: {}", r.author.as_deref().unwrap_or(""), r.modified.as_deref().unwrap_or(""));
            for line in r.contents.as_deref().unwrap_or("").lines() {
                let _ = writeln!(out, "{indent}{line}");
            }
            write_replies(out, children, r, depth + 1);
        }
    }
    let mut numbered: Vec<(usize, &pdfcraft_render::Annotation)> = Vec::new();
    let mut last = usize::MAX;
    let mut n = 0;
    for a in &top {
        if a.page != last {
            last = a.page;
            n = 0;
        }
        n += 1;
        numbered.push((n, a));
    }
    match sort {
        SummarySort::Page => {}
        SummarySort::Author => numbered.sort_by(|x, y| x.1.author.cmp(&y.1.author)),
        SummarySort::Date => numbered.sort_by(|x, y| x.1.modified.cmp(&y.1.modified)),
        SummarySort::Type => numbered.sort_by(|x, y| comment_kind(x.1).cmp(comment_kind(y.1))),
    }
    let mut out = format!("Summary of Comments on {name}\n\n");
    if numbered.is_empty() {
        out.push_str("This document has no comments.\n");
        return out;
    }
    let mut heading = usize::MAX;
    for (n, a) in numbered {
        if sort == SummarySort::Page && a.page != heading {
            heading = a.page;
            let _ = writeln!(out, "Page: {}", a.page + 1);
        }
        let _ = write!(out, "Number: {n}  Author: {}  Subject: {}", a.author.as_deref().unwrap_or(""), comment_kind(a));
        if sort != SummarySort::Page {
            let _ = write!(out, "  Page: {}", a.page + 1);
        }
        let _ = writeln!(out, "  Date: {}", a.modified.as_deref().unwrap_or(""));
        if let Some(c) = a.contents.as_deref().filter(|c| !c.is_empty()) {
            let _ = writeln!(out, "{c}");
        }
        write_replies(&mut out, &children, a, 0);
        out.push('\n');
    }
    out
}

/// This fork (COMMENTS.1): what deleting the comments `targets` removes, worked out on `doc`
/// without changing it.
///
/// A target is a comment of its own (a card in the Comments panel) or a reply, as the panel tells
/// them apart: by the comment list it is drawn from ([`comment_list`]). Each must match its entry
/// there exactly, page, position and object, or the document changed and nothing is deleted.
/// The comments go with their pop-ups and their replies on their page
/// ([`pdfcraft_annot::deletion_set`]). A reply among the targets that goes with one of them is
/// counted once, as a reply; a reply that wouldn't (its comment isn't a target, or it is on
/// another page) refuses the whole deletion: several comments are deleted at once, replies one
/// at a time. The counts cover every entry removed except pop-ups, each once: `comments` the
/// comments targeted, `replies` all the rest.
pub(crate) fn plan_comment_deletion(
    doc: &pdfcraft_cos::Document,
    targets: &[pdfcraft_annot::DeleteTarget],
) -> Result<(CommentDeletion, pdfcraft_annot::DeletionSet), crate::EditError> {
    use std::collections::BTreeSet;
    let list = comment_list(doc);
    let invalid = |m: String| crate::EditError::Comment(pdfcraft_annot::AnnotError::Invalid(m));
    let (mut roots, mut replies) = (BTreeSet::new(), BTreeSet::new());
    for t in targets {
        let entry = list.iter().find(|a| a.page == t.page && a.index == t.index && a.object == t.object).ok_or_else(|| {
            invalid(format!("comment {} on page {} changed; take the comments again", t.index.saturating_add(1), t.page.saturating_add(1)))
        })?;
        if entry.is_comment() {
            roots.insert(*t);
        } else {
            replies.insert(*t);
        }
    }
    if roots.is_empty() {
        return Err(invalid("several comments are deleted together; delete a reply from its own menu".into()));
    }
    let roots: Vec<_> = roots.into_iter().collect();
    let set = pdfcraft_annot::deletion_set(doc, &roots)?;
    let gone: BTreeSet<(usize, usize)> = set.removed.iter().map(|&(page, index, _)| (page, index)).collect();
    if let Some(r) = replies.iter().find(|r| !gone.contains(&(r.page, r.index))) {
        return Err(invalid(format!(
            "comment {} on page {} is a reply that doesn't go with the comments chosen: delete it from its own menu",
            r.index.saturating_add(1),
            r.page.saturating_add(1)
        )));
    }
    let shown = set.removed.iter().filter(|&&(_, _, popup)| !popup).count();
    let comments = roots.len();
    Ok((CommentDeletion { replies: shown.saturating_sub(comments), comments, roots }, set))
}

/// This fork (COMMENTS.1): the comments a deletion takes, and how many comments and replies go.
#[derive(Clone, Debug, PartialEq)]
pub struct CommentDeletion {
    /// The targets that are comments of their own: what is deleted (replies go with them).
    pub roots: Vec<pdfcraft_annot::DeleteTarget>,
    pub comments: usize,
    pub replies: usize,
}
